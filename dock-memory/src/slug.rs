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
            let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
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
    let path = resolve_remote_path(url)?;

    let cleaned = path
        .trim_end_matches(".git")
        .trim_end_matches('/')
        .trim_start_matches('/');

    if cleaned.is_empty() || !cleaned.contains('/') {
        return None;
    }
    Some(cleaned.to_string())
}

/// Pull the repository path out of a remote URL, accepting both SCP-like
/// (`git@host:owner/repo`) and URL (`https://host/owner/repo`) forms.
fn resolve_remote_path(url: &str) -> Option<&str> {
    let colon_pos = url.find(':')?;
    if url
        .get(..colon_pos)
        .is_some_and(|h| h.contains('@') && !h.contains('/'))
    {
        url.get(colon_pos + 1..)
    } else {
        url.split("//")
            .nth(1)
            .and_then(|after_scheme| after_scheme.split_once('/'))
            .map(|(_, path)| path)
    }
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

/// 笔记文件名（不含 `.md`）：各种文字的字母数字都留着，其余连成一个 `-`，英文小写。
///
/// [`slugify`] 只留 ASCII，纯中文标题会变成空串；这里是给标题直接当文件名用的，
/// `[[链接]]` 里写的也是它。全是符号的标题仍然回空串，由调用方另起名。
pub fn note_stem(input: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_dash = false;
    for c in input.trim().chars() {
        if c.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(c.to_lowercase());
        } else {
            pending_dash = true;
        }
    }
    let truncated: String = out.chars().take(max_chars).collect();
    truncated.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_stem_keeps_non_ascii_letters() {
        assert_eq!(
            note_stem("审计日志改造（降噪 + 补全）", 48),
            "审计日志改造-降噪-补全"
        );
        assert_eq!(
            note_stem("  ChatGPT 订阅通道 / SIWC  ", 48),
            "chatgpt-订阅通道-siwc"
        );
        // 纯 ASCII 的标题和 slugify 一致。
        assert_eq!(
            note_stem("Auth: JWT & refresh", 48),
            slugify("Auth: JWT & refresh", 48)
        );
        // 按字符截断，不会切在多字节字符中间；截出来的尾巴不留 `-`。
        assert_eq!(note_stem("记忆 图谱 可视化", 3), "记忆");
        assert_eq!(note_stem("！？…", 48), "");
        assert_eq!(note_stem("../../etc/passwd", 48), "etc-passwd");
    }

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
