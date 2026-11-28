//! 出口代理端到端：真的 curl 走代理，打本地起的 HTTP / HTTPS 服务。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dock_sandbox::audit::ca::Authority;
use dock_sandbox::audit::proxy::{Options, Proxy};
use dock_sandbox::audit::record::Flow;
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
    proxy: Arc<Proxy>,
    _listener: dock_sandbox::audit::proxy::Listener,
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
        proxy,
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
    wait_flows_for(flows, n, Duration::from_secs(2)).await
}

async fn wait_flows_for(flows: &Arc<Mutex<Vec<Flow>>>, n: usize, limit: Duration) -> Vec<Flow> {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
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
    let (ok, first_out) = curl(rig.port, &["--cacert", &only_upstream, &url]).await;
    assert!(!ok, "信不过 Dock 根证书的客户端第一次会失败：{first_out}");
    // 代理记名单和 curl 退出不同步：等它发出这一次的出错记录（和记名单在一处）再看名单。
    // 拒绝有两种：macOS 的 curl 在握手里就拒绝；Linux 上 curl 8 + OpenSSL 握手做完才验证书，
    // 验不过直接断开（见下一条用例）。
    let first = wait_flows(&rig.flows, 1).await;
    if first.is_empty() {
        // 诊断（只在已经失败时跑）：再等一会儿，看代理最后记成了什么。
        let late = wait_flows_for(&rig.flows, 1, Duration::from_secs(10)).await;
        let late: Vec<_> = late
            .iter()
            .map(|f| {
                (
                    f.kind,
                    f.decrypted,
                    f.note.clone(),
                    f.error.clone(),
                    f.duration_ms,
                )
            })
            .collect();
        panic!(
            "第一次要记一条出错记录；curl：{first_out}；直通名单：{:?}；10 秒后的记录：{late:?}",
            rig.learned.lock().unwrap()
        );
    }
    // 两种拒绝（握手里拒绝 / 握手后不发请求就断开）措辞不同，结果一样：之后原样转发。
    let rejected = &first[0];
    assert!(
        rejected
            .error
            .as_deref()
            .unwrap_or("")
            .contains("之后这个主机原样转发"),
        "{rejected:?}"
    );
    // 这个进程里马上直通。落不落盘看客户端怎么拒的（见下面两条）：macOS 的 curl 在握手里
    // 发 UnknownCA（落盘），Linux 上 curl 8 + OpenSSL 握手后断开（只记内存）。
    assert_eq!(rig.proxy.passthrough_hosts(), ["localhost"]);
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

/// 有的客户端握手做完才自己验证书（curl 8 + OpenSSL，Linux 上的常态）：验不过就一个字节
/// 不发、直接断开，不在握手里拒绝。这也算拒绝了 Dock 根证书：记进直通名单，之后原样转发。
/// 以前代理只认握手中途的拒绝，这种客户端每次都失败、名单永远学不到。
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_drops_right_after_the_handshake_is_passed_through_afterwards() {
    let pki = test_pki();
    let server = serve(Some(pki.server)).await;
    let rig = rig(vec![pki.ca_der]).await;
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", rig.port))
        .await
        .unwrap();
    tcp.write_all(
        format!("CONNECT localhost:{server} HTTP/1.1\r\nHost: localhost:{server}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(tcp.read(&mut byte).await.unwrap(), 1, "代理没回 CONNECT");
        head.push(byte[0]);
    }
    assert!(String::from_utf8_lossy(&head).contains(" 200"), "{head:?}");
    // 握手时什么证书都收，握手做完就断开、不发请求。
    let connector = tokio_native_tls::TlsConnector::from(
        native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap(),
    );
    let tls = connector.connect("localhost", tcp).await.unwrap();
    drop(tls);

    let flows = wait_flows(&rig.flows, 1).await;
    let f = flows.first().expect("要记一条出错记录");
    assert!(
        f.error.as_deref().unwrap_or("").contains("没发请求"),
        "{f:?}"
    );
    // 说不准是不信证书还是连接被取消：这个进程里直通，不落盘（下次启动还会再试着解密）。
    assert_eq!(rig.proxy.passthrough_hosts(), ["localhost"]);
    assert!(rig.learned.lock().unwrap().is_empty(), "握手后断开不落盘");
    rig.proxy.forget_passthrough(Some("LOCALHOST"));
    assert!(rig.proxy.passthrough_hosts().is_empty(), "清掉之后又解密");
}

/// 握手里收到证书类的 alert（UnknownCA）：确定是不信 Dock 根证书，落盘，下次启动还认。
/// 以前握手里的任何错误都落盘，断网、进程被杀也会永久关掉这个主机的解密。
#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_alert_is_remembered_but_a_broken_handshake_is_not() {
    let pki = test_pki();
    let upstream_ca = pki.ca_der.clone();
    let server = serve(Some(pki.server)).await;
    let rig = rig(vec![pki.ca_der]).await;
    let connect = || async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", rig.port))
            .await
            .unwrap();
        tcp.write_all(
            format!("CONNECT localhost:{server} HTTP/1.1\r\nHost: localhost:{server}\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(tcp.read(&mut byte).await.unwrap(), 1, "代理没回 CONNECT");
            head.push(byte[0]);
        }
        tcp
    };

    // 握手到一半就断开（发了 ClientHello、没等回话）：不落盘。
    let mut tcp = connect().await;
    let hello = {
        // 一个最小的 ClientHello 由客户端库生成：起一个握手、拿到第一段字节就停。
        let (mut ours, theirs) = tokio::io::duplex(16 * 1024);
        let connector = tokio_native_tls::TlsConnector::from(
            native_tls::TlsConnector::builder().build().unwrap(),
        );
        let task = tokio::spawn(async move { connector.connect("localhost", theirs).await });
        let mut buf = vec![0u8; 16 * 1024];
        let n = ours.read(&mut buf).await.unwrap();
        task.abort();
        buf.truncate(n);
        buf
    };
    tcp.write_all(&hello).await.unwrap();
    drop(tcp);
    let flows = wait_flows(&rig.flows, 1).await;
    assert!(!flows.is_empty(), "要记一条出错记录");
    assert!(
        rig.learned.lock().unwrap().is_empty(),
        "握手不完整不落盘：{flows:?}"
    );
    rig.proxy.forget_passthrough(None);

    // 客户端只信上游的 CA、不信 Dock 根证书，验不过就在握手里发 alert（rustls 客户端会发；
    // macOS 的系统 TLS、Linux 上的 curl 8 不发，直接断开——那种只记内存）：落盘。
    let tcp = connect().await;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(upstream_ca)).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    assert!(connector.connect(name, tcp).await.is_err());
    let flows = wait_flows(&rig.flows, 2).await;
    assert_eq!(flows.len(), 2, "{flows:?}");
    assert_eq!(
        rig.learned.lock().unwrap().as_slice(),
        ["localhost"],
        "{:?}",
        flows[1]
    );
}

/// 沙箱策略的关卡：阻断的连接回 403、记一条，命令拿到的是明确的拒绝，别的照常走。
#[tokio::test(flavor = "multi_thread")]
async fn the_gate_blocks_hosts_the_policy_denies() {
    if !curl_or_skip() {
        return;
    }
    let server = serve(None).await;
    let dir = tempfile::tempdir().unwrap();
    let flows: Arc<Mutex<Vec<Flow>>> = Arc::default();
    let sink_flows = flows.clone();
    let gate: dock_sandbox::audit::proxy::Gate = Arc::new(|_tag: &str, host: &str, _port: u16| {
        let deny = host == "localhost";
        Box::pin(async move { deny.then(|| "规则 localhost → deny".to_string()) })
    });
    let proxy = Proxy::with_gate(
        Some(Arc::new(Authority::load_or_create(dir.path()).unwrap())),
        Options::default(),
        Arc::new(move |f| sink_flows.lock().unwrap().push(f)),
        Vec::new(),
        None,
        Some(gate),
    );
    let listener = proxy.listen("call-1").await.unwrap();
    let (ok, out) = curl(
        listener.port,
        &["-f", &format!("http://localhost:{server}/no")],
    )
    .await;
    assert!(!ok, "被策略阻断：{out}");
    let (ok, out) = curl(listener.port, &[&format!("http://127.0.0.1:{server}/yes")]).await;
    assert!(ok, "{out}");
    let flows = wait_flows(&flows, 2).await;
    assert!(
        flows
            .iter()
            .any(|f| f.error.as_deref().unwrap_or("").contains("沙箱策略阻断")),
        "{flows:?}"
    );
}

// ---- WebSocket ----------------------------------------------------------------------------

/// 一帧：`b0` 是第一个字节（FIN / RSV / 操作码），`mask` 有就加掩码（客户端发的都要加）。
fn ws_frame(b0: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
    let mut out = vec![b0];
    let bit = if mask.is_some() { 0x80 } else { 0 };
    match payload.len() {
        n if n < 126 => out.push(bit | n as u8),
        n if n <= 0xffff => {
            out.push(bit | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(bit | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    match mask {
        Some(m) => {
            out.extend_from_slice(&m);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ m[i % 4]));
        }
        None => out.extend_from_slice(payload),
    }
    out
}

/// 读一帧，给回 (第一个字节, 去掉掩码的内容)。`pending` 是上次多读的。
async fn ws_read<S: tokio::io::AsyncRead + Unpin>(
    s: &mut S,
    pending: &mut Vec<u8>,
) -> Option<(u8, Vec<u8>)> {
    loop {
        if pending.len() >= 2 {
            let masked = pending[1] & 0x80 != 0;
            let (len, at) = match pending[1] & 0x7f {
                126 if pending.len() >= 4 => {
                    (u16::from_be_bytes([pending[2], pending[3]]) as usize, 4)
                }
                127 if pending.len() >= 10 => (
                    u64::from_be_bytes(pending[2..10].try_into().unwrap()) as usize,
                    10,
                ),
                n if n < 126 => (n as usize, 2),
                _ => (usize::MAX, 0),
            };
            let head = at + if masked { 4 } else { 0 };
            if len != usize::MAX && pending.len() >= head + len {
                let mask = masked.then(|| {
                    [
                        pending[at],
                        pending[at + 1],
                        pending[at + 2],
                        pending[at + 3],
                    ]
                });
                let b0 = pending[0];
                let payload: Vec<u8> = pending[head..head + len]
                    .iter()
                    .enumerate()
                    .map(|(i, b)| mask.map_or(*b, |m| b ^ m[i % 4]))
                    .collect();
                pending.drain(..head + len);
                return Some((b0, payload));
            }
        }
        let mut chunk = [0u8; 4096];
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        pending.extend_from_slice(&chunk[..n]);
    }
}

/// 读到 `\r\n\r\n` 为止，多读的留在 `pending`。
async fn read_head<S: tokio::io::AsyncRead + Unpin>(s: &mut S, pending: &mut Vec<u8>) -> String {
    loop {
        if let Some(i) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&pending[..i + 4]).to_string();
            pending.drain(..i + 4);
            return head;
        }
        let mut chunk = [0u8; 4096];
        let n = s.read(&mut chunk).await.unwrap();
        assert!(
            n > 0,
            "握手没读完就断了：{:?}",
            String::from_utf8_lossy(pending)
        );
        pending.extend_from_slice(&chunk[..n]);
    }
}

/// WebSocket 回声服务器：握手（`deflate` 时同意 permessage-deflate），之后收到什么帧就（去掉
/// 掩码）原样回什么帧——压缩的帧原样回，回来的那一路照样是一条完整的压缩流。收到关闭帧回一个
/// 关闭帧就断开。代理不校验 `Sec-WebSocket-Accept`，这里不算。
async fn ws_echo(tls: Option<rustls::ServerConfig>, deflate: bool) -> u16 {
    async fn conn<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
        mut s: S,
        deflate: bool,
    ) {
        let mut pending = Vec::new();
        let head = read_head(&mut s, &mut pending).await;
        assert!(
            head.to_ascii_lowercase().contains("upgrade: websocket"),
            "{head}"
        );
        let ext = if deflate {
            "Sec-WebSocket-Extensions: permessage-deflate\r\n"
        } else {
            ""
        };
        let resp = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: test\r\n{ext}\r\n"
        );
        s.write_all(resp.as_bytes()).await.unwrap();
        while let Some((b0, payload)) = ws_read(&mut s, &mut pending).await {
            s.write_all(&ws_frame(b0, &payload, None)).await.unwrap();
            if b0 & 0x0f == 8 {
                break;
            }
        }
        let _ = s.shutdown().await;
    }
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
                            conn(tls, deflate).await;
                        }
                    }
                    None => conn(s, deflate).await,
                }
            });
        }
    });
    port
}

fn upgrade_request(target: &str, host: &str, deflate: bool) -> String {
    let ext = if deflate {
        "Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits\r\n"
    } else {
        ""
    };
    format!(
        "GET {target} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{ext}\r\n"
    )
}

const MASK: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];

/// 记录里的 ws 段（`kind == "ws"`）按顺序拼起来的全部消息，和最后一段（关了才有）。
fn ws_messages(flows: &[Flow]) -> (Vec<serde_json::Value>, Option<serde_json::Value>) {
    let mut all = Vec::new();
    let mut last = None;
    for f in flows.iter().filter(|f| f.kind == "ws") {
        let part = serde_json::to_value(f.ws.as_ref().unwrap()).unwrap()["part"].clone();
        let sample = &f
            .ws
            .as_ref()
            .unwrap()
            .part
            .as_ref()
            .unwrap()
            .messages
            .sample;
        if !sample.is_empty() {
            let batch: Vec<serde_json::Value> = serde_json::from_slice(sample).unwrap();
            all.extend(batch);
        }
        if part["done"] == true {
            last = Some(part);
        }
    }
    (all, last)
}

async fn wait_until(
    flows: &Arc<Mutex<Vec<Flow>>>,
    limit: Duration,
    done: impl Fn(&[Flow]) -> bool,
) -> Vec<Flow> {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if done(&flows.lock().unwrap()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    flows.lock().unwrap().clone()
}

/// 明文 ws://：握手一成就记（连接还开着），之后两个方向的消息（文本、二进制、关闭帧）都记下来。
#[tokio::test(flavor = "multi_thread")]
async fn websocket_messages_are_recorded_both_ways_while_the_connection_is_open() {
    let server = ws_echo(None, false).await;
    let rig = rig(Vec::new()).await;
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", rig.port))
        .await
        .unwrap();
    let host = format!("127.0.0.1:{server}");
    s.write_all(upgrade_request(&format!("http://{host}/chat?room=1"), &host, false).as_bytes())
        .await
        .unwrap();
    let mut pending = Vec::new();
    let head = read_head(&mut s, &mut pending).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    s.write_all(&ws_frame(0x81, b"hello", Some(MASK)))
        .await
        .unwrap();
    assert_eq!(
        ws_read(&mut s, &mut pending).await.unwrap(),
        (0x81, b"hello".to_vec())
    );
    s.write_all(&ws_frame(0x82, &[0, 1, 2, 0xff], Some(MASK)))
        .await
        .unwrap();
    assert_eq!(
        ws_read(&mut s, &mut pending).await.unwrap(),
        (0x82, vec![0, 1, 2, 0xff])
    );

    // 连接还开着：握手那条已经在了，消息也在几秒内落下来。
    let open = wait_until(&rig.flows, Duration::from_secs(5), |fs| {
        fs.iter().any(|f| f.kind == "ws")
    })
    .await;
    let hs = open
        .iter()
        .find(|f| f.kind == "http")
        .expect("握手一成就该记一条");
    assert_eq!(
        (hs.scheme, hs.status, hs.path.as_deref()),
        ("ws", Some(101), Some("/chat?room=1"))
    );
    let conn = hs.ws.as_ref().expect("握手那条带 ws.conn").conn.clone();
    let (msgs, done) = ws_messages(&open);
    assert!(done.is_none(), "还没关：{open:?}");
    assert_eq!(msgs.len(), 4, "{msgs:?}");

    let mut close = 1000u16.to_be_bytes().to_vec();
    close.extend_from_slice(b"bye");
    s.write_all(&ws_frame(0x88, &close, Some(MASK)))
        .await
        .unwrap();
    assert_eq!(ws_read(&mut s, &mut pending).await.unwrap().0, 0x88);
    drop(s);

    let flows = wait_until(&rig.flows, Duration::from_secs(5), |fs| {
        ws_messages(fs).1.is_some()
    })
    .await;
    assert!(flows
        .iter()
        .filter(|f| f.kind == "ws")
        .all(|f| f.ws.as_ref().unwrap().conn == conn));
    let hs = flows.iter().find(|f| f.kind == "http").unwrap();
    assert!(
        flows
            .iter()
            .filter(|f| f.kind == "ws")
            .all(|f| f.decrypted == hs.decrypted),
        "每一段的 decrypted 和握手一致"
    );
    let (msgs, done) = ws_messages(&flows);
    let done = done.expect("关了要有最后一段");
    let brief: Vec<(String, String, serde_json::Value)> = msgs
        .iter()
        .map(|m| {
            let content = if m["op"] == "close" {
                serde_json::json!([m["code"], m["reason"]])
            } else {
                m.get("text")
                    .or(m.get("base64"))
                    .cloned()
                    .unwrap_or_default()
            };
            (
                m["dir"].as_str().unwrap().into(),
                m["op"].as_str().unwrap().into(),
                content,
            )
        })
        .collect();
    use serde_json::json;
    assert_eq!(
        brief,
        vec![
            ("up".into(), "text".into(), json!("hello")),
            ("down".into(), "text".into(), json!("hello")),
            ("up".into(), "binary".into(), json!("AAEC/w==")),
            ("down".into(), "binary".into(), json!("AAEC/w==")),
            ("up".into(), "close".into(), json!([1000, "bye"])),
            ("down".into(), "close".into(), json!([1000, "bye"])),
        ]
    );
    assert_eq!(
        (done["sent"].as_u64(), done["received"].as_u64()),
        (Some(3), Some(3))
    );
    assert_eq!(done["close"]["by"], "client");
    assert_eq!(done["close"]["code"], 1000);
    let wire: u64 = flows
        .iter()
        .filter(|f| f.kind == "ws")
        .map(|f| f.bytes_up)
        .sum();
    assert_eq!(
        wire,
        (6 + 5 + 6 + 4 + 6 + 5) as u64,
        "上行的帧按线上的字节数算"
    );
}

/// wss:// 解密后，协商了 permessage-deflate 的消息照样解出来（带上下文：第二条引用第一条的内容）。
#[tokio::test(flavor = "multi_thread")]
async fn wss_with_permessage_deflate_is_decoded() {
    use flate2::{Compress, Compression, FlushCompress};
    let pki = test_pki();
    let server = ws_echo(Some(pki.server), true).await;
    let rig = rig(vec![pki.ca_der]).await;

    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", rig.port))
        .await
        .unwrap();
    tcp.write_all(
        format!("CONNECT localhost:{server} HTTP/1.1\r\nHost: localhost:{server}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut pending = Vec::new();
    let head = read_head(&mut tcp, &mut pending).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let pem = std::fs::read_to_string(&rig.ca_path).unwrap();
    let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    use base64::Engine as _;
    let der = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(der)).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut s = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .unwrap();

    let host = format!("localhost:{server}");
    s.write_all(upgrade_request("/feed", &host, true).as_bytes())
        .await
        .unwrap();
    let mut pending = Vec::new();
    let head = read_head(&mut s, &mut pending).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");

    let mut z = Compress::new(Compression::default(), false);
    let mut deflate = |text: &[u8]| {
        let mut out = Vec::with_capacity(text.len() + 64);
        z.compress_vec(text, &mut out, FlushCompress::Sync).unwrap();
        assert!(out.ends_with(&[0, 0, 0xff, 0xff]));
        out.truncate(out.len() - 4);
        out
    };
    for text in [
        &b"hello websocket, hello websocket"[..],
        b"hello websocket again",
    ] {
        let packed = deflate(text);
        s.write_all(&ws_frame(0xc1, &packed, Some(MASK)))
            .await
            .unwrap();
        assert_eq!(ws_read(&mut s, &mut pending).await.unwrap(), (0xc1, packed));
    }
    s.write_all(&ws_frame(0x88, &1000u16.to_be_bytes(), Some(MASK)))
        .await
        .unwrap();
    assert_eq!(ws_read(&mut s, &mut pending).await.unwrap().0, 0x88);
    drop(s);

    let flows = wait_until(&rig.flows, Duration::from_secs(5), |fs| {
        ws_messages(fs).1.is_some()
    })
    .await;
    let hs = flows.iter().find(|f| f.kind == "http").unwrap();
    assert_eq!((hs.scheme, hs.status), ("wss", Some(101)));
    let (msgs, _) = ws_messages(&flows);
    let texts: Vec<(&str, &str, bool)> = msgs
        .iter()
        .filter(|m| m["op"] == "text")
        .map(|m| {
            (
                m["dir"].as_str().unwrap(),
                m["text"].as_str().unwrap(),
                m["compressed"] == true,
            )
        })
        .collect();
    assert_eq!(
        texts,
        [
            ("up", "hello websocket, hello websocket", true),
            ("down", "hello websocket, hello websocket", true),
            ("up", "hello websocket again", true),
            ("down", "hello websocket again", true),
        ]
    );
}

/// `ws://` 经 CONNECT 隧道走代理（Python websockets、Node 的代理 agent 都这么走）：隧道里是
/// 明文 HTTP，照样认出握手和消息——以前当成「不是 TLS」的 tcp 隧道，只记了字节数。
#[tokio::test(flavor = "multi_thread")]
async fn websocket_inside_a_connect_tunnel_is_decoded() {
    let server = ws_echo(None, false).await;
    let rig = rig(Vec::new()).await;
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", rig.port))
        .await
        .unwrap();
    let host = format!("127.0.0.1:{server}");
    s.write_all(format!("CONNECT {host} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut pending = Vec::new();
    let head = read_head(&mut s, &mut pending).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    s.write_all(upgrade_request("/chat", &host, false).as_bytes())
        .await
        .unwrap();
    let head = read_head(&mut s, &mut pending).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    s.write_all(&ws_frame(0x81, b"via tunnel", Some(MASK)))
        .await
        .unwrap();
    assert_eq!(
        ws_read(&mut s, &mut pending).await.unwrap().1,
        b"via tunnel"
    );
    s.write_all(&ws_frame(0x88, &1000u16.to_be_bytes(), Some(MASK)))
        .await
        .unwrap();
    assert_eq!(ws_read(&mut s, &mut pending).await.unwrap().0, 0x88);
    drop(s);

    let flows = wait_until(&rig.flows, Duration::from_secs(5), |fs| {
        ws_messages(fs).1.is_some()
    })
    .await;
    assert!(
        !flows.iter().any(|f| f.kind == "tunnel"),
        "不该当成只有字节数的隧道：{flows:?}"
    );
    let hs = flows.iter().find(|f| f.kind == "http").unwrap();
    assert_eq!(
        (hs.scheme, hs.status, hs.path.as_deref()),
        ("ws", Some(101), Some("/chat"))
    );
    let (msgs, _) = ws_messages(&flows);
    assert_eq!(msgs[0]["text"], "via tunnel", "{msgs:?}");
}

/// 明文 HTTP 经 CONNECT（`curl --proxytunnel`）：和明文代理请求一样记下请求和响应。
#[tokio::test(flavor = "multi_thread")]
async fn plain_http_inside_a_connect_tunnel_is_recorded() {
    if !curl_or_skip() {
        return;
    }
    let server = serve(None).await;
    let rig = rig(Vec::new()).await;
    let (ok, out) = curl(
        rig.port,
        &[
            "--proxytunnel",
            "-d",
            "a=1",
            &format!("http://127.0.0.1:{server}/t"),
        ],
    )
    .await;
    assert!(ok, "{out}");
    assert_eq!(out, "hello /t a=1");
    let flows = wait_flows(&rig.flows, 1).await;
    let f = &flows[0];
    assert_eq!(
        (f.kind, f.scheme, f.status),
        ("http", "http", Some(200)),
        "{f:?}"
    );
    assert_eq!(f.request.as_ref().unwrap().body.sample, b"a=1");
    assert_eq!(f.response.as_ref().unwrap().body.sample, b"hello /t a=1");
}
