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
use std::sync::{mpsc, Arc};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::audit::record::{now_ms, Captured, Flow};

/// 通道里最多排这么多条（之后丢弃、计数）。
const QUEUE: usize = 8192;
pub const BODIES: &str = "bodies";

enum Entry {
    Record(Map<String, Value>),
    Flow(Box<Flow>),
}

/// 一个写入方。写盘在它自己的线程上（不占 tokio 的工作线程、不依赖运行时还在）；
/// drop 时等这个线程把排着的写完再走，关掉沙箱、退出 Dock 都不丢最后几条。
pub struct AuditLog {
    tx: Option<mpsc::SyncSender<Entry>>,
    writer: Option<std::thread::JoinHandle<()>>,
    dropped: Arc<AtomicU64>,
    dir: PathBuf,
}

impl Drop for AuditLog {
    fn drop(&mut self) {
        // 先放掉发送端，写入线程把队列里剩下的写完、收到断开就退出。
        drop(self.tx.take());
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
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
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = Writer {
            out: std::io::BufWriter::new(out),
            bodies: dir.join(BODIES),
            seq,
            prev,
            dropped: dropped.clone(),
        };
        let writer = std::thread::Builder::new()
            .name("dock-audit-writer".into())
            .spawn(move || writer.run(rx))?;
        Ok(Self {
            tx: Some(tx),
            writer: Some(writer),
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
        let sent = self
            .tx
            .as_ref()
            .is_some_and(|tx| tx.try_send(entry).is_ok());
        if !sent {
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
    fn run(mut self, rx: mpsc::Receiver<Entry>) {
        while let Ok(entry) = rx.recv() {
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

/// 同一个会话的好几次运行（Dock 每起一次，开了沙箱的会话各开一份目录）接起来读，按时间排。
///
/// - `source` 写成 `<目录名>/<文件名>`：按它找得回是哪一份目录（[`crate::audit::packet::record_id`]）。
/// - 命令的 job id 每次运行都从 `job-1` 数起。除了最后一次，其余运行的 `job` 和 `tag`
///   后面都接 `@<第几次>`：不然前一次的出网会算到后一次同名的命令头上。浏览器的 `tag`
///   （`browser:<标签页>`）一样改，它按前缀认，改了照样认得出。
pub fn read_runs(dirs: &[PathBuf]) -> Vec<Value> {
    let last = dirs.len().saturating_sub(1);
    let mut out = Vec::new();
    for (i, dir) in dirs.iter().enumerate() {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for mut v in read(dir) {
            let file = v["source"].as_str().unwrap_or("").to_string();
            v["source"] = json!(format!("{name}/{file}"));
            if i < last {
                let run = i + 1;
                if let Some(job) = v["job"].as_str().map(str::to_string) {
                    v["job"] = json!(format!("{job}@{run}"));
                }
                if let Some(tag) = v["tag"].as_str().map(str::to_string) {
                    v["tag"] = json!(format!("{tag}@{run}"));
                }
            }
            out.push(v);
        }
    }
    // 稳定排序：同一毫秒里保持目录内的顺序。
    out.sort_by_key(|v| v["ts"].as_u64().unwrap_or(0));
    out
}

/// 校验结果：每个文件多少条、链断在哪（`None` = 完好）、哪些正文块不见了或被改过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub file: String,
    pub records: usize,
    pub broken_at: Option<usize>,
    /// 记录里引用、但 `bodies/` 里没有或内容对不上哈希的正文块（sha256）。
    pub bad_bodies: Vec<String>,
}

impl Verified {
    /// 哈希链完好，引用的正文块也都在、都对得上。
    pub fn intact(&self) -> bool {
        self.broken_at.is_none() && self.bad_bodies.is_empty()
    }
}

/// 逐个文件验哈希链，并核对记录引用的正文块（`bodies/<sha256>` 在不在、内容对不对）。
pub fn verify(dir: &Path) -> Vec<Verified> {
    let bodies = dir.join(BODIES);
    logs(dir)
        .into_iter()
        .map(|(file, lines)| {
            let mut prev = String::new();
            let mut broken_at = None;
            let mut bad_bodies = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                let parsed = serde_json::from_str::<Value>(line).ok();
                let claimed = parsed
                    .as_ref()
                    .and_then(|v| v["prev"].as_str().map(str::to_string));
                if broken_at.is_none() && claimed.as_deref() != Some(prev.as_str()) {
                    broken_at = Some(i + 1);
                }
                prev = hex(&Sha256::digest(line.as_bytes()));
                for side in ["request", "response"] {
                    let Some(sha) = parsed
                        .as_ref()
                        .and_then(|v| v[side]["body"]["sha256"].as_str())
                    else {
                        continue;
                    };
                    // 不是正文块的名字（日志被改过）：算对不上，也不拿它去读别处的文件。
                    let ok = crate::audit::packet::is_sha256(sha)
                        && std::fs::read(bodies.join(sha))
                            .is_ok_and(|bytes| hex(&Sha256::digest(&bytes)) == sha);
                    if !ok && !bad_bodies.iter().any(|b| b == sha) {
                        bad_bodies.push(sha.to_string());
                    }
                }
            }
            Verified {
                file,
                records: lines.len(),
                broken_at,
                bad_bodies,
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

    /// 两次运行都有 `job-1`（和同一个标签页名）：接起来以后前一次的改名，出网仍跟着自己那次的
    /// 命令走。
    #[test]
    fn runs_are_stitched_in_order_and_job_ids_do_not_collide() {
        let root = tempfile::tempdir().unwrap();
        let one = root.path().join("20261128-111307-s");
        let two = root.path().join("20261128-120123-s");
        for (dir, program) in [(&one, "curl"), (&two, "git")] {
            let log = AuditLog::open(dir, "events.jsonl").unwrap();
            log.record("command", json!({ "job": "job-1", "program": program }));
            log.record(
                "net",
                json!({ "tag": "job-1", "host": format!("{program}.example") }),
            );
            log.record("net", json!({ "tag": "browser:T1", "host": "b.example" }));
            drop(log);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let events = read_runs(&[one, two]);
        let summary: Vec<(String, String, String)> = events
            .iter()
            .map(|e| {
                (
                    e["source"].as_str().unwrap().to_string(),
                    e["job"].as_str().or(e["tag"].as_str()).unwrap().to_string(),
                    e["host"]
                        .as_str()
                        .or(e["program"].as_str())
                        .unwrap()
                        .to_string(),
                )
            })
            .collect();
        let src1 = "20261128-111307-s/events.jsonl".to_string();
        let src2 = "20261128-120123-s/events.jsonl".to_string();
        assert_eq!(
            summary,
            vec![
                (src1.clone(), "job-1@1".into(), "curl".into()),
                (src1.clone(), "job-1@1".into(), "curl.example".into()),
                (src1, "browser:T1@1".into(), "b.example".into()),
                (src2.clone(), "job-1".into(), "git".into()),
                (src2.clone(), "job-1".into(), "git.example".into()),
                (src2, "browser:T1".into(), "b.example".into()),
            ]
        );
    }
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

    #[tokio::test]
    async fn records_chain_resume_and_detect_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::open(dir.path(), "events.jsonl").unwrap();
        log.record("tool_call", json!({"name": "bash"}));
        log.flow(flow(b"<html>hi</html>"));
        log.flow(flow(b"<html>hi</html>"));
        // 关掉就写完了，不用等：drop 等写入线程把队列写完（以前是 tokio 任务，关掉之后
        // 还要等一会儿，进程一退就丢最后几条）。
        drop(log);
        // 重开接着写：序号和链接得上。
        let log = AuditLog::open(dir.path(), "events.jsonl").unwrap();
        log.record("tool_result", json!({"name": "bash"}));
        drop(log);

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
        assert!(verify(dir.path())[0].intact());

        // 正文块被改过、被删了：校验要报出来（以前只验 jsonl 的链）。
        let body = dir.path().join(BODIES).join(sha);
        std::fs::write(&body, b"<html>tampered</html>").unwrap();
        let v = &verify(dir.path())[0];
        assert_eq!(v.bad_bodies, [sha.to_string()]);
        assert!(!v.intact());
        std::fs::remove_file(&body).unwrap();
        assert_eq!(verify(dir.path())[0].bad_bodies.len(), 1);
        std::fs::write(&body, b"<html>hi</html>").unwrap();
        assert!(verify(dir.path())[0].intact());

        // 改中间一行：从下一行起链断了。
        let path = dir.path().join("events.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replacen("\"bash\"", "\"zsh\"", 1)).unwrap();
        assert_eq!(verify(dir.path())[0].broken_at, Some(2));
    }
}
