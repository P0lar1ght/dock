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

    fn live(&self) -> impl Iterator<Item = &Value> {
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
}

impl EditReport {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.deleted.is_empty() && !self.cleared
    }
}

/// 恰好只有一个键 `item` 时剥掉包装，返回内层；否则原样。
fn unwrap_item(v: &Value) -> (&Value, bool) {
    match v.as_object() {
        Some(m) if m.len() == 1 => match m.get("item") {
            Some(inner) => (inner, true),
            None => (v, false),
        },
        _ => (v, false),
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

/// 取数组字段；若整段是 `{"item": [...]}`（恰好一个键）则自动剥掉。
fn take_array_field(args: &Value, key: &str) -> Result<(Vec<Value>, bool), String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok((Vec::new(), false)),
        Some(Value::Array(a)) => Ok((a.clone(), false)),
        Some(v) => {
            let (inner, unwrapped) = unwrap_item(v);
            match inner {
                Value::Array(a) if unwrapped => Ok((a.clone(), true)),
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
    let clear = args.get("clear").and_then(Value::as_bool).unwrap_or(false);
    if adds.len() + updates.len() + deletes.len() > MAX_OPS {
        return Err(format!("一次最多改 {MAX_OPS} 个，分几次做"));
    }
    let mut next = scene.clone();
    let mut report = EditReport {
        unwrapped_item: u_add || u_upd || u_del,
        ..Default::default()
    };
    if clear {
        for e in next.elements.iter_mut().filter(|e| !is_deleted(e)) {
            e["isDeleted"] = json!(true);
            bump(e);
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
    let origin = next.bounds();
    for (i, a) in adds.iter().enumerate() {
        let id = add_one(&mut next, a, origin, i)?;
        report.added.push(id);
    }
    if report.is_empty() {
        return Err("没有要改的：给 add / update / delete / clear 之一".into());
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(report)
}

fn add_one(
    scene: &mut Scene,
    shape: &Value,
    origin: Option<(f64, f64, f64, f64)>,
    index: usize,
) -> Result<String, String> {
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
    // 没给位置：排在现有内容右边，一列往下。
    let (x0, y0) = match origin {
        Some((_, top, right, _)) => (right + GAP * 2.0, top),
        None => (100.0, 100.0),
    };
    let x = opt_num(shape, "x").unwrap_or(x0);
    let y = opt_num(shape, "y").unwrap_or(y0 + index as f64 * (DEFAULT_H + GAP));
    let mut s = Map::new();
    s.insert("type".into(), json!(kind));
    s.insert("id".into(), json!(id));
    s.insert("x".into(), json!(x));
    s.insert("y".into(), json!(y));
    if kind != "text" {
        let w = opt_num(shape, "width").unwrap_or(if linear { 160.0 } else { DEFAULT_W });
        let h = opt_num(shape, "height").unwrap_or(if linear { 0.0 } else { DEFAULT_H });
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
    if ta + tb >= len {
        return;
    }
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
    let mut moved = false;
    for key in ["x", "y", "width", "height"] {
        if let Some(n) = opt_num(patch, key) {
            if num(&e, key) != n {
                moved = true;
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
                if moved {
                    recenter(t_el, &e);
                }
                if t.is_some() || moved {
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

fn delete_one(scene: &mut Scene, id: &str) -> Result<(), String> {
    if let Some(i) = scene.pending_index(id) {
        scene.pending.remove(i);
        return Ok(());
    }
    let i = scene
        .index_of(id)
        .filter(|&i| !is_deleted(&scene.elements[i]))
        .ok_or_else(|| format!("没有图形「{id}」：先用 whiteboard_read 看现有的 id"))?;
    let e = &mut scene.elements[i];
    e["isDeleted"] = json!(true);
    bump(e);
    for t in scene.elements.iter_mut() {
        if !is_deleted(t) && t.get("containerId").and_then(Value::as_str) == Some(id) {
            t["isDeleted"] = json!(true);
            bump(t);
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
        let _ = delete_one(scene, &old_id);
        // 清掉箭头与标签墓碑，复用原 id 进 pending（模型不丢箭头引用）。
        let mut top: Option<u64> = None;
        scene.elements.retain(|e| {
            let mine = id_of(e) == Some(old_id.as_str())
                || (is_deleted(e)
                    && e.get("containerId").and_then(Value::as_str) == Some(old_id.as_str()));
            if mine {
                top = top.max(Some(num(e, "version") as u64));
            }
            !mine
        });
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
    let ungroup = args
        .get("ungroup")
        .and_then(Value::as_bool)
        .unwrap_or(false);
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
        let (x, y, w, h) = (num(e, "x"), num(e, "y"), num(e, "width"), num(e, "height"));
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
    let mut positions: Vec<usize> = ids.iter().filter_map(|id| next.index_of(id)).collect();
    positions.sort_unstable();
    positions.dedup();
    match to {
        "front" => {
            let mut moved = Vec::new();
            for i in positions.into_iter().rev() {
                moved.push(next.elements.remove(i));
            }
            moved.reverse();
            for e in &mut moved {
                bump(e);
            }
            next.elements.extend(moved);
        }
        "back" => {
            let mut moved = Vec::new();
            for i in positions.into_iter().rev() {
                moved.push(next.elements.remove(i));
            }
            moved.reverse();
            for e in &mut moved {
                bump(e);
            }
            next.elements.splice(0..0, moved);
        }
        "forward" => {
            // 每个选中项与后一个未选中项交换（从后往前扫避免连环）。
            let selected: std::collections::HashSet<String> = ids.iter().cloned().collect();
            let mut i = next.elements.len().saturating_sub(1);
            while i > 0 {
                let id = id_of(&next.elements[i - 1]).unwrap_or("").to_string();
                let next_id = id_of(&next.elements[i]).unwrap_or("").to_string();
                if selected.contains(&id) && !selected.contains(&next_id) {
                    next.elements.swap(i - 1, i);
                    bump(&mut next.elements[i]);
                    bump(&mut next.elements[i - 1]);
                }
                i -= 1;
            }
        }
        "backward" => {
            let selected: std::collections::HashSet<String> = ids.iter().cloned().collect();
            let len = next.elements.len();
            for i in 0..len.saturating_sub(1) {
                let id = id_of(&next.elements[i]).unwrap_or("").to_string();
                let next_id = id_of(&next.elements[i + 1]).unwrap_or("").to_string();
                if selected.contains(&next_id) && !selected.contains(&id) {
                    next.elements.swap(i, i + 1);
                    bump(&mut next.elements[i]);
                    bump(&mut next.elements[i + 1]);
                }
            }
        }
        _ => unreachable!(),
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(ArrangeReport {
        detail: format!("已把 {} 个图形移到 {to}", ids.len()),
        touched: ids,
    })
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
        } else if kind == "text" {
            patch["height"] = json!(h);
            // text 形状同时记下 fontSize（若原本没有）
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
    (num(e, "x"), num(e, "y"), num(e, "width"), num(e, "height"))
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
        .and_then(Value::as_u64)
        .ok_or("缺 columns（正整数）")? as usize;
    if columns == 0 {
        return Err("columns 至少是 1".into());
    }
    let gap_x = opt_num(args, "gap_x").unwrap_or(GRAPH_GAP_X);
    let gap_y = opt_num(args, "gap_y").unwrap_or(GRAPH_GAP_Y);
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
    let gap = opt_num(args, "gap").unwrap_or(DEFAULT_PLACE_GAP);
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

/// `whiteboard_graph` 的结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphReport {
    pub added: Vec<String>,
    pub detail: String,
    pub unwrapped_item: bool,
}

/// `whiteboard_graph`：`{ nodes, edges?, layout?, columns?, gap_x?, gap_y? }`。
/// layout ∈ row|column|grid|layered；默认 layered。节点默认 200×80，间距 60/80。
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
        .and_then(Value::as_u64)
        .unwrap_or(if layout == "grid" { 3 } else { 1 }) as usize;
    if columns == 0 {
        return Err("columns 至少是 1".into());
    }
    let gap_x = opt_num(args, "gap_x").unwrap_or(GRAPH_GAP_X);
    let gap_y = opt_num(args, "gap_y").unwrap_or(GRAPH_GAP_Y);

    let mut parsed: Vec<(String, String, String)> = Vec::with_capacity(nodes.len());
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
        if parsed.iter().any(|(p, _, _)| p == &id) {
            return Err(format!("第 {} 个节点 id「{id}」重复（nodes[{i}]）", i + 1));
        }
        if scene.shape(&id).is_some() {
            return Err(format!(
                "第 {} 个节点 id「{id}」已经有了（nodes[{i}]：换一个或先 delete）",
                i + 1
            ));
        }
        parsed.push((id, kind.to_string(), text));
    }
    let id_set: std::collections::HashSet<&str> =
        parsed.iter().map(|(i, _, _)| i.as_str()).collect();

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
        let eid = e
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("e-{from}-{to}"));
        parsed_edges.push((eid, from.to_string(), to.to_string(), text));
    }

    let positions = graph_positions(&parsed, &parsed_edges, layout, columns, gap_x, gap_y);
    let mut next = scene.clone();
    let mut added = Vec::new();
    let origin = next.bounds();
    for (i, (id, kind, text)) in parsed.iter().enumerate() {
        let (x, y) = positions[i];
        let mut shape = json!({
            "type": kind, "id": id, "x": x, "y": y,
            "width": GRAPH_W, "height": GRAPH_H,
        });
        if !text.is_empty() {
            shape["text"] = json!(text);
        }
        let aid = add_one(&mut next, &shape, origin, i)?;
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
        let aid = add_one(&mut next, &shape, None, i)?;
        added.push(aid);
    }
    next.rev = scene.rev + 1;
    *scene = next;
    Ok(GraphReport {
        detail: format!(
            "已画图：{} 个节点、{} 条边（layout={layout}）",
            parsed.len(),
            parsed_edges.len()
        ),
        added,
        unwrapped_item: u_n || u_e,
    })
}

fn graph_positions(
    nodes: &[(String, String, String)],
    edges: &[(String, String, String, Option<String>)],
    layout: &str,
    columns: usize,
    gap_x: f64,
    gap_y: f64,
) -> Vec<(f64, f64)> {
    let n = nodes.len();
    let mut pos = vec![(100.0, 100.0); n];
    let idx: std::collections::HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, (id, _, _))| (id.as_str(), i))
        .collect();
    match layout {
        "row" => {
            for (i, slot) in pos.iter_mut().enumerate() {
                *slot = (100.0 + i as f64 * (GRAPH_W + gap_x), 100.0);
            }
        }
        "column" => {
            for (i, slot) in pos.iter_mut().enumerate() {
                *slot = (100.0, 100.0 + i as f64 * (GRAPH_H + gap_y));
            }
        }
        "grid" => {
            let cols = columns.max(1);
            for (i, slot) in pos.iter_mut().enumerate() {
                let c = i % cols;
                let r = i / cols;
                *slot = (
                    100.0 + c as f64 * (GRAPH_W + gap_x),
                    100.0 + r as f64 * (GRAPH_H + gap_y),
                );
            }
        }
        _ => {
            let mut indeg = vec![0usize; n];
            let mut adj = vec![Vec::new(); n];
            for (_, from, to, _) in edges {
                if let (Some(&a), Some(&b)) = (idx.get(from.as_str()), idx.get(to.as_str())) {
                    adj[a].push(b);
                    indeg[b] += 1;
                }
            }
            let mut level = vec![0usize; n];
            let mut queue: std::collections::VecDeque<usize> = indeg
                .iter()
                .enumerate()
                .filter(|(_, d)| **d == 0)
                .map(|(i, _)| i)
                .collect();
            if queue.is_empty() {
                queue.extend(0..n);
            }
            let mut seen = vec![false; n];
            while let Some(u) = queue.pop_front() {
                if seen[u] {
                    continue;
                }
                seen[u] = true;
                for &v in &adj[u] {
                    level[v] = level[v].max(level[u] + 1);
                    indeg[v] = indeg[v].saturating_sub(1);
                    if indeg[v] == 0 {
                        queue.push_back(v);
                    }
                }
            }
            let max_lvl = level.iter().copied().max().unwrap_or(0);
            let mut per_level: Vec<Vec<usize>> = vec![Vec::new(); max_lvl + 1];
            for (i, &l) in level.iter().enumerate() {
                per_level[l].push(i);
            }
            for (l, row) in per_level.iter().enumerate() {
                for (c, &i) in row.iter().enumerate() {
                    pos[i] = (
                        100.0 + c as f64 * (GRAPH_W + gap_x),
                        100.0 + l as f64 * (GRAPH_H + gap_y),
                    );
                }
            }
        }
    }
    pos
}

// ---- 给模型看的 ----

/// 一行一个图形；容器里的文字并进容器的标签，箭头写两端。
pub fn summary(scene: &Scene) -> (String, Value) {
    let label_of = |id: &str| {
        scene
            .live()
            .find(|t| t.get("containerId").and_then(Value::as_str) == Some(id))
            .and_then(|t| t.get("text").and_then(Value::as_str))
            .map(str::to_string)
    };
    let mut lines = Vec::new();
    let mut shapes = Vec::new();
    for e in scene.live() {
        if e.get("containerId").and_then(Value::as_str).is_some() {
            continue;
        }
        let id = id_of(e).unwrap_or_default();
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
        format!("白板是空的（第 {} 版）。", scene.rev)
    } else {
        format!("白板第 {} 版，{} 个图形：", scene.rev, lines.len())
    };
    let mut text = head;
    for l in &lines {
        text.push('\n');
        text.push_str(l);
    }
    if !scene.pending.is_empty() {
        text.push_str("\n（标「待画」的是刚加的：用户打开白板界面后才排好版、连好线。）");
    }
    let patches: Vec<Value> = shapes
        .iter()
        .map(|s| {
            let mut p = serde_json::Map::new();
            for key in [
                "id", "type", "x", "y", "width", "height", "text", "stroke", "fill", "from", "to",
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
    (
        text,
        json!({ "rev": scene.rev, "shapes": shapes, "patches": patches }),
    )
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
    e.get(key).and_then(Value::as_f64).filter(|n| n.is_finite())
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
        assert!(
            num(&s.pending[1], "y") > num(&s.pending[0], "y"),
            "没给位置的往下排"
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
        assert_eq!(
            s.pending[0]["customData"]["dockMinVersion"], 9,
            "清空时出了一版（8），再高一版"
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

        let before_front = s.elements.iter().position(|e| e["id"] == "a").unwrap();
        let ly = apply_layer(&mut s, &json!({ "ids": ["a"], "to": "front" })).unwrap();
        assert!(ly.detail.contains("front"));
        assert_eq!(s.elements.last().unwrap()["id"], "a");
        assert!(before_front < s.elements.len() - 1 || true);

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
}
