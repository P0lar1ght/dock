//! Shared resume → preset apply for every restore path.
//!
//! [`Sessions::restore`] only copies `live_preset_id` from meta. Callers must
//! then run [`apply_restored_preset`] with the **archived** `preset_id` field
//! (not `Sessions::preset_id()` after restore — that may still hold a startup
//! seed when the field was absent).

use crate::agent::presets::AgentPresets;

/// Result of applying a restored session's optional `preset_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyRestoredPreset {
    /// Meta had no `preset_id` (old sessions) — leave `AgentPresets` alone.
    SkippedAbsent,
    /// `AgentPresets::apply` succeeded.
    Applied(String),
    /// Unknown / broken id: fail-open — live preset unchanged; caller must
    /// flash/log (do not `let _ =` swallow).
    Failed { id: String, error: String },
}

/// Apply the preset stamped on a restored session.
///
/// - `Some(id)` → [`AgentPresets::apply`]; on `Err`, return [`ApplyRestoredPreset::Failed`]
///   without changing the live preset (`apply` already leaves `current` alone).
/// - `None` / empty → [`ApplyRestoredPreset::SkippedAbsent`].
pub fn apply_restored_preset(
    presets: &AgentPresets,
    preset_id: Option<&str>,
) -> ApplyRestoredPreset {
    let Some(id) = preset_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return ApplyRestoredPreset::SkippedAbsent;
    };
    match presets.apply(id) {
        Ok(_) => ApplyRestoredPreset::Applied(id.to_string()),
        Err(error) => ApplyRestoredPreset::Failed {
            id: id.to_string(),
            error,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::presets::{AgentPresets, DEFAULT_PRESET_ID, WARDEN_PRESET_ID};
    use tempfile::tempdir;

    #[test]
    fn absent_skips_without_touching_current() {
        let dir = tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        assert_eq!(presets.current_id(), DEFAULT_PRESET_ID);
        assert_eq!(
            apply_restored_preset(&presets, None),
            ApplyRestoredPreset::SkippedAbsent
        );
        assert_eq!(
            apply_restored_preset(&presets, Some("")),
            ApplyRestoredPreset::SkippedAbsent
        );
        assert_eq!(
            apply_restored_preset(&presets, Some("   ")),
            ApplyRestoredPreset::SkippedAbsent
        );
        assert_eq!(presets.current_id(), DEFAULT_PRESET_ID);
    }

    #[test]
    fn known_id_applies() {
        let dir = tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        assert_eq!(
            apply_restored_preset(&presets, Some(WARDEN_PRESET_ID)),
            ApplyRestoredPreset::Applied(WARDEN_PRESET_ID.into())
        );
        assert_eq!(presets.current_id(), WARDEN_PRESET_ID);
    }

    #[test]
    fn unknown_id_fail_open_leaves_current() {
        let dir = tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let before = presets.current_id();
        match apply_restored_preset(&presets, Some("no-such-preset")) {
            ApplyRestoredPreset::Failed { id, error } => {
                assert_eq!(id, "no-such-preset");
                assert!(!error.is_empty(), "error must be reported, not swallowed");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(presets.current_id(), before);
    }
}
