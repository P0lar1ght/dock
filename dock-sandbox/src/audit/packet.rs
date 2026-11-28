//! 一次出网的请求包 / 响应包，给界面像抓包工具那样原样显示：请求行 / 状态行、头、正文。
//!
//! 正文在记录里只有 sha256，内容在 `bodies/<sha256>`（最多 `body_cap_kb`）。按 `Content-Encoding`
//! 解开 gzip / deflate / br 再给（解开后最多 [`DECODED_MAX`]）；不是 UTF-8 的按二进制给 base64。
//! 凭据类的头遮不遮在记录时就定了（`[sandbox] redact_secrets`，默认不遮），这里原样给。

use std::io::Read;
use std::path::Path;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::audit::store::BODIES;

/// 解开后的正文最多给这么多（再多界面也画不动）。
pub const DECODED_MAX: usize = 1024 * 1024;

/// 一条记录的定位：`<文件名>:<序号>`（[`crate::audit::store::read`] 给每条记录带的 `source` 和 `seq`）。
pub fn record_id(event: &Value) -> Option<String> {
    Some(format!(
        "{}:{}",
        event["source"].as_str()?,
        event["seq"].as_u64()?
    ))
}

/// 按 [`record_id`] 读回一条出网记录，拼成请求包 / 响应包。不是出网记录、找不到都是 `None`。
/// WebSocket 握手还带上这条连接到目前为止的消息（`ws`，[`crate::audit::ws::conversation`]）；
/// `live` = 这份目录还在写（没记到关闭的连接才算开着）。
pub fn flow_detail(dir: &Path, id: &str, live: bool) -> Option<Value> {
    let (file, seq) = id.rsplit_once(':')?;
    let seq: u64 = seq.parse().ok()?;
    // 只认审计目录里的 jsonl，不让 `../` 之类的读到别处。
    if file.contains(['/', '\\']) || !file.ends_with(".jsonl") {
        return None;
    }
    let text = std::fs::read_to_string(dir.join(file)).ok()?;
    let records: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();
    let e = records.iter().find(|v| v["seq"].as_u64() == Some(seq))?;
    if e["kind"] != "net" {
        return None;
    }
    let bodies = dir.join(BODIES);
    // 一条连接的各段和握手是同一个写入方写的，在同一个文件里。
    let ws = e["ws"]["conn"].as_str().map(|conn| {
        let parts: Vec<&Value> = records
            .iter()
            .filter(|r| r["kind"] == "ws" && r["ws"]["conn"].as_str() == Some(conn))
            .collect();
        crate::audit::ws::conversation(&parts, &bodies, live)
    });
    let decrypted = e["decrypted"].as_bool() == Some(true);
    let request = (decrypted || e["request"].is_object()).then(|| {
        let line = format!(
            "{} {} HTTP/1.1",
            e["method"].as_str().unwrap_or("GET"),
            e["path"].as_str().unwrap_or("/")
        );
        message(&e["request"], line, &bodies)
    });
    let response = e["response"].is_object().then(|| {
        let status = e["status"].as_u64().unwrap_or(0) as u16;
        let reason = http::StatusCode::from_u16(status)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or("");
        message(
            &e["response"],
            format!("HTTP/1.1 {status} {reason}").trim_end().to_string(),
            &bodies,
        )
    });
    Some(json!({
        "id": id,
        "tag": e["tag"],
        "scheme": e["scheme"],
        "host": e["host"],
        "port": e["port"],
        "method": e["method"],
        "path": e["path"],
        "status": e["status"],
        "decrypted": decrypted,
        "note": e["note"],
        "error": e["error"],
        "startedMs": e["started_ms"],
        "durationMs": e["duration_ms"],
        "bytesUp": e["bytes_up"],
        "bytesDown": e["bytes_down"],
        "request": request,
        "response": response,
        "ws": ws,
    }))
}

/// 一边（请求或响应）：首行、头、正文。
fn message(m: &Value, line: String, bodies: &Path) -> Value {
    let headers: Vec<(String, String)> = m["headers"]
        .as_array()
        .map(|hs| {
            hs.iter()
                .filter_map(|h| {
                    Some((
                        h.get(0)?.as_str()?.to_string(),
                        h.get(1)?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    let body = &m["body"];
    let size = body["size"].as_u64().unwrap_or(0);
    // 记录里的 sha256 只认 64 位十六进制：日志要是被改过，也不能拿它去读 `bodies/` 以外的文件。
    let captured = body["sha256"]
        .as_str()
        .filter(|sha| is_sha256(sha))
        .and_then(|sha| std::fs::read(bodies.join(sha)).ok())
        .unwrap_or_default();
    // 记录的是开头一段（`body_cap_kb`）：截过的压缩数据解不全，能解多少给多少。
    let truncated = body["truncated"].as_bool() == Some(true);
    let (bytes, decoded) = decode(&captured, header("content-encoding"));
    let (text, binary) = match std::str::from_utf8(&bytes) {
        Ok(s) => (Some(s.to_string()), None),
        Err(_) => (
            None,
            Some(
                base64::engine::general_purpose::STANDARD
                    .encode(&bytes[..bytes.len().min(64 * 1024)]),
            ),
        ),
    };
    json!({
        "line": line,
        "headers": headers,
        "contentType": header("content-type"),
        "body": {
            "size": size,
            "captured": captured.len(),
            "truncated": truncated,
            "decoded": decoded,
            "text": text,
            "base64": binary,
            "shown": bytes.len(),
        },
    })
}

/// 正文块的名字：64 位小写十六进制（[`crate::audit::store`] 写的就是这样）。
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// 按 `Content-Encoding` 解开。解不开（不认识的编码、数据截断了）就给原样的字节。
/// 第二个返回值是解开了什么（`gzip` / `deflate` / `br`），没解是 `None`。
fn decode(raw: &[u8], encoding: Option<&str>) -> (Vec<u8>, Option<&'static str>) {
    let limit = |r: &mut dyn Read| -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let n = r.take(DECODED_MAX as u64).read_to_end(&mut out);
        // 截断的压缩流读到一半会报错：读到的那些照样给。
        (n.is_ok() || !out.is_empty()).then_some(out)
    };
    let enc = encoding.unwrap_or("").trim().to_ascii_lowercase();
    let decoded = match enc.as_str() {
        "gzip" | "x-gzip" => limit(&mut flate2::read::GzDecoder::new(raw)).map(|b| (b, "gzip")),
        "deflate" => limit(&mut flate2::read::ZlibDecoder::new(raw))
            .or_else(|| limit(&mut flate2::read::DeflateDecoder::new(raw)))
            .map(|b| (b, "deflate")),
        "br" => limit(&mut brotli_decompressor::Decompressor::new(raw, 4096)).map(|b| (b, "br")),
        _ => None,
    };
    match decoded {
        Some((bytes, name)) => (bytes, Some(name)),
        None => (raw[..raw.len().min(DECODED_MAX)].to_vec(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn a_flow_comes_back_as_request_and_response_packets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(BODIES)).unwrap();
        let body = gzip(br#"{"ok":true}"#);
        let sha = "a".repeat(64);
        let req_sha = "b".repeat(64);
        std::fs::write(dir.path().join(BODIES).join(&sha), &body).unwrap();
        std::fs::write(dir.path().join(BODIES).join(&req_sha), b"q=1").unwrap();
        // 日志外面的文件：被改过的记录拿 `../` 也读不到它。
        std::fs::write(dir.path().join("secret.txt"), b"top secret").unwrap();
        let line = json!({
            "kind": "net", "seq": 7, "tag": "job-1", "scheme": "https", "host": "api.example.com", "port": 443,
            "method": "POST", "path": "/v1/x?y=1", "status": 201, "decrypted": true,
            "request": { "headers": [["content-type", "application/x-www-form-urlencoded"], ["authorization", "[redacted 12 sha256:abcd]"]],
                         "body": { "size": 3, "truncated": false, "sha256": req_sha } },
            "response": { "headers": [["content-type", "application/json"], ["content-encoding", "gzip"]],
                          "body": { "size": body.len(), "truncated": false, "sha256": sha } },
        });
        std::fs::write(dir.path().join("events.jsonl"), format!("{line}\n")).unwrap();

        let d = flow_detail(dir.path(), "events.jsonl:7", false).expect("找得到");
        assert_eq!(d["request"]["line"], "POST /v1/x?y=1 HTTP/1.1");
        assert_eq!(d["request"]["headers"][1][1], "[redacted 12 sha256:abcd]");
        assert_eq!(d["request"]["body"]["text"], "q=1");
        assert_eq!(d["response"]["line"], "HTTP/1.1 201 Created");
        assert_eq!(d["response"]["body"]["decoded"], "gzip");
        assert_eq!(d["response"]["body"]["text"], r#"{"ok":true}"#);
        assert_eq!(d["response"]["contentType"], "application/json");

        assert!(
            flow_detail(dir.path(), "events.jsonl:8", false).is_none(),
            "没有这条"
        );
        assert!(
            flow_detail(dir.path(), "../x.jsonl:7", false).is_none(),
            "不出审计目录"
        );

        // 回归：sha256 字段被改成路径时不跟着读（以前直接 bodies.join(sha)）。
        let evil = json!({
            "kind": "net", "seq": 9, "host": "x", "method": "GET", "path": "/", "status": 200, "decrypted": true,
            "request": { "headers": [], "body": { "size": 10, "truncated": false, "sha256": "../secret.txt" } },
        });
        std::fs::write(dir.path().join("events.jsonl"), format!("{line}\n{evil}\n")).unwrap();
        let d = flow_detail(dir.path(), "events.jsonl:9", false).expect("记录本身还在");
        assert_eq!(d["request"]["body"]["text"], "", "不读 bodies/ 以外的文件");
    }

    #[test]
    fn binary_bodies_come_back_as_base64_and_truncated_gzip_still_shows_what_it_can() {
        let png = [0x89u8, b'P', b'N', b'G', 0, 1, 2, 0xff];
        let (bytes, decoded) = decode(&png, None);
        assert_eq!((bytes.as_slice(), decoded), (&png[..], None));
        assert!(std::str::from_utf8(&bytes).is_err());

        let long = "hello ".repeat(5000);
        let zipped = gzip(long.as_bytes());
        let (bytes, decoded) = decode(&zipped[..zipped.len() / 2], Some("gzip"));
        assert_eq!(decoded, Some("gzip"));
        assert!(
            !bytes.is_empty() && long.as_bytes().starts_with(&bytes),
            "截断的压缩数据能解多少给多少"
        );
    }

    /// WebSocket 握手那条带上这条连接的消息：从各段的正文块读回来按顺序拼好；块被删了的计进 `lost`。
    #[test]
    fn a_websocket_handshake_comes_back_with_its_messages() {
        use crate::audit::record::{Flow, Message};
        use crate::audit::store::AuditLog;
        use crate::audit::ws::{self, Dir, Op, Payload, Recorder};
        let dir = tempfile::tempdir().unwrap();
        let log = std::sync::Arc::new(AuditLog::open(dir.path(), "events.jsonl").unwrap());
        let conn = ws::conn_id();
        log.flow(Flow {
            tag: "job-1".into(),
            kind: "http",
            scheme: "wss",
            host: "chat.example".into(),
            port: 443,
            method: Some("GET".into()),
            path: Some("/live".into()),
            status: Some(101),
            request: Some(Message {
                headers: vec![("upgrade".into(), "websocket".into())],
                body: Default::default(),
            }),
            response: Some(Message {
                headers: vec![],
                body: Default::default(),
            }),
            bytes_up: 0,
            bytes_down: 0,
            started_ms: 1,
            duration_ms: 1,
            decrypted: true,
            note: None,
            error: None,
            ws: Some(ws::Ws {
                conn: conn.clone(),
                part: None,
            }),
        });
        let start = |conn: String| {
            let sink_log = log.clone();
            Recorder::start(
                std::sync::Arc::new(move |f| sink_log.flow(f)),
                conn,
                "job-1",
                "wss",
                "chat.example",
                443,
                true,
                1024,
            )
        };
        let text = |s: &'static str| Payload {
            size: s.len() as u64,
            sample: s.as_bytes(),
            ..Payload::default()
        };
        let rec = start(conn);
        // 同一个文件里另一条连接：它的消息不该串过来。
        let other = start(ws::conn_id());
        rec.message(Dir::Up, Op::Text, text("hi"));
        other.message(Dir::Up, Op::Text, text("other"));
        other.finish(None);
        rec.finish(None);
        drop((rec, other, log));
        let d = flow_detail(dir.path(), "events.jsonl:1", false).expect("握手那条");
        assert_eq!(
            (d["scheme"].as_str(), d["status"].as_u64()),
            (Some("wss"), Some(101))
        );
        let ws = &d["ws"];
        assert_eq!(ws["messages"].as_array().map(Vec::len), Some(1), "{ws}");
        assert_eq!(ws["messages"][0]["text"], "hi", "{ws}");
        assert_eq!(
            (
                ws["sent"].as_u64(),
                ws["open"].as_bool(),
                ws["lost"].as_u64()
            ),
            (Some(1), Some(false), Some(0))
        );

        // 正文块没了：消息少了，`lost` 记下少了几条；哈希校验也报出来。
        let blobs = dir.path().join(BODIES);
        for entry in std::fs::read_dir(&blobs).unwrap().flatten() {
            std::fs::remove_file(entry.path()).unwrap();
        }
        let d = flow_detail(dir.path(), "events.jsonl:1", false).unwrap();
        assert_eq!(
            (
                d["ws"]["messages"].as_array().map(Vec::len),
                d["ws"]["lost"].as_u64()
            ),
            (Some(0), Some(1))
        );
        assert!(!crate::audit::store::verify(dir.path())[0].intact());
    }
}
