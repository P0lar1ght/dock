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
//!
//! 遍历仍是 `ignore` 的顺序 + 路径排序（结果要可复现），但**扫描按批并发**：
//! 一批文件里每颗各由一个 worker 扫，扫完按原顺序合并。顺序不变，`head_limit`
//! 的「取前 N 条」语义也不变。批大小自适应（见 [`INITIAL_BATCH`]）：一批是扫完
//! 才合并的，批开太大会让「额度早早填满」的查询白扫一整批。
//!
//! 超过 [`MAX_FILE_BYTES`] 的文件整颗跳过并报数（对齐 Grok `grep` 的
//! `--max-filesize 5M`）：一条几百 MB 的日志能把 20s 墙钟吃光，让结果变成
//! 「看起来搜完了、其实只搜了前缀」。搜索目标本身就是单个文件时不设这道闸。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use grep_regex::{RegexMatcher, RegexMatcherBuilder};
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
/// 单颗文件的字节闸：目录遍历时超过它就整颗跳过，并在结果里报跳过几颗。
/// 对齐 Grok `grep` 的 `--max-filesize 5M`。
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// 一个 worker 一批最多领多少颗文件（批大小的上限，见 [`INITIAL_BATCH`]）。
///
/// 批太小摊不掉线程启动（macOS 上一次几十 µs），批太大又会让「额度填满就停」
/// 的粒度过粗、把没用的命中堆在内存里。
const FILES_PER_WORKER: usize = 32;
/// 第一批只领这么多颗文件。
///
/// 一批是「扫完才合并」的，所以额度在批中间填满也得先把整批扫完。而 content
/// 模式下提前填满是常态（默认 200 行，一颗热门文件就够），批要是一上来就开到
/// 满额，`grep "fn "` 这种查询得先白扫几百颗文件——实测比纯顺序版慢 4.8x。
/// 于是批从小开始、每批没填满就放大 [`BATCH_GROWTH`] 倍：早停最多浪费一小批，
/// 全扫也只多几轮合并。
const INITIAL_BATCH: usize = 16;
/// 上一批没把额度填满，下一批就放大这么多倍（上限 `FILES_PER_WORKER * workers`）。
const BATCH_GROWTH: usize = 4;
/// 少于这么多颗文件就整批顺序扫。
///
/// 搜一个小目录是常见操作，那时线程启动比扫描本身还贵。
const PARALLEL_MIN_BATCH: usize = 8;

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
    /// 调用方会话的工作目录：相对 `path` 按它展开，结果也显示成相对它的路径。
    /// 由调用方传入，不读进程 cwd（同一进程可能同时服务多个项目）。
    cwd: PathBuf,
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

pub fn parse(args: &str, cwd: &Path) -> Result<Input, String> {
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
            cwd.join(p)
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
        cwd: cwd.to_path_buf(),
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
    /// 正在扫的文件。显示名**命中之后**才算（见 [`Collector::prefix`]）。
    path: &'a Path,
    /// 搜索根，用于把绝对路径折回相对形式。
    root: &'a Path,
    /// 启动时取一次的 cwd——不是每颗文件都去 `getcwd`。
    cwd: &'a Path,
    display: Option<String>,
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
    /// 这颗文件的显示名，第一次要用时才算。
    ///
    /// 绝大多数文件不命中，而算显示名要一次 `getcwd` + 一次 String 分配——
    /// 那是**每颗文件**都要付的串行开销，放在命中路径上就白省了。
    fn display(&mut self) -> &str {
        if self.display.is_none() {
            self.display = Some(display_path(self.path, self.root, self.cwd));
        }
        self.display.as_deref().unwrap_or_default()
    }

    fn push(&mut self, sep: char, line_number: u64, bytes: &[u8]) -> bool {
        if self.remaining == 0 {
            self.hit_limit = true;
            return false;
        }
        let bytes = String::from_utf8_lossy(bytes);
        let text = bytes.trim_end_matches(['\n', '\r']);
        let text = truncate_chars(text, MAX_CHARS_PER_LINE);
        if self.display.is_some() {
            let display = self.display.as_deref().unwrap_or_default();
            self.hits
                .lines
                .push(format!("{display}{sep}{line_number}{sep}{text}"));
        } else {
            let display = display_path(self.path, self.root, self.cwd);
            let line = format!("{display}{sep}{line_number}{sep}{text}");
            self.display = Some(display);
            self.hits.lines.push(line);
        }
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
fn display_path(path: &Path, root: &Path, cwd: &Path) -> String {
    path.strip_prefix(cwd)
        .or_else(|_| path.strip_prefix(root))
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `cwd` 是调用方会话的工作目录，见 [`Input::cwd`]。
pub async fn run(call_id: &str, args: &str, cwd: &Path) -> String {
    let input = match parse(args, cwd) {
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
    /// 因为超过 [`MAX_FILE_BYTES`] 被整颗跳过的文件数。
    skipped_large: usize,
}

/// 每 worker 一份 `Searcher`：它不是 `Sync`，也不该跨线程共享。
fn build_searcher(input: &Input) -> Searcher {
    SearcherBuilder::new()
        .line_number(true)
        .multi_line(input.multiline)
        // NUL 字节即判定为二进制并停搜：不这么做会把 .o / .png 的字节当文本吐出来。
        .binary_detection(BinaryDetection::quit(0))
        .before_context(input.before)
        .after_context(input.after)
        .build()
}

/// 待搜的一颗文件。显示名不在这里——它跟着命中一起算（见 [`Collector::display`]）。
struct FileEntry {
    path: PathBuf,
}

/// 一颗文件的扫描结果。
enum FileScan {
    /// 没命中，或读不动（权限、坏软链）——一颗文件不该废掉整次搜索。
    NoMatch,
    /// 超过大小闸，整颗跳过（数量要回报给模型）。
    SkippedLarge,
    Hits {
        display: String,
        hits: FileHits,
        /// 这颗文件的收集因为额度用尽提前停了。
        hit_limit: bool,
    },
}

/// 一次扫描需要的前后文（root / cwd / 大小闸），跨 worker 共享。
struct ScanCtx<'a> {
    input: &'a Input,
    root: &'a Path,
    cwd: &'a Path,
    size_guard: bool,
}

/// 扫一颗文件。
fn scan_file(
    entry: &FileEntry,
    ctx: &ScanCtx<'_>,
    matcher: &RegexMatcher,
    searcher: &mut Searcher,
    per_file_cap: usize,
) -> FileScan {
    // 大小闸在这里（而不是遍历时）查：`stat` 于是跟着扫描一起摊到多核上，
    // 而且没命中的文件本来就要打开读，它是同一份元数据的顺路开销。
    if ctx.size_guard
        && std::fs::metadata(&entry.path).map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES
    {
        return FileScan::SkippedLarge;
    }
    let mut collector = Collector {
        path: &entry.path,
        root: ctx.root,
        cwd: ctx.cwd,
        display: None,
        hits: FileHits::default(),
        remaining: per_file_cap,
        hit_limit: false,
        collect_lines: ctx.input.mode == OutputMode::Content,
        stop_at_first: ctx.input.mode == OutputMode::FilesWithMatches,
    };
    if searcher
        .search_path(matcher, &entry.path, &mut collector)
        .is_err()
    {
        return FileScan::NoMatch;
    }
    if collector.hits.matches == 0 {
        return FileScan::NoMatch;
    }
    FileScan::Hits {
        display: collector.display().to_string(),
        hits: collector.hits,
        hit_limit: collector.hit_limit,
    }
}

/// 一批文件的扫描结果，按 batch 顺序对齐。
struct BatchScan {
    scans: Vec<FileScan>,
    /// 这一批里有文件因为墙钟到点没扫。
    timed_out: bool,
    /// 有 worker 崩了（内部 bug）：那一小段文件没扫到，必须说出来而不是静悄悄
    /// 给出不完整的结果。
    panicked: bool,
}

/// 并发扫一批文件，**按 batch 顺序**交回结果。
///
/// 顺序必须确定：结果要可复现，`head_limit` 的「取前 N 条」也只有顺序固定时
/// 才有意义。所以这里是「先并发算、再按序合并」，不是谁先算完谁先出。
fn scan_batch(
    batch: &[FileEntry],
    ctx: &ScanCtx<'_>,
    matcher: &RegexMatcher,
    per_file_cap: usize,
    deadline: Instant,
    workers: usize,
) -> BatchScan {
    let workers = if batch.len() < PARALLEL_MIN_BATCH {
        1
    } else {
        workers.min(batch.len()).max(1)
    };

    // 单 worker 走顺序路径：小批不该为几十 µs 的线程启动付账，结果完全一样。
    if workers == 1 {
        let mut searcher = build_searcher(ctx.input);
        let mut scans = Vec::with_capacity(batch.len());
        let mut timed_out = false;
        for entry in batch {
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            scans.push(scan_file(entry, ctx, matcher, &mut searcher, per_file_cap));
        }
        scans.resize_with(batch.len(), || FileScan::NoMatch);
        return BatchScan {
            scans,
            timed_out,
            panicked: false,
        };
    }

    struct Slice {
        start: usize,
        scans: Vec<FileScan>,
        timed_out: bool,
    }
    let per_slice = batch.len().div_ceil(workers);
    let slices: Vec<std::thread::Result<Slice>> = std::thread::scope(|scope| {
        let handles: Vec<_> = batch
            .chunks(per_slice)
            .enumerate()
            .map(|(i, slice)| {
                let start = i * per_slice;
                scope.spawn(move || {
                    let mut searcher = build_searcher(ctx.input);
                    let mut scans = Vec::with_capacity(slice.len());
                    let mut timed_out = false;
                    for entry in slice {
                        if Instant::now() >= deadline {
                            timed_out = true;
                            break;
                        }
                        scans.push(scan_file(entry, ctx, matcher, &mut searcher, per_file_cap));
                    }
                    scans.resize_with(slice.len(), || FileScan::NoMatch);
                    Slice {
                        start,
                        scans,
                        timed_out,
                    }
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join()).collect()
    });

    let mut scans: Vec<FileScan> = Vec::with_capacity(batch.len());
    scans.resize_with(batch.len(), || FileScan::NoMatch);
    let mut timed_out = false;
    let mut panicked = false;
    for slice in slices {
        // worker 崩了：它领的那一段没扫到。不能假装扫过——把那一段留空并上报。
        let Ok(mut slice) = slice else {
            panicked = true;
            continue;
        };
        timed_out |= slice.timed_out;
        for (offset, scan) in slice.scans.drain(..).enumerate() {
            if let Some(slot) = scans.get_mut(slice.start + offset) {
                *slot = scan;
            }
        }
    }
    BatchScan {
        scans,
        timed_out,
        panicked,
    }
}

/// 跨批累计的结果与额度状态。
struct SearchAcc {
    mode: OutputMode,
    head_limit: usize,
    all_lines: Vec<String>,
    entries: Vec<String>,
    remaining: usize,
    truncated: bool,
    timed_out: bool,
    /// 命中的文件数（写进结果的那些）。
    total_seen: usize,
    /// 因为超过 [`MAX_FILE_BYTES`] 被整颗跳过的文件数。
    skipped_large: usize,
    /// 有扫描 worker 崩过：结果可能缺一段。
    panicked: bool,
}

impl SearchAcc {
    fn new(input: &Input) -> Self {
        Self {
            mode: input.mode,
            head_limit: input.head_limit,
            all_lines: Vec::new(),
            entries: Vec::new(),
            remaining: input.head_limit,
            truncated: false,
            timed_out: false,
            total_seen: 0,
            skipped_large: 0,
            panicked: false,
        }
    }

    /// 这颗文件**最多**还能收多少行；非 `content` 模式没有单文件上限
    /// （`count` 要真实计数）。
    ///
    /// 这是批开头取的一个上界，批内每颗文件拿到的是同一个值——所以它只挡得住
    /// 单颗文件超额，挡不住批内多颗叠加超额。整趟额度由 [`SearchAcc::absorb`]
    /// 按顺序合并时守。
    fn per_file_cap(&self) -> usize {
        if self.mode == OutputMode::Content {
            self.remaining
        } else {
            usize::MAX
        }
    }

    /// 按顺序并入一批结果。返回 `true` = 额度已填满，后面不必再搜。
    ///
    /// 额度满与墙钟到点在顺序实现里是互斥的（额度一满就 break，轮不到超时），
    /// 这里保持同一约定：凡是「填满」返回的那条路径都不记 `timed_out`。
    fn absorb(&mut self, scanned: BatchScan) -> bool {
        let timed_out = scanned.timed_out;
        self.panicked |= scanned.panicked;
        // 先数跳过的大文件：额度填满会提前 return，不能让剩下的跳过数漏报。
        for scan in scanned.scans.iter() {
            if matches!(scan, FileScan::SkippedLarge) {
                self.skipped_large += 1;
            }
        }
        for scan in scanned.scans {
            let FileScan::Hits {
                display,
                hits,
                hit_limit,
            } = scan
            else {
                continue;
            };
            self.total_seen += 1;
            match self.mode {
                OutputMode::Content => {
                    // 额度在这里夹住，不能只靠单文件的 `per_file_cap`：那个值是
                    // 批开头取的，批内每颗文件都按它收，叠起来会超过 head_limit
                    // （`head_limit_holds_across_multiple_files`）。
                    let want = hits.lines.len();
                    let take = want.min(self.remaining);
                    let overflowed = take < want;
                    self.all_lines.extend(hits.lines.into_iter().take(take));
                    self.remaining -= take;
                    if hit_limit || overflowed || self.remaining == 0 {
                        self.truncated = true;
                        return true;
                    }
                }
                OutputMode::FilesWithMatches => {
                    self.entries.push(display);
                    if self.entries.len() >= self.head_limit {
                        self.truncated = true;
                        return true;
                    }
                }
                OutputMode::Count => {
                    self.entries.push(format!("{display}:{}", hits.matches));
                    if self.entries.len() >= self.head_limit {
                        self.truncated = true;
                        return true;
                    }
                }
            }
        }
        self.timed_out |= timed_out;
        false
    }
}

fn search(input: &Input) -> Result<Outcome, String> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(input.case_insensitive)
        .multi_line(input.multiline)
        .dot_matches_new_line(input.multiline)
        .build(&input.pattern)
        .map_err(|e| format!("Error: invalid regex `{}`: {e}", input.pattern))?;

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

    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let max_batch = FILES_PER_WORKER.saturating_mul(workers).max(1);
    let mut batch_size = INITIAL_BATCH.min(max_batch);
    // 显式点名一颗文件时不设大小闸：那是模型明说要搜它，跳过等于答非所问。
    let size_guard = !input.path.is_file();
    let ctx = ScanCtx {
        input,
        root: &input.path,
        cwd: &input.cwd,
        size_guard,
    };
    let deadline = Instant::now() + WALL_CLOCK;
    let mut acc = SearchAcc::new(input);
    let mut batch: Vec<FileEntry> = Vec::with_capacity(batch_size);

    for dirent in walk.build() {
        if Instant::now() >= deadline {
            acc.timed_out = true;
            break;
        }
        let Ok(dirent) = dirent else { continue };
        if !dirent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        batch.push(FileEntry {
            path: dirent.path().to_path_buf(),
        });
        if batch.len() < batch_size {
            continue;
        }
        let scanned = scan_batch(
            &batch,
            &ctx,
            &matcher,
            acc.per_file_cap(),
            deadline,
            workers,
        );
        let full = acc.absorb(scanned);
        batch.clear();
        if full || acc.timed_out {
            break;
        }
        // 这一批没把额度填满，说明这趟大概是全扫：放大下一批，摊掉合并与线程启动。
        batch_size = batch_size.saturating_mul(BATCH_GROWTH).min(max_batch);
    }
    if !acc.truncated && !acc.timed_out && !batch.is_empty() {
        let scanned = scan_batch(
            &batch,
            &ctx,
            &matcher,
            acc.per_file_cap(),
            deadline,
            workers,
        );
        acc.absorb(scanned);
    }

    let scope = describe_scope(
        input,
        &type_note,
        acc.timed_out,
        acc.skipped_large,
        acc.panicked,
    );
    let (rendered, shown, unit, next_hint) = match input.mode {
        OutputMode::Content => (
            acc.all_lines.join("\n"),
            acc.all_lines.len(),
            "matching lines",
            " Narrow the pattern, add a glob/type filter, or raise head_limit.",
        ),
        _ => (
            acc.entries.join("\n"),
            acc.entries.len(),
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
            // scope 已经写进 body，别让 render 再追加一遍。
            scope: String::new(),
            skipped_large: acc.skipped_large,
        });
    }

    // 额度填满就停了走查，所以总量是「至少」而不是精确值——不能谎报精确数。
    // worker 崩过同理：缺一段就不是精确总数。
    let counts = if acc.truncated || acc.timed_out || acc.panicked {
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
        scope: if acc.total_seen > 0 {
            scope
        } else {
            String::new()
        },
        skipped_large: acc.skipped_large,
    })
}

/// 「搜了什么范围」。空结果时这一句是模型判断「没有」还是「搜错地方」的唯一依据。
fn describe_scope(
    input: &Input,
    type_note: &str,
    timed_out: bool,
    skipped_large: usize,
    panicked: bool,
) -> String {
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
    if skipped_large > 0 {
        s.push_str(&format!(
            " NOTE: skipped {skipped_large} file(s) larger than {} MB.",
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    if timed_out {
        s.push_str(&format!(
            " NOTE: the search hit its {}s wall-clock budget and stopped; the above is only what \
             it had scanned by then.",
            WALL_CLOCK.as_secs()
        ));
    }
    if panicked {
        s.push_str(
            " NOTE: a scan worker failed; one slice of files was not searched, so the above may \
             be incomplete. Re-run the search.",
        );
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
        skipped_large,
    } = outcome;
    let spill = match full {
        Some(full) => tool_output::offload(call_id, &full).await,
        None => None,
    };
    let footer = tool_output::footer(&counts, unit, next_hint, spill.as_deref());
    let footer_empty = footer.is_empty();
    let mut out = body;
    if !footer_empty {
        out.push_str(&footer);
    }
    // 跳过了超大文件这件事必须说出口：结果非空时模型尤其容易把它当成「搜全了」。
    if !scope.is_empty() && (!footer_empty || skipped_large > 0) {
        out.push_str(&format!("\n{scope}"));
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
        run("t", &args.to_string(), &std::env::current_dir().unwrap()).await
    }

    /// 相对 `path` 按调用方给的会话 cwd 展开，不按进程 cwd：同一进程里两页
    /// 可能在不同项目，按进程 cwd 会搜错仓库。
    #[tokio::test]
    async fn relative_path_resolves_against_the_given_cwd() {
        let dir = fixture();
        let out = run(
            "t",
            r#"{"pattern":"beta","path":"src","output_mode":"files_with_matches"}"#,
            dir.path(),
        )
        .await;
        assert!(
            out.contains("src/a.rs"),
            "应在会话 cwd 下的 src/ 里找到：{out}"
        );
        // 按进程 cwd 展开的话搜的是 cordis-base/src，本文件自己就含 "beta"。
        assert!(!out.contains("grep.rs"), "搜到了进程 cwd 下的仓库：{out}");
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

    /// 并发扫描必须与顺序扫描给出**逐字相同**的结果顺序：并发改的是速度，
    /// 不是「前 N 条」是哪 N 条。
    #[test]
    fn parallel_scan_matches_sequential_scan() {
        let dir = fixture();
        // 多到跨好几个 slice，且文件名顺序与创建顺序不同，专门抓「谁先算完谁先出」。
        let mut paths: Vec<PathBuf> = Vec::new();
        for i in (0..14).rev() {
            let p = dir.path().join(format!("src/f{i:02}.rs"));
            std::fs::write(&p, format!("fn alpha_{i}() {{}}\n")).unwrap();
            paths.push(p);
        }
        paths.sort();
        let input = parse(
            r#"{"pattern":"alpha","path":".","head_limit":100}"#,
            &std::env::current_dir().unwrap(),
        )
        .unwrap();
        let matcher = RegexMatcherBuilder::new()
            .build("alpha")
            .expect("valid regex");
        let batch: Vec<FileEntry> = paths
            .iter()
            .map(|p| FileEntry { path: p.clone() })
            .collect();
        let root = dir.path().to_path_buf();
        let ctx = ScanCtx {
            input: &input,
            root: &root,
            cwd: &input.cwd,
            size_guard: true,
        };

        let deadline = Instant::now() + WALL_CLOCK;
        let seq = scan_batch(&batch, &ctx, &matcher, 100, deadline, 1);
        let par = scan_batch(&batch, &ctx, &matcher, 100, deadline, 8);

        assert_eq!(seq.scans.len(), batch.len());
        assert_eq!(par.scans.len(), batch.len());
        for (i, (a, b)) in seq.scans.iter().zip(par.scans.iter()).enumerate() {
            match (a, b) {
                (FileScan::Hits { hits: ha, .. }, FileScan::Hits { hits: hb, .. }) => {
                    assert_eq!(ha.lines, hb.lines, "第 {i} 颗结果不一致");
                    assert_eq!(ha.matches, hb.matches, "第 {i} 颗命中数不一致");
                }
                (FileScan::NoMatch, FileScan::NoMatch) => {}
                _ => panic!("第 {i} 颗：两边结论不一致"),
            }
        }
        // 显示名只能算一次，且必须相对 cwd——它是并行路径上唯一带状态的字段。
        let hit_display = seq
            .scans
            .iter()
            .find_map(|s| match s {
                FileScan::Hits { display, .. } => Some(display.clone()),
                _ => None,
            })
            .expect("至少一颗命中");
        assert!(hit_display.starts_with("src/"), "{hit_display}");
    }

    /// 只并行扫描、不搬走串行开销的话只有 1.03–1.20x（见 docs/tools/workspace.md）：
    /// 这条守住的是「worker 崩了不能静悄悄给不完整结果」。
    #[test]
    fn a_failed_worker_marks_the_result_incomplete() {
        let input = parse(
            r#"{"pattern":"x","path":"."}"#,
            &std::env::current_dir().unwrap(),
        )
        .unwrap();
        let mut acc = SearchAcc::new(&input);
        acc.absorb(BatchScan {
            scans: vec![FileScan::NoMatch],
            timed_out: false,
            panicked: true,
        });
        assert!(acc.panicked, "worker 崩过要记住");
        let scope = describe_scope(&input, "", acc.timed_out, acc.skipped_large, acc.panicked);
        assert!(scope.contains("scan worker failed"), "{scope}");
        assert!(scope.contains("Re-run"), "要说清怎么办：{scope}");
    }

    /// 超过 5MB 的文件在目录遍历里整颗跳过，但必须报数——不说，模型会把结果
    /// 当成「搜全了」。
    #[tokio::test]
    async fn oversized_file_is_skipped_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("small.txt"), "needle here\n").unwrap();
        let big = dir.path().join("big.log");
        std::fs::write(
            &big,
            format!("needle {}\n", "x".repeat(MAX_FILE_BYTES as usize + 16)),
        )
        .unwrap();

        let out = run(
            "t",
            &serde_json::json!({"pattern": "needle", "path": dir.path()}).to_string(),
            &std::env::current_dir().unwrap(),
        )
        .await;
        assert!(out.contains("small.txt:1:needle here"), "{out}");
        assert!(!out.contains("big.log"), "超大文件不该被展开：{out}");
        assert!(
            out.contains("skipped 1 file(s) larger than 5 MB"),
            "跳过了几颗必须回报：{out}"
        );

        // 只有那颗大文件命中时，也要说清楚「没有」是因为跳过了它。
        let only_big = run(
            "t",
            &serde_json::json!({"pattern": "xxxxxx", "path": dir.path()}).to_string(),
            &std::env::current_dir().unwrap(),
        )
        .await;
        assert!(only_big.contains("no matches"), "{only_big}");
        assert!(only_big.contains("skipped 1 file(s)"), "{only_big}");
    }

    /// 显式点名一颗文件时不吃大小闸：跳过等于答非所问。
    #[tokio::test]
    async fn explicit_single_file_target_is_never_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.log");
        std::fs::write(
            &big,
            format!("needle {}\n", "x".repeat(MAX_FILE_BYTES as usize + 16)),
        )
        .unwrap();
        let out = run(
            "t",
            &serde_json::json!({"pattern": "needle", "path": big}).to_string(),
            &std::env::current_dir().unwrap(),
        )
        .await;
        assert!(out.contains("needle"), "{out}");
        assert!(!out.contains("skipped"), "{out}");
    }

    /// `head_limit` 是**整趟**的额度，不是每颗文件的。
    ///
    /// 按批扫描把单文件额度冻结在批开头，批内每颗命中文件都按同一个 `remaining`
    /// 收行，叠起来就冲破了额度：head_limit=200 曾经实收 300 行，页脚却还在说
    /// 「showing 300 ... at least 300 more」。单文件的用例盖不到这条，因为那时
    /// `Collector` 自己的计数还管用。
    #[tokio::test]
    async fn head_limit_holds_across_multiple_files() {
        let _env = crate::test_env::scoped().home();
        let dir = tempfile::tempdir().unwrap();
        // 每颗都不足额度、两颗就超——专抓「跨文件叠加」而不是「单文件超额」。
        for f in 0..10 {
            let body: String = (1..=150).map(|i| format!("hit {f} line {i}\n")).collect();
            std::fs::write(dir.path().join(format!("f{f:02}.txt")), body).unwrap();
        }
        let out = run(
            "multi-file-limit",
            &serde_json::json!({"pattern": "hit", "path": dir.path(), "head_limit": 200})
                .to_string(),
            &std::env::current_dir().unwrap(),
        )
        .await;
        let inline = out.lines().filter(|l| l.contains(":hit ")).count();
        assert_eq!(inline, 200, "内联行数必须正好是 head_limit：\n{out}");
        assert!(out.contains("at least"), "超额的部分要说清楚：{out}");
    }

    /// 批大小是自适应的（先小后大），所以结果必然跨批合并——合并顺序必须仍是
    /// 路径顺序，否则 `head_limit` 的「前 N 条」是哪 N 条就不确定了。
    #[tokio::test]
    async fn results_stay_path_ordered_across_batches() {
        let dir = tempfile::tempdir().unwrap();
        // 比 INITIAL_BATCH 多得多，保证至少跨两批。
        for f in 0..(INITIAL_BATCH * 4) {
            std::fs::write(dir.path().join(format!("f{f:03}.txt")), "needle\n").unwrap();
        }
        let out = run(
            "ordered",
            &serde_json::json!({
                "pattern": "needle",
                "path": dir.path(),
                "output_mode": "files_with_matches",
            })
            .to_string(),
            &std::env::current_dir().unwrap(),
        )
        .await;
        let files: Vec<&str> = out.lines().filter(|l| l.ends_with(".txt")).collect();
        assert_eq!(files.len(), INITIAL_BATCH * 4, "一颗都不能丢：{out}");
        let mut sorted = files.clone();
        sorted.sort_unstable();
        assert_eq!(files, sorted, "跨批合并必须保持路径顺序");
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
            &std::env::current_dir().unwrap(),
        )
        .await;
        assert_eq!(out.matches("big.txt:").count(), 5, "只内联 5 行：{out}");
        assert!(out.contains("at least"), "要说是至少而不是精确总数：{out}");
        let spill =
            tool_output::spill_dir().join(format!("{}.txt", tool_output::offload_stem("spill-1")));
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
            &std::env::current_dir().unwrap(),
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
            &std::env::current_dir().unwrap(),
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
        let input = parse(
            r#"{"pattern":"x","head_limit":999999}"#,
            &std::env::current_dir().unwrap(),
        )
        .unwrap();
        assert_eq!(input.head_limit, CONTENT_LINE_LIMIT);
        let files = parse(
            r#"{"pattern":"x","output_mode":"count","head_limit":999999}"#,
            &std::env::current_dir().unwrap(),
        )
        .unwrap();
        assert_eq!(files.head_limit, ENTRY_LIMIT);
    }

    /// `-C` 要同时盖住 `-A` / `-B`（对齐 rg）。
    #[test]
    fn context_flag_overrides_before_and_after() {
        let input = parse(
            r#"{"pattern":"x","-A":1,"-B":1,"-C":5}"#,
            &std::env::current_dir().unwrap(),
        )
        .unwrap();
        assert_eq!((input.before, input.after), (5, 5));
    }
}
