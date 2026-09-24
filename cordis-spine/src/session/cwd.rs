//! 会话的工作目录。
//!
//! 一个进程里可以同时跑多个会话（分页、将来 GUI 的多项目），每个会话有自己的
//! cwd，所以工具、系统提示、子进程都不能再读 `std::env::current_dir()`——那是
//! 整个进程共用的。统一从这里取：
//!
//! - 手里有 ctx：[`session_cwd`]，读那一页（或子代理）的 [`Sessions`]。
//! - 手里没有 ctx（工具体深处、系统提示的辅助函数）：[`current_cwd`]，读正在跑的
//!   那个 agent 的 ctx（[`crate::tools::registry::exec_ctx`]，整轮和每次工具
//!   调用都挂着）。
//!
//! 会话没钉 cwd 时两者都退回进程 cwd，行为与改造前一致。

use std::path::{Path, PathBuf};

use cordis::Context;

use crate::agent::presets::AgentPresets;
use crate::names::{AGENT_PRESETS, SESSIONS};
use crate::session::log::Sessions;

/// `ctx` 所属会话的工作目录；没钉就是进程 cwd。
pub fn session_cwd(ctx: &Context) -> PathBuf {
    ctx.get::<Sessions>(SESSIONS)
        .and_then(|s| s.workspace_cwd())
        .unwrap_or_else(process_cwd)
}

/// 正在跑的 agent 的工作目录；不在任何一轮里（TUI 视图、测试）时是进程 cwd。
pub fn current_cwd() -> PathBuf {
    crate::tools::registry::exec_ctx()
        .map(|ctx| session_cwd(&ctx))
        .unwrap_or_else(process_cwd)
}

fn process_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// `/cd` 的结果：这一页新的工作目录，以及要告诉用户的提示（可能为空）。
#[derive(Debug)]
pub struct ChangedDir {
    pub cwd: PathBuf,
    pub note: Option<String>,
}

/// 只改 `ctx` 这一页的工作目录：相对路径按这页当前的 cwd 展开，钉进
/// [`Sessions`]，这页预设的项目层跟着指过去。**不动进程 cwd**——别的页、全局
/// 服务（MCP、浏览器）都不受影响。
///
/// 项目级 `.dock/config.toml` 与 skills 仍按启动目录（进程 cwd）加载；目标目录
/// 里有这两样时在 `note` 里说一声，免得用户以为它们跟着切过来了。
pub fn change_dir(ctx: &Context, path: &Path) -> Result<ChangedDir, String> {
    let sessions = ctx
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| "这一页没有会话，无法切换目录".to_string())?;
    let target = session_cwd(ctx).join(path);
    let cwd = target
        .canonicalize()
        .map_err(|e| format!("无法进入 {}：{e}", target.display()))?;
    if !cwd.is_dir() {
        return Err(format!("{} 不是目录", cwd.display()));
    }
    sessions.pin_workspace_cwd(&cwd);
    if let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) {
        presets.set_workspace_root(&cwd);
    }
    Ok(ChangedDir {
        note: startup_only_note(&cwd),
        cwd,
    })
}

/// 目标目录自带、但这一页不会加载的项目级资源。
fn startup_only_note(cwd: &Path) -> Option<String> {
    let startup = process_cwd()
        .canonicalize()
        .unwrap_or_else(|_| process_cwd());
    if cwd == startup {
        return None;
    }
    let mut unused = Vec::new();
    if cwd.join(".dock").join("config.toml").is_file() {
        unused.push(".dock/config.toml");
    }
    if cwd.join(".dock").join("skills").is_dir() || cwd.join(".agents").join("skills").is_dir() {
        unused.push("skills");
    }
    if unused.is_empty() {
        return None;
    }
    Some(format!(
        "{} 只在启动目录生效，这一页不会加载它们",
        unused.join(" 与 ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::runtime::{BoxFuture, Driver, LoopHandle};
    use crate::tools::registry::with_exec_ctx_async;
    use cordis_base::types::TurnOutcome;
    use std::sync::{Arc, Mutex};

    /// 一页：自己的 ctx + 自己的 `Sessions`，按需钉住 cwd。
    fn page(pinned: Option<&std::path::Path>) -> Context {
        let ctx = Context::new();
        let sessions = Sessions::tab(ctx.clone(), 2);
        if let Some(cwd) = pinned {
            sessions.pin_workspace_cwd(cwd);
        }
        // Drop 不会反注册，服务留在这份 ctx 上。
        let _reg = ctx.provide(SESSIONS, sessions).expect("provide sessions");
        ctx
    }

    fn canon(p: &std::path::Path) -> PathBuf {
        p.canonicalize().unwrap()
    }

    #[tokio::test]
    async fn unpinned_session_follows_the_process_cwd() {
        let ctx = page(None);
        assert_eq!(session_cwd(&ctx), process_cwd());
        assert_eq!(
            with_exec_ctx_async(ctx, async { current_cwd() }).await,
            process_cwd()
        );
        assert_eq!(current_cwd(), process_cwd(), "不在任何一轮里就是进程 cwd");
    }

    /// 回归：同一进程里两页钉在不同目录，相对路径和 bash 的默认目录都要各走
    /// 各的，而不是都落到进程 cwd 上。
    #[tokio::test]
    async fn two_pages_resolve_paths_and_bash_in_their_own_cwd() {
        let alpha = tempfile::tempdir().unwrap();
        let beta = tempfile::tempdir().unwrap();
        let a = page(Some(alpha.path()));
        let b = page(Some(beta.path()));

        let resolved = |ctx: Context| {
            with_exec_ctx_async(ctx, async { crate::tools::fs_common::resolve("x.txt") })
        };
        assert_eq!(resolved(a.clone()).await, alpha.path().join("x.txt"));
        assert_eq!(resolved(b.clone()).await, beta.path().join("x.txt"));

        let pwd = |ctx: Context| {
            with_exec_ctx_async(ctx, async {
                crate::tools::bash::run(r#"{"command":"pwd -P"}"#, &|| false, None).await
            })
        };
        let out_a = pwd(a).await;
        let out_b = pwd(b).await;
        assert!(
            out_a.contains(&canon(alpha.path()).display().to_string()),
            "bash 没在 alpha 里跑：{out_a}"
        );
        assert!(
            out_b.contains(&canon(beta.path()).display().to_string()),
            "bash 没在 beta 里跑：{out_b}"
        );
    }

    /// 回归：`/cd` 只改这一页。另一页、以及进程 cwd 都不能跟着变——以前
    /// `/cd` 是 `set_current_dir`，所有页一起被搬走。
    #[tokio::test]
    async fn change_dir_moves_only_this_page() {
        // 要断言进程 cwd 没动，就得拿住进程环境锁：别的用例（`scoped().cwd(..)`）
        // 会在并行时改它，不拿锁这条断言读到的是别人的目录。
        let _env = cordis_base::test_env::scoped();
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir(base.path().join("sub")).unwrap();
        let a = page(Some(base.path()));
        let b = page(Some(base.path()));
        let before = process_cwd();

        let changed = change_dir(&a, Path::new("sub")).unwrap();
        assert_eq!(changed.cwd, canon(&base.path().join("sub")));
        assert_eq!(session_cwd(&a), changed.cwd, "这页该钉到 sub");
        assert_eq!(session_cwd(&b), base.path(), "另一页不该动");
        assert_eq!(process_cwd(), before, "进程 cwd 不该动");

        let err = change_dir(&a, Path::new("no-such-dir")).unwrap_err();
        assert!(err.contains("无法进入"), "{err}");
        assert_eq!(session_cwd(&a), changed.cwd, "失败时保持原目录");
    }

    #[tokio::test]
    async fn change_dir_notes_project_config_that_stays_behind() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".dock/skills")).unwrap();
        std::fs::write(dir.path().join(".dock/config.toml"), "").unwrap();
        let changed = change_dir(&page(None), dir.path()).unwrap();
        let note = changed.note.expect("应提示项目级资源不生效");
        assert!(
            note.contains(".dock/config.toml") && note.contains("skills"),
            "{note}"
        );

        let plain = tempfile::tempdir().unwrap();
        assert!(change_dir(&page(None), plain.path())
            .unwrap()
            .note
            .is_none());
    }

    /// 记下 `handle_prompt` 里看到的 cwd——也就是系统提示、压缩这些轮内逻辑
    /// 看到的 cwd。
    struct SeeCwd(Arc<Mutex<Option<PathBuf>>>);

    impl Driver for SeeCwd {
        fn handle_prompt<'a>(
            &'a self,
            _ctx: &'a Context,
            _prompt: String,
        ) -> BoxFuture<'a, crate::error::Result<TurnOutcome>> {
            Box::pin(async move {
                *self.0.lock().unwrap() = Some(current_cwd());
                Ok(TurnOutcome::Text(String::new()))
            })
        }
    }

    /// 回归：一整轮都挂在这页的 exec ctx 下，轮内（不只是工具体里）取到的也是
    /// 这页钉住的 cwd。
    #[tokio::test]
    async fn a_turn_sees_its_page_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(None));
        let handle = LoopHandle::new(page(Some(dir.path())), Arc::new(SeeCwd(seen.clone())));
        handle.run("hi").await.unwrap();
        assert_eq!(seen.lock().unwrap().clone(), Some(dir.path().to_path_buf()));
    }
}
