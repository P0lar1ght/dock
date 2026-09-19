//! `host.secret(name) -> String` — read a named secret for dynamic plugins.
//!
//! Lookup order:
//! 1. `$DOCK_HOME/secrets.json` (flat JSON object `name → string`)
//! 2. env `DOCK_SECRET_<NAME>` where NAME is uppercased and every non-alphanumeric
//!    char becomes `_` (e.g. `openai` → `DOCK_SECRET_OPENAI`,
//!    `aliyun-ak` → `DOCK_SECRET_ALIYUN_AK`)
//!
//! First use gates `secret {name}` via [`Permissions::request`] (summary never
//! includes the value). Missing / denied throws — never an empty-string success.
//! File perms are owner-only `0600` like MCP credentials. Not available at
//! define-time preflight (`Host` only exists on the run engine).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cordis::Context;
use cordis_base::config::dock_home;
use serde_json::Value;

use crate::host::permissions::Permissions;
use crate::names::PERMISSIONS;
use crate::tools::fs_perms::ensure_owner_only;

const SECRETS_FILENAME: &str = "secrets.json";
const ENV_PREFIX: &str = "DOCK_SECRET_";
/// `[a-zA-Z][a-zA-Z0-9_-]{0,63}` — rejects empty, path traversal (`.` `/`), etc.
const MAX_NAME_LEN: usize = 64;

pub(crate) const SECRET_BUILTIN: (&str, &str, &[&str]) = (
    "host.secret",
    "Read a named secret for Authorization headers and signed requests. Looks up `$DOCK_HOME/secrets.json` first, then env `DOCK_SECRET_<NAME>` (NAME uppercased, non-alnum → `_`: `openai` → `DOCK_SECRET_OPENAI`, `aliyun-ak` → `DOCK_SECRET_ALIYUN_AK`). First use asks permission like `http_request` (gate `secret {name}`; summary never includes the value). Missing or denied throws — never returns empty. Host-only (run engine); not available during define-time preflight.",
    &["host.secret(\"openai\") -> String"],
);

/// Validate a secret name. Rejects empty / path-like / oversized names.
pub(crate) fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("host.secret 需要非空名称".into());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "host.secret 名称过长（最多 {MAX_NAME_LEN} 字符）：{name:?}"
        ));
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err("host.secret 需要非空名称".into());
    };
    if !first.is_ascii_alphabetic() {
        return Err(format!(
            "host.secret 名称必须以字母开头（got {name:?}）；允许 [a-zA-Z][a-zA-Z0-9_-]{{0,63}}"
        ));
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(format!(
                "host.secret 名称含非法字符 {c:?}（{name:?}）；允许 [a-zA-Z][a-zA-Z0-9_-]{{0,63}}"
            ));
        }
    }
    Ok(())
}

/// Map a secret name to its env var: `aliyun-ak` → `DOCK_SECRET_ALIYUN_AK`.
pub(crate) fn env_key_for(name: &str) -> String {
    let mut out = String::with_capacity(ENV_PREFIX.len() + name.len());
    out.push_str(ENV_PREFIX);
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    out
}

pub(crate) fn default_path() -> PathBuf {
    dock_home().join(SECRETS_FILENAME)
}

/// Flat name→string store. Missing file → empty. Non-object / non-string entries skipped.
pub(crate) fn load_from(path: &Path) -> Result<BTreeMap<String, String>, String> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    if let Err(e) = ensure_owner_only(path) {
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "secrets: failed to enforce owner-only permissions"
        );
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("读取 secrets.json 失败：{e}"))?;
    let value: Value =
        serde_json::from_str(&content).map_err(|e| format!("secrets.json 不是合法 JSON：{e}"))?;
    let Value::Object(obj) = value else {
        return Err("secrets.json 必须是扁平对象 name→string".into());
    };
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        if let Value::String(s) = v {
            out.insert(k, s);
        }
    }
    Ok(out)
}

/// Lookup without permission gate — for unit tests and the gated resolver.
pub(crate) fn lookup(name: &str, store_path: &Path) -> Result<String, String> {
    validate_name(name)?;
    let store = load_from(store_path)?;
    if let Some(v) = store.get(name) {
        if !v.is_empty() {
            return Ok(v.clone());
        }
    }
    let key = env_key_for(name);
    match std::env::var(&key) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err(format!("密钥不存在：{name}")),
    }
}

/// Gate + lookup. Summary never includes the secret value.
pub(crate) fn resolve(ctx: &Context, plugin_id: &str, name: &str) -> Result<String, String> {
    validate_name(name)?;
    request_permission(ctx, plugin_id, name)?;
    lookup(name, &default_path())
}

fn request_permission(ctx: &Context, plugin_id: &str, name: &str) -> Result<(), String> {
    let Some(perms) = ctx.get::<Permissions>(PERMISSIONS) else {
        return Ok(());
    };
    let gate = format!("secret {name}");
    // Never put the secret value in the summary.
    let summary = format!("读取密钥 {name} （插件 {plugin_id}）");
    let handle = tokio::runtime::Handle::try_current().map_err(|e| e.to_string())?;
    let allowed = tokio::task::block_in_place(|| {
        handle.block_on(async { perms.request(&gate, &summary).await })
    });
    if !allowed {
        return Err(format!("权限被拒绝：secret {name}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use cordis_base::acp::PermissionOptionKind;

    use crate::host::settings::{settings, AppSettings, PermissionMode};
    use crate::names::SETTINGS;

    #[test]
    fn accepts_reasonable_names() {
        let long = "a".repeat(64);
        for name in ["openai", "aliyun-ak", "A", "x_1", "Z9-_", long.as_str()] {
            validate_name(name).unwrap_or_else(|e| panic!("{name:?}: {e}"));
        }
    }

    #[test]
    fn rejects_bad_names() {
        for name in [
            "",
            "1openai",
            "-x",
            "a.b",
            "../x",
            "a/b",
            "has space",
            &"a".repeat(65),
        ] {
            assert!(validate_name(name).is_err(), "should reject {name:?}");
        }
    }

    #[test]
    fn env_key_mapping() {
        assert_eq!(env_key_for("openai"), "DOCK_SECRET_OPENAI");
        assert_eq!(env_key_for("aliyun-ak"), "DOCK_SECRET_ALIYUN_AK");
        assert_eq!(env_key_for("my.key"), "DOCK_SECRET_MY_KEY"); // mapping only; name itself rejected
    }

    #[test]
    fn load_missing_file_is_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        let store = load_from(&path).unwrap();
        assert!(store.is_empty());
    }

    #[test]
    fn load_from_temp_path_and_enforces_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        std::fs::write(&path, r#"{"openai":"sk-test","empty":""}"#).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        let store = load_from(&path).unwrap();
        assert_eq!(store.get("openai").map(String::as_str), Some("sk-test"));
        assert_eq!(store.get("empty").map(String::as_str), Some(""));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn lookup_prefers_file_over_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        std::fs::write(&path, r#"{"openai":"from-file"}"#).unwrap();
        let _env = cordis_base::test_env::scoped().set("DOCK_SECRET_OPENAI", "from-env");
        assert_eq!(lookup("openai", &path).unwrap(), "from-file");
    }

    #[test]
    fn lookup_falls_back_to_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json"); // missing → empty
        let _env = cordis_base::test_env::scoped().set("DOCK_SECRET_OPENAI", "from-env");
        assert_eq!(lookup("openai", &path).unwrap(), "from-env");
    }

    #[test]
    fn lookup_missing_is_error_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        // Hold the env lock and clear any DOCK_SECRET_OPENAI left by sibling tests.
        let _env = cordis_base::test_env::scoped().remove("DOCK_SECRET_OPENAI");
        let err = lookup("openai", &path).unwrap_err();
        assert!(err.contains("不存在"), "{err}");
        assert!(!err.is_empty());
    }

    #[test]
    fn lookup_empty_file_value_falls_through_to_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        std::fs::write(&path, r#"{"openai":""}"#).unwrap();
        let _env = cordis_base::test_env::scoped().set("DOCK_SECRET_OPENAI", "from-env");
        assert_eq!(lookup("openai", &path).unwrap(), "from-env");
    }

    #[test]
    fn does_not_read_arbitrary_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        let _env = cordis_base::test_env::scoped()
            .set("OPENAI", "leaked")
            .remove("DOCK_SECRET_OPENAI");
        let err = lookup("openai", &path).unwrap_err();
        assert!(err.contains("不存在"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn permission_deny_throws() {
        let ctx = Context::new();
        ctx.plugin(settings(), ()).unwrap().wait().await.unwrap();
        ctx.plugin(crate::host::permissions::permissions(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        // Default mode is Ask — queue a reject once the prompt appears.
        let perms = ctx.get::<Permissions>(PERMISSIONS).unwrap();
        let ctx_resolve = ctx.clone();
        let resolve_task =
            tokio::task::spawn_blocking(move || resolve(&ctx_resolve, "plug-1", "openai"));
        let mut saw = false;
        for _ in 0..100 {
            if perms.front().is_some() {
                saw = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(saw, "expected a permission prompt for secret openai");
        let prompt = perms.front().unwrap();
        assert_eq!(prompt.tool, "secret openai");
        assert!(prompt.summary.contains("openai"), "{prompt:?}");
        assert!(!prompt.summary.contains("sk-"), "{prompt:?}");
        assert!(perms.resolve(PermissionOptionKind::RejectOnce));
        let err = resolve_task.await.unwrap().unwrap_err();
        assert!(err.contains("权限被拒绝"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn permission_allow_mode_skips_prompt() {
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_SECRET_OPENAI", "sk-allowed");
        // home() set DOCK_HOME; write nothing so env fallback wins
        let ctx = Context::new();
        ctx.plugin(settings(), ()).unwrap().wait().await.unwrap();
        ctx.plugin(crate::host::permissions::permissions(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        ctx.get::<AppSettings>(SETTINGS)
            .unwrap()
            .set_permission_mode(PermissionMode::Allow);
        let value = tokio::task::spawn_blocking({
            let ctx = ctx.clone();
            move || resolve(&ctx, "plug-1", "openai")
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(value, "sk-allowed");
        assert!(ctx
            .get::<Permissions>(PERMISSIONS)
            .unwrap()
            .front()
            .is_none());
    }
}
