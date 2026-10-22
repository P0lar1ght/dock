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
}

impl EditReport {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.deleted.is_empty() && !self.cleared
    }
}

/// `{ add?, update?, delete?, clear? }`：先清空，再删、改、加。任何一条不合法整次不改。
pub fn apply_edit(scene: &mut Scene, args: &Value) -> Result<EditReport, String> {
    let list = |key: &str| -> Result<Vec<Value>, String> {
        match args.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(a)) => Ok(a.clone()),
            Some(_) => Err(format!("{key} 要是数组")),
        }
    };
    let (adds, updates, deletes) = (list("add")?, list("update")?, list("delete")?);
    let clear = args.get("clear").and_then(Value::as_bool).unwrap_or(false);
    if adds.len() + updates.len() + deletes.len() > MAX_OPS {
        return Err(format!("一次最多改 {MAX_OPS} 个，分几次做"));
    }
    let mut next = scene.clone();
    let mut report = EditReport::default();
    if clear {
        for e in next.elements.iter_mut().filter(|e| !is_deleted(e)) {
            e["isDeleted"] = json!(true);
            bump(e);
        }
        next.pending.clear();
        report.cleared = true;
    }
    for d in &deletes {
        let id = d.as_str().ok_or("delete 里要放图形 id（字符串）")?;
        delete_one(&mut next, id)?;
        report.deleted.push(id.to_string());
    }
    for u in &updates {
        let id = u
            .get("id")
            .and_then(Value::as_str)
            .ok_or("update 的每一项要有 id")?
            .to_string();
        update_one(&mut next, &id, u)?;
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
    let kind = shape
        .get("type")
        .and_then(Value::as_str)
        .ok_or("add 的每一项要有 type")?;
    if !SHAPES.contains(&kind) {
        return Err(format!(
            "不认识的 type「{kind}」：可用 {}",
            SHAPES.join(" / ")
        ));
    }
    // 删掉的图形只剩墓碑：同一个 id 可以再用（清空后重画很常见）。新元素的版本要比墓碑高，
    // 不然按版本合并时墓碑会赢、新画的又被删掉——记在 customData 里，界面画出来时照办。
    // 框里的标签（界面定死成 `<id>-label`，containerId 指向这个 id）也留着墓碑：一并让位，
    // 下限取两者里高的那个，界面给标签用同一个下限（不然重画后框在、字没了）。
    let mut min_version = None;
    let id = match shape.get("id").and_then(Value::as_str).map(str::trim) {
        Some(id) if !id.is_empty() => {
            if scene.shape(id).is_some() {
                return Err(format!("id「{id}」已经有了：换一个，或用 update 改它"));
            }
            let mut top: Option<u64> = None;
            scene.elements.retain(|e| {
                let mine = id_of(e) == Some(id)
                    || e.get("containerId").and_then(Value::as_str) == Some(id);
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
                    return Err(format!("{key}「{target}」不存在"));
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

/// 绑在 `id` 上的完整箭头：留墓碑，换成一个新 id 的骨架（同颜色、同标签、同两端）。
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
        let _ = delete_one(scene, &old_id);
        let new_id = {
            let base = format!("{old_id}~{}", scene.rev + 1);
            if scene.id_taken(&base) {
                fresh_id(scene)
            } else {
                base
            }
        };
        let mut s = Map::new();
        s.insert("type".into(), json!("arrow"));
        s.insert("id".into(), json!(new_id));
        s.insert("x".into(), json!(num(&old, "x")));
        s.insert("y".into(), json!(num(&old, "y")));
        for key in ["strokeColor", "strokeStyle", "strokeWidth"] {
            if let Some(v) = old.get(key) {
                s.insert(key.into(), v.clone());
            }
        }
        if let Some(from) = ends(&old, "startBinding") {
            s.insert("start".into(), json!({ "id": from }));
        }
        if let Some(to) = ends(&old, "endBinding") {
            s.insert("end".into(), json!({ "id": to }));
        }
        if let Some(text) = label {
            s.insert("label".into(), json!({ "text": text }));
        }
        let from = ends(&old, "startBinding");
        let to = ends(&old, "endBinding");
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
    (text, json!({ "rev": scene.rev, "shapes": shapes }))
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

    /// 清空后重画带标签的框：框里那段字的墓碑一并让位，下限也要压过它（标签墓碑版本更高时）。
    /// 回归：只看框自己的墓碑时，界面新画的 `<id>-label` 版本低，合并时被墓碑删掉。
    #[test]
    fn reusing_an_id_also_clears_its_label_tombstone() {
        let mut label = full("a-label", "text", 0.0, 12);
        label["containerId"] = json!("a");
        let mut s = Scene {
            elements: vec![
                full("a", "rectangle", 0.0, 7),
                label,
                full("b", "rectangle", 300.0, 3),
            ],
            ..Default::default()
        };
        apply_edit(
            &mut s,
            &json!({ "clear": true, "add": [{ "type": "rectangle", "id": "a", "text": "又是 a" }] }),
        )
        .unwrap();
        assert!(
            s.elements.iter().all(|e| id_of(e) != Some("a-label")),
            "标签墓碑让位：{:?}",
            s.elements
        );
        assert!(
            s.elements.iter().any(|e| id_of(e) == Some("b")),
            "别的墓碑不动"
        );
        assert_eq!(
            s.pending[0]["customData"]["dockMinVersion"], 14,
            "清空时标签出了一版（13），再高一版"
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
        assert_eq!(s.elements[3]["isDeleted"], true, "旧箭头留墓碑");
        assert_eq!(s.pending.len(), 1, "箭头退回骨架重新连");
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
}
