//! MCP App 界面的独立源（同官方 ext-apps 参考宿主的「沙箱代理」）。
//!
//! srcdoc + 只给 allow-scripts 的 iframe 是不透明源（origin "null"）：localStorage 抛错、
//! 一些库的 Worker 起不来（官方 map 示例的 Cesium 一张瓦片都取不到）。这里给每台 MCP 服务器
//! 一个自己的源 `mcpapp://<源键>`，界面文档由本进程按 id 吐出，iframe 就能带
//! `allow-same-origin`——源和 GUI 自己的不同，碰不到 GUI 的 DOM 和存储。
//!
//! - 前端先 `mcp_app_doc { origin, html, csp }` 存一份，拿回 URL 给 iframe 的 src；
//! - CSP 走响应头（文档里的 meta 也留着）；只认存过的文档，别的路径一律 404；
//! - 最多留 64 份（界面重建会再存一份），旧的丢掉。
//!
//! Windows 上 WebView2 的自定义协议是 `http://mcpapp.localhost/…`：所有界面同一个源，
//! 仍和 GUI 不同源，只是 App 之间不再互相隔开。

use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext};

pub const SCHEME: &str = "mcpapp";
const MAX_DOCS: usize = 64;
/// 单份文档上限：界面资源本来就由 Dock 读来，这里只防异常大。
const MAX_DOC_BYTES: usize = 32 * 1024 * 1024;

struct Doc {
    key: String,
    html: Arc<String>,
    csp: String,
}

#[derive(Clone, Default)]
pub struct AppDocs {
    docs: Arc<Mutex<VecDeque<Doc>>>,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// 源键：小写字母数字和 `-`，1–63 个字符（要能当主机名的一段）。
fn valid_origin(origin: &str) -> bool {
    !origin.is_empty()
        && origin.len() <= 63
        && origin
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !origin.starts_with('-')
        && !origin.ends_with('-')
}

/// CSP 进响应头：不能带换行（会拆出别的头）。
fn valid_csp(csp: &str) -> bool {
    !csp.contains(['\r', '\n']) && csp.len() <= 16 * 1024
}

#[tauri::command]
pub fn mcp_app_doc(
    docs: tauri::State<'_, AppDocs>,
    origin: String,
    html: String,
    csp: String,
) -> Result<String, String> {
    if !valid_origin(&origin) {
        return Err("界面源键不合法".into());
    }
    if !valid_csp(&csp) {
        return Err("界面 CSP 不合法".into());
    }
    if html.len() > MAX_DOC_BYTES {
        return Err("界面文档太大".into());
    }
    let id = format!(
        "{:x}{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let key = format!("{origin}/{id}");
    let mut list = docs.docs.lock().unwrap();
    if list.len() >= MAX_DOCS {
        list.pop_front();
    }
    list.push_back(Doc {
        key,
        html: Arc::new(html),
        csp,
    });
    Ok(doc_url(&origin, &id))
}

#[cfg(windows)]
fn doc_url(origin: &str, id: &str) -> String {
    format!("http://{SCHEME}.localhost/{origin}/{id}")
}

#[cfg(not(windows))]
fn doc_url(origin: &str, id: &str) -> String {
    format!("{SCHEME}://{origin}/{id}")
}

/// 请求 → 存文档时的键（`<源键>/<id>`）。macOS / Linux：主机是源键；Windows：路径里带。
fn request_key(uri: &tauri::http::Uri) -> String {
    let path = uri.path().trim_matches('/');
    match uri.host() {
        Some(host) if host != format!("{SCHEME}.localhost") && !host.is_empty() => {
            format!("{host}/{path}")
        }
        _ => path.to_string(),
    }
}

pub fn protocol<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    let key = request_key(request.uri());
    let found = ctx.app_handle().state::<AppDocs>().docs.lock().unwrap().iter().find_map(|d| {
        (d.key == key).then(|| (d.html.clone(), d.csp.clone()))
    });
    match found {
        Some((html, csp)) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header("Content-Security-Policy", csp)
            .header(header::CACHE_CONTROL, "no-store")
            .body(Cow::Owned(html.as_bytes().to_vec()))
            .unwrap_or_else(|_| not_found()),
        None => not_found(),
    }
}

fn not_found() -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Cow::Borrowed("not found".as_bytes()))
        .expect("static response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_keys_and_csp_are_checked() {
        assert!(valid_origin("whiteboard-1a2b3c4d"));
        assert!(!valid_origin(""));
        assert!(!valid_origin("Upper"));
        assert!(!valid_origin("a.b"));
        assert!(!valid_origin("-a"));
        assert!(!valid_origin(&"a".repeat(64)));
        assert!(valid_csp("default-src 'none'; img-src data:"));
        assert!(!valid_csp("default-src 'none'\r\nSet-Cookie: x=1"));
    }

    #[test]
    fn request_keys_cover_both_url_forms() {
        let mac: tauri::http::Uri = "mcpapp://board-12345678/abc".parse().unwrap();
        assert_eq!(request_key(&mac), "board-12345678/abc");
        let win: tauri::http::Uri = "http://mcpapp.localhost/board-12345678/abc".parse().unwrap();
        assert_eq!(request_key(&win), "board-12345678/abc");
    }
}
