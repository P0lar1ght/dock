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
        let model = model.into();
        let settings = Self {
            model: Mutex::new(model.clone()),
            effort: Mutex::new(String::new()),
            thinking: Mutex::new(true),
            timestamps: Mutex::new(true),
            permission_mode: Mutex::new(PermissionMode::Ask),
            mermaid_engine: Mutex::new(MermaidEngineKind::Pure),
        };
        settings.seed_from_model(&model);
        settings
    }

    pub fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    /// 切模型要重新播种推理默认值：强度和思考开关是**每个模型自己的**属性，
    /// 把上一个模型的档位带过去正是"harness 固定配置"那类错的来源。
    pub fn set_model(&self, model: impl Into<String>) {
        let model = model.into();
        *self.model.lock().unwrap() = model.clone();
        self.seed_from_model(&model);
    }

    /// `[model.<id>]` 的 `reasoning` / `reasoning_effort` → 运行时默认值。
    ///
    /// 目录里没这个模型时**也要清零**：早退会把上一个模型的档位留给下一个，
    /// 正是这个函数要避免的事。未知模型 = 什么都不假设 = 空强度（不发）+ 思考开。
    fn seed_from_model(&self, model: &str) {
        let choice = config::lookup_model(model);
        *self.effort.lock().unwrap() = choice
            .as_ref()
            .map(config::ModelChoice::default_effort)
            .unwrap_or_default();
        *self.thinking.lock().unwrap() = choice
            .as_ref()
            .is_none_or(config::ModelChoice::supports_reasoning);
    }

    /// 模型自己说了没有推理档时，思考开关是死的（UI 据此置灰）。
    pub fn reasoning_available(&self) -> bool {
        config::lookup_model(&self.model()).is_none_or(|c| c.supports_reasoning())
    }

    /// 当前模型认识哪几档强度，`/effort` 与设置面板照着列。目录里没有这个模型
    /// 就给通用四档。`reasoning = false` 的模型返回空。
    pub fn effort_choices(&self) -> Vec<String> {
        match config::lookup_model(&self.model()) {
            Some(choice) => choice.effort_choices(),
            None => config::DEFAULT_EFFORT_CHOICES
                .iter()
                .map(|e| (*e).to_string())
                .collect(),
        }
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

    /// 不支持推理的模型扳不动，始终返回 false。
    pub fn toggle_thinking(&self) -> bool {
        if !self.reasoning_available() {
            *self.thinking.lock().unwrap() = false;
            return false;
        }
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
        // 没有 DOCK_MODEL、没有 [models].default、目录也空：留空字符串。以前这里
        // 兜底成 "grok-4"，等于凭空造一个没端点的模型出来。
        let model = std::env::var("DOCK_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(config::load_default_model)
            .or_else(|| config::load_catalog().first().map(|m| m.id.clone()))
            .unwrap_or_default();
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

    /// 目录里没有这个模型：强度留空（= 不发），思考照常开着。
    #[test]
    fn unknown_model_keeps_empty_effort() {
        let settings = AppSettings::new("not-in-any-catalog");
        assert_eq!(settings.effort(), "");
        assert!(settings.thinking());
        assert!(settings.reasoning_available());
    }

    /// 切到目录外的模型必须把上一个模型的档位清掉——早退会把 "high" 带过去，
    /// 正是 `seed_from_model` 要避免的事。
    #[test]
    fn switching_to_an_unknown_model_clears_the_previous_effort() {
        let settings = AppSettings::new("x");
        settings.set_effort("high");
        settings.set_thinking(false);
        settings.set_model("still-not-in-any-catalog");
        assert_eq!(settings.effort(), "", "强度要清零");
        assert!(settings.thinking(), "未知模型不假设它不能思考");
    }

    /// 目录里没有的模型给通用四档，够用又不假装知道它真支持什么。
    #[test]
    fn effort_choices_fall_back_to_the_generic_ladder() {
        let settings = AppSettings::new("not-in-any-catalog");
        assert_eq!(settings.effort_choices(), config::DEFAULT_EFFORT_CHOICES);
    }

    /// `reasoning_efforts` 三态：没写 = 通用四档；写了列表 = 就这几档；
    /// **显式写空列表 = 会推理但不接受档位参数**（菜单空着）。
    #[test]
    fn reasoning_efforts_distinguishes_unset_from_explicitly_empty() {
        let mut choice = config::ModelChoice {
            id: "m".into(),
            name: "m".into(),
            description: String::new(),
            api_base_url: None,
            api_key: None,
            env_key: None,
            context_window: None,
            api_backend: config::ApiBackend::ChatCompletions,
            auth_scheme: None,
            api_model: None,
            prompt_cache: None,
            max_output_tokens: None,
            reasoning: None,
            reasoning_effort: None,
            reasoning_efforts: None,
            supports_images: None,
        };
        assert_eq!(choice.effort_choices(), config::DEFAULT_EFFORT_CHOICES);

        choice.reasoning_efforts = Some(vec!["low".into(), "high".into()]);
        assert_eq!(choice.effort_choices(), vec!["low", "high"]);

        choice.reasoning_efforts = Some(Vec::new());
        assert!(
            choice.effort_choices().is_empty(),
            "写空列表就是明说没有档位可选，不该退回通用四档"
        );

        choice.reasoning = Some(false);
        choice.reasoning_efforts = Some(vec!["low".into()]);
        assert!(
            choice.effort_choices().is_empty(),
            "不支持推理时档位列表无意义"
        );
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
