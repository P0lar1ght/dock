//! 智能授权的第一层：固定规则，不调模型。
//!
//! - **硬清单**（[`Rule::Hard`]）：一定问人，判断模型推翻不了。sudo、rm -rf、碰 `~/.ssh` /
//!   `.env`、写项目目录外、git push --force、把文件传出去、桌面操作；另外建条件命令 /
//!   监听项目外文件的定时任务、加载插件、改 `.git` / `.dock` 里的东西——这几样等于
//!   扩大自己的权限，也在这一档。
//! - **直接放行**（[`Rule::Allow`]）：只读命令（和计划模式同一套判定）、项目里的文件
//!   编辑（编辑前 Dock 存了底，审查面板撤得回来）。
//! - 其余 [`Rule::Unsure`]：交给判断模型。

use std::path::{Component, Path, PathBuf};

use cordis_base::acp::{is_cua_driver_mcp, BROWSER_FILE_UPLOAD_MCP};
use cordis_base::read_only_shell::is_read_only_command;
use cordis_base::risky_shell::{risky_reason, sensitive_path};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    /// 直接放行；理由只给日志 / 测试看（规则放行的不记到工具卡上）。
    Allow(&'static str),
    /// 一定问人，理由写进审批框。
    Hard(String),
    /// 交给判断模型。
    Unsure,
}

/// `tool` 是工具名，`key` 是权限键（`scheduler_create:watch` 这种按参数分出来的）。
pub fn classify(tool: &str, key: &str, arguments: &str, cwd: &Path) -> Rule {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let text = |name: &str| args.get(name).and_then(Value::as_str).unwrap_or_default();
    match (tool, key) {
        (_, "scheduler_create:watch") => Rule::Hard("建定时跑的条件命令".into()),
        (_, "scheduler_create:files") => Rule::Hard("监听项目外的文件".into()),
        ("cordis_plugin", _) => Rule::Hard("加载插件".into()),
        (BROWSER_FILE_UPLOAD_MCP, _) => Rule::Hard("把文件传出去".into()),
        (t, _) if is_cua_driver_mcp(t) => Rule::Hard("桌面操作".into()),
        ("bash" | "run_terminal_cmd" | "monitor", _) => shell(text("command")),
        ("write_file", _) => edit(text("target_file"), cwd),
        ("search_replace", _) => edit(text("file_path"), cwd),
        _ => Rule::Unsure,
    }
}

fn shell(command: &str) -> Rule {
    if command.trim().is_empty() {
        return Rule::Unsure;
    }
    if let Some(why) = risky_reason(command) {
        return Rule::Hard(why.into());
    }
    if is_read_only_command(command) {
        return Rule::Allow("只读命令");
    }
    Rule::Unsure
}

fn edit(path: &str, cwd: &Path) -> Rule {
    let path = path.trim();
    if path.is_empty() {
        return Rule::Unsure;
    }
    if let Some(why) = sensitive_path(path) {
        return Rule::Hard(why.into());
    }
    let Some(rel) = inside(path, cwd) else {
        return Rule::Hard("写项目目录外".into());
    };
    match rel.components().next() {
        Some(Component::Normal(top)) if top == ".git" => Rule::Hard("改 .git 里的文件".into()),
        Some(Component::Normal(top)) if top == ".dock" => {
            Rule::Hard("改 .dock 里的配置 / 插件".into())
        }
        _ => Rule::Allow("项目里的改动"),
    }
}

/// `path`（相对 `cwd` 或绝对、`~/` 开头算家目录）落在 `cwd` 里就回相对路径。
///
/// 按真实路径比：已经存在的那一截 canonicalize（解开符号链接、按文件系统的意思走 `..`），
/// 还不存在的尾巴原样接上。所以项目里一个指向外面的符号链接（`link/x`、`link/../x`）
/// 不能拿来绕，会话目录本身是符号链接（macOS 的 `/tmp`、`/var`）也不会把项目里的
/// 绝对路径误判成外面。还不存在的那截里有 `..`，说不清落在哪，按外面算。
fn inside(path: &str, cwd: &Path) -> Option<PathBuf> {
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
        None if path == "~" => PathBuf::from(std::env::var_os("HOME")?),
        None => cwd.join(path),
    };
    let cwd_real = cwd.canonicalize().ok()?;
    let real = real_path(&expanded)?;
    real.strip_prefix(&cwd_real).ok().map(Path::to_path_buf)
}

/// 存在的那一截 canonicalize，不存在的尾巴原样接上；尾巴里有 `..` 回 `None`。
fn real_path(path: &Path) -> Option<PathBuf> {
    let mut probe = path.to_path_buf();
    let mut tail = Vec::new();
    loop {
        if let Ok(mut real) = probe.canonicalize() {
            for part in tail.iter().rev() {
                real.push(part);
            }
            return Some(real);
        }
        match probe.components().next_back()? {
            Component::Normal(name) => tail.push(name.to_os_string()),
            Component::CurDir => {}
            _ => return None,
        }
        if !probe.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(cmd: &str) -> String {
        serde_json::json!({ "command": cmd }).to_string()
    }

    /// 只读的放行、硬清单上的问人、别的交给模型；硬清单先于只读（`cat ~/.ssh/id_rsa`
    /// 是只读命令，也得问）。
    #[test]
    fn shell_commands() {
        let cwd = Path::new("/tmp");
        assert_eq!(
            classify("bash", "bash", &bash("git log --oneline -5"), cwd),
            Rule::Allow("只读命令")
        );
        assert_eq!(
            classify("bash", "bash", &bash("cat ~/.ssh/id_rsa"), cwd),
            Rule::Hard("碰 ~/.ssh".into())
        );
        assert_eq!(
            classify("bash", "bash", &bash("sudo make install"), cwd),
            Rule::Hard("sudo".into())
        );
        assert_eq!(
            classify("monitor", "monitor", &bash("rm -rf ~/x"), cwd),
            Rule::Hard("rm -rf".into())
        );
        assert_eq!(
            classify("bash", "bash", &bash("cargo test"), cwd),
            Rule::Unsure
        );
    }

    /// 项目里的编辑放行；项目外、密钥、`.git` / `.dock` 里的问人；符号链接指到外面的也算外面。
    #[test]
    fn edits() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("p");
        let outside = dir.path().join("o");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, project.join("link")).unwrap();
        let write = |path: &str| {
            classify(
                "write_file",
                "write_file",
                &serde_json::json!({ "target_file": path, "contents": "x" }).to_string(),
                &project,
            )
        };
        assert_eq!(write("src/new.rs"), Rule::Allow("项目里的改动"));
        assert_eq!(write("./src/../README.md"), Rule::Allow("项目里的改动"));
        assert_eq!(
            write(&project.join("src/a.rs").display().to_string()),
            Rule::Allow("项目里的改动")
        );
        assert_eq!(write("../o/x.txt"), Rule::Hard("写项目目录外".into()));
        assert_eq!(write("/etc/hosts"), Rule::Hard("写项目目录外".into()));
        assert_eq!(write(".env"), Rule::Hard("碰 .env".into()));
        assert_eq!(
            write(".git/hooks/pre-commit"),
            Rule::Hard("改 .git 里的文件".into())
        );
        assert_eq!(
            write(".dock/config.toml"),
            Rule::Hard("改 .dock 里的配置 / 插件".into())
        );
        #[cfg(unix)]
        {
            assert_eq!(write("link/evil.sh"), Rule::Hard("写项目目录外".into()));
            // `link/..` 按文件系统走到的是链接目标的上级，不是项目根。
            assert_eq!(write("link/../evil.sh"), Rule::Hard("写项目目录外".into()));
        }
        // 会话目录本身经过符号链接（macOS 的临时目录在 /var → /private/var 下）：
        // 用真实路径写项目里的文件照样算项目里。
        let real = project.canonicalize().unwrap().join("src/real.rs");
        assert_eq!(
            write(&real.display().to_string()),
            Rule::Allow("项目里的改动")
        );
        // 还不存在的那截里有 `..`：说不清落在哪，按外面算。
        assert_eq!(
            write("new/../../o/x.txt"),
            Rule::Hard("写项目目录外".into())
        );
        let replace = classify(
            "search_replace",
            "search_replace",
            &serde_json::json!({ "file_path": "src/a.rs" }).to_string(),
            &project,
        );
        assert_eq!(replace, Rule::Allow("项目里的改动"));
    }

    /// 扩大自己权限的、桌面操作、往外传文件的：一定问人。
    #[test]
    fn the_rest_of_the_hard_list() {
        let cwd = Path::new("/tmp");
        for (tool, key) in [
            ("scheduler_create", "scheduler_create:watch"),
            ("scheduler_create", "scheduler_create:files"),
            ("cordis_plugin", "cordis_plugin:x"),
            ("mcp_cua-driver__click", "mcp_cua-driver__click"),
            (BROWSER_FILE_UPLOAD_MCP, BROWSER_FILE_UPLOAD_MCP),
        ] {
            assert!(
                matches!(classify(tool, key, "{}", cwd), Rule::Hard(_)),
                "{tool}"
            );
        }
        assert_eq!(
            classify("scheduler_create", "scheduler_create", "{}", cwd),
            Rule::Unsure
        );
        assert_eq!(
            classify(
                "mcp_browser__browser_js",
                "mcp_browser__browser_js",
                "{}",
                cwd
            ),
            Rule::Unsure
        );
    }
}
