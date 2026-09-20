//! `write_file` workspace tool.

use crate::tools::fs_common::{parse_args, resolve, str_field};
use cordis_base::types::ToolSpec;

const WRITE_FILE_PARAMS: &str = r#"{"type":"object","properties":{"target_file":{"type":"string","description":"Path to write, relative to cwd or absolute. Parent directories are created."},"contents":{"type":"string","description":"Full text content to write. Existing files are overwritten in full."}},"required":["target_file","contents"]}"#;

const WRITE_FILE_DESC: &str = "Write contents to a file, creating it or replacing it in full.\n\
- Existing files are overwritten entirely, so read the file first unless you just created it. For a targeted change use search_replace instead.\n\
- Parent directories are created automatically.";

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "write_file".into(),
        description: WRITE_FILE_DESC.into(),
        parameters_json: WRITE_FILE_PARAMS.into(),
    }
}

pub(crate) fn run(args: &str) -> String {
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
