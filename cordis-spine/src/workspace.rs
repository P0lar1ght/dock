//! Workspace tools with Grok JSON names. Copied field names from
//! `xai-grok-tools` `list_dir` / `read_file` / `grep` / `search_replace` / `bash`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::jobs::Jobs;
use crate::types::{ToolCall, ToolResult, ToolSpec};

const LIST_DIR_PARAMS: &str = r#"{"type":"object","properties":{"target_directory":{"type":"string","description":"Path to directory to list, relative to cwd or absolute."}},"required":["target_directory"]}"#;
const READ_FILE_PARAMS: &str = r#"{"type":"object","properties":{"target_file":{"type":"string","description":"Path of the file to read (relative to cwd or absolute)."},"offset":{"type":"integer","description":"1-based start line. Omit to start at line 1. Use with limit for large files."},"limit":{"type":"integer","description":"Max lines to return. Omit to use the default cap (1000). Pass a smaller value for a tight window."}},"required":["target_file"]}"#;
const GREP_PARAMS: &str = r#"{"type":"object","properties":{"pattern":{"type":"string","description":"Regex pattern (rg --regexp)."},"path":{"type":"string","description":"File or directory to search."}},"required":["pattern"]}"#;
const SEARCH_REPLACE_PARAMS: &str = r#"{"type":"object","properties":{"file_path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["file_path","old_string","new_string"]}"#;
const BASH_PARAMS: &str = r#"{"type":"object","properties":{"command":{"type":"string","description":"The bash command to run."},"is_background":{"type":"boolean","description":"Set to true for long-running commands. Returns a task id immediately."},"block_until_ms":{"type":"integer","description":"Foreground wait in ms. 0 backgrounds immediately."}},"required":["command"]}"#;
const GLOB_PARAMS: &str = r#"{"type":"object","properties":{"glob_pattern":{"type":"string"},"target_directory":{"type":"string"}},"required":["glob_pattern"]}"#;
const WRITE_FILE_PARAMS: &str = r#"{"type":"object","properties":{"target_file":{"type":"string"},"contents":{"type":"string"}},"required":["target_file","contents"]}"#;

/// Default max lines when the model omits `limit` (grok `MAX_LINES_READ`).
const MAX_LINES_READ: usize = 1_000;

const READ_FILE_DESC: &str = "Read a file.\n\
- By default reads up to 1000 lines from offset (default line 1).\n\
- For large files, pass offset + limit to page through; the result notes how many lines remain.\n\
- Line anchors appear as N→ on line 1 and every 10th line.";

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "list_dir".into(),
            description: "List directory contents.".into(),
            parameters_json: LIST_DIR_PARAMS.into(),
        },
        ToolSpec {
            name: "read_file".into(),
            description: READ_FILE_DESC.into(),
            parameters_json: READ_FILE_PARAMS.into(),
        },
        ToolSpec {
            name: "grep".into(),
            description: "Search file contents with ripgrep.".into(),
            parameters_json: GREP_PARAMS.into(),
        },
        ToolSpec {
            name: "search_replace".into(),
            description: "Replace an exact string in a file.".into(),
            parameters_json: SEARCH_REPLACE_PARAMS.into(),
        },
        ToolSpec {
            name: "bash".into(),
            description: "Run a bash command in the current working directory. Set is_background true (or block_until_ms: 0) for long-running commands; you get a task id and check it with get_task_output / kill_task.".into(),
            parameters_json: BASH_PARAMS.into(),
        },
        ToolSpec {
            name: "glob".into(),
            description: "Find files matching a glob pattern.".into(),
            parameters_json: GLOB_PARAMS.into(),
        },
        ToolSpec {
            name: "write_file".into(),
            description: "Write contents to a file (creates or overwrites).".into(),
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
pub fn execute(call: ToolCall) -> ToolResult {
    execute_with(call, || false, None)
}

pub fn execute_with(
    call: ToolCall,
    is_cancelled: impl Fn() -> bool,
    jobs: Option<&Jobs>,
) -> ToolResult {
    if call.name == "read_file" {
        if let Some((content, images)) = read_file_maybe_image(&call.arguments) {
            return ToolResult {
                call_id: call.id,
                name: call.name,
                content,
                images: crate::tool_images::cap_images(images),
            };
        }
    }
    let content = match call.name.as_str() {
        "list_dir" => list_dir(&call.arguments),
        "read_file" => read_file(&call.arguments),
        "grep" => grep(&call.arguments),
        "search_replace" => search_replace(&call.arguments),
        "bash" | "run_terminal_cmd" => bash(&call.arguments, &is_cancelled, jobs),
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

fn list_dir(args: &str) -> String {
    let v = parse_args(args);
    let target = str_field(&v, &["target_directory", "path"]).unwrap_or_else(|| ".".into());
    let path = resolve(&target);
    let read = match std::fs::read_dir(&path) {
        Ok(r) => r,
        Err(e) => return format!("Error: {} is not a valid directory ({e})", path.display()),
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(format!("{name}/"));
        } else {
            files.push(name);
        }
    }
    dirs.sort();
    files.sort();
    let mut lines = dirs;
    lines.extend(files);
    if lines.is_empty() {
        format!("{} (empty)", path.display())
    } else {
        format!("{}\n{}", path.display(), lines.join("\n"))
    }
}

/// When the target is png/jpeg/webp/gif, return inline image + placeholder
/// instead of `read_to_string` (which fails / garbles binaries).
fn read_file_maybe_image(args: &str) -> Option<(String, Vec<crate::types::UserImage>)> {
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
    let img = crate::tool_images::user_image_from_path(&path)?;
    let content = format!(
        "{}
{}",
        path.display(),
        crate::tool_images::IMAGE_INLINE_PLACEHOLDER
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
            if n == 1 || n % 10 == 0 {
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

fn grep(args: &str) -> String {
    let v = parse_args(args);
    let Some(pattern) = str_field(&v, &["pattern"]) else {
        return "Error: pattern is required".into();
    };
    let path = str_field(&v, &["path"]).unwrap_or_else(|| ".".into());
    match Command::new("rg")
        .args(["--no-heading", "-n", "--color", "never", "-m", "100"])
        .arg(&pattern)
        .arg(&path)
        .output()
    {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                if text.is_empty() {
                    text = err.into_owned();
                }
            }
            if text.trim().is_empty() {
                "no matches".into()
            } else {
                text
            }
        }
        Err(_) => grep_walk(Path::new(&path), &pattern),
    }
}

fn grep_walk(root: &Path, pattern: &str) -> String {
    let Ok(re) = regex::Regex::new(pattern) else {
        return format!("Error: invalid regex: {pattern}");
    };
    let mut hits = Vec::new();
    fn walk(dir: &Path, re: &regex::Regex, hits: &mut Vec<String>) {
        if hits.len() >= 100 {
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read.flatten() {
            if hits.len() >= 100 {
                return;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" || name == "target" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                walk(&path, re, hits);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                for (i, line) in text.lines().enumerate() {
                    if re.is_match(line) {
                        hits.push(format!("{}:{}:{line}", path.display(), i + 1));
                        if hits.len() >= 100 {
                            return;
                        }
                    }
                }
            }
        }
    }
    walk(root, &re, &mut hits);
    if hits.is_empty() {
        "no matches".into()
    } else {
        hits.join("\n")
    }
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
    if !text.contains(&old) {
        return format!("Error: old_string not found in {}", path.display());
    }
    let next = if replace_all {
        text.replace(&old, &new)
    } else {
        text.replacen(&old, &new, 1)
    };
    match std::fs::write(&path, next) {
        Ok(()) => format!("updated {}", path.display()),
        Err(e) => format!("Error writing {}: {e}", path.display()),
    }
}

fn bash(args: &str, is_cancelled: &dyn Fn() -> bool, jobs: Option<&Jobs>) -> String {
    let v = parse_args(args);
    let Some(command) = str_field(&v, &["command"]) else {
        return "Error: command is required".into();
    };
    let background = bool_field(&v, "is_background")
        || bool_field(&v, "background")
        || int_field(&v, "block_until_ms") == Some(0);
    if background {
        if let Some(jobs) = jobs {
            let id = jobs.start(command);
            // Grok `format_default_prompt` backgrounded + `background_retrieval_hint`.
            return format!(
                "[Command moved to background]\n\n\
                 task_id: {id}\n\n\
                 The command is still running in the background. You can continue with other tasks.\n\
                 Use get_task_output with task_ids=[\"{id}\"] when you need the output."
            );
        }
    }
    let mut child = match Command::new("bash")
        .arg("-lc")
        .arg(&command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return format!("Error spawning bash: {e}"),
    };
    let timeout = Duration::from_secs(30);
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = std::io::Read::read_to_string(&mut out, &mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut err, &mut stderr);
                }
                let mut body = stdout;
                if !stderr.is_empty() {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(&stderr);
                }
                if !status.success() {
                    body = format!("exit {status}\n{body}");
                }
                if body.trim().is_empty() {
                    body = "(no output)".into();
                }
                return body;
            }
            Ok(None) if is_cancelled() => {
                let _ = child.kill();
                return "cancelled".into();
            }
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                return "Error: command timed out after 30s".into();
            }
            Err(e) => return format!("Error waiting on bash: {e}"),
        }
    }
}

fn glob_files(args: &str) -> String {
    let v = parse_args(args);
    let Some(pattern) = str_field(&v, &["glob_pattern", "pattern"]) else {
        return "Error: glob_pattern is required".into();
    };
    let root = str_field(&v, &["target_directory", "path"]).unwrap_or_else(|| ".".into());
    let root = resolve(&root);
    let mut hits = Vec::new();
    fn walk(dir: &Path, root: &Path, pat: &str, hits: &mut Vec<String>) {
        if hits.len() >= 200 {
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read.flatten() {
            if hits.len() >= 200 {
                return;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" || name == "target" || name == "node_modules" {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if glob_match(pat, &rel) || glob_match(pat, &name) {
                hits.push(rel);
            }
            if path.is_dir() {
                walk(&path, root, pat, hits);
            }
        }
    }
    walk(&root, &root, &pattern, &mut hits);
    if hits.is_empty() {
        "no matches".into()
    } else {
        hits.join("\n")
    }
}

fn glob_match(pat: &str, text: &str) -> bool {
    let mut regex = String::from("^");
    for ch in pat.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            c if ".+()[]{}|^$\\".contains(c) => {
                regex.push('\\');
                regex.push(c);
            }
            c => regex.push(c),
        }
    }
    regex.push('$');
    regex::Regex::new(&regex)
        .map(|re| re.is_match(text))
        .unwrap_or(false)
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

    #[test]
    fn list_dir_json_name() {
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
        let out = execute(call);
        assert!(out.content.contains("a.txt"), "{}", out.content);
    }

    #[test]
    fn search_replace_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "alpha beta alpha").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "search_replace".into(),
            arguments: serde_json::json!({
                "file_path": path,
                "old_string": "alpha",
                "new_string": "AAA",
            })
            .to_string(),
        };
        execute(call);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "AAA beta alpha");
    }

    #[test]
    fn glob_finds_txt() {
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
        let out = execute(call);
        assert!(out.content.contains("keep.txt"), "{}", out.content);
        assert!(!out.content.contains("skip.md"), "{}", out.content);
    }

    #[test]
    fn write_file_creates() {
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
        let out = execute(call);
        assert!(out.content.contains("wrote"), "{}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn read_file_omitted_limit_caps_at_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let body: String = (1..=1_050).map(|i| format!("L{i}\n")).collect();
        std::fs::write(&path, &body).unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "target_file": path }).to_string(),
        };
        let out = execute(call);
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

    #[test]
    fn read_file_explicit_limit_respected() {
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
        let out = execute(call);
        assert!(out.content.contains("b\n"), "{}", out.content);
        assert!(out.content.contains("c\n"), "{}", out.content);
        assert!(!out.content.contains("d\n"), "{}", out.content);
        assert!(out.content.contains("offset=4"), "{}", out.content);
    }

    #[test]
    fn read_file_small_file_no_truncation_note() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.txt");
        std::fs::write(&path, "only\n").unwrap();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "target_file": path }).to_string(),
        };
        let out = execute(call);
        assert!(out.content.contains("1→only"), "{}", out.content);
        assert!(!out.content.contains("truncated"), "{}", out.content);
    }

    #[test]
    fn read_file_returns_image_for_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend(std::iter::repeat(0u8).take(40));
        std::fs::write(&path, &png).unwrap();
        let args = serde_json::json!({"target_file": path}).to_string();
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: args,
        };
        let result = execute(call);
        assert!(
            result.content.contains("Image content included inline"),
            "{}",
            result.content
        );
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].mime, "image/png");
    }
}
