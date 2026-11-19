//! 「整棵树减去几个洞」展开成一组只放行的路径。
//!
//! Landlock 只有「放行」：放行 `/` 就放行了 `~/.ssh`。要读得到别处、读不到密钥，只能沿着
//! 每个洞的祖先往下走，把洞的兄弟一个个放行（Grok 在 Linux 上也是这样展开的）。祖先目录
//! 本身不放行（放行它就放行了下面的洞），由调用方另给一条只能列目录的规则。
//!
//! 只在 Linux 后端用；逻辑与平台无关，所以在哪都编、都测。

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::fs;
use std::path::{Path, PathBuf};

use crate::paths::within;

/// 放行 `root` 下除 `holes` 以外的一切，需要哪些规则。`holes` 用真实路径。
pub(crate) fn tree_minus(root: &Path, holes: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    expand(root, holes, 0, &mut out);
    out
}

fn expand(dir: &Path, holes: &[PathBuf], depth: usize, out: &mut Vec<PathBuf>) {
    if holes.iter().any(|h| within(dir, h)) {
        return;
    }
    if !holes.iter().any(|h| within(h, dir)) {
        out.push(dir.to_path_buf());
        return;
    }
    if depth > 64 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            // 规则按链接指向的东西生效：指进洞里、或者指向洞的祖先（会把洞一起放行），都不放。
            let Ok(target) = fs::canonicalize(&child) else {
                continue;
            };
            if holes
                .iter()
                .any(|h| within(&target, h) || within(h, &target))
            {
                continue;
            }
            out.push(child);
        } else if kind.is_dir() {
            expand(&child, holes, depth + 1, out);
        } else if !holes.iter().any(|h| within(&child, h)) {
            out.push(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_holes_is_one_rule() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        assert_eq!(tree_minus(&root, &[]), vec![root]);
    }

    #[test]
    fn a_hole_expands_its_ancestors_into_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let home = root.join("home");
        fs::create_dir_all(home.join(".ssh")).unwrap();
        fs::create_dir_all(home.join("code/proj")).unwrap();
        fs::write(home.join(".bashrc"), "").unwrap();
        fs::create_dir_all(root.join("usr/bin")).unwrap();

        let mut got = tree_minus(&root, &[home.join(".ssh")]);
        got.sort();
        let mut want = vec![home.join(".bashrc"), home.join("code"), root.join("usr")];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn the_root_itself_in_a_hole_grants_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        assert!(tree_minus(&root.join("a"), std::slice::from_ref(&root)).is_empty());
    }

    #[test]
    fn symlinks_into_or_above_a_hole_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let home = root.join("home");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::create_dir_all(root.join("data")).unwrap();
        std::os::unix::fs::symlink(&ssh, home.join("keys")).unwrap();
        std::os::unix::fs::symlink(&home, home.join("me")).unwrap();
        std::os::unix::fs::symlink(root.join("data"), home.join("data")).unwrap();

        let got = tree_minus(&home, &[ssh]);
        assert_eq!(got, vec![home.join("data")]);
    }
}
