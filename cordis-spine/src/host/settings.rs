use std::sync::Mutex;

use cordis::{plugin, Inject, Plugin};

use crate::names::SETTINGS;
use cordis_base::config::{self, ApiBackend, ModelChoice};

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

/// 单次委派的采样覆写，挂在**子会话**的 `"model-override"` 上。
///
/// 主会话永远没有这一项 = 跟 [`AppSettings`] 走。之所以另起一个服务而不是给子
/// 会话隔离一份 `AppSettings`：那里面还有权限档位、时间戳、思考开关这些**会话
/// 级**状态，隔离一份等于让子代理带着一张过期的权限快照跑。
///
/// 只有 workflow 脚本的 `agent(model:, effort:, max_output_tokens:)` 会填它。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelOverride {
    /// 必须是用户模型目录（`config.toml`）里真有的 id——不在目录里的名字解析不出
    /// 端点，发出去就是一个 404。校验在 workflow host 那边做，到这里的都已经过关。
    pub model: Option<String>,
    /// 推理强度。模型不支持推理时无效（那时一律不发强度）。
    pub effort: Option<String>,
    pub max_output_tokens: Option<u32>,
}

impl ModelOverride {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Live-looked-up app settings. Do not capture this `Arc` in a long-lived closure.
#[derive(Debug)]
pub struct AppSettings {
    model: Mutex<String>,
    effort: Mutex<String>,
    /// 当前这一轮走哪条 wire。种子来自 `[model.<id>].api_backends` 的第一条，
    /// `/protocol` 在**该模型声明过的**协议之间切。
    backend: Mutex<ApiBackend>,
    /// When false, requests send `reasoning.exclude` / effort none (no think card).
    thinking: Mutex<bool>,
    timestamps: Mutex<bool>,
    permission_mode: Mutex<PermissionMode>,
    mermaid_engine: Mutex<MermaidEngineKind>,
}

impl AppSettings {
    /// 新开一页时抄一份。之后两页各改各的：模型、协议、权限模式不再串页。
    ///
    /// 不走 [`Self::new`]：那个会按模型重新播种，把这一页已经选好的协议和强度清掉。
    pub fn fork(&self) -> Self {
        Self {
            model: Mutex::new(self.model()),
            effort: Mutex::new(self.effort()),
            backend: Mutex::new(*self.backend.lock().unwrap()),
            thinking: Mutex::new(self.thinking()),
            timestamps: Mutex::new(self.timestamps()),
            permission_mode: Mutex::new(self.permission_mode()),
            mermaid_engine: Mutex::new(self.mermaid_engine()),
        }
    }

    pub fn new(model: impl Into<String>) -> Self {
        let model = model.into();
        let settings = Self {
            model: Mutex::new(model.clone()),
            effort: Mutex::new(String::new()),
            backend: Mutex::new(ApiBackend::default()),
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
        // 协议同理：上一个模型选的 wire 带到下一个端点上就是 404。目录里没有
        // 这个模型时退回全局默认，不留着前一个的。
        *self.backend.lock().unwrap() = choice
            .as_ref()
            .map(config::ModelChoice::default_backend)
            .unwrap_or_default();
    }

    /// 当前模型声明支持哪几条 wire。目录里没有这个模型就给通用三条——够用，
    /// 又不假装知道那个端点开了什么。
    pub fn backend_choices(&self) -> Vec<ApiBackend> {
        match config::lookup_model(&self.model()) {
            Some(choice) => choice.api_backends.clone(),
            None => ApiBackend::ALL.to_vec(),
        }
    }

    /// 这一轮实际走哪条。config.toml 是热读的，存着的值可能已经不在声明列表里
    /// （用户刚把那条协议删了），所以每次都对着当前目录校一遍。
    pub fn backend(&self) -> ApiBackend {
        let current = *self.backend.lock().unwrap();
        match config::lookup_model(&self.model()) {
            Some(choice) if !choice.supports_backend(current) => choice.default_backend(),
            _ => current,
        }
    }

    pub fn set_backend(&self, backend: ApiBackend) {
        *self.backend.lock().unwrap() = backend;
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
    fn fork_keeps_model_protocol_and_mode_until_the_copy_changes() {
        let settings = AppSettings::new("x");
        settings.set_permission_mode(PermissionMode::Allow);
        settings.set_backend(ApiBackend::Messages);
        let other = settings.fork();
        assert_eq!(other.model(), "x");
        assert_eq!(other.permission_mode(), PermissionMode::Allow);
        assert_eq!(other.backend(), ApiBackend::Messages);
        other.set_model("page-two");
        other.set_permission_mode(PermissionMode::Ask);
        assert_eq!(settings.model(), "x");
        assert_eq!(settings.permission_mode(), PermissionMode::Allow);
        assert_eq!(other.model(), "page-two");
    }

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
            api_backends: vec![config::ApiBackend::ChatCompletions],
            backend_overrides: Default::default(),
            pricing: None,
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

    /// 目录里没有这个模型：协议给通用三条，默认落在 Responses。
    #[test]
    fn unknown_model_offers_every_wire() {
        let settings = AppSettings::new("not-in-any-catalog");
        assert_eq!(settings.backend(), ApiBackend::Responses);
        assert_eq!(settings.backend_choices(), ApiBackend::ALL.to_vec());
    }

    /// 切模型必须把上一个模型选的协议清掉——把 messages 带到一个只开
    /// /chat/completions 的端点上就是 404。
    #[test]
    fn switching_models_reseeds_the_protocol() {
        let settings = AppSettings::new("x");
        settings.set_backend(ApiBackend::Messages);
        assert_eq!(settings.backend(), ApiBackend::Messages);
        settings.set_model("still-not-in-any-catalog");
        assert_eq!(settings.backend(), ApiBackend::Responses, "协议要重新播种");
    }

    /// 声明了两条 wire 的端点：`/protocol` 列这两条，切过去生效；config 里没
    /// 声明的那条被存进来（用户切完又改了 config）时，`backend()` 要挡回默认，
    /// 而不是照发一个 404 的路径出去。
    #[test]
    fn protocol_switches_within_the_declared_list_only() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            r#"
[model.dual]
api_base_url = "https://example.test/v1"
api_backends = ["responses", "chat_completions"]
"#,
        )
        .unwrap();
        let _env = cordis_base::test_env::scoped()
            .set("DOCK_HOME", home.path())
            .cwd(cwd.path());

        let settings = AppSettings::new("dual");
        assert_eq!(settings.backend(), ApiBackend::Responses);
        assert_eq!(
            settings.backend_choices(),
            vec![ApiBackend::Responses, ApiBackend::ChatCompletions]
        );

        settings.set_backend(ApiBackend::ChatCompletions);
        assert_eq!(settings.backend(), ApiBackend::ChatCompletions);

        settings.set_backend(ApiBackend::Messages);
        assert_eq!(
            settings.backend(),
            ApiBackend::Responses,
            "没声明的协议不能生效，要退回该模型的默认"
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
