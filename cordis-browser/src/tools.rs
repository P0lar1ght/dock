//! 浏览器工具清单（MCP `tools/list` 的形状）与参数解析。
//!
//! 工具名保持 `browser_*`：Dock 里的公名是 `mcp_browser__browser_*`，换个服务器键名
//! 挂也不影响工具本身。

use serde_json::{json, Value};

/// 一颗工具的静态定义。
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema（`inputSchema`）。
    pub schema: &'static str,
    /// 只读（不改页面状态）：进 MCP `annotations.readOnlyHint`。
    pub read_only: bool,
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "browser_open",
        description: "Open a URL in this session's own tab in Dock's Chromium (chromiumoxide CDP). Lazily launches Chromium under $DOCK_HOME/browser/user-data, or attaches to the one already running. Logins/cookies are shared across sessions; tabs are not. Use browser_open for first connect; use browser_navigate to jump in an already-open session.",
        schema: r#"{"type":"object","properties":{"url":{"type":"string","description":"URL to open."}},"required":["url"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_navigate",
        description: "Navigate this session's active tab to a URL. Errors if this session has no tabs yet — call browser_open first. Unlike browser_open, does not launch Chromium.",
        schema: r#"{"type":"object","properties":{"url":{"type":"string","description":"URL to navigate to."}},"required":["url"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_navigate_back",
        description: "Go back in history for the active tab (existing session only; errors if Closed).",
        schema: r#"{"type":"object","properties":{}}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_snapshot",
        description: "Lean accessibility snapshot of the current page (or same-origin iframe via frame/frame_selector) with refs (@eN) for click/type/hover/select/fill. Prefer interactive=true. Cross-origin iframes fail clearly.",
        schema: r#"{"type":"object","properties":{"interactive":{"type":"boolean","description":"If true (default), only interactive element refs."},"frame":{"type":"string","description":"Optional CSS selector for a same-origin iframe/frame."},"frame_selector":{"type":"string","description":"Alias of frame."}}}"#,
        read_only: true,
    },
    ToolDef {
        name: "browser_click",
        description: "Click an element from browser_snapshot by ref (@eN). If the ref came from a framed snapshot, pass the same frame/frame_selector for clarity (refs already target that frame).",
        schema: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."},"frame":{"type":"string","description":"Optional CSS selector matching the iframe used for snapshot."},"frame_selector":{"type":"string","description":"Alias of frame."}},"required":["ref"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_hover",
        description: "Hover an element from browser_snapshot by ref (@eN).",
        schema: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."}},"required":["ref"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_type",
        description: "Type text into an element from browser_snapshot (optional ref focuses first).",
        schema: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."},"text":{"type":"string","description":"Text to type."},"submit":{"type":"boolean","description":"Press Enter after typing."}},"required":["text"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_press_key",
        description: "Press a key or shortcut (Enter/Tab/Escape/ArrowDown or Control+a, Meta+Shift+t). Optional ref focuses that element first.",
        schema: r#"{"type":"object","properties":{"key":{"type":"string","description":"Key or chord (e.g. Enter, Tab, Control+a)."},"ref":{"type":"string","description":"Optional snapshot ref to focus before pressing."}},"required":["key"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_select_option",
        description: "Select an option on a <select> (or similar) by snapshot ref plus value and/or label.",
        schema: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."},"value":{"type":"string","description":"Option value attribute."},"label":{"type":"string","description":"Option visible label/text."}},"required":["ref"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_fill_form",
        description: "Fill multiple form fields in one call. Pass fields: [{ref, value}, ...].",
        schema: r#"{"type":"object","properties":{"fields":{"type":"array","description":"Array of {ref, value} objects.","items":{"type":"object","properties":{"ref":{"type":"string"},"value":{}},"required":["ref"]}}},"required":["fields"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_wait_for",
        description: "Wait for visible text and/or a CSS selector, or just sleep for timeout_ms when neither is set.",
        schema: r#"{"type":"object","properties":{"text":{"type":"string","description":"Substring to wait for in document body text."},"selector":{"type":"string","description":"CSS selector to wait for."},"timeout_ms":{"type":"integer","description":"Max wait in milliseconds (default 30000)."}}}"#,
        read_only: true,
    },
    ToolDef {
        name: "browser_drag",
        description: "Drag from a source snapshot ref (or start_x/start_y) to a target ref (or end_x/end_y) via CDP mouse events.",
        schema: r#"{"type":"object","properties":{"source_ref":{"type":"string","description":"Snapshot ref to drag from."},"target_ref":{"type":"string","description":"Snapshot ref to drop on."},"start_x":{"type":"number"},"start_y":{"type":"number"},"end_x":{"type":"number"},"end_y":{"type":"number"},"steps":{"type":"integer","description":"Intermediate mouseMoved steps (default 10)."}}}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_handle_dialog",
        description: "Accept or dismiss a JavaScript dialog (alert/confirm/prompt/beforeunload). Optional prompt_text for prompt dialogs. Wire Page.javascriptDialogOpening so agents are not stuck on native dialogs.",
        schema: r#"{"type":"object","properties":{"accept":{"type":"boolean","description":"true to accept/OK, false to dismiss/Cancel."},"prompt_text":{"type":"string","description":"Text for prompt dialogs before accepting."}},"required":["accept"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_file_upload",
        description: "Set files on a file input from browser_snapshot ref via DOM.setFileInputFiles. Pass paths: [\"/abs/path\", ...] or path for one file.",
        schema: r#"{"type":"object","properties":{"ref":{"type":"string","description":"File input ref from browser_snapshot."},"paths":{"type":"array","items":{"type":"string"},"description":"Absolute or relative file paths."},"path":{"type":"string","description":"Single file path (alternative to paths)."}},"required":["ref"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_resize",
        description: "Set the page viewport width/height via Emulation.setDeviceMetricsOverride.",
        schema: r#"{"type":"object","properties":{"width":{"type":"integer","description":"Viewport width in CSS pixels."},"height":{"type":"integer","description":"Viewport height in CSS pixels."}},"required":["width","height"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_evaluate",
        description: "Run JavaScript in the page (or same-origin iframe via frame/frame_selector) via CDP Runtime.evaluate. Returns a truncated string/JSON result. The host gates this like a shell command.",
        schema: r#"{"type":"object","properties":{"expression":{"type":"string","description":"JS expression or function to evaluate."},"code":{"type":"string","description":"Alias of expression."},"frame":{"type":"string","description":"Optional CSS selector for a same-origin iframe/frame."},"frame_selector":{"type":"string","description":"Alias of frame."}},"required":["expression"]}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_console_messages",
        description: "Read-only recent console API messages captured since this session's tabs opened (truncated).",
        schema: r#"{"type":"object","properties":{}}"#,
        read_only: true,
    },
    ToolDef {
        name: "browser_network_requests",
        description: "Read-only list of recent network requests (method/url/status/type). Truncated; never dumps response bodies.",
        schema: r#"{"type":"object","properties":{}}"#,
        read_only: true,
    },
    ToolDef {
        name: "browser_screenshot",
        description: "Capture a screenshot of this session's active tab into $DOCK_HOME/browser/screenshots/ and return it as an image.",
        schema: r#"{"type":"object","properties":{"full_page":{"type":"boolean","description":"Capture the full scrollable page."}}}"#,
        read_only: true,
    },
    ToolDef {
        name: "browser_tabs",
        description: "List or switch this session's tabs (list|new|switch|close). Other sessions' tabs are never listed.",
        schema: r#"{"type":"object","properties":{"action":{"type":"string","enum":["list","new","switch","close"],"description":"Tab action. Default list."},"index":{"type":"integer","description":"Tab index for switch/close."},"url":{"type":"string","description":"URL when action is new."}}}"#,
        read_only: false,
    },
    ToolDef {
        name: "browser_close",
        description: "Close this session's tabs. Chromium quits once no session has tabs left.",
        schema: r#"{"type":"object","properties":{}}"#,
        read_only: false,
    },
];

/// 全部工具名，顺序同 [`TOOLS`]。
pub fn tool_names() -> Vec<&'static str> {
    TOOLS.iter().map(|t| t.name).collect()
}

/// MCP `tools/list` 结果里的 `tools` 数组。
pub fn list_json() -> Value {
    Value::Array(
        TOOLS
            .iter()
            .map(|t| {
                let schema: Value =
                    serde_json::from_str(t.schema).unwrap_or_else(|_| json!({"type": "object"}));
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": schema,
                    "annotations": {
                        "readOnlyHint": t.read_only,
                        "openWorldHint": true,
                    },
                })
            })
            .collect(),
    )
}

pub(crate) fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

pub(crate) fn arg_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|x| x.as_bool())
}

pub(crate) fn arg_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key).and_then(|x| {
        x.as_u64()
            .map(|n| n as usize)
            .or_else(|| x.as_i64().and_then(|n| usize::try_from(n).ok()))
    })
}

pub(crate) fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|x| {
        x.as_u64()
            .or_else(|| x.as_i64().and_then(|n| u64::try_from(n).ok()))
    })
}

pub(crate) fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_u64().and_then(|n| i64::try_from(n).ok()))
            .or_else(|| x.as_f64().map(|n| n as i64))
    })
}

pub(crate) fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|x| {
        x.as_f64()
            .or_else(|| x.as_i64().map(|n| n as f64))
            .or_else(|| x.as_u64().map(|n| n as f64))
    })
}

pub(crate) fn parse_paths(args: &Value) -> Result<Vec<String>, String> {
    if let Some(arr) = args.get("paths").and_then(|x| x.as_array()) {
        let mut out = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let s = item
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("paths[{i}] must be a non-empty string"))?;
            out.push(s.to_string());
        }
        return Ok(out);
    }
    if let Some(p) = args
        .get("path")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
    {
        return Ok(vec![p.to_string()]);
    }
    Err("paths array (or path string) is required".into())
}

pub(crate) fn parse_fill_fields(args: &Value) -> Result<Vec<(String, String)>, String> {
    let arr = args
        .get("fields")
        .and_then(|x| x.as_array())
        .ok_or_else(|| "fields array is required".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let r = item
            .get("ref")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("fields[{i}].ref is required"))?;
        let value = item
            .get("value")
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        out.push((r.to_string(), value));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_schema_parses_and_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(t.name.starts_with("browser_"), "{}", t.name);
            assert!(seen.insert(t.name), "duplicate {}", t.name);
            let schema: Value = serde_json::from_str(t.schema).expect(t.name);
            assert_eq!(schema["type"], "object", "{}", t.name);
            assert!(!t.description.contains("use_tool"), "{}", t.name);
        }
        assert_eq!(TOOLS.len(), 21);
    }

    #[test]
    fn list_json_marks_read_only_tools() {
        let list = list_json();
        let snapshot = list
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "browser_snapshot")
            .unwrap();
        assert_eq!(snapshot["annotations"]["readOnlyHint"], true);
        assert!(snapshot["inputSchema"]["properties"]["interactive"].is_object());
        let click = list
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "browser_click")
            .unwrap();
        assert_eq!(click["annotations"]["readOnlyHint"], false);
    }

    #[test]
    fn fill_fields_and_paths_parse() {
        let fields = parse_fill_fields(&json!({"fields": [{"ref": "e1", "value": 3}]})).unwrap();
        assert_eq!(fields, vec![("e1".to_string(), "3".to_string())]);
        assert!(parse_fill_fields(&json!({})).is_err());
        assert_eq!(parse_paths(&json!({"path": "/a"})).unwrap(), vec!["/a"]);
        assert!(parse_paths(&json!({"paths": [""]})).is_err());
    }
}
