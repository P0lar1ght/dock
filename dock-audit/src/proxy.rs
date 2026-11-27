//! 出口代理：HTTP 代理协议（CONNECT 隧道 + 明文 HTTP 转发），只听回环。
//!
//! 每条命令一个监听口（[`Proxy::listen`]），流量记录带上开口时给的 `tag`，所以每次出网都能
//! 追到是哪个会话、哪次工具调用。HTTPS 在这里解密：按主机现签叶子证书（[`Authority`]），
//! 请求原样转给真实站点，请求 / 响应的头和正文（开头一段）记成 [`Flow`]。
//!
//! 不削弱命令能做的事——下面这些都**原样转发**（只记目的地和字节数），不让它们失败：
//! - 上游的 TLS 先连、先验（系统证书库），没过的（自签的开发服务器、客户端本来就加了 `-k`）
//!   交给客户端自己判断，代理不替它把关。
//! - 客户端只说 h2（gRPC）：解密后只能讲 HTTP/1.1。
//! - 客户端不信 Dock 根证书（握手被拒）：这个主机记进直通名单，之后都原样转发。
//! - 已知不读证书环境变量的客户端（macOS 上的 Go、Java），以及不是 TLS 的隧道（SSH 之类）。

use std::collections::HashSet;
use std::convert::Infallible;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{HeaderValue, HOST, UPGRADE, USER_AGENT};
use http::{Method, Request, Response, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Body, Frame, Incoming, SizeHint};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex as AsyncMutex;

use crate::ca::Authority;
use crate::record::{headers, now_ms, Captured, Flow, Message};

/// 每条流量记录交给谁（spine 那边落盘）。必须很快返回：在代理的读写路径上调用。
pub type Sink = Arc<dyn Fn(Flow) + Send + Sync>;
/// 学到了新的直通主机（客户端不信 Dock 根证书）：交给调用方落盘，下次启动还记得。
pub type Learn = Arc<dyn Fn(&str, &str) + Send + Sync>;

type ProxyBody = BoxBody<Bytes, hyper::Error>;

/// 第一个字节最多等这么久：等不到就是服务器先说话的协议（SMTP 之类），按原始隧道转。
const FIRST_BYTE_WAIT: Duration = Duration::from_secs(3);
const HELLO_MAX: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone)]
pub struct Options {
    /// HTTPS 解不解密（不解密时只记目的地和字节数）。
    pub decrypt: bool,
    /// 请求 / 响应正文最多记这么多字节（之后只计长度）。
    pub body_cap: usize,
    /// 凭据类的头（Authorization、Cookie…）遮住。
    pub redact: bool,
    /// 用户本来就配了上游代理（Dock 进程的 `HTTPS_PROXY`）：代理往外连时经它走。
    pub upstream: Option<Upstream>,
    /// 验上游证书时在系统证书库之外再信这些根证书（DER；企业内网 CA）。
    pub extra_roots: Vec<Vec<u8>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            decrypt: true,
            body_cap: 256 * 1024,
            redact: true,
            upstream: None,
            extra_roots: Vec::new(),
        }
    }
}

/// 上游 HTTP 代理（只认 `http://[user:pass@]host:port`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    pub host: String,
    pub port: u16,
    /// `Proxy-Authorization` 的值（URL 里带了用户名密码时）。
    pub auth: Option<String>,
    /// 原来的 `NO_PROXY`：这些主机直连。
    pub no_proxy: Vec<String>,
}

impl Upstream {
    /// 从一个代理地址解析；不是 `http://` 的（`socks5://`）不认，返回 `None`。
    pub fn parse(url: &str, no_proxy: &str) -> Option<Self> {
        let rest = url.trim().strip_prefix("http://")?;
        let rest = rest.trim_end_matches('/');
        let (userinfo, hostport) = match rest.rsplit_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, rest),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().ok()?),
            None => (hostport.to_string(), 80),
        };
        let auth = userinfo.map(|u| {
            use base64_lite::encode;
            format!("Basic {}", encode(u.as_bytes()))
        });
        Some(Self {
            host,
            port,
            auth,
            no_proxy: no_proxy
                .split(',')
                .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
        })
    }

    fn bypass(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.no_proxy
            .iter()
            .any(|n| n == "*" || host == *n || host.ends_with(&format!(".{n}")))
    }
}

/// 一个出口代理（一个 Dock 进程一个），按命令开监听口。
pub struct Proxy {
    ca: Option<Arc<Authority>>,
    opts: Options,
    sink: Sink,
    passthrough: Mutex<HashSet<String>>,
    learn: Option<Learn>,
}

/// 一个监听口；drop 就不再接新连接（已经连上的照常跑完）。
pub struct Listener {
    pub port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Proxy {
    /// `ca` 为 `None` 时不解密（`opts.decrypt` 也就不起作用）。`passthrough` 是之前学到的直通主机。
    pub fn new(
        ca: Option<Arc<Authority>>,
        opts: Options,
        sink: Sink,
        passthrough: impl IntoIterator<Item = String>,
        learn: Option<Learn>,
    ) -> Arc<Self> {
        Arc::new(Self {
            ca,
            opts,
            sink,
            passthrough: Mutex::new(passthrough.into_iter().collect()),
            learn,
        })
    }

    /// 在 `127.0.0.1` 上开一个口，进来的流量都记成 `tag` 的。
    pub async fn listen(self: &Arc<Self>, tag: impl Into<String>) -> io::Result<Listener> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let tag: Arc<str> = Arc::from(tag.into());
        let proxy = self.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let proxy = proxy.clone();
                let tag = tag.clone();
                tokio::spawn(async move { proxy.serve_client(tag, stream).await });
            }
        });
        Ok(Listener { port, task })
    }

    /// 同 [`Proxy::listen`]，但同步绑定（在同步代码里起命令时用；要在 tokio 运行时里调）。
    pub fn listen_now(self: &Arc<Self>, tag: impl Into<String>) -> io::Result<Listener> {
        let std_listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        std_listener.set_nonblocking(true)?;
        let port = std_listener.local_addr()?.port();
        let listener = TcpListener::from_std(std_listener)?;
        let tag: Arc<str> = Arc::from(tag.into());
        let proxy = self.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let proxy = proxy.clone();
                let tag = tag.clone();
                tokio::spawn(async move { proxy.serve_client(tag, stream).await });
            }
        });
        Ok(Listener { port, task })
    }

    async fn serve_client(self: Arc<Self>, tag: Arc<str>, stream: TcpStream) {
        let _ = stream.set_nodelay(true);
        let proxy = self.clone();
        let service = service_fn(move |req| {
            let proxy = proxy.clone();
            let tag = tag.clone();
            async move { Ok::<_, Infallible>(proxy.handle(tag, req).await) }
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .preserve_header_case(true)
            .serve_connection(TokioIo::new(stream), service)
            .with_upgrades()
            .await;
    }

    async fn handle(self: Arc<Self>, tag: Arc<str>, req: Request<Incoming>) -> Response<ProxyBody> {
        if req.method() == Method::CONNECT {
            let Some((host, port)) = authority(req.uri(), 443) else {
                return text(StatusCode::BAD_REQUEST, "CONNECT 要 host:port");
            };
            let ua = req
                .headers()
                .get(USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let proxy = self.clone();
            tokio::spawn(async move {
                match hyper::upgrade::on(req).await {
                    Ok(upgraded) => {
                        proxy
                            .tunnel(tag, host, port, ua, TokioIo::new(upgraded))
                            .await
                    }
                    Err(e) => proxy.emit_error(&tag, "tcp", &host, port, format!("upgrade: {e}")),
                }
            });
            return Response::new(empty());
        }
        self.forward_plain(tag, req).await
    }

    /// CONNECT 之后：看第一包是不是 TLS、决定解不解密。
    async fn tunnel<S>(
        self: Arc<Self>,
        tag: Arc<str>,
        host: String,
        port: u16,
        ua: String,
        mut client: S,
    ) where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let started = now_ms();
        let t0 = Instant::now();
        let (prefix, hello) = peek_hello(&mut client).await;
        let name = hello
            .as_ref()
            .and_then(|h| h.sni.clone())
            .unwrap_or_else(|| host.clone());
        let mut reason = self.passthrough_reason(&name, &ua, hello.as_ref());
        if reason.is_none() {
            match self.upstream_tls(&name, &host, port).await {
                Ok(upstream) => {
                    self.mitm(tag, name, port, prefix, client, upstream).await;
                    return;
                }
                Err(e) => reason = Some(format!("上游 TLS 没过系统校验，交给客户端自己判断：{e}")),
            }
        }
        let scheme = if hello.is_some() { "https" } else { "tcp" };
        self.raw(tag, scheme, name, port, prefix, client, reason, started, t0)
            .await;
    }

    /// 不解密的理由（`None` = 解密）。
    fn passthrough_reason(&self, host: &str, ua: &str, hello: Option<&Hello>) -> Option<String> {
        let Some(hello) = hello else {
            return Some("不是 TLS".into());
        };
        if !self.opts.decrypt || self.ca.is_none() {
            return Some("没开解密".into());
        }
        if !hello.alpn.is_empty() && !hello.alpn.iter().any(|p| p == b"http/1.1") {
            return Some("客户端只说 h2（gRPC 之类），解密后只能讲 HTTP/1.1".into());
        }
        let host = host.to_ascii_lowercase();
        if self.passthrough.lock().unwrap().contains(&host) {
            return Some("这个主机之前拒绝过 Dock 根证书".into());
        }
        if ua.starts_with("Java/")
            || (cfg!(target_os = "macos") && ua.starts_with("Go-http-client"))
        {
            return Some(format!("{ua} 不读证书环境变量，不信 Dock 根证书"));
        }
        None
    }

    async fn upstream_tls(
        &self,
        name: &str,
        host: &str,
        port: u16,
    ) -> Result<tokio_native_tls::TlsStream<TcpStream>, String> {
        let tcp = self.connect(host, port).await.map_err(|e| e.to_string())?;
        let mut builder = native_tls::TlsConnector::builder();
        builder.request_alpns(&["http/1.1"]);
        for der in &self.opts.extra_roots {
            if let Ok(cert) = native_tls::Certificate::from_der(der) {
                builder.add_root_certificate(cert);
            }
        }
        let connector = builder.build().map_err(|e| e.to_string())?;
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio_native_tls::TlsConnector::from(connector).connect(name, tcp),
        )
        .await
        .map_err(|_| "TLS 握手超时".to_string())?
        .map_err(|e| e.to_string())
    }

    /// 往外连 `host:port`：配了上游代理就经它走。
    async fn connect(&self, host: &str, port: u16) -> io::Result<TcpStream> {
        let fut = async {
            match self.opts.upstream.as_ref().filter(|u| !u.bypass(host)) {
                None => TcpStream::connect((host, port)).await,
                Some(up) => {
                    let mut s = TcpStream::connect((up.host.as_str(), up.port)).await?;
                    let mut head =
                        format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
                    if let Some(auth) = &up.auth {
                        head.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
                    }
                    head.push_str("\r\n");
                    s.write_all(head.as_bytes()).await?;
                    let mut buf = Vec::new();
                    let mut byte = [0u8; 1];
                    while !buf.ends_with(b"\r\n\r\n") {
                        if s.read(&mut byte).await? == 0 || buf.len() > 8192 {
                            return Err(io::Error::other("上游代理没回话"));
                        }
                        buf.push(byte[0]);
                    }
                    let line = String::from_utf8_lossy(&buf);
                    if !line.split_whitespace().nth(1).is_some_and(|c| c == "200") {
                        return Err(io::Error::other(format!(
                            "上游代理拒绝：{}",
                            line.lines().next().unwrap_or("")
                        )));
                    }
                    Ok(s)
                }
            }
        };
        tokio::time::timeout(CONNECT_TIMEOUT, fut)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "连接超时"))?
    }

    /// 中间人：用 Dock 签的证书和客户端握手，之后一个请求一个请求地转给上游。
    async fn mitm<S>(
        self: Arc<Self>,
        tag: Arc<str>,
        name: String,
        port: u16,
        prefix: Vec<u8>,
        client: S,
        upstream: tokio_native_tls::TlsStream<TcpStream>,
    ) where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let Some(ca) = self.ca.clone() else {
            return;
        };
        let config = match ca.leaf(&name).and_then(|leaf| server_config(&ca, leaf)) {
            Ok(c) => c,
            Err(e) => return self.emit_error(&tag, "https", &name, port, e),
        };
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let tls = match acceptor.accept(Rewind::new(prefix, client)).await {
            Ok(tls) => tls,
            Err(e) => {
                let why = format!("客户端拒绝了 Dock 根证书（{e}），之后这个主机原样转发");
                self.passthrough
                    .lock()
                    .unwrap()
                    .insert(name.to_ascii_lowercase());
                if let Some(learn) = &self.learn {
                    learn(&name, &why);
                }
                return self.emit_error(&tag, "https", &name, port, why);
            }
        };
        let (sender, conn) = match hyper::client::conn::http1::Builder::new()
            .preserve_header_case(true)
            .handshake::<_, ProxyBody>(TokioIo::new(upstream))
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                return self.emit_error(&tag, "https", &name, port, format!("上游 HTTP：{e}"))
            }
        };
        tokio::spawn(conn.with_upgrades());
        let target = Arc::new(Target {
            scheme: "https",
            host: name,
            port,
            sender: AsyncMutex::new(Some(sender)),
        });
        let proxy = self.clone();
        let service = service_fn(move |req| {
            let proxy = proxy.clone();
            let tag = tag.clone();
            let target = target.clone();
            async move { Ok::<_, Infallible>(proxy.relay(tag, target, req).await) }
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .preserve_header_case(true)
            .serve_connection(TokioIo::new(tls), service)
            .with_upgrades()
            .await;
    }

    /// 明文 HTTP 代理请求（`GET http://host/path`）：一个请求一条上游连接。
    async fn forward_plain(
        self: Arc<Self>,
        tag: Arc<str>,
        req: Request<Incoming>,
    ) -> Response<ProxyBody> {
        let Some((host, port)) = authority(req.uri(), 80) else {
            return text(
                StatusCode::BAD_REQUEST,
                "代理请求要完整网址（http://host/path）",
            );
        };
        let tcp = match self.connect(&host, port).await {
            Ok(s) => s,
            Err(e) => {
                self.emit_error(&tag, "http", &host, port, e.to_string());
                return text(
                    StatusCode::BAD_GATEWAY,
                    &format!("Dock 出口代理连不上 {host}:{port}：{e}"),
                );
            }
        };
        let (sender, conn) = match hyper::client::conn::http1::Builder::new()
            .preserve_header_case(true)
            .handshake::<_, ProxyBody>(TokioIo::new(tcp))
            .await
        {
            Ok(pair) => pair,
            Err(e) => return text(StatusCode::BAD_GATEWAY, &format!("Dock 出口代理：{e}")),
        };
        tokio::spawn(conn.with_upgrades());
        let target = Arc::new(Target {
            scheme: "http",
            host,
            port,
            sender: AsyncMutex::new(Some(sender)),
        });
        self.relay(tag, target, req).await
    }

    /// 转一个请求：两边的正文边转边抄（[`Tee`]），响应结束（或连接断了）时记一条 [`Flow`]。
    async fn relay(
        self: Arc<Self>,
        tag: Arc<str>,
        target: Arc<Target>,
        mut req: Request<Incoming>,
    ) -> Response<ProxyBody> {
        let path = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_else(|| "/".into());
        let shared = Arc::new(Shared::new(
            self.sink.clone(),
            Flow {
                tag: tag.to_string(),
                kind: "http",
                scheme: target.scheme,
                host: target.host.clone(),
                port: target.port,
                method: Some(req.method().to_string()),
                path: Some(path.clone()),
                status: None,
                request: Some(Message {
                    headers: headers(req.headers(), self.opts.redact),
                    body: Captured::default(),
                }),
                response: None,
                bytes_up: 0,
                bytes_down: 0,
                started_ms: now_ms(),
                duration_ms: 0,
                decrypted: target.scheme == "https",
                note: None,
                error: None,
            },
        ));
        let client_upgrade = req
            .headers()
            .contains_key(UPGRADE)
            .then(|| hyper::upgrade::on(&mut req));
        let (mut parts, body) = req.into_parts();
        parts.uri = path.parse().unwrap_or_else(|_| Uri::from_static("/"));
        // 明文代理请求里的逐跳头别带给上游。
        parts.headers.remove("proxy-connection");
        parts.headers.remove("proxy-authorization");
        if !parts.headers.contains_key(HOST) {
            if let Ok(v) = HeaderValue::from_str(&target.host) {
                parts.headers.insert(HOST, v);
            }
        }
        let up = Request::from_parts(
            parts,
            Tee::new(body, shared.clone(), Dir::Up, self.opts.body_cap).boxed(),
        );
        let sent = {
            let mut slot = target.sender.lock().await;
            match slot.as_mut() {
                Some(sender) => match sender.ready().await {
                    Ok(()) => sender.send_request(up).await,
                    Err(e) => Err(e),
                },
                None => return text(StatusCode::BAD_GATEWAY, "上游连接已经关了"),
            }
        };
        let mut resp = match sent {
            Ok(r) => r,
            Err(e) => {
                shared.fail(format!("上游：{e}"));
                return text(
                    StatusCode::BAD_GATEWAY,
                    &format!("Dock 出口代理：上游出错：{e}"),
                );
            }
        };
        shared.respond(
            resp.status().as_u16(),
            headers(resp.headers(), self.opts.redact),
        );
        if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
            if let Some(client_upgrade) = client_upgrade {
                let upstream_upgrade = hyper::upgrade::on(&mut resp);
                let shared = shared.clone();
                tokio::spawn(async move {
                    if let (Ok(a), Ok(b)) = tokio::join!(client_upgrade, upstream_upgrade) {
                        let mut a = TokioIo::new(a);
                        let mut b = TokioIo::new(b);
                        if let Ok((up, down)) = tokio::io::copy_bidirectional(&mut a, &mut b).await
                        {
                            shared.upgraded(up, down);
                        }
                    }
                });
            }
        }
        let (parts, body) = resp.into_parts();
        Response::from_parts(
            parts,
            Tee::new(body, shared, Dir::Down, self.opts.body_cap).boxed(),
        )
    }

    /// 原样转发：先把读到的第一包补给上游，再两边对拷，记目的地和字节数。
    #[allow(clippy::too_many_arguments)] // 隧道的全部上下文；拆结构体只是换个地方放
    async fn raw<S>(
        &self,
        tag: Arc<str>,
        scheme: &'static str,
        host: String,
        port: u16,
        prefix: Vec<u8>,
        mut client: S,
        note: Option<String>,
        started: u64,
        t0: Instant,
    ) where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut flow = Flow {
            tag: tag.to_string(),
            kind: "tunnel",
            scheme,
            host: host.clone(),
            port,
            method: None,
            path: None,
            status: None,
            request: None,
            response: None,
            bytes_up: prefix.len() as u64,
            bytes_down: 0,
            started_ms: started,
            duration_ms: 0,
            decrypted: false,
            note,
            error: None,
        };
        match self.connect(&host, port).await {
            Ok(mut upstream) => {
                if upstream.write_all(&prefix).await.is_ok() {
                    if let Ok((up, down)) =
                        tokio::io::copy_bidirectional(&mut client, &mut upstream).await
                    {
                        flow.bytes_up += up;
                        flow.bytes_down = down;
                    }
                }
            }
            Err(e) => flow.error = Some(e.to_string()),
        }
        flow.duration_ms = t0.elapsed().as_millis() as u64;
        (self.sink)(flow);
    }

    fn emit_error(&self, tag: &str, scheme: &'static str, host: &str, port: u16, error: String) {
        (self.sink)(Flow {
            tag: tag.to_string(),
            kind: "tunnel",
            scheme,
            host: host.to_string(),
            port,
            method: None,
            path: None,
            status: None,
            request: None,
            response: None,
            bytes_up: 0,
            bytes_down: 0,
            started_ms: now_ms(),
            duration_ms: 0,
            decrypted: false,
            note: None,
            error: Some(error),
        });
    }
}

struct Target {
    scheme: &'static str,
    host: String,
    port: u16,
    sender: AsyncMutex<Option<hyper::client::conn::http1::SendRequest<ProxyBody>>>,
}

fn server_config(
    ca: &Authority,
    leaf: Arc<rustls::sign::CertifiedKey>,
) -> Result<rustls::ServerConfig, String> {
    #[derive(Debug)]
    struct One(Arc<rustls::sign::CertifiedKey>);
    impl rustls::server::ResolvesServerCert for One {
        fn resolve(
            &self,
            _: rustls::server::ClientHello<'_>,
        ) -> Option<Arc<rustls::sign::CertifiedKey>> {
            Some(self.0.clone())
        }
    }
    let mut config = rustls::ServerConfig::builder_with_provider(ca.provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(One(leaf)));
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

/// ClientHello 里决定怎么转的两样：SNI、ALPN。
struct Hello {
    sni: Option<String>,
    alpn: Vec<Vec<u8>>,
}

/// 读到完整的 ClientHello（或者确定不是 TLS）为止。返回读到的字节（转发时要补给对端）。
async fn peek_hello<S: AsyncRead + Unpin>(io: &mut S) -> (Vec<u8>, Option<Hello>) {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let mut acceptor = rustls::server::Acceptor::default();
    loop {
        let read = if buf.is_empty() {
            match tokio::time::timeout(FIRST_BYTE_WAIT, io.read(&mut chunk)).await {
                Ok(r) => r,
                Err(_) => return (buf, None),
            }
        } else {
            io.read(&mut chunk).await
        };
        let n = match read {
            Ok(0) | Err(_) => return (buf, None),
            Ok(n) => n,
        };
        if buf.is_empty() && chunk[0] != 0x16 {
            buf.extend_from_slice(&chunk[..n]);
            return (buf, None);
        }
        buf.extend_from_slice(&chunk[..n]);
        let mut cursor = io::Cursor::new(&chunk[..n]);
        if acceptor.read_tls(&mut cursor).is_err() {
            return (buf, None);
        }
        match acceptor.accept() {
            Ok(Some(accepted)) => {
                let hello = accepted.client_hello();
                let sni = hello.server_name().map(str::to_string);
                let alpn = hello
                    .alpn()
                    .map(|it| it.map(<[u8]>::to_vec).collect())
                    .unwrap_or_default();
                return (buf, Some(Hello { sni, alpn }));
            }
            Ok(None) if buf.len() < HELLO_MAX => continue,
            _ => return (buf, None),
        }
    }
}

/// 先吐出已经读走的 `prefix`，再接着读底下的流（写直接给底下）。
struct Rewind<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> Rewind<S> {
    fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Rewind<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(out.remaining());
            let start = self.pos;
            out.put_slice(&self.prefix[start..start + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, out)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Rewind<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[derive(Clone, Copy)]
enum Dir {
    Up,
    Down,
}

/// 一次请求的记录。两边的正文（[`Tee`]）和处理函数各拿一份；最后一份放掉时（响应读完、
/// 或者连接断了）补上耗时、交给 [`Sink`]——所以哪条路走完都只记一次。
struct Shared {
    flow: Mutex<Flow>,
    sink: Sink,
    t0: Instant,
}

impl Shared {
    fn new(sink: Sink, flow: Flow) -> Self {
        Self {
            flow: Mutex::new(flow),
            sink,
            t0: Instant::now(),
        }
    }

    fn push(&self, dir: Dir, chunk: &[u8], cap: usize) {
        let mut f = self.flow.lock().unwrap();
        match dir {
            Dir::Up => {
                f.bytes_up += chunk.len() as u64;
                if let Some(m) = f.request.as_mut() {
                    m.body.push(chunk, cap);
                }
            }
            Dir::Down => {
                f.bytes_down += chunk.len() as u64;
                if let Some(m) = f.response.as_mut() {
                    m.body.push(chunk, cap);
                }
            }
        }
    }

    fn respond(&self, status: u16, headers: Vec<(String, String)>) {
        let mut f = self.flow.lock().unwrap();
        f.status = Some(status);
        f.response = Some(Message {
            headers,
            body: Captured::default(),
        });
    }

    fn fail(&self, error: String) {
        self.flow.lock().unwrap().error = Some(error);
    }

    fn upgraded(&self, up: u64, down: u64) {
        let mut f = self.flow.lock().unwrap();
        f.bytes_up += up;
        f.bytes_down += down;
        f.note = Some("升级成了 WebSocket（之后的帧只计字节）".into());
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        let mut flow = std::mem::replace(
            self.flow.get_mut().unwrap(),
            Flow {
                tag: String::new(),
                kind: "http",
                scheme: "http",
                host: String::new(),
                port: 0,
                method: None,
                path: None,
                status: None,
                request: None,
                response: None,
                bytes_up: 0,
                bytes_down: 0,
                started_ms: 0,
                duration_ms: 0,
                decrypted: false,
                note: None,
                error: None,
            },
        );
        flow.duration_ms = self.t0.elapsed().as_millis() as u64;
        (self.sink)(flow);
    }
}

/// 边转边抄的正文。
struct Tee<B> {
    inner: B,
    shared: Arc<Shared>,
    dir: Dir,
    cap: usize,
}

impl<B> Tee<B> {
    fn new(inner: B, shared: Arc<Shared>, dir: Dir, cap: usize) -> Self {
        Self {
            inner,
            shared,
            dir,
            cap,
        }
    }
}

impl<B> Body for Tee<B>
where
    B: Body<Data = Bytes, Error = hyper::Error> + Unpin,
{
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
        let polled = Pin::new(&mut self.inner).poll_frame(cx);
        if let Poll::Ready(Some(Ok(frame))) = &polled {
            if let Some(data) = frame.data_ref() {
                self.shared.push(self.dir, data, self.cap);
            }
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

fn authority(uri: &Uri, default_port: u16) -> Option<(String, u16)> {
    let host = uri
        .host()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = uri.port_u16().unwrap_or(match uri.scheme_str() {
        Some("https") => 443,
        _ => default_port,
    });
    Some((host, port))
}

fn empty() -> ProxyBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

fn text(status: StatusCode, msg: &str) -> Response<ProxyBody> {
    let mut resp = Response::new(
        Full::new(Bytes::from(msg.to_string()))
            .map_err(|never| match never {})
            .boxed(),
    );
    *resp.status_mut() = status;
    resp
}

/// 最小的 base64（上游代理的 Basic 认证用；不为这一处加依赖）。
mod base64_lite {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    pub fn encode(input: &[u8]) -> String {
        let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
        for chunk in input.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_proxy_parses_auth_and_no_proxy() {
        let up = Upstream::parse("http://u:p@10.0.0.1:7890/", "localhost, .corp.example").unwrap();
        assert_eq!((up.host.as_str(), up.port), ("10.0.0.1", 7890));
        assert_eq!(up.auth.as_deref(), Some("Basic dTpw"));
        assert!(up.bypass("git.corp.example"));
        assert!(up.bypass("LOCALHOST"));
        assert!(!up.bypass("github.com"));
        assert!(Upstream::parse("socks5://127.0.0.1:1080", "").is_none());
        assert_eq!(base64_lite::encode(b"hello"), "aGVsbG8=");
    }
}
