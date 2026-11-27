//! 只追加的审计日志。一次审计一个目录，里面每个写入方一个 `*.jsonl`（Dock 自己写
//! `events.jsonl`，浏览器服务写 `browser.jsonl`），各自一条哈希链：每行带上一行的 sha256
//! （`prev`），改过或删过中间一行，之后的 `prev` 就对不上（[`verify`]）。正文块存在
//! `bodies/<sha256>`，同样的内容只存一份。
//!
//! 写盘在后台任务里：调用方（代理的读写路径、工具调用钩子）只往有界通道里放，满了就丢掉、
//! 计数，下一条落盘的记录里报 `dropped`——审计不能把 agent 拖慢。

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::audit::record::{now_ms, Captured, Flow};

/// 通道里最多排这么多条（之后丢弃、计数）。
const QUEUE: usize = 8192;
pub const BODIES: &str = "bodies";

enum Entry {
    Record(Map<String, Value>),
    Flow(Box<Flow>),
}

/// 一个写入方。drop 时把排着的写完。需要在 tokio 运行时里打开。
pub struct AuditLog {
    tx: mpsc::Sender<Entry>,
    dropped: Arc<AtomicU64>,
    dir: PathBuf,
}

impl AuditLog {
    /// 往 `dir/<file>` 追加（接着已有的链写）。
    pub fn open(dir: &Path, file: &str) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir.join(BODIES))?;
        let path = dir.join(file);
        let (seq, prev) = tail(&path);
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let (tx, rx) = mpsc::channel(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = Writer {
            out: std::io::BufWriter::new(out),
            bodies: dir.join(BODIES),
            seq,
            prev,
            dropped: dropped.clone(),
        };
        tokio::spawn(writer.run(rx));
        Ok(Self {
            tx,
            dropped,
            dir: dir.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 记一条：`kind` 加上 `fields` 里的字段（必须是对象）。
    pub fn record(&self, kind: &str, fields: Value) {
        let mut map = match fields {
            Value::Object(m) => m,
            other => {
                let mut m = Map::new();
                m.insert("value".into(), other);
                m
            }
        };
        map.insert("kind".into(), json!(kind));
        self.send(Entry::Record(map));
    }

    /// 记一条出网（正文块另存）。
    pub fn flow(&self, flow: Flow) {
        self.send(Entry::Flow(Box::new(flow)));
    }

    fn send(&self, entry: Entry) {
        if self.tx.try_send(entry).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct Writer {
    out: std::io::BufWriter<std::fs::File>,
    bodies: PathBuf,
    seq: u64,
    prev: String,
    dropped: Arc<AtomicU64>,
}

impl Writer {
    async fn run(mut self, mut rx: mpsc::Receiver<Entry>) {
        while let Some(entry) = rx.recv().await {
            self.write(entry);
            while let Ok(more) = rx.try_recv() {
                self.write(more);
            }
            let _ = self.out.flush();
        }
        let _ = self.out.flush();
    }

    fn write(&mut self, entry: Entry) {
        let mut map = match entry {
            Entry::Record(m) => m,
            Entry::Flow(mut flow) => {
                for msg in [flow.request.as_mut(), flow.response.as_mut()]
                    .into_iter()
                    .flatten()
                {
                    self.store_body(&mut msg.body);
                }
                let mut m = match serde_json::to_value(&flow) {
                    Ok(Value::Object(m)) => m,
                    _ => return,
                };
                m.insert("kind".into(), json!("net"));
                m
            }
        };
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            map.insert("dropped".into(), json!(dropped));
        }
        self.seq += 1;
        map.insert("seq".into(), json!(self.seq));
        map.entry("ts").or_insert_with(|| json!(now_ms()));
        map.insert("prev".into(), json!(self.prev));
        let Ok(line) = serde_json::to_string(&Value::Object(map)) else {
            return;
        };
        self.prev = hex(&Sha256::digest(line.as_bytes()));
        let _ = writeln!(self.out, "{line}");
    }

    fn store_body(&self, body: &mut Captured) {
        if body.sample.is_empty() {
            return;
        }
        let sha = hex(&Sha256::digest(&body.sample));
        let path = self.bodies.join(&sha);
        if !path.exists() {
            let _ = std::fs::write(&path, &body.sample);
        }
        body.sha256 = Some(sha);
    }
}

/// 已有文件最后一行的序号和哈希（接着写）；没有就从头来。
fn tail(path: &Path) -> (u64, String) {
    let Ok(file) = std::fs::File::open(path) else {
        return (0, String::new());
    };
    let mut last = None;
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        if !line.trim().is_empty() {
            last = Some(line);
        }
    }
    match last {
        Some(line) => {
            let seq = serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|v| v["seq"].as_u64())
                .unwrap_or(0);
            (seq, hex(&Sha256::digest(line.as_bytes())))
        }
        None => (0, String::new()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 一个目录里全部写入方的记录，按时间排好（同一时刻按文件、序号）。
pub fn read(dir: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    for (name, lines) in logs(dir) {
        for line in lines {
            if let Ok(mut v) = serde_json::from_str::<Value>(&line) {
                v["source"] = json!(name);
                out.push(v);
            }
        }
    }
    out.sort_by_key(|v| {
        (
            v["ts"].as_u64().unwrap_or(0),
            v["seq"].as_u64().unwrap_or(0),
        )
    });
    out
}

/// 校验结果：每个文件多少条、链断在哪（`None` = 完好）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub file: String,
    pub records: usize,
    pub broken_at: Option<usize>,
}

/// 逐个文件验哈希链。
pub fn verify(dir: &Path) -> Vec<Verified> {
    logs(dir)
        .into_iter()
        .map(|(file, lines)| {
            let mut prev = String::new();
            let mut broken_at = None;
            for (i, line) in lines.iter().enumerate() {
                let claimed = serde_json::from_str::<Value>(line)
                    .ok()
                    .and_then(|v| v["prev"].as_str().map(str::to_string));
                if claimed.as_deref() != Some(prev.as_str()) {
                    broken_at = Some(i + 1);
                    break;
                }
                prev = hex(&Sha256::digest(line.as_bytes()));
            }
            Verified {
                file,
                records: lines.len(),
                broken_at,
            }
        })
        .collect()
}

fn logs(dir: &Path) -> Vec<(String, Vec<String>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&p).ok()?;
            Some((
                name,
                text.lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(str::to_string)
                    .collect(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::record::Message;

    fn flow(body: &[u8]) -> Flow {
        let mut captured = Captured::default();
        captured.push(body, 1024);
        Flow {
            tag: "t".into(),
            kind: "http",
            scheme: "https",
            host: "example.com".into(),
            port: 443,
            method: Some("GET".into()),
            path: Some("/".into()),
            status: Some(200),
            request: None,
            response: Some(Message {
                headers: vec![],
                body: captured,
            }),
            bytes_up: 0,
            bytes_down: body.len() as u64,
            started_ms: 1,
            duration_ms: 2,
            decrypted: true,
            note: None,
            error: None,
        }
    }

    async fn settle(log: AuditLog) {
        drop(log);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn records_chain_resume_and_detect_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::open(dir.path(), "events.jsonl").unwrap();
        log.record("tool_call", json!({"name": "bash"}));
        log.flow(flow(b"<html>hi</html>"));
        log.flow(flow(b"<html>hi</html>"));
        settle(log).await;
        // 重开接着写：序号和链接得上。
        let log = AuditLog::open(dir.path(), "events.jsonl").unwrap();
        log.record("tool_result", json!({"name": "bash"}));
        settle(log).await;

        let events = read(dir.path());
        assert_eq!(events.len(), 4);
        assert_eq!(events[3]["seq"], 4);
        let sha = events[1]["response"]["body"]["sha256"].as_str().unwrap();
        assert_eq!(
            std::fs::read(dir.path().join(BODIES).join(sha)).unwrap(),
            b"<html>hi</html>"
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join(BODIES)).unwrap().count(),
            1,
            "同样的正文只存一份"
        );
        assert_eq!(verify(dir.path())[0].broken_at, None);

        // 改中间一行：从下一行起链断了。
        let path = dir.path().join("events.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replacen("\"bash\"", "\"zsh\"", 1)).unwrap();
        assert_eq!(verify(dir.path())[0].broken_at, Some(2));
    }
}
