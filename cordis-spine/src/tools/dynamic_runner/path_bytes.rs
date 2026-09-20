//! Path → bytes for script uploads (`host.read_bytes`).
//!
//! Public contract is a **path string** (like `read_file`), never file ids or
//! embedded content. Reachability matches `read_file`: any path the user can
//! name — relative to cwd, absolute, or `~`-prefixed. The control is the
//! permission gate (`read_bytes {path}`, per path), not a directory allowlist;
//! uploading a file the model can already `read_file` should not need a
//! different vocabulary.
//!
//! No rhai-fs: no directory walks, no writes, no globs — one file per call.
//!
//! Reads are capped at [`MAX_READ_BYTES`]; over that throws, so an upload is
//! never silently truncated. Empty files are allowed.
//!
//! Bytes read here are fingerprinted into a small ring ([`remember_file_bytes`])
//! so `rhai_http` can tell a **file reference** from model-generated content and
//! grant it the larger request-body ceiling. Model-built bodies stay at 1 MiB.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use cordis::Context;
use sha2::{Digest, Sha256};

use crate::host::permissions::Permissions;
use crate::names::PERMISSIONS;

/// Upload ceiling for file-referenced bytes. Larger than the inline request-body
/// cap on purpose: big payloads are allowed *only* by reference, never inline.
pub const MAX_READ_BYTES: usize = 16 * 1024 * 1024;

/// How many recent file reads stay fingerprinted. Provenance is per-run scratch,
/// not a durable store — a script that re-reads gets a fresh entry.
const DIGEST_MEMORY: usize = 64;

fn digests() -> &'static Mutex<VecDeque<[u8; 32]>> {
    static DIGESTS: OnceLock<Mutex<VecDeque<[u8; 32]>>> = OnceLock::new();
    DIGESTS.get_or_init(|| Mutex::new(VecDeque::with_capacity(DIGEST_MEMORY)))
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Record that `bytes` came off disk through the gate above.
pub fn remember_file_bytes(bytes: &[u8]) {
    let d = digest(bytes);
    let mut ring = digests().lock().unwrap();
    if ring.iter().any(|seen| seen == &d) {
        return;
    }
    if ring.len() >= DIGEST_MEMORY {
        ring.pop_front();
    }
    ring.push_back(d);
}

/// True when `bytes` are byte-identical to something `host.read_bytes` returned.
/// Mutating a blob drops the provenance — an edited payload is model content again.
pub fn is_file_sourced(bytes: &[u8]) -> bool {
    let d = digest(bytes);
    digests().lock().unwrap().iter().any(|seen| seen == &d)
}

fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Expand a leading `~`, then resolve against cwd and canonicalize.
/// Any readable file is fair game — the permission gate is the control.
pub fn resolve_readable_file(raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("host.read_bytes: path must not be empty".into());
    }
    if raw.contains('\0') {
        return Err("host.read_bytes: path must not contain NUL".into());
    }

    let path = if raw == "~" {
        home_dir().ok_or_else(|| "host.read_bytes: cannot resolve '~' (no HOME)".to_string())?
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home_dir()
            .ok_or_else(|| "host.read_bytes: cannot resolve '~' (no HOME)".to_string())?
            .join(rest)
    } else {
        PathBuf::from(raw)
    };

    let abs = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| format!("host.read_bytes: cwd: {e}"))?
            .join(path)
    };

    let canon = abs
        .canonicalize()
        .map_err(|e| format!("host.read_bytes: cannot resolve {}: {e}", abs.display()))?;

    // canonicalize() already followed every symlink, so this is the real target.
    if !canon.is_file() {
        return Err(format!(
            "host.read_bytes: not a regular file: {}",
            canon.display()
        ));
    }
    Ok(canon)
}

/// Workspace-relative when the file sits under cwd, absolute otherwise — the
/// permission prompt must show enough for the user to judge what they approve.
fn display_path(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| cwd.canonicalize().ok())
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(|p| p.display().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

/// Permission gate + read. Summary carries the path only — never file bytes.
pub fn read_bytes_gated(ctx: &Context, plugin_id: &str, raw_path: &str) -> Result<Vec<u8>, String> {
    let canon = resolve_readable_file(raw_path)?;
    let shown = display_path(&canon);

    request_permission(ctx, plugin_id, &shown)?;

    let meta =
        std::fs::metadata(&canon).map_err(|e| format!("host.read_bytes: metadata {shown}: {e}"))?;
    let len = meta.len() as usize;
    if len > MAX_READ_BYTES {
        return Err(format!(
            "host.read_bytes: file exceeds {MAX_READ_BYTES} bytes ({len} in {shown})"
        ));
    }
    let bytes = std::fs::read(&canon).map_err(|e| format!("host.read_bytes: read {shown}: {e}"))?;
    // Re-check: the file may have grown between metadata and read.
    if bytes.len() > MAX_READ_BYTES {
        return Err(format!(
            "host.read_bytes: file exceeds {MAX_READ_BYTES} bytes ({} in {shown})",
            bytes.len()
        ));
    }
    remember_file_bytes(&bytes);
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
    fn rejects_empty_missing_and_directories() {
        assert!(resolve_readable_file("   ").unwrap_err().contains("empty"));
        assert!(resolve_readable_file("no/such/file-xyz")
            .unwrap_err()
            .contains("cannot resolve"));
        let (abs, rel) = scratch();
        let err = resolve_readable_file(&rel).unwrap_err();
        assert!(err.contains("not a regular file"), "{err}");
        let _ = fs::remove_dir_all(&abs);
    }

    #[test]
    fn accepts_file_under_cwd() {
        let (abs, rel) = scratch();
        fs::write(abs.join("a.bin"), b"hi").unwrap();
        let got = resolve_readable_file(&format!("{rel}/a.bin")).unwrap();
        assert_eq!(std::fs::read(&got).unwrap(), b"hi");
        fs::write(abs.join("empty"), b"").unwrap();
        resolve_readable_file(&format!("{rel}/empty")).unwrap();
        let _ = fs::remove_dir_all(&abs);
    }

    /// Convenience is the point now: `..`, absolute and symlinked paths resolve.
    #[test]
    fn accepts_parent_dir_absolute_and_symlink_targets() {
        let (abs, rel) = scratch();
        fs::write(abs.join("t.bin"), b"data").unwrap();

        let up = format!("{rel}/../{rel}/t.bin");
        assert_eq!(resolve_readable_file(&up).unwrap(), abs.join("t.bin"));

        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.bin");
        fs::write(&target, b"outside").unwrap();
        let got = resolve_readable_file(target.to_str().unwrap()).unwrap();
        assert_eq!(fs::read(got).unwrap(), b"outside");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, abs.join("link")).unwrap();
            let via_link = resolve_readable_file(&format!("{rel}/link")).unwrap();
            assert_eq!(via_link, target.canonicalize().unwrap());
        }
        let _ = fs::remove_dir_all(&abs);
    }

    #[test]
    fn tilde_expands_to_home() {
        let Some(home) = home_dir() else { return };
        let probe = home.join(format!(".tmp-dock-tilde-{}", std::process::id()));
        fs::write(&probe, b"tilde").unwrap();
        let name = probe.file_name().unwrap().to_str().unwrap().to_string();
        let got = resolve_readable_file(&format!("~/{name}")).unwrap();
        assert_eq!(fs::read(&got).unwrap(), b"tilde");
        let _ = fs::remove_file(&probe);
    }

    /// Both provenance tests mutate one process-wide ring, so they must not
    /// interleave — the bounded test would evict the other's entry mid-assert.
    fn ring_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn provenance_tracks_exact_bytes_only() {
        let _guard = ring_guard();
        let payload = format!(
            "prov-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        )
        .into_bytes();
        assert!(!is_file_sourced(&payload));
        remember_file_bytes(&payload);
        assert!(is_file_sourced(&payload));

        let mut mutated = payload.clone();
        mutated.push(b'!');
        assert!(
            !is_file_sourced(&mutated),
            "mutating a blob must drop file provenance"
        );
    }

    #[test]
    fn provenance_ring_is_bounded() {
        let _guard = ring_guard();
        let tag = format!(
            "ring-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        let first = format!("{tag}-0").into_bytes();
        remember_file_bytes(&first);
        for i in 1..=DIGEST_MEMORY {
            remember_file_bytes(format!("{tag}-{i}").as_bytes());
        }
        assert!(!is_file_sourced(&first), "oldest entry should be evicted");
        assert!(digests().lock().unwrap().len() <= DIGEST_MEMORY);
    }
}
