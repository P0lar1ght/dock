//! `tool-canvas`：模型写自包含 HTML，Dock 桌面端放进会话旁的沙箱 iframe 里跑。
//!
//! 四颗工具：`canvas_create` 新建、`canvas_edit` 改 HTML 出新版、`canvas_data`
//! 只换数据（不出新版）、`canvas_read` 读回。落盘见 [`cordis_base::canvas`]：
//! `<会话目录>/canvas/<id>/`。网关的 `canvas/*` 读同一份文件。
//!
//! 画布属于**页**：子代理（自己的 `Sessions`、不落盘）调用时写到父页的会话目录，
//! 借 `"planMode"` 找页——它按页挂、不按子代理隔离（同 plan 文件的解析）。

use std::path::PathBuf;

use cordis::{plugin, Context, Inject, Plugin};
use cordis_base::canvas::{self, Edit, Meta};
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};
use serde::Deserialize;
use serde_json::Value;

use crate::names::{PLAN_MODE, SESSIONS, TOOLS};
use crate::session::log::Sessions;
use crate::tools::plan_mode::PlanMode;
use crate::tools::registry::{exec_ctx, own_registered, tool_result, ToolBody, Tools};

const CREATE_DESC: &str = "Create a canvas: a self-contained HTML page the user sees live in a panel next to the chat (Dock desktop app). Use it for dashboards, charts, reports, tables, comparisons, interactive tools and UI prototypes — anything that reads better rendered than as markdown.\n\n\
The page runs in a sandboxed iframe: scripts and network (CDNs, fetch) work, but it cannot reach the user's files or Dock itself. Bundled offline libraries: `<script src=\"dock:lib/chart.js\"></script>` (Chart.js 4, UMD global `Chart`).\n\n\
Put the numbers / rows the page shows in `data`, not inline in the HTML, and render from `window.dock.data`. Then later updates go through `canvas_data` without rewriting the page. Bridge:\n\
- `window.dock.data` — current data (null if none)\n\
- `dock.onData(fn)` — called with the new data whenever it changes; re-render there\n\
- `dock.setData(obj)` — persist edits the user made inside the page\n\
- `dock.send(text)` — put a message for you into the user's input box (the user still presses send)\n\n\
Uncaught errors in the page are shown to the user with a button to send them back to you. Returns the canvas id (`canvas-N`).";

const CREATE_PARAMS: &str = r#"{"type":"object","properties":{"title":{"type":"string","description":"Short title shown on the canvas card and tab."},"html":{"type":"string","description":"Complete HTML document (<!doctype html> … </html>). Inline CSS/JS; keep data in `data`."},"data":{"description":"Optional JSON the page renders from (`window.dock.data`). Stored as data.json."}},"required":["title","html"]}"#;

const EDIT_DESC: &str = "Change a canvas's HTML. Each call saves a new version; the user can view and roll back old ones. Prefer a targeted patch: `old_string` must appear exactly once in the latest version and is replaced by `new_string`. Pass `html` instead to rewrite the whole page. To change only the numbers, use `canvas_data`.";

const EDIT_PARAMS: &str = r#"{"type":"object","properties":{"id":{"type":"string","description":"Canvas id, e.g. canvas-1."},"old_string":{"type":"string","description":"Exact text to replace; must occur exactly once."},"new_string":{"type":"string","description":"Replacement text."},"html":{"type":"string","description":"Full replacement document, instead of old_string/new_string."},"note":{"type":"string","description":"One short line describing the change, shown in version history."},"title":{"type":"string","description":"New title, if it should change."}},"required":["id","note"]}"#;

const DATA_DESC: &str = "Replace a canvas's data (`window.dock.data`). The open page receives it through `dock.onData` and re-renders in place; no new HTML version is created. Send the whole new value.";

const DATA_PARAMS: &str = r#"{"type":"object","properties":{"id":{"type":"string","description":"Canvas id, e.g. canvas-1."},"data":{"description":"The complete new data value."}},"required":["id","data"]}"#;

const READ_DESC: &str = "Read a canvas back: its HTML (latest version unless `version` is given), its data, and version history. Without `id`, lists this session's canvases. Data the user edited in the page (via `dock.setData`) shows up here.";

const READ_PARAMS: &str = r#"{"type":"object","properties":{"id":{"type":"string","description":"Canvas id. Omit to list all canvases of this session."},"version":{"type":"integer","description":"Version number; default latest."}}}"#;

pub fn tool_canvas() -> Plugin {
    plugin("tool-canvas", Inject::from([TOOLS]), |ctx, _: &()| {
        let tools = ctx.require::<Tools>(TOOLS)?;
        let mut handles = Vec::new();
        for (name, desc, params) in [
            ("canvas_create", CREATE_DESC, CREATE_PARAMS),
            ("canvas_edit", EDIT_DESC, EDIT_PARAMS),
            ("canvas_data", DATA_DESC, DATA_PARAMS),
            ("canvas_read", READ_DESC, READ_PARAMS),
        ] {
            let root_ctx = ctx.clone();
            let body: ToolBody = std::sync::Arc::new(move |call| {
                // 画布跟着调用者那一页走，不是注册时的根页。
                let ctx = exec_ctx().unwrap_or_else(|| root_ctx.clone());
                Box::pin(async move {
                    let Some(dir) = page_session_dir(&ctx) else {
                        return failed(call, "这个会话不落盘，没法存画布");
                    };
                    let shell = ToolCall {
                        arguments: String::new(),
                        ..call.clone()
                    };
                    tokio::task::spawn_blocking(move || run(&dir, call))
                        .await
                        .unwrap_or_else(|e| failed(shell, format!("画布工具没跑完：{e}")))
                })
            });
            // 内置预设把它设成按需（`on_demand_tools`）：画布只有桌面端渲染。GUI 按 `use_tool`
            // 的内层工具名认画布项（`calledTool`），经不经 `use_tool` 都接得住。
            handles.push(tools.register(
                ToolSpec {
                    name: name.into(),
                    description: desc.into(),
                    parameters_json: params.into(),
                },
                body,
            )?);
        }
        own_registered(ctx, handles)?;
        Ok(None)
    })
}

/// 调用者那一页的会话目录。子代理经 `"planMode"` 找到父页。
fn page_session_dir(ctx: &Context) -> Option<PathBuf> {
    let sessions = ctx
        .get::<PlanMode>(PLAN_MODE)
        .and_then(|plan| plan.page_sessions())
        .or_else(|| ctx.get::<Sessions>(SESSIONS))?;
    sessions.disk_session_dir()
}

fn failed(call: ToolCall, msg: impl std::fmt::Display) -> ToolResult {
    ToolResult {
        is_error: true,
        ..tool_result(call, format!("Error: {msg}"))
    }
}

#[derive(Deserialize)]
struct CreateArgs {
    #[serde(default)]
    title: String,
    html: String,
    #[serde(default)]
    data: Option<Value>,
}

#[derive(Deserialize)]
struct EditArgs {
    id: String,
    #[serde(default)]
    old_string: Option<String>,
    #[serde(default)]
    new_string: Option<String>,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    note: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct DataArgs {
    id: String,
    data: Value,
}

#[derive(Deserialize)]
struct ReadArgs {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    version: Option<u32>,
}

fn parse<T: for<'de> Deserialize<'de>>(call: &ToolCall) -> Result<T, String> {
    serde_json::from_str(&call.arguments).map_err(|e| format!("参数不对（{e}）"))
}

fn run(dir: &std::path::Path, call: ToolCall) -> ToolResult {
    let out = match call.name.as_str() {
        "canvas_create" => parse::<CreateArgs>(&call).and_then(|a| {
            let meta = canvas::create(dir, &a.title, &a.html, a.data.as_ref())?;
            Ok(format!(
                "已创建画布 {}「{}」v1，用户在会话旁的画布面板里看得到。",
                meta.id, meta.title
            ))
        }),
        "canvas_edit" => parse::<EditArgs>(&call).and_then(|a| {
            let edit = match (&a.html, &a.old_string, &a.new_string) {
                (Some(html), None, None) => Edit::Rewrite { html },
                (None, Some(old), Some(new)) => Edit::Replace { old, new },
                _ => return Err("要么给 old_string + new_string，要么只给 html".into()),
            };
            let meta = canvas::edit(dir, &a.id, edit, &a.note, a.title.as_deref())?;
            Ok(format!("画布 {} 已更新到 v{}。", meta.id, meta.latest()))
        }),
        "canvas_data" => parse::<DataArgs>(&call).and_then(|a| {
            let meta = canvas::set_data(dir, &a.id, &a.data)?;
            Ok(format!(
                "画布 {} 的数据已更新（仍是 v{}），打开的页面会就地重绘。",
                meta.id,
                meta.latest()
            ))
        }),
        "canvas_read" => parse::<ReadArgs>(&call).and_then(|a| match a.id {
            None => Ok(listing(&canvas::list(dir))),
            Some(id) => {
                let c = canvas::read(dir, &id, a.version)?;
                let data = serde_json::to_string(&c.data).unwrap_or_default();
                Ok(format!(
                    "{}\n\n--- v{} HTML ---\n{}\n\n--- data ---\n{}",
                    header(&c.meta),
                    c.version,
                    c.html,
                    data
                ))
            }
        }),
        other => Err(format!("未知的画布工具：{other}")),
    };
    match out {
        Ok(text) => tool_result(call, text),
        Err(msg) => failed(call, msg),
    }
}

fn header(meta: &Meta) -> String {
    let mut out = format!(
        "{}「{}」最新 v{}，版本：",
        meta.id,
        meta.title,
        meta.latest()
    );
    let versions: Vec<String> = meta
        .versions
        .iter()
        .map(|v| format!("v{} {}", v.n, v.note))
        .collect();
    out.push_str(&versions.join("；"));
    out
}

fn listing(all: &[Meta]) -> String {
    if all.is_empty() {
        return "这个会话还没有画布。".into();
    }
    all.iter().map(header).collect::<Vec<_>>().join("\n")
}
