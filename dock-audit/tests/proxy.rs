//! 出口代理端到端：真的 curl 走代理，打本地起的 HTTP / HTTPS 服务。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dock_audit::ca::Authority;
use dock_audit::proxy::{Options, Proxy};
use dock_audit::record::Flow;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn curl_or_skip() -> bool {
    let ok = std::process::Command::new("curl")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skip: curl not installed");
    }
    ok
}

/// 一个测试用的根证书，和它签的 `localhost` 服务器证书。
struct TestPki {
    ca_der: Vec<u8>,
    ca_pem: String,
    server: rustls::ServerConfig,
}

fn server_config(cert_der: Vec<u8>, key: &KeyPair) -> rustls::ServerConfig {
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert_der)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
        .unwrap()
}

fn test_pki() -> TestPki {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Test Upstream CA");
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca_params, ca_key);
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    TestPki {
        ca_der: ca_cert.der().to_vec(),
        ca_pem: ca_cert.pem(),
        server: server_config(cert.der().to_vec(), &key),
    }
}

fn self_signed() -> rustls::ServerConfig {
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    server_config(cert.der().to_vec(), &key)
}

/// 读一个请求（头 + Content-Length 的正文），回 `hello <path> <正文>`。
async fn answer<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(mut s: S) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let Ok(n) = s.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let len: usize = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let Ok(n) = s.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
    let body = format!("hello {path} {}", String::from_utf8_lossy(&buf[head_end..]));
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = s.write_all(resp.as_bytes()).await;
    let _ = s.shutdown().await;
}

async fn serve(tls: Option<rustls::ServerConfig>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = tls.map(|c| tokio_rustls::TlsAcceptor::from(Arc::new(c)));
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                match acceptor {
                    Some(a) => {
                        if let Ok(tls) = a.accept(s).await {
                            answer(tls).await;
                        }
                    }
                    None => answer(s).await,
                }
            });
        }
    });
    port
}

struct Rig {
    _dir: tempfile::TempDir,
    ca_path: PathBuf,
    port: u16,
    flows: Arc<Mutex<Vec<Flow>>>,
    learned: Arc<Mutex<Vec<String>>>,
    _listener: dock_audit::proxy::Listener,
}

async fn rig(extra_roots: Vec<Vec<u8>>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let ca = Arc::new(Authority::load_or_create(dir.path()).unwrap());
    let flows: Arc<Mutex<Vec<Flow>>> = Arc::default();
    let learned: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink_flows = flows.clone();
    let sink_learned = learned.clone();
    let proxy = Proxy::new(
        Some(ca.clone()),
        Options {
            extra_roots,
            ..Options::default()
        },
        Arc::new(move |f| sink_flows.lock().unwrap().push(f)),
        Vec::new(),
        Some(Arc::new(move |host: &str, _why: &str| {
            sink_learned.lock().unwrap().push(host.to_string())
        })),
    );
    let listener = proxy.listen("call-1").await.unwrap();
    Rig {
        ca_path: ca.cert_path(),
        port: listener.port,
        _dir: dir,
        flows,
        learned,
        _listener: listener,
    }
}

async fn curl(port: u16, args: &[&str]) -> (bool, String) {
    let mut cmd = std::process::Command::new("curl");
    cmd.args([
        "-sS",
        "--max-time",
        "10",
        "-x",
        &format!("http://127.0.0.1:{port}"),
    ])
    .args(args);
    for k in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        cmd.env_remove(k);
    }
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// 记录是在正文读完、记录的最后一份引用放掉时交出来的：等一会儿。
async fn wait_flows(flows: &Arc<Mutex<Vec<Flow>>>, n: usize) -> Vec<Flow> {
    for _ in 0..100 {
        if flows.lock().unwrap().len() >= n {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    flows.lock().unwrap().clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_http_is_recorded_with_bodies() {
    if !curl_or_skip() {
        return;
    }
    let server = serve(None).await;
    let rig = rig(Vec::new()).await;
    let (ok, out) = curl(
        rig.port,
        &[
            "-d",
            "ping=1",
            &format!("http://127.0.0.1:{server}/form?q=2"),
        ],
    )
    .await;
    assert!(ok, "{out}");
    assert_eq!(out, "hello /form?q=2 ping=1");
    let flows = wait_flows(&rig.flows, 1).await;
    let f = &flows[0];
    assert_eq!(
        (f.kind, f.scheme, f.tag.as_str()),
        ("http", "http", "call-1")
    );
    assert_eq!(f.method.as_deref(), Some("POST"));
    assert_eq!(f.path.as_deref(), Some("/form?q=2"));
    assert_eq!(f.status, Some(200));
    assert_eq!(f.request.as_ref().unwrap().body.sample, b"ping=1");
    assert_eq!(
        f.response.as_ref().unwrap().body.sample,
        b"hello /form?q=2 ping=1"
    );
}

/// 客户端信 Dock 根证书、上游证书过得了校验：HTTPS 在代理处解密，请求和响应都看得到。
#[tokio::test(flavor = "multi_thread")]
async fn https_is_decrypted_when_the_client_trusts_dock() {
    if !curl_or_skip() {
        return;
    }
    let pki = test_pki();
    let server = serve(Some(pki.server)).await;
    let rig = rig(vec![pki.ca_der]).await;
    let ca = rig.ca_path.display().to_string();
    let (ok, out) = curl(
        rig.port,
        &[
            "--cacert",
            &ca,
            "-H",
            "Authorization: Bearer sk-secret",
            "-d",
            "x",
            &format!("https://localhost:{server}/api"),
        ],
    )
    .await;
    assert!(ok, "{out}");
    assert_eq!(out, "hello /api x");
    let flows = wait_flows(&rig.flows, 1).await;
    let f = &flows[0];
    assert!(f.decrypted, "{f:?}");
    assert_eq!(
        (f.kind, f.scheme, f.host.as_str()),
        ("http", "https", "localhost")
    );
    assert_eq!(f.path.as_deref(), Some("/api"));
    assert_eq!(f.response.as_ref().unwrap().body.sample, b"hello /api x");
    let auth = &f
        .request
        .as_ref()
        .unwrap()
        .headers
        .iter()
        .find(|(k, _)| k == "authorization")
        .unwrap()
        .1;
    assert!(auth.starts_with("[redacted"), "凭据要遮住：{auth}");
}

/// 上游证书过不了校验（自签的开发服务器，客户端加了 -k）：原样转发，交给客户端自己判断，
/// 不让它因为代理失败。
#[tokio::test(flavor = "multi_thread")]
async fn untrusted_upstream_is_passed_through() {
    if !curl_or_skip() {
        return;
    }
    let server = serve(Some(self_signed())).await;
    let rig = rig(Vec::new()).await;
    let (ok, out) = curl(
        rig.port,
        &["-k", &format!("https://localhost:{server}/dev")],
    )
    .await;
    assert!(ok, "{out}");
    assert_eq!(out, "hello /dev ");
    let flows = wait_flows(&rig.flows, 1).await;
    let f = &flows[0];
    assert_eq!((f.kind, f.decrypted), ("tunnel", false), "{f:?}");
    assert!(f.note.as_deref().unwrap_or("").contains("上游"), "{f:?}");
    assert!(f.bytes_down > 0);
}

/// 客户端不信 Dock 根证书：这一次失败，主机记进直通名单；之后原样转发，客户端照常用自己
/// 信的证书验上游。
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_rejects_dock_is_passed_through_afterwards() {
    if !curl_or_skip() {
        return;
    }
    let pki = test_pki();
    let upstream_ca = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(upstream_ca.path(), &pki.ca_pem).unwrap();
    let server = serve(Some(pki.server)).await;
    let rig = rig(vec![pki.ca_der]).await;
    let only_upstream = upstream_ca.path().display().to_string();
    let url = format!("https://localhost:{server}/again");
    let (ok, _) = curl(rig.port, &["--cacert", &only_upstream, &url]).await;
    assert!(!ok, "信不过 Dock 根证书的客户端第一次会失败");
    assert_eq!(rig.learned.lock().unwrap().as_slice(), ["localhost"]);
    let (ok, out) = curl(rig.port, &["--cacert", &only_upstream, &url]).await;
    assert!(ok, "第二次原样转发：{out}");
    assert_eq!(out, "hello /again ");
    let flows = wait_flows(&rig.flows, 2).await;
    let last = flows.last().unwrap();
    assert_eq!((last.kind, last.decrypted), ("tunnel", false), "{last:?}");
    assert!(
        last.note.as_deref().unwrap_or("").contains("拒绝过"),
        "{last:?}"
    );
}
