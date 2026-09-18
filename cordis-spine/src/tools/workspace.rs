//! Workspace tools with Grok JSON names. Copied field names from
//! `xai-grok-tools` `list_dir` / `read_file` / `grep` / `search_replace` / `bash`.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::tools::jobs::Jobs;
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

const LIST_DIR_PARAMS: &str = r#"{"type":"object","properties":{"target_directory":{"type":"string","description":"Path to directory to list, relative to cwd or absolute."}},"required":["target_directory"]}"#;
const READ_FILE_PARAMS: &str = r#"{"type":"object","properties":{"target_file":{"type":"string","description":"Path of the file to read (relative to cwd or absolute)."},"offset":{"type":"integer","description":"1-based start line. Omit to start at line 1. Use with limit for large files."},"limit":{"type":"integer","description":"Max lines to return. Omit to use the default cap (1000). Pass a smaller value for a tight window."}},"required":["target_file"]}"#;
const SEARCH_REPLACE_PARAMS: &str = r#"{"type":"object","properties":{"file_path":{"type":"string","description":"Path of the file to modify, relative to cwd or absolute."},"old_string":{"type":"string","description":"Exact text to replace. Must match exactly one place in the file unless replace_all is true. Set to an empty string to create a new file."},"new_string":{"type":"string","description":"Replacement text. Must differ from old_string."},"replace_all":{"type":"boolean","description":"Replace every occurrence instead of requiring old_string to be unique. Use when renaming an identifier."}},"required":["file_path","old_string","new_string"]}"#;
const BASH_PARAMS: &str = r#"{"type":"object","properties":{"command":{"type":"string","description":"The bash command to run."},"description":{"type":"string","description":"Clear, concise description of what this command does in active voice, 5-10 words (shown to the user in the permission prompt and the UI). Examples: \"git status\" -> \"Show working tree status\"; \"npm install\" -> \"Install package dependencies\"."},"workdir":{"type":"string","description":"Working directory for this command. Defaults to the session cwd; if you pass a relative workdir it resolves against the session cwd. Prefer this over a leading cd. IMPORTANT: relative paths inside command then resolve against workdir, not against the session cwd — with workdir \"sub\", write \"src/file.txt\", not \"sub/src/file.txt\"."},"timeout_ms":{"type":"integer","description":"How long to wait in the foreground, in ms. Defaults to 300000 (5 min) and is capped at it. On expiry the command is NOT killed: it moves to the background and you get a task id plus whatever it printed so far."},"is_background":{"type":"boolean","description":"Set to true for long-running commands (dev servers, long builds). Returns a task id immediately; collect with get_task_output, stop with kill_task."},"block_until_ms":{"type":"integer","description":"Foreground wait in ms. 0 backgrounds immediately."}},"required":["command"]}"#;
const GLOB_PARAMS: &str = r#"{"type":"object","properties":{"glob_pattern":{"type":"string","description":"Glob to match file paths against, e.g. \"**/*.rs\" or \"src/**/test_*.py\". * does not cross directory separators; use ** to span directories. A pattern with no / matches the basename at any depth."},"target_directory":{"type":"string","description":"Directory to search in. Defaults to the current working directory."}},"required":["glob_pattern"]}"#;
const WRITE_FILE_PARAMS: &str = r#"{"type":"object","properties":{"target_file":{"type":"string","description":"Path to write, relative to cwd or absolute. Parent directories are created."},"contents":{"type":"string","description":"Full text content to write. Existing files are overwritten in full."}},"required":["target_file","contents"]}"#;

/// Default max lines when the model omits `limit` (grok `MAX_LINES_READ`).
const MAX_LINES_READ: usize = 1_000;

/// 前台 bash 的阻塞预算。到点把命令**转入后台**并带回已产出的输出
/// （见 [`detach_to_background`]），不是杀掉。
///
/// Grok 这里是 30s（`block_until_ms` 的省略默认值），用意是催模型把长命令交后台。
/// 在本仓库太短：一条 `cargo clippy -p cordis-spine` 就 1 分多钟，`cargo test` 全量
/// 更久，30s 到点必然被收掉——模型只能反复「起后台 + `get_task_output` 轮询」，多花
/// 的回合比省下的等待贵。放宽到 5 分钟：仍有上限（这一轮挂不死）。覆盖用的 env
/// 对齐 Grok 的 `GROK_MAX_FOREGROUND_BLOCK_MS`。
const FOREGROUND_MS_ENV: &str = "DOCK_BASH_FOREGROUND_MS";
const DEFAULT_FOREGROUND_MS: u64 = 300_000;

fn foreground_budget() -> Duration {
    std::env::var(FOREGROUND_MS_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_millis(DEFAULT_FOREGROUND_MS))
}

/// 本次调用的前台预算：模型传的 `timeout_ms` 优先，但**只能收紧不能放宽**。
///
/// 放宽会让一条命令把整轮挂住；收紧是有用的——模型知道 `cargo check` 该 30s
/// 内回来，超了就是卡住了，早点拿到部分输出比干等五分钟强。
fn call_budget(v: &Value) -> Duration {
    let ceiling = foreground_budget();
    match int_field(v, "timeout_ms").filter(|ms| *ms > 0) {
        Some(ms) => Duration::from_millis(ms as u64).min(ceiling),
        None => ceiling,
    }
}

/// 解析 `workdir`：相对路径按 cwd 展开；必须是已存在的目录。
///
/// 不存在就直接报错，而不是让 bash 在一个意外的目录里把命令跑掉——后者会产生
/// 「命令看起来成功了但作用在错误的地方」这种最难查的失败。
fn resolve_workdir(v: &Value) -> Result<Option<PathBuf>, String> {
    let Some(raw) = str_field(v, &["workdir", "cwd"]) else {
        return Ok(None);
    };
    let path = resolve(&raw);
    if !path.is_dir() {
        return Err(format!(
            "Error: workdir {} 不是一个已存在的目录",
            path.display()
        ));
    }
    Ok(Some(path))
}

const READ_FILE_DESC: &str = "Read a file.\n\
- Use this instead of `cat` / `head` / `sed -n` through bash: it is gated as read-only, works in plan mode, and tells you how much of the file you have not seen.\n\
- By default reads up to 1000 lines from offset (default line 1).\n\
- For large files, pass offset + limit to page through; the result notes how many lines remain.\n\
- Line anchors appear as N→ on line 1 and every 10th line. That prefix is not part of the file — when passing text to search_replace, match only what comes after the →.\n\
- Image files (png/jpg/jpeg/webp/gif) come back as pixels, not text.";

const LIST_DIR_DESC: &str = "List the contents of a directory.\n\
- Use this instead of `ls` through bash: it is gated as read-only and works in plan mode.\n\
- Respects .gitignore, so build output and vendored dependencies do not drown the result. Dot-files are hidden.\n\
- One level only — it does not recurse. Use glob to find files by path pattern, or grep to find them by content.\n\
- Large directories are summarized with a file count and an extension breakdown instead of listing every entry.";

const GLOB_DESC: &str = "Find files whose path matches a glob pattern.\n\
- Use this instead of `find` or `ls **` through bash: it is gated as read-only and works in plan mode.\n\
- Returns files only, never directories, newest first (modification time), so the head of the result is the code most recently worked on.\n\
- `*` does not cross `/`; use `**` to span directories. A pattern with no `/` matches the basename at any depth, so `*.rs` searches the whole tree.\n\
- Respects .gitignore and skips dot-files. Capped results say how many paths were not shown.\n\
- This finds files by *name*. To find them by *content*, use grep.";

const SEARCH_REPLACE_DESC: &str = "Replace an exact string in a file.\n\
- read_file prefixes each line with \"N→\". That prefix is not part of the file: match only what comes after the →, with its exact indentation.\n\
- old_string must match exactly one place in the file. If it appears more than once the call fails and reports the count — add surrounding lines to make it unique, or set replace_all to change every occurrence (handy for renaming an identifier).\n\
- To create a new file, set old_string to an empty string. An empty old_string cannot overwrite an existing non-empty file.\n\
- Prefer this over write_file for targeted edits: write_file replaces the whole file.";

const WRITE_FILE_DESC: &str = "Write contents to a file, creating it or replacing it in full.\n\
- Existing files are overwritten entirely, so read the file first unless you just created it. For a targeted change use search_replace instead.\n\
- Parent directories are created automatically.";

const BASH_DESC: &str = "Run a bash command in the workspace and return its output.\n\
- Prefer the dedicated tools when one fits: read_file over `cat`, grep over `grep`/`rg`, glob over `find`, list_dir over `ls`, search_replace over `sed -i`. They are cheaper, are not gated behind a permission prompt, keep working in plan mode, and report what they truncated.\n\
- Each call runs in a fresh shell: cwd, variables and functions do not persist between calls. Pass workdir instead of using `cd`. Once you pass workdir, every relative path in the command is relative to it — do not also prefix those paths with the directory you just moved into.\n\
- A foreground command that outlives timeout_ms (default and cap 300000 ms) is not killed: it moves to the background and you get a task id plus the output so far. Never re-run it — collect with get_task_output.\n\
- Set is_background true (or block_until_ms: 0) for dev servers and long builds: you get a task id immediately and check it with get_task_output / kill_task.\n\
- Output is capped; the head and tail are kept and the middle is reported as elided.";

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "list_dir".into(),
            description: LIST_DIR_DESC.into(),
            parameters_json: LIST_DIR_PARAMS.into(),
        },
        ToolSpec {
            name: "read_file".into(),
            description: READ_FILE_DESC.into(),
            parameters_json: READ_FILE_PARAMS.into(),
        },
        ToolSpec {
            name: "grep".into(),
            description: cordis_base::grep::DESCRIPTION.into(),
            parameters_json: cordis_base::grep::PARAMS.into(),
        },
        ToolSpec {
            name: "search_replace".into(),
            description: SEARCH_REPLACE_DESC.into(),
            parameters_json: SEARCH_REPLACE_PARAMS.into(),
        },
        ToolSpec {
            name: "bash".into(),
            description: BASH_DESC.into(),
            parameters_json: BASH_PARAMS.into(),
        },
        ToolSpec {
            name: "glob".into(),
            description: GLOB_DESC.into(),
            parameters_json: GLOB_PARAMS.into(),
        },
        ToolSpec {
            name: "write_file".into(),
            description: WRITE_FILE_DESC.into(),
            parameters_json: WRITE_FILE_PARAMS.into(),
        },
    ]
}

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "list_dir"
            | "read_file"
            | "grep"
            | "search_replace"
            | "bash"
            | "run_terminal_cmd"
            | "glob"
            | "write_file"
    )
}

#[allow(dead_code)]
pub async fn execute(call: ToolCall) -> ToolResult {
    execute_with(call, || false, None).await
}

pub async fn execute_with(
    call: ToolCall,
    is_cancelled: impl Fn() -> bool + Send + Sync,
    jobs: Option<&Jobs>,
) -> ToolResult {
    if call.name == "read_file" {
        if let Some((content, images)) = read_file_maybe_image(&call.arguments) {
            return ToolResult {
                call_id: call.id,
                name: call.name,
                content,
                images: crate::tools::tool_images::cap_images(images),
            };
        }
    }
    let content = match call.name.as_str() {
        "list_dir" => list_dir(&call.arguments),
        "read_file" => read_file(&call.arguments),
        "grep" => cordis_base::grep::run(&call.id, &call.arguments).await,
        "search_replace" => search_replace(&call.arguments),
        "bash" | "run_terminal_cmd" => bash(&call.arguments, &is_cancelled, jobs).await,
        "glob" => glob_files(&call.arguments),
        "write_file" => write_file(&call.arguments),
        other => format!("unknown tool: {other}"),
    };
    ToolResult {
        call_id: call.id,
        name: call.name,
        content,
        ..Default::default()
    }
}

fn parse_args(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or(Value::Null)
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

fn int_field(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_u64().map(|n| n as i64))
            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
    })
}

fn bool_field(v: &Value, key: &str) -> bool {
    v.get(key)
        .and_then(|x| {
            x.as_bool()
                .or_else(|| x.as_str().map(|s| s.eq_ignore_ascii_case("true")))
        })
        .unwrap_or(false)
}

fn resolve(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    }
}

/// 超过这个条目数就不再逐条列，改成计数 + 扩展名分布。
const LIST_DIR_SUMMARY_THRESHOLD: usize = 100;

fn list_dir(args: &str) -> String {
    let v = parse_args(args);
    let target = str_field(&v, &["target_directory", "path"]).unwrap_or_else(|| ".".into());
    let path = resolve(&target);
    if !path.is_dir() {
        return format!("Error: {} is not a valid directory", path.display());
    }
    // 只列一层，但过滤交给 `ignore`：`target/`、`node_modules/` 这些被
    // `.gitignore` 排掉的目录不该在结果里，否则一次 `list_dir` 就是几千行噪声。
    // `max_depth(1)` 下 walker 只吐出直接子项。
    let walk = ignore::WalkBuilder::new(&path)
        .max_depth(Some(1))
        .hidden(true)
        // 与 grep 同一条理由：承诺「respects .gitignore」就不该随当前目录
        // 恰好是不是 git 仓库而变。
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();

    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in walk.flatten() {
        // walker 的第一项是根目录自身。
        if entry.path() == path {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(format!("{name}/"));
        } else {
            files.push(name);
        }
    }

    if dirs.is_empty() && files.is_empty() {
        return format!("{} (empty)", path.display());
    }
    let total = dirs.len() + files.len();
    if total > LIST_DIR_SUMMARY_THRESHOLD {
        return format!(
            "{}\n{}",
            path.display(),
            summarize_dir(&dirs, &files, LIST_DIR_SUMMARY_THRESHOLD)
        );
    }
    let mut lines = dirs;
    lines.extend(files);
    format!("{}\n{}", path.display(), lines.join("\n"))
}

/// 大目录的摘要：目录仍逐条列（通常不多且是导航要用的），文件收成计数 +
/// 扩展名分布。对齐 Grok list_dir 的 "summarized with file counts and extension
/// breakdowns instead of listing all files"。
fn summarize_dir(dirs: &[String], files: &[String], sample: usize) -> String {
    use std::collections::BTreeMap;
    let mut out = String::new();
    for d in dirs {
        out.push_str(d);
        out.push('\n');
    }
    let mut by_ext: BTreeMap<&str, usize> = BTreeMap::new();
    for f in files {
        let ext = f.rsplit_once('.').map(|(_, e)| e).unwrap_or("(无扩展名)");
        *by_ext.entry(ext).or_default() += 1;
    }
    // 多的排前面，方便一眼看出这个目录主要是什么。
    let mut counts: Vec<(&str, usize)> = by_ext.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let breakdown = counts
        .iter()
        .map(|(ext, n)| format!("{ext} × {n}"))
        .collect::<Vec<_>>()
        .join("，");
    out.push_str(&format!(
        "\n[目录较大：{} 个子目录、{} 个文件。扩展名分布：{breakdown}]\n",
        dirs.len(),
        files.len()
    ));
    let shown = files.len().min(sample);
    out.push_str(&format!("[前 {shown} 个文件]\n"));
    out.push_str(&files[..shown].join("\n"));
    if files.len() > shown {
        out.push_str(&format!(
            "\n[另有 {} 个文件未列出——用 glob 按模式取，或 grep 按内容找]",
            files.len() - shown
        ));
    }
    out
}

/// When the target is png/jpeg/webp/gif, return inline image + placeholder
/// instead of `read_to_string` (which fails / garbles binaries).
fn read_file_maybe_image(args: &str) -> Option<(String, Vec<cordis_base::types::UserImage>)> {
    let v = parse_args(args);
    let target = str_field(&v, &["target_file", "path"])?;
    let path = resolve(&target);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") {
        return None;
    }
    let img = crate::tools::tool_images::user_image_from_path(&path)?;
    let content = format!(
        "{}
{}",
        path.display(),
        crate::tools::tool_images::IMAGE_INLINE_PLACEHOLDER
    );
    Some((content, vec![img]))
}

fn read_file(args: &str) -> String {
    let v = parse_args(args);
    let Some(target) = str_field(&v, &["target_file", "path"]) else {
        return "Error: target_file is required".into();
    };
    let path = resolve(&target);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return format!("Error reading {}: {e}", path.display()),
    };
    let offset = int_field(&v, "offset").unwrap_or(1).max(1) as usize;
    // Explicit limit wins; otherwise cap at MAX_LINES_READ so omitting limit
    // no longer dumps an entire large file into context.
    let limit = int_field(&v, "limit")
        .map(|n| n.max(1) as usize)
        .unwrap_or(MAX_LINES_READ);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let total = lines.len();
    let start = offset.saturating_sub(1).min(total);
    let end = (start + limit).min(total);
    let slice = &lines[start..end];
    if slice.is_empty() {
        return format!(
            "{}: no lines in range (file has {total} lines)",
            path.display()
        );
    }
    let mut out: String = slice
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let n = start + i + 1;
            if n == 1 || n.is_multiple_of(10) {
                format!("{n}→{line}")
            } else {
                (*line).to_string()
            }
        })
        .collect();
    if end < total {
        let shown = end - start;
        let next = end + 1;
        let remaining = total - end;
        out.push_str(&format!(
            "\n\n[… truncated: showed lines {}-{end} ({shown} of {total}). \
             {remaining} lines remain — call read_file again with offset={next} \
             and a limit, or raise limit.]",
            start + 1
        ));
    }
    out
}

fn search_replace(args: &str) -> String {
    let v = parse_args(args);
    let Some(file_path) = str_field(&v, &["file_path", "path"]) else {
        return "Error: file_path is required".into();
    };
    let Some(old) = str_field(&v, &["old_string"]) else {
        return "Error: old_string is required".into();
    };
    let Some(new) = str_field(&v, &["new_string"]) else {
        return "Error: new_string is required".into();
    };
    let replace_all = bool_field(&v, "replace_all");
    let path = resolve(&file_path);
    if old.is_empty() {
        if path.exists() {
            return format!(
                "Error: {} already exists; empty old_string cannot overwrite",
                path.display()
            );
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        return match std::fs::write(&path, &new) {
            Ok(()) => format!("created {}", path.display()),
            Err(e) => format!("Error writing {}: {e}", path.display()),
        };
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return format!("Error reading {}: {e}", path.display()),
    };
    // 命中数要先数清楚再动手。旧实现在多处命中时 `replacen(..., 1)` 悄悄改掉
    // 第一处——模型以为改的是自己瞄准的那处，实际可能是文件里另一处同名代码。
    // 这是数据损坏级的坑，而且模型被烧一次就再也不信这颗工具、整场会话改用
    // `sed` / `python -c`。Grok 与 DSH 在这里都是**报错**。
    let hits = text.matches(&old).count();
    if hits == 0 {
        return format!("Error: old_string 未在 {} 中找到", path.display());
    }
    if hits > 1 && !replace_all {
        return format!(
            "Error: old_string 在 {} 中出现 {hits} 次，无法确定改哪一处。\
             补上前后文让它唯一，或传 replace_all: true 改掉全部 {hits} 处。",
            path.display()
        );
    }
    let next = if replace_all {
        text.replace(&old, &new)
    } else {
        text.replacen(&old, &new, 1)
    };
    match std::fs::write(&path, next) {
        Ok(()) => {
            if replace_all && hits > 1 {
                format!("updated {}（{hits} 处）", path.display())
            } else {
                format!("updated {}", path.display())
            }
        }
        Err(e) => format!("Error writing {}: {e}", path.display()),
    }
}

async fn bash(
    args: &str,
    is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    jobs: Option<&Jobs>,
) -> String {
    let v = parse_args(args);
    let Some(command) = str_field(&v, &["command"]) else {
        return "Error: command is required".into();
    };
    let description = str_field(&v, &["description"]);
    let workdir = match resolve_workdir(&v) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let background = bool_field(&v, "is_background")
        || bool_field(&v, "background")
        || int_field(&v, "block_until_ms") == Some(0);
    if background {
        if let Some(jobs) = jobs {
            let id = jobs.start_ex_in(command, description, false, workdir);
            // Grok `format_default_prompt` backgrounded + `background_retrieval_hint`.
            return format!(
                "[Command moved to background]\n\n\
                 task_id: {id}\n\n\
                 The command is still running in the background. You can continue with other tasks.\n\
                 Use get_task_output with task_ids=[\"{id}\"] when you need the output."
            );
        }
    }
    // 前台也走 `Jobs`：同一套并发抽干 + 增量累积 + 输出上限，且 TUI 能在命令
    // 还在跑的时候就读到它的输出（tasks pane / scrollback 都读 JobSnapshot）。
    // 没挂 `"jobs"` 服务时（单测、精简装配）临时起一张本地表，行为完全一致，
    // 只是没人能查它 —— 所以后台请求仍然退回前台执行，不发无处可查的 task_id。
    let local;
    // 有没有**别人能查**的任务表，决定了超时能不能转后台：本地表随本次调用一起
    // 析构，往外发它的 task_id 等于发一张空头支票。
    let collectable = jobs.is_some();
    let jobs = match jobs {
        Some(jobs) => jobs,
        None => {
            local = Jobs::new();
            &local
        }
    };
    let id = jobs.start_foreground_ex(&command, description, workdir);
    let budget = call_budget(&v);
    let start = std::time::Instant::now();
    loop {
        // 轮询只读完成位；输出只在真正要返回时取一次，避免每 20ms 白拼一个
        // 最大 20KB 的 String。
        match jobs.is_done(&id) {
            Some(true) => {
                let out = jobs.snapshot(&id).map(|s| s.output).unwrap_or_default();
                jobs.forget(&id);
                return out;
            }
            // 任务凭空消失：只有我们自己会 forget，正常不会走到。
            None => return "(no output)".into(),
            Some(false) => {}
        }
        if is_cancelled() {
            return finish_early(jobs, &id, "cancelled".into()).await;
        }
        if start.elapsed() >= budget {
            if collectable {
                return detach_to_background(jobs, &id, budget);
            }
            // 没有可查的任务表：转后台就成了「还在跑，但你永远拿不到」。退回旧的
            // kill + 说明，宁可诚实地失败。
            return finish_early(
                jobs,
                &id,
                format!(
                    "Error: 命令超时（前台等待 {}）已被终止。\
                     下面是终止前已产出的输出。",
                    human_budget(budget)
                ),
            )
            .await;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 预算的人读形式。秒级预算用 `{:.0}s` 会把 500ms 印成「0s」，那看着像个 bug。
fn human_budget(budget: Duration) -> String {
    if budget < Duration::from_secs(1) {
        format!("{}ms", budget.as_millis())
    } else {
        format!("{:.0}s", budget.as_secs_f64())
    }
}

/// 前台预算到点：**不杀命令**，把它转成后台任务，带回已产出的输出与 task_id。
///
/// 杀掉再让模型重跑是双重浪费：那几分钟的工作扔了，重跑还要再花同样的时间，
/// 而且大概率再超时一次——`cargo build` 不会因为重跑就变快。命令已经过了权限门、
/// 进程还活着，留着它比杀掉严格更优。
///
/// 这里**不是错误**，所以开头不写 `Error:`：模型看到 `Error:` 的第一反应是重试或
/// 换路子，而这次它什么都没做错，只是命令比预算长。
///
/// 取消（用户按 Esc）仍然走 [`finish_early`] 杀掉——那是明确要它停。
fn detach_to_background(jobs: &Jobs, id: &str, budget: Duration) -> String {
    let out = jobs.snapshot(id).map(|s| s.output).unwrap_or_default();
    // 转不动只有一种情况：这一瞬间它自己跑完了。那就当正常完成，别报超时。
    if !jobs.detach(id) {
        jobs.forget(id);
        return out;
    }
    let waited = human_budget(budget);
    let head = format!(
        "[命令仍在运行，已转入后台]（前台等待 {waited} 到点）\n\n\
         task_id: {id}\n\n\
         进程没有被终止，还在继续跑。不要重跑这条命令——用 get_task_output 配 \
         task_ids=[\"{id}\"] 取后续输出，要停就用 kill_task。\n\
         下面是转入后台前已产出的输出。"
    );
    if out.trim().is_empty() || out.trim() == "(no output)" {
        head
    } else {
        format!("{head}\n{out}")
    }
}

/// 取消的收尾：杀掉命令，把已经产出的输出带回来，再把任务摘掉。
///
/// 旧实现在这里直接 `kill` 后返回一句错误字符串，从不读管道，模型拿不到任何
/// 已完成的工作。
async fn finish_early(jobs: &Jobs, id: &str, reason: String) -> String {
    let _ = jobs.kill(id).await;
    // kill 是发信号，run 任务还要收尾；给它一小段时间把尾巴写完。
    for _ in 0..25 {
        if jobs.is_done(id) != Some(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let out = jobs.snapshot(id).map(|s| s.output).unwrap_or_default();
    jobs.forget(id);
    if out.trim().is_empty() || out.trim() == "(no output)" {
        reason
    } else {
        format!("{reason}\n{out}")
    }
}

/// `glob` 一次最多内联多少条路径。
const GLOB_MAX_RESULTS: usize = 200;

fn glob_files(args: &str) -> String {
    let v = parse_args(args);
    let Some(pattern) = str_field(&v, &["glob_pattern", "pattern"]) else {
        return "Error: glob_pattern is required".into();
    };
    let root = str_field(&v, &["target_directory", "path"]).unwrap_or_else(|| ".".into());
    let root = resolve(&root);

    // 旧实现把 glob 手翻成正则（`*` → `.*`），两头都错：
    //   `**/*.rs`  漏掉根目录下的文件（正则强制要有一个 `/`）
    //   `src/*.rs` 又会匹配进 `src/deep/b.rs`（`.*` 跨过了 `/`）
    // globset 是 ripgrep 自己的 glob 实现，`*` 不跨 `/`、`**` 才跨。
    let matcher = match build_glob(&pattern) {
        Ok(m) => m,
        Err(e) => return format!("Error: 无效的 glob `{pattern}`：{e}"),
    };

    let walk = ignore::WalkBuilder::new(&root)
        .hidden(true)
        .require_git(false)
        .build();
    let mut hits: Vec<(std::time::SystemTime, String)> = Vec::new();
    for entry in walk.flatten() {
        // 只要文件。旧实现在判 `is_dir` 之前就把条目推进结果，目录也会混进来。
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        // 无 `/` 的模式匹配任意深度的 basename（对齐 ripgrep / DSH glob）。
        let basename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !(matcher.is_match(rel.as_str())
            || (!pattern.contains('/') && matcher.is_match(basename.as_str())))
        {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified().map_err(Into::into))
            .unwrap_or(std::time::UNIX_EPOCH);
        hits.push((mtime, rel));
    }

    if hits.is_empty() {
        return format!(
            "no matches\n搜索范围：{}，模式：{pattern}，已按 .gitignore 过滤并跳过隐藏文件。\
             提示：`*` 不跨 `/`，跨目录要用 `**`。",
            root.display()
        );
    }
    // 新的在前：最近动过的代码通常就是要找的那批。
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let total = hits.len();
    let shown = total.min(GLOB_MAX_RESULTS);
    let mut out = hits[..shown]
        .iter()
        .map(|(_, rel)| rel.clone())
        .collect::<Vec<_>>()
        .join("\n");
    if total > shown {
        out.push_str(&format!(
            "\n\n[截断：显示 {shown} / 共 {total} 条，按修改时间新→旧。收窄模式取更多。]"
        ));
    }
    out
}

fn build_glob(pattern: &str) -> Result<globset::GlobMatcher, globset::Error> {
    Ok(globset::GlobBuilder::new(pattern)
        // `*` 不跨路径分隔符，`**` 才跨——这正是旧实现缺的那条语义。
        .literal_separator(true)
        .build()?
        .compile_matcher())
}

fn write_file(args: &str) -> String {
    let v = parse_args(args);
    let Some(target) = str_field(&v, &["target_file", "file_path", "path"]) else {
        return "Error: target_file is required".into();
    };
    let contents = v.get("contents").and_then(|x| x.as_str()).unwrap_or("");
    let path = resolve(&target);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, contents) {
        Ok(()) => format!("wrote {}", path.display()),
        Err(e) => format!("Error writing {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_dir_json_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "list_dir".into(),
            arguments: format!(
                r#"{{"target_directory":{}}}"#,
                serde_json::to_string(&dir.path().to_string_lossy()).unwrap()
            ),
        };
        let out = execute(call).await;
        assert!(out.content.contains("a.txt"), "{}", out.content);
    }

    async fn replace(path: &std::path::Path, old: &str, new: &str, all: bool) -> String {
        let call = ToolCall {
            id: "1".into(),
            name: "search_replace".into(),
            arguments: serde_json::json!({
                "file_path": path,
                "old_string": old,
                "new_string": new,
                "replace_all": all,
            })
            .to_string(),
        };
        execute(call).await.content
    }

    #[tokio::test]
    async fn search_replace_unique_match_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let out = replace(&path, "alpha", "AAA", false).await;
        assert!(out.contains("updated"), "{out}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "AAA beta gamma");
    }

    /// 原缺陷：`replace_all: false` 且 `old_string` 多处命中时，旧实现
    /// `replacen(..., 1)` **静默改掉第一处**——模型以为改的是自己瞄准的那处，
    /// 实际可能是文件里另一处同名代码。必须报错并回报命中数，且**一个字节都
    /// 不许写**。
    #[tokio::test]
    async fn search_replace_refuses_ambiguous_match_and_leaves_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        let before = "alpha beta alpha";
        std::fs::write(&path, before).unwrap();
        let out = replace(&path, "alpha", "AAA", false).await;
        assert!(out.contains("Error"), "多处命中必须报错：{out}");
        assert!(
            out.contains("2 次"),
            "要回报命中数，模型才知道该补多少上下文：{out}"
        );
        assert!(out.contains("replace_all"), "要给出逃生舱：{out}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "报错路径上文件必须原封不动"
        );
    }

    /// 多处命中时 `replace_all: true` 是明示的逃生舱，要全改并说清改了几处。
    #[tokio::test]
    async fn search_replace_all_changes_every_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "alpha beta alpha").unwrap();
        let out = replace(&path, "alpha", "AAA", true).await;
        assert!(out.contains("2 处"), "{out}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "AAA beta AAA");
    }

    #[tokio::test]
    async fn search_replace_missing_old_string_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "alpha").unwrap();
        let out = replace(&path, "zzz", "AAA", false).await;
        assert!(out.contains("未在"), "{out}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha");
    }

    async fn run_glob(pattern: &str, dir: &std::path::Path) -> String {
        let call = ToolCall {
            id: "1".into(),
            name: "glob".into(),
            arguments: serde_json::json!({
                "glob_pattern": pattern,
                "target_directory": dir,
            })
            .to_string(),
        };
        execute(call).await.content
    }

    fn glob_tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        std::fs::write(dir.path().join("root.rs"), "x").unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "x").unwrap();
        std::fs::write(dir.path().join("src/deep/b.rs"), "x").unwrap();
        dir
    }

    /// 原缺陷一：手搓 matcher 把 `**/*.rs` 翻成 `^.*.*/.*\.rs$`，强制要有一个
    /// `/`，于是**根目录下的文件全被漏掉**——而 `**/*.rs` 正是模型最常打的模式。
    #[tokio::test]
    async fn glob_doublestar_includes_root_level_files() {
        let dir = glob_tree();
        let out = run_glob("**/*.rs", dir.path()).await;
        assert!(out.contains("root.rs"), "根目录文件不该被漏掉：{out}");
        assert!(out.contains("src/a.rs"), "{out}");
        assert!(out.contains("src/deep/b.rs"), "{out}");
    }

    /// 原缺陷二：`*` 被翻成 `.*`，会跨过 `/`，于是 `src/*.rs` 把
    /// `src/deep/b.rs` 也匹配进来。globset 的 `literal_separator` 修掉这个。
    #[tokio::test]
    async fn glob_single_star_does_not_cross_separators() {
        let dir = glob_tree();
        let out = run_glob("src/*.rs", dir.path()).await;
        assert!(out.contains("src/a.rs"), "{out}");
        assert!(!out.contains("deep/b.rs"), "`*` 不该跨过 `/`：{out}");
    }

    /// 无 `/` 的模式匹配任意深度的 basename（对齐 ripgrep / DSH）。
    #[tokio::test]
    async fn glob_bare_pattern_matches_basename_at_any_depth() {
        let dir = glob_tree();
        let out = run_glob("*.rs", dir.path()).await;
        for expected in ["root.rs", "src/a.rs", "src/deep/b.rs"] {
            assert!(out.contains(expected), "缺 {expected}：{out}");
        }
    }

    /// 原缺陷三：旧实现在判 `is_dir` 之前就把条目推进结果，目录混在文件里。
    #[tokio::test]
    async fn glob_returns_files_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/x.rs"), "x").unwrap();
        let out = run_glob("target", dir.path()).await;
        assert!(
            out.contains("no matches"),
            "`target` 是目录，不该作为结果返回：{out}"
        );
    }

    #[tokio::test]
    async fn glob_respects_gitignore() {
        let dir = glob_tree();
        std::fs::write(dir.path().join(".gitignore"), "src/deep/\n").unwrap();
        let out = run_glob("**/*.rs", dir.path()).await;
        assert!(out.contains("src/a.rs"), "{out}");
        assert!(
            !out.contains("deep/b.rs"),
            "被 .gitignore 排除的不该出现：{out}"
        );
    }

    /// 空结果要说清楚搜了什么，并点出 `*` 不跨 `/` 这条最常见的踩坑。
    #[tokio::test]
    async fn glob_empty_result_explains_itself() {
        let dir = glob_tree();
        let out = run_glob("*.zzz", dir.path()).await;
        assert!(out.contains("no matches"), "{out}");
        assert!(out.contains("*.zzz"), "要回显模式：{out}");
    }

    /// `list_dir` 不该再把 `.gitignore` 排除掉的构建产物吐出来。
    #[tokio::test]
    async fn list_dir_filters_gitignored_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("keep.rs"), "x").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "list_dir".into(),
            arguments: serde_json::json!({ "target_directory": dir.path() }).to_string(),
        };
        let out = execute(call).await.content;
        assert!(out.contains("keep.rs"), "{out}");
        assert!(
            !out.contains("target/"),
            "被 .gitignore 排除的不该出现：{out}"
        );
    }

    /// 大目录收成计数 + 扩展名分布，不再逐条吐几千行。
    #[tokio::test]
    async fn list_dir_summarizes_large_directories() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..150 {
            std::fs::write(dir.path().join(format!("f{i}.rs")), "x").unwrap();
        }
        let call = ToolCall {
            id: "1".into(),
            name: "list_dir".into(),
            arguments: serde_json::json!({ "target_directory": dir.path() }).to_string(),
        };
        let out = execute(call).await.content;
        assert!(out.contains("150 个文件"), "{out}");
        assert!(out.contains("rs × 150"), "要给扩展名分布：{out}");
        assert!(out.contains("另有 50 个文件未列出"), "{out}");
    }

    /// `workdir` 让模型不必写 `cd x && ...`。
    #[tokio::test]
    async fn bash_runs_in_workdir() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "30000");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "x").unwrap();
        let out = bash(
            &serde_json::json!({"command": "ls", "workdir": dir.path()}).to_string(),
            &|| false,
            None,
        )
        .await;
        assert!(out.contains("marker.txt"), "{out}");
    }

    /// 不存在的 `workdir` 要直接报错，而不是让命令在一个意外的目录里跑掉。
    #[tokio::test]
    async fn bash_rejects_missing_workdir() {
        let out = bash(
            r#"{"command":"echo hi","workdir":"/definitely/not/here"}"#,
            &|| false,
            None,
        )
        .await;
        assert!(out.contains("不是一个已存在的目录"), "{out}");
        assert!(!out.contains("hi"), "命令不该被执行：{out}");
    }

    /// `timeout_ms` 只能收紧：传一个很短的值应该提前收掉并带回已有输出。
    #[tokio::test]
    async fn bash_timeout_ms_tightens_the_budget() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "60000");
        let start = std::time::Instant::now();
        let out = bash(
            r#"{"command":"echo early; sleep 30","timeout_ms":600}"#,
            &|| false,
            None,
        )
        .await;
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "应按 timeout_ms 收紧，实际 {:?}",
            start.elapsed()
        );
        assert!(out.contains("early"), "超时也要带回已产出的输出：{out}");
    }

    /// `timeout_ms` 不能放宽超过前台预算上限，否则一条命令能把整轮挂住。
    #[tokio::test]
    async fn bash_timeout_ms_cannot_exceed_the_ceiling() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "500");
        let start = std::time::Instant::now();
        let jobs = Jobs::new();
        let out = bash(
            r#"{"command":"sleep 30","timeout_ms":600000}"#,
            &|| false,
            Some(&jobs),
        )
        .await;
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "不该被放宽到 10 分钟，实际 {:?}",
            start.elapsed()
        );
        assert!(out.contains("转入后台"), "预算到点要转后台：{out}");
    }

    #[tokio::test]
    async fn glob_finds_txt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), "x").unwrap();
        std::fs::write(dir.path().join("skip.md"), "y").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "glob".into(),
            arguments: serde_json::json!({
                "glob_pattern": "*.txt",
                "target_directory": dir.path(),
            })
            .to_string(),
        };
        let out = execute(call).await;
        assert!(out.content.contains("keep.txt"), "{}", out.content);
        assert!(!out.content.contains("skip.md"), "{}", out.content);
    }

    #[tokio::test]
    async fn write_file_creates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let call = ToolCall {
            id: "1".into(),
            name: "write_file".into(),
            arguments: serde_json::json!({
                "target_file": path,
                "contents": "hello",
            })
            .to_string(),
        };
        let out = execute(call).await;
        assert!(out.content.contains("wrote"), "{}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[tokio::test]
    async fn read_file_omitted_limit_caps_at_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let body: String = (1..=1_050).map(|i| format!("L{i}\n")).collect();
        std::fs::write(&path, &body).unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "target_file": path }).to_string(),
        };
        let out = execute(call).await;
        assert!(
            out.content.contains("1→L1\n"),
            "{}",
            &out.content[..80.min(out.content.len())]
        );
        assert!(
            out.content.contains("truncated"),
            "should note truncation: {}",
            out.content.lines().last().unwrap_or("")
        );
        assert!(
            out.content.contains("offset=1001"),
            "hint next offset: {}",
            out.content.lines().last().unwrap_or("")
        );
        assert!(!out.content.contains("L1050\n"), "must not dump past cap");
    }

    #[tokio::test]
    async fn read_file_explicit_limit_respected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "a\nb\nc\nd\ne\n").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({
                "target_file": path,
                "offset": 2,
                "limit": 2,
            })
            .to_string(),
        };
        let out = execute(call).await;
        assert!(out.content.contains("b\n"), "{}", out.content);
        assert!(out.content.contains("c\n"), "{}", out.content);
        assert!(!out.content.contains("d\n"), "{}", out.content);
        assert!(out.content.contains("offset=4"), "{}", out.content);
    }

    #[tokio::test]
    async fn read_file_small_file_no_truncation_note() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.txt");
        std::fs::write(&path, "only\n").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "target_file": path }).to_string(),
        };
        let out = execute(call).await;
        assert!(out.content.contains("1→only"), "{}", out.content);
        assert!(!out.content.contains("truncated"), "{}", out.content);
    }

    #[tokio::test]
    async fn read_file_returns_image_for_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend(std::iter::repeat_n(0u8, 40));
        std::fs::write(&path, &png).unwrap();
        let args = serde_json::json!({"target_file": path}).to_string();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: args,
        };
        let result = execute(call).await;
        assert!(
            result.content.contains("Image content included inline"),
            "{}",
            result.content
        );
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].mime, "image/png");
    }

    /// 前台 bash 的管道死锁：输出超过管道缓冲（64KB）就会把子进程堵死，
    /// 一条 0.1s 的命令要等满整个前台预算再被杀，输出还全丢。
    #[tokio::test]
    async fn bash_large_output_does_not_deadlock() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "3000");
        let start = std::time::Instant::now();
        let out = bash(
            r#"{"command":"yes OUTLINE0123456789 | head -20000; echo finished"}"#,
            &|| false,
            None,
        )
        .await;
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "0.1s 的命令不该耗满前台预算，实际 {:?}",
            start.elapsed()
        );
        assert!(
            out.contains("finished"),
            "收尾行应保留：{}",
            &out[..out.len().min(200)]
        );
    }

    /// 到了前台预算要把已经产出的输出带回来。旧实现直接 `child.kill()` 后返回
    /// 一句错误字符串，从不读管道，模型什么都拿不到。
    #[tokio::test]
    async fn bash_timeout_keeps_partial_output() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "700");
        let jobs = Jobs::new();
        let out = bash(
            r#"{"command":"echo early-line; sleep 30"}"#,
            &|| false,
            Some(&jobs),
        )
        .await;
        assert!(
            out.contains("early-line"),
            "超时也要带回已产出的输出：{out}"
        );
        assert!(out.contains("转入后台"), "要说明去向：{out}");
    }

    /// 前台预算到点**不杀进程**，转后台接着跑。
    ///
    /// 旧行为是 kill + 「需要跑完就用 is_background: true 重跑」：那条命令已经跑了
    /// 几分钟，杀掉等于把这几分钟扔了，重跑还要再花同样的时间、大概率再超时一次。
    #[tokio::test]
    async fn bash_timeout_backgrounds_instead_of_killing() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "500");
        let jobs = Jobs::new();
        let out = bash(
            r#"{"command":"echo before-budget; sleep 1.2; echo after-budget"}"#,
            &|| false,
            Some(&jobs),
        )
        .await;

        // 返回的不是错误：模型看到 `Error:` 会重试或换路子，而它什么都没做错。
        assert!(!out.contains("Error:"), "超时转后台不是错误：{out}");
        assert!(out.contains("before-budget"), "已产出的输出要带回：{out}");
        assert!(out.contains("不要重跑"), "要明说别重跑：{out}");

        // task_id 必须能对上一条还活着的任务，否则这条提示是空头支票。
        let id = out
            .lines()
            .find_map(|l| l.strip_prefix("task_id: "))
            .expect("要给出 task_id")
            .trim()
            .to_string();
        let snap = jobs.snapshot(&id).expect("任务还在表里");
        assert!(!snap.foreground, "转后台后不该再算前台：{id}");

        // 关键：进程没被杀，预算之后的那一行照样产出。
        let mut tail = String::new();
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            tail = jobs.snapshot(&id).map(|s| s.output).unwrap_or_default();
            if tail.contains("after-budget") {
                break;
            }
        }
        assert!(
            tail.contains("after-budget"),
            "转后台后命令应继续跑完，实际：{tail}"
        );
    }

    /// 取消（用户按 Esc）仍然是**杀掉**，不是转后台——那是明确要它停。
    #[tokio::test]
    async fn bash_cancel_still_kills() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "30000");
        let jobs = Jobs::new();
        let cancel_at = std::time::Instant::now() + Duration::from_millis(400);
        let out = bash(
            r#"{"command":"echo started; sleep 30"}"#,
            &|| std::time::Instant::now() >= cancel_at,
            Some(&jobs),
        )
        .await;
        assert!(out.contains("cancelled"), "取消要说明自己是取消：{out}");
        assert!(out.contains("started"), "已产出的输出仍要带回：{out}");
        assert!(!out.contains("task_id"), "取消不该留下后台任务：{out}");
        assert!(jobs.list().is_empty(), "取消后任务要摘掉");
    }

    /// 没挂 `"jobs"` 服务时不能转后台：本地表随调用一起析构，那个 task_id 没人
    /// 查得到。宁可退回 kill + 诚实报错，也不发空头支票。
    #[tokio::test]
    async fn bash_timeout_without_a_jobs_service_still_kills() {
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "500");
        let out = bash(r#"{"command":"echo only-line; sleep 30"}"#, &|| false, None).await;
        assert!(out.contains("only-line"), "已产出的输出要带回：{out}");
        assert!(
            out.contains("超时"),
            "没有可查的任务表就该照实报超时：{out}"
        );
        assert!(!out.contains("task_id"), "不该发无处可查的 task_id：{out}");
    }

    /// 前台命令必须在 `jobs` 里现身，否则 TUI 无处读它的实时输出；
    /// 结束后要摘掉，不能常驻。
    #[tokio::test]
    async fn bash_foreground_shows_live_progress_in_jobs() {
        // 同一进程里别的用例会改 DOCK_BASH_FOREGROUND_MS；不拿这把锁就会被它们的
        // 短预算污染，表现为本用例随机超时。
        let _env = cordis_base::test_env::scoped().set(FOREGROUND_MS_ENV, "30000");
        let jobs = Jobs::new();
        let run = bash(
            r#"{"command":"for i in 1 2 3 4 5 6; do echo tick-$i; sleep 0.2; done"}"#,
            &|| false,
            Some(&jobs),
        );
        let watch = async {
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if jobs
                    .list()
                    .iter()
                    .any(|j| j.foreground && j.output.contains("tick-1"))
                {
                    return true;
                }
            }
            false
        };
        let (out, seen) = tokio::join!(run, watch);
        assert!(seen, "前台命令运行期间应能在 jobs 里看到它的实时输出");
        assert!(out.contains("tick-6"), "最终结果要完整：{out}");
        assert!(jobs.list().is_empty(), "前台任务结束后应从表里摘掉");
    }
}
