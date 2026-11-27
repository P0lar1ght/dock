//! 把一段审计记录汇总成链路图（Mermaid）和导出报告（自包含的 HTML，图离线渲染成 SVG）。
//!
//! 记录的种类（spine 的 `audit` 插件与浏览器服务写，这里只读）：
//! - `tool_call` / `tool_result`：`tool`、`call_id`、`is_error`。
//! - `command` / `command_end`：`job`、`tool`、`program`、`command`、`exit`、`ms`。
//! - `net`：一次出网（[`crate::audit::record::Flow`]）。`tag` 是命令的 `job`，浏览器的是 `browser:<标签页>`。
//!
//! 图分四层：会话 → 工具 → 命令里的程序 → 目的主机。边上写次数和流量；主机太多时每个来源只画
//! 流量最大的几个，其余并成一个「其他」。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

use crate::audit::store::{self, Verified};

/// 每个来源最多画几个主机（其余并成「其他 N 个」）。
const HOSTS_PER_SOURCE: usize = 8;

#[derive(Default)]
struct Count {
    n: u64,
    errors: u64,
    bytes: u64,
}

/// 汇总出来的图：节点和边都按出现顺序排（图每次画出来一样）。
#[derive(Default)]
pub struct Summary {
    pub tool_calls: u64,
    pub commands: u64,
    pub requests: u64,
    pub decrypted: u64,
    pub bytes: u64,
    pub errors: u64,
    tools: BTreeMap<String, Count>,
    programs: BTreeMap<(String, String), Count>,
    hosts: BTreeMap<String, BTreeMap<String, Count>>,
}

impl Summary {
    pub fn build(events: &[Value]) -> Self {
        let mut s = Summary::default();
        // job → (工具, 程序)：出网按 tag 找回是哪条命令发的。
        let mut jobs: HashMap<String, (String, String)> = HashMap::new();
        for e in events {
            match e["kind"].as_str().unwrap_or("") {
                "tool_call" => {
                    s.tool_calls += 1;
                    s.tools.entry(str_of(e, "tool")).or_default().n += 1;
                }
                "tool_result" if e["is_error"].as_bool() == Some(true) => {
                    s.errors += 1;
                    s.tools.entry(str_of(e, "tool")).or_default().errors += 1;
                }
                "command" => {
                    s.commands += 1;
                    let key = (str_of(e, "tool"), str_of(e, "program"));
                    jobs.insert(str_of(e, "job"), key.clone());
                    s.programs.entry(key).or_default().n += 1;
                }
                "command_end" => {
                    if e["exit"].as_i64().is_some_and(|c| c != 0) {
                        if let Some(key) = jobs.get(&str_of(e, "job")) {
                            s.programs.entry(key.clone()).or_default().errors += 1;
                        }
                    }
                }
                "net" => {
                    s.requests += 1;
                    if e["decrypted"].as_bool() == Some(true) {
                        s.decrypted += 1;
                    }
                    let bytes =
                        e["bytes_up"].as_u64().unwrap_or(0) + e["bytes_down"].as_u64().unwrap_or(0);
                    s.bytes += bytes;
                    let tag = str_of(e, "tag");
                    let source = if tag.starts_with("browser") {
                        "浏览器".to_string()
                    } else if let Some((tool, program)) = jobs.get(&tag) {
                        format!("{tool}/{program}")
                    } else {
                        "其他".to_string()
                    };
                    let c = s
                        .hosts
                        .entry(source)
                        .or_default()
                        .entry(str_of(e, "host"))
                        .or_default();
                    c.n += 1;
                    c.bytes += bytes;
                    if e["error"].is_string() || e["status"].as_u64().is_some_and(|st| st >= 400) {
                        c.errors += 1;
                    }
                }
                _ => {}
            }
        }
        s
    }

    /// Mermaid `flowchart LR`。
    pub fn mermaid(&self) -> String {
        let mut out = String::from("flowchart LR\n  S([\"会话\"])\n");
        let mut ids: HashMap<String, String> = HashMap::new();
        let mut next = 0usize;
        let mut id = |key: &str| -> (String, bool) {
            if let Some(id) = ids.get(key) {
                return (id.clone(), false);
            }
            next += 1;
            let id = format!("n{next}");
            ids.insert(key.to_string(), id.clone());
            (id, true)
        };
        for (tool, c) in &self.tools {
            let (tid, _) = id(&format!("tool:{tool}"));
            out.push_str(&format!(
                "  {tid}[\"{}\"]\n  S -->|{}| {tid}\n",
                label(&format!("{tool}{}", errors_suffix(c.errors))),
                c.n
            ));
        }
        for ((tool, program), c) in &self.programs {
            let (tid, fresh) = id(&format!("tool:{tool}"));
            if fresh {
                out.push_str(&format!("  {tid}[\"{}\"]\n  S --> {tid}\n", label(tool)));
            }
            let (pid, _) = id(&format!("prog:{tool}/{program}"));
            out.push_str(&format!(
                "  {pid}[\"{}\"]\n  {tid} -->|{}| {pid}\n",
                label(&format!("{program}{}", errors_suffix(c.errors))),
                c.n
            ));
        }
        for (source, hosts) in &self.hosts {
            let (sid, fresh) = if source == "浏览器" {
                id("tool:浏览器")
            } else if source == "其他" {
                id("src:其他")
            } else {
                id(&format!("prog:{source}"))
            };
            if fresh {
                out.push_str(&format!("  {sid}[\"{}\"]\n  S --> {sid}\n", label(source)));
            }
            let mut ranked: Vec<(&String, &Count)> = hosts.iter().collect();
            ranked.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes).then(b.1.n.cmp(&a.1.n)));
            let (shown, rest) = ranked.split_at(ranked.len().min(HOSTS_PER_SOURCE));
            for (host, c) in shown {
                let (hid, fresh) = id(&format!("host:{host}"));
                if fresh {
                    out.push_str(&format!("  {hid}{{{{\"{}\"}}}}\n", label(host)));
                }
                out.push_str(&format!(
                    "  {sid} -->|\"{} 次 · {}{}\"| {hid}\n",
                    c.n,
                    human_bytes(c.bytes),
                    errors_suffix(c.errors)
                ));
            }
            if !rest.is_empty() {
                let (oid, _) = id(&format!("others:{source}"));
                let n: u64 = rest.iter().map(|(_, c)| c.n).sum();
                let bytes: u64 = rest.iter().map(|(_, c)| c.bytes).sum();
                out.push_str(&format!(
                    "  {oid}{{{{\"其他 {} 个主机\"}}}}\n  {sid} -->|\"{n} 次 · {}\"| {oid}\n",
                    rest.len(),
                    human_bytes(bytes)
                ));
            }
        }
        out
    }
}

/// 给界面画的链路图：节点和边，都带次数、出错数、流量。节点分四层（`kind`）：`session` →
/// `tool` → `program`（命令里的程序）→ `host`（目的主机）；`source` 是认不出是谁发的出网，
/// `others` 是并起来的「其他 N 个主机」。和 [`Summary::mermaid`] 是同一张图。
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
    pub n: u64,
    pub errors: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub n: u64,
    pub errors: u64,
    pub bytes: u64,
}

impl Graph {
    fn node(&mut self, id: &str, kind: &'static str, label: &str) -> usize {
        if let Some(i) = self.nodes.iter().position(|n| n.id == id) {
            return i;
        }
        self.nodes.push(Node {
            id: id.to_string(),
            kind,
            label: label.to_string(),
            n: 0,
            errors: 0,
            bytes: 0,
        });
        self.nodes.len() - 1
    }

    fn edge(&mut self, from: &str, to: &str, c: &Count) {
        self.edges.push(Edge {
            from: from.to_string(),
            to: to.to_string(),
            n: c.n,
            errors: c.errors,
            bytes: c.bytes,
        });
    }
}

impl Summary {
    /// 结构化的链路图（界面自己排版）。节点的 `n` / `bytes` 是进它的边加起来的。
    pub fn graph(&self) -> Graph {
        let mut g = Graph::default();
        g.node("S", "session", "会话");
        for (tool, c) in &self.tools {
            let id = format!("tool:{tool}");
            g.node(&id, "tool", tool);
            g.edge("S", &id, c);
        }
        for ((tool, program), c) in &self.programs {
            let tid = format!("tool:{tool}");
            if !g.nodes.iter().any(|n| n.id == tid) {
                g.node(&tid, "tool", tool);
                g.edge("S", &tid, &Count::default());
            }
            let pid = format!("prog:{tool}/{program}");
            g.node(&pid, "program", program);
            g.edge(&tid, &pid, c);
        }
        for (source, hosts) in &self.hosts {
            let sid = match source.as_str() {
                "浏览器" => "tool:浏览器".to_string(),
                "其他" => "src:其他".to_string(),
                other => format!("prog:{other}"),
            };
            if !g.nodes.iter().any(|n| n.id == sid) {
                let kind = if source == "其他" { "source" } else { "tool" };
                g.node(&sid, kind, source);
                g.edge("S", &sid, &Count::default());
            }
            let mut ranked: Vec<(&String, &Count)> = hosts.iter().collect();
            ranked.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes).then(b.1.n.cmp(&a.1.n)));
            let (shown, rest) = ranked.split_at(ranked.len().min(HOSTS_PER_SOURCE));
            for (host, c) in shown {
                let hid = format!("host:{host}");
                g.node(&hid, "host", host);
                g.edge(&sid, &hid, c);
            }
            if !rest.is_empty() {
                let oid = format!("others:{source}");
                g.node(&oid, "others", &format!("其他 {} 个主机", rest.len()));
                let c = Count {
                    n: rest.iter().map(|(_, c)| c.n).sum(),
                    errors: rest.iter().map(|(_, c)| c.errors).sum(),
                    bytes: rest.iter().map(|(_, c)| c.bytes).sum(),
                };
                g.edge(&sid, &oid, &c);
            }
        }
        // 节点上的数：进它的边加起来；会话是出它的边加起来。
        for i in 0..g.nodes.len() {
            let id = g.nodes[i].id.clone();
            let incoming: Vec<&Edge> = if id == "S" {
                g.edges.iter().filter(|e| e.from == id).collect()
            } else {
                g.edges.iter().filter(|e| e.to == id).collect()
            };
            let (n, errors, bytes) = incoming.iter().fold((0, 0, 0), |(n, er, b), e| {
                (n + e.n, er + e.errors, b + e.bytes)
            });
            let node = &mut g.nodes[i];
            node.n = n;
            node.errors = errors;
            node.bytes = bytes;
        }
        g
    }
}

/// 时间线里的一行（界面直接画）：`ts`、`kind`（`tool` / `result` / `command` / `command_end` /
/// `net` / `audit`）、`title`、`detail`、`error`（出错了说什么，没出错是 `null`），以及把几行串起来
/// 的 `call`（工具调用 id）/ `job`（命令）/ `tag`（出网是哪条命令或哪个标签页发的）。认不出的记录不出。
pub fn timeline_entry(e: &Value) -> Option<Value> {
    let clip = |s: &str, n: usize| -> String {
        let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if one.chars().count() > n {
            format!("{}…", one.chars().take(n).collect::<String>())
        } else {
            one
        }
    };
    let text = |k: &str| e[k].as_str().unwrap_or("");
    let ts = e["ts"].as_u64().unwrap_or(0);
    let entry = match e["kind"].as_str()? {
        "tool_call" => json!({
            "kind": "tool",
            "title": text("tool"),
            "detail": clip(text("args"), 240),
            "error": e["denied"].as_str().map(|d| format!("被拦下：{d}")),
            "call": e["call_id"],
        }),
        "tool_result" => json!({
            "kind": "result",
            "title": text("tool"),
            "detail": clip(text("result_head"), 240),
            "error": (e["is_error"].as_bool() == Some(true)).then_some("出错"),
            "call": e["call_id"],
        }),
        "command" => json!({
            "kind": "command",
            "title": text("program"),
            "detail": clip(text("command"), 300),
            "error": null,
            "call": e["call_id"],
            "job": e["job"],
        }),
        "command_end" => {
            let exit = e["exit"].as_i64();
            json!({
                "kind": "command_end",
                "title": match exit { Some(c) => format!("退出 {c}"), None => "结束".into() },
                "detail": format!("{} ms", e["ms"].as_u64().unwrap_or(0)),
                "error": exit.filter(|c| *c != 0).map(|c| format!("退出码 {c}")),
                "job": e["job"],
            })
        }
        "net" => {
            let method = e["method"].as_str().unwrap_or("CONNECT");
            let host = text("host");
            let status = e["status"].as_u64();
            let bytes = e["bytes_up"].as_u64().unwrap_or(0) + e["bytes_down"].as_u64().unwrap_or(0);
            let mut detail = format!(
                "{}{}",
                text("path"),
                status.map(|s| format!(" · {s}")).unwrap_or_default()
            );
            detail.push_str(&format!(" · {}", human_bytes(bytes)));
            if e["decrypted"].as_bool() != Some(true) {
                detail.push_str(" · 未解密");
            }
            let error = e["error"]
                .as_str()
                .map(str::to_string)
                .or_else(|| status.filter(|s| *s >= 400).map(|s| format!("HTTP {s}")));
            json!({
                "kind": "net",
                "title": format!("{method} {host}"),
                "detail": clip(detail.trim_start_matches(" · "), 300),
                "error": error,
                "tag": e["tag"],
                "host": host,
                "bytes": bytes,
                "decrypted": e["decrypted"],
                "method": e["method"],
                "scheme": e["scheme"],
                "port": e["port"],
                "path": e["path"],
                "status": e["status"],
                "ms": e["duration_ms"],
                "note": e["note"],
            })
        }
        "audit" => json!({
            "kind": "audit",
            "title": text("event"),
            "detail": clip(e["host"].as_str().or(e["command"].as_str()).unwrap_or(""), 200),
            "error": e["error"],
        }),
        _ => return None,
    };
    let mut entry = entry;
    entry["ts"] = json!(ts);
    // 按它取完整的请求 / 响应包（`packet::flow_detail`）。
    entry["id"] = json!(crate::audit::packet::record_id(e));
    Some(entry)
}

/// 最近 `limit` 次出网（按时间），HTTP 历史那一页用。
pub fn requests(events: &[Value], limit: usize) -> Vec<Value> {
    let rows: Vec<Value> = events
        .iter()
        .filter(|e| e["kind"] == "net")
        .filter_map(timeline_entry)
        .collect();
    let skip = rows.len().saturating_sub(limit);
    rows.into_iter().skip(skip).collect()
}

/// 最近 `limit` 条时间线（按时间）。
pub fn timeline(events: &[Value], limit: usize) -> Vec<Value> {
    let rows: Vec<Value> = events.iter().filter_map(timeline_entry).collect();
    let skip = rows.len().saturating_sub(limit);
    rows.into_iter().skip(skip).collect()
}

impl Summary {
    /// 终端里看的链路图（缩进树）：会话 → 工具 → 程序 → 主机，主机按流量排。
    pub fn tree(&self) -> String {
        let mut out = String::from("会话\n");
        // 工具 → 它下面的程序；浏览器 / 其他直接挂主机。
        let mut tools: Vec<(String, Option<&Count>)> = self
            .tools
            .iter()
            .map(|(t, c)| (t.clone(), Some(c)))
            .collect();
        for (tool, _) in self.programs.keys() {
            if !tools.iter().any(|(t, _)| t == tool) {
                tools.push((tool.clone(), None));
            }
        }
        for source in self.hosts.keys().filter(|s| *s == "浏览器" || *s == "其他") {
            if !tools.iter().any(|(t, _)| t == source) {
                tools.push((source.clone(), None));
            }
        }
        let n_tools = tools.len();
        for (i, (tool, count)) in tools.iter().enumerate() {
            let last = i + 1 == n_tools;
            let label = match count {
                Some(c) => format!("{tool} ×{}{}", c.n, errors_suffix(c.errors)),
                None => tool.clone(),
            };
            out.push_str(&format!("{}{label}\n", if last { "└─ " } else { "├─ " }));
            let pad = if last { "   " } else { "│  " };
            let mut children: Vec<(String, Option<&BTreeMap<String, Count>>)> = Vec::new();
            for ((t, program), c) in &self.programs {
                if t == tool {
                    let key = format!("{t}/{program}");
                    children.push((
                        format!("{program} ×{}{}", c.n, errors_suffix(c.errors)),
                        self.hosts.get(&key),
                    ));
                }
            }
            if let Some(hosts) = self.hosts.get(tool) {
                for line in host_lines(hosts) {
                    children.push((line, None));
                }
            }
            let n = children.len();
            for (j, (line, hosts)) in children.into_iter().enumerate() {
                let last_child = j + 1 == n;
                out.push_str(&format!(
                    "{pad}{}{line}\n",
                    if last_child { "└─ " } else { "├─ " }
                ));
                if let Some(hosts) = hosts {
                    let pad2 = format!("{pad}{}", if last_child { "   " } else { "│  " });
                    let lines = host_lines(hosts);
                    let m = lines.len();
                    for (k, h) in lines.into_iter().enumerate() {
                        out.push_str(&format!(
                            "{pad2}{}{h}\n",
                            if k + 1 == m { "└─ " } else { "├─ " }
                        ));
                    }
                }
            }
        }
        out
    }
}

/// 一个来源下的主机行（按流量排，多了并成「其他 N 个」）。
fn host_lines(hosts: &BTreeMap<String, Count>) -> Vec<String> {
    let mut ranked: Vec<(&String, &Count)> = hosts.iter().collect();
    ranked.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes).then(b.1.n.cmp(&a.1.n)));
    let (shown, rest) = ranked.split_at(ranked.len().min(HOSTS_PER_SOURCE));
    let mut out: Vec<String> = shown
        .iter()
        .map(|(h, c)| {
            format!(
                "{h}  {} 次 · {}{}",
                c.n,
                human_bytes(c.bytes),
                errors_suffix(c.errors)
            )
        })
        .collect();
    if !rest.is_empty() {
        let n: u64 = rest.iter().map(|(_, c)| c.n).sum();
        let bytes: u64 = rest.iter().map(|(_, c)| c.bytes).sum();
        out.push(format!(
            "其他 {} 个主机  {n} 次 · {}",
            rest.len(),
            human_bytes(bytes)
        ));
    }
    out
}

fn str_of(e: &Value, key: &str) -> String {
    e[key].as_str().unwrap_or("?").to_string()
}

fn errors_suffix(n: u64) -> String {
    if n == 0 {
        String::new()
    } else {
        format!(" ⚠{n}")
    }
}

/// Mermaid 标签里不能出现的字符换掉。
fn label(s: &str) -> String {
    s.replace('"', "'")
        .replace(['[', ']', '{', '}', '|', '<', '>'], " ")
}

pub fn human_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / (1u64 << 10) as f64),
        n => format!("{n} B"),
    }
}

/// 时间线上的一行：何时、哪类、一句话。
fn timeline_row(e: &Value) -> Option<(u64, &'static str, String)> {
    let ts = e["ts"].as_u64().unwrap_or(0);
    let line = match e["kind"].as_str()? {
        "tool_call" => (
            "工具",
            format!(
                "{} {}",
                str_of(e, "tool"),
                clip(e["args"].as_str().unwrap_or(""), 160)
            ),
        ),
        "tool_result" => (
            "结果",
            format!(
                "{}{}",
                str_of(e, "tool"),
                if e["is_error"].as_bool() == Some(true) {
                    " 失败"
                } else {
                    " 完成"
                }
            ),
        ),
        "command" => ("命令", clip(e["command"].as_str().unwrap_or(""), 200)),
        "command_end" => (
            "命令结束",
            format!(
                "{} exit={}{}",
                str_of(e, "job"),
                e["exit"],
                e["ms"]
                    .as_u64()
                    .map(|ms| format!(" {ms}ms"))
                    .unwrap_or_default()
            ),
        ),
        "net" => {
            let what = match (e["method"].as_str(), e["path"].as_str()) {
                (Some(m), Some(p)) => format!(
                    "{m} {}://{}{}",
                    str_of(e, "scheme"),
                    str_of(e, "host"),
                    clip(p, 120)
                ),
                _ => format!("隧道 {}:{}", str_of(e, "host"), e["port"]),
            };
            let status = e["status"]
                .as_u64()
                .map(|s| format!(" → {s}"))
                .unwrap_or_default();
            let note = e["note"]
                .as_str()
                .or(e["error"].as_str())
                .map(|n| format!("（{n}）"))
                .unwrap_or_default();
            (
                "出网",
                format!("[{}] {what}{status}{note}", str_of(e, "tag")),
            )
        }
        "audit" => ("审计", str_of(e, "event")),
        _ => return None,
    };
    Some((ts, line.0, line.1))
}

fn clip(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn clock(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    match time::OffsetDateTime::from_unix_timestamp(secs) {
        Ok(t) => {
            let t = t
                .to_offset(time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC));
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            )
        }
        Err(_) => ms.to_string(),
    }
}

/// 报告里每条正文最多展示这么多字符（全文在导出目录的 `bodies/` 里）。
const BODY_PREVIEW: usize = 4000;

/// 一条出网展开后的内容：请求 / 响应的头和正文开头（文本才显示，二进制只报大小）。
fn net_details(e: &Value, bodies: Option<&Path>) -> String {
    let mut out = String::new();
    for (label, key) in [("请求", "request"), ("响应", "response")] {
        let msg = &e[key];
        if msg.is_null() {
            continue;
        }
        out.push_str(&format!("── {label} ──\n"));
        for pair in msg["headers"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "{}: {}\n",
                pair[0].as_str().unwrap_or(""),
                pair[1].as_str().unwrap_or("")
            ));
        }
        let size = msg["body"]["size"].as_u64().unwrap_or(0);
        if size == 0 {
            continue;
        }
        let text = msg["body"]["sha256"]
            .as_str()
            .zip(bodies)
            .and_then(|(sha, dir)| std::fs::read(dir.join(sha)).ok())
            .and_then(|raw| String::from_utf8(raw).ok());
        match text {
            Some(t) => {
                out.push_str(&format!("\n{}", clip(&t, BODY_PREVIEW)));
                if msg["body"]["truncated"].as_bool() == Some(true)
                    || t.chars().count() > BODY_PREVIEW
                {
                    out.push_str(&format!("\n…（共 {}）", human_bytes(size)));
                }
                out.push('\n');
            }
            None => out.push_str(&format!(
                "\n（正文 {}，不是文本或未保存）\n",
                human_bytes(size)
            )),
        }
    }
    out
}

/// 自包含的报告：概况、校验结果、链路图（离线渲染的 SVG）、没解密的连接、时间线（出网可展开看
/// 头和正文）。`bodies` 是正文块所在目录（导出时给）。
pub fn report_html(
    title: &str,
    events: &[Value],
    verified: &[Verified],
    bodies: Option<&Path>,
) -> String {
    let summary = Summary::build(events);
    let mermaid = summary.mermaid();
    let graph = match mermaid_to_svg::render_mermaid_to_svg(&mermaid, None) {
        Ok(svg) => svg,
        Err(e) => format!(
            "<p>链路图渲染失败：{}</p><pre>{}</pre>",
            esc(&e.to_string()),
            esc(&mermaid)
        ),
    };
    let chain: String = verified
        .iter()
        .map(|v| {
            let chain = match v.broken_at {
                None => format!("<li>{}：{} 条，哈希链完好</li>", esc(&v.file), v.records),
                Some(at) => format!(
                    "<li class=bad>{}：{} 条，<b>第 {at} 条起哈希链对不上（被改过或删过）</b></li>",
                    esc(&v.file),
                    v.records
                ),
            };
            if v.bad_bodies.is_empty() {
                chain
            } else {
                format!(
                    "{chain}<li class=bad>{}：<b>{} 个正文块不见了或被改过</b></li>",
                    esc(&v.file),
                    v.bad_bodies.len()
                )
            }
        })
        .collect();
    let opaque: String = events
        .iter()
        .filter(|e| {
            e["kind"] == "net"
                && e["decrypted"].as_bool() != Some(true)
                && !str_of(e, "tag").starts_with("browser")
        })
        .map(|e| {
            format!(
                "<tr><td>{}</td><td>{}:{}</td><td>{}</td></tr>",
                esc(&str_of(e, "tag")),
                esc(&str_of(e, "host")),
                e["port"],
                esc(e["note"].as_str().or(e["error"].as_str()).unwrap_or(""))
            )
        })
        .collect();
    let rows: String = events
        .iter()
        .filter_map(|e| timeline_row(e).map(|row| (e, row)))
        .map(|(e, (ts, kind, text))| {
            let cell =
                if e["kind"] == "net" && (!e["request"].is_null() || !e["response"].is_null()) {
                    format!(
                        "<details><summary>{}</summary><pre>{}</pre></details>",
                        esc(&text),
                        esc(&net_details(e, bodies))
                    )
                } else {
                    esc(&text)
                };
            format!(
                "<tr><td>{}</td><td>{kind}</td><td>{cell}</td></tr>",
                clock(ts)
            )
        })
        .collect();
    let (from, to) = (
        events.first().and_then(|e| e["ts"].as_u64()).unwrap_or(0),
        events.last().and_then(|e| e["ts"].as_u64()).unwrap_or(0),
    );
    format!(
        r#"<!doctype html><html lang="zh"><head><meta charset="utf-8"><title>{title}</title>
<style>
body{{font:14px/1.5 -apple-system,BlinkMacSystemFont,"PingFang SC",sans-serif;margin:24px;color:#1f2328}}
h1{{font-size:20px}}h2{{font-size:16px;margin-top:28px}}
.stats span{{display:inline-block;margin-right:18px}}
table{{border-collapse:collapse;width:100%}}td{{border-top:1px solid #eee;padding:4px 8px;vertical-align:top}}
td:first-child{{white-space:nowrap;color:#666}}.bad{{color:#cf222e}}.graph svg{{max-width:100%;height:auto}}
summary{{cursor:pointer}}pre{{white-space:pre-wrap;word-break:break-all;background:#f6f8fa;padding:8px;font-size:12px}}
</style></head><body>
<h1>{title}</h1>
<p>{} — {}</p>
<p class=stats><span>工具调用 <b>{}</b></span><span>命令 <b>{}</b></span><span>出网 <b>{}</b>（解密 {}）</span><span>流量 <b>{}</b></span><span>失败 <b>{}</b></span></p>
<h2>完整性</h2><ul>{chain}</ul>
<h2>链路图</h2><div class=graph>{graph}</div>
<h2>没解密的连接</h2><table>{opaque}</table>
<h2>时间线</h2><table>{rows}</table>
</body></html>"#,
        clock(from),
        clock(to),
        summary.tool_calls,
        summary.commands,
        summary.requests,
        summary.decrypted,
        human_bytes(summary.bytes),
        summary.errors,
    )
}

/// 导出一次审计：`<dir>/export-<时刻>/` 下放报告、原始记录、正文块和校验结果。
pub fn export(dir: &Path, title: &str) -> Result<PathBuf, String> {
    let events = store::read(dir);
    let verified = store::verify(dir);
    let out = dir.join(format!("export-{}", crate::audit::record::now_ms()));
    std::fs::create_dir_all(out.join(store::BODIES))
        .map_err(|e| format!("create {}: {e}", out.display()))?;
    std::fs::write(
        out.join("report.html"),
        report_html(title, &events, &verified, Some(&dir.join(store::BODIES))),
    )
    .map_err(|e| format!("write report: {e}"))?;
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|e| e == "jsonl") {
            let _ = std::fs::copy(&p, out.join(entry.file_name()));
        }
    }
    if let Ok(bodies) = std::fs::read_dir(dir.join(store::BODIES)) {
        for b in bodies.flatten() {
            let _ = std::fs::copy(b.path(), out.join(store::BODIES).join(b.file_name()));
        }
    }
    let verify_txt: String = verified
        .iter()
        .map(|v| {
            let chain = match v.broken_at {
                None => format!("{}\t{} records\tok\n", v.file, v.records),
                Some(at) => format!("{}\t{} records\tBROKEN at {at}\n", v.file, v.records),
            };
            let bodies: String = v
                .bad_bodies
                .iter()
                .map(|sha| format!("{}\tbody {sha}\tMISSING OR ALTERED\n", v.file))
                .collect();
            format!("{chain}{bodies}")
        })
        .collect();
    std::fs::write(out.join("verify.txt"), verify_txt).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Vec<Value> {
        vec![
            json!({"kind": "tool_call", "tool": "bash", "call_id": "c1", "args": "cargo build", "ts": 1}),
            json!({"kind": "command", "tool": "bash", "job": "job-1", "program": "cargo", "command": "cargo build", "ts": 2}),
            json!({"kind": "net", "tag": "job-1", "host": "index.crates.io", "port": 443, "scheme": "https", "method": "GET", "path": "/se/rd/serde", "status": 200, "decrypted": true, "bytes_up": 10, "bytes_down": 2048, "ts": 3}),
            json!({"kind": "net", "tag": "job-1", "host": "static.crates.io", "port": 443, "scheme": "https", "decrypted": false, "note": "客户端只说 h2", "bytes_up": 1, "bytes_down": 1, "ts": 4}),
            json!({"kind": "command_end", "job": "job-1", "exit": 0, "ms": 900, "ts": 5}),
            json!({"kind": "tool_result", "tool": "bash", "call_id": "c1", "is_error": false, "ts": 6}),
            json!({"kind": "tool_call", "tool": "browser_js", "call_id": "c2", "ts": 7}),
            json!({"kind": "net", "tag": "browser:T1", "host": "www.apple.com", "port": 443, "scheme": "https", "method": "GET", "path": "/", "status": 200, "decrypted": true, "bytes_down": 4096, "ts": 8}),
            json!({"kind": "tool_result", "tool": "browser_js", "call_id": "c2", "is_error": true, "ts": 9}),
        ]
    }

    #[test]
    fn the_graph_links_tools_programs_and_hosts() {
        let s = Summary::build(&sample());
        assert_eq!(
            (s.tool_calls, s.commands, s.requests, s.decrypted, s.errors),
            (2, 1, 3, 2, 1)
        );
        let m = s.mermaid();
        assert!(m.starts_with("flowchart LR"), "{m}");
        assert!(m.contains("\"cargo\""), "{m}");
        assert!(m.contains("\"index.crates.io\""), "{m}");
        assert!(m.contains("1 次 · 2.0 KB"), "{m}");
        assert!(m.contains("\"browser_js ⚠1\""), "{m}");
        assert!(m.contains("\"www.apple.com\""), "{m}");
        // 渲染得出来（导出报告离线画 SVG 用的就是它）。
        assert!(mermaid_to_svg::render_mermaid_to_svg(&m, None)
            .unwrap()
            .contains("<svg"));
    }

    /// 界面用的结构化图和 Mermaid 是同一张：四层、边上的次数和流量、出错数都对得上。
    #[test]
    fn the_structured_graph_matches_the_mermaid_one() {
        let g = Summary::build(&sample()).graph();
        let node = |id: &str| {
            g.nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("{id}: {g:?}"))
        };
        let edge = |a: &str, b: &str| {
            g.edges
                .iter()
                .find(|e| e.from == a && e.to == b)
                .unwrap_or_else(|| panic!("{a}->{b}: {g:?}"))
        };
        assert_eq!(node("S").kind, "session");
        assert_eq!(node("tool:bash").kind, "tool");
        assert_eq!(node("prog:bash/cargo").kind, "program");
        assert_eq!(node("host:index.crates.io").kind, "host");
        assert_eq!(edge("S", "tool:bash").n, 1);
        assert_eq!(edge("tool:bash", "prog:bash/cargo").n, 1);
        let crates = edge("prog:bash/cargo", "host:index.crates.io");
        assert_eq!((crates.n, crates.bytes), (1, 2058));
        assert_eq!(node("tool:browser_js").errors, 1);
        assert_eq!(edge("tool:浏览器", "host:www.apple.com").bytes, 4096);
        assert_eq!(node("prog:bash/cargo").n, 1);
        // 每条边两头都是图里的节点。
        for e in &g.edges {
            assert!(g.nodes.iter().any(|n| n.id == e.from) && g.nodes.iter().any(|n| n.id == e.to));
        }
    }

    /// 时间线：每种记录一行，串起来的 id（call / job / tag）留着，出错的写明原因。
    #[test]
    fn the_timeline_normalizes_each_record() {
        let rows = timeline(&sample(), 100);
        assert_eq!(rows.len(), 9);
        assert_eq!(rows[0]["kind"], "tool");
        assert_eq!(rows[0]["title"], "bash");
        assert_eq!(rows[0]["call"], "c1");
        assert_eq!(rows[1]["kind"], "command");
        assert_eq!(rows[1]["job"], "job-1");
        assert_eq!(rows[2]["kind"], "net");
        assert_eq!(rows[2]["title"], "GET index.crates.io");
        assert_eq!(rows[2]["tag"], "job-1");
        assert!(rows[2]["error"].is_null());
        assert!(
            rows[3]["detail"].as_str().unwrap().contains("未解密"),
            "{}",
            rows[3]
        );
        assert_eq!(rows[4]["title"], "退出 0");
        assert_eq!(rows[8]["error"], "出错");
        // 只要最近的几条。
        let tail = timeline(&sample(), 2);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[1]["ts"], 9);
    }

    #[test]
    fn the_terminal_tree_nests_programs_and_hosts() {
        let t = Summary::build(&sample()).tree();
        assert!(t.starts_with("会话\n"), "{t}");
        assert!(t.contains("├─ bash ×1"), "{t}");
        assert!(t.contains("│  └─ cargo ×1"), "{t}");
        assert!(t.contains("index.crates.io  1 次 · 2.0 KB"), "{t}");
        assert!(t.contains("browser_js ×1 ⚠1"), "{t}");
        assert!(t.contains("└─ 浏览器"), "{t}");
        assert!(t.contains("www.apple.com  1 次 · 4.0 KB"), "{t}");
    }

    #[test]
    fn many_hosts_fold_into_others() {
        let mut events =
            vec![json!({"kind": "command", "tool": "bash", "job": "j", "program": "curl"})];
        for i in 0..20 {
            events.push(json!({"kind": "net", "tag": "j", "host": format!("h{i}.example"), "bytes_down": i}));
        }
        let m = Summary::build(&events).mermaid();
        assert!(m.contains("其他 12 个主机"), "{m}");
        assert!(
            m.contains("h19.example") && !m.contains("h0.example"),
            "{m}"
        );
    }

    #[test]
    fn report_is_self_contained() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("abc"), "{\"ok\":true}").unwrap();
        let mut events = sample();
        events.push(json!({"kind": "net", "tag": "job-1", "host": "api.example", "port": 443, "scheme": "https", "method": "POST", "path": "/v1", "status": 201, "decrypted": true,
            "request": {"headers": [["authorization", "[redacted len=9 sha256=00]"]], "body": {"size": 11, "truncated": false, "sha256": "abc"}}, "ts": 10}));
        let html = report_html("审计", &events, &[], Some(dir.path()));
        assert!(html.contains("<details>"), "出网可以展开");
        assert!(html.contains("{&quot;ok&quot;:true}"), "正文开头要能看到");
        assert!(html.contains("authorization: [redacted"));
        assert!(html.contains("<svg"), "图要离线渲染进报告");
        assert!(!html.contains("<script"), "报告里不带脚本、不出网");
        assert!(html.contains("客户端只说 h2"));
        assert!(html.contains("cargo build"));
    }
}
