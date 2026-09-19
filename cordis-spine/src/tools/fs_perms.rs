//! Shared file-permission helpers for secrets / credentials on disk.

use std::path::Path;

/// Ensure `path` is owner-read/write only (`0600`) on Unix. Missing path is OK.
/// Non-Unix is a no-op (same contract as the former MCP helper).
pub fn ensure_owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(metadata) => {
                if metadata.permissions().mode() & 0o777 != 0o600 {
                    let mut perms = metadata.permissions();
                    perms.set_mode(0o600);
                    std::fs::set_permissions(path, perms)?;
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}
