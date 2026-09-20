//! `search_replace` workspace tool.

use crate::tools::fs_common::{bool_field, parse_args, resolve, str_field};
use cordis_base::types::ToolSpec;

const SEARCH_REPLACE_PARAMS: &str = r#"{"type":"object","properties":{"file_path":{"type":"string","description":"Path of the file to modify, relative to cwd or absolute."},"old_string":{"type":"string","description":"Exact text to replace. Must match exactly one place in the file unless replace_all is true. Set to an empty string to create a new file."},"new_string":{"type":"string","description":"Replacement text. Must differ from old_string."},"replace_all":{"type":"boolean","description":"Replace every occurrence instead of requiring old_string to be unique. Use when renaming an identifier."}},"required":["file_path","old_string","new_string"]}"#;

const SEARCH_REPLACE_DESC: &str = "Replace an exact string in a file.\n\
- read_file prefixes each line with \"N→\". That prefix is not part of the file: match only what comes after the →, with its exact indentation.\n\
- old_string must match exactly one place in the file. If it appears more than once the call fails and reports the count — add surrounding lines to make it unique, or set replace_all to change every occurrence (handy for renaming an identifier).\n\
- To create a new file, set old_string to an empty string. An empty old_string cannot overwrite an existing non-empty file.\n\
- Prefer this over write_file for targeted edits: write_file replaces the whole file.";

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "search_replace".into(),
        description: SEARCH_REPLACE_DESC.into(),
        parameters_json: SEARCH_REPLACE_PARAMS.into(),
    }
}

pub(crate) fn run(args: &str) -> String {
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
