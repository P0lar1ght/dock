//! 出口代理发出来的一条记录（[`Flow`]）：一次 HTTP 请求，或一条没解密的隧道。

use serde::Serialize;
use sha2::{Digest, Sha256};

/// 一次出网。解密了的 HTTPS / 明文 HTTP 是一次请求一条；没解密的是一条连接一条。
#[derive(Debug, Clone, Serialize)]
pub struct Flow {
    /// 这条流量属于谁（spine 开监听口时给的：会话、工具调用、命令）。
    pub tag: String,
    /// `http`（看得到请求）或 `tunnel`（只有字节数）。
    pub kind: &'static str,
    /// `https` / `http` / `tcp`。
    pub scheme: &'static str,
    pub host: String,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<Message>,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub started_ms: u64,
    pub duration_ms: u64,
    /// HTTPS 在代理处解密了没有。
    pub decrypted: bool,
    /// 没解密的原因、升级成了 WebSocket 之类。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 请求或响应的头和正文。
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub headers: Vec<(String, String)>,
    pub body: Captured,
}

/// 正文：总长度 + 开头最多 `cap` 字节（存成单独的块，记录里只放它的 sha256）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct Captured {
    pub size: u64,
    pub truncated: bool,
    /// 抓到的那段的 sha256（[`crate::store`] 写块时填）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip)]
    pub sample: Vec<u8>,
}

impl Captured {
    pub fn push(&mut self, chunk: &[u8], cap: usize) {
        self.size += chunk.len() as u64;
        let room = cap.saturating_sub(self.sample.len());
        if room < chunk.len() {
            self.truncated = true;
        }
        self.sample
            .extend_from_slice(&chunk[..room.min(chunk.len())]);
    }
}

/// 值要遮住的头（凭据）。遮住后留长度和 sha256 的前 12 位：能比对是不是同一个，看不到原文。
const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "x-auth-token",
    "x-goog-api-key",
];

pub fn headers(map: &http::HeaderMap, redact: bool) -> Vec<(String, String)> {
    map.iter()
        .map(|(k, v)| {
            let name = k.as_str().to_string();
            let raw = String::from_utf8_lossy(v.as_bytes()).into_owned();
            let value = if redact && SECRET_HEADERS.contains(&name.as_str()) {
                redacted(&raw)
            } else {
                raw
            };
            (name, value)
        })
        .collect()
}

/// 同 [`headers`]，给不是 `http::HeaderMap` 的来源（浏览器 CDP 的头是 JSON 对象）。
pub fn header_pairs(
    pairs: impl IntoIterator<Item = (String, String)>,
    redact: bool,
) -> Vec<(String, String)> {
    pairs
        .into_iter()
        .map(|(k, v)| {
            let name = k.to_ascii_lowercase();
            if redact && SECRET_HEADERS.contains(&name.as_str()) {
                (name, redacted(&v))
            } else {
                (name, v)
            }
        })
        .collect()
}

pub fn redacted(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("[redacted len={} sha256={hex}]", raw.len())
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_keeps_the_head_and_counts_the_rest() {
        let mut c = Captured::default();
        c.push(b"hello ", 8);
        c.push(b"world", 8);
        assert_eq!(c.size, 11);
        assert!(c.truncated);
        assert_eq!(c.sample, b"hello wo");
    }

    #[test]
    fn secrets_are_masked_but_comparable() {
        let mut map = http::HeaderMap::new();
        map.insert("authorization", "Bearer sk-123".parse().unwrap());
        map.insert("accept", "*/*".parse().unwrap());
        let out = headers(&map, true);
        let auth = &out.iter().find(|(k, _)| k == "authorization").unwrap().1;
        assert!(auth.starts_with("[redacted len=13 sha256="), "{auth}");
        assert!(!auth.contains("sk-123"));
        assert_eq!(auth, &redacted("Bearer sk-123"));
        assert!(out.contains(&("accept".into(), "*/*".into())));
        let raw = headers(&map, false);
        assert!(raw.contains(&("authorization".into(), "Bearer sk-123".into())));
    }
}
