//! Rhai 脚本自带的 HTTP 能力：`http_request(#{ ... }) -> #{ ... }`。
//!
//! **不经过工具表**。动态插件要能只靠脚本本身封装一个 API——先去依赖
//! `web_fetch` 之类现成工具的话，模型就得同时懂两套东西，而且 `web_fetch`
//! 只有 GET、不能带 header、正文还会被转成 markdown，根本封装不了带认证的接口。
//!
//! 安全边界：
//!
//! - **SSRF** 复用 [`crate::tools::web_fetch::ssrf`] 的同一份策略。不另写一份——
//!   策略分叉成两份，改一处漏一处。
//! - **不跟随重定向**（`reqwest::redirect::Policy::none()`），与 `web_fetch` 的
//!   client 侧一致。SSRF / 权限只校验初始 URL；若自动跟 302，攻击者可用公开域
//!   跳到 `127.0.0.1` / 元数据 IP。3xx 原样返回，由调用方决定是否再请求。
//! - **权限**走 `"permissions"` named service，与 `bash` 同级。按 **host** 记
//!   「始终允许」，所以一颗插件访问同一个域只问一次，不会变成每次调用都弹窗，
//!   也不会因为允许了一个域就放开全网。
//!
//! 社区有 [`rhai-http`](https://crates.io/crates/rhai-http)，这里没有采用：它依赖
//! `reqwest 0.13`（本仓是 0.12，会拖进第二份 hyper/TLS 栈），且不带 SSRF、权限、
//! 大小上限。请求 map 的形状借用了它，headers 改成 map（它用的是
//! `["Name: value"]` 数组，在 Rhai 里不如 map 顺手）。

use std::time::Duration;

use cordis::Context;
use rhai::{Dynamic, Engine, EvalAltResult, Map};
use url::Url;

use crate::host::permissions::Permissions;
use crate::names::PERMISSIONS;
use crate::tools::web_fetch::{ssrf, WebFetchParams};

/// 与 `web_fetch` 的 `MAX_URL_LENGTH` 同值，安全边界，不可配。
const MAX_URL_LEN: usize = 2000;
const MAX_REQUEST_BODY: usize = 1024 * 1024;
const MAX_RESPONSE_BODY: usize = 4 * 1024 * 1024;
const MAX_HEADERS: usize = 32;
const MAX_HEADER_LEN: usize = 4096;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

pub(crate) const HTTP_BUILTIN: (&str, &str, &[&str]) = (
    "http_request",
    "Make an HTTP request from the script itself (no tool needed). Returns #{ status, ok, url, headers, body }; body is a string — use parse_json(body) for JSON APIs. 4xx/5xx come back normally (check .ok / .status); network, SSRF and permission failures throw. Private/loopback addresses are blocked, and the first call to a host asks for permission like bash.",
    &[
        "http_request(#{ url }) -> #{ status, ok, url, headers, body }",
        "http_request(#{ method: \"POST\", url, headers: #{ \"Authorization\": \"Bearer …\" }, body: to_json(#{ … }), timeout_secs: 30 })",
    ],
);

/// 一次请求的规格，从 Rhai map 解出来之后就只剩已校验的值。
#[derive(Debug)]
struct RequestSpec {
    method: reqwest::Method,
    url: Url,
    headers: Vec<(String, String)>,
    body: Option<String>,
    timeout: Duration,
}

pub(crate) fn register(
    engine: &mut Engine,
    ctx: Context,
    plugin_id: String,
    params: WebFetchParams,
) {
    engine.register_fn(
        "http_request",
        move |spec: Map| -> Result<Map, Box<EvalAltResult>> {
            let spec = parse_spec(&spec).map_err(err)?;
            run_blocking(&ctx, &plugin_id, &params, spec).map_err(err)
        },
    );
}

fn err(message: String) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        message.into(),
        rhai::Position::NONE,
    ))
}

fn parse_spec(spec: &Map) -> Result<RequestSpec, String> {
    let raw_url = str_field(spec, "url").ok_or_else(|| "http_request needs `url`".to_string())?;
    if raw_url.len() > MAX_URL_LEN {
        return Err(format!("url exceeds {MAX_URL_LEN} chars"));
    }
    let url = Url::parse(&raw_url).map_err(|e| format!("bad url {raw_url:?}: {e}"))?;
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(format!("url scheme must be http or https, got {other:?}")),
    }
    // URL 里带账密直接拒，和 `web_fetch::validate_url` 一致：凭证该走 header。
    if !url.username().is_empty() || url.password().is_some() {
        return Err("credentials in the url are not allowed; use `headers` instead".into());
    }

    let method_raw = str_field(spec, "method").unwrap_or_else(|| "GET".into());
    let method_up = method_raw.trim().to_ascii_uppercase();
    if !METHODS.contains(&method_up.as_str()) {
        return Err(format!(
            "method {method_raw:?} is not supported; use one of {}",
            METHODS.join(", ")
        ));
    }
    let method = reqwest::Method::from_bytes(method_up.as_bytes())
        .map_err(|e| format!("bad method {method_up:?}: {e}"))?;

    let headers = parse_headers(spec)?;

    let body = match spec.get("body") {
        None => None,
        Some(v) if v.is_unit() => None,
        Some(v) => {
            let text = dynamic_text(v);
            if text.len() > MAX_REQUEST_BODY {
                return Err(format!("request body exceeds {MAX_REQUEST_BODY} bytes"));
            }
            Some(text)
        }
    };

    let timeout = spec
        .get("timeout_secs")
        .and_then(|v| v.as_int().ok())
        .map(|n| n.clamp(1, MAX_TIMEOUT_SECS as i64) as u64)
        .unwrap_or(DEFAULT_TIMEOUT_SECS);

    Ok(RequestSpec {
        method,
        url,
        headers,
        body,
        timeout: Duration::from_secs(timeout),
    })
}

fn parse_headers(spec: &Map) -> Result<Vec<(String, String)>, String> {
    let Some(raw) = spec.get("headers") else {
        return Ok(Vec::new());
    };
    if raw.is_unit() {
        return Ok(Vec::new());
    }
    let map = raw
        .read_lock::<Map>()
        .ok_or_else(|| "headers must be a map #{ \"Name\": \"value\" }".to_string())?;
    if map.len() > MAX_HEADERS {
        return Err(format!("too many headers (max {MAX_HEADERS})"));
    }
    let mut out = Vec::with_capacity(map.len());
    for (name, value) in map.iter() {
        let value = dynamic_text(value);
        if name.len() + value.len() > MAX_HEADER_LEN {
            return Err(format!("header {name:?} is too long"));
        }
        // 头里夹 CR/LF 就是请求拆分，挡在拼进请求之前。
        if value.contains(['\r', '\n']) || name.contains(['\r', '\n']) {
            return Err(format!("header {name:?} must not contain newlines"));
        }
        out.push((name.to_string(), value));
    }
    Ok(out)
}

fn str_field(spec: &Map, key: &str) -> Option<String> {
    spec.get(key)
        .filter(|v| !v.is_unit())
        .map(dynamic_text)
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
}

fn dynamic_text(value: &Dynamic) -> String {
    value
        .read_lock::<rhai::ImmutableString>()
        .map(|s| s.to_string())
        .unwrap_or_else(|| value.to_string())
}

/// Rhai 是同步的，这里和 `host.call_tool` 用同一套桥：`block_in_place` +
/// `block_on`，别把 runtime 的 worker 掐死。
fn run_blocking(
    ctx: &Context,
    plugin_id: &str,
    params: &WebFetchParams,
    spec: RequestSpec,
) -> Result<Map, String> {
    let handle = tokio::runtime::Handle::try_current().map_err(|e| e.to_string())?;
    let ctx = ctx.clone();
    let plugin_id = plugin_id.to_string();
    let params = params.clone();
    tokio::task::block_in_place(|| {
        handle.block_on(async move { send(&ctx, &plugin_id, &params, spec).await })
    })
}

async fn send(
    ctx: &Context,
    plugin_id: &str,
    params: &WebFetchParams,
    spec: RequestSpec,
) -> Result<Map, String> {
    ssrf::check_ssrf(&spec.url, params.allow_local(), params.via_proxy())
        .await
        .map_err(|e| e.to_string())?;

    let host = spec.url.host_str().unwrap_or_default().to_string();
    // 权限按 host 记，不按 "http_request" 记：允许一个域不该等于放开全网。
    if let Some(perms) = ctx.get::<Permissions>(PERMISSIONS) {
        let gate = format!("http_request {host}");
        // TUI 摘要去掉 query/fragment，避免 token 落进权限确认文案。
        let mut summary_url = spec.url.clone();
        summary_url.set_query(None);
        summary_url.set_fragment(None);
        let summary = format!(
            "{} {} （插件 {plugin_id}）",
            spec.method.as_str(),
            summary_url.as_str()
        );
        if !perms.request(&gate, &summary).await {
            return Err(format!("权限被拒绝：{} {}", spec.method.as_str(), host));
        }
    }

    // 与 web_fetch 一致：不自动跟重定向。SSRF 只校验初始 URL，跟跳会绕过。
    let mut builder = reqwest::Client::builder()
        .timeout(spec.timeout)
        .redirect(reqwest::redirect::Policy::none());
    if let Some(proxy) = params.proxy_endpoint.as_deref() {
        builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|e| e.to_string())?);
    }
    let client = builder.build().map_err(|e| e.to_string())?;

    let mut req = client.request(spec.method.clone(), spec.url.clone());
    for (name, value) in &spec.headers {
        req = req.header(name.as_str(), value.as_str());
    }
    if let Some(body) = spec.body {
        req = req.body(body);
    }
    let resp = req.send().await.map_err(|e| format!("http: {e}"))?;

    let status = resp.status();
    let final_url = resp.url().to_string();
    let mut headers = Map::new();
    for (name, value) in resp.headers().iter() {
        if let Ok(v) = value.to_str() {
            headers.insert(name.as_str().into(), Dynamic::from(v.to_string()));
        }
    }
    let bytes = resp.bytes().await.map_err(|e| format!("http body: {e}"))?;
    if bytes.len() > MAX_RESPONSE_BODY {
        return Err(format!(
            "response body exceeds {MAX_RESPONSE_BODY} bytes ({} received)",
            bytes.len()
        ));
    }
    let body = String::from_utf8_lossy(&bytes).to_string();

    let mut out = Map::new();
    out.insert("status".into(), Dynamic::from(status.as_u16() as i64));
    out.insert("ok".into(), Dynamic::from(status.is_success()));
    out.insert("url".into(), Dynamic::from(final_url));
    out.insert("headers".into(), Dynamic::from(headers));
    out.insert("body".into(), Dynamic::from(body));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    fn map(pairs: &[(&str, Dynamic)]) -> Map {
        let mut m = Map::new();
        for (k, v) in pairs {
            m.insert((*k).into(), v.clone());
        }
        m
    }

    fn url_spec(url: &str) -> Map {
        map(&[("url", Dynamic::from(url.to_string()))])
    }

    #[test]
    fn rejects_non_http_schemes() {
        let err = parse_spec(&url_spec("file:///etc/passwd")).unwrap_err();
        assert!(err.contains("scheme"), "{err}");
    }

    #[test]
    fn rejects_credentials_in_url() {
        let err = parse_spec(&url_spec("https://user:pw@example.com/")).unwrap_err();
        assert!(err.contains("credentials"), "{err}");
    }

    #[test]
    fn rejects_unknown_methods() {
        let spec = map(&[
            ("url", Dynamic::from("https://example.com".to_string())),
            ("method", Dynamic::from("TRACE".to_string())),
        ]);
        let err = parse_spec(&spec).unwrap_err();
        assert!(err.contains("not supported"), "{err}");
    }

    /// 头里夹 CRLF 就是请求拆分，必须在拼进请求之前挡掉。
    #[test]
    fn rejects_crlf_in_headers() {
        let spec = map(&[
            ("url", Dynamic::from("https://example.com".to_string())),
            (
                "headers",
                Dynamic::from(map(&[(
                    "X-Evil",
                    Dynamic::from("a\r\nX-Injected: 1".to_string()),
                )])),
            ),
        ]);
        let err = parse_spec(&spec).unwrap_err();
        assert!(err.contains("newlines"), "{err}");
    }

    #[test]
    fn timeout_is_clamped() {
        let spec = map(&[
            ("url", Dynamic::from("https://example.com".to_string())),
            ("timeout_secs", Dynamic::from(9999_i64)),
        ]);
        let got = parse_spec(&spec).unwrap();
        assert_eq!(got.timeout, Duration::from_secs(MAX_TIMEOUT_SECS));
    }

    #[test]
    fn method_defaults_to_get() {
        let got = parse_spec(&url_spec("https://example.com")).unwrap();
        assert_eq!(got.method, reqwest::Method::GET);
    }

    /// 起一个最小的 HTTP/1.1 服务端，别为测试拖一个 mock 框架进来。
    fn spawn_echo_server() -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            if content_length > 0 {
                let mut body = vec![0u8; content_length];
                std::io::Read::read_exact(&mut reader, &mut body).unwrap();
                request.push_str(&String::from_utf8_lossy(&body));
            }
            let payload = br#"{"hello":"world","n":7}"#;
            let resp = format!(
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            stream.write_all(resp.as_bytes()).unwrap();
            stream.write_all(payload).unwrap();
            stream.flush().unwrap();
            request
        });
        (format!("http://{addr}/api"), handle)
    }

    fn local_params() -> WebFetchParams {
        WebFetchParams {
            allow_local: Some(true),
            ..WebFetchParams::default()
        }
    }

    /// 走通整条：方法、自定义 header、请求体、状态码、响应头、正文。
    #[tokio::test(flavor = "multi_thread")]
    async fn round_trips_method_headers_and_body() {
        let (url, server) = spawn_echo_server();
        let ctx = Context::new();
        let spec = parse_spec(&map(&[
            ("url", Dynamic::from(url)),
            ("method", Dynamic::from("POST".to_string())),
            ("body", Dynamic::from(r#"{"a":1}"#.to_string())),
            (
                "headers",
                Dynamic::from(map(&[("X-Token", Dynamic::from("s3cret".to_string()))])),
            ),
        ]))
        .unwrap();

        let out = send(&ctx, "probe-1", &local_params(), spec).await.unwrap();
        assert_eq!(out.get("status").unwrap().as_int().unwrap(), 201);
        assert!(out.get("ok").unwrap().as_bool().unwrap(), "201 属于 2xx");
        let body = dynamic_text(out.get("body").unwrap());
        assert!(body.contains("\"hello\":\"world\""), "{body}");
        let headers = out.get("headers").unwrap().read_lock::<Map>().unwrap();
        assert_eq!(
            dynamic_text(headers.get("content-type").unwrap()),
            "application/json"
        );

        let seen = server.join().unwrap();
        assert!(seen.starts_with("POST /api "), "{seen}");
        assert!(
            seen.contains("x-token: s3cret") || seen.contains("X-Token: s3cret"),
            "{seen}"
        );
        assert!(seen.contains(r#"{"a":1}"#), "{seen}");
    }

    /// 默认策略下环回地址要被 SSRF 拦住——这是 `allow_local` 关着时的样子。
    #[tokio::test(flavor = "multi_thread")]
    async fn loopback_is_blocked_without_allow_local() {
        let ctx = Context::new();
        let spec = parse_spec(&url_spec("http://127.0.0.1:9/nope")).unwrap();
        let err = send(&ctx, "probe-1", &WebFetchParams::default(), spec)
            .await
            .unwrap_err();
        assert!(err.contains("SSRF"), "{err}");
    }

    /// 302 → 环回：client 不得自动跟随，否则 SSRF 只校验初始 URL 会被绕过。
    fn spawn_redirect_to(target: &str) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let location = target.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                if line == "\r\n" {
                    break;
                }
            }
            let body = b"redirected";
            let resp = format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(resp.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
            stream.flush().unwrap();
        });
        (format!("http://{addr}/start"), handle)
    }

    /// 第二跳监听器：若被跟到会置位；Policy::none 下应保持 false。
    fn spawn_probe_listener() -> (String, std::sync::Arc<std::sync::atomic::AtomicBool>, std::thread::JoinHandle<()>) {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hit = Arc::new(AtomicBool::new(false));
        let hit2 = hit.clone();
        // 短超时 accept，避免测试卡住；无人来就正常结束。
        listener.set_nonblocking(true).unwrap();
        let handle = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        hit2.store(true, Ordering::SeqCst);
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        );
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        (format!("http://{addr}/secret"), hit, handle)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn does_not_follow_redirects() {
        use std::sync::atomic::Ordering;
        let (probe_url, probe_hit, probe_server) = spawn_probe_listener();
        let (url, redirect_server) = spawn_redirect_to(&probe_url);
        let ctx = Context::new();
        let spec = parse_spec(&url_spec(&url)).unwrap();

        let out = send(&ctx, "probe-1", &local_params(), spec).await.unwrap();
        assert_eq!(out.get("status").unwrap().as_int().unwrap(), 302);
        assert!(!out.get("ok").unwrap().as_bool().unwrap(), "302 不是 2xx");
        let body = dynamic_text(out.get("body").unwrap());
        assert_eq!(body, "redirected");
        let headers = out.get("headers").unwrap().read_lock::<Map>().unwrap();
        let location = dynamic_text(headers.get("location").unwrap());
        assert_eq!(location, probe_url);

        redirect_server.join().unwrap();
        probe_server.join().unwrap();
        assert!(
            !probe_hit.load(Ordering::SeqCst),
            "redirect Location must not be fetched"
        );
    }
}
