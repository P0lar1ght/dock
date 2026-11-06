//! 内置白板（`dock mcp whiteboard`）：Excalidraw 界面 + 按项目存的场景。
//!
//! - 模型用 `whiteboard_read`（读，不挂界面）、`whiteboard_edit`（改，挂界面 `ui://whiteboard/board.html`），
//!   以及 group/align/style/layer/fit_text/distribute/layout/place/graph（布局与样式，避免塞进 edit 撑爆 schema）；
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

const READ_DESC: &str = "Read the project's whiteboard: every shape with id/type/label/position/size/colors/arrow endpoints, plus reusable `patches` you can copy into whiteboard_edit update/add. The board is shared with the user — read before editing.\n\nScope it on a big board: `overview` (just the totals and the board bounds), `ids` (only these shapes), `related` (this shape plus its container, label, arrow ends and group). Find shapes by meaning with whiteboard_find instead of guessing ids.\n\nTo SEE what the board actually looks like (the real rendering, not coordinates), use the app tool `app_whiteboard__snapshot` — calling any whiteboard tool opens the board panel, after which the snapshot tool is available.";

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
    "For layout (group/align/distribute/layout/place/graph/style/layer/fit-text) use the dedicated whiteboard_* tools — do not stuff them into this schema.\n\nTo find shapes by meaning use whiteboard_find; to copy a shape (with its label/group) use whiteboard_duplicate."
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

/// 存盘 + 回结果。正文只讲这次改了什么：全量场景（几十行 + shapes + patches）交给
/// `whiteboard_read` —— 每次工具都回全量，几轮就把上下文吃满了。
fn save_ok(
    scene: &mut Scene,
    path: &std::path::Path,
    head: String,
    detail: String,
    extra: Option<Value>,
) -> Value {
    scene.gc_files();
    if let Err(e) = scene.save(path) {
        return tool_result(e, None, true);
    }
    let mut sc = json!({ "rev": scene.rev });
    if let Some(Value::Object(m)) = extra {
        for (k, v) in m {
            sc[k] = v;
        }
    }
    let text = if detail.is_empty() {
        head
    } else {
        format!("{head}\n{detail}")
    };
    tool_result(text, Some(sc), false)
}

fn arrange_ok(scene: &mut Scene, path: &std::path::Path, report: scene::ArrangeReport) -> Value {
    let head = format!("已改白板：{}。", report.detail);
    let detail = scene::lines_for(scene, &report.touched);
    save_ok(
        scene,
        path,
        head,
        detail,
        Some(json!({ "touched": report.touched })),
    )
}

/// 这次调用实际收到的参数（键 + 类型）：模型能一眼看出哪个参数没到服务端。
fn args_hint(args: &Value) -> String {
    let Some(m) = args.as_object() else {
        return String::new();
    };
    m.iter()
        .map(|(k, v)| {
            let t = match v {
                Value::Array(_) => "array",
                Value::Object(_) => "object",
                Value::String(_) => "string",
                Value::Number(_) => "number",
                Value::Bool(_) => "bool",
                Value::Null => "null",
            };
            format!("{k}={t}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 工具没改成的结果：带上「实际收到什么」。
fn not_changed(e: String, args: &Value) -> Value {
    let hint = args_hint(args);
    let text = if hint.is_empty() {
        format!("白板没改：{e}")
    } else {
        format!("白板没改：{e}（这次收到：{hint}）")
    };
    tool_result(text, None, true)
}

/// 工具没读成的结果（find / read）。
fn not_found(e: String, args: &Value) -> Value {
    let hint = args_hint(args);
    let text = if hint.is_empty() {
        format!("没找到：{e}")
    } else {
        format!("没找到：{e}（这次收到：{hint}）")
    };
    tool_result(text, None, true)
}

/// 一次工具调用：读 / 改 / 存都在调用方拿着锁的时候做。
fn apply(name: &str, args: &Value, path: &std::path::Path, mut scene: Scene) -> Value {
    match name {
        "whiteboard_read" => {
            // 分层读：整板 / 总览 / 只看这几个 / 某个图形的相关。
            let scope = match scene::read_scope(args) {
                Ok(s) => s,
                Err(e) => return not_found(format!("白板没读：{e}"), args),
            };
            let (text, data) = match scope {
                scene::ReadScope::All => scene::summary(&scene),
                scene::ReadScope::Overview => scene::summary_with(&scene, None, false),
                scene::ReadScope::Ids(ids) => scene::summary_with(&scene, Some(&ids), true),
                scene::ReadScope::Related(id) => {
                    let ids = scene::related_ids(&scene, &id);
                    scene::summary_with(&scene, Some(&ids), true)
                }
            };
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
                if !report.ignored.is_empty() {
                    head.push_str(&format!(
                        "忽略了不认识的字段：{}（可用：type/id/x/y/width/height/text/stroke/fill/strokeStyle/from/to/fontSize）",
                        report.ignored.join("、")
                    ));
                }
                let mut ids = report.added.clone();
                ids.extend(report.updated.clone());
                let detail = scene::lines_for(&scene, &ids);
                let mut extra = json!({
                    "added": report.added,
                    "updated": report.updated,
                    "deleted": report.deleted,
                    "unwrappedItem": report.unwrapped_item,
                });
                if !report.ignored.is_empty() {
                    extra["ignored"] = json!(report.ignored);
                }
                save_ok(&mut scene, path, head, detail, Some(extra))
            }
            Err(e) => not_changed(e, args),
        },
        "whiteboard_group" => match scene::apply_group(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => not_changed(e, args),
        },
        "whiteboard_align" => match scene::apply_align(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => not_changed(e, args),
        },
        "whiteboard_style" => match scene::apply_style(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => not_changed(e, args),
        },
        "whiteboard_layer" => match scene::apply_layer(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => not_changed(e, args),
        },
        "whiteboard_find" => match scene::apply_find(&mut scene, args) {
            Ok(r) => tool_result(r.detail, Some(json!({ "found": r.found })), false),
            Err(e) => not_found(e, args),
        },
        "whiteboard_duplicate" => match scene::apply_duplicate(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => not_changed(e, args),
        },
        "whiteboard_fit_text" => match scene::apply_fit_text(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => not_changed(e, args),
        },
        "whiteboard_distribute" => match scene::apply_distribute(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => not_changed(e, args),
        },
        "whiteboard_layout" => match scene::apply_layout(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => not_changed(e, args),
        },
        "whiteboard_place" => match scene::apply_place(&mut scene, args) {
            Ok(r) => arrange_ok(&mut scene, path, r),
            Err(e) => not_changed(e, args),
        },
        "whiteboard_graph" => match scene::apply_graph(&mut scene, args) {
            Ok(report) => {
                let mut head = format!("已改白板：{}", report.detail);
                if report.unwrapped_item {
                    head.push_str("。已自动去掉 item 包装层");
                }
                head.push('。');
                let detail = scene::lines_for(&scene, &report.added);
                save_ok(
                    &mut scene,
                    path,
                    head,
                    detail,
                    Some(json!({ "added": report.added })),
                )
            }
            Err(e) => not_changed(e, args),
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
                "name": "whiteboard_distribute",
                "description": "Evenly distribute ≥3 shapes. `direction` ∈ horizontal|vertical: keep outermost positions, space the middle ones evenly. `ids` flat string array (order ignored; sorted by current position).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ids": ids_schema(),
                        "direction": { "type": "string" }
                    },
                    "required": ["ids", "direction"]
                },
                "_meta": ui(),
            }),
            json!({
                "name": "whiteboard_layout",
                "description": "Arrange existing shapes into a grid. `columns` (required), optional `gap_x`/`gap_y` (defaults 60/80). Starts at the selection's top-left; keeps each shape's size. `ids` flat string array.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ids": ids_schema(),
                        "columns": { "type": "integer" },
                        "gap_x": { "type": "number" },
                        "gap_y": { "type": "number" }
                    },
                    "required": ["ids", "columns"]
                },
                "_meta": ui(),
            }),
            json!({
                "name": "whiteboard_place",
                "description": "Place one existing shape next to another. Args: `id` (move), `ref` (anchor), `where` ∈ left|right|above|below, optional `gap` (default 40). Works for pending and completed shapes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "ref": { "type": "string" },
                        "where": { "type": "string" },
                        "gap": { "type": "number" }
                    },
                    "required": ["id", "ref", "where"]
                },
                "_meta": ui(),
            }),
            json!({
                "name": "whiteboard_graph",
                "description": "Draw a flowchart from nodes+edges without guessing coordinates. Args: `nodes` [{id, text?, type?}] (type default rectangle; also ellipse|diamond|text), `edges`? [{from, to, text?, id?}], `layout`? ∈ layered|row|column|grid (default layered = BFS levels), `columns`? (grid default 3), `gap_x`?/`gap_y`? (default 60/80). Boxes default 200×80. Atomic; unwraps exactly-one-key {item:[...]}. NOT {\"item\":...} wrappers for nodes/edges arrays.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "nodes": { "type": "array", "items": shape_item_schema() },
                        "edges": { "type": "array", "items": shape_item_schema() },
                        "layout": { "type": "string" },
                        "columns": { "type": "integer" },
                        "gap_x": { "type": "number" },
                        "gap_y": { "type": "number" }
                    },
                    "required": ["nodes"]
                },
                "_meta": ui(),
            }),
            json!({
                "name": "whiteboard_find",
                "description": "Find shapes by what they say or look like, not by id: `queries` [{text?, type?, stroke?, fill?, near?, radius?}] — several per call, each ranked on its own. text matches the label/text (case-insensitive), type ∈ rectangle|ellipse|diamond|text|arrow|line, stroke/fill are colors (#1e1e1e), near = id of a shape to search around (radius default 300). Returns id/type/label/position plus what each shape is related to (container, group, arrow ends).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "queries": { "type": "array", "items": { "type": "object", "additionalProperties": true } },
                        "limit": { "type": "integer" }
                    },
                    "required": ["queries"]
                },
            }),
            json!({
                "name": "whiteboard_duplicate",
                "description": "Copy shapes with their whole subtree (labels, group, arrow bindings). `ids` flat string array — the shapes to copy, copied together keeping their relative positions; `count`? copies (default 1, max 20); `texts`? one new label per copy (array, length = count or 1); `place`? ∈ right|left|above|below (beside the original) or `dx`/`dy` offset (default 40). Returns the new ids in copy order — use them directly, no re-read needed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ids": ids_schema(),
                        "count": { "type": "integer" },
                        "texts": { "type": "array", "items": { "type": "string" } },
                        "place": { "type": "string" },
                        "dx": { "type": "number" },
                        "dy": { "type": "number" }
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
                        "files": { "type": "object" },
                        "order": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Current array order of the UI (= z-order). Send it only when the order changed.",
                        }
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
