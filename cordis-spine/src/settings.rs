use std::sync::Mutex;

use cordis::{plugin, Inject, Plugin};

use crate::config::{self, ModelChoice};
use crate::names::SETTINGS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    Ask,
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MermaidEngineKind {
    Pure,
    Mmdc,
}

/// Live-looked-up app settings. Do not capture this `Arc` in a long-lived closure.
#[derive(Debug)]
pub struct AppSettings {
    model: Mutex<String>,
    effort: Mutex<String>,
    /// When false, requests send `reasoning.exclude` / effort none (no think card).
    thinking: Mutex<bool>,
    timestamps: Mutex<bool>,
    permission_mode: Mutex<PermissionMode>,
    mermaid_engine: Mutex<MermaidEngineKind>,
}

impl AppSettings {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: Mutex::new(model.into()),
            effort: Mutex::new("medium".into()),
            thinking: Mutex::new(true),
            timestamps: Mutex::new(true),
            permission_mode: Mutex::new(PermissionMode::Ask),
            mermaid_engine: Mutex::new(MermaidEngineKind::Pure),
        }
    }

    pub fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    pub fn set_model(&self, model: impl Into<String>) {
        *self.model.lock().unwrap() = model.into();
    }

    pub fn effort(&self) -> String {
        self.effort.lock().unwrap().clone()
    }

    pub fn set_effort(&self, effort: impl Into<String>) {
        *self.effort.lock().unwrap() = effort.into();
    }

    pub fn thinking(&self) -> bool {
        *self.thinking.lock().unwrap()
    }

    pub fn set_thinking(&self, on: bool) {
        *self.thinking.lock().unwrap() = on;
    }

    pub fn toggle_thinking(&self) -> bool {
        let mut on = self.thinking.lock().unwrap();
        *on = !*on;
        *on
    }

    pub fn timestamps(&self) -> bool {
        *self.timestamps.lock().unwrap()
    }

    pub fn set_timestamps(&self, on: bool) {
        *self.timestamps.lock().unwrap() = on;
    }

    pub fn toggle_timestamps(&self) -> bool {
        let mut on = self.timestamps.lock().unwrap();
        *on = !*on;
        *on
    }

    pub fn permission_mode(&self) -> PermissionMode {
        *self.permission_mode.lock().unwrap()
    }

    pub fn set_permission_mode(&self, mode: PermissionMode) {
        *self.permission_mode.lock().unwrap() = mode;
    }

    /// Settings modal Ask ↔ Always-allow. Shift+Tab session cycle lives in
    /// the TUI (`mode_cycle`): Normal → Plan → Always-Approve → Normal.
    pub fn cycle_permission_mode(&self) -> PermissionMode {
        let next = match self.permission_mode() {
            PermissionMode::Ask => PermissionMode::Allow,
            PermissionMode::Allow => PermissionMode::Ask,
        };
        self.set_permission_mode(next);
        next
    }

    pub fn mermaid_engine(&self) -> MermaidEngineKind {
        *self.mermaid_engine.lock().unwrap()
    }

    pub fn set_mermaid_engine(&self, kind: MermaidEngineKind) {
        *self.mermaid_engine.lock().unwrap() = kind;
    }

    /// Live-read from config.toml. Do not store this Vec on a long-lived Arc.
    pub fn catalog(&self) -> Vec<ModelChoice> {
        config::load_catalog()
    }
}

pub fn settings() -> Plugin {
    plugin("settings", Inject::new(), |ctx, _: &()| {
        let model = std::env::var("DOCK_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(config::load_default_model)
            .or_else(|| config::load_catalog().first().map(|m| m.id.clone()))
            .unwrap_or_else(|| "grok-4".into());
        Ok(Some(ctx.provide(SETTINGS, AppSettings::new(model))?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_default_on_like_grok() {
        let settings = AppSettings::new("x");
        assert!(settings.timestamps());
        assert!(!settings.toggle_timestamps());
        assert!(settings.toggle_timestamps());
    }

    #[test]
    fn thinking_defaults_on_and_toggles() {
        let settings = AppSettings::new("x");
        assert!(settings.thinking());
        assert!(!settings.toggle_thinking());
        assert!(settings.toggle_thinking());
    }

    #[test]
    fn settings_modal_cycles_ask_and_allow() {
        let settings = AppSettings::new("x");
        assert_eq!(settings.permission_mode(), PermissionMode::Ask);
        assert_eq!(settings.cycle_permission_mode(), PermissionMode::Allow);
        assert_eq!(settings.cycle_permission_mode(), PermissionMode::Ask);
    }
}
