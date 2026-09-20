//! `read_file` workspace tool (text + images via `tool_images`).

use crate::tools::fs_common::{int_field, parse_args, resolve, str_field};
use cordis_base::types::ToolSpec;

const READ_FILE_PARAMS: &str = r#"{"type":"object","properties":{"target_file":{"type":"string","description":"Path of the file to read (relative to cwd or absolute)."},"offset":{"type":"integer","description":"1-based start line. Omit to start at line 1. Use with limit for large files."},"limit":{"type":"integer","description":"Max lines to return. Omit to use the default cap (1000). Pass a smaller value for a tight window."}},"required":["target_file"]}"#;

const READ_FILE_DESC: &str = "Read a file.\n\
- Use this instead of `cat` / `head` / `sed -n` through bash: it is gated as read-only, works in plan mode, and tells you how much of the file you have not seen.\n\
- By default reads up to 1000 lines from offset (default line 1).\n\
- For large files, pass offset + limit to page through; the result notes how many lines remain.\n\
- Line anchors appear as N→ on line 1 and every 10th line. That prefix is not part of the file — when passing text to search_replace, match only what comes after the →.\n\
- Image files (png/jpg/jpeg/webp/gif) come back as pixels, not text.";

/// Default max lines when the model omits `limit` (grok `MAX_LINES_READ`).
const MAX_LINES_READ: usize = 1_000;

/// When the target is png/jpeg/webp/gif, return inline image + placeholder
/// instead of `read_to_string` (which fails / garbles binaries).
pub(crate) fn maybe_image(args: &str) -> Option<(String, Vec<cordis_base::types::UserImage>)> {
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

pub(crate) fn run(args: &str) -> String {
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

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "read_file".into(),
        description: READ_FILE_DESC.into(),
        parameters_json: READ_FILE_PARAMS.into(),
    }
}
