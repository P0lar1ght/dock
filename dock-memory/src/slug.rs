//! Workspace directory slug: prefer `origin` org/repo + short hash, else cwd path hash.

use std::path::Path;
use std::process::Command;

/// `{slug}-{hash8}` where slug comes from the repo name (or directory name) and
/// `hash8` is the first 8 hex chars of blake3 over the identity string.
pub fn workspace_slug(cwd: &Path) -> String {
    let identity = extract_repo_identity(cwd);

    let (slug, hash_input) = match identity {
        Some(ref repo_id) => {
            let slug_source = repo_id.rsplit('/').next().unwrap_or(repo_id);
            (slugify(slug_source, 40), repo_id.clone())
        }
        None => {
            let canonical = cwd
                .canonicalize()
                .unwrap_or_else(|_| cwd.to_path_buf());
            let dir_name = canonical
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("workspace");
            (
                slugify(dir_name, 40),
                canonical.to_string_lossy().into_owned(),
            )
        }
    };

    let slug = if slug.is_empty() { "workspace" } else { &slug };
    let hash = blake3::hash(hash_input.as_bytes());
    let hex = hash.to_hex();
    let hash8 = hex.get(..8).unwrap_or(&*hex);
    format!("{slug}-{hash8}")
}

fn extract_repo_identity(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout);
    normalize_remote_url(url.trim())
}

fn normalize_remote_url(url: &str) -> Option<String> {
    let path = if let Some(colon_pos) = url.find(':') {
        if url
            .get(..colon_pos)
            .is_some_and(|h| h.contains('@') && !h.contains('/'))
        {
            url.get(colon_pos + 1..)?
        } else {
            url.split("//")
                .nth(1)
                .and_then(|after_scheme| after_scheme.split_once('/'))
                .map(|(_, path)| path)?
        }
    } else {
        return None;
    };

    let cleaned = path
        .trim_end_matches(".git")
        .trim_end_matches('/')
        .trim_start_matches('/');

    if cleaned.is_empty() || !cleaned.contains('/') {
        return None;
    }
    Some(cleaned.to_string())
}

pub fn slugify(input: &str, max_len: usize) -> String {
    let slug: String = input
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    let mut result = String::with_capacity(slug.len());
    let mut prev_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash {
                result.push('-');
            }
            prev_dash = true;
        } else {
            result.push(c);
            prev_dash = false;
        }
    }

    let truncated: String = result.chars().take(max_len).collect();
    truncated.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_deterministic_for_path() {
        let a = workspace_slug(Path::new("/tmp/dock-memory-slug-a"));
        let b = workspace_slug(Path::new("/tmp/dock-memory-slug-a"));
        assert_eq!(a, b);
        assert!(a.contains('-'));
        assert!(a.len() > 9);
    }

    #[test]
    fn different_paths_differ() {
        let a = workspace_slug(Path::new("/tmp/dock-memory-slug-x"));
        let b = workspace_slug(Path::new("/tmp/dock-memory-slug-y"));
        assert_ne!(a, b);
    }

    #[test]
    fn normalize_ssh_and_https() {
        assert_eq!(
            normalize_remote_url("git@github.com:P0lar1ght/dock.git").as_deref(),
            Some("P0lar1ght/dock")
        );
        assert_eq!(
            normalize_remote_url("https://github.com/P0lar1ght/dock.git").as_deref(),
            Some("P0lar1ght/dock")
        );
    }

    #[test]
    fn git_repo_uses_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output();
        let _ = Command::new("git")
            .args(["remote", "add", "origin", "git@github.com:acme/demo.git"])
            .current_dir(tmp.path())
            .output();
        let slug = workspace_slug(tmp.path());
        assert!(slug.starts_with("demo-"), "got {slug}");
        let sub = tmp.path().join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(workspace_slug(&sub), slug);
    }
}
