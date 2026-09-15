//! 前缀缓存诊断日志（`DOCK_CACHE_DEBUG`）。默认整个模块是死的。
//!
//! 命中率掉下去时，`/usage` 只能告诉你「掉了」，没法回答「前缀是从哪一条消息
//! 开始不一样的」。这里把每次请求与**同一会话上一次请求**逐条比对：系统提示、
//! 工具表、消息数组各自比，第一处不同报出下标、角色、字节偏移和两侧片段。
//!
//! 再把上游报回来的 `read` / `write` / `miss` 贴在同一条记录下面。两边合起来
//! 就能分辨两种成因：
//! - **前缀真被改写**：同前 N 条 < 上一次的条数，日志直接指出改在哪一条；
//! - **上游计数问题**：前缀一字节没变（同前 = 全部），写入却仍然很大。
//!
//! TUI 占着 stdout，所以只写文件。日志里**含对话片段**（每处 160 字节封顶），
//! 是本机调试用的，别提交、别外发。

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::Value;

use crate::config::dock_home;
use crate::stream_acc::StreamDelta;

/// 每处片段最多记这么多字节，免得把整份工具表和文件内容抄进日志。
const EXCERPT_BYTES: usize = 160;

/// 上一次请求的形状，按会话身份分开存——子代理与主会话交替发请求，混在一起比
/// 出来的「前缀变了」全是假的。
struct Prev {
    system: String,
    tools: String,
    messages: Vec<String>,
}

static PREV: Mutex<Option<HashMap<String, Prev>>> = Mutex::new(None);

/// `DOCK_CACHE_DEBUG` 没设 = 整个模块不做事。值是 `1` / `on` / `true` 时写
/// `$DOCK_HOME/scratch/cache-debug.log`，否则把值当成路径。
fn log_path() -> Option<PathBuf> {
    let raw = std::env::var("DOCK_CACHE_DEBUG").ok()?;
    let raw = raw.trim();
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") {
        return None;
    }
    if matches!(raw, "1") || raw.eq_ignore_ascii_case("on") || raw.eq_ignore_ascii_case("true") {
        return Some(dock_home().join("scratch").join("cache-debug.log"));
    }
    Some(PathBuf::from(raw))
}

fn append(lines: &str) {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // 写不进去就算了：诊断日志绝不能把一次采样搞失败。
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(lines.as_bytes());
    }
}

fn stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 只要能把几条记录排出先后就够，不值当为此拉一个时间库。
    let secs = now % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// 取请求体里参与前缀的三块。三条 wire 的键名不同，但都是这三样。
fn parts(body: &Value) -> (String, String, Vec<String>) {
    let system = match &body["system"] {
        Value::Null => String::new(),
        v => v.to_string(),
    };
    let tools = match &body["tools"] {
        Value::Null => String::new(),
        v => v.to_string(),
    };
    let arr = body
        .get("messages")
        .or_else(|| body.get("input"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (system, tools, arr.iter().map(Value::to_string).collect())
}

/// 首个不同的字节偏移（落在字符边界上）。
fn first_diff(a: &str, b: &str) -> usize {
    let n = a
        .as_bytes()
        .iter()
        .zip(b.as_bytes())
        .take_while(|(x, y)| x == y)
        .count();
    let mut n = n.min(a.len()).min(b.len());
    while n > 0 && (!a.is_char_boundary(n) || !b.is_char_boundary(n)) {
        n -= 1;
    }
    n
}

fn excerpt(s: &str, from: usize) -> String {
    let mut start = from.min(s.len());
    while start > 0 && !s.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + EXCERPT_BYTES).min(s.len());
    while end > start && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = s[start..end].replace('\n', "\\n");
    if end < s.len() {
        out.push('…');
    }
    out
}

/// `same`，或者「共享 N/M 字节」。
///
/// 只报 same / CHANGED 是不够的：主会话与子代理的 system、tools 本来就不一样，
/// 有意义的问题是**共享了多少**——工具表按「共用在前、专属在后」排序，收益就是
/// 这个 N 变大。
fn blob_note(prev: &str, now: &str) -> String {
    if prev == now {
        return "same".to_string();
    }
    format!("共享 {}B/{}B", first_diff(prev, now), now.len())
}

fn role_of(serialized: &str) -> String {
    serde_json::from_str::<Value>(serialized)
        .ok()
        .and_then(|v| {
            v.get("role")
                .or_else(|| v.get("type"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "?".into())
}

/// 记一次请求，并与同一会话上一次请求比前缀。
pub(crate) fn record_request(identity: &str, wire: &str, model: &str, body: &Value) {
    if log_path().is_none() {
        return;
    }
    let (system, tools, messages) = parts(body);
    let mut out = format!("\n[{}] {wire} {model} session={identity}\n", stamp());
    let mut guard = PREV.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    match map.get(identity) {
        None => {
            out.push_str(&format!(
                "  首次请求：消息 {} 条，system {}B，tools {}B\n",
                messages.len(),
                system.len(),
                tools.len()
            ));
        }
        Some(prev) => {
            let same = prev
                .messages
                .iter()
                .zip(&messages)
                .take_while(|(a, b)| a == b)
                .count();
            let head: usize = messages[..same].iter().map(String::len).sum();
            let system_note = blob_note(&prev.system, &system);
            let tools_note = blob_note(&prev.tools, &tools);
            // 「同前 N / 上次 M」：N < M 就是前缀被改写了，N == M 是纯追加。
            let flag = if same < prev.messages.len() {
                " ⚠"
            } else {
                ""
            };
            out.push_str(&format!(
                "  消息 {} 条 · 同前 {same}/{}{flag} · 可缓存头 {head}B · system {system_note} · tools {tools_note}\n",
                messages.len(),
                prev.messages.len(),
            ));
            if same < prev.messages.len() {
                let now = messages.get(same);
                let was = &prev.messages[same];
                match now {
                    None => out.push_str(&format!(
                        "  ⚠ message[{same}] 被删掉了（role={}，{}B）\n    旧: {}\n",
                        role_of(was),
                        was.len(),
                        excerpt(was, 0)
                    )),
                    Some(now) => {
                        let at = first_diff(was, now);
                        out.push_str(&format!(
                            "  ⚠ message[{same}] role={} 变了 · 旧 {}B 新 {}B · 首个差异 @{at}B\n    旧: {}\n    新: {}\n",
                            role_of(now),
                            was.len(),
                            now.len(),
                            excerpt(was, at),
                            excerpt(now, at),
                        ));
                    }
                }
            }
        }
    }
    map.insert(
        identity.to_string(),
        Prev {
            system,
            tools,
            messages,
        },
    );
    drop(guard);
    append(&out);
}

/// 把上游报回来的用量贴在刚才那条记录下面。非官方（本地估算）的忽略。
pub(crate) fn note_usage(delta: &StreamDelta) {
    if log_path().is_none() {
        return;
    }
    let StreamDelta::Usage {
        tokens,
        official: true,
        ..
    } = delta
    else {
        return;
    };
    let miss = tokens
        .prompt_tokens
        .saturating_sub(tokens.cached_prompt_tokens)
        .saturating_sub(tokens.cache_creation_prompt_tokens);
    append(&format!(
        "  usage prompt {} · read {} · write {} · miss {} · out {}\n",
        tokens.prompt_tokens,
        tokens.cached_prompt_tokens,
        tokens.cache_creation_prompt_tokens,
        miss,
        tokens.completion_tokens,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(msgs: &[&str]) -> Value {
        json!({
            "system": "you are dock",
            "tools": [{"name": "read_file"}],
            "messages": msgs.iter().map(|m| json!({"role": "user", "content": m})).collect::<Vec<_>>(),
        })
    }

    /// 纯追加 = 同前 N 等于上一次的全部条数；中途改写 = 同前更少，且能指出是
    /// 哪一条。这两句正是日志要回答的问题。
    #[test]
    fn prefix_comparison_separates_append_from_rewrite() {
        let (_, _, first) = parts(&body(&["a", "b"]));
        let (_, _, appended) = parts(&body(&["a", "b", "c"]));
        let same = first
            .iter()
            .zip(&appended)
            .take_while(|(x, y)| x == y)
            .count();
        assert_eq!(same, first.len(), "纯追加不该动到已有前缀");

        let (_, _, rewritten) = parts(&body(&["a", "B!", "c"]));
        let same = first
            .iter()
            .zip(&rewritten)
            .take_while(|(x, y)| x == y)
            .count();
        assert_eq!(same, 1, "第 2 条被改写了");
        assert_eq!(role_of(&rewritten[1]), "user");
        let at = first_diff(&first[1], &rewritten[1]);
        assert!(
            excerpt(&rewritten[1], at).starts_with("B!"),
            "{}",
            excerpt(&rewritten[1], at)
        );
    }

    /// 多字节字符被切一半会让日志自己 panic —— 诊断工具不许把会话搞崩。
    #[test]
    fn excerpts_stay_on_char_boundaries() {
        let a = "{\"content\":\"已中断。\"}";
        let b = "{\"content\":\"已完成。\"}";
        let at = first_diff(a, b);
        assert!(a.is_char_boundary(at) && b.is_char_boundary(at));
        assert!(!excerpt(b, at).is_empty());
        assert_eq!(
            excerpt(&"数".repeat(200), 0).chars().count(),
            EXCERPT_BYTES / 3 + 1
        );
    }

    /// 默认（没设 / 设成 0 / off）一行都不写，也不去碰文件系统。
    ///
    /// 每种取值各自开一个作用域：`scoped()` 是同一把进程级互斥，guard 还活着就
    /// 再要一把会自锁（模块文档里那句「一次 `scoped()` 只持一把锁」说的就是这个）。
    #[test]
    fn disabled_unless_asked_for() {
        {
            let _env = crate::test_env::scoped().remove("DOCK_CACHE_DEBUG");
            assert!(log_path().is_none(), "没设就不该开");
        }
        {
            let _env = crate::test_env::scoped().set("DOCK_CACHE_DEBUG", "0");
            assert!(log_path().is_none(), "0 = 关");
        }
        {
            let _env = crate::test_env::scoped()
                .home()
                .set("DOCK_CACHE_DEBUG", "1");
            assert!(log_path()
                .expect("1 = 开")
                .ends_with("scratch/cache-debug.log"));
        }
        {
            let _env = crate::test_env::scoped().set("DOCK_CACHE_DEBUG", "/tmp/dock-cache.log");
            assert_eq!(log_path(), Some(PathBuf::from("/tmp/dock-cache.log")));
        }
    }
}
