//! 白板的数据：每个项目一份 `scene.json`（`$DOCK_HOME/apps/whiteboard/<项目目录名>/`）。
//!
//! ```text
//! { "type": "dock-whiteboard", "rev": 12,
//!   "elements": [ Excalidraw 元素… ],   界面画出来的完整元素；删掉的留墓碑（isDeleted）
//!   "pending":  [ 元素骨架… ],          Agent 加的、界面还没画成完整元素的（Excalidraw skeleton）
//!   "appState": { … }, "files": { … } }
//! ```
//!
//! - Agent 加的图形先进 `pending`：箭头连线、标签排版要 Excalidraw 自己算（`convertToExcalidrawElements`），
//!   界面一打开就把它们画成完整元素存回来（`consumed`）。界面没开（终端、无头）时照样读得到。
//! - 改已有的完整元素：直接改字段、`version + 1`；两边同时改按 Excalidraw 的规则合并
//!   （版本大的赢，同版本 `versionNonce` 小的赢）。
//! - 每次有变化 `rev + 1`，界面据此判断要不要重载。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

/// 整个场景文件的上限（含粘贴的图片）。
pub const MAX_SCENE_BYTES: usize = 24 * 1024 * 1024;
/// 一次 `edit` 最多加 / 改 / 删这么多个。
pub const MAX_OPS: usize = 300;

const SHAPES: &[&str] = &["rectangle", "ellipse", "diamond", "text", "arrow", "line"];
const DEFAULT_W: f64 = 180.0;
const DEFAULT_H: f64 = 80.0;
const GAP: f64 = 60.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub rev: u64,
    pub elements: Vec<Value>,
    pub pending: Vec<Value>,
    pub app_state: Value,
    pub files: Value,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            rev: 0,
            elements: Vec::new(),
            pending: Vec::new(),
            app_state: json!({}),
            files: json!({}),
        }
    }
}

impl Scene {
    /// 读场景。没有文件 = 空白板；读不了（权限、IO）报错，不能当空的再写回去；
    /// 内容坏了：原文件改名留着（`scene.json.corrupt-<毫秒>`），从空白板开始，回一句说明。
    pub fn load(path: &Path) -> Result<(Scene, Option<String>), String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Scene::default(), None))
            }
            Err(e) => return Err(format!("读白板失败：{e}")),
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => Ok((Scene::from_value(&v), None)),
            _ => {
                let aside = path.with_file_name(format!("scene.json.corrupt-{}", now_ms()));
                std::fs::rename(path, &aside)
                    .map_err(|e| format!("白板文件坏了，挪开也失败：{e}"))?;
                Ok((
                    Scene::default(),
                    Some(format!(
                        "原来的白板文件读不了，已另存为 {}，从空白板开始。",
                        aside.display()
                    )),
                ))
            }
        }
    }

    fn from_value(v: &Value) -> Scene {
        let list = |key: &str| {
            v.get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        Scene {
            rev: v.get("rev").and_then(Value::as_u64).unwrap_or(0),
            elements: list("elements"),
            pending: list("pending"),
            app_state: v.get("appState").cloned().unwrap_or_else(|| json!({})),
            files: v.get("files").cloned().unwrap_or_else(|| json!({})),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "type": "dock-whiteboard",
            "rev": self.rev,
            "elements": self.elements,
            "pending": self.pending,
            "appState": if self.app_state.is_object() { self.app_state.clone() } else { json!({}) },
            "files": if self.files.is_object() { self.files.clone() } else { json!({}) },
        })
    }

    /// 写临时文件再改名：写到一半崩了也不会留下半个场景。
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = self.to_json().to_string();
        if text.len() > MAX_SCENE_BYTES {
            return Err(format!(
                "白板太大（{} MB，上限 {} MB）：删掉些图片再试",
                text.len() / 1024 / 1024,
                MAX_SCENE_BYTES / 1024 / 1024
            ));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("建白板目录失败：{e}"))?;
        }
        // 临时文件带进程号：终端和桌面端各拉起一个白板服务时不会互踩（读改写本身由文件锁串起来）。
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, text).map_err(|e| format!("写白板失败：{e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("写白板失败：{e}"))
    }

    pub(crate) fn live(&self) -> impl Iterator<Item = &Value> {
        self.elements.iter().filter(|e| !is_deleted(e))
    }

    /// 丢掉没人用的图片：只留还在的图片元素引用的，和一天内删掉的（界面里撤销还要用）。
    /// 不然粘贴过又删掉的图片一直攒着，攒到上限连 Agent 的编辑都存不进去。
    pub fn gc_files(&mut self) {
        let Some(files) = self.files.as_object_mut() else {
            return;
        };
        let recent = now_ms().saturating_sub(24 * 60 * 60 * 1000);
        let used: std::collections::HashSet<&str> = self
            .elements
            .iter()
            .filter(|e| !is_deleted(e) || num(e, "updated") as u64 >= recent)
            .filter_map(|e| e.get("fileId").and_then(Value::as_str))
            .collect();
        files.retain(|id, _| used.contains(id.as_str()));
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.elements.iter().position(|e| id_of(e) == Some(id))
    }

    fn pending_index(&self, id: &str) -> Option<usize> {
        self.pending.iter().position(|e| id_of(e) == Some(id))
    }

    fn id_taken(&self, id: &str) -> bool {
        self.index_of(id).is_some() || self.pending_index(id).is_some()
    }

    /// 还在的图形（没删的完整元素，或骨架）。删掉的只剩墓碑，不算。
    fn shape(&self, id: &str) -> Option<&Value> {
        self.live()
            .find(|e| id_of(e) == Some(id))
            .or_else(|| self.pending.iter().find(|e| id_of(e) == Some(id)))
    }

    /// 现有内容的外框（完整元素 + 骨架）；空白板是 `None`。
    fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let mut b: Option<(f64, f64, f64, f64)> = None;
        for e in self.live().chain(self.pending.iter()) {
            let (x, y) = (num(e, "x"), num(e, "y"));
            let (w, h) = (num(e, "width").abs(), num(e, "height").abs());
            b = Some(match b {
                None => (x, y, x + w, y + h),
                Some((a, c, d, f)) => (a.min(x), c.min(y), d.max(x + w), f.max(y + h)),
            });
        }
        b
    }
}

// ---- 存放位置 ----

/// 项目的场景文件。`cwd` 来自 Dock 的 `_meta["dock/cwd"]`；别的客户端没带就归 `default`。
///
/// 目录名 = 会话目录那套名字 + 完整路径的短哈希：那套名字把非 ASCII 字符、`/` 都折成 `-`，
/// `…/SunGods-中台` 和 `…/SunGods`、`/a/b-c` 和 `/a/b/c` 会撞成一个；会话目录靠 `meta.cwd`
/// 还分得开，白板没有，所以带上哈希。
pub fn scene_path(root: &Path, cwd: Option<&str>) -> PathBuf {
    let key = match cwd.map(str::trim).filter(|c| !c.is_empty()) {
        Some(cwd) => format!(
            "{}-{:08x}",
            cordis_base::config::cwd_dirname(Path::new(cwd)),
            fnv1a(cwd.as_bytes()) as u32
        ),
        None => "default".into(),
    };
    root.join(key).join("scene.json")
}

/// FNV-1a：跨版本稳定（不用 `DefaultHasher`），只用来区分目录名。
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

// ---- Agent 的编辑 ----

/// 一次 `whiteboard_edit` 做了什么。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub deleted: Vec<String>,
    pub cleared: bool,
    /// 是否剥过恰好只有一个键 `item` 的包装。
    pub unwrapped_item: bool,
    /// 图形对象里不认识的字段名（模型多半是把 `text` 写成了 `label` 之类）。
    pub ignored: Vec<String>,
}

impl EditReport {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.deleted.is_empty() && !self.cleared
    }
}

/// 递归剥 `{"item": ...}` 包装：模型常把数组包一层，有时套两层（客户端还会再包一层）。
fn unwrap_item(v: &Value) -> (&Value, bool) {
    let mut cur = v;
    let mut stripped = false;
    for _ in 0..4 {
        let Some(m) = cur.as_object() else { break };
        if m.len() != 1 {
            break;
        }
        let Some(inner) = m.get("item") else { break };
        cur = inner;
        stripped = true;
    }
    (cur, stripped)
}

/// 这个对象像「数组里的一个元素」吗：id 列表是字符串集合；形状对象有 `type`/`id`。
fn looks_like_one(key: &str, v: &Value) -> bool {
    let Some(m) = v.as_object() else {
        return false;
    };
    if m.is_empty() {
        return false;
    }
    match key {
        "ids" | "delete" => m.values().all(|x| x.is_string()),
        "add" | "update" | "nodes" => m.contains_key("type") || m.contains_key("id"),
        "edges" => m.contains_key("from") || m.contains_key("to"),
        // 查询对象：{text?,type?,stroke?,fill?,near?,radius?}
        "queries" => {
            m.contains_key("text")
                || m.contains_key("type")
                || m.contains_key("stroke")
                || m.contains_key("fill")
                || m.contains_key("near")
        }
        _ => false,
    }
}

fn value_kind_hint(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let keys: Vec<String> = m.keys().map(|k| format!("\"{k}\"")).collect();
            format!("对象，键为 [{}]", keys.join(", "))
        }
        Value::Array(_) => "数组".into(),
        Value::String(_) => "字符串".into(),
        Value::Number(_) => "数字".into(),
        Value::Bool(_) => "布尔".into(),
        Value::Null => "null".into(),
    }
}

fn array_field_error(key: &str, got: &Value) -> String {
    let example = match key {
        "delete" => format!("\"{key}\": [\"shape-id\"]"),
        "update" => format!("\"{key}\": [{{\"id\":\"shape-id\",\"x\":0}}]"),
        _ => format!("\"{key}\": [{{\"type\":\"rectangle\",\"x\":0,\"y\":0}}]"),
    };
    format!(
        "{key} 要是数组（收到的是{}）。\n把数组直接放在 {key} 下，例如 {example}。\n如果这是 {{\"item\": [...]}}，去掉 item 这层包装。",
        value_kind_hint(got)
    )
}

/// 数组里又套了数组、或元素被包了壳：都摊平。
/// 模型端把 `[[a, b]]` 摊成 `[[a], [b]]`，甚至把单个元素包成 `{"item": a}`。
fn flatten_one(items: &[Value]) -> Vec<Value> {
    let mut out = Vec::with_capacity(items.len());
    for v in items {
        let (inner, _) = unwrap_item(v);
        match inner {
            Value::Array(nested) => out.extend(nested.iter().cloned()),
            other => out.push(other.clone()),
        }
    }
    out
}

/// 取数组字段；模型把数组包成 `{"item": [...]}`、把单个元素拍平成对象、或者套一层数组，都兜住。
fn take_array_field(args: &Value, key: &str) -> Result<(Vec<Value>, bool), String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok((Vec::new(), false)),
        Some(Value::Array(a)) => Ok((flatten_one(a), false)),
        Some(v) => {
            let (inner, unwrapped) = unwrap_item(v);
            match inner {
                Value::Array(a) => Ok((flatten_one(a), unwrapped)),
                // 单个元素被拍平成对象：当成一个元素的数组收下。
                Value::Object(_) if looks_like_one(key, inner) => {
                    Ok((vec![inner.clone()], unwrapped))
                }
                // 单个 id 写成字符串：也当一个元素。
                Value::String(s) if key == "ids" || key == "delete" => {
                    Ok((vec![Value::String(s.clone())], unwrapped))
                }
                other => Err(array_field_error(key, if unwrapped { other } else { v })),
            }
        }
    }
}

/// `{ add?, update?, delete?, clear? }`：先清空，再删、改、加。任何一条不合法整次不改。
pub fn apply_edit(scene: &mut Scene, args: &Value) -> Result<EditReport, String> {
    let (adds, u_add) = take_array_field(args, "add")?;
    let (updates, u_upd) = take_array_field(args, "update")?;
    let (deletes, u_del) = take_array_field(args, "delete")?;
    let clear = args.get("clear").and_then(bool_arg).unwrap_or(false);
    if adds.len() + updates.len() + deletes.len() > MAX_OPS {
        return Err(format!("一次最多改 {MAX_OPS} 个，分几次做"));
    }
    let mut next = scene.clone();
    let mut report = EditReport {
        unwrapped_item: u_add || u_upd || u_del,
        ..Default::default()
    };
    // 模型写错的字段名：记下来，结果里说一句（不然图形成了空框还没人发现）。
    for shape in adds.iter().chain(updates.iter()) {
        for f in unknown_fields(shape) {
            if !report.ignored.contains(&f) {
                report.ignored.push(f);
            }
        }
    }
    if clear {
        for e in next.elements.iter_mut().filter(|e| !is_deleted(e)) {
            tombstone(e);
        }
        next.pending.clear();
        report.cleared = true;
    }
    for (i, d) in deletes.iter().enumerate() {
        let id = d.as_str().ok_or_else(|| {
            format!(
                "第 {} 个删除项要是图形 id 字符串（delete[{}] 收到的是{}）",
                i + 1,
                i,
                value_kind_hint(d)
            )
        })?;
        delete_one(&mut next, id)
            .map_err(|e| format!("第 {} 个删除项：{e}（delete[{i}]）", i + 1))?;
        report.deleted.push(id.to_string());
    }
    for (i, u) in updates.iter().enumerate() {
        let id = u
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("第 {} 个修改缺 id（update[{}] 需要字符串 id）", i + 1, i))?
            .to_string();
        update_one(&mut next, &id, u)
            .map_err(|e| format!("第 {} 个修改：{e}（update[{i}]）", i + 1))?;
        report.updated.push(id);
    }
    for (i, a) in adds.iter().enumerate() {
        let id = add_one(&mut next, a, i)?;
        report.added.push(id);
    }
    if report.is_empty() {
        return Err("没有要改的：给 add / update / delete / clear 之一".into());
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(report)
}

fn add_one(scene: &mut Scene, shape: &Value, index: usize) -> Result<String, String> {
    let where_ = format!("add[{index}]");
    for key in ["x", "y", "width", "height"] {
        check_num(shape, key, &where_)?;
    }
    let types = SHAPES.join("|");
    let kind = shape.get("type").and_then(Value::as_str).ok_or_else(|| {
        format!(
            "第 {} 个图形缺 type（add[{index}] 需要 type ∈ {types}）",
            index + 1
        )
    })?;
    if !SHAPES.contains(&kind) {
        return Err(format!(
            "第 {} 个图形 type「{kind}」不认识（add[{index}] 需要 type ∈ {types}）",
            index + 1
        ));
    }
    // 删掉的图形只剩墓碑：同一个 id 可以再用（清空后重画很常见）。新元素的版本要比墓碑高，
    // 不然按版本合并时墓碑会赢、新画的又被删掉——记在 customData 里，界面画出来时照办。
    // 容器（框或箭头）上的标签（界面定死成 `<id>-label`，containerId 指向这个 id）也留着墓碑：
    // 一并让位，下限取两者里高的那个，界面给标签用同一个下限（不然重画后框在、字没了）。
    // 只清墓碑：还活着的孤儿文字不动。
    let mut min_version = None;
    let id = match shape.get("id").and_then(Value::as_str).map(str::trim) {
        Some(id) if !id.is_empty() => {
            if scene.shape(id).is_some() {
                return Err(format!(
                    "第 {} 个图形 id「{id}」已经有了（add[{index}]：换一个，或用 update 改它）",
                    index + 1
                ));
            }
            let mut top: Option<u64> = None;
            scene.elements.retain(|e| {
                let mine = id_of(e) == Some(id)
                    || (is_deleted(e) && e.get("containerId").and_then(Value::as_str) == Some(id));
                if mine {
                    top = top.max(Some(num(e, "version") as u64));
                }
                !mine
            });
            min_version = top.map(|v| v + 1);
            id.to_string()
        }
        _ => fresh_id(scene),
    };
    let linear = kind == "arrow" || kind == "line";
    let w = if kind == "text" {
        120.0
    } else {
        opt_num(shape, "width").unwrap_or(if linear { 160.0 } else { DEFAULT_W })
    };
    let h = if kind == "text" {
        30.0
    } else {
        opt_num(shape, "height").unwrap_or(if linear { 0.0 } else { DEFAULT_H })
    };
    // 没给位置：找一块空地（横着排开），别全堆成一列。
    let (x, y) = match (opt_num(shape, "x"), opt_num(shape, "y")) {
        (Some(x), Some(y)) => (x, y),
        (Some(x), None) => (x, default_y(scene)),
        (None, Some(y)) => (default_x(scene), y),
        (None, None) => free_slot(scene, w, h),
    };
    let mut s = Map::new();
    s.insert("type".into(), json!(kind));
    s.insert("id".into(), json!(id));
    s.insert("x".into(), json!(x));
    s.insert("y".into(), json!(y));
    if kind != "text" {
        s.insert("width".into(), json!(w));
        s.insert("height".into(), json!(h));
    }
    style_into(&mut s, shape)?;
    if let Some(text) = shape.get("text").and_then(Value::as_str) {
        if kind == "text" {
            s.insert("text".into(), json!(text));
        } else {
            s.insert("label".into(), json!({ "text": text }));
        }
    }
    if let Some(size) = opt_num(shape, "fontSize") {
        if kind == "text" {
            s.insert("fontSize".into(), json!(size));
        }
    }
    if linear {
        for (key, end) in [("from", "start"), ("to", "end")] {
            if let Some(target) = shape.get(key).and_then(Value::as_str) {
                if scene.shape(target).is_none() {
                    return Err(format!(
                        "第 {} 个图形的 {key}「{target}」不存在（add[{index}]）",
                        index + 1
                    ));
                }
                s.insert(end.into(), json!({ "id": target }));
            }
        }
        if let Some(points) = shape.get("points").and_then(Value::as_array) {
            s.insert("points".into(), json!(points));
        } else if opt_num(shape, "x").is_none() && opt_num(shape, "y").is_none() {
            // 两端都给了、没给位置：从一个图形的边连到另一个的边（Excalidraw 只绑定、不挪箭头）。
            let from = shape
                .get("from")
                .and_then(Value::as_str)
                .and_then(|t| scene.shape(t));
            let to = shape
                .get("to")
                .and_then(Value::as_str)
                .and_then(|t| scene.shape(t));
            if let (Some(a), Some(b)) = (from, to) {
                connect(&mut s, a, b);
            }
        }
    }
    if let Some(v) = min_version {
        s.insert("customData".into(), json!({ "dockMinVersion": v }));
    }
    scene.pending.push(Value::Object(s));
    Ok(id)
}

/// 图形的外框：`(中心 x, 中心 y, 半宽, 半高)`。
fn frame(e: &Value) -> (f64, f64, f64, f64) {
    let (w, h) = (num(e, "width").abs(), num(e, "height").abs());
    (
        num(e, "x") + w / 2.0,
        num(e, "y") + h / 2.0,
        w / 2.0,
        h / 2.0,
    )
}

/// 箭头从 `a` 的边指向 `b` 的边：两个中心连线与外框的交点，再各让出一点空隙。
fn connect(s: &mut Map<String, Value>, a: &Value, b: &Value) {
    const GAP: f64 = 8.0;
    let (ax, ay, aw, ah) = frame(a);
    let (bx, by, bw, bh) = frame(b);
    let (dx, dy) = (bx - ax, by - ay);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    // 沿方向走多远出外框：半宽 / |ux| 与半高 / |uy| 取小的。
    let exit = |hw: f64, hh: f64| {
        let tx = if ux.abs() > 1e-9 {
            hw / ux.abs()
        } else {
            f64::INFINITY
        };
        let ty = if uy.abs() > 1e-9 {
            hh / uy.abs()
        } else {
            f64::INFINITY
        };
        tx.min(ty)
    };
    let ta = exit(aw, ah) + GAP;
    let tb = exit(bw, bh) + GAP;
    // 两端太近、箭头塞不进两个框之间时：退到两点中间，别让它落回默认位置。
    let (ta, tb) = if ta + tb >= len {
        (len * 0.4, len * 0.4)
    } else {
        (ta, tb)
    };
    let (sx, sy) = (ax + ux * ta, ay + uy * ta);
    let (ex, ey) = (bx - ux * tb, by - uy * tb);
    s.insert("x".into(), json!(sx));
    s.insert("y".into(), json!(sy));
    s.insert("width".into(), json!((ex - sx).abs()));
    s.insert("height".into(), json!((ey - sy).abs()));
    s.insert("points".into(), json!([[0.0, 0.0], [ex - sx, ey - sy]]));
}

/// 颜色、线型：Agent 的叫法 → Excalidraw 字段。
fn style_into(s: &mut Map<String, Value>, shape: &Value) -> Result<(), String> {
    if let Some(c) = shape.get("stroke").and_then(Value::as_str) {
        s.insert("strokeColor".into(), json!(color(c)?));
    }
    if let Some(c) = shape.get("fill").and_then(Value::as_str) {
        let c = color(c)?;
        s.insert("backgroundColor".into(), json!(c));
        if c != "transparent" {
            s.insert("fillStyle".into(), json!("solid"));
        }
    }
    if let Some(style) = shape.get("strokeStyle").and_then(Value::as_str) {
        if !["solid", "dashed", "dotted"].contains(&style) {
            return Err(format!(
                "strokeStyle 只能是 solid / dashed / dotted，不是「{style}」"
            ));
        }
        s.insert("strokeStyle".into(), json!(style));
    }
    Ok(())
}

fn color(c: &str) -> Result<String, String> {
    let c = c.trim();
    let hex = c.strip_prefix('#').is_some_and(|h| {
        matches!(h.len(), 3 | 6 | 8) && h.chars().all(|ch| ch.is_ascii_hexdigit())
    });
    if hex || c == "transparent" || (c.len() <= 20 && c.chars().all(|ch| ch.is_ascii_alphabetic()))
    {
        Ok(c.to_string())
    } else {
        Err(format!(
            "颜色「{c}」不认识：用 #rrggbb、颜色名或 transparent"
        ))
    }
}

fn update_one(scene: &mut Scene, id: &str, patch: &Value) -> Result<(), String> {
    for key in ["x", "y", "width", "height"] {
        check_num(patch, key, &format!("update[{id}]"))?;
    }
    // 还没画出来的骨架：直接改骨架。
    if let Some(i) = scene.pending_index(id) {
        let mut s = scene.pending[i].as_object().cloned().unwrap_or_default();
        for key in ["x", "y", "width", "height"] {
            if let Some(n) = opt_num(patch, key) {
                s.insert(key.into(), json!(n));
            }
        }
        style_into(&mut s, patch)?;
        if let Some(text) = patch.get("text").and_then(Value::as_str) {
            if s.get("type").and_then(Value::as_str) == Some("text") {
                s.insert("text".into(), json!(text));
            } else {
                s.insert("label".into(), json!({ "text": text }));
            }
        }
        for (key, end) in [("from", "start"), ("to", "end")] {
            if let Some(target) = patch.get(key).and_then(Value::as_str) {
                s.insert(end.into(), json!({ "id": target }));
            }
        }
        scene.pending[i] = Value::Object(s);
        return Ok(());
    }
    let i = scene
        .index_of(id)
        .filter(|&i| !is_deleted(&scene.elements[i]))
        .ok_or_else(|| format!("没有图形「{id}」：先用 whiteboard_read 看现有的 id"))?;
    let before = scene.elements[i].clone();
    let mut e = before.clone();
    // 位置变了要重连箭头；只改了尺寸不用（箭头绑在容器上，界面会自己跟）。
    let mut moved = false;
    let mut resized = false;
    for key in ["x", "y"] {
        if let Some(n) = opt_num(patch, key) {
            if num(&e, key) != n {
                moved = true;
            }
            e[key] = json!(n);
        }
    }
    for key in ["width", "height"] {
        if let Some(n) = opt_num(patch, key) {
            if num(&e, key) != n {
                resized = true;
            }
            e[key] = json!(n);
        }
    }
    let mut style = Map::new();
    style_into(&mut style, patch)?;
    for (k, v) in style {
        e[k] = v;
    }
    let text = patch.get("text").and_then(Value::as_str);
    if let (Some(t), true) = (text, kind_of(&e) == "text") {
        e["text"] = json!(t);
        e["originalText"] = json!(t);
    }
    bump(&mut e);
    scene.elements[i] = e.clone();
    // 标签：容器里绑着的那段文字跟着改 / 跟着挪；没有就新绑一段。
    if kind_of(&e) != "text" {
        let label = scene.elements.iter().position(|t| {
            !is_deleted(t) && t.get("containerId").and_then(Value::as_str) == Some(id)
        });
        match (label, text) {
            (Some(j), t) => {
                let t_el = &mut scene.elements[j];
                if let Some(t) = t {
                    t_el["text"] = json!(t);
                    t_el["originalText"] = json!(t);
                }
                if moved || resized {
                    recenter(t_el, &e);
                }
                if t.is_some() || moved || resized {
                    bump(t_el);
                }
            }
            (None, Some(t)) if !t.is_empty() => {
                let text_id = format!("{id}-label");
                let text_id = if scene.id_taken(&text_id) {
                    fresh_id(scene)
                } else {
                    text_id
                };
                scene.elements.push(bound_text(&text_id, &e, t));
                let c = &mut scene.elements[i];
                let mut bound = c
                    .get("boundElements")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                bound.push(json!({ "type": "text", "id": text_id }));
                c["boundElements"] = json!(bound);
            }
            _ => {}
        }
    }
    // 挪了位置：连在它上面的箭头退回骨架，让界面按新位置重新连。
    if moved {
        reroute_arrows(scene, id);
    }
    Ok(())
}

/// 留墓碑。
///
/// 版本要**稳稳高于界面手里那份**：Excalidraw 合并时版本大的赢，可版本一样时它会保留
/// 本地的 —— 于是「已删的图形」留在画布上又冒出来（服务端已经是墓碑，界面还在显示）。
/// 这里跳一版（+1000）：界面再编辑也追不上，服务端的删除说了算。
fn tombstone(e: &mut Value) {
    e["isDeleted"] = json!(true);
    bump(e);
    e["version"] = json!(num(e, "version") as u64 + 1000);
}

fn delete_one(scene: &mut Scene, id: &str) -> Result<(), String> {
    if let Some(i) = scene.pending_index(id) {
        scene.pending.remove(i);
        return Ok(());
    }
    let i = scene
        .index_of(id)
        .filter(|&i| !is_deleted(&scene.elements[i]))
        .ok_or_else(|| format!("没有图形「{id}」：先用 whiteboard_read 看现有的 id"))?;
    tombstone(&mut scene.elements[i]);
    for t in scene.elements.iter_mut() {
        if !is_deleted(t) && t.get("containerId").and_then(Value::as_str) == Some(id) {
            tombstone(t);
        }
    }
    Ok(())
}

/// 绑在 `id` 上的完整箭头：退回骨架重新连线，**复用原来的箭头 id**（同颜色、同标签、同两端）。
fn reroute_arrows(scene: &mut Scene, id: &str) {
    let ends = |e: &Value, key: &str| {
        e.pointer(&format!("/{key}/elementId"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let arrows: Vec<usize> = scene
        .elements
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            !is_deleted(e)
                && kind_of(e) == "arrow"
                && (ends(e, "startBinding").as_deref() == Some(id)
                    || ends(e, "endBinding").as_deref() == Some(id))
        })
        .map(|(i, _)| i)
        .collect();
    for i in arrows {
        let old = scene.elements[i].clone();
        let old_id = id_of(&old).unwrap_or_default().to_string();
        let label = scene
            .elements
            .iter()
            .find(|t| {
                !is_deleted(t)
                    && t.get("containerId").and_then(Value::as_str) == Some(old_id.as_str())
            })
            .and_then(|t| t.get("text").and_then(Value::as_str).map(str::to_string));
        let from = ends(&old, "startBinding");
        let to = ends(&old, "endBinding");
        // 旧箭头留墓碑（不删）：墓碑版本比界面手里的旧版本高，界面把旧箭头存回来时按版本比较，
        // 旧的赢不了 → 不会出现「同一个 id 两个箭头」（一个墓碑外的旧元素 + 一个待画骨架）。
        let _ = delete_one(scene, &old_id);
        let mut top: Option<u64> = None;
        for e in scene.elements.iter() {
            let mine = id_of(e) == Some(old_id.as_str())
                || (is_deleted(e)
                    && e.get("containerId").and_then(Value::as_str) == Some(old_id.as_str()));
            if mine {
                top = top.max(Some(num(e, "version") as u64));
            }
        }
        // 墓碑再出一版：稳稳压过界面手里那个（用户动过就是旧版本 +1）。
        if let Some(i) = scene.index_of(&old_id) {
            let mut tomb = scene.elements[i].clone();
            bump(&mut tomb);
            scene.elements[i] = tomb;
            top = top.max(Some(num(&scene.elements[i], "version") as u64));
        }
        let mut s = Map::new();
        s.insert("type".into(), json!("arrow"));
        s.insert("id".into(), json!(old_id));
        s.insert("x".into(), json!(num(&old, "x")));
        s.insert("y".into(), json!(num(&old, "y")));
        for key in ["strokeColor", "strokeStyle", "strokeWidth"] {
            if let Some(v) = old.get(key) {
                s.insert(key.into(), v.clone());
            }
        }
        if let Some(ref from) = from {
            s.insert("start".into(), json!({ "id": from }));
        }
        if let Some(ref to) = to {
            s.insert("end".into(), json!({ "id": to }));
        }
        if let Some(text) = label {
            s.insert("label".into(), json!({ "text": text }));
        }
        if let Some(v) = top.map(|v| v + 1) {
            s.insert("customData".into(), json!({ "dockMinVersion": v }));
        }
        if let (Some(a), Some(b)) = (
            from.as_deref().and_then(|t| scene.shape(t)),
            to.as_deref().and_then(|t| scene.shape(t)),
        ) {
            let (a, b) = (a.clone(), b.clone());
            connect(&mut s, &a, &b);
        }
        scene.pending.push(Value::Object(s));
    }
}

fn recenter(text: &mut Value, container: &Value) {
    let (cx, cy) = (
        num(container, "x") + num(container, "width") / 2.0,
        num(container, "y") + num(container, "height") / 2.0,
    );
    text["x"] = json!(cx - num(text, "width") / 2.0);
    text["y"] = json!(cy - num(text, "height") / 2.0);
}

/// 新绑进容器的一段文字。尺寸是估的，界面载入时 Excalidraw 会重新量（`refreshDimensions`）。
fn bound_text(id: &str, container: &Value, text: &str) -> Value {
    let lines = text.lines().count().max(1) as f64;
    let width = text.lines().map(|l| l.chars().count()).max().unwrap_or(1) as f64 * 11.0;
    let height = lines * 25.0;
    let mut t = json!({
        "type": "text", "id": id,
        "x": 0.0, "y": 0.0, "width": width, "height": height, "angle": 0,
        "strokeColor": container.get("strokeColor").cloned().unwrap_or(json!("#1e1e1e")),
        "backgroundColor": "transparent", "fillStyle": "solid", "strokeWidth": 2,
        "strokeStyle": "solid", "roughness": 1, "opacity": 100, "groupIds": [],
        "frameId": null, "roundness": null, "seed": nonce(), "version": 1, "versionNonce": nonce(),
        "isDeleted": false, "boundElements": null, "updated": now_ms(), "link": null, "locked": false,
        "text": text, "originalText": text, "fontSize": 20, "fontFamily": 5,
        "textAlign": "center", "verticalAlign": "middle", "containerId": id_of(container),
        "autoResize": true, "lineHeight": 1.25,
    });
    recenter(&mut t, container);
    t
}

// ---- 界面存回来的 ----

/// 界面存回来的：`elements` 按版本合并，`consumed` 是它画好的骨架 id，`appState` / `files` 覆盖 / 合并。
/// 回有没有变。
pub fn merge_save(scene: &mut Scene, args: &Value) -> Result<bool, String> {
    let mut changed = false;
    if let Some(list) = args.get("elements").and_then(Value::as_array) {
        for incoming in list {
            // 最低限度的形状：有 id、type、数字版本。别的不认（缺字段会被当成版本 0 乱赢乱输）。
            let valid = incoming.get("type").and_then(Value::as_str).is_some()
                && incoming.get("version").and_then(Value::as_f64).is_some();
            let Some(id) = id_of(incoming).filter(|_| valid).map(str::to_string) else {
                continue;
            };
            match scene.index_of(&id) {
                None => {
                    scene.elements.push(incoming.clone());
                    changed = true;
                }
                Some(i) if wins(incoming, &scene.elements[i]) => {
                    scene.elements[i] = incoming.clone();
                    changed = true;
                }
                Some(_) => {}
            }
        }
    }
    // 界面给的数组顺序 = 渲染顺序（z 序）。只在界面顺序真的变了的时候才带过来。
    if let Some(list) = args.get("order").and_then(Value::as_array) {
        let ids: Vec<String> = list
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        if !ids.is_empty() && reorder_by_ids(scene, &ids) {
            changed = true;
        }
    }
    if let Some(ids) = args.get("consumed").and_then(Value::as_array) {
        let before = scene.pending.len();
        scene
            .pending
            .retain(|p| !ids.iter().any(|id| id.as_str() == id_of(p)));
        changed |= scene.pending.len() != before;
    }
    if let Some(state) = args.get("appState").filter(|s| s.is_object()) {
        if &scene.app_state != state {
            scene.app_state = state.clone();
            changed = true;
        }
    }
    if let Some(files) = args.get("files").and_then(Value::as_object) {
        if !scene.files.is_object() {
            scene.files = json!({});
        }
        let ours = scene.files.as_object_mut().expect("just ensured object");
        for (k, v) in files {
            if !ours.contains_key(k) {
                ours.insert(k.clone(), v.clone());
                changed = true;
            }
        }
    }
    if changed {
        scene.rev += 1;
    }
    Ok(changed)
}

/// 按界面给的 id 顺序重排未删元素（其余保持相对位置，排在后面）；返回有没有变。
fn reorder_by_ids(scene: &mut Scene, ids: &[String]) -> bool {
    let mut used = vec![false; scene.elements.len()];
    let mut picked: Vec<Value> = Vec::new();
    for id in ids {
        if let Some(i) = scene
            .elements
            .iter()
            .position(|e| id_of(e) == Some(id.as_str()))
        {
            if used[i] {
                continue;
            }
            used[i] = true;
            picked.push(scene.elements[i].clone());
        }
    }
    let mut rest_live: Vec<Value> = Vec::new();
    let mut tombs: Vec<Value> = Vec::new();
    for (i, e) in scene.elements.iter().enumerate() {
        if used[i] {
            continue;
        }
        if is_deleted(e) {
            tombs.push(e.clone());
        } else {
            rest_live.push(e.clone());
        }
    }
    let mut new_order: Vec<Value> = picked;
    new_order.extend(rest_live);
    new_order.extend(tombs);
    if new_order == scene.elements {
        return false;
    }
    scene.elements = new_order;
    true
}

/// Excalidraw 的合并规则：版本大的赢；同版本 `versionNonce` 小的赢（两边算出同一个结果）。
fn wins(incoming: &Value, ours: &Value) -> bool {
    let (vi, vo) = (num(incoming, "version"), num(ours, "version"));
    vi > vo || (vi == vo && num(incoming, "versionNonce") < num(ours, "versionNonce"))
}

// ---- 布局 / 样式（独立工具，避免塞进 edit 撑爆 schema）----

/// 一次布局类操作的结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArrangeReport {
    pub touched: Vec<String>,
    pub detail: String,
}

fn require_ids(args: &Value) -> Result<Vec<String>, String> {
    let (raw, _) = take_array_field(args, "ids")?;
    parse_ids(raw)
}

/// 一组 id：空、不是字符串、空串都不行。
fn parse_ids(raw: Vec<Value>) -> Result<Vec<String>, String> {
    if raw.is_empty() {
        return Err("ids 不能为空：给出要操作的图形 id 数组".into());
    }
    let mut ids = Vec::with_capacity(raw.len());
    for (i, v) in raw.iter().enumerate() {
        let id = v.as_str().ok_or_else(|| {
            format!(
                "第 {} 个 id 要是字符串（ids[{}] 收到的是{}）",
                i + 1,
                i,
                value_kind_hint(v)
            )
        })?;
        if id.is_empty() {
            return Err(format!("第 {} 个 id 是空的（ids[{i}]）", i + 1));
        }
        ids.push(id.to_string());
    }
    Ok(ids)
}

/// `whiteboard_read` 的读法：全量 / 总览 / 只看这几个 / 某个图形的相关。
#[derive(Clone, Debug, PartialEq)]
pub enum ReadScope {
    All,
    /// 只给总览（几个图形、画板多大），不逐个列。
    Overview,
    /// 只给这几个图形。
    Ids(Vec<String>),
    /// 给这个图形和它相关的（容器、标签、箭头两端、指向它的箭头、同组）。
    Related(String),
}

/// 解析 `whiteboard_read` 的读法参数。
pub fn read_scope(args: &Value) -> Result<ReadScope, String> {
    if args.get("overview").and_then(bool_arg).unwrap_or(false) {
        return Ok(ReadScope::Overview);
    }
    if let Some(rel) = args
        .get("related")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(ReadScope::Related(rel.to_string()));
    }
    if let Some(v) = args.get("ids") {
        let (raw, _) = take_array_field(&json!({ "ids": v.clone() }), "ids")?;
        return Ok(ReadScope::Ids(parse_ids(raw)?));
    }
    Ok(ReadScope::All)
}

/// 箭头 / 线的一端：完整元素看绑定，骨架看 `start` / `end`。
fn endpoint_id<'a>(e: &'a Value, binding: &str, skeleton: &str) -> Option<&'a str> {
    e.pointer(&format!("/{binding}/elementId"))
        .or_else(|| e.pointer(&format!("/{skeleton}/id")))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// 这个图形和谁有关系：容器、标签、箭头两端、指向它的箭头、同组。
pub fn related_ids(scene: &Scene, id: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![id.to_string()];
    if let Some(e) = scene.shape(id) {
        if let Some(c) = e.get("containerId").and_then(Value::as_str) {
            out.push(c.to_string());
        }
        for key in ["startBinding", "endBinding"] {
            if let Some(t) = e
                .pointer(&format!("/{key}/elementId"))
                .and_then(Value::as_str)
            {
                out.push(t.to_string());
            }
        }
        if let Some(groups) = e.get("groupIds").and_then(Value::as_array) {
            for g in groups.iter().filter_map(|g| g.as_str()) {
                for other in scene.live() {
                    let same = other
                        .get("groupIds")
                        .and_then(Value::as_array)
                        .map(|gs| gs.iter().any(|x| x.as_str() == Some(g)))
                        .unwrap_or(false);
                    if same {
                        if let Some(oid) = id_of(other) {
                            out.push(oid.to_string());
                        }
                    }
                }
            }
        }
    }
    for t in scene.live().chain(scene.pending.iter()) {
        if t.get("containerId").and_then(Value::as_str) == Some(id) {
            if let Some(tid) = id_of(t) {
                out.push(tid.to_string());
            }
        }
    }
    for e in scene.live().chain(scene.pending.iter()) {
        let kind = kind_of(e);
        if kind != "arrow" && kind != "line" {
            continue;
        }
        let points = endpoint_id(e, "startBinding", "start") == Some(id)
            || endpoint_id(e, "endBinding", "end") == Some(id);
        if points {
            if let Some(aid) = id_of(e) {
                out.push(aid.to_string());
            }
        }
    }
    // 刚带上的箭头，框里的字也算相关。
    let seeds = out.clone();
    for t in scene.live().chain(scene.pending.iter()) {
        if let Some(c) = t.get("containerId").and_then(Value::as_str) {
            if seeds.iter().any(|s| s == c) {
                if let Some(tid) = id_of(t) {
                    out.push(tid.to_string());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn live_mut<'a>(scene: &'a mut Scene, id: &str) -> Result<&'a mut Value, String> {
    if let Some(i) = scene.pending_index(id) {
        return Ok(&mut scene.pending[i]);
    }
    let i = scene
        .index_of(id)
        .filter(|&i| !is_deleted(&scene.elements[i]))
        .ok_or_else(|| format!("没有图形「{id}」：先用 whiteboard_read 看现有的 id"))?;
    Ok(&mut scene.elements[i])
}

fn live_ref<'a>(scene: &'a Scene, id: &str) -> Result<&'a Value, String> {
    scene
        .shape(id)
        .ok_or_else(|| format!("没有图形「{id}」：先用 whiteboard_read 看现有的 id"))
}

/// `whiteboard_group`：`{ ids, ungroup? }`。把 ids 合成一组，或拆掉它们共享的组。
pub fn apply_group(scene: &mut Scene, args: &Value) -> Result<ArrangeReport, String> {
    let ids = require_ids(args)?;
    let ungroup = args.get("ungroup").and_then(bool_arg).unwrap_or(false);
    let mut next = scene.clone();
    if ungroup {
        for id in &ids {
            let in_elements = next.index_of(id).is_some();
            let e = live_mut(&mut next, id)?;
            e["groupIds"] = json!([]);
            if in_elements {
                bump(e);
            }
        }
        next.rev = scene.rev + 1;
        *scene = next;
        return Ok(ArrangeReport {
            detail: format!("已取消 {} 个图形的分组", ids.len()),
            touched: ids,
        });
    }
    if ids.len() < 2 {
        return Err("group 至少要 2 个 id（把它们合成一组）".into());
    }
    let gid = format!("g{}", nonce());
    for id in &ids {
        let in_elements = next.index_of(id).is_some();
        let e = live_mut(&mut next, id)?;
        let mut groups = e
            .get("groupIds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !groups.iter().any(|g| g.as_str() == Some(gid.as_str())) {
            groups.push(json!(gid));
        }
        e["groupIds"] = Value::Array(groups);
        if in_elements {
            bump(e);
        }
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(ArrangeReport {
        detail: format!("已把 {} 个图形合成组 {gid}", ids.len()),
        touched: ids,
    })
}

const ALIGN_MODES: &[&str] = &["left", "right", "top", "bottom", "center_x", "center_y"];

/// `whiteboard_align`：`{ ids, align }`。按包围盒对齐。
pub fn apply_align(scene: &mut Scene, args: &Value) -> Result<ArrangeReport, String> {
    let ids = require_ids(args)?;
    if ids.len() < 2 {
        return Err("align 至少要 2 个 id".into());
    }
    let mode = args
        .get("align")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("缺 align（∈ {}）", ALIGN_MODES.join("|")))?;
    if !ALIGN_MODES.contains(&mode) {
        return Err(format!(
            "align「{mode}」不认识（∈ {}）",
            ALIGN_MODES.join("|")
        ));
    }
    let mut frames = Vec::with_capacity(ids.len());
    for id in &ids {
        let e = live_ref(scene, id)?;
        let (x, y, w, h) = frame_xywh(e);
        frames.push((id.clone(), x, y, w, h));
    }
    let min_x = frames.iter().map(|f| f.1).fold(f64::INFINITY, f64::min);
    let max_r = frames
        .iter()
        .map(|f| f.1 + f.3)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = frames.iter().map(|f| f.2).fold(f64::INFINITY, f64::min);
    let max_b = frames
        .iter()
        .map(|f| f.2 + f.4)
        .fold(f64::NEG_INFINITY, f64::max);
    let mid_x = (min_x + max_r) / 2.0;
    let mid_y = (min_y + max_b) / 2.0;

    let mut next = scene.clone();
    for (id, x, y, w, h) in &frames {
        let (nx, ny) = match mode {
            "left" => (min_x, *y),
            "right" => (max_r - *w, *y),
            "top" => (*x, min_y),
            "bottom" => (*x, max_b - *h),
            "center_x" => (mid_x - *w / 2.0, *y),
            "center_y" => (*x, mid_y - *h / 2.0),
            _ => unreachable!(),
        };
        if (nx - *x).abs() > 1e-9 || (ny - *y).abs() > 1e-9 {
            update_one(&mut next, id, &json!({ "x": nx, "y": ny }))?;
        }
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(ArrangeReport {
        detail: format!("已按 {mode} 对齐 {} 个图形", ids.len()),
        touched: ids,
    })
}

/// `whiteboard_style`：`{ ids, stroke?, fill?, strokeStyle? }` 批量改样式。
pub fn apply_style(scene: &mut Scene, args: &Value) -> Result<ArrangeReport, String> {
    let ids = require_ids(args)?;
    let has = ["stroke", "fill", "strokeStyle"]
        .iter()
        .any(|k| args.get(k).is_some());
    if !has {
        return Err("至少给 stroke / fill / strokeStyle 之一".into());
    }
    let mut patch = serde_json::Map::new();
    for k in ["stroke", "fill", "strokeStyle"] {
        if let Some(v) = args.get(k) {
            patch.insert(k.into(), v.clone());
        }
    }
    let patch = Value::Object(patch);
    let mut next = scene.clone();
    for (i, id) in ids.iter().enumerate() {
        update_one(&mut next, id, &patch)
            .map_err(|e| format!("第 {} 个图形：{e}（ids[{i}]）", i + 1))?;
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(ArrangeReport {
        detail: format!("已改 {} 个图形的样式", ids.len()),
        touched: ids,
    })
}

const LAYER_TO: &[&str] = &["front", "back", "forward", "backward"];

/// `whiteboard_layer`：`{ ids, to }`。调 z 序（数组越后越靠上）。
pub fn apply_layer(scene: &mut Scene, args: &Value) -> Result<ArrangeReport, String> {
    let ids = require_ids(args)?;
    let to = args
        .get("to")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("缺 to（∈ {}）", LAYER_TO.join("|")))?;
    if !LAYER_TO.contains(&to) {
        return Err(format!("to「{to}」不认识（∈ {}）", LAYER_TO.join("|")));
    }
    // 只动完整元素；pending 本来就在「待画」层，不参与 z 序。
    for id in &ids {
        if scene.pending_index(id).is_some() && scene.index_of(id).is_none() {
            return Err(format!("「{id}」还在待画：等界面画成完整元素后再调图层"));
        }
        if scene
            .index_of(id)
            .filter(|&i| !is_deleted(&scene.elements[i]))
            .is_none()
        {
            return Err(format!(
                "没有图形「{id}」：先用 whiteboard_read 看现有的 id"
            ));
        }
    }
    let mut next = scene.clone();
    // Excalidraw 0.18 用 `index` 存 z 序，本界面重载时还按数组顺序画，两处一起改。
    // 缺 index 的先按数组顺序补一个（界面上也是这么分配的）。
    let mut last: Option<String> = None;
    for e in next.elements.iter_mut() {
        if is_deleted(e) {
            continue;
        }
        match e.get("index").and_then(Value::as_str) {
            Some(k) => last = Some(k.to_string()),
            None => {
                let k = key_between(last.as_deref(), None).unwrap_or_else(|| "a0".into());
                e["index"] = json!(k);
                last = Some(k);
            }
        }
    }
    // 现在的 z 序（按 index 排）。
    let mut order: Vec<usize> = (0..next.elements.len())
        .filter(|&i| !is_deleted(&next.elements[i]))
        .collect();
    order.sort_by(|&a, &b| {
        next.elements[a]["index"]
            .as_str()
            .unwrap_or("")
            .cmp(next.elements[b]["index"].as_str().unwrap_or(""))
    });
    let selected: std::collections::HashSet<String> = ids.iter().cloned().collect();
    let is_sel = |i: usize| selected.contains(id_of(&next.elements[i]).unwrap_or(""));
    match to {
        "front" => {
            let sel: Vec<usize> = order.iter().copied().filter(|&i| is_sel(i)).collect();
            order.retain(|i| !is_sel(*i));
            order.extend(sel);
        }
        "back" => {
            let sel: Vec<usize> = order.iter().copied().filter(|&i| is_sel(i)).collect();
            order.retain(|i| !is_sel(*i));
            let mut head = sel;
            head.extend(order.iter().copied());
            order = head;
        }
        "forward" => {
            // 每个选中项与后一个未选中项交换（从后往前扫避免连环）。
            for i in (1..order.len()).rev() {
                if is_sel(order[i - 1]) && !is_sel(order[i]) {
                    order.swap(i - 1, i);
                }
            }
        }
        "backward" => {
            for i in 0..order.len().saturating_sub(1) {
                if is_sel(order[i + 1]) && !is_sel(order[i]) {
                    order.swap(i, i + 1);
                }
            }
        }
        _ => unreachable!(),
    }
    // index 只给选中的元素重排（其他元素版本不动，界面才认这次改动）；
    // 数组顺序才是渲染顺序（Excalidraw 按数组顺序画，index 只是持久化的 z 序键）。
    assign_indices(&mut next, &order, &selected)?;
    reorder_elements(&mut next, &order);
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(ArrangeReport {
        detail: format!("已把 {} 个图形移到 {to}", ids.len()),
        touched: ids,
    })
}

/// 按新的 z 序给选中的元素分配 `index`：夹在前后的未选中元素之间。
/// 只动选中的（其他元素版本不变，界面才认这次改动）。
fn assign_indices(
    next: &mut Scene,
    order: &[usize],
    selected: &std::collections::HashSet<String>,
) -> Result<(), String> {
    // 未选中元素的（位置, index）：位置和 index 都不动，它们是选中元素的前后邻居。
    let mut fixed: Vec<(usize, String)> = Vec::new();
    for (pos, &i) in order.iter().enumerate() {
        let id = id_of(&next.elements[i]).unwrap_or("").to_string();
        if selected.contains(&id) {
            continue;
        }
        let k = next.elements[i]
            .get("index")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "a0".into());
        fixed.push((pos, k));
    }
    let mut last_in_group: Option<String> = None;
    let mut last_pos: Option<usize> = None;
    for (pos, &i) in order.iter().enumerate() {
        let id = id_of(&next.elements[i]).unwrap_or("").to_string();
        if !selected.contains(&id) {
            continue;
        }
        // 连续选中算一组：组内沿用上一个分到的 key 当下界。
        if last_pos.map(|p| p + 1 != pos).unwrap_or(true) {
            last_in_group = None;
        }
        let lower = match &last_in_group {
            Some(k) => Some(k.clone()),
            None => fixed
                .iter()
                .rev()
                .find(|(p, _)| *p < pos)
                .map(|(_, k)| k.clone()),
        };
        let upper = fixed.iter().find(|(p, _)| *p > pos).map(|(_, k)| k.clone());
        let key = key_between(lower.as_deref(), upper.as_deref())
            .ok_or_else(|| "这一板的图形太多了，排不出顺序了".to_string())?;
        if next.elements[i]["index"] != json!(key) {
            next.elements[i]["index"] = json!(key);
            bump(&mut next.elements[i]);
        }
        last_in_group = Some(key);
        last_pos = Some(pos);
    }
    Ok(())
}

/// 数组顺序 = 渲染顺序：按新 z 序重排未删的元素；墓碑留在后面，相对顺序不变。
fn reorder_elements(next: &mut Scene, order: &[usize]) {
    let mut in_order = vec![false; next.elements.len()];
    for &i in order {
        in_order[i] = true;
    }
    let mut live: Vec<Value> = Vec::with_capacity(order.len());
    for &i in order {
        live.push(next.elements[i].clone());
    }
    let mut tombs: Vec<Value> = Vec::new();
    for (i, e) in next.elements.iter().enumerate() {
        if !in_order[i] {
            tombs.push(e.clone());
        }
    }
    next.elements = live.into_iter().chain(tombs).collect();
}

/// 估算文字占位：中文约 1em 宽；padding 左右各 16、上下各 12。
fn estimate_text_box(text: &str, font_size: f64) -> (f64, f64) {
    let lines: Vec<&str> = text.lines().collect();
    let lines = if lines.is_empty() { vec![""] } else { lines };
    let max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f64;
    let width = (max_chars * font_size).max(font_size) + 32.0;
    let height = lines.len() as f64 * font_size * 1.25 + 24.0;
    (width, height)
}

fn shape_label_text(scene: &Scene, e: &Value) -> Option<String> {
    let kind = kind_of(e);
    if kind == "text" {
        return e.get("text").and_then(Value::as_str).map(str::to_string);
    }
    if let Some(t) = e.pointer("/label/text").and_then(Value::as_str) {
        return Some(t.to_string());
    }
    let id = id_of(e)?;
    scene
        .live()
        .find(|t| t.get("containerId").and_then(Value::as_str) == Some(id))
        .and_then(|t| t.get("text").and_then(Value::as_str))
        .map(str::to_string)
}

/// `whiteboard_fit_text`：`{ ids }`。按文字估算把框调到够用的大小（菱形 height 只抬下限）。
pub fn apply_fit_text(scene: &mut Scene, args: &Value) -> Result<ArrangeReport, String> {
    let ids = require_ids(args)?;
    let mut next = scene.clone();
    for (i, id) in ids.iter().enumerate() {
        let e = live_ref(&next, id)
            .map_err(|e| format!("第 {} 个图形：{e}（ids[{i}]）", i + 1))?
            .clone();
        let kind = kind_of(&e);
        if kind == "arrow" || kind == "line" {
            return Err(format!(
                "第 {} 个图形是 {kind}，fit_text 只适用于矩形/椭圆/菱形/文字（ids[{i}]）",
                i + 1
            ));
        }
        let text = shape_label_text(&next, &e).unwrap_or_default();
        if text.is_empty() {
            return Err(format!("第 {} 个图形没有文字可适配（ids[{i}]）", i + 1));
        }
        let font = opt_num(&e, "fontSize").unwrap_or(20.0).max(8.0);
        let (w, h) = estimate_text_box(&text, font);
        let mut patch = json!({ "width": w });
        if kind == "diamond" {
            // 菱形 height 只是下限：只抬高、不缩小。
            let cur_h = num(&e, "height");
            if h > cur_h {
                patch["height"] = json!(h);
            }
        } else {
            patch["height"] = json!(h);
        }
        update_one(&mut next, id, &patch)
            .map_err(|e| format!("第 {} 个图形：{e}（ids[{i}]）", i + 1))?;
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(ArrangeReport {
        detail: format!("已按文字适配 {} 个图形尺寸", ids.len()),
        touched: ids,
    })
}

const PLACE_WHERE: &[&str] = &["left", "right", "above", "below"];
const DEFAULT_PLACE_GAP: f64 = 40.0;
const GRAPH_W: f64 = 200.0;
const GRAPH_H: f64 = 80.0;
const GRAPH_GAP_X: f64 = 60.0;
const GRAPH_GAP_Y: f64 = 80.0;

fn frame_xywh(e: &Value) -> (f64, f64, f64, f64) {
    // 宽高取绝对值：模型能传负宽高，按外框算位置时要按真实大小算。
    (
        num(e, "x"),
        num(e, "y"),
        num(e, "width").abs(),
        num(e, "height").abs(),
    )
}

fn place_beside(ref_e: &Value, where_: &str, gap: f64, w: f64, h: f64) -> (f64, f64) {
    let (rx, ry, rw, rh) = frame_xywh(ref_e);
    match where_ {
        "right" => (rx + rw + gap, ry + (rh - h) / 2.0),
        "left" => (rx - gap - w, ry + (rh - h) / 2.0),
        "below" => (rx + (rw - w) / 2.0, ry + rh + gap),
        "above" => (rx + (rw - w) / 2.0, ry - gap - h),
        _ => (rx + rw + gap, ry),
    }
}

/// 间距参数：不能为负（负间距会让图形叠在一起）。
fn gap_arg(args: &Value, key: &str, default: f64) -> Result<f64, String> {
    match opt_num(args, key) {
        Some(v) if v < 0.0 => Err(format!("{key} 不能为负（{v}）")),
        Some(v) => Ok(v),
        None => Ok(default),
    }
}

// ---- fractional index（Excalidraw 的 z 序键）----
//
// Excalidraw 0.18 用 `index`（base62 分数索引）存 z 序。本界面重载时还按数组顺序画，
// 所以调图层、复制时 index 和数组顺序一起改。键的算法照搬 npm fractional-indexing
// （CC0）的 `generateKeyBetween`，键空间与界面一致。

const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn digit_of(c: u8) -> Option<usize> {
    BASE62.iter().position(|&d| d == c)
}

/// 整数部分的长度：头字符决定（`a`→2、`b`→3…、`A`→27…）。
fn integer_len(head: u8) -> Option<usize> {
    match head {
        b'a'..=b'z' => Some((head - b'a') as usize + 2),
        b'A'..=b'Z' => Some((b'Z' - head) as usize + 2),
        _ => None,
    }
}

fn integer_part(key: &str) -> Option<&str> {
    let head = key.as_bytes().first().copied()?;
    let len = integer_len(head)?;
    if len > key.len() {
        return None;
    }
    Some(&key[..len])
}

fn increment_integer(x: &str) -> Option<String> {
    let bytes = x.as_bytes();
    let head = bytes[0];
    let mut digs: Vec<u8> = bytes[1..].to_vec();
    let mut carry = true;
    let mut i = digs.len();
    while carry && i > 0 {
        i -= 1;
        let d = digit_of(digs[i]).unwrap_or(0) + 1;
        if d == BASE62.len() {
            digs[i] = BASE62[0];
        } else {
            digs[i] = BASE62[d];
            carry = false;
        }
    }
    if carry {
        if head == b'Z' {
            return Some(format!("a{}", BASE62[0] as char));
        }
        if head == b'z' {
            return None;
        }
        let h = head + 1;
        if h > b'a' {
            digs.push(BASE62[0]);
        } else {
            digs.pop();
        }
        Some(format!("{}{}", h as char, String::from_utf8_lossy(&digs)))
    } else {
        Some(format!(
            "{}{}",
            head as char,
            String::from_utf8_lossy(&digs)
        ))
    }
}

fn decrement_integer(x: &str) -> Option<String> {
    let bytes = x.as_bytes();
    let head = bytes[0];
    let mut digs: Vec<u8> = bytes[1..].to_vec();
    let mut borrow = true;
    let mut i = digs.len();
    while borrow && i > 0 {
        i -= 1;
        let d = digit_of(digs[i]).unwrap_or(0).checked_sub(1);
        match d {
            None => digs[i] = *BASE62.last().unwrap(),
            Some(d) => {
                digs[i] = BASE62[d];
                borrow = false;
            }
        }
    }
    if borrow {
        if head == b'a' {
            return Some(format!("Z{}", *BASE62.last().unwrap() as char));
        }
        if head == b'A' {
            return None;
        }
        let h = head - 1;
        if h < b'Z' {
            digs.push(*BASE62.last().unwrap());
        } else {
            digs.pop();
        }
        Some(format!("{}{}", h as char, String::from_utf8_lossy(&digs)))
    } else {
        Some(format!(
            "{}{}",
            head as char,
            String::from_utf8_lossy(&digs)
        ))
    }
}

/// 两个键之间的中点（npm 版同款）。
fn midpoint(a: &str, b: Option<&str>) -> String {
    let zero = BASE62[0];
    if let Some(b) = b {
        // 去最长公共前缀；`a` 短了按 zero 补，`b` 短了停。
        let ab = a.as_bytes();
        let bb = b.as_bytes();
        let mut n = 0usize;
        while let Some(&cb) = bb.get(n) {
            let ca = *ab.get(n).unwrap_or(&zero);
            if ca != cb {
                break;
            }
            n += 1;
        }
        if n > 0 {
            return format!("{}{}", &b[..n], midpoint(&a[n..], Some(&b[n..])));
        }
    }
    let digit_a = if a.is_empty() {
        0
    } else {
        digit_of(a.as_bytes()[0]).unwrap_or(0)
    };
    let digit_b = match b {
        Some(x) if !x.is_empty() => digit_of(x.as_bytes()[0]).unwrap_or(BASE62.len()),
        _ => BASE62.len(),
    };
    if digit_b.saturating_sub(digit_a) > 1 {
        let mid = ((digit_a + digit_b) as f64 * 0.5).round() as usize;
        return (BASE62[mid.min(BASE62.len() - 1)] as char).to_string();
    }
    if let Some(b) = b {
        if b.len() > 1 {
            return b[..1].to_string();
        }
    }
    format!("{}{}", BASE62[digit_a] as char, midpoint(&a[1..], None))
}

/// `a` 与 `b` 之间的键（`a < b`，端点可为 None）。
pub fn key_between(a: Option<&str>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (None, None) => return Some(format!("a{}", BASE62[0] as char)),
        (Some(x), Some(y)) if x >= y => return None,
        _ => {}
    }
    if let (Some(a), None) = (a, b) {
        let ia = integer_part(a)?;
        let fa = &a[ia.len()..];
        return match increment_integer(ia) {
            Some(i) => Some(i),
            None => Some(format!("{ia}{}", midpoint(fa, None))),
        };
    }
    if let (None, Some(b)) = (a, b) {
        let ib = integer_part(b)?;
        let fb = &b[ib.len()..];
        if ib == format!("A{}", "0".repeat(26)) {
            return Some(format!("{ib}{}", midpoint("", Some(fb))));
        }
        if ib < b {
            return Some(ib.to_string());
        }
        let res = decrement_integer(ib)?;
        return Some(res);
    }
    let a = a?;
    let b = b?;
    let ia = integer_part(a)?;
    let fa = &a[ia.len()..];
    let ib = integer_part(b)?;
    let fb = &b[ib.len()..];
    if ia == ib {
        return Some(format!("{ia}{}", midpoint(fa, Some(fb))));
    }
    let i = increment_integer(ia)?;
    if i.as_str() < b {
        return Some(i);
    }
    Some(format!("{ia}{}", midpoint(fa, None)))
}

/// `whiteboard_distribute`：`{ ids, direction: horizontal|vertical }`。≥3 个；两端不动，中间均分。
pub fn apply_distribute(scene: &mut Scene, args: &Value) -> Result<ArrangeReport, String> {
    let ids = require_ids(args)?;
    if ids.len() < 3 {
        return Err("distribute 至少要 3 个 id（两端固定，中间均分）".into());
    }
    let dir = args
        .get("direction")
        .and_then(Value::as_str)
        .ok_or("缺 direction（horizontal|vertical）")?;
    if dir != "horizontal" && dir != "vertical" {
        return Err(format!("direction「{dir}」不认识（horizontal|vertical）"));
    }
    let mut frames = Vec::with_capacity(ids.len());
    for id in &ids {
        let e = live_ref(scene, id)?;
        frames.push((id.clone(), frame_xywh(e)));
    }
    if dir == "horizontal" {
        frames.sort_by(|a, b| {
            a.1 .0
                .partial_cmp(&b.1 .0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    } else {
        frames.sort_by(|a, b| {
            a.1 .1
                .partial_cmp(&b.1 .1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    let n = frames.len();
    let first = frames[0].1;
    let last = frames[n - 1].1;
    let mut next = scene.clone();
    if dir == "horizontal" {
        let span = (last.0 + last.2) - first.0;
        let sizes: f64 = frames.iter().map(|f| f.1 .2).sum();
        let gap = (span - sizes) / (n as f64 - 1.0);
        if gap < 0.0 {
            return Err(format!(
                "{dir} 均分放不下：图形总宽 {sizes} 大于两端跨度 {span}"
            ));
        }
        let mut cursor = first.0;
        for (i, (id, (_, y, w, _))) in frames.iter().enumerate() {
            if i == 0 {
                cursor = first.0 + w;
                continue;
            }
            if i + 1 == n {
                break;
            }
            let nx = cursor + gap;
            update_one(&mut next, id, &json!({ "x": nx, "y": *y }))?;
            cursor = nx + w;
        }
    } else {
        let span = (last.1 + last.3) - first.1;
        let sizes: f64 = frames.iter().map(|f| f.1 .3).sum();
        let gap = (span - sizes) / (n as f64 - 1.0);
        if gap < 0.0 {
            return Err(format!(
                "{dir} 均分放不下：图形总高 {sizes} 大于两端跨度 {span}"
            ));
        }
        let mut cursor = first.1;
        for (i, (id, (x, _, _, h))) in frames.iter().enumerate() {
            if i == 0 {
                cursor = first.1 + h;
                continue;
            }
            if i + 1 == n {
                break;
            }
            let ny = cursor + gap;
            update_one(&mut next, id, &json!({ "x": *x, "y": ny }))?;
            cursor = ny + h;
        }
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(ArrangeReport {
        detail: format!("已沿 {dir} 均分 {} 个图形", ids.len()),
        touched: ids,
    })
}

/// `whiteboard_layout`：`{ ids, columns, gap_x?, gap_y? }`。按网格重排（保留各自身高宽）。
pub fn apply_layout(scene: &mut Scene, args: &Value) -> Result<ArrangeReport, String> {
    let ids = require_ids(args)?;
    let columns = args
        .get("columns")
        .and_then(num_arg)
        .filter(|c| *c >= 1.0)
        .map(|c| c as usize)
        .ok_or("缺 columns（正整数；写成 \"3\" 也行）")?;
    let gap_x = gap_arg(args, "gap_x", GRAPH_GAP_X)?;
    let gap_y = gap_arg(args, "gap_y", GRAPH_GAP_Y)?;
    let mut origin_x = f64::INFINITY;
    let mut origin_y = f64::INFINITY;
    let mut sizes = Vec::with_capacity(ids.len());
    for id in &ids {
        let e = live_ref(scene, id)?;
        let (x, y, w, h) = frame_xywh(e);
        origin_x = origin_x.min(x);
        origin_y = origin_y.min(y);
        sizes.push((id.clone(), w, h));
    }
    let rows = ids.len().div_ceil(columns);
    let mut col_w = vec![0.0; columns];
    let mut row_h = vec![0.0; rows];
    for (i, (_, w, h)) in sizes.iter().enumerate() {
        let c = i % columns;
        let r = i / columns;
        col_w[c] = f64::max(col_w[c], *w);
        row_h[r] = f64::max(row_h[r], *h);
    }
    let mut next = scene.clone();
    for (i, (id, w, h)) in sizes.iter().enumerate() {
        let c = i % columns;
        let r = i / columns;
        let x =
            origin_x + col_w.iter().take(c).sum::<f64>() + gap_x * c as f64 + (col_w[c] - w) / 2.0;
        let y =
            origin_y + row_h.iter().take(r).sum::<f64>() + gap_y * r as f64 + (row_h[r] - h) / 2.0;
        update_one(&mut next, id, &json!({ "x": x, "y": y }))?;
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(ArrangeReport {
        detail: format!("已按 {columns} 列网格排布 {} 个图形", ids.len()),
        touched: ids,
    })
}

/// `whiteboard_place`：`{ id, ref, where: left|right|above|below, gap? }`。把已有图形放到参照旁。
pub fn apply_place(scene: &mut Scene, args: &Value) -> Result<ArrangeReport, String> {
    let id = args
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("缺 id（要挪的图形）")?;
    let ref_id = args
        .get("ref")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("缺 ref（参照图形 id）")?;
    if id == ref_id {
        return Err("id 和 ref 不能相同".into());
    }
    let where_ = args
        .get("where")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("缺 where（∈ {}）", PLACE_WHERE.join("|")))?;
    if !PLACE_WHERE.contains(&where_) {
        return Err(format!(
            "where「{where_}」不认识（∈ {}）",
            PLACE_WHERE.join("|")
        ));
    }
    let gap = gap_arg(args, "gap", DEFAULT_PLACE_GAP)?;
    let target = live_ref(scene, id)?.clone();
    let refer = live_ref(scene, ref_id)?.clone();
    let (_, _, w, h) = frame_xywh(&target);
    let (nx, ny) = place_beside(&refer, where_, gap, w, h);
    let mut next = scene.clone();
    update_one(&mut next, id, &json!({ "x": nx, "y": ny }))?;
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(ArrangeReport {
        detail: format!("已把 {id} 放到 {ref_id} 的 {where_}"),
        touched: vec![id.to_string()],
    })
}

/// 现有内容右边找一块空地：横着排开，别都堆成一列。
fn free_slot(scene: &Scene, w: f64, h: f64) -> (f64, f64) {
    let (start_x, start_y) = match scene.bounds() {
        Some((_, top, right, _)) => (right + GAP * 2.0, top),
        None => (100.0, 100.0),
    };
    let ww = w.max(24.0);
    let hh = h.max(24.0);
    let mut y = start_y;
    for _ in 0..200 {
        let mut x = start_x;
        while x < start_x + 6000.0 {
            if !hits_any(scene, x, y, ww, hh) {
                return (x, y);
            }
            x += GAP;
        }
        y += hh + GAP;
    }
    (start_x, y)
}

/// 这块地和现有图形撞不撞。
fn hits_any(scene: &Scene, x: f64, y: f64, w: f64, h: f64) -> bool {
    scene.live().chain(scene.pending.iter()).any(|e| {
        let (ex, ey) = (num(e, "x"), num(e, "y"));
        let (ew, eh) = (num(e, "width").abs(), num(e, "height").abs());
        x < ex + ew && x + w > ex && y < ey + eh && y + h > ey
    })
}

fn default_x(scene: &Scene) -> f64 {
    match scene.bounds() {
        Some((_, _, right, _)) => right + GAP * 2.0,
        None => 100.0,
    }
}

fn default_y(scene: &Scene) -> f64 {
    match scene.bounds() {
        Some((_, top, _, _)) => top,
        None => 100.0,
    }
}

/// 图形和谁有关系：容器、组、箭头两端（模型找图时靠它认「父子兄弟」）。
fn relation_of(e: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = e.get("containerId").and_then(Value::as_str) {
        parts.push(format!("在 {c} 里"));
    }
    if let Some(groups) = e.get("groupIds").and_then(Value::as_array) {
        let names: Vec<&str> = groups.iter().filter_map(|g| g.as_str()).collect();
        if !names.is_empty() {
            parts.push(format!("组 {}", names.join(",")));
        }
    }
    for (key, name) in [("startBinding", "起于"), ("endBinding", "止于")] {
        if let Some(t) = e
            .pointer(&format!("/{key}/elementId"))
            .and_then(Value::as_str)
        {
            parts.push(format!("{name} {t}"));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("（{}）", parts.join("，"))
    }
}

/// `whiteboard_find` 的结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FindReport {
    pub found: Vec<String>,
    pub detail: String,
}

/// `whiteboard_find`：`{ queries: [{ text?, type?, stroke?, fill?, near?, radius? }], limit? }`。
/// 按文字、类型、颜色、距离找图形，不用背 id。不是向量语义。
/// 一次可以给多个查询。每个查询先收齐，再按「整段相等 > 前缀 > 包含」、离 `near` 的距离排序，然后截断。
pub fn apply_find(scene: &mut Scene, args: &Value) -> Result<FindReport, String> {
    let (queries, _) = take_array_field(args, "queries")?;
    if queries.is_empty() {
        return Err("queries 不能为空：给要找的词（可以多个）".into());
    }
    let limit = args
        .get("limit")
        .and_then(num_arg)
        .map(|l| (l as usize).clamp(1, 50))
        .unwrap_or(20);
    let mut detail = String::new();
    let mut found: Vec<String> = Vec::new();
    for (qi, q) in queries.iter().enumerate() {
        let text = q
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase());
        let kind = q.get("type").and_then(Value::as_str);
        let stroke = q
            .get("stroke")
            .and_then(Value::as_str)
            .map(|s| s.to_lowercase());
        let fill = q
            .get("fill")
            .and_then(Value::as_str)
            .map(|s| s.to_lowercase());
        let near = q
            .get("near")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let radius = opt_num(q, "radius").unwrap_or(300.0);
        if text.is_none() && kind.is_none() && stroke.is_none() && fill.is_none() && near.is_none()
        {
            return Err(format!(
                "第 {} 个查询没说要找什么（text / type / stroke / fill / near 至少给一个）",
                qi + 1
            ));
        }
        let near_center = if let Some(n) = near {
            let e = scene
                .shape(n)
                .ok_or_else(|| format!("没有图形「{n}」：near 要是板上已有的 id"))?;
            Some((
                num(e, "x") + num(e, "width") / 2.0,
                num(e, "y") + num(e, "height") / 2.0,
                n.to_string(),
            ))
        } else {
            None
        };
        // (匹配档, 距离, id, 元素)。先收齐再排，limit 不能截在排序前面。
        let mut hits: Vec<(i32, f64, String, Value)> = Vec::new();
        for e in scene.live().chain(scene.pending.iter()) {
            // 容器里的标签不单独列（跟着它的框走）。
            if e.get("containerId").and_then(Value::as_str).is_some() {
                continue;
            }
            let id = id_of(e).unwrap_or_default().to_string();
            if near_center
                .as_ref()
                .is_some_and(|(_, _, anchor)| anchor == &id)
            {
                continue;
            }
            let score = if let Some(t) = &text {
                let label = shape_label_text(scene, e)
                    .unwrap_or_default()
                    .to_lowercase();
                if label == *t {
                    3
                } else if label.starts_with(t.as_str()) {
                    2
                } else if label.contains(t.as_str()) {
                    1
                } else {
                    continue;
                }
            } else {
                0
            };
            if let Some(k) = kind {
                if kind_of(e) != k {
                    continue;
                }
            }
            if let Some(c) = &stroke {
                let got = e
                    .get("strokeColor")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase();
                if &got != c {
                    continue;
                }
            }
            if let Some(c) = &fill {
                let got = e
                    .get("backgroundColor")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase();
                if &got != c {
                    continue;
                }
            }
            let dist = if let Some((cx, cy, _)) = near_center {
                let (ex, ey) = (
                    num(e, "x") + num(e, "width") / 2.0,
                    num(e, "y") + num(e, "height") / 2.0,
                );
                let d = ((ex - cx).powi(2) + (ey - cy).powi(2)).sqrt();
                if d > radius {
                    continue;
                }
                d
            } else {
                0.0
            };
            hits.push((score, dist, id, e.clone()));
        }
        hits.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.2.cmp(&b.2))
        });
        hits.truncate(limit);
        detail.push_str(&format!("查询 {}：{} 个\n", qi + 1, hits.len()));
        for (_, _, id, e) in &hits {
            let label = shape_label_text(scene, e).unwrap_or_default();
            let line = describe(
                kind_of(e),
                id,
                e,
                Some(label.as_str()),
                e.pointer("/startBinding/elementId")
                    .or_else(|| e.pointer("/start/id"))
                    .and_then(Value::as_str),
                e.pointer("/endBinding/elementId")
                    .or_else(|| e.pointer("/end/id"))
                    .and_then(Value::as_str),
                false,
            );
            detail.push_str(&format!("{line}{}\n", relation_of(e)));
            found.push(id.clone());
        }
    }
    Ok(FindReport { found, detail })
}

/// 外框并集：`(min_x, min_y, max_x, max_y)`。宽高取绝对值。
fn union_frame(acc: Option<(f64, f64, f64, f64)>, e: &Value) -> (f64, f64, f64, f64) {
    let (x, y, w, h) = frame_xywh(e);
    match acc {
        None => (x, y, x + w, y + h),
        Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x + w), d.max(y + h)),
    }
}

/// 还在 pending、而且没有活着的完整元素：副本回到 pending。墓碑加骨架也算待画。
fn stored_pending(scene: &Scene, id: &str) -> bool {
    scene.pending_index(id).is_some() && !scene.live().any(|e| id_of(e) == Some(id))
}

fn max_index_key(scene: &Scene) -> Option<String> {
    scene
        .elements
        .iter()
        .chain(scene.pending.iter())
        .filter_map(|e| e.get("index").and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .max()
        .map(str::to_string)
}

/// 完整元素丢掉克隆来的 `index`，接到当前最大键之后；骨架不带 index。
fn assign_z(copy: &mut Value, pending: bool, last: &mut Option<String>) {
    if let Some(obj) = copy.as_object_mut() {
        obj.remove("index");
    }
    if pending {
        return;
    }
    let key = key_between(last.as_deref(), None).unwrap_or_else(|| "a0".into());
    copy["index"] = json!(key);
    *last = Some(key);
}

fn push_copy(scene: &mut Scene, copy: Value, pending: bool) {
    if pending {
        scene.pending.push(copy);
    } else {
        scene.elements.push(copy);
    }
}

/// 只改嵌套字段（`startBinding.elementId`），不写出扁平键。
fn retarget_nested(
    copy: &mut Value,
    key: &str,
    field: &str,
    map: &std::collections::HashMap<String, String>,
) {
    let old = copy
        .get(key)
        .and_then(|v| v.get(field))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(old) = old else {
        return;
    };
    let Some(new_id) = map.get(&old) else {
        return;
    };
    if let Some(obj) = copy.get_mut(key).and_then(Value::as_object_mut) {
        obj.insert(field.to_string(), json!(new_id));
    }
}

/// 改写副本的 `boundElements`：映射表里有的换成新 id；没跟着复制的箭头 / 线
/// （只连一端的外连箭头）直接去掉，它们并不绑在副本上。
fn retarget_bound(copy: &mut Value, map: &std::collections::HashMap<String, String>) {
    let Some(bound) = copy.get_mut("boundElements").and_then(Value::as_array_mut) else {
        return;
    };
    bound.retain(|b| {
        let unmapped = b
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !map.contains_key(id));
        let is_link = matches!(
            b.get("type").and_then(Value::as_str),
            Some("arrow") | Some("line")
        );
        !(unmapped && is_link)
    });
    for b in bound {
        let Some(old) = b.get("id").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        if let Some(new_id) = map.get(&old) {
            if let Some(obj) = b.as_object_mut() {
                obj.insert("id".into(), json!(new_id));
            }
        }
    }
}

fn retarget_container(copy: &mut Value, map: &std::collections::HashMap<String, String>) {
    let Some(old) = copy
        .get("containerId")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    if let Some(new_id) = map.get(&old) {
        copy["containerId"] = json!(new_id);
    }
}

fn remap_groups(copy: &mut Value, group_map: &mut std::collections::HashMap<String, String>) {
    let Some(groups) = copy.get("groupIds").and_then(Value::as_array) else {
        return;
    };
    let mapped: Vec<Value> = groups
        .iter()
        .filter_map(|g| g.as_str())
        .map(|g| {
            let ng = group_map
                .entry(g.to_string())
                .or_insert_with(|| format!("g{}", nonce()))
                .clone();
            json!(ng)
        })
        .collect();
    copy["groupIds"] = json!(mapped);
}

fn apply_copy_text(copy: &mut Value, text: &str) {
    if kind_of(copy) == "text" || copy.get("containerId").is_some() {
        copy["text"] = json!(text);
        copy["originalText"] = json!(text);
    } else if let Some(label) = copy.get_mut("label") {
        label["text"] = json!(text);
    }
}

fn copy_id(
    old_id: &str,
    k: usize,
    scene: &Scene,
    used: &mut std::collections::HashSet<String>,
) -> String {
    let base = format!("{old_id}-copy");
    let mut candidate = if k == 0 {
        base.clone()
    } else {
        format!("{base}{}", k + 1)
    };
    let mut n = 2usize;
    while scene.id_taken(&candidate) || used.contains(&candidate) {
        candidate = format!("{base}{n}");
        n += 1;
    }
    used.insert(candidate.clone());
    candidate
}

fn label_copy_id(
    container_new: &str,
    scene: &Scene,
    used: &mut std::collections::HashSet<String>,
) -> String {
    let base = format!("{container_new}-label");
    let mut candidate = base.clone();
    let mut n = 2usize;
    while scene.id_taken(&candidate) || used.contains(&candidate) {
        candidate = format!("{base}{n}");
        n += 1;
    }
    used.insert(candidate.clone());
    candidate
}

/// `whiteboard_duplicate`：`{ ids, count?, texts?, place?, dx?, dy? }`。
/// 复制给出的图形。两端都在这批 id 里的箭头 / 线也复制（不再往外扩）；只连一端的留在原件上。
/// 标签跟着容器走（容器在 pending，标签也进 pending）。`texts` 给每份一份新文案。
/// `place` 按选区外框平移；给了 `dx` / `dy` 就按偏移（缺的那个当 0）；都没给就找空地。
/// 返回的 id：每份先是请求的，再是自动带上的箭头。标签不在里面。
pub fn apply_duplicate(scene: &mut Scene, args: &Value) -> Result<ArrangeReport, String> {
    let ids = require_ids(args)?;
    // texts 走能剥 `{"item": ...}` 的解析（模型常把数组包一层）。
    let (texts_raw, _) = take_array_field(args, "texts")?;
    let texts: Vec<String> = texts_raw
        .iter()
        .filter_map(|t| t.as_str().map(str::to_string))
        .collect();
    // count 没给就按 texts 的份数来（少一个参数就少一次往返）。
    let count = args
        .get("count")
        .and_then(num_arg)
        .map(|c| (c as usize).clamp(1, 20))
        .unwrap_or_else(|| texts.len().clamp(1, 20));
    let place = args.get("place").and_then(Value::as_str);
    if let Some(p) = place {
        if !["right", "left", "above", "below"].contains(&p) {
            return Err(format!("place「{p}」不认识（right|left|above|below）"));
        }
    }
    let dx = opt_num(args, "dx");
    let dy = opt_num(args, "dy");
    // 先收请求的 id（重复的只留第一次），再补两端都在这批 id 里的箭头 / 线。不把补上的再拿去扩。
    let mut sources: Vec<String> = Vec::new();
    let mut source_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for id in &ids {
        if scene.shape(id).is_none() {
            return Err(format!(
                "没有图形「{id}」：先用 whiteboard_read / whiteboard_find 看现有的 id"
            ));
        }
        if source_set.insert(id.clone()) {
            sources.push(id.clone());
        }
    }
    let requested = source_set.clone();
    let mut extras: Vec<String> = Vec::new();
    for e in scene.live().chain(scene.pending.iter()) {
        let kind = kind_of(e);
        if kind != "arrow" && kind != "line" {
            continue;
        }
        let Some(id) = id_of(e) else {
            continue;
        };
        if source_set.contains(id) {
            continue;
        }
        let start = endpoint_id(e, "startBinding", "start");
        let end = endpoint_id(e, "endBinding", "end");
        if let (Some(start), Some(end)) = (start, end) {
            if requested.contains(start) && requested.contains(end) {
                extras.push(id.to_string());
                source_set.insert(id.to_string());
            }
        }
    }
    sources.extend(extras.iter().cloned());
    // 标签跟着容器走。已经在选区里的不再复制第二遍。
    let mut labels: Vec<(String, Value)> = Vec::new();
    for id in &sources {
        for t in scene.live().chain(scene.pending.iter()) {
            if t.get("containerId").and_then(Value::as_str) != Some(id.as_str()) {
                continue;
            }
            let Some(lid) = id_of(t) else {
                continue;
            };
            if source_set.contains(lid) || labels.iter().any(|(_, l)| id_of(l) == Some(lid)) {
                continue;
            }
            labels.push((id.clone(), t.clone()));
        }
    }
    let mut bbox: Option<(f64, f64, f64, f64)> = None;
    for id in &sources {
        let e = scene.shape(id).expect("上面刚确认过");
        bbox = Some(union_frame(bbox, e));
    }
    for (_, label) in &labels {
        bbox = Some(union_frame(bbox, label));
    }
    let (min_x, min_y, max_x, max_y) = bbox.ok_or("ids 不能为空：给出要操作的图形 id 数组")?;
    let (bw, bh) = (max_x - min_x, max_y - min_y);
    let mut next = scene.clone();
    let mut ordered: Vec<String> = Vec::new();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut last_key = max_index_key(&next);
    let step = |k: usize| (k + 1) as f64;
    for k in 0..count {
        let text = if texts.is_empty() {
            None
        } else {
            Some(texts[k % texts.len()].clone())
        };
        // place 按外框平移；dx/dy 缺一个当 0；都没给就把外框左上角放到空地上。
        // 空地按已经放进去的副本算，不再额外加一份默认宽高。
        let (dx_k, dy_k) = if let Some(p) = place {
            match p {
                "right" => ((bw + GAP) * step(k), 0.0),
                "left" => (-(bw + GAP) * step(k), 0.0),
                "below" => (0.0, (bh + GAP) * step(k)),
                "above" => (0.0, -(bh + GAP) * step(k)),
                _ => (0.0, 0.0),
            }
        } else if dx.is_some() || dy.is_some() {
            (step(k) * dx.unwrap_or(0.0), step(k) * dy.unwrap_or(0.0))
        } else {
            let (fx, fy) = free_slot(&next, bw, bh);
            (fx - min_x, fy - min_y)
        };
        // 这一份里：原 id → 新 id。标签的新 id 也先放进同一张表，再改绑定。
        let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for old_id in &sources {
            map.insert(old_id.clone(), copy_id(old_id, k, &next, &mut used));
        }
        let copy_labels: Vec<(String, Value)> = labels
            .iter()
            .filter(|(container, _)| sources.iter().any(|id| id == container))
            .map(|(container, label)| (container.clone(), label.clone()))
            .collect();
        for (container, label) in &copy_labels {
            let Some(old_label) = id_of(label) else {
                continue;
            };
            let Some(new_container) = map.get(container).cloned() else {
                continue;
            };
            let new_label = label_copy_id(&new_container, &next, &mut used);
            map.insert(old_label.to_string(), new_label);
        }
        let mut group_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for old_id in &sources {
            let Some(src) = scene.shape(old_id) else {
                continue;
            };
            let new_id = map.get(old_id).cloned().unwrap_or_default();
            let pending = stored_pending(scene, old_id);
            let mut copy = src.clone();
            copy["id"] = json!(new_id);
            copy["x"] = json!(num(src, "x") + dx_k);
            copy["y"] = json!(num(src, "y") + dy_k);
            copy["isDeleted"] = json!(false);
            retarget_nested(&mut copy, "startBinding", "elementId", &map);
            retarget_nested(&mut copy, "endBinding", "elementId", &map);
            retarget_nested(&mut copy, "start", "id", &map);
            retarget_nested(&mut copy, "end", "id", &map);
            retarget_bound(&mut copy, &map);
            retarget_container(&mut copy, &map);
            remap_groups(&mut copy, &mut group_map);
            // texts 只改请求里的图形；自动带上的内部箭头保留原标签。
            if let Some(t) = text.as_ref().filter(|_| requested.contains(old_id)) {
                apply_copy_text(&mut copy, t);
            }
            if copy.get("version").is_some() {
                bump(&mut copy);
            }
            assign_z(&mut copy, pending, &mut last_key);
            push_copy(&mut next, copy, pending);
            ordered.push(new_id);
        }
        for (container, label) in &copy_labels {
            let Some(old_label) = id_of(label).map(str::to_string) else {
                continue;
            };
            let Some(new_id) = map.get(&old_label).cloned() else {
                continue;
            };
            let Some(new_container) = map.get(container).cloned() else {
                continue;
            };
            // 标签跟容器进同一个库：容器还是骨架，标签也进 pending。
            let pending = stored_pending(scene, container);
            let mut l = label.clone();
            l["id"] = json!(new_id);
            l["containerId"] = json!(new_container);
            l["x"] = json!(num(label, "x") + dx_k);
            l["y"] = json!(num(label, "y") + dy_k);
            l["isDeleted"] = json!(false);
            retarget_bound(&mut l, &map);
            remap_groups(&mut l, &mut group_map);
            if let Some(t) = text.as_ref().filter(|_| requested.contains(container)) {
                apply_copy_text(&mut l, t);
            }
            if l.get("version").is_some() {
                bump(&mut l);
            }
            assign_z(&mut l, pending, &mut last_key);
            push_copy(&mut next, l, pending);
        }
    }
    next.rev = scene.rev + 1;
    *scene = next;
    let extra_note = if extras.is_empty() {
        String::new()
    } else {
        format!("，其中 {} 个是两端都在选区里的连线", extras.len())
    };
    Ok(ArrangeReport {
        detail: format!(
            "复制了 {count} 份（{} 个图形{extra_note}）：{}",
            sources.len(),
            ordered.join("、")
        ),
        touched: ordered,
    })
}

/// `whiteboard_graph` 的结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphReport {
    pub added: Vec<String>,
    pub detail: String,
    pub unwrapped_item: bool,
}

/// `whiteboard_graph`：`{ nodes, edges?, layout?, columns?, gap_x?, gap_y? }`。
/// layout ∈ row|column|grid|layered；默认 layered（顺着前进边取最长路，回边不改层）。
/// 节点按自己的宽高占位，默认 200×80，间距 60/80。
pub fn apply_graph(scene: &mut Scene, args: &Value) -> Result<GraphReport, String> {
    let (nodes, u_n) = take_array_field(args, "nodes")?;
    let (edges, u_e) = match args.get("edges") {
        None | Some(Value::Null) => (Vec::new(), false),
        Some(_) => take_array_field(args, "edges")?,
    };
    if nodes.is_empty() {
        return Err("nodes 不能为空".into());
    }
    if nodes.len() + edges.len() > MAX_OPS {
        return Err(format!("一次最多改 {MAX_OPS} 个，分几次做"));
    }
    let layout = args
        .get("layout")
        .and_then(Value::as_str)
        .unwrap_or("layered");
    if !["row", "column", "grid", "layered"].contains(&layout) {
        return Err(format!(
            "layout「{layout}」不认识（row|column|grid|layered）"
        ));
    }
    let columns = args
        .get("columns")
        .and_then(num_arg)
        .filter(|c| *c >= 1.0)
        .map(|c| c as usize)
        .unwrap_or(if layout == "grid" { 3 } else { 1 });
    let gap_x = gap_arg(args, "gap_x", GRAPH_GAP_X)?;
    let gap_y = gap_arg(args, "gap_y", GRAPH_GAP_Y)?;

    let mut parsed: Vec<(String, String, String, f64, f64)> = Vec::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        let id = n
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("第 {} 个节点缺 id（nodes[{i}]）", i + 1))?
            .to_string();
        let kind = n.get("type").and_then(Value::as_str).unwrap_or("rectangle");
        if !["rectangle", "ellipse", "diamond", "text"].contains(&kind) {
            return Err(format!(
                "第 {} 个节点 type「{kind}」不支持（nodes[{i}]：rectangle|ellipse|diamond|text）",
                i + 1
            ));
        }
        let text = n
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let w = opt_num(n, "width").unwrap_or(GRAPH_W).max(1.0);
        let h = opt_num(n, "height").unwrap_or(GRAPH_H).max(1.0);
        if parsed.iter().any(|(p, _, _, _, _)| p == &id) {
            return Err(format!("第 {} 个节点 id「{id}」重复（nodes[{i}]）", i + 1));
        }
        if scene.shape(&id).is_some() {
            return Err(format!(
                "第 {} 个节点 id「{id}」已经有了（nodes[{i}]：换一个或先 delete）",
                i + 1
            ));
        }
        parsed.push((id, kind.to_string(), text, w, h));
    }
    let id_set: std::collections::HashSet<&str> =
        parsed.iter().map(|(i, _, _, _, _)| i.as_str()).collect();

    let mut parsed_edges: Vec<(String, String, String, Option<String>)> = Vec::new();
    for (i, e) in edges.iter().enumerate() {
        let from = e
            .get("from")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("第 {} 条边缺 from（edges[{i}]）", i + 1))?;
        let to = e
            .get("to")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("第 {} 条边缺 to（edges[{i}]）", i + 1))?;
        if !id_set.contains(from) && scene.shape(from).is_none() {
            return Err(format!(
                "第 {} 条边 from「{from}」不存在（edges[{i}]）",
                i + 1
            ));
        }
        if !id_set.contains(to) && scene.shape(to).is_none() {
            return Err(format!("第 {} 条边 to「{to}」不存在（edges[{i}]）", i + 1));
        }
        let text = e.get("text").and_then(Value::as_str).map(str::to_string);
        let eid = match e.get("id").and_then(Value::as_str).map(str::trim) {
            Some(id) if !id.is_empty() => id.to_string(),
            Some(_) => return Err(format!("第 {} 条边 id 是空的（edges[{i}]）", i + 1)),
            None => {
                // 默认 id 是 e-<from>-<to>；同 from/to 的第二条边加序号，别撞 id。
                let base = format!("e-{from}-{to}");
                let mut candidate = base.clone();
                let mut n = 2;
                while scene.shape(&candidate).is_some()
                    || parsed_edges.iter().any(|(p, _, _, _)| p == &candidate)
                {
                    candidate = format!("{base}-{n}");
                    n += 1;
                }
                candidate
            }
        };
        parsed_edges.push((eid, from.to_string(), to.to_string(), text));
    }

    // 新图排在现有内容右边（空板才从 100,100 起）：别压在已有图形上。
    let (origin_x, origin_y) = match scene.bounds() {
        Some((_, top, right, _)) => (right + GAP * 2.0, top),
        None => (100.0, 100.0),
    };
    let positions = graph_positions(
        &parsed,
        &parsed_edges,
        &GraphLayout {
            kind: layout.to_string(),
            columns,
            gap_x,
            gap_y,
            origin_x,
            origin_y,
        },
    );
    let mut next = scene.clone();
    let mut added = Vec::new();
    for (i, (id, kind, text, w, h)) in parsed.iter().enumerate() {
        let (x, y) = positions[i];
        let mut shape = json!({
            "type": kind, "id": id, "x": x, "y": y,
            "width": w, "height": h,
        });
        if !text.is_empty() {
            shape["text"] = json!(text);
        }
        let aid = add_one(&mut next, &shape, i)?;
        added.push(aid);
    }
    for (i, (eid, from, to, text)) in parsed_edges.iter().enumerate() {
        if next.shape(eid).is_some() {
            return Err(format!(
                "第 {} 条边 id「{eid}」已经有了（edges[{i}]）",
                i + 1
            ));
        }
        let mut shape = json!({
            "type": "arrow", "id": eid, "from": from, "to": to,
        });
        if let Some(t) = text {
            shape["text"] = json!(t);
        }
        let aid = add_one(&mut next, &shape, i)?;
        added.push(aid);
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(GraphReport {
        detail: format!(
            "已画 {} 个节点、{} 条边（layout={layout}）",
            parsed.len(),
            parsed_edges.len()
        ),
        added,
        unwrapped_item: u_n || u_e,
    })
}

/// `whiteboard_graph` 的排版参数。
struct GraphLayout {
    kind: String,
    columns: usize,
    gap_x: f64,
    gap_y: f64,
    origin_x: f64,
    origin_y: f64,
}

fn graph_positions(
    nodes: &[(String, String, String, f64, f64)],
    edges: &[(String, String, String, Option<String>)],
    cfg: &GraphLayout,
) -> Vec<(f64, f64)> {
    let n = nodes.len();
    let mut pos = vec![(cfg.origin_x, cfg.origin_y); n];
    if n == 0 {
        return pos;
    }
    let sizes: Vec<(f64, f64)> = nodes.iter().map(|(_, _, _, w, h)| (*w, *h)).collect();
    let (gap_x, gap_y) = (cfg.gap_x, cfg.gap_y);
    let (origin_x, origin_y) = (cfg.origin_x, cfg.origin_y);
    // 一行：按各自宽度横排，在这一行的最大高度里垂直居中。空隙只加在图形之间。
    let pack_row = |indices: &[usize], y0: f64, pos: &mut Vec<(f64, f64)>| {
        let row_h = indices.iter().map(|&i| sizes[i].1).fold(0.0, f64::max);
        let mut x = origin_x;
        for (slot, &i) in indices.iter().enumerate() {
            let (w, h) = sizes[i];
            pos[i] = (x, y0 + (row_h - h) / 2.0);
            x += w;
            if slot + 1 != indices.len() {
                x += gap_x;
            }
        }
    };
    match cfg.kind.as_str() {
        "row" => {
            let all: Vec<usize> = (0..n).collect();
            pack_row(&all, origin_y, &mut pos);
        }
        "column" => {
            let col_w = sizes.iter().map(|(w, _)| *w).fold(0.0, f64::max);
            let mut y = origin_y;
            for i in 0..n {
                let (w, h) = sizes[i];
                pos[i] = (origin_x + (col_w - w) / 2.0, y);
                y += h;
                if i + 1 != n {
                    y += gap_y;
                }
            }
        }
        "grid" => {
            let cols = cfg.columns.max(1);
            let rows = n.div_ceil(cols);
            let mut col_w = vec![0.0f64; cols];
            let mut row_h = vec![0.0f64; rows];
            for (i, &(w, h)) in sizes.iter().enumerate() {
                let c = i % cols;
                let r = i / cols;
                col_w[c] = col_w[c].max(w);
                row_h[r] = row_h[r].max(h);
            }
            for i in 0..n {
                let c = i % cols;
                let r = i / cols;
                let (w, h) = sizes[i];
                let x = origin_x
                    + col_w.iter().take(c).sum::<f64>()
                    + gap_x * c as f64
                    + (col_w[c] - w) / 2.0;
                let y = origin_y
                    + row_h.iter().take(r).sum::<f64>()
                    + gap_y * r as f64
                    + (row_h[r] - h) / 2.0;
                pos[i] = (x, y);
            }
        }
        _ => {
            let idx: std::collections::HashMap<&str, usize> = nodes
                .iter()
                .enumerate()
                .map(|(i, (id, _, _, _, _))| (id.as_str(), i))
                .collect();
            let mut adj = vec![Vec::new(); n];
            for (_, from, to, _) in edges {
                if let (Some(&a), Some(&b)) = (idx.get(from.as_str()), idx.get(to.as_str())) {
                    if a != b {
                        adj[a].push(b);
                    }
                }
            }
            // 灰节点上的边是回边，不进分层；树边 / 前向边 / 交叉边留下，再取最长路。
            let mut color = vec![0u8; n];
            let mut forward: Vec<(usize, usize)> = Vec::new();
            fn walk(
                u: usize,
                adj: &[Vec<usize>],
                color: &mut [u8],
                forward: &mut Vec<(usize, usize)>,
            ) {
                color[u] = 1;
                for &v in &adj[u] {
                    if color[v] == 1 {
                        continue;
                    }
                    forward.push((u, v));
                    if color[v] == 0 {
                        walk(v, adj, color, forward);
                    }
                }
                color[u] = 2;
            }
            for i in 0..n {
                if color[i] == 0 {
                    walk(i, &adj, &mut color, &mut forward);
                }
            }
            let mut indeg = vec![0usize; n];
            let mut out = vec![Vec::new(); n];
            for (u, v) in forward {
                out[u].push(v);
                indeg[v] += 1;
            }
            let mut level = vec![0usize; n];
            let mut queued = vec![false; n];
            let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
            for i in 0..n {
                if indeg[i] == 0 {
                    queue.push_back(i);
                    queued[i] = true;
                }
            }
            while let Some(u) = queue.pop_front() {
                for &v in &out[u] {
                    level[v] = level[v].max(level[u] + 1);
                    indeg[v] = indeg[v].saturating_sub(1);
                    if indeg[v] == 0 && !queued[v] {
                        queued[v] = true;
                        queue.push_back(v);
                    }
                }
            }
            if queued.contains(&false) {
                let extra = level.iter().copied().max().unwrap_or(0) + 1;
                for i in 0..n {
                    if !queued[i] {
                        level[i] = extra;
                    }
                }
            }
            let max_lvl = level.iter().copied().max().unwrap_or(0);
            let mut per_level: Vec<Vec<usize>> = vec![Vec::new(); max_lvl + 1];
            for (i, &l) in level.iter().enumerate() {
                per_level[l].push(i);
            }
            let mut y = origin_y;
            for row in &per_level {
                if row.is_empty() {
                    continue;
                }
                let row_h = row.iter().map(|&i| sizes[i].1).fold(0.0, f64::max);
                pack_row(row, y, &mut pos);
                y += row_h + gap_y;
            }
        }
    }
    pos
}

// ---- 给模型看的 ----

/// 一行一个图形；容器里的文字并进容器的标签，箭头写两端。
pub fn summary(scene: &Scene) -> (String, Value) {
    summary_with(scene, None, true)
}

/// 分层读：`only` 限定只看哪几个图形；`with_patches` 决定给不给可复制的 patches。
/// 场景一大，整板一股脑倒出来既贵又容易用错：先总览，再按 id / 关系取。
pub fn summary_with(scene: &Scene, only: Option<&[String]>, with_patches: bool) -> (String, Value) {
    let label_of = |id: &str| {
        scene
            .live()
            .find(|t| t.get("containerId").and_then(Value::as_str) == Some(id))
            .and_then(|t| t.get("text").and_then(Value::as_str))
            .map(str::to_string)
    };
    let keep = |id: &str| only.map(|ids| ids.iter().any(|x| x == id)).unwrap_or(true);
    let mut lines = Vec::new();
    let mut shapes = Vec::new();
    for e in scene.live() {
        if e.get("containerId").and_then(Value::as_str).is_some() {
            continue;
        }
        let id = id_of(e).unwrap_or_default();
        if !keep(id) {
            continue;
        }
        let kind = kind_of(e);
        let text = if kind == "text" {
            e.get("text").and_then(Value::as_str).map(str::to_string)
        } else {
            label_of(id)
        };
        let from = e.pointer("/startBinding/elementId").and_then(Value::as_str);
        let to = e.pointer("/endBinding/elementId").and_then(Value::as_str);
        lines.push(describe(kind, id, e, text.as_deref(), from, to, false));
        shapes.push(shape_json(kind, id, e, text.as_deref(), from, to, false));
    }
    for p in &scene.pending {
        let id = id_of(p).unwrap_or_default();
        if !keep(id) {
            continue;
        }
        let kind = kind_of(p);
        let text = p
            .get("text")
            .or_else(|| p.pointer("/label/text"))
            .and_then(Value::as_str);
        let from = p.pointer("/start/id").and_then(Value::as_str);
        let to = p.pointer("/end/id").and_then(Value::as_str);
        lines.push(describe(kind, id, p, text, from, to, true));
        shapes.push(shape_json(kind, id, p, text, from, to, true));
    }
    let head = if lines.is_empty() {
        if only.is_some() {
            format!("白板第 {} 版：这几个图形都没有（或还没画）。", scene.rev)
        } else {
            format!("白板是空的（第 {} 版）。", scene.rev)
        }
    } else {
        match scene.bounds() {
            Some((x1, y1, x2, y2)) => format!(
                "白板第 {} 版，{} 个图形（范围 {:.0},{:.0} 到 {:.0},{:.0}）：",
                scene.rev,
                lines.len(),
                x1,
                y1,
                x2,
                y2
            ),
            None => format!("白板第 {} 版，{} 个图形：", scene.rev, lines.len()),
        }
    };
    let mut text = head;
    for l in &lines {
        text.push('\n');
        text.push_str(l);
    }
    if !scene.pending.is_empty() {
        text.push_str("\n（标「待画」的是刚加的：用户打开白板界面后才排好版、连好线。）");
    }
    let mut out = json!({ "rev": scene.rev, "shapes": shapes });
    if with_patches {
        let patches: Vec<Value> = out["shapes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                let mut p = serde_json::Map::new();
                for key in [
                    "id", "type", "x", "y", "width", "height", "text", "stroke", "fill", "from",
                    "to",
                ] {
                    if let Some(v) = s.get(key) {
                        if !v.is_null() {
                            p.insert(key.into(), v.clone());
                        }
                    }
                }
                Value::Object(p)
            })
            .collect();
        out["patches"] = json!(patches);
    }
    (text, out)
}

/// 给模型看的几行：只讲这几个图形。
/// 每次工具都回全量场景（几十行 + shapes + patches 三份重复）很快把上下文吃满，
/// 全量清单交给 `whiteboard_read`。
pub fn lines_for(scene: &Scene, ids: &[String]) -> String {
    let mut out = String::new();
    for id in ids {
        let Some(e) = scene.shape(id) else {
            continue;
        };
        let kind = kind_of(e);
        let text = shape_label_text(scene, e);
        let from = e
            .pointer("/startBinding/elementId")
            .or_else(|| e.pointer("/start/id"))
            .and_then(Value::as_str);
        let to = e
            .pointer("/endBinding/elementId")
            .or_else(|| e.pointer("/end/id"))
            .and_then(Value::as_str);
        let pending = scene.pending.iter().any(|p| id_of(p) == Some(id.as_str()));
        out.push_str(&describe(kind, id, e, text.as_deref(), from, to, pending));
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn describe(
    kind: &str,
    id: &str,
    e: &Value,
    text: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    pending: bool,
) -> String {
    let mut s = format!("- {kind} id={id}");
    if let Some(t) = text.filter(|t| !t.is_empty()) {
        s.push_str(&format!("「{}」", clip(t, 60)));
    }
    if kind == "arrow" || kind == "line" {
        if from.is_some() || to.is_some() {
            s.push_str(&format!(" {} → {}", from.unwrap_or("?"), to.unwrap_or("?")));
        }
    } else {
        s.push_str(&format!(" ({:.0},{:.0})", num(e, "x"), num(e, "y")));
        if kind != "text" || num(e, "width") > 0.0 {
            s.push_str(&format!(" {:.0}×{:.0}", num(e, "width"), num(e, "height")));
        }
    }
    if let Some(c) = e.get("strokeColor").and_then(Value::as_str) {
        s.push_str(&format!(" 线{c}"));
    }
    if let Some(c) = e
        .get("backgroundColor")
        .and_then(Value::as_str)
        .filter(|c| *c != "transparent")
    {
        s.push_str(&format!(" 填{c}"));
    }
    if pending {
        s.push_str(" 待画");
    }
    s
}

fn shape_json(
    kind: &str,
    id: &str,
    e: &Value,
    text: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    pending: bool,
) -> Value {
    json!({
        "id": id, "type": kind, "text": text,
        "x": num(e, "x"), "y": num(e, "y"), "width": num(e, "width"), "height": num(e, "height"),
        "stroke": e.get("strokeColor"), "fill": e.get("backgroundColor"),
        "from": from, "to": to, "pending": pending,
    })
}

// ---- 小工具 ----

fn id_of(e: &Value) -> Option<&str> {
    e.get("id").and_then(Value::as_str)
}

fn kind_of(e: &Value) -> &str {
    e.get("type").and_then(Value::as_str).unwrap_or("?")
}

fn is_deleted(e: &Value) -> bool {
    e.get("isDeleted").and_then(Value::as_bool).unwrap_or(false)
}

fn num(e: &Value, key: &str) -> f64 {
    e.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn opt_num(e: &Value, key: &str) -> Option<f64> {
    match e.get(key) {
        Some(Value::Number(n)) => n.as_f64().filter(|v| v.is_finite()),
        // 模型常把数字写成字符串（"1500"）：认下来，别当没传。
        Some(Value::String(s)) => s.trim().parse::<f64>().ok().filter(|v| v.is_finite()),
        _ => None,
    }
}

/// 写了坐标却不是数字：说清楚。默默挪到默认位置最难查。
fn check_num(shape: &Value, key: &str, where_: &str) -> Result<(), String> {
    if let Some(v) = shape.get(key) {
        if !v.is_null() && opt_num(shape, key).is_none() {
            return Err(format!(
                "{where_} 的 {key} 要是数字（收到的是{}）",
                value_kind_hint(v)
            ));
        }
    }
    Ok(())
}

/// 顶层数字参数：模型端会把数字写成字符串（"2"），两种都认。
fn num_arg(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|x| x.is_finite()),
        _ => None,
    }
}

/// 顶层布尔参数：模型端可能写成 "true" / "false"。
fn bool_arg(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|x| x != 0.0),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" | "" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// 形状对象里认识的字段。
const SHAPE_FIELDS: &[&str] = &[
    "type",
    "id",
    "x",
    "y",
    "width",
    "height",
    "text",
    "stroke",
    "fill",
    "strokeStyle",
    "from",
    "to",
    "fontSize",
    "points",
];

/// 图形对象里不认识的键：模型写错字段名（`label` / `caption`）时至少回一句，
/// 别让图形变成空框还没人发现。
fn unknown_fields(shape: &Value) -> Vec<String> {
    let Some(m) = shape.as_object() else {
        return Vec::new();
    };
    m.keys()
        .filter(|k| !SHAPE_FIELDS.contains(&k.as_str()))
        .cloned()
        .collect()
}

fn clip(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out.replace('\n', " ")
}

fn bump(e: &mut Value) {
    e["version"] = json!(num(e, "version") as u64 + 1);
    e["versionNonce"] = json!(nonce());
    e["updated"] = json!(now_ms());
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Excalidraw 的 `versionNonce` / `seed`：31 位正整数，只要求「大概率不同」。
fn nonce() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0x9e37_79b9);
    let mut x = now_ms() ^ COUNTER.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x & 0x7fff_ffff
}

fn fresh_id(scene: &Scene) -> String {
    (1..)
        .map(|n| format!("s{n}"))
        .find(|id| !scene.id_taken(id))
        .expect("unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(id: &str, kind: &str, x: f64, version: u64) -> Value {
        json!({ "id": id, "type": kind, "x": x, "y": 0.0, "width": 100.0, "height": 50.0,
                "version": version, "versionNonce": 10, "isDeleted": false, "strokeColor": "#1e1e1e" })
    }

    #[test]
    fn agent_adds_go_to_pending_and_show_up_in_the_summary() {
        let mut s = Scene::default();
        let r = apply_edit(
            &mut s,
            &json!({ "add": [
                { "type": "rectangle", "id": "login", "text": "登录", "fill": "#a5d8ff" },
                { "type": "rectangle", "text": "首页" },
                { "type": "arrow", "from": "login", "to": "s1", "text": "成功" }
            ] }),
        )
        .unwrap();
        assert_eq!(r.added, ["login", "s1", "s2"]);
        assert_eq!(s.rev, 1);
        assert_eq!(s.pending.len(), 3);
        assert_eq!(s.pending[0]["label"]["text"], "登录");
        assert_eq!(s.pending[0]["fillStyle"], "solid");
        assert_eq!(s.pending[2]["start"]["id"], "login");
        // 没给位置的自动找空位：不叠在一起（横着排开）。
        let (ax, ay) = (num(&s.pending[0], "x"), num(&s.pending[0], "y"));
        let (bx, by) = (num(&s.pending[1], "x"), num(&s.pending[1], "y"));
        assert!(
            !(ax < bx + 180.0 && ax + 180.0 > bx && ay < by + 80.0 && ay + 80.0 > by),
            "没给位置的不能重叠：{ax},{ay} vs {bx},{by}"
        );
        let (text, data) = summary(&s);
        assert!(text.contains("rectangle id=login「登录」"), "{text}");
        assert!(text.contains("arrow id=s2「成功」 login → s1"), "{text}");
        assert!(text.contains("待画"), "{text}");
        assert_eq!(data["shapes"].as_array().unwrap().len(), 3);
    }

    /// 两端都给了的箭头：从一个框的右边连到另一个框的左边，不按默认位置排。
    #[test]
    fn arrows_between_shapes_get_edge_to_edge_geometry() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [
                { "type": "rectangle", "id": "a", "x": 0, "y": 0, "width": 100, "height": 50 },
                { "type": "rectangle", "id": "b", "x": 300, "y": 0, "width": 100, "height": 50 },
                { "type": "arrow", "id": "e", "from": "a", "to": "b" }
            ] }),
        )
        .unwrap();
        let e = &s.pending[2];
        assert_eq!(e["x"], 108.0, "从 a 的右边让出 8");
        assert_eq!(e["y"], 25.0, "在中线上");
        assert_eq!(e["points"], json!([[0.0, 0.0], [184.0, 0.0]]));
        assert_eq!(e["start"]["id"], "a");
    }

    /// 重画已删掉的带标签的框：框里那段字的墓碑一并让位，下限也要压过它（标签墓碑版本更高时）。
    /// 回归：只看框自己的墓碑时，界面新画的 `<id>-label` 版本低，合并时被墓碑删掉。
    /// 还活着、containerId 也指向这个 id 的孤儿文字不是墓碑，不动。
    #[test]
    fn reusing_an_id_also_clears_its_label_tombstone() {
        let mut boxed = full("a", "rectangle", 0.0, 7);
        boxed["isDeleted"] = json!(true);
        let mut label = full("a-label", "text", 0.0, 12);
        label["containerId"] = json!("a");
        label["isDeleted"] = json!(true);
        let mut orphan = full("a-note", "text", 0.0, 20);
        orphan["containerId"] = json!("a");
        let mut s = Scene {
            elements: vec![boxed, label, orphan, full("b", "rectangle", 300.0, 3)],
            ..Default::default()
        };
        apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "rectangle", "id": "a", "text": "又是 a" }] }),
        )
        .unwrap();
        let ids: Vec<_> = s.elements.iter().filter_map(id_of).collect();
        assert_eq!(ids, ["a-note", "b"], "框和标签的墓碑让位，别的不动");
        assert_eq!(
            s.pending[0]["customData"]["dockMinVersion"], 13,
            "压过标签墓碑（12）"
        );
    }

    /// 清空后用同样的 id 重画：墓碑让位，新元素记下要比墓碑高的版本。
    #[test]
    fn cleared_ids_can_be_reused() {
        let mut s = Scene {
            elements: vec![full("a", "rectangle", 0.0, 7)],
            ..Default::default()
        };
        apply_edit(
            &mut s,
            &json!({ "clear": true, "add": [{ "type": "rectangle", "id": "a", "text": "又是 a" }] }),
        )
        .unwrap();
        assert!(s.elements.is_empty(), "墓碑让位");
        assert!(
            s.pending[0]["customData"]["dockMinVersion"]
                .as_u64()
                .unwrap()
                > 8,
            "清空时的墓碑版本 +1 要压过旧版本"
        );
        assert!(
            apply_edit(
                &mut s,
                &json!({ "add": [{ "type": "rectangle", "id": "a" }] })
            )
            .is_err(),
            "还在的不能重名"
        );
    }

    /// 文件坏了：原文件改名留着，从空白板开始并说一句；不能当空的直接写回去。
    #[test]
    fn corrupt_scene_is_set_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scene.json");
        std::fs::write(&path, "{ 写了一半").unwrap();
        let (scene, note) = Scene::load(&path).unwrap();
        assert_eq!(scene, Scene::default());
        assert!(note.unwrap().contains("另存为"));
        assert!(!path.exists());
        let kept: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("scene.json.corrupt-")
            })
            .collect();
        assert_eq!(kept.len(), 1, "坏文件要留着");
        assert_eq!(
            std::fs::read_to_string(kept[0].path()).unwrap(),
            "{ 写了一半"
        );
    }

    /// 没人用的图片要回收：只留还在的图片元素、一天内删掉的图片元素引用的。
    #[test]
    fn unused_files_are_collected() {
        let mut s = Scene::default();
        let mut live = full("img1", "image", 0.0, 1);
        live["fileId"] = json!("f-live");
        let mut old = full("img2", "image", 0.0, 2);
        old["fileId"] = json!("f-old");
        old["isDeleted"] = json!(true);
        old["updated"] = json!(1);
        s.elements = vec![live, old];
        s.files = json!({ "f-live": {}, "f-old": {}, "f-orphan": {} });
        s.gc_files();
        let kept: Vec<&String> = s.files.as_object().unwrap().keys().collect();
        assert_eq!(kept, ["f-live"]);
    }

    /// 界面存回来的元素至少要有 id、type、数字版本，缺了的不收。
    #[test]
    fn saves_skip_malformed_elements() {
        let mut s = Scene::default();
        let changed = merge_save(
            &mut s,
            &json!({ "elements": [{ "id": "a" }, { "id": "b", "type": "rectangle" }, full("c", "rectangle", 0.0, 1)] }),
        )
        .unwrap();
        assert!(changed);
        let ids: Vec<&str> = s.elements.iter().filter_map(id_of).collect();
        assert_eq!(ids, ["c"]);
    }

    #[test]
    fn bad_edits_change_nothing() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "rectangle", "id": "a" }] }),
        )
        .unwrap();
        let before = s.clone();
        for bad in [
            json!({ "add": [{ "type": "star" }] }),
            json!({ "add": [{ "type": "rectangle", "id": "a" }] }),
            json!({ "add": [{ "type": "arrow", "from": "nope" }] }),
            json!({ "add": [{ "type": "rectangle", "stroke": "red; x" }] }),
            json!({ "update": [{ "id": "ghost", "x": 1 }] }),
            json!({ "delete": ["ghost"] }),
            json!({}),
        ] {
            assert!(apply_edit(&mut s, &bad).is_err(), "{bad}");
            assert_eq!(s, before, "失败的编辑不能改一半：{bad}");
        }
    }

    #[test]
    fn updates_bump_full_elements_and_move_labels_and_reroute_arrows() {
        let mut s = Scene::default();
        let mut label = full("a-t", "text", 30.0, 1);
        label["containerId"] = json!("a");
        label["text"] = json!("旧");
        let mut arrow = full("e", "arrow", 100.0, 1);
        arrow["startBinding"] = json!({ "elementId": "a" });
        arrow["endBinding"] = json!({ "elementId": "b" });
        s.elements = vec![
            full("a", "rectangle", 0.0, 3),
            label,
            full("b", "ellipse", 300.0, 1),
            arrow,
        ];
        apply_edit(
            &mut s,
            &json!({ "update": [{ "id": "a", "x": 50, "text": "新", "fill": "#ffc9c9" }] }),
        )
        .unwrap();
        let a = &s.elements[0];
        assert_eq!(a["version"], 4);
        assert_eq!(a["x"], 50.0);
        assert_eq!(a["backgroundColor"], "#ffc9c9");
        assert_eq!(s.elements[1]["text"], "新");
        assert_eq!(s.elements[1]["version"], 2, "标签跟着出新版");
        assert!(
            s.elements
                .iter()
                .all(|e| e["id"] != "e" || e.get("isDeleted") == Some(&json!(true))),
            "原箭头元素已让位给同 id 骨架"
        );
        assert_eq!(s.pending.len(), 1, "箭头退回骨架重新连");
        assert_eq!(s.pending[0]["id"], "e", "箭头 id 保持不变");
        assert_eq!(s.pending[0]["start"]["id"], "a");
        assert_eq!(s.pending[0]["end"]["id"], "b");

        // 给没有标签的容器加标签：新绑一段文字。
        apply_edit(
            &mut s,
            &json!({ "update": [{ "id": "b", "text": "终点" }] }),
        )
        .unwrap();
        let t = s.elements.iter().find(|e| e["containerId"] == "b").unwrap();
        assert_eq!(t["text"], "终点");
        let b = s.elements.iter().find(|e| e["id"] == "b").unwrap();
        assert_eq!(b["boundElements"][0]["id"], t["id"]);
    }

    #[test]
    fn delete_and_clear_leave_tombstones() {
        let mut s = Scene::default();
        let mut label = full("a-t", "text", 0.0, 1);
        label["containerId"] = json!("a");
        s.elements = vec![
            full("a", "rectangle", 0.0, 1),
            label,
            full("b", "ellipse", 0.0, 1),
        ];
        apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "text", "id": "p", "text": "x" }] }),
        )
        .unwrap();
        apply_edit(&mut s, &json!({ "delete": ["a", "p"] })).unwrap();
        assert_eq!(s.elements[0]["isDeleted"], true);
        assert_eq!(s.elements[1]["isDeleted"], true, "标签跟着删");
        assert!(s.pending.is_empty());
        apply_edit(&mut s, &json!({ "clear": true })).unwrap();
        assert!(s.elements.iter().all(|e| e["isDeleted"] == true));
        assert!(summary(&s).0.contains("空的"));
    }

    #[test]
    fn saves_merge_by_version_and_consume_pending() {
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 5),
                full("b", "rectangle", 0.0, 2),
            ],
            ..Default::default()
        };
        apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "rectangle", "id": "c" }] }),
        )
        .unwrap();
        let rev = s.rev;
        let changed = merge_save(
            &mut s,
            &json!({
                "elements": [full("a", "rectangle", 9.0, 4), full("b", "rectangle", 7.0, 3), full("c", "rectangle", 1.0, 1)],
                "consumed": ["c"],
                "files": { "f1": { "dataURL": "data:," } }
            }),
        )
        .unwrap();
        assert!(changed);
        assert_eq!(s.rev, rev + 1);
        assert_eq!(s.elements[0]["x"], 0.0, "旧版本不覆盖");
        assert_eq!(s.elements[1]["x"], 7.0, "新版本覆盖");
        assert_eq!(s.elements.len(), 3);
        assert!(s.pending.is_empty());
        assert!(s.files.get("f1").is_some());
        assert!(!merge_save(
            &mut s,
            &json!({ "elements": [full("a", "rectangle", 9.0, 4)] })
        )
        .unwrap());
    }

    #[test]
    fn scene_round_trips_and_paths_follow_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = scene_path(dir.path(), Some("/Users/me/proj"));
        let name = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            name.starts_with("Users-me-proj-") && name.len() == "Users-me-proj-".len() + 8,
            "{name}"
        );
        // 会话目录那套名字会撞的几对，带上哈希就分开了。
        for (a, b) in [("/p/SunGods-中台", "/p/SunGods"), ("/a/b-c", "/a/b/c")] {
            assert_ne!(
                scene_path(dir.path(), Some(a)),
                scene_path(dir.path(), Some(b)),
                "{a} vs {b}"
            );
        }
        assert!(scene_path(dir.path(), None).ends_with("default/scene.json"));
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "ellipse", "text": "x" }] }),
        )
        .unwrap();
        s.save(&path).unwrap();
        assert_eq!(Scene::load(&path).unwrap().0, s);
        assert_eq!(
            Scene::load(&dir.path().join("missing.json")).unwrap().0,
            Scene::default()
        );
    }

    #[test]
    fn unwraps_exactly_item_wrapper_and_mentions_it() {
        let mut s = Scene::default();
        let r = apply_edit(
            &mut s,
            &json!({ "add": { "item": [{ "type": "rectangle", "id": "a", "text": "x" }] } }),
        )
        .unwrap();
        assert!(r.unwrapped_item);
        assert_eq!(r.added, ["a"]);
        assert_eq!(s.pending[0]["id"], "a");
    }

    #[test]
    fn does_not_unwrap_item_when_other_keys_present() {
        let mut s = Scene::default();
        let err = apply_edit(
            &mut s,
            &json!({ "add": { "item": [{ "type": "rectangle" }], "extra": 1 } }),
        )
        .unwrap_err();
        assert!(err.contains("add 要是数组"), "{err}");
        assert!(err.contains("\"item\""), "{err}");
        assert!(err.contains("\"extra\""), "{err}");
        assert!(err.contains("去掉 item 这层包装"), "{err}");
        assert!(err.contains("\"add\": [{\"type\":\"rectangle\""), "{err}");
        assert_eq!(s, Scene::default());
    }

    #[test]
    fn missing_type_error_includes_index_and_allowed_set() {
        let mut s = Scene::default();
        // add[0] ok, add[1] ok, add[2] missing type → 第 3 个 / add[2]
        let err = apply_edit(
            &mut s,
            &json!({ "add": [
                { "type": "rectangle", "id": "a" },
                { "type": "ellipse", "id": "b" },
                { "id": "c", "x": 0 }
            ] }),
        )
        .unwrap_err();
        assert!(err.contains("第 3 个图形缺 type"), "{err}");
        assert!(err.contains("add[2]"), "{err}");
        assert!(
            err.contains("rectangle|ellipse|diamond|text|arrow|line"),
            "{err}"
        );
        assert_eq!(s, Scene::default(), "原子性：失败不改");
    }

    #[test]
    fn summary_returns_reusable_patches() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "rectangle", "id": "a", "text": "登录", "x": 10, "y": 20 }] }),
        )
        .unwrap();
        let (_, data) = summary(&s);
        let patches = data["patches"].as_array().unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0]["id"], "a");
        assert_eq!(patches[0]["type"], "rectangle");
        assert_eq!(patches[0]["text"], "登录");
        assert_eq!(patches[0]["x"], 10.0);
        assert!(
            patches[0].get("pending").is_none(),
            "patch 不含 pending 标记"
        );
    }

    #[test]
    fn group_align_style_layer_fit_text_happy_paths() {
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "rectangle", 200.0, 1),
                full("c", "ellipse", 50.0, 1),
            ],
            ..Default::default()
        };
        s.elements[0]["y"] = json!(10.0);
        s.elements[0]["height"] = json!(40.0);
        s.elements[1]["y"] = json!(80.0);
        s.elements[1]["text"] = json!("很长的中文标签需要更宽");
        // pending label path: put text via bound — use update after; for fit_text use label on pending-less full element
        // Give a a label element
        let mut label = full("a-t", "text", 0.0, 1);
        label["containerId"] = json!("a");
        label["text"] = json!("短");
        s.elements.push(label);

        let g = apply_group(&mut s, &json!({ "ids": ["a", "b"] })).unwrap();
        assert!(g.detail.contains("合成组"));
        assert_eq!(s.elements[0]["groupIds"][0], s.elements[1]["groupIds"][0]);

        let al = apply_align(&mut s, &json!({ "ids": ["a", "b"], "align": "left" })).unwrap();
        assert!(al.detail.contains("left"));
        assert_eq!(s.elements[0]["x"], s.elements[1]["x"]);

        let st = apply_style(
            &mut s,
            &json!({ "ids": ["a", "b"], "stroke": "#ff0000", "fill": "#00ff00" }),
        )
        .unwrap();
        assert!(st.detail.contains("样式"));
        assert_eq!(s.elements[0]["strokeColor"], "#ff0000");
        assert_eq!(s.elements[1]["backgroundColor"], "#00ff00");

        let ly = apply_layer(&mut s, &json!({ "ids": ["a"], "to": "front" })).unwrap();
        assert!(ly.detail.contains("front"));
        // index 和数组顺序一起改：front 之后 a 排在最后。
        let z = |id: &str| {
            s.elements.iter().find(|e| e["id"] == id).unwrap()["index"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert!(z("a") > z("b"), "a 要在最上：{} vs {}", z("a"), z("b"));
        let pos_a = s.elements.iter().position(|e| e["id"] == "a").unwrap();
        assert_eq!(pos_a, s.elements.len() - 1, "front 之后 a 在数组末尾");

        // fit_text on b: add a label first via update
        apply_edit(
            &mut s,
            &json!({ "update": [{ "id": "b", "text": "很长的中文标签需要更宽" }] }),
        )
        .unwrap();
        let old_w = num(s.elements.iter().find(|e| e["id"] == "b").unwrap(), "width");
        let ft = apply_fit_text(&mut s, &json!({ "ids": ["b"] })).unwrap();
        assert!(ft.detail.contains("适配"));
        let new_w = num(s.elements.iter().find(|e| e["id"] == "b").unwrap(), "width");
        assert!(new_w > old_w, "width {old_w} -> {new_w}");
    }

    #[test]
    fn moving_a_node_keeps_connected_arrow_id() {
        let mut s = Scene::default();
        let mut arrow = full("edge", "arrow", 100.0, 1);
        arrow["startBinding"] = json!({ "elementId": "a" });
        arrow["endBinding"] = json!({ "elementId": "b" });
        s.elements = vec![
            full("a", "rectangle", 0.0, 1),
            full("b", "ellipse", 300.0, 1),
            arrow,
        ];
        apply_edit(&mut s, &json!({ "update": [{ "id": "a", "x": 40 }] })).unwrap();
        assert_eq!(s.pending.len(), 1);
        assert_eq!(s.pending[0]["id"], "edge", "箭头 id 不能变成 edge~N");
        assert_eq!(s.pending[0]["start"]["id"], "a");
        assert_eq!(s.pending[0]["end"]["id"], "b");
        let (_, data) = summary(&s);
        let ids: Vec<_> = data["shapes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s.get("id").and_then(Value::as_str))
            .collect();
        assert!(ids.contains(&"edge"), "{ids:?}");
    }

    #[test]
    fn distribute_horizontal_spaces_middle_evenly() {
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "rectangle", 50.0, 1),
                full("c", "rectangle", 400.0, 1),
            ],
            ..Default::default()
        };
        // widths are 100; span from 0 to 500; one gap → middle at 200
        apply_distribute(
            &mut s,
            &json!({ "ids": ["a", "b", "c"], "direction": "horizontal" }),
        )
        .unwrap();
        let bx = num(s.elements.iter().find(|e| e["id"] == "b").unwrap(), "x");
        assert!((bx - 200.0).abs() < 1e-6, "middle x={bx}");
        assert_eq!(
            s.elements.iter().find(|e| e["id"] == "a").unwrap()["x"],
            0.0
        );
        assert_eq!(
            s.elements.iter().find(|e| e["id"] == "c").unwrap()["x"],
            400.0
        );
    }

    #[test]
    fn layout_grid_arranges_into_columns() {
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 10.0, 1),
                full("b", "rectangle", 20.0, 1),
                full("c", "rectangle", 30.0, 1),
                full("d", "rectangle", 40.0, 1),
            ],
            ..Default::default()
        };
        for e in &mut s.elements {
            e["y"] = json!(5.0);
        }
        apply_layout(
            &mut s,
            &json!({ "ids": ["a", "b", "c", "d"], "columns": 2, "gap_x": 20.0, "gap_y": 30.0 }),
        )
        .unwrap();
        let pos = |id: &str| {
            let e = s.elements.iter().find(|e| e["id"] == id).unwrap();
            (num(e, "x"), num(e, "y"))
        };
        let (ax, ay) = pos("a");
        let (bx, by) = pos("b");
        let (cx, cy) = pos("c");
        assert!((bx - ax).abs() > 50.0, "b to the right of a: {ax},{bx}");
        assert!((by - ay).abs() < 1e-6, "same row");
        assert!((cx - ax).abs() < 1e-6, "c under a");
        assert!(cy > ay + 40.0, "c lower");
    }

    #[test]
    fn place_puts_shape_to_the_right_of_ref() {
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "rectangle", 500.0, 1),
            ],
            ..Default::default()
        };
        apply_place(
            &mut s,
            &json!({ "id": "b", "ref": "a", "where": "right", "gap": 40.0 }),
        )
        .unwrap();
        let a = s.elements.iter().find(|e| e["id"] == "a").unwrap();
        let b = s.elements.iter().find(|e| e["id"] == "b").unwrap();
        let expected = num(a, "x") + num(a, "width") + 40.0;
        assert!((num(b, "x") - expected).abs() < 1e-6, "b.x={}", num(b, "x"));
    }

    #[test]
    fn graph_layered_creates_nodes_and_arrows_at_distinct_positions() {
        let mut s = Scene::default();
        let r = apply_graph(
            &mut s,
            &json!({
                "nodes": [
                    { "id": "start", "text": "开始" },
                    { "id": "mid", "text": "处理" },
                    { "id": "end", "text": "结束" }
                ],
                "edges": [
                    { "from": "start", "to": "mid", "text": "下一步" },
                    { "from": "mid", "to": "end" }
                ],
                "layout": "layered"
            }),
        )
        .unwrap();
        assert_eq!(r.added.len(), 5);
        assert!(s.pending.iter().any(|p| p["id"] == "start"));
        assert!(s.pending.iter().any(|p| p["id"] == "e-start-mid"));
        let ys: Vec<f64> = ["start", "mid", "end"]
            .iter()
            .map(|id| num(s.pending.iter().find(|p| p["id"] == *id).unwrap(), "y"))
            .collect();
        assert!(
            ys[0] < ys[1] && ys[1] < ys[2],
            "layered top-to-bottom: {ys:?}"
        );
        let (_, data) = summary(&s);
        assert!(data["patches"].as_array().unwrap().len() >= 5);
    }

    /// 模型移动节点后，界面把旧箭头存回来：按版本比较，旧的赢不了 → 不会出现两个同 id 的箭头。
    #[test]
    fn old_arrow_saved_back_does_not_duplicate_the_id() {
        let mut arrow = full("e", "arrow", 110.0, 5);
        arrow["startBinding"] = json!({ "elementId": "a" });
        arrow["endBinding"] = json!({ "elementId": "b" });
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 3),
                full("b", "ellipse", 300.0, 1),
                arrow,
            ],
            ..Default::default()
        };
        apply_edit(&mut s, &json!({ "update": [{ "id": "a", "x": 50 }] })).unwrap();
        // 界面那边动过那个箭头（版本 +1）后存回来。
        let mut ui = full("e", "arrow", 999.0, 6);
        ui["startBinding"] = json!({ "elementId": "a" });
        ui["endBinding"] = json!({ "elementId": "b" });
        merge_save(&mut s, &json!({ "elements": [ui] })).unwrap();
        let (_, data) = summary(&s);
        let ids: Vec<_> = data["shapes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.get("id").and_then(Value::as_str))
            .collect();
        assert_eq!(
            ids.iter().filter(|i| **i == "e").count(),
            1,
            "同一个 id 不该出现两次：{ids:?}"
        );
    }

    /// 只改尺寸不该重连箭头（箭头绑在容器上，界面会自己跟）。
    #[test]
    fn fit_text_does_not_reconnect_arrows() {
        let mut arrow = full("e", "arrow", 110.0, 1);
        arrow["startBinding"] = json!({ "elementId": "a" });
        arrow["endBinding"] = json!({ "elementId": "b" });
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "ellipse", 300.0, 1),
                arrow,
            ],
            ..Default::default()
        };
        apply_edit(
            &mut s,
            &json!({ "update": [{ "id": "a", "text": "一段很长的中文标签" }] }),
        )
        .unwrap();
        assert!(s.pending.is_empty(), "改文字不该动箭头");
        apply_fit_text(&mut s, &json!({ "ids": ["a"] })).unwrap();
        assert!(s.pending.is_empty(), "只改尺寸不该重连箭头");
    }

    /// 新图排到已有内容右边，不压在上面。
    #[test]
    fn graph_places_new_diagram_beside_existing_content() {
        let mut s = Scene {
            elements: vec![full("old", "rectangle", 100.0, 1)],
            ..Default::default()
        };
        s.elements[0]["width"] = json!(200.0);
        apply_graph(&mut s, &json!({ "nodes": [{ "id": "n1", "text": "A" }] })).unwrap();
        let n = s.pending.iter().find(|p| p["id"] == "n1").unwrap();
        let x = num(n, "x");
        assert!(x >= 300.0, "新图要排在已有内容右边，x={x}");
    }

    /// 两个节点贴太近、箭头塞不下时：边也得有几何，不能落在默认位置。
    #[test]
    fn graph_edge_between_touching_nodes_still_has_geometry() {
        let mut s = Scene::default();
        apply_graph(
            &mut s,
            &json!({
                "nodes": [{ "id": "a" }, { "id": "b" }],
                "edges": [{ "from": "a", "to": "b" }],
                "layout": "row",
                "gap_x": 0
            }),
        )
        .unwrap();
        let e = s.pending.iter().find(|p| p["type"] == "arrow").unwrap();
        assert!(e.get("points").is_some(), "边得有几何：{e}");
        let a = s.pending.iter().find(|p| p["id"] == "a").unwrap();
        assert_ne!(num(e, "x"), num(a, "x"), "边不能落在节点位置：{e}");
    }

    /// 同 from/to 的两条边：默认 id 加序号，别撞。
    #[test]
    fn graph_parallel_edges_get_unique_ids() {
        let mut s = Scene::default();
        let r = apply_graph(
            &mut s,
            &json!({
                "nodes": [{ "id": "a" }, { "id": "b" }],
                "edges": [{ "from": "a", "to": "b" }, { "from": "a", "to": "b" }]
            }),
        )
        .unwrap();
        assert_eq!(r.added.len(), 4);
        let ids: Vec<_> = s.pending.iter().filter_map(|p| p["id"].as_str()).collect();
        assert!(
            ids.contains(&"e-a-b") && ids.contains(&"e-a-b-2"),
            "{ids:?}"
        );
    }

    /// 参照物宽高为负时，按真实大小算位置。
    #[test]
    fn place_uses_absolute_width_of_the_anchor() {
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "rectangle", 500.0, 1),
            ],
            ..Default::default()
        };
        s.elements[0]["width"] = json!(-100.0);
        apply_place(
            &mut s,
            &json!({ "id": "b", "ref": "a", "where": "right", "gap": 40.0 }),
        )
        .unwrap();
        let b = s.elements.iter().find(|e| e["id"] == "b").unwrap();
        assert!((num(b, "x") - 140.0).abs() < 1e-6, "b.x={}", num(b, "x"));
    }

    /// 间距不能为负。
    #[test]
    fn negative_gap_is_rejected() {
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "rectangle", 300.0, 1),
            ],
            ..Default::default()
        };
        assert!(apply_place(
            &mut s,
            &json!({ "id": "b", "ref": "a", "where": "right", "gap": -5.0 })
        )
        .is_err());
        assert!(apply_layout(
            &mut s,
            &json!({ "ids": ["a", "b"], "columns": 2, "gap_x": -5.0 })
        )
        .is_err());
        assert!(apply_graph(&mut s, &json!({ "nodes": [{ "id": "n" }], "gap_x": -5.0 })).is_err());
    }

    /// 模型把单元素数组拍平成对象：当一个元素收下。
    #[test]
    fn flattened_single_element_is_accepted() {
        let mut s = Scene::default();
        let r = apply_edit(
            &mut s,
            &json!({ "add": { "type": "rectangle", "id": "a", "text": "x" } }),
        )
        .unwrap();
        assert_eq!(r.added, ["a"]);
    }

    /// 套了两层的 item 包装：都剥掉。
    #[test]
    fn double_item_wrapper_is_unwrapped() {
        let mut s = Scene::default();
        let r = apply_edit(
            &mut s,
            &json!({ "add": { "item": { "item": [{ "type": "rectangle", "id": "a" }] } } }),
        )
        .unwrap();
        assert_eq!(r.added, ["a"]);
    }

    /// ids 被拍平成对象：一个 id 也收下。
    #[test]
    fn flattened_ids_is_accepted() {
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "rectangle", 300.0, 1),
            ],
            ..Default::default()
        };
        apply_align(&mut s, &json!({ "ids": "a", "align": "left" })).unwrap_err();
        // 单个 id 不够对齐（至少两个），但不能是「ids 要是数组」那种错。
        let err = apply_align(&mut s, &json!({ "ids": "a", "align": "left" })).unwrap_err();
        assert!(err.contains("至少要 2 个"), "{err}");
    }

    /// 坐标写成字符串数字：认。
    #[test]
    fn string_numbers_are_accepted_for_coordinates() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "rectangle", "id": "a", "x": "1500", "y": "700" }] }),
        )
        .unwrap();
        assert_eq!(s.pending[0]["x"], 1500.0);
        assert_eq!(s.pending[0]["y"], 700.0);
    }

    /// 坐标不是数字：说清楚，不默默挪到默认位置。
    #[test]
    fn invalid_coordinate_is_reported() {
        let mut s = Scene::default();
        let err = apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "rectangle", "id": "a", "x": "左一点" }] }),
        )
        .unwrap_err();
        assert!(err.contains("x 要是数字"), "{err}");
        assert_eq!(s, Scene::default(), "失败不改");
    }

    /// 不认识的字段名：报告里说一句（不然就是空框）。
    #[test]
    fn unknown_field_is_reported() {
        let mut s = Scene::default();
        let r = apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "rectangle", "id": "a", "label": "写错了" }] }),
        )
        .unwrap();
        assert_eq!(r.ignored, ["label"]);
    }

    /// 按语义找：不用背 id。
    #[test]
    fn find_by_text_type_and_color() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [
                { "type": "rectangle", "id": "a", "text": "Sign Up", "fill": "#a5d8ff" },
                { "type": "ellipse", "id": "b", "text": "完成", "fill": "#d8f5d0" }
            ] }),
        )
        .unwrap();
        let r = apply_find(&mut s, &json!({ "queries": [{ "text": "sign" }] })).unwrap();
        assert_eq!(r.found, ["a"], "{:?}", r.detail);
        let r = apply_find(&mut s, &json!({ "queries": [{ "type": "ellipse" }] })).unwrap();
        assert_eq!(r.found, ["b"], "{:?}", r.detail);
        let r = apply_find(&mut s, &json!({ "queries": [{ "fill": "#D8F5D0" }] })).unwrap();
        assert_eq!(r.found, ["b"], "{:?}", r.detail);
    }

    /// 复制：连标签一起，文案可改，每份 id 不同。
    #[test]
    fn duplicate_copies_label_and_applies_texts() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "rectangle", "id": "a", "text": "原" }] }),
        )
        .unwrap();
        let mut label = full("a-label", "text", 0.0, 2);
        label["containerId"] = json!("a");
        label["text"] = json!("原");
        s.elements.push(label);
        s.elements[0]["boundElements"] = json!([{ "type": "text", "id": "a-label" }]);
        let r = apply_duplicate(
            &mut s,
            &json!({ "ids": ["a"], "count": 2, "texts": ["甲", "乙"] }),
        )
        .unwrap();
        assert_eq!(r.touched.len(), 2, "{:?}", r.touched);
        let l1 = s
            .pending
            .iter()
            .find(|e| e["id"] == "a-copy-label")
            .expect("第一份的标签跟着待画的框进 pending");
        assert_eq!(l1["text"], "甲");
        assert_eq!(l1["containerId"], "a-copy");
        assert!(l1.get("index").is_none(), "待画标签不带 index：{l1}");
        let l2 = s
            .pending
            .iter()
            .find(|e| e["id"] == "a-copy2-label")
            .expect("第二份的标签");
        assert_eq!(l2["text"], "乙");
        assert_eq!(l2["containerId"], "a-copy2");
    }

    /// 没给坐标：找空位，别都叠在一起。
    #[test]
    fn add_without_position_avoids_overlap() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [
                { "type": "rectangle", "id": "a", "width": 100, "height": 50 },
                { "type": "rectangle", "id": "b", "width": 100, "height": 50 }
            ] }),
        )
        .unwrap();
        let (ax, ay) = (num(&s.pending[0], "x"), num(&s.pending[0], "y"));
        let (bx, by) = (num(&s.pending[1], "x"), num(&s.pending[1], "y"));
        let overlap = ax < bx + 100.0 && ax + 100.0 > bx && ay < by + 50.0 && ay + 50.0 > by;
        assert!(!overlap, "两个图形不该重叠：{ax},{ay} vs {bx},{by}");
    }

    /// 分层读：总览不给 patches；只看这几个；related 把关系带上。
    #[test]
    fn read_scope_overview_ids_and_related() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [
                { "type": "rectangle", "id": "a", "text": "甲" },
                { "type": "rectangle", "id": "b", "text": "乙" }
            ] }),
        )
        .unwrap();
        let (_, data) = summary_with(&s, None, false);
        assert!(data.get("patches").is_none(), "总览不带 patches：{data}");
        let (_, data) = summary_with(&s, Some(&["a".to_string()]), true);
        assert_eq!(data["shapes"].as_array().unwrap().len(), 1);
        assert!(related_ids(&s, "a").contains(&"a".to_string()));
    }

    /// 模型端把数字写成字符串（"2"）、布尔写成 "true"：都认。
    #[test]
    fn string_numbers_and_booleans_are_accepted() {
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "rectangle", 300.0, 1),
            ],
            ..Default::default()
        };
        apply_layout(&mut s, &json!({ "ids": ["a", "b"], "columns": "2" })).unwrap();
        assert_eq!(
            read_scope(&json!({ "overview": "true" })).unwrap(),
            ReadScope::Overview
        );
        assert_eq!(
            read_scope(&json!({ "overview": true })).unwrap(),
            ReadScope::Overview
        );
        let mut s2 = Scene::default();
        apply_edit(
            &mut s2,
            &json!({ "add": [{ "type": "rectangle", "id": "x" }] }),
        )
        .unwrap();
        let r = apply_duplicate(
            &mut s2,
            &json!({ "ids": ["x"], "count": "2", "texts": ["甲", "乙"] }),
        )
        .unwrap();
        assert_eq!(r.touched.len(), 2, "{:?}", r.touched);
    }

    /// 删掉的元素留墓碑，版本要压过界面手里那份（版本一样时界面会留着它，删了又活）。
    #[test]
    fn deleted_elements_get_a_tombstone_version_above_the_local_copy() {
        let mut s = Scene {
            elements: vec![full("a", "rectangle", 0.0, 7)],
            ..Default::default()
        };
        apply_edit(&mut s, &json!({ "delete": ["a"] })).unwrap();
        let v = s.elements[0]["version"].as_u64().unwrap();
        assert!(v > 7 + 1, "墓碑版本要远高于界面手里的：{v}");
    }

    /// 分数索引要和 npm 的 fractional-indexing 对得上（界面的 index 是同一套键空间）。
    #[test]
    fn fractional_index_matches_the_npm_implementation() {
        assert_eq!(key_between(None, None).unwrap(), "a0");
        assert_eq!(key_between(None, Some("a0")).unwrap(), "Zz");
        assert_eq!(key_between(None, Some("a1")).unwrap(), "a0");
        assert_eq!(key_between(Some("a0"), None).unwrap(), "a1");
        assert_eq!(key_between(Some("a0"), Some("a1")).unwrap(), "a0V");
        assert_eq!(key_between(Some("a0"), Some("a2")).unwrap(), "a1");
        assert_eq!(key_between(Some("a0"), Some("a1V")).unwrap(), "a1");
        assert_eq!(key_between(Some("az"), Some("b00")).unwrap(), "azV");
        assert_eq!(key_between(Some("a0V"), Some("a1")).unwrap(), "a0l");
        assert_eq!(key_between(Some("Zz"), Some("a0")).unwrap(), "ZzV");
        assert!(key_between(Some("a0"), Some("Zz")).is_none());
    }

    /// layer 同时改 `index` 和数组顺序（界面按数组顺序画）。
    #[test]
    fn layer_rewrites_fractional_index() {
        let mut s = Scene::default();
        for (id, index) in [("a", "a0"), ("b", "a1"), ("c", "a2")] {
            let mut e = full(id, "rectangle", 0.0, 1);
            e["index"] = json!(index);
            s.elements.push(e);
        }
        apply_layer(&mut s, &json!({ "ids": ["a"], "to": "front" })).unwrap();
        let idx = |id: &str| {
            s.elements.iter().find(|e| e["id"] == id).unwrap()["index"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert!(idx("a") > idx("c"), "a 应该在最上：{}", idx("a"));
        assert!(idx("b") < idx("c"), "b、c 的相对顺序不变");
    }

    /// 数组元素自己被包了壳（模型端逐个包 item）：也要认。
    #[test]
    fn element_level_item_wrappers_are_unwrapped() {
        let mut s = Scene::default();
        let r = apply_edit(
            &mut s,
            &json!({ "add": { "item": [
                { "type": "rectangle", "id": "a" },
                { "item": { "type": "rectangle", "id": "b" } }
            ] } }),
        )
        .unwrap();
        assert_eq!(r.added, ["a", "b"], "{:?}", r.added);
    }

    /// 工具结果只讲给定的图形：全量清单交给 whiteboard_read。
    #[test]
    fn lines_for_mentions_only_given_shapes() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [
                { "type": "rectangle", "id": "a", "text": "甲" },
                { "type": "rectangle", "id": "b", "text": "乙" }
            ] }),
        )
        .unwrap();
        let text = lines_for(&s, &["a".to_string()]);
        assert!(text.contains("id=a"), "{text}");
        assert!(!text.contains("id=b"), "{text}");
    }

    fn shape_of<'a>(scene: &'a Scene, id: &str) -> &'a Value {
        scene
            .elements
            .iter()
            .chain(scene.pending.iter())
            .find(|e| e["id"] == id)
            .unwrap_or_else(|| panic!("没有 {id}"))
    }

    /// 复制完整元素：嵌套绑定指向副本，z 序键不重复，place 按外框平移且不斜移。
    #[test]
    fn duplicate_retargets_bindings_and_places_by_bbox() {
        let mut a = full("a", "rectangle", 100.0, 1);
        a["index"] = json!("a0");
        a["boundElements"] = json!([{ "id": "a-label", "type": "text" }]);
        let mut b = full("b", "rectangle", 400.0, 1);
        b["index"] = json!("a1");
        let mut edge = full("edge", "arrow", 110.0, 1);
        edge["index"] = json!("a2");
        edge["startBinding"] = json!({ "elementId": "a", "focus": 0, "gap": 8 });
        edge["endBinding"] = json!({ "elementId": "b", "focus": 0, "gap": 8 });
        let mut label = full("a-label", "text", 120.0, 1);
        label["containerId"] = json!("a");
        label["text"] = json!("原");
        label["index"] = json!("a3");
        let mut s = Scene {
            elements: vec![a, b, edge, label],
            ..Default::default()
        };
        let before = s.clone();
        let r = apply_duplicate(
            &mut s,
            &json!({ "ids": ["a", "b", "edge"], "count": 2, "place": "right" }),
        )
        .unwrap();
        assert_eq!(
            r.touched,
            vec![
                "a-copy",
                "b-copy",
                "edge-copy",
                "a-copy2",
                "b-copy2",
                "edge-copy2"
            ]
        );
        let copy = shape_of(&s, "edge-copy");
        assert_eq!(copy["startBinding"]["elementId"], "a-copy");
        assert_eq!(copy["endBinding"]["elementId"], "b-copy");
        assert!(copy.get("startBinding.elementId").is_none(), "{copy}");
        assert!(copy.get("endBinding.elementId").is_none(), "{copy}");
        let a_copy = shape_of(&s, "a-copy");
        assert_eq!(a_copy["boundElements"][0]["id"], "a-copy-label");
        assert_ne!(a_copy["index"], "a0");
        assert!(
            a_copy["index"].as_str().unwrap() > "a3",
            "副本要排在原件之上：{}",
            a_copy["index"]
        );
        // 选区 x=100..500，右移一份后左缘 = 500 + GAP(60)。
        assert!(
            (num(a_copy, "x") - 560.0).abs() < 1e-6,
            "x={}",
            num(a_copy, "x")
        );
        assert!((num(a_copy, "y") - 0.0).abs() < 1e-6, "y 不该跟着斜移");
        let a2 = shape_of(&s, "a-copy2");
        assert!((num(a2, "y") - 0.0).abs() < 1e-6, "第二份仍在同一行");
        assert!(num(a2, "x") > num(a_copy, "x"));
        assert_eq!(shape_of(&s, "a")["index"], before.elements[0]["index"]);
    }

    /// 两端都在选区里的箭头跟着复制；只有一端在选区里的留在原件上。
    #[test]
    fn duplicate_includes_internal_arrows_only() {
        let mut edge = full("edge", "arrow", 200.0, 1);
        edge["startBinding"] = json!({ "elementId": "a" });
        edge["endBinding"] = json!({ "elementId": "b" });
        let mut stray = full("stray", "arrow", 200.0, 1);
        stray["startBinding"] = json!({ "elementId": "a" });
        stray["endBinding"] = json!({ "elementId": "c" });
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "rectangle", 300.0, 1),
                full("c", "rectangle", 600.0, 1),
                edge,
                stray,
            ],
            ..Default::default()
        };
        let r = apply_duplicate(&mut s, &json!({ "ids": ["a", "b"], "place": "right" })).unwrap();
        assert_eq!(r.touched, vec!["a-copy", "b-copy", "edge-copy"]);
        assert!(s.shape("stray-copy").is_none(), "外连箭头不该复制");
        let copy = shape_of(&s, "edge-copy");
        assert_eq!(copy["startBinding"]["elementId"], "a-copy");
        assert_eq!(copy["endBinding"]["elementId"], "b-copy");
    }

    /// texts 只改请求里的图形；自动带上的内部箭头（及其标签）保留原文案。
    #[test]
    fn duplicate_texts_skip_auto_copied_arrow_labels() {
        let mut a = full("a", "rectangle", 0.0, 1);
        a["boundElements"] = json!([{ "id": "a-label", "type": "text" }]);
        let mut a_label = full("a-label", "text", 10.0, 1);
        a_label["containerId"] = json!("a");
        a_label["text"] = json!("原框");
        let mut edge = full("edge", "arrow", 200.0, 1);
        edge["startBinding"] = json!({ "elementId": "a" });
        edge["endBinding"] = json!({ "elementId": "b" });
        edge["boundElements"] = json!([{ "id": "edge-label", "type": "text" }]);
        let mut edge_label = full("edge-label", "text", 210.0, 1);
        edge_label["containerId"] = json!("edge");
        edge_label["text"] = json!("连线");
        let mut s = Scene {
            elements: vec![
                a,
                full("b", "rectangle", 300.0, 1),
                edge,
                a_label,
                edge_label,
            ],
            ..Default::default()
        };
        let r = apply_duplicate(
            &mut s,
            &json!({ "ids": ["a", "b"], "texts": ["新"], "place": "right" }),
        )
        .unwrap();
        assert_eq!(r.touched, vec!["a-copy", "b-copy", "edge-copy"]);
        assert_eq!(shape_of(&s, "a-copy-label")["text"], "新");
        assert_eq!(
            shape_of(&s, "edge-copy-label")["text"],
            "连线",
            "自动带上的箭头标签不该被 texts 改掉"
        );
        assert_eq!(shape_of(&s, "edge-copy-label")["containerId"], "edge-copy");
    }

    /// 副本的 boundElements 去掉没复制的外连箭头，保留映射过的内部箭头和标签。
    #[test]
    fn duplicate_drops_unmapped_bound_arrows() {
        let mut a = full("a", "rectangle", 0.0, 1);
        a["boundElements"] = json!([
            { "id": "a-label", "type": "text" },
            { "id": "edge", "type": "arrow" },
            { "id": "stray", "type": "arrow" }
        ]);
        let mut a_label = full("a-label", "text", 10.0, 1);
        a_label["containerId"] = json!("a");
        a_label["text"] = json!("原");
        let mut edge = full("edge", "arrow", 200.0, 1);
        edge["startBinding"] = json!({ "elementId": "a" });
        edge["endBinding"] = json!({ "elementId": "b" });
        let mut stray = full("stray", "arrow", 200.0, 1);
        stray["startBinding"] = json!({ "elementId": "a" });
        stray["endBinding"] = json!({ "elementId": "c" });
        let mut s = Scene {
            elements: vec![
                a,
                full("b", "rectangle", 300.0, 1),
                full("c", "rectangle", 600.0, 1),
                edge,
                stray,
                a_label,
            ],
            ..Default::default()
        };
        apply_duplicate(&mut s, &json!({ "ids": ["a", "b"], "place": "right" })).unwrap();
        let bound: Vec<&str> = shape_of(&s, "a-copy")["boundElements"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|b| b["id"].as_str())
            .collect();
        assert_eq!(bound, vec!["a-copy-label", "edge-copy"]);
        let orig: Vec<&str> = shape_of(&s, "a")["boundElements"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|b| b["id"].as_str())
            .collect();
        assert_eq!(orig, vec!["a-label", "edge", "stray"], "原件不动");
    }

    /// 待画骨架的标签跟着进 pending，不另写一份完整元素。
    #[test]
    fn duplicate_pending_label_stays_pending() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [{ "type": "rectangle", "id": "a", "text": "原", "x": 10, "y": 20 }] }),
        )
        .unwrap();
        apply_duplicate(&mut s, &json!({ "ids": ["a"], "place": "right" })).unwrap();
        assert!(
            s.elements.is_empty(),
            "骨架副本不该进 elements：{:?}",
            s.elements
        );
        let copy = s.pending.iter().find(|e| e["id"] == "a-copy").unwrap();
        assert_eq!(copy["label"]["text"], "原");
        assert!(copy.get("index").is_none(), "骨架不带 index：{copy}");
    }

    /// 回边不参与分层；超宽节点按真实宽度占位。
    #[test]
    fn graph_back_edge_and_wide_nodes_do_not_overlap() {
        let mut s = Scene::default();
        apply_graph(
            &mut s,
            &json!({
                "nodes": [
                    { "id": "A", "width": 400, "height": 80 },
                    { "id": "B" },
                    { "id": "C" }
                ],
                "edges": [
                    { "from": "A", "to": "B" },
                    { "from": "B", "to": "C" },
                    { "from": "C", "to": "B" }
                ]
            }),
        )
        .unwrap();
        let at = |id: &str| {
            let e = s.pending.iter().find(|p| p["id"] == id).unwrap();
            (num(e, "x"), num(e, "y"), num(e, "width"), num(e, "height"))
        };
        let (ax, ay, aw, ah) = at("A");
        let (_, by, _, _) = at("B");
        let (cx, cy, cw, ch) = at("C");
        assert!(cy > by, "C 在 B 的下一层：B.y={by} C.y={cy}");
        let overlap = ax < cx + cw && ax + aw > cx && ay < cy + ch && ay + ah > cy;
        assert!(
            !overlap,
            "A 与 C 不该重叠：A=({ax},{ay},{aw},{ah}) C=({cx},{cy},{cw},{ch})"
        );
    }

    /// 横排用节点自己的宽度，不按默认 200 挤在一起。
    #[test]
    fn graph_row_uses_actual_widths() {
        let mut s = Scene::default();
        apply_graph(
            &mut s,
            &json!({
                "nodes": [
                    { "id": "A", "width": 400, "height": 80 },
                    { "id": "B", "width": 400, "height": 80 }
                ],
                "layout": "row",
                "gap_x": 60
            }),
        )
        .unwrap();
        let ax = num(s.pending.iter().find(|p| p["id"] == "A").unwrap(), "x");
        let bx = num(s.pending.iter().find(|p| p["id"] == "B").unwrap(), "x");
        assert!(bx >= ax + 400.0 + 60.0, "B.x={bx} A.x={ax}");
    }

    /// 找不到的 near 报错；命中按整段相等优先，距离近的优先。
    #[test]
    fn find_rejects_unknown_near_and_ranks_matches() {
        let mut s = Scene::default();
        apply_edit(
            &mut s,
            &json!({ "add": [
                { "type": "rectangle", "id": "long", "text": "Sign Up", "x": 0, "y": 0 },
                { "type": "rectangle", "id": "exact", "text": "Sign", "x": 800, "y": 0 },
                { "type": "rectangle", "id": "near", "text": "别的", "x": 40, "y": 0 },
                { "type": "rectangle", "id": "far", "text": "别的", "x": 900, "y": 0 }
            ] }),
        )
        .unwrap();
        let err = apply_find(&mut s, &json!({ "queries": [{ "near": "missing" }] })).unwrap_err();
        assert!(err.contains("missing"), "{err}");
        assert!(err.contains("near"), "{err}");
        let ranked = apply_find(&mut s, &json!({ "queries": [{ "text": "sign" }] })).unwrap();
        assert_eq!(
            ranked.found.first().map(String::as_str),
            Some("exact"),
            "{:?}",
            ranked.found
        );
        let near = apply_find(
            &mut s,
            &json!({ "queries": [{ "near": "long", "radius": 2000 }] }),
        )
        .unwrap();
        assert!(
            !near.found.iter().any(|id| id == "long"),
            "参照自己不算：{:?}",
            near.found
        );
        assert_eq!(
            near.found.first().map(String::as_str),
            Some("near"),
            "{:?}",
            near.found
        );
    }

    /// related 带上指向这个图形的箭头。
    #[test]
    fn related_includes_arrows_pointing_at_the_shape() {
        let mut edge = full("edge", "arrow", 200.0, 1);
        edge["startBinding"] = json!({ "elementId": "a" });
        edge["endBinding"] = json!({ "elementId": "b" });
        let s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "rectangle", 300.0, 1),
                edge,
            ],
            ..Default::default()
        };
        let ids = related_ids(&s, "a");
        assert!(ids.iter().any(|id| id == "edge"), "{ids:?}");
    }

    /// 图形总宽超过两端跨度时不改坐标。
    #[test]
    fn distribute_rejects_when_sizes_exceed_span() {
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 1),
                full("b", "rectangle", 10.0, 1),
                full("c", "rectangle", 50.0, 1),
            ],
            ..Default::default()
        };
        let err = apply_distribute(
            &mut s,
            &json!({ "ids": ["a", "b", "c"], "direction": "horizontal" }),
        )
        .unwrap_err();
        assert!(err.contains("放不下"), "{err}");
        assert_eq!(num(&s.elements[0], "x"), 0.0);
        assert_eq!(num(&s.elements[1], "x"), 10.0);
        assert_eq!(num(&s.elements[2], "x"), 50.0);
    }
}
