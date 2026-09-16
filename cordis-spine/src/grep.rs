//! `grep`：进程内 ripgrep。
//!
//! 走 ripgrep 自己的库（`ignore` 走目录、`grep-regex` 编译模式、`grep-searcher`
//! 扫文件），**不 spawn `rg` 二进制**——codex 的 `file-search` 是同一个形态。
//! 这样消掉的是一整类不确定性：旧实现 `Command::new("rg")` 失败才回落到一个
//! 手搓的递归 walker，那条兜底既不读 `.gitignore`、regex 方言也不同，等于
//! 「装没装 rg，搜索结果不一样」。
//!
//! 为什么不删掉这颗工具、让模型走 `bash` 里的 `rg`（codex 那样）：dock 的权限
//! 门、计划门、预设 allowlist 全是 **tool-level** 的（`acp.rs` 的
//! `gated_builtin` 里有 `bash`、没有 `grep`）。`presets/code/agents/explore.yml`
//! 和 `plan.yml` 明确列了 `grep`、明确不含 `bash`，计划模式下 `bash` 也被挡；
//! 搜索一旦归进 `bash`，这两颗只读子代理和整个计划模式就都搜不了了。
//!
//! 输出预算走 `crate::tool_output`：按**匹配行**分页（不是字节），回报
//! 「显示 N / 共 M」，溢出落盘并回路径。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::overrides::OverrideBuilder;
use ignore::types::TypesBuilder;
use ignore::WalkBuilder;
use serde_json::Value;

use crate::tool_output::{self, Budget, Counts};

/// 省略 `head_limit` 时的内联匹配行数（Grok `CONTENT_LINE_DEFAULT`）。
const CONTENT_LINE_DEFAULT: usize = 200;
/// 显式 `head_limit` 的硬上限（Grok `CONTENT_LINE_LIMIT`）。
const CONTENT_LINE_LIMIT: usize = 2_000;
/// `files_with_matches` / `count` 省略 `head_limit` 时的条目数。
const ENTRY_DEFAULT: usize = 500;
/// `files_with_matches` / `count` 的硬上限。
const ENTRY_LIMIT: usize = 10_000;
/// 单行超过这个长度就截断——一行几 MB 的 minified 文件不该吃掉整个窗口
/// （Grok `DEFAULT_MAX_CHARS_PER_LINE`）。
const MAX_CHARS_PER_LINE: usize = 1_000;
/// 整趟搜索的墙钟预算。到点把已经拿到的结果带回来，不是报错清空。
const WALL_CLOCK: Duration = Duration::from_secs(20);
/// 字节兜底帽。语义分页兜不住的才由它拦。
const MAX_OUTPUT_BYTES: usize = 40_000;

pub const PARAMS: &str = r#"{"type":"object","properties":{
"pattern":{"type":"string","description":"Regular expression to search for (ripgrep syntax). Escape literal special characters: `functionCall\\(`, or `interface\\{\\}` to match Go's interface{}."},
"path":{"type":"string","description":"File or directory to search. Defaults to the current working directory; a relative path resolves against it."},
"glob":{"type":"string","description":"Glob filter for which files to search, e.g. \"*.rs\" or \"*.{ts,tsx}\". Prefix with ! to exclude. Use when you know the filename shape."},
"type":{"type":"string","description":"File type to search, e.g. rust, py, js, ts, go, java, md. More efficient than glob for standard types. Ignored if unknown."},
"-i":{"type":"boolean","description":"Case insensitive search."},
"-A":{"type":"integer","description":"Lines of context to show after each match."},
"-B":{"type":"integer","description":"Lines of context to show before each match."},
"-C":{"type":"integer","description":"Lines of context before and after each match. Overrides -A/-B."},
"output_mode":{"type":"string","enum":["content","files_with_matches","count"],"description":"content (default) returns matching lines; files_with_matches returns only paths; count returns per-file match counts. Use files_with_matches first when you only need to know where something lives."},
"head_limit":{"type":"integer","description":"Cap inline results, like | head -N. Defaults to 200 matching lines (content) or 500 entries (other modes)."},
"multiline":{"type":"boolean","description":"Let . match newlines so a pattern can span lines."}
},"required":["pattern"]}"#;

pub const DESCRIPTION: &str = "Search file contents with regular expressions (ripgrep, in-process).
- Use this instead of running `grep` or `rg` through bash: it is gated as read-only, works in plan mode, and reports how many matches it did not show.
- Full regex syntax, so escape literal special characters: `functionCall\\(`.
- Respects .gitignore and skips binary files. Hidden files are skipped.
- Narrow with `glob` or `type` only when you are sure of the file type; import paths often do not match source extensions (.js vs .ts).
- Start with `output_mode: \"files_with_matches\"` to locate code, then read the file or re-grep with `-C` for context.
- Output is ripgrep-style: ':' marks match lines, '-' marks context lines, grouped by file. Capped results say so and report the full count.";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OutputMode {
    #[default]
    Content,
    FilesWithMatches,
    Count,
}

pub struct Input {
    pattern: String,
    path: PathBuf,
    glob: Option<String>,
    file_type: Option<String>,
    case_insensitive: bool,
    before: usize,
    after: usize,
    head_limit: usize,
    multiline: bool,
    mode: OutputMode,
}

fn str_field(v: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = v.get(*key).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// 模型时常把整数写成字符串、把布尔写成 `"true"`，宽松解析比报错有用。
fn usize_field(v: &Value, keys: &[&str]) -> Option<usize> {
    for key in keys {
        let found = v.get(*key).and_then(|x| {
            x.as_u64()
                .or_else(|| x.as_i64().and_then(|n| u64::try_from(n).ok()))
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
        });
        if let Some(n) = found {
            return Some(n as usize);
        }
    }
    None
}

fn bool_field(v: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|key| {
        v.get(*key)
            .and_then(|x| {
                x.as_bool()
                    .or_else(|| x.as_str().map(|s| s.eq_ignore_ascii_case("true")))
            })
            .unwrap_or(false)
    })
}

pub fn parse(args: &str) -> Result<Input, String> {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let Some(pattern) = str_field(&v, &["pattern", "regex", "query"]) else {
        return Err("Error: pattern is required".into());
    };
    let mode = match str_field(&v, &["output_mode"]).as_deref() {
        Some("files_with_matches") | Some("files") => OutputMode::FilesWithMatches,
        Some("count") => OutputMode::Count,
        _ => OutputMode::Content,
    };
    let path = str_field(&v, &["path", "target_directory"]).unwrap_or_else(|| ".".into());
    let path = {
        let p = PathBuf::from(&path);
        if p.is_absolute() {
            p
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(p)
        }
    };
    let context = usize_field(&v, &["-C", "context"]);
    let (default, cap) = match mode {
        OutputMode::Content => (CONTENT_LINE_DEFAULT, CONTENT_LINE_LIMIT),
        _ => (ENTRY_DEFAULT, ENTRY_LIMIT),
    };
    Ok(Input {
        pattern,
        path,
        glob: str_field(&v, &["glob", "include"]),
        file_type: str_field(&v, &["type"]),
        case_insensitive: bool_field(&v, &["-i", "case_insensitive", "ignore_case"]),
        before: context
            .or_else(|| usize_field(&v, &["-B", "before_context"]))
            .unwrap_or(0),
        after: context
            .or_else(|| usize_field(&v, &["-A", "after_context"]))
            .unwrap_or(0),
        head_limit: usize_field(&v, &["head_limit", "limit"])
            .unwrap_or(default)
            .clamp(1, cap),
        multiline: bool_field(&v, &["multiline", "-U"]),
        mode,
    })
}

/// 一个文件的搜索结果。
#[derive(Default)]
struct FileHits {
    /// 渲染好的行（`path:line:text` / `path-line-text`）。
    lines: Vec<String>,
    /// 匹配数（`matched` 被调用的次数，多行模式下一次可跨多行）。
    matches: usize,
}

/// 收集一个文件里的命中与上下文行；额度用尽就让 searcher 停下。
struct Collector<'a> {
    display: &'a str,
    hits: FileHits,
    /// 本文件还能再收多少行。0 = 已满。
    remaining: usize,
    /// 是否因为额度用尽提前停的。
    hit_limit: bool,
    /// 是否要收集正文。只有 `content` 模式要。
    collect_lines: bool,
    /// 命中一次就够——只有 `files_with_matches` 能这么停。
    ///
    /// `count` **不能**：它要的就是真实计数，第一处命中就返回会把每个文件都
    /// 报成 1。
    stop_at_first: bool,
}

impl Collector<'_> {
    fn push(&mut self, sep: char, line_number: u64, bytes: &[u8]) -> bool {
        if self.remaining == 0 {
            self.hit_limit = true;
            return false;
        }
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end_matches(['\n', '\r']);
        let text = truncate_chars(text, MAX_CHARS_PER_LINE);
        self.hits
            .lines
            .push(format!("{}{sep}{line_number}{sep}{text}", self.display));
        self.remaining -= 1;
        true
    }
}

/// 按**字符**边界截断，不按字节——按字节切会把中文切成非法 UTF-8。
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    let dropped = text.chars().count() - max;
    format!("{head}… [line truncated: {dropped} more characters]")
}

impl Sink for Collector<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.hits.matches += 1;
        if self.stop_at_first {
            // 只要知道「这个文件有命中」，不必把文件读完。
            return Ok(false);
        }
        if !self.collect_lines {
            // `count`：继续扫完以拿到真实计数，但不收正文。
            return Ok(true);
        }
        // 多行模式下一次命中可以跨多行，行号从命中的首行往下递增。
        let first = mat.line_number().unwrap_or(0);
        for (n, line) in (first..).zip(mat.lines()) {
            if !self.push(':', n, line) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn context(&mut self, _s: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, Self::Error> {
        if !self.collect_lines {
            return Ok(true);
        }
        Ok(self.push('-', ctx.line_number().unwrap_or(0), ctx.bytes()))
    }
}

/// 把绝对路径显示成相对 cwd 的形式（对齐 rg 的输出习惯）。
fn display_path(path: &Path, root: &Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| root.to_path_buf());
    path.strip_prefix(&cwd)
        .or_else(|_| path.strip_prefix(root))
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub async fn run(call_id: &str, args: &str) -> String {
    let input = match parse(args) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let call_id = call_id.to_string();
    // 目录遍历与文件扫描都是阻塞 IO，别占着 tokio 的 worker。
    let joined = tokio::task::spawn_blocking(move || search(&input)).await;
    let outcome = match joined {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return e,
        Err(e) => return format!("Error: grep failed to run: {e}"),
    };
    render(&call_id, outcome).await
}

struct Outcome {
    body: String,
    /// 完整正文（含被截掉的部分），只在截断时用来落盘。
    full: Option<String>,
    counts: Counts,
    unit: &'static str,
    next_hint: &'static str,
    /// 搜了什么——空结果时明确回报，免得模型把「工具坏了」和「真没有」搞混。
    scope: String,
}

fn search(input: &Input) -> Result<Outcome, String> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(input.case_insensitive)
        .multi_line(input.multiline)
        .dot_matches_new_line(input.multiline)
        .build(&input.pattern)
        .map_err(|e| format!("Error: invalid regex `{}`: {e}", input.pattern))?;

    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .multi_line(input.multiline)
        // NUL 字节即判定为二进制并停搜：不这么做会把 .o / .png 的字节当文本吐出来。
        .binary_detection(BinaryDetection::quit(0))
        .before_context(input.before)
        .after_context(input.after)
        .build();

    let mut walk = WalkBuilder::new(&input.path);
    // 顺序遍历 + 路径排序：工具结果要可复现，rg 的并行遍历顺序是不稳的。
    walk.sort_by_file_path(|a, b| a.cmp(b));
    // 有意偏离 `rg` CLI 的默认（它是 `require_git(true)`，只在 git 仓库里才读
    // ignore 文件）：工具对模型的承诺是「respects .gitignore」，那就不该随
    // 「当前目录恰好是不是 git 仓库」变。一个写了 `.gitignore` 的非 git 目录，
    // 意图是明确的。
    walk.require_git(false);
    if let Some(glob) = input.glob.as_deref() {
        let mut ob = OverrideBuilder::new(&input.path);
        ob.add(glob)
            .map_err(|e| format!("Error: invalid glob `{glob}`: {e}"))?;
        walk.overrides(
            ob.build()
                .map_err(|e| format!("Error: invalid glob `{glob}`: {e}"))?,
        );
    }
    let mut type_note = String::new();
    if let Some(ty) = input.file_type.as_deref() {
        let mut tb = TypesBuilder::new();
        tb.add_defaults();
        tb.select(ty);
        match tb.build() {
            Ok(types) => {
                walk.types(types);
            }
            // 未知类型名不该让整次搜索失败——降级成不过滤，并在结果里说清楚。
            Err(_) => {
                type_note =
                    format!(" (`type: {ty}` is not a known type name; the filter was ignored)")
            }
        }
    }

    let collect_lines = input.mode == OutputMode::Content;
    let stop_at_first = input.mode == OutputMode::FilesWithMatches;
    let deadline = Instant::now() + WALL_CLOCK;
    let mut all_lines: Vec<String> = Vec::new();
    let mut entries: Vec<String> = Vec::new();
    let mut remaining = input.head_limit;
    let mut truncated = false;
    let mut timed_out = false;
    let mut total_seen = 0usize;

    for dirent in walk.build() {
        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        let Ok(dirent) = dirent else { continue };
        if !dirent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = dirent.path();
        let display = display_path(path, &input.path);
        let mut collector = Collector {
            display: &display,
            hits: FileHits::default(),
            remaining: if collect_lines { remaining } else { usize::MAX },
            hit_limit: false,
            collect_lines,
            stop_at_first,
        };
        // 读不动的文件（权限、坏软链）跳过，不让一个文件废掉整次搜索。
        if searcher
            .search_path(&matcher, path, &mut collector)
            .is_err()
        {
            continue;
        }
        if collector.hits.matches == 0 {
            continue;
        }
        total_seen += 1;
        match input.mode {
            OutputMode::Content => {
                let hit_limit = collector.hit_limit;
                let taken = collector.hits.lines.len();
                all_lines.extend(collector.hits.lines);
                remaining = remaining.saturating_sub(taken);
                if hit_limit || remaining == 0 {
                    truncated = true;
                    break;
                }
            }
            OutputMode::FilesWithMatches => {
                entries.push(display.clone());
                if entries.len() >= input.head_limit {
                    truncated = true;
                    break;
                }
            }
            OutputMode::Count => {
                entries.push(format!("{display}:{}", collector.hits.matches));
                if entries.len() >= input.head_limit {
                    truncated = true;
                    break;
                }
            }
        }
    }

    let scope = describe_scope(input, &type_note, timed_out);
    let (rendered, shown, unit, next_hint) = match input.mode {
        OutputMode::Content => (
            all_lines.join("\n"),
            all_lines.len(),
            "matching lines",
            " Narrow the pattern, add a glob/type filter, or raise head_limit.",
        ),
        _ => (
            entries.join("\n"),
            entries.len(),
            "files",
            " Narrow the pattern or raise head_limit.",
        ),
    };

    if shown == 0 {
        return Ok(Outcome {
            body: format!("no matches\n{scope}"),
            full: None,
            counts: Counts::exact(0, 0),
            unit,
            next_hint,
            scope,
        });
    }

    // 额度填满就停了走查，所以总量是「至少」而不是精确值——不能谎报精确数。
    let counts = if truncated || timed_out {
        Counts::at_least(shown)
    } else {
        Counts::exact(shown, shown)
    };
    Ok(Outcome {
        full: counts.truncated().then(|| rendered.clone()),
        body: rendered,
        counts,
        unit,
        next_hint,
        scope: if total_seen > 0 { scope } else { String::new() },
    })
}

/// 「搜了什么范围」。空结果时这一句是模型判断「没有」还是「搜错地方」的唯一依据。
fn describe_scope(input: &Input, type_note: &str, timed_out: bool) -> String {
    let mut bits = vec![format!("searched {}", input.path.display())];
    if let Some(g) = input.glob.as_deref() {
        bits.push(format!("glob={g}"));
    }
    if let Some(t) = input.file_type.as_deref() {
        bits.push(format!("type={t}"));
    }
    if input.case_insensitive {
        bits.push("case-insensitive".into());
    }
    bits.push(".gitignore applied; hidden and binary files skipped".into());
    let mut s = bits.join("; ");
    if !type_note.is_empty() {
        s.push_str(type_note);
    }
    if timed_out {
        s.push_str(&format!(
            " NOTE: the search hit its {}s wall-clock budget and stopped; the above is only what \
             it had scanned by then.",
            WALL_CLOCK.as_secs()
        ));
    }
    s
}

async fn render(call_id: &str, outcome: Outcome) -> String {
    let Outcome {
        body,
        full,
        counts,
        unit,
        next_hint,
        scope,
    } = outcome;
    let spill = match full {
        Some(full) => tool_output::offload(call_id, &full).await,
        None => None,
    };
    let footer = tool_output::footer(&counts, unit, next_hint, spill.as_deref());
    let mut out = body;
    if !footer.is_empty() {
        out.push_str(&footer);
        if !scope.is_empty() {
            out.push_str(&format!("\n{scope}"));
        }
    }
    tool_output::cap_bytes(call_id, out, &Budget::list(MAX_OUTPUT_BYTES)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/a.rs"),
            "fn alpha() {}\nfn beta() {}\nlet x = ALPHA;\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/b.ts"), "const alpha = 1;\n").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("ignored")).unwrap();
        std::fs::write(dir.path().join("ignored/c.rs"), "fn alpha() {}\n").unwrap();
        dir
    }

    async fn grep(dir: &tempfile::TempDir, args: serde_json::Value) -> String {
        let mut args = args;
        args["path"] = serde_json::json!(dir.path());
        run("t", &args.to_string()).await
    }

    #[tokio::test]
    async fn finds_matches_with_line_numbers() {
        let dir = fixture();
        let out = grep(&dir, serde_json::json!({"pattern": "alpha"})).await;
        assert!(out.contains("src/a.rs:1:fn alpha() {}"), "{out}");
        assert!(out.contains("src/b.ts:1:const alpha = 1;"), "{out}");
    }

    /// 旧实现 spawn 失败时回落的手搓 walker 不读 .gitignore，导致「装没装 rg
    /// 结果不一样」。进程内实现必须始终过滤。
    #[tokio::test]
    async fn respects_gitignore() {
        let dir = fixture();
        let out = grep(&dir, serde_json::json!({"pattern": "alpha"})).await;
        assert!(
            !out.contains("ignored/c.rs"),
            "被 .gitignore 排除的文件不该出现：{out}"
        );
    }

    #[tokio::test]
    async fn case_insensitive_flag() {
        let dir = fixture();
        let sensitive = grep(&dir, serde_json::json!({"pattern": "ALPHA"})).await;
        assert!(!sensitive.contains("a.rs:1:"), "{sensitive}");
        let insensitive = grep(&dir, serde_json::json!({"pattern": "ALPHA", "-i": true})).await;
        assert!(insensitive.contains("src/a.rs:1:"), "{insensitive}");
    }

    #[tokio::test]
    async fn type_filter_uses_ripgrep_default_table() {
        let dir = fixture();
        let out = grep(
            &dir,
            serde_json::json!({"pattern": "alpha", "type": "rust"}),
        )
        .await;
        assert!(out.contains("src/a.rs"), "{out}");
        assert!(!out.contains("src/b.ts"), "type=rust 不该带上 .ts：{out}");
    }

    /// 未知类型名降级成不过滤并说明，不能让整次搜索失败。
    #[tokio::test]
    async fn unknown_type_degrades_instead_of_failing() {
        let dir = fixture();
        let out = grep(
            &dir,
            serde_json::json!({"pattern": "alpha", "type": "nosuchtype"}),
        )
        .await;
        assert!(out.contains("src/a.rs"), "{out}");
    }

    #[tokio::test]
    async fn glob_filter() {
        let dir = fixture();
        let out = grep(
            &dir,
            serde_json::json!({"pattern": "alpha", "glob": "*.ts"}),
        )
        .await;
        assert!(out.contains("src/b.ts"), "{out}");
        assert!(!out.contains("src/a.rs"), "{out}");
    }

    #[tokio::test]
    async fn context_lines() {
        let dir = fixture();
        let out = grep(&dir, serde_json::json!({"pattern": "beta", "-C": 1})).await;
        assert!(
            out.contains("src/a.rs-1-fn alpha() {}"),
            "前一行应作为上下文：{out}"
        );
        assert!(out.contains("src/a.rs:2:fn beta() {}"), "{out}");
        assert!(
            out.contains("src/a.rs-3-let x = ALPHA;"),
            "后一行应作为上下文：{out}"
        );
    }

    #[tokio::test]
    async fn files_with_matches_mode() {
        let dir = fixture();
        let out = grep(
            &dir,
            serde_json::json!({"pattern": "alpha", "output_mode": "files_with_matches"}),
        )
        .await;
        assert!(out.contains("src/a.rs"), "{out}");
        assert!(!out.contains(":1:"), "只要路径，不要正文：{out}");
    }

    #[tokio::test]
    async fn count_mode() {
        let dir = fixture();
        let out = grep(
            &dir,
            serde_json::json!({"pattern": "alpha", "output_mode": "count", "-i": true}),
        )
        .await;
        assert!(out.contains("src/a.rs:2"), "a.rs 有两处 alpha/ALPHA：{out}");
    }

    /// 空结果必须说清楚搜了什么范围，否则模型分不清「真没有」和「工具坏了」，
    /// 一旦判成后者就整场会话改用 bash。
    #[tokio::test]
    async fn empty_result_reports_scope() {
        let dir = fixture();
        let out = grep(&dir, serde_json::json!({"pattern": "zzz-not-there"})).await;
        assert!(out.contains("no matches"), "{out}");
        assert!(out.contains("searched"), "{out}");
        assert!(out.contains(".gitignore"), "要说明做了哪些过滤：{out}");
    }

    /// 截断必须回报「至少 N」并给落盘路径——否则模型会把前 N 条当成全部。
    #[tokio::test]
    async fn head_limit_truncates_with_count_and_spill() {
        let _env = crate::test_env::scoped().home();
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=50).map(|i| format!("hit line {i}\n")).collect();
        std::fs::write(dir.path().join("big.txt"), body).unwrap();
        let out = run(
            "spill-1",
            &serde_json::json!({"pattern": "hit", "path": dir.path(), "head_limit": 5}).to_string(),
        )
        .await;
        assert_eq!(out.matches("big.txt:").count(), 5, "只内联 5 行：{out}");
        assert!(out.contains("at least"), "要说是至少而不是精确总数：{out}");
        let spill = tool_output::spill_dir().join("spill-1.txt");
        assert!(spill.exists(), "完整结果应落盘：{}", spill.display());
        assert_eq!(std::fs::read_to_string(&spill).unwrap().lines().count(), 5);
    }

    /// 没截断时不许挂「至少」页脚——那会让模型白白多搜一轮。
    #[tokio::test]
    async fn complete_result_has_no_truncation_footer() {
        let dir = fixture();
        let out = grep(&dir, serde_json::json!({"pattern": "beta"})).await;
        assert!(!out.contains("truncated"), "{out}");
    }

    /// 单行几 MB 的 minified 文件不该吃掉整个窗口。
    #[tokio::test]
    async fn long_line_is_truncated_per_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("min.js"),
            format!("needle{}\n", "x".repeat(5_000)),
        )
        .unwrap();
        let out = run(
            "t",
            &serde_json::json!({"pattern": "needle", "path": dir.path()}).to_string(),
        )
        .await;
        assert!(out.contains("line truncated"), "{out}");
        assert!(
            out.len() < 3_000,
            "单行截断后整体应很小，实际 {}",
            out.len()
        );
    }

    /// 二进制文件不该把字节当文本吐出来。
    #[tokio::test]
    async fn binary_files_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blob.bin"), b"needle\x00\x01\x02needle").unwrap();
        let out = run(
            "t",
            &serde_json::json!({"pattern": "needle", "path": dir.path()}).to_string(),
        )
        .await;
        assert!(!out.contains("\u{1}"), "不该吐出二进制字节：{out:?}");
    }

    #[tokio::test]
    async fn invalid_regex_explains_itself() {
        let dir = fixture();
        let out = grep(&dir, serde_json::json!({"pattern": "("})).await;
        assert!(out.contains("invalid regex"), "{out}");
    }

    #[test]
    fn head_limit_is_clamped_to_the_hard_cap() {
        let input = parse(r#"{"pattern":"x","head_limit":999999}"#).unwrap();
        assert_eq!(input.head_limit, CONTENT_LINE_LIMIT);
        let files = parse(r#"{"pattern":"x","output_mode":"count","head_limit":999999}"#).unwrap();
        assert_eq!(files.head_limit, ENTRY_LIMIT);
    }

    /// `-C` 要同时盖住 `-A` / `-B`（对齐 rg）。
    #[test]
    fn context_flag_overrides_before_and_after() {
        let input = parse(r#"{"pattern":"x","-A":1,"-B":1,"-C":5}"#).unwrap();
        assert_eq!((input.before, input.after), (5, 5));
    }
}
