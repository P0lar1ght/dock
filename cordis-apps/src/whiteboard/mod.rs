//! 内置白板（`dock mcp whiteboard`）：Excalidraw 界面 + 按项目存的场景。
//!
//! - 模型用 `whiteboard_read`（读，不挂界面）、`whiteboard_edit`（改，挂界面 `ui://whiteboard/board.html`）；
//! - 界面用只给界面的 `whiteboard_load` / `whiteboard_save` 读写同一份场景；
//! - 场景按 Dock 带来的 `_meta["dock/cwd"]` 分项目：`$DOCK_HOME/apps/whiteboard/<项目目录名>/scene.json`。

pub mod scene;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::stdio::{tool_result, App, BoxFuture};
use scene::Scene;

/// Dock 里的服务器键名：公名 `mcp_whiteboard__*`。
pub const SERVER_NAME: &str = "whiteboard";
pub const RESOURCE_URI: &str = "ui://whiteboard/board.html";
const MIME: &str = "text/html;profile=mcp-app";
/// Dock 带来的调用方工作目录（见 cordis-spine `protocol::META_CWD`）。
const META_CWD: &str = "dock/cwd";

/// 构建好的界面（`cordis-apps/web/whiteboard`，`npm run build` 生成）。
const BOARD_HTML: &str = include_str!("../../assets/whiteboard.html");

/// 中文手写字体（Xiaolai，12 MB）不打进界面：联网时从这里取，离线退回系统字体。
const FONT_CDN: &str = "https://esm.sh";

const READ_DESC: &str = "Read the project's whiteboard: every shape with its id, type, label, position, size, colors, and arrow endpoints. The whiteboard is shared with the user, who may have edited it since you last looked — read before editing.";

const EDIT_DESC: &str = "Edit the project's whiteboard (an Excalidraw canvas the user sees and edits too). In one call: `clear` everything, `delete` shapes by id, `update` shapes ({id, x?, y?, width?, height?, text?, stroke?, fill?, strokeStyle?}), and `add` shapes ({type: rectangle|ellipse|diamond|text|arrow|line, id?, x?, y?, width?, height?, text?, stroke?, fill?, strokeStyle?, from?, to?, fontSize?}). `text` is the label inside a shape or on an arrow, or the content of a text shape. Arrows connect shapes with `from`/`to` ids. Omit x/y to lay new shapes out in a column next to the existing content. Colors are #rrggbb. Give your own short ids to shapes you will connect or update later. The whole edit is rejected if any part is invalid.";

fn shape_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "type": { "type": "string", "enum": ["rectangle", "ellipse", "diamond", "text", "arrow", "line"] },
            "x": { "type": "number" }, "y": { "type": "number" },
            "width": { "type": "number" }, "height": { "type": "number" },
            "text": { "type": "string" },
            "stroke": { "type": "string", "description": "Line / text color, #rrggbb." },
            "fill": { "type": "string", "description": "Fill color, #rrggbb or transparent." },
            "strokeStyle": { "type": "string", "enum": ["solid", "dashed", "dotted"] },
            "from": { "type": "string", "description": "Arrow / line start shape id." },
            "to": { "type": "string", "description": "Arrow / line end shape id." },
            "fontSize": { "type": "number" }
        }
    })
}

pub struct Whiteboard {
    root: PathBuf,
    /// 读改写串在一起：模型的编辑和界面的保存会撞。
    lock: Arc<Mutex<()>>,
}

impl Whiteboard {
    /// 场景存在 `root/<项目目录名>/scene.json`。
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            lock: Arc::default(),
        }
    }

    /// 默认位置：`$DOCK_HOME/apps/whiteboard`。
    pub fn in_dock_home() -> Self {
        Self::new(
            cordis_base::config::dock_home()
                .join("apps")
                .join(SERVER_NAME),
        )
    }

    fn run(&self, name: &str, args: &Value, cwd: Option<&str>) -> Value {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let path = scene::scene_path(&self.root, cwd);
        let mut scene = Scene::load(&path);
        match name {
            "whiteboard_read" => {
                let (text, data) = scene::summary(&scene);
                tool_result(text, Some(data), false)
            }
            "whiteboard_edit" => match scene::apply_edit(&mut scene, args) {
                Ok(report) => {
                    if let Err(e) = scene.save(&path) {
                        return tool_result(e, None, true);
                    }
                    let (summary, data) = scene::summary(&scene);
                    let mut head = format!(
                        "已改白板：新增 {}、修改 {}、删除 {}",
                        report.added.len(),
                        report.updated.len(),
                        report.deleted.len()
                    );
                    if report.cleared {
                        head.push_str("（先清空了）");
                    }
                    tool_result(
                        format!("{head}。\n{summary}"),
                        Some(json!({
                            "rev": scene.rev,
                            "added": report.added, "updated": report.updated, "deleted": report.deleted,
                            "shapes": data["shapes"],
                        })),
                        false,
                    )
                }
                Err(e) => tool_result(format!("白板没改：{e}"), None, true),
            },
            "whiteboard_load" => {
                let since = args.get("sinceRev").and_then(Value::as_u64);
                if since == Some(scene.rev) {
                    return tool_result(
                        "没有变化",
                        Some(json!({ "rev": scene.rev, "unchanged": true })),
                        false,
                    );
                }
                tool_result(format!("第 {} 版", scene.rev), Some(scene.to_json()), false)
            }
            "whiteboard_save" => match scene::merge_save(&mut scene, args) {
                Ok(changed) => {
                    if changed {
                        if let Err(e) = scene.save(&path) {
                            return tool_result(e, None, true);
                        }
                    }
                    tool_result(
                        format!("第 {} 版", scene.rev),
                        Some(json!({ "rev": scene.rev, "changed": changed })),
                        false,
                    )
                }
                Err(e) => tool_result(e, None, true),
            },
            other => tool_result(format!("没有工具 {other}"), None, true),
        }
    }
}

impl App for Whiteboard {
    fn name(&self) -> &'static str {
        "dock-whiteboard"
    }

    fn tools(&self) -> Vec<Value> {
        let app_only = json!({ "ui": { "resourceUri": RESOURCE_URI, "visibility": ["app"] } });
        vec![
            json!({
                "name": "whiteboard_read",
                "description": READ_DESC,
                "inputSchema": { "type": "object", "properties": {} },
                "annotations": { "readOnlyHint": true },
            }),
            json!({
                "name": "whiteboard_edit",
                "description": EDIT_DESC,
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "add": { "type": "array", "items": shape_schema() },
                        "update": { "type": "array", "items": shape_schema() },
                        "delete": { "type": "array", "items": { "type": "string" } },
                        "clear": { "type": "boolean" }
                    }
                },
                "_meta": { "ui": { "resourceUri": RESOURCE_URI } },
            }),
            json!({
                "name": "whiteboard_load",
                "description": "Load the whole scene (for the whiteboard UI).",
                "inputSchema": { "type": "object", "properties": { "sinceRev": { "type": "integer" } } },
                "_meta": app_only,
            }),
            json!({
                "name": "whiteboard_save",
                "description": "Save elements edited in the whiteboard UI (merged by element version).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "elements": { "type": "array" },
                        "consumed": { "type": "array", "items": { "type": "string" } },
                        "appState": { "type": "object" },
                        "files": { "type": "object" }
                    }
                },
                "_meta": app_only,
            }),
        ]
    }

    fn resources(&self) -> Vec<Value> {
        vec![json!({
            "uri": RESOURCE_URI,
            "name": "whiteboard",
            "title": "白板",
            "mimeType": MIME,
        })]
    }

    fn read(&self, uri: &str) -> Option<Value> {
        (uri == RESOURCE_URI).then(|| {
            json!({ "contents": [{
                "uri": RESOURCE_URI,
                "mimeType": MIME,
                "text": BOARD_HTML,
                "_meta": { "ui": {
                    "csp": { "resourceDomains": [FONT_CDN], "connectDomains": [FONT_CDN] },
                    "permissions": { "clipboardWrite": {} },
                    "prefersBorder": true,
                } },
            }] })
        })
    }

    fn call(self: Arc<Self>, name: String, args: Value, meta: Value) -> BoxFuture<Value> {
        Box::pin(async move {
            let cwd = meta
                .get(META_CWD)
                .and_then(Value::as_str)
                .map(str::to_string);
            let me = self.clone();
            tokio::task::spawn_blocking(move || me.run(&name, &args, cwd.as_deref()))
                .await
                .unwrap_or_else(|e| tool_result(format!("白板工具没跑完：{e}"), None, true))
        })
    }
}

/// 在进程自己的 stdin / stdout 上跑（`dock mcp whiteboard`）。
pub async fn serve_stdio() -> std::io::Result<()> {
    crate::stdio::serve_stdio(Arc::new(Whiteboard::in_dock_home())).await
}
