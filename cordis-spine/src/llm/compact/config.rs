//! grok-build compaction configuration.
//!
//! Copied from `xai-grok-compaction/src/code_compaction/config.rs`. Trigger
//! wiring (pre-sampling checks, suppression) stays on the Cordis plugin.

/// Default auto-compact threshold (% of context window) when no other source
/// sets it. Shared by grok-build and Grok chat (~85% trigger on both sides).
pub const DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT: u8 = 85;

/// Minimum character count for a cleaned summary seed.
///
/// grok-build retries when the cleaned summary is shorter than this — the
/// smallest healthy prod summary observed was ~3,242 chars; anything under
/// 500 is treated as degenerate and retried like a transient failure.
pub const MIN_SUMMARY_SEED_CHARS: usize = 500;

/// Tunables for the full-replace pass.
///
/// Copied from grok `FullReplaceConfig`. `sampling_timeout_secs` is unused
/// here: Dock cancel is `"turn"`, not a compact-local timer.
#[derive(Debug, Clone)]
pub struct FullReplaceConfig {
    /// Total LLM attempts (first try + retries) on transient failures.
    pub max_attempts: u32,
    /// Delay between transient retries.
    pub retry_delay_secs: u64,
}

impl Default for FullReplaceConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            retry_delay_secs: 3,
        }
    }
}

/// `used * 100 >= window * percent`. False when `window == 0`.
///
/// Copied from grok `xai-token-estimation::exceeds_threshold`.
pub fn exceeds_threshold(used: u64, window: u64, threshold_percent: u8) -> bool {
    if window == 0 {
        return false;
    }
    used.saturating_mul(100) >= window.saturating_mul(threshold_percent as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exceeds_threshold_matches_grok_integer_gate() {
        assert!(!exceeds_threshold(84, 100, 85));
        assert!(exceeds_threshold(85, 100, 85));
        assert!(exceeds_threshold(90, 100, 85));
        assert!(!exceeds_threshold(0, 100, 85));
        assert!(!exceeds_threshold(100, 0, 85));
        assert!(exceeds_threshold(850, 1000, 85));
        assert!(!exceeds_threshold(849, 1000, 85));
    }
}
