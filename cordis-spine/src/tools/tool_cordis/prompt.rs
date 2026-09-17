//! Model guidance for the Cordis dynamic-plugin tools.

/// Pointer only. Authoring contract lives in
/// `skills/cordis-plugin-development/SKILL.md` (load via `skill` / slash).
pub const CORDIS_SYSTEM_PROMPT: &str =
    "动态 Host 扩展用 search_tool 查 cordis，再用 use_tool 调用。\
写插件前加载 cordis-plugin-development 技能（斜杠 /cordis-plugin-development 或 skill 工具）。";
