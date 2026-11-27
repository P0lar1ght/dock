//! 沙箱的工作区：会话在哪里干活。
//!
//! 会话属于一个工作区，cwd 只是工作区里的「当前目录」，像 shell 一样随时可以换，不决定
//! 会话归属。工作区有两种：
//!
//! - **原地**（默认）：就是选定的项目目录，和以前一样。
//! - **工作树**：从项目所在的 git 仓库 `git worktree` 出一份独立副本（新分支、从 `HEAD`
//!   检出），改动不碰原项目，几个任务并行也不会互相踩。做完由人用 git 合回去。
//!
//! 两种都按 [`Space::home`]（选定的项目目录）归档会话，所以历史列表、按项目分组都不变。
//!
//! 本模块不认识会话和配置：放哪、叫什么都由调用方传进来。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// `$DOCK_HOME` 下放工作树的目录：`<它>/<id>/<仓库名>/`。限制模式下 `$DOCK_HOME` 整个护住，
/// 只有工作区自己落在这里时放开它（见 [`crate::Policy::for_space`]）。
pub const STORE_DIR: &str = "worktrees";

/// 工作区怎么来的。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Mode {
    /// 原地：就是选定的项目目录。
    InPlace,
    /// 工作树：项目仓库的一份 `git worktree`。
    Worktree {
        /// 工作树检出的分支（`dock/<id>`）。
        branch: String,
        /// 原仓库的 git 目录（`--git-common-dir`）：在工作树里提交要写它的 objects / refs。
        git_dir: PathBuf,
    },
}

/// 一个会话的工作区。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    /// 在这里干活：命令的默认目录、限制模式下的可写根。
    pub root: PathBuf,
    /// 选定的项目目录：会话按它归档。原地模式下和 `root` 相同。
    pub home: PathBuf,
    #[serde(flatten)]
    pub mode: Mode,
}

impl Space {
    /// 原地工作区：就在 `dir` 里。
    pub fn in_place(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            root: dir.clone(),
            home: dir,
            mode: Mode::InPlace,
        }
    }

    /// 就是选定目录本身（原地、没换过根）：落盘时不用另记。
    pub fn is_in_place(&self) -> bool {
        self.mode == Mode::InPlace && self.root == self.home
    }

    /// `path` 在不在工作区里。两边都按调用方给的写法比，调用方负责先规范化。
    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }

    /// 原项目里的路径换到工作区里的同一位置（工作树模式）；不在原项目里的原样返回。
    pub fn translate(&self, path: &Path) -> PathBuf {
        match path.strip_prefix(&self.home) {
            Ok(rel) if self.root != self.home => self.root.join(rel),
            _ => path.to_path_buf(),
        }
    }

    /// 反过来：工作区里的路径换回原项目里的同一位置。
    pub fn untranslate(&self, path: &Path) -> PathBuf {
        match path.strip_prefix(&self.root) {
            Ok(rel) if self.root != self.home => self.home.join(rel),
            _ => path.to_path_buf(),
        }
    }

    /// 工作树的分支；原地是 `None`。
    pub fn branch(&self) -> Option<&str> {
        match &self.mode {
            Mode::Worktree { branch, .. } => Some(branch),
            Mode::InPlace => None,
        }
    }

    /// 工作树要额外可写的原仓库 git 目录；原地是 `None`。
    pub fn git_dir(&self) -> Option<&Path> {
        match &self.mode {
            Mode::Worktree { git_dir, .. } => Some(git_dir),
            Mode::InPlace => None,
        }
    }

    /// 给人看的一句：「原地」或「工作树（分支 …）」。
    pub fn describe_mode(&self) -> String {
        match &self.mode {
            Mode::InPlace => "原地".into(),
            Mode::Worktree { branch, .. } => format!("工作树（分支 {branch}）"),
        }
    }
}

/// 从 `home` 所在的 git 仓库建一棵工作树：放在 `store/<id>/<仓库名>/`，新分支 `dock/<id>`，
/// 从 `HEAD` 检出。`home` 是仓库里的子目录时，工作区落在工作树里的同一个子目录。
///
/// 没提交的改动不带过去（工作树从 `HEAD` 检出）。
pub fn create_worktree(home: &Path, store: &Path, id: &str) -> Result<Space, String> {
    let home = home
        .canonicalize()
        .map_err(|e| format!("找不到 {}：{e}", home.display()))?;
    let top = git(&home, &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("{} 不在 git 仓库里，建不了工作树（{e}）", home.display()))?;
    let top = PathBuf::from(top)
        .canonicalize()
        .map_err(|e| format!("找不到仓库根：{e}"))?;
    git(&home, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .map_err(|_| "仓库还没有提交，建不了工作树".to_string())?;
    let common = PathBuf::from(git(&home, &["rev-parse", "--git-common-dir"])?);
    let git_dir = if common.is_absolute() {
        common
    } else {
        home.join(common)
    };
    let git_dir = git_dir.canonicalize().unwrap_or(git_dir);
    let rel = home
        .strip_prefix(&top)
        .unwrap_or(Path::new(""))
        .to_path_buf();
    let name = top
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "repo".into());
    let tree = store.join(id).join(name);
    if tree.exists() {
        return Err(format!("{} 已经存在", tree.display()));
    }
    if let Some(parent) = tree.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建 {}：{e}", parent.display()))?;
    }
    let branch = format!("dock/{id}");
    let out = Command::new("git")
        .arg("-C")
        .arg(&top)
        .args(["worktree", "add", "--quiet", "-b", &branch])
        .arg(&tree)
        .arg("HEAD")
        .output()
        .map_err(|e| format!("跑不了 git：{e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git worktree add 失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let tree = tree.canonicalize().unwrap_or(tree);
    let root = tree.join(rel);
    // `home` 是没进版本库的子目录时，工作树里没有它。
    std::fs::create_dir_all(&root).map_err(|e| format!("建 {}：{e}", root.display()))?;
    Ok(Space {
        root,
        home,
        mode: Mode::Worktree { branch, git_dir },
    })
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("跑不了 git：{e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap().join("proj");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        std::fs::write(repo.join("sub/a.txt"), "a").unwrap();
        run(&repo, &["init", "--quiet"]);
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "--quiet", "-m", "init"]);
        (dir, repo)
    }

    #[test]
    fn in_place_round_trips_through_json_and_maps_paths_to_itself() {
        let space = Space::in_place("/work/proj");
        assert!(space.is_in_place());
        let json = serde_json::to_value(&space).unwrap();
        assert_eq!(json["mode"], "in_place");
        assert_eq!(serde_json::from_value::<Space>(json).unwrap(), space);
        let p = Path::new("/work/proj/src/a.rs");
        assert_eq!(space.translate(p), p);
        assert!(space.contains(p));
        assert!(!space.contains(Path::new("/work/other")));
    }

    #[test]
    fn a_worktree_checks_out_head_on_its_own_branch_and_keeps_the_subdir() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let (dir, repo) = repo();
        // 没提交的改动不带过去。
        std::fs::write(repo.join("sub/a.txt"), "dirty").unwrap();
        let store = dir.path().join("trees");
        let space = create_worktree(&repo.join("sub"), &store, "t1").unwrap();

        assert_eq!(space.home, repo.join("sub"));
        assert_eq!(space.branch(), Some("dock/t1"));
        assert!(
            space.root.ends_with("t1/proj/sub"),
            "{}",
            space.root.display()
        );
        assert_eq!(
            std::fs::read_to_string(space.root.join("a.txt")).unwrap(),
            "a"
        );
        assert_eq!(space.git_dir(), Some(repo.join(".git").as_path()));
        // 原项目里的位置换到工作树里的同一位置，再换回来。
        let inner = space.translate(&repo.join("sub/deep"));
        assert_eq!(inner, space.root.join("deep"));
        assert_eq!(space.untranslate(&inner), repo.join("sub/deep"));

        let json = serde_json::to_value(&space).unwrap();
        assert_eq!(json["mode"], "worktree");
        assert_eq!(serde_json::from_value::<Space>(json).unwrap(), space);

        // 同一个 id 不覆盖。
        assert!(create_worktree(&repo, &store, "t1").is_err());
    }

    #[test]
    fn outside_a_repo_there_is_no_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let err = create_worktree(dir.path(), &dir.path().join("trees"), "x").unwrap_err();
        assert!(err.contains("不在 git 仓库里"), "{err}");
    }
}
