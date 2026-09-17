//! Named `"tui.slots"` service. Dynamic packages register data+callback
//! panes; the TUI live-looks this and paints one generic `Overlay::Slot`.

use std::sync::{Arc, Mutex};

use cordis::{plugin, Disposable, Inject, Plugin};
use indexmap::IndexMap;

use crate::names::TUI_SLOTS;

pub enum SlotKeyResult {
    Keep,
    Close,
}

/// Host/TUI-owned slot body. Rhai (or tests) implement this; the TUI does
/// not paint ratatui widgets from scripts.
pub trait SlotHandler: Send + Sync {
    fn title(&self) -> String;
    fn hud(&self) -> bool;
    fn render(&self) -> String;
    fn on_key(&self, key: &str) -> SlotKeyResult;
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
}

impl TuiSlots {
    pub fn new() -> Self {
        Self {
            extra: Arc::new(Mutex::new(IndexMap::new())),
            open: Arc::new(Mutex::new(None)),
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
        let extra = self.extra.clone();
        Ok(Disposable::from_fn(move || {
            extra.lock().unwrap().shift_remove(&id);
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
            Some(h) => h.on_key(key),
            None => SlotKeyResult::Close,
        }
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
        Ok(Some(ctx.provide(TUI_SLOTS, TuiSlots::new())?))
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
