//! OAuth 的共用原语：PKCE、随机串、回环回调、私有文件的原子写、跨进程锁。
//!
//! 订阅登录（`auth::chatgpt`）和 MCP 的 OAuth（`tools::mcp::oauth`）都用这一份。
//! 以前两边各抄一份，修了一边的坑另一边还在：回调只接第一个连接、临时文件先按
//! 默认权限写再 chmod、刷新不加锁。这里**不挂插件**，也不认识 ctx 键。

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::tools::fs_perms::ensure_owner_only;

pub(crate) use super::callback_page::{CallbackPage, PageOutcome, Tone};

/// PKCE verifier（32 字节随机，base64url）。
pub(crate) fn pkce_verifier() -> String {
    random_urlsafe(32)
}

/// PKCE challenge：`S256` = base64url(sha256(verifier))。
pub(crate) fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// `nbytes` 字节的随机数，base64url 编码（state / nonce 用）。
pub(crate) fn random_urlsafe(nbytes: usize) -> String {
    let mut buf = vec![0u8; nbytes];
    getrandom::getrandom(&mut buf).expect("getrandom");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

pub(crate) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 查询串 → 键值表（`application/x-www-form-urlencoded`：`+` 是空格，`%xx` 解码）。
pub(crate) fn parse_query(qs: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(qs.as_bytes())
        .into_owned()
        .collect()
}

/// 回调不成立时，浏览器那一页显示什么。默认「没有连上 {service}」+ 这条错误的原文；
/// 调用方可以给结构化的页面（一句原因 + 接下来可以怎么做）。
pub(crate) trait CallbackFailure: std::fmt::Display + From<String> {
    fn page(&self, service: &str) -> PageOutcome {
        PageOutcome::failed(service, self.to_string())
    }
}

impl CallbackFailure for String {}

/// 等一次授权回调。
///
/// **只认 `path` 这一个路径**：别的请求（浏览器的 favicon、用户误敲的地址、别的
/// 进程扫端口）回 404 就继续等。只接第一个连接的话，随便一个无关请求先打进来，
/// 这次登录就废了——用户得重新点一次授权。
///
/// `parse` 看回调参数决定结果：
/// - `Err`：一次回完整的「没有连上 {service}」页（带原因），返回这个错；
/// - `Ok`：先回**前半页**（「正在连接 {service}…」），连接留着，连同 [`CallbackPage`]
///   一起交给调用方——换码有了结果，调用方 [`CallbackPage::finish`] 把页面切过去。
///   页面在换码之前就发出去了，所以前半页不能说「完成」。
pub(crate) async fn accept_callback<T, E, F>(
    listener: tokio::net::TcpListener,
    path: &str,
    service: &str,
    parse: F,
) -> Result<(T, CallbackPage), E>
where
    F: Fn(&HashMap<String, String>) -> Result<T, E>,
    E: CallbackFailure,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| E::from(format!("等回调失败：{e}")))?;
        let mut buf = vec![0u8; 8192];
        let n = match stream.read(&mut buf).await {
            Ok(n) => n,
            // 一条连接读坏了不代表授权失败：接着等真正的回调。
            Err(_) => continue,
        };
        let req = String::from_utf8_lossy(&buf[..n]);
        let target = req
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        let (route, qs) = target.split_once('?').unwrap_or((target, ""));
        if route != path {
            write_page(
                &mut stream,
                "404 Not Found",
                "text/plain",
                "not the callback endpoint",
            )
            .await;
            let _ = stream.shutdown().await;
            continue;
        }
        return match parse(&parse_query(qs)) {
            Ok(value) => Ok((value, CallbackPage::open(stream, service).await)),
            Err(e) => {
                CallbackPage::reject(stream, service, &e.page(service)).await;
                Err(e)
            }
        };
    }
}

async fn write_page(stream: &mut tokio::net::TcpStream, status: &str, kind: &str, body: &str) {
    use tokio::io::AsyncWriteExt;
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

/// 原子写一个只有本人能读的文件：同目录临时文件（**创建时就是 0600**）→ fsync → rename。
///
/// 先按默认权限写、事后 chmod 的话，中间那一瞬文件是 0644 的——同机器上别的用户
/// 能读到 token。临时文件名带随机后缀、`create_new`：不会撞上一份残留的、权限更松的旧文件。
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let tmp = parent.join(format!(".{name}.{}.tmp", random_urlsafe(6)));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        // umask 只会让权限更紧，不会更松；这里再钉一次，跨平台语义一致。
        ensure_owner_only(&tmp)?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    ensure_owner_only(path)
}

/// 跨进程锁：刷新 / 登出 / 改凭据文件时用。
///
/// **为什么需要**：refresh token 每次刷新都轮换，旧的那份立刻作废。两个进程
/// 同时刷会有一方撞上 `refresh_token_reused`（凭据直接废掉）。所以刷新全程
/// 持锁，锁内再读一次盘上的凭据。
///
/// 用**文件锁**（flock）：进程崩了锁由内核释放，不用猜「是不是陈旧锁」。
/// 早先用「创建独占文件 + 看修改时间」那套：超过 30 秒就把锁当陈旧的删掉——
/// 慢一点的刷新正会被别人当成陈旧锁删掉，两个进程一起刷，refresh token 直接
/// 作废。文件锁没有这个坑：谁在持锁谁说了算，释放即消失。
pub struct FileLock {
    /// 锁就挂在这个文件上：**文件必须活到锁释放**（drop 时关闭即解锁）。
    #[allow(dead_code)]
    file: std::fs::File,
}

impl FileLock {
    pub async fn acquire(path: &Path, timeout: Duration) -> std::io::Result<FileLock> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(path)?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(FileLock { file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("等锁超时：{}", path.display()),
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256() {
        // RFC 7636 附录 B 的样例。
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(v),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn query_parsing_handles_percent_escapes() {
        let q = parse_query("code=a%2Fb&state=x+y");
        assert_eq!(q["code"], "a/b");
        assert_eq!(q["state"], "x y");
    }

    fn take_code(p: &HashMap<String, String>) -> Result<String, String> {
        p.get("code")
            .cloned()
            .ok_or_else(|| "没有 code".to_string())
    }

    /// 无关请求（favicon、路径前缀相同的别的地址）回 404 并**接着等**，真正的回调
    /// 来了照样收。早先只接第一个连接：随便一个请求先到，这次登录就废了。
    #[tokio::test]
    async fn unrelated_requests_do_not_consume_the_callback() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(accept_callback(
            listener,
            "/callback",
            "测试服务",
            take_code,
        ));

        for path in ["/favicon.ico", "/callbackx?code=nope"] {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).await.unwrap();
            assert!(out.starts_with("HTTP/1.1 404"), "{path}: {out}");
        }

        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /callback?code=abc&state=s HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let (code, page) = server.await.unwrap().unwrap();
        assert_eq!(code, "abc");
        // 换码有了结果才把页面切过去。
        page.finish(&PageOutcome::connected("测试服务").detail("说明"))
            .await;
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert!(out.starts_with("HTTP/1.1 200"), "{out}");
        let (first, last) = out.split_once("<script>settle(").expect("后半页补上了结果");
        // 前半页在换码之前就发了：只说「正在连接」，不说「完成」。
        assert!(first.contains("正在连接 测试服务"), "{first}");
        assert!(
            last.contains("已连接 测试服务") && last.contains("说明"),
            "{last}"
        );
        assert!(out.trim_end().ends_with("</html>"), "{out}");
    }

    /// 调用方没给结果就丢了页面（登录被取消、出错）：连接关掉、页面只有前半页——
    /// 页面那头据此显示「这次登录中断了」，而不是一直转圈。
    #[tokio::test]
    async fn a_dropped_page_closes_without_a_result() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(accept_callback(
            listener,
            "/callback",
            "测试服务",
            take_code,
        ));
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /callback?code=abc HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let (_, page) = server.await.unwrap().unwrap();
        drop(page);
        let mut out = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            s.read_to_string(&mut out),
        )
        .await
        .expect("连接要关，不能让浏览器一直等")
        .unwrap();
        assert!(out.contains("正在连接 测试服务"), "{out}");
        assert!(!out.contains("<script>settle("), "{out}");
        assert!(
            out.contains("这次登录中断了"),
            "中断的兜底文案在前半页里：{out}"
        );
    }

    /// 回调本身不对（没有 code）：回「授权失败」页，并把原因交给调用方。
    #[tokio::test]
    async fn a_bad_callback_returns_the_reason() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(accept_callback(
            listener,
            "/callback",
            "测试服务",
            take_code,
        ));
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /callback?state=s HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert!(out.starts_with("HTTP/1.1 400"), "{out}");
        let (_, last) = out.split_once("<script>settle(").expect("一次发完整页");
        assert!(
            last.contains("没有连上 测试服务") && last.contains("没有 code"),
            "{last}"
        );
        assert_eq!(server.await.unwrap().unwrap_err(), "没有 code");
    }

    /// 写出来的文件是 0600，目录里不留临时文件；旧文件权限再松也会被整份换掉。
    #[test]
    fn write_private_replaces_the_file_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("creds.json");
        std::fs::write(&path, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        write_private(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
