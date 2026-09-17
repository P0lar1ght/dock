//! 工具输出的共享预算：**语义分页 + 回报总数 + 溢出落盘回路径**。
//!
//! 三层，缺一层就会丢信息：
//!
//! 1. **语义分页**——每颗工具按自己的单位设限（grep 数匹配行、`list_dir` 数
//!    条目、`read_file` 数行），不是按字节切。字节数只是副产品。
//! 2. **回报总数**——「显示 200 条 / 至少 1847 条」和「200 条」是两种完全不同
//!    的信号：前者模型会去收窄条件或翻页，后者它会当成结论。
//! 3. **溢出落盘**——完整结果写进 `$DOCK_HOME/tool-output/<call_id>.txt`，截断
//!    提示里带路径。有这条回路，「截断」就不是丢信息，而是把信息从上下文降级
//!    成可寻址：模型能用 `read_file` / `grep` 把剩下的捞回来。
//!
//! 这套原先是 `mcp/discover.rs` 的私有实现，只服务 `use_tool`；提到这里之后
//! 内置工具共用同一套语义与同一个落盘目录。
//!
//! 落盘是 **best-effort**：写盘失败绝不能让工具调用失败，也不能把已经拿到的
//! 内联结果吞掉——退回纯截断，提示里照实说没存下来（对齐 DSH
//! `trySaveFormattedResult`）。

use std::path::PathBuf;

/// 字节兜底帽。语义分页兜不住的病态输入（单行几 MB 的 minified 文件）由它拦。
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 20_000;

/// 齐质列表（grep 命中行、`list_dir` 条目、`glob` 路径）的截断方向。
///
/// 列表保头：第 1..N 条是完整的，尾部是被丢掉的那部分，配合「至少 M 条」的
/// 计数与落盘路径，模型知道自己看到的是前缀而不是全部。**不要**对列表做
/// 头尾各留一半的截断——中间挖个洞，模型无从知道挖掉了什么。
///
/// `bash` 那种「结论在尾巴上」的输出是另一回事，由 `jobs` 自己保头 4KB +
/// 尾 16KB，不走这里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// 齐质列表：保头，丢尾。
    List,
    /// 不透明文本：保头，丢尾（与 `use_tool` 的历史行为一致）。
    Opaque,
}

/// 一次工具调用的输出预算。
#[derive(Clone, Debug)]
pub struct Budget {
    /// 字节兜底帽。
    pub max_bytes: usize,
    /// 截断形状。
    pub shape: Shape,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            shape: Shape::Opaque,
        }
    }
}

impl Budget {
    pub fn list(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            shape: Shape::List,
        }
    }

    pub fn opaque(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            shape: Shape::Opaque,
        }
    }
}

/// 语义分页的计数，渲染成「显示 N / 至少 M」那一句。
///
/// `total` 为 `None` 表示上游自己也没数完（例如额度填满就把搜索停了）——那就
/// 只能说「至少」。这正是 Grok grep 的 `report "at least" counts`。
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    /// 内联返回的条数。
    pub shown: usize,
    /// 总条数；`None` = 未数完，只知道 `shown` 是前缀。
    pub total: Option<usize>,
}

impl Counts {
    pub fn exact(shown: usize, total: usize) -> Self {
        Self {
            shown,
            total: Some(total),
        }
    }

    /// 额度填满就停了，没数完总量。
    pub fn at_least(shown: usize) -> Self {
        Self { shown, total: None }
    }

    /// 是否还有没返回的内容。
    pub fn truncated(&self) -> bool {
        match self.total {
            Some(total) => total > self.shown,
            None => true,
        }
    }
}

/// 把完整内容写进 `$DOCK_HOME/tool-output/<call_id>.txt`，返回路径。
///
/// best-effort：任何一步失败都回 `None`，调用方退回纯截断。
pub async fn offload(call_id: &str, content: &str) -> Option<String> {
    let dir = spill_dir();
    tokio::fs::create_dir_all(&dir).await.ok()?;
    let path = dir.join(format!("{}.txt", offload_stem(call_id)));
    tokio::fs::write(&path, content).await.ok()?;
    Some(path.to_string_lossy().into_owned())
}

pub fn spill_dir() -> PathBuf {
    crate::config::dock_home().join("tool-output")
}

/// 本进程的落盘前缀，进程内固定、跨进程不重复。
///
/// 没有它就会**把错的内容交给模型**：`Jobs::seq` 每进程从 1 重新计数，所以
/// `job-3.txt` 跨重启复用；而旧会话历史里还写着那条路径，`/resume` 之后模型
/// 按路径读回来的是另一条命令的输出。缺文件只是读不到，读到错的东西更糟。
/// 模型自带的 `call_id` 通常唯一，但 `stream_acc` 在 id 缺失时会兜底成
/// `call-0`，同样会撞——所以这条前缀对所有落盘一视同仁地加。
fn process_tag() -> &'static str {
    static TAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TAG.get_or_init(|| {
        uuid::Uuid::now_v7()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect()
    })
}

/// 把 `call_id` 映射成安全文件名：`/` 或 `..` 逃不出 `tool-output/`
/// （同 `session_persist::persist_tool_images`），并带上本进程前缀。
pub fn offload_stem(call_id: &str) -> String {
    let safe: String = call_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() { "tool" } else { &safe };
    format!("{}-{safe}", process_tag())
}

/// 落盘文件的保留期。超过就删。
const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);
/// 整个目录的总量上限。仅靠时限挡不住「一周内跑了两百次 `cargo test`」。
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// 清一次 `$DOCK_HOME/tool-output/`：先按时限删，仍超总量就从最旧的接着删。
///
/// **只在启动时跑。** 本进程自己的文件带着本进程的 [`process_tag`]，而启动这
/// 一刻还一个都没写，所以这轮清理不可能删掉正在用的文件。会话中途跑才有那个
/// 风险——那也正是这里不提供定时清理的原因。
///
/// 全程 best-effort：读不动目录、删不掉文件都直接放过，绝不让清理影响启动。
pub fn gc_spill_dir() {
    gc_spill_dir_with(MAX_AGE, MAX_TOTAL_BYTES);
}

/// [`gc_spill_dir`] 的可注入阈值版本。测试用它避免真的造 512MB 或等一周。
fn gc_spill_dir_with(max_age: std::time::Duration, max_total: u64) {
    let dir = spill_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    // (mtime, size, path)
    let mut kept: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        let aged = now
            .duration_since(mtime)
            .map(|age| age > max_age)
            .unwrap_or(false);
        if aged {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        kept.push((mtime, meta.len(), path));
    }
    let mut total: u64 = kept.iter().map(|(_, size, _)| *size).sum();
    if total <= max_total {
        return;
    }
    // 旧的先走。
    kept.sort_by_key(|(mtime, _, _)| *mtime);
    for (_, size, path) in kept {
        if total <= max_total {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

/// 字节兜底帽：超帽先落盘，再按 `shape` 截断并附说明。
///
/// 语义分页已经在工具内部做过了，这里只拦「条数不多但单条巨大」的情况。
pub async fn cap_bytes(call_id: &str, content: String, budget: &Budget) -> String {
    if content.len() <= budget.max_bytes {
        return content;
    }
    let total = content.len();
    let mut end = budget.max_bytes;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    // 文案保持 `use_tool` 原样（英文）：这条路径是它原本就在走的，提到这里
    // 只是换了归属，不该顺手改掉模型已经见过的措辞。
    let hint = match offload(call_id, &content).await {
        Some(path) => format!(
            " Full output written to: {path}. Read that path with read_file or grep to retrieve \
             the rest."
        ),
        None => String::new(),
    };
    let head = &content[..end];
    match budget.shape {
        Shape::List | Shape::Opaque => {
            format!("{head}\n\n[output truncated: showing first {end} of {total} bytes.{hint}]")
        }
    }
}

/// 渲染语义分页的页脚。没有截断时返回空串。
///
/// `unit` 是中文量词（「条匹配」「个条目」「行」），`next_hint` 是告诉模型怎么
/// 拿下一页的一句话（例如 `head_limit` 调大、或收窄 pattern）。
pub fn footer(counts: &Counts, unit: &str, next_hint: &str, spill: Option<&str>) -> String {
    if !counts.truncated() {
        return String::new();
    }
    let head = match counts.total {
        Some(total) => format!("[truncated: showing {} of {total} {unit}.", counts.shown),
        None => format!(
            "[truncated: showing {} {unit}; the search stopped at the limit, so there are at \
             least {} more.",
            counts.shown, counts.shown
        ),
    };
    let spill = match spill {
        Some(path) => {
            format!(" Full result written to: {path}. Read that path with read_file or grep.")
        }
        None => String::new(),
    };
    format!("\n\n{head}{spill}{next_hint}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_cannot_escape_the_folder() {
        for raw in ["../../etc/passwd", "", "call-1_a", "a/b", "..", "C:\\x"] {
            let stem = offload_stem(raw);
            assert!(
                stem.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{raw:?} -> {stem:?} 含有可能越出目录的字符"
            );
        }
        assert!(offload_stem("../../etc/passwd").ends_with("______etc_passwd"));
        assert!(offload_stem("").ends_with("-tool"));
    }

    /// 跨进程不能撞名：`Jobs::seq` 每进程从 1 重数，撞了就会把**另一条命令的
    /// 输出**按旧会话记下的路径交给模型。
    #[test]
    fn stem_is_scoped_to_this_process() {
        let a = offload_stem("job-3");
        assert!(a.ends_with("-job-3"), "{a}");
        assert_ne!(a, "job-3", "必须带进程前缀");
        // 同一进程内稳定，否则同一次调用先写后读会找不到文件。
        assert_eq!(a, offload_stem("job-3"));
    }

    /// 过期文件删掉，没过期的留下。用真实 mtime + 极小保留期，
    /// 不引 `filetime` 只为改一个时间戳。
    #[test]
    fn gc_removes_aged_files_and_keeps_fresh_ones() {
        let _env = crate::test_env::scoped().home();
        let dir = spill_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.txt");
        std::fs::write(&old, b"old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(60));
        let fresh = dir.join("fresh.txt");
        std::fs::write(&fresh, b"fresh").unwrap();

        // 保留期 40ms：old 已经超了，fresh 还没有。
        gc_spill_dir_with(std::time::Duration::from_millis(40), u64::MAX);
        assert!(!old.exists(), "过期文件应被删除");
        assert!(fresh.exists(), "未过期的不该动");
    }

    /// 时限之外还要有总量上限：一周内跑两百次 `cargo test`，光靠时限挡不住。
    /// 超额时从最旧的开始删。
    #[test]
    fn gc_enforces_a_total_size_cap_oldest_first() {
        let _env = crate::test_env::scoped().home();
        let dir = spill_dir();
        std::fs::create_dir_all(&dir).unwrap();
        for (i, name) in ["a.txt", "b.txt", "c.txt"].iter().enumerate() {
            if i > 0 {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            std::fs::write(dir.join(name), vec![b'x'; 100]).unwrap();
        }
        // 总量 300 字节，上限 150 → 最旧的 a（必要时连 b）被删到达标。
        gc_spill_dir_with(std::time::Duration::from_secs(3600), 150);
        assert!(!dir.join("a.txt").exists(), "最旧的应先被删");
        assert!(dir.join("c.txt").exists(), "最新的应保留");
        let total: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        assert!(total <= 150, "清理后应在上限内，实际 {total}");
    }

    /// 没超上限时一个都不该删。
    #[test]
    fn gc_keeps_everything_under_the_cap() {
        let _env = crate::test_env::scoped().home();
        let dir = spill_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep.txt"), b"small").unwrap();
        gc_spill_dir_with(std::time::Duration::from_secs(3600), u64::MAX);
        assert!(dir.join("keep.txt").exists());
    }

    /// 目录不存在时清理是空转，不能 panic —— 首次启动就是这个状态。
    #[test]
    fn gc_on_missing_dir_is_a_noop() {
        let _env = crate::test_env::scoped().home();
        gc_spill_dir();
    }

    #[test]
    fn counts_truncated_only_when_something_is_missing() {
        assert!(!Counts::exact(5, 5).truncated());
        assert!(Counts::exact(5, 6).truncated());
        assert!(Counts::at_least(5).truncated());
    }

    /// 没截断就不该有页脚——否则每次搜索都挂一句噪声。
    #[test]
    fn footer_empty_when_not_truncated() {
        assert_eq!(footer(&Counts::exact(3, 3), "matches", "", None), "");
    }

    /// 「显示 N / 共 M」必须出现，这是模型判断自己有没有看全的唯一依据。
    #[test]
    fn footer_reports_both_numbers_and_spill_path() {
        let out = footer(
            &Counts::exact(200, 1847),
            "matches",
            " Narrow the pattern or raise head_limit.",
            Some("/tmp/x.txt"),
        );
        assert!(out.contains("showing 200 of 1847 matches"), "{out}");
        assert!(out.contains("/tmp/x.txt"), "{out}");
        assert!(out.contains("head_limit"), "{out}");
    }

    /// 额度填满就停搜的情况下说「至少」，不能谎报一个精确总数。
    #[test]
    fn footer_says_at_least_when_total_unknown() {
        let out = footer(&Counts::at_least(200), "matches", "", None);
        assert!(out.contains("at least"), "{out}");
        assert!(!out.contains("of 200 matches"), "不能谎报精确总数：{out}");
    }

    #[tokio::test]
    async fn cap_bytes_passes_through_under_budget() {
        let budget = Budget::opaque(100);
        let out = cap_bytes("c1", "short".into(), &budget).await;
        assert_eq!(out, "short");
    }

    /// 超帽要落盘并把路径写进提示，否则「截断」就是真丢信息。
    #[tokio::test]
    async fn cap_bytes_offloads_and_reports_path() {
        let _env = crate::test_env::scoped().home();
        let budget = Budget::opaque(50);
        let out = cap_bytes("c2", "z".repeat(500), &budget).await;
        assert!(out.contains("output truncated"), "{out}");
        assert!(out.contains("of 500 bytes"), "{out}");
        let path = spill_dir().join(format!("{}.txt", offload_stem("c2")));
        assert!(path.exists(), "完整输出应落盘：{}", path.display());
        assert_eq!(std::fs::read_to_string(&path).unwrap().len(), 500);
    }

    /// 截断点必须落在字符边界上，否则多字节中文会被切碎成非法 UTF-8。
    #[tokio::test]
    async fn cap_bytes_respects_char_boundary() {
        let _env = crate::test_env::scoped().home();
        let budget = Budget::opaque(10);
        let out = cap_bytes("c3", "中文中文中文中文".into(), &budget).await;
        assert!(out.starts_with("中文中"), "{out}");
    }
}
