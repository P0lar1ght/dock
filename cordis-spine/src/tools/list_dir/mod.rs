//! `list_dir` workspace tool.

use crate::tools::fs_common::{parse_args, resolve, str_field};
use cordis_base::types::ToolSpec;

const LIST_DIR_PARAMS: &str = r#"{"type":"object","properties":{"target_directory":{"type":"string","description":"Path to directory to list, relative to cwd or absolute."}},"required":["target_directory"]}"#;

const LIST_DIR_DESC: &str = "List the contents of a directory.\n\
- Use this instead of `ls` through bash: it is gated as read-only and works in plan mode.\n\
- Respects .gitignore, so build output and vendored dependencies do not drown the result. Dot-files are hidden.\n\
- One level only — it does not recurse. Use glob to find files by path pattern, or grep to find them by content.\n\
- Large directories are summarized with a file count and an extension breakdown instead of listing every entry.";

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "list_dir".into(),
        description: LIST_DIR_DESC.into(),
        parameters_json: LIST_DIR_PARAMS.into(),
    }
}

/// 超过这个条目数就不再逐条列，改成计数 + 扩展名分布。
const LIST_DIR_SUMMARY_THRESHOLD: usize = 100;

pub(crate) fn run(args: &str) -> String {
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
