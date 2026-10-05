//! Named `"tui.slots"` service. Dynamic packages register data+callback
//! panes; the TUI live-looks this and paints one generic `Overlay::Slot`.
//!
//! 插槽本身不分端：正文是一段文本，[`SlotHandler::actions`] 声明可点的动作。终端按键、
//! GUI 点按钮都落到 [`SlotHandler::on_key`]（动作 id 就是那个键）。名字里的 `tui` 是
//! 历史原因——磁盘上的 Rhai 包 `inject: ["tui.slots"]`，改名会让它们挂不上。
//! 插槽增删或被操作后发 [`TUI_SLOTS_CHANGED`]，网关据此推给 GUI。

use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use indexmap::IndexMap;

use crate::names::{TUI_SLOTS, TUI_SLOTS_CHANGED};

pub enum SlotKeyResult {
    Keep,
    Close,
}

/// 插槽上一个可点的动作。`id` 就是交给 [`SlotHandler::on_key`] 的键。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotAction {
    pub id: String,
    pub label: String,
}

/// Host/TUI-owned slot body. Rhai (or tests) implement this; the TUI does
/// not paint ratatui widgets from scripts.
pub trait SlotHandler: Send + Sync {
    fn title(&self) -> String;
    fn hud(&self) -> bool;
    fn render(&self) -> String;
    fn on_key(&self, key: &str) -> SlotKeyResult;
    /// 可点的动作。没声明就只有终端按键能操作它。
    fn actions(&self) -> Vec<SlotAction> {
        Vec::new()
    }
}

pub struct SlotInfo {
    pub id: String,
    pub title: String,
    pub hud: bool,
}

struct SlotRec {
    handler: Arc<dyn SlotHandler>,
}

/// Named `tui.slots` service. Duplicate ids throw. Disposed with the fiber.
#[derive(Clone)]
pub struct TuiSlots {
    extra: Arc<Mutex<IndexMap<String, SlotRec>>>,
    open: Arc<Mutex<Option<String>>>,
    /// 发 [`TUI_SLOTS_CHANGED`] 用；测试里裸建的没有。
    ctx: Option<Context>,
}

impl TuiSlots {
    pub fn new() -> Self {
        Self {
            extra: Arc::new(Mutex::new(IndexMap::new())),
            open: Arc::new(Mutex::new(None)),
            ctx: None,
        }
    }

    fn on(ctx: Context) -> Self {
        Self {
            ctx: Some(ctx),
            ..Self::new()
        }
    }

    /// 插槽 `id` 变了（增删、正文要重画）。载荷是插槽 id。
    pub fn changed(&self, id: &str) {
        if let Some(ctx) = &self.ctx {
            ctx.emit(TUI_SLOTS_CHANGED, id.to_string());
        }
    }

    pub fn register(
        &self,
        id: String,
        handler: Arc<dyn SlotHandler>,
    ) -> cordis::Result<Disposable> {
        let id = normalize_slot_id(&id).map_err(cordis::Error::plugin)?;
        {
            let mut extra = self.extra.lock().unwrap();
            if extra.contains_key(&id) {
                return Err(cordis::Error::plugin(format!("duplicate slot {id}")));
            }
            extra.insert(id.clone(), SlotRec { handler });
        }
        self.changed(&id);
        let extra = self.extra.clone();
        let ctx = self.ctx.clone();
        Ok(Disposable::from_fn(move || {
            extra.lock().unwrap().shift_remove(&id);
            if let Some(ctx) = &ctx {
                ctx.emit(TUI_SLOTS_CHANGED, id.clone());
            }
        }))
    }

    pub fn list(&self) -> Vec<SlotInfo> {
        self.extra
            .lock()
            .unwrap()
            .iter()
            .map(|(id, rec)| SlotInfo {
                id: id.clone(),
                title: rec.handler.title(),
                hud: rec.handler.hud(),
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn SlotHandler>> {
        self.extra
            .lock()
            .unwrap()
            .get(id)
            .map(|rec| rec.handler.clone())
    }

    pub fn title(&self, id: &str) -> Option<String> {
        self.get(id).map(|h| h.title())
    }

    pub fn render(&self, id: &str) -> Option<String> {
        self.get(id).map(|h| h.render())
    }

    pub fn on_key(&self, id: &str, key: &str) -> SlotKeyResult {
        match self.get(id) {
            Some(h) => {
                let result = h.on_key(key);
                // 按键 / 点按钮多半改了正文：别的客户端（GUI）据此重拉。
                self.changed(id);
                result
            }
            None => SlotKeyResult::Close,
        }
    }

    pub fn actions(&self, id: &str) -> Vec<SlotAction> {
        self.get(id).map(|h| h.actions()).unwrap_or_default()
    }

    pub fn hud_lines(&self) -> Vec<(String, String)> {
        self.extra
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, rec)| rec.handler.hud())
            .map(|(id, rec)| {
                let text = rec.handler.render();
                let line = text.lines().next().unwrap_or("").trim();
                let line = if line.is_empty() {
                    rec.handler.title()
                } else {
                    line.to_string()
                };
                (id.clone(), line)
            })
            .collect()
    }

    pub fn request_open(&self, id: &str) -> Result<(), String> {
        let id = normalize_slot_id(id)?;
        if self.get(&id).is_none() {
            return Err(format!("slot \"{id}\" is not registered"));
        }
        *self.open.lock().unwrap() = Some(id);
        Ok(())
    }

    pub fn take_open_request(&self) -> Option<String> {
        self.open.lock().unwrap().take()
    }
}

impl Default for TuiSlots {
    fn default() -> Self {
        Self::new()
    }
}

pub fn normalize_slot_id(raw: &str) -> Result<String, String> {
    let n = raw.trim();
    if n.is_empty() {
        return Err("slot id must be non-empty".into());
    }
    let bytes = n.as_bytes();
    if !(1..=32).contains(&bytes.len()) {
        return Err("slot id must be 1–32 characters".into());
    }
    if !bytes[0].is_ascii_lowercase() {
        return Err("slot id must start with a lowercase English letter".into());
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
    {
        return Err(
            "slot id may contain only lowercase English letters, digits, hyphen, and underscore"
                .into(),
        );
    }
    Ok(n.to_string())
}

pub fn tui_slots() -> Plugin {
    plugin("tui-slots", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(TUI_SLOTS, TuiSlots::on(ctx.clone()))?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StaticSlot(&'static str);

    impl SlotHandler for StaticSlot {
        fn title(&self) -> String {
            self.0.into()
        }
        fn hud(&self) -> bool {
            true
        }
        fn render(&self) -> String {
            format!("body {}", self.0)
        }
        fn on_key(&self, key: &str) -> SlotKeyResult {
            if key == "esc" {
                SlotKeyResult::Close
            } else {
                SlotKeyResult::Keep
            }
        }
    }

    struct ActionSlot;

    impl SlotHandler for ActionSlot {
        fn title(&self) -> String {
            "计数".into()
        }
        fn hud(&self) -> bool {
            false
        }
        fn render(&self) -> String {
            "0".into()
        }
        fn on_key(&self, _key: &str) -> SlotKeyResult {
            SlotKeyResult::Keep
        }
        fn actions(&self) -> Vec<SlotAction> {
            vec![SlotAction {
                id: "inc".into(),
                label: "加一".into(),
            }]
        }
    }

    /// 插槽增删、被操作都发 `tui.slots/changed`（载荷是 id），GUI 据此重拉；
    /// 声明的动作原样可取。
    #[tokio::test]
    async fn changes_are_announced_and_actions_are_listed() {
        let root = Context::new();
        root.plugin(tui_slots(), ()).unwrap().wait().await.unwrap();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let _l = root
            .on(TUI_SLOTS_CHANGED, move |id: &String| {
                log.lock().unwrap().push(id.clone())
            })
            .unwrap();
        let slots = root.get::<TuiSlots>(TUI_SLOTS).unwrap();
        let d = slots
            .register("counter".into(), Arc::new(ActionSlot))
            .unwrap();
        assert_eq!(slots.actions("counter")[0].label, "加一");
        let _ = slots.on_key("counter", "inc");
        d.dispose_sync();
        assert_eq!(*seen.lock().unwrap(), vec!["counter"; 3]);
    }

    #[test]
    fn register_open_and_esc() {
        let slots = TuiSlots::new();
        slots
            .register("memo".into(), Arc::new(StaticSlot("便签")))
            .unwrap();
        assert_eq!(slots.list()[0].id, "memo");
        slots.request_open("memo").unwrap();
        assert_eq!(slots.take_open_request().as_deref(), Some("memo"));
        assert!(matches!(slots.on_key("memo", "esc"), SlotKeyResult::Close));
        assert!(matches!(slots.on_key("memo", "enter"), SlotKeyResult::Keep));
    }
}
