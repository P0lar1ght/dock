//! 内置白板（`dock mcp whiteboard`）：Excalidraw 界面 + 按项目存的场景。
//!
//! - 模型用 `whiteboard_read`（读，不挂界面）、`whiteboard_edit`（改，挂界面 `ui://whiteboard/board.html`），
//!   以及 `whiteboard_group` / `align` / `style` / `layer` / `fit_text`（布局与样式，避免塞进 edit 撑爆 schema）；
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

const READ_DESC: &str = "Read the project's whiteboard: every shape with id/type/label/position/size/colors/arrow endpoints, plus reusable `patches` you can copy into whiteboard_edit update/add. The board is shared with the user — read before editing.";

const EDIT_DESC: &str = concat!(
    "Edit the project's whiteboard (Excalidraw; user sees it too). One call: `clear`?, `delete`? (id strings), ",
    "`update`? (objects), `add`? (objects). Whole edit is atomic.\n\n",
    "`add`/`update` MUST be flat arrays of objects — NOT {\"item\": [...]}. ",
    "Example: \"add\": [{\"type\":\"rectangle\",\"x\":0,\"y\":0}].\n\n",
    "Shape fields (validated server-side): `type` ∈ rectangle|ellipse|diamond|text|arrow|line; `id`?; ",
    "`x`/`y`/`width`/`height`?; `text`? (label inside shape / on arrow / text content); ",
    "`stroke`? / `fill`? (#rrggbb or transparent); `strokeStyle`? ∈ solid|dashed|dotted; ",
    "`from`/`to`? (arrow/line endpoint shape ids); `fontSize`? (text shapes). ",
    "Omit x/y to stack new shapes in a column to the right of existing content. ",
    "Give short ids to shapes you will connect or update later.\n\n",
    "For group/align/batch-style/layer/fit-text use the dedicated whiteboard_* tools — do not stuff them into this schema."
);

/// 故意不列 properties：嵌套一长串会触发部分模型把数组包成 `{item: ...}`。
fn shape_item_schema() -> Value {
    json!({ "type": "object", "additionalProperties": true })
}

fn ids_schema() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
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
        // 读改写整段拿文件锁：终端和桌面端各拉起一个白板服务，会同时写同一个项目的场景。
        let _file = match lock_scene(&path) {
            Ok(f) => f,
            Err(e) => return tool_result(e, None, true),
        };
        let (scene, note) = match Scene::load(&path) {
            Ok(loaded) => loaded,
            Err(e) => return tool_result(e, None, true),
        };
        let mut out = apply(name, args, &path, scene);
        if let Some(note) = note {
            if let Some(text) = out.pointer_mut("/content/0/text") {
                *text = json!(format!("{note}\n{}", text.as_str().unwrap_or_default()));
            }
        }
        out
    }
}

fn save_ok(scene: &mut Scene, path: &std::path::Path, head: String) -> Value {
    scene.gc_files();
    if let Err(e) = scene.save(path) {
        return tool_result(e, None, true);
    }
    let (summary, data) = scene::summary(scene);
    tool_result(
        format!("{head}\n{summary}"),
        Some(json!({
            "rev": scene.rev,
            "shapes": data["shapes"],
            "patches": data["patches"],
        })),
        false,
    )
}

fn arrange_ok(scene: &mut Scene, path: &std::path::Path, report: scene::ArrangeReport) -> Value {
    let head = format!("已改白板：{}。", report.detail);
    let mut out = save_ok(scene, path, head);
    if let Some(sc) = out.get_mut("structuredContent") {
        sc["touched"] = json!(report.touched);
    }
    out
}

/// 一次工具调用：读 / 改 / 存都在调用方拿着锁的时候做。
fn apply(name: &str, args: &Value, path: &std::path::Path, mut scene: Scene) -> Value {
    match name {
        "whiteboard_read" => {
            let (text, data) = scene::summary(&scene);
            tool_result(text, Some(data), false)
        }
        "whiteboard_edit" => match scene::apply_edit(&mut scene, args) {
            Ok(report) => {
                let mut head = format!(
                    "已改白板：新增 {}、修改 {}、删除 {}",
                    report.added.len(),
                    report.updated.len(),
                    report.deleted.len()
                );
                if report.cleared {
                    head.push_str("（先清空了）");
                }
                if report.unwrapped_item {
                    head.push_str("。已自动去掉 item 包装层");
                }
                head.push('。');
                let mut out = save_ok(&mut scene, path, head);
                if let Some(sc) = out.get_mut("structuredContent") {
                    sc["added"] = json!(report.added);
                    sc["updated"] = json!(report.updated);
                    sc["deleted"] = json!(report.deleted);
                    if report.unwrapped_item {
                        sc["unwrappedItem"] = json!(true);
                    }
                }
                out
            }
            Err(e) => tool_result(format!("白板没改：{e}"), None, true),
        },
        "whiteboard_group" => match scene::apply_group(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => tool_result(format!("白板没改：{e}"), None, true),
        },
        "whiteboard_align" => match scene::apply_align(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => tool_result(format!("白板没改：{e}"), None, true),
        },
        "whiteboard_style" => match scene::apply_style(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => tool_result(format!("白板没改：{e}"), None, true),
        },
        "whiteboard_layer" => match scene::apply_layer(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => tool_result(format!("白板没改：{e}"), None, true),
        },
        "whiteboard_fit_text" => match scene::apply_fit_text(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
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
                    scene.gc_files();
                    if let Err(e) = scene.save(path) {
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

/// 场景目录里的 `scene.lock`：拿到排他锁才读改写，放手（drop）就解锁。
fn lock_scene(path: &std::path::Path) -> Result<std::fs::File, String> {
    let dir = path.parent().ok_or("白板路径不对")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("建白板目录失败：{e}"))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("scene.lock"))
        .map_err(|e| format!("打开白板锁失败：{e}"))?;
    file.lock().map_err(|e| format!("锁白板失败：{e}"))?;
    Ok(file)
}

impl App for Whiteboard {
    fn name(&self) -> &'static str {
        "dock-whiteboard"
    }

    fn tools(&self) -> Vec<Value> {
        let app_only = json!({ "ui": { "resourceUri": RESOURCE_URI, "visibility": ["app"] } });
        let ui = || json!({ "ui": { "resourceUri": RESOURCE_URI } });
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
                        "add": { "type": "array", "items": shape_item_schema() },
                        "update": { "type": "array", "items": shape_item_schema() },
                        "delete": { "type": "array", "items": { "type": "string" } },
                        "clear": { "type": "boolean" }
                    }
                },
                "_meta": ui(),
            }),
            json!({
                "name": "whiteboard_group",
                "description": "Group shapes together or ungroup them. Pass flat `ids` string array (NOT {\"item\":...}). `ungroup: true` clears groupIds; otherwise needs ≥2 ids.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ids": ids_schema(),
                        "ungroup": { "type": "boolean" }
                    },
                    "required": ["ids"]
                },
                "_meta": ui(),
            }),
            json!({
                "name": "whiteboard_align",
                "description": "Align ≥2 shapes. `align` ∈ left|right|top|bottom|center_x|center_y. `ids` must be a flat string array.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ids": ids_schema(),
                        "align": { "type": "string" }
                    },
                    "required": ["ids", "align"]
                },
                "_meta": ui(),
            }),
            json!({
                "name": "whiteboard_style",
                "description": "Batch-set stroke/fill/strokeStyle on many shapes. Flat args: `ids` plus optional `stroke` (#rrggbb), `fill` (#rrggbb|transparent), `strokeStyle` (solid|dashed|dotted).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ids": ids_schema(),
                        "stroke": { "type": "string" },
                        "fill": { "type": "string" },
                        "strokeStyle": { "type": "string" }
                    },
                    "required": ["ids"]
                },
                "_meta": ui(),
            }),
            json!({
                "name": "whiteboard_layer",
                "description": "Change z-order of shapes already drawn (not pending). `to` ∈ front|back|forward|backward. `ids` flat string array.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ids": ids_schema(),
                        "to": { "type": "string" }
                    },
                    "required": ["ids", "to"]
                },
                "_meta": ui(),
            }),
            json!({
                "name": "whiteboard_fit_text",
                "description": "Resize rectangle/ellipse/diamond/text boxes to fit their label/text. Diamond height is raised only (lower bound). `ids` flat string array.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ids": ids_schema()
                    },
                    "required": ["ids"]
                },
                "_meta": ui(),
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
                    // 字体走 @font-face（font-src），不用放开 connect-src。
                    "csp": { "resourceDomains": [FONT_CDN] },
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
