//! `glob` workspace tool.

use crate::tools::fs_common::{parse_args, resolve, str_field};
use cordis_base::types::ToolSpec;

const GLOB_PARAMS: &str = r#"{"type":"object","properties":{"glob_pattern":{"type":"string","description":"Glob to match file paths against, e.g. \"**/*.rs\" or \"src/**/test_*.py\". * does not cross directory separators; use ** to span directories. A pattern with no / matches the basename at any depth."},"target_directory":{"type":"string","description":"Directory to search in. Defaults to the current working directory."}},"required":["glob_pattern"]}"#;

const GLOB_DESC: &str = "Find files whose path matches a glob pattern.\n\
- Use this instead of `find` or `ls **` through bash: it is gated as read-only and works in plan mode.\n\
- Returns files only, never directories, newest first (modification time), so the head of the result is the code most recently worked on.\n\
- `*` does not cross `/`; use `**` to span directories. A pattern with no `/` matches the basename at any depth, so `*.rs` searches the whole tree.\n\
- Respects .gitignore and skips dot-files. Capped results say how many paths were not shown.\n\
- This finds files by *name*. To find them by *content*, use grep.";

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "glob".into(),
        description: GLOB_DESC.into(),
        parameters_json: GLOB_PARAMS.into(),
    }
}

/// `glob` 一次最多内联多少条路径。
const GLOB_MAX_RESULTS: usize = 200;

pub(crate) fn run(args: &str) -> String {
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
