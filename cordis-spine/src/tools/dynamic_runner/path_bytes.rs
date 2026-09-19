//! Controlled workspace path → bytes for script uploads (`host.read_bytes`).
//!
//! Public contract is a **path string** (like `read_file`), never file ids or
//! embedded content. Stronger than workspace `resolve`: reject `..`, require the
//! canonical path to stay under the process cwd (workspace root), refuse
//! non-files. Symlinks are followed by canonicalize; escape outside cwd fails.
//!
//! No rhai-fs. Size capped at the HTTP request body limit (1 MiB) so uploads
//! cannot silently truncate. Empty files are allowed.

use std::path::{Component, Path, PathBuf};

use cordis::Context;

use crate::host::permissions::Permissions;
use crate::names::PERMISSIONS;

/// Same ceiling as HTTP request bodies — over limit throws (never silent truncate).
pub const MAX_READ_BYTES: usize = 1024 * 1024;

/// Resolve `raw` against cwd and ensure the canonical result stays under cwd.
pub fn resolve_workspace_file(raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("host.read_bytes: path must not be empty".into());
    }
    if raw.contains('\0') {
        return Err("host.read_bytes: path must not contain NUL".into());
    }
    let path = PathBuf::from(raw);
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("host.read_bytes: path must not contain '..'".into());
    }

    let cwd = std::env::current_dir().map_err(|e| format!("host.read_bytes: cwd: {e}"))?;
    let cwd_canon = cwd
        .canonicalize()
        .map_err(|e| format!("host.read_bytes: cwd canonicalize: {e}"))?;

    let abs = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };

    let canon = abs.canonicalize().map_err(|e| {
        format!(
            "host.read_bytes: cannot resolve {}: {e}",
            display_path(&abs, &cwd_canon)
        )
    })?;

    if !canon.starts_with(&cwd_canon) {
        return Err(format!(
            "host.read_bytes: path escapes workspace (cwd); got {}",
            canon.display()
        ));
    }
    let meta = std::fs::symlink_metadata(&canon)
        .map_err(|e| format!("host.read_bytes: metadata {}: {e}", canon.display()))?;
    if meta.file_type().is_symlink() {
        return Err("host.read_bytes: refusing symlink leaf".into());
    }
    if !meta.is_file() {
        return Err(format!(
            "host.read_bytes: not a regular file: {}",
            display_path(&canon, &cwd_canon)
        ));
    }
    Ok(canon)
}

fn display_path(path: &Path, cwd: &Path) -> String {
    path.strip_prefix(cwd)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

/// Permission gate + read. Summary carries path only — never file bytes.
pub fn read_bytes_gated(
    ctx: &Context,
    plugin_id: &str,
    raw_path: &str,
) -> Result<Vec<u8>, String> {
    let canon = resolve_workspace_file(raw_path)?;
    let cwd = std::env::current_dir().ok();
    let shown = cwd
        .as_ref()
        .and_then(|c| c.canonicalize().ok())
        .map(|c| display_path(&canon, &c))
        .unwrap_or_else(|| canon.display().to_string());

    request_permission(ctx, plugin_id, &shown)?;

    let meta = std::fs::metadata(&canon)
        .map_err(|e| format!("host.read_bytes: metadata {shown}: {e}"))?;
    let len = meta.len() as usize;
    if len > MAX_READ_BYTES {
        return Err(format!(
            "host.read_bytes: file exceeds {MAX_READ_BYTES} bytes ({len} in {shown})"
        ));
    }
    let bytes =
        std::fs::read(&canon).map_err(|e| format!("host.read_bytes: read {shown}: {e}"))?;
    if bytes.len() > MAX_READ_BYTES {
        return Err(format!(
            "host.read_bytes: file exceeds {MAX_READ_BYTES} bytes ({} in {shown})",
            bytes.len()
        ));
    }
    Ok(bytes)
}

fn request_permission(ctx: &Context, plugin_id: &str, shown_path: &str) -> Result<(), String> {
    let Some(perms) = ctx.get::<Permissions>(PERMISSIONS) else {
        return Ok(());
    };
    let gate = format!("read_bytes {shown_path}");
    let summary = format!("读取文件字节 {shown_path} （插件 {plugin_id}）");
    let handle = tokio::runtime::Handle::try_current().map_err(|e| e.to_string())?;
    let allowed = tokio::task::block_in_place(|| {
        handle.block_on(async { perms.request(&gate, &summary).await })
    });
    if !allowed {
        return Err(format!("权限被拒绝：read_bytes {shown_path}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Subdir under the real cwd (no set_current_dir — parallel-safe).
    fn scratch() -> (std::path::PathBuf, String) {
        let name = format!(
            ".tmp-path-bytes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let abs = std::env::current_dir().unwrap().join(&name);
        fs::create_dir_all(&abs).unwrap();
        (abs, name)
    }

    #[test]
    fn rejects_parent_dir_and_escape() {
        let err = resolve_workspace_file("../etc/passwd").unwrap_err();
        assert!(err.contains(".."), "{err}");
        let err = resolve_workspace_file("/etc/passwd").unwrap_err();
        assert!(
            err.contains("escapes") || err.contains("cannot resolve") || err.contains("not a"),
            "{err}"
        );
    }

    #[test]
    fn accepts_file_under_cwd() {
        let (abs, rel) = scratch();
        fs::write(abs.join("a.bin"), b"hi").unwrap();
        let got = resolve_workspace_file(&format!("{rel}/a.bin")).unwrap();
        assert_eq!(std::fs::read(&got).unwrap(), b"hi");
        fs::write(abs.join("empty"), b"").unwrap();
        resolve_workspace_file(&format!("{rel}/empty")).unwrap();
        let _ = fs::remove_dir_all(&abs);
    }

    #[test]
    fn rejects_symlink_escape() {
        let (abs, rel) = scratch();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), b"leak").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path().join("secret"), abs.join("link")).unwrap();
            let err = resolve_workspace_file(&format!("{rel}/link")).unwrap_err();
            assert!(
                err.contains("escapes") || err.contains("symlink"),
                "{err}"
            );
        }
        let _ = fs::remove_dir_all(&abs);
    }
}
