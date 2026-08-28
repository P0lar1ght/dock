/// Default max time to wait for the user to answer the questionnaire (all
/// questions in this tool call share one timer): 30 minutes. On expiry the
/// tool returns the same skipped/cancel text as a user dismiss
/// (`format::unanswered_text`), not a tool failure.
///
/// The shell resolves `[toolset.ask_user_question]` across its config tiers
/// and injects the result as [`AskUserQuestionParams`]; when no resolved
/// params are injected, `GROK_ASK_USER_QUESTION_TIMEOUT_SECS` (positive
/// integer seconds) still overrides this default directly —
/// e.g. `GROK_ASK_USER_QUESTION_TIMEOUT_SECS=8` for tests / TUI repro.
pub const RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Default for `timeout_enabled` across every resolver tier and settings
/// surface: the questionnaire timer is armed unless something disarms it.
/// Single source — the shell resolver's `.default(...)` and the pager's
/// settings registry both anchor on this const.
pub const DEFAULT_ASK_USER_QUESTION_TIMEOUT_ENABLED: bool = true;

/// Env var: override [`RESPONSE_TIMEOUT`] with a duration in **seconds**.
pub const RESPONSE_TIMEOUT_ENV: &str = "GROK_ASK_USER_QUESTION_TIMEOUT_SECS";

/// Parse the [`RESPONSE_TIMEOUT_ENV`] override (positive integer seconds).
/// Invalid or non-positive values are warned and treated as unset. Single
/// source for this parse — the shell's env tier calls it too, so the two
/// resolutions can't drift.
pub fn response_timeout_env_secs() -> Option<u64> {
    let raw = std::env::var(RESPONSE_TIMEOUT_ENV).ok()?;
    match raw.trim().parse::<u64>() {
        Ok(secs) if secs > 0 => Some(secs),
        _ => None,
    }
}

/// Effective wait budget for one questionnaire (env override or default).
pub fn response_timeout() -> std::time::Duration {
    response_timeout_env_secs()
        .map(std::time::Duration::from_secs)
        .unwrap_or(RESPONSE_TIMEOUT)
}
