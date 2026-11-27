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
//!
//! cwd 只是沙箱工作区（[`Space`]）里的「当前目录」：会话属于工作区，归档、限制模式的可写根、
//! 预设的项目层都按工作区走（[`session_space`] / [`session_home`]），`/cd` 在工作区里移动
//! 不改这些；`/cd` 到工作区外才算换了一个项目（[`enter_workspace`]）。

use std::path::{Path, PathBuf};

use cordis::Context;
use dock_sandbox::space::Space;

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

/// 正在跑的 agent 的沙箱工作区；不在任何一轮里时是进程 cwd 上的原地工作区。
pub fn current_space() -> Space {
    crate::tools::registry::exec_ctx()
        .map(|ctx| session_space(&ctx))
        .unwrap_or_else(|| Space::in_place(process_cwd()))
}

fn process_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// `ctx` 所属会话的沙箱工作区；没有会话就是进程 cwd 上的原地工作区。
pub fn session_space(ctx: &Context) -> Space {
    ctx.get::<Sessions>(SESSIONS)
        .map(|s| s.space())
        .unwrap_or_else(|| Space::in_place(process_cwd()))
}

/// `ctx` 所属会话归档在哪个项目名下（`$DOCK_HOME/sessions/<它>/`）：落了盘是归档目录，
/// 没落盘是工作区的项目目录。按会话找历史、开页、定时任务回到会话都用它，不用 cwd。
pub fn session_home(ctx: &Context) -> PathBuf {
    match ctx.get::<Sessions>(SESSIONS) {
        Some(s) => s.disk_home().unwrap_or_else(|| s.space().home),
        None => process_cwd(),
    }
}

/// `/cd` 的结果：这一页新的工作目录，以及要告诉用户的提示（可能为空）。
#[derive(Debug)]
pub struct ChangedDir {
    pub cwd: PathBuf,
    pub note: Option<String>,
    /// 出了原来的工作区、换到了这个项目目录上（[`enter_workspace`]）；在工作区里移动是 `None`。
    pub workspace: Option<PathBuf>,
}

/// 只改 `ctx` 这一页的工作目录：相对路径按这页当前的 cwd 展开，钉进
/// [`Sessions`]。**不动进程 cwd**——别的页、全局服务（MCP、浏览器）都不受影响。
///
/// 目标在这一页的沙箱工作区里：只是换当前目录，会话归属、预设的项目层、限制模式的可写根
/// 都不变。目标在工作区外：这一页换到目标上的原地工作区（[`enter_workspace`]）；还没聊过
/// 的页连归档一起换过去，聊过的那一份仍记在原来的项目下。
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
    let space = sessions.space();
    let root = space
        .root
        .canonicalize()
        .unwrap_or_else(|_| space.root.clone());
    let note = startup_only_note(&cwd);
    if cwd.starts_with(&root) {
        sessions.pin_space(space);
        sessions.pin_workspace_cwd(&cwd);
        return Ok(ChangedDir {
            cwd,
            note,
            workspace: None,
        });
    }
    enter_workspace(ctx, &cwd)?;
    let talked = crate::session::persist::has_conversation(&sessions.events());
    if sessions.disk_home().is_some() && !talked {
        sessions.attach_disk();
    }
    Ok(ChangedDir {
        workspace: Some(cwd.clone()),
        cwd,
        note,
    })
}

/// 把 `ctx` 这一页放到 `dir` 上的原地工作区（选定一个项目目录）：工作区、cwd、预设的项目层
/// 都指过去。不动归档：要按新项目落盘的调用方接着 [`Sessions::attach_disk`]。
pub fn enter_workspace(ctx: &Context, dir: &Path) -> Result<PathBuf, String> {
    let sessions = ctx
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| "这一页没有会话，无法切换目录".to_string())?;
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("无法进入 {}：{e}", dir.display()))?;
    if !dir.is_dir() {
        return Err(format!("{} 不是目录", dir.display()));
    }
    sessions.pin_space(Space::in_place(&dir));
    sessions.pin_workspace_cwd(&dir);
    if let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) {
        presets.set_workspace_root(&dir);
    }
    Ok(dir)
}

/// 把 `ctx` 这一页换进工作区 `space`：cwd 换到工作区里的同一个位置（不在里面就是工作区根），
/// 预设的项目层跟着项目目录（工作树还是原项目）。归档不动：会话按 `space.home` 归档，
/// 工作树和原地是同一个项目。
pub fn switch_space(ctx: &Context, space: Space) -> Result<PathBuf, String> {
    let sessions = ctx
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| "这一页没有会话".to_string())?;
    let old = sessions.space();
    // 工作树的根是规范化过的（`create_worktree`）：当前目录也规范化了再对位置，
    // 不然 `/var` 和 `/private/var` 这种写法对不上，cwd 就退回工作区根了。
    let here = session_cwd(ctx);
    let here = old.untranslate(&here.canonicalize().unwrap_or(here));
    let cwd = space.translate(&here);
    let cwd = if cwd.is_dir() && space.contains(&cwd) {
        cwd
    } else {
        space.root.clone()
    };
    let moved_home = old.home != space.home;
    sessions.pin_space(space.clone());
    sessions.pin_workspace_cwd(&cwd);
    if moved_home {
        if let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) {
            presets.set_workspace_root(&space.home);
        }
    }
    Ok(cwd)
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
        // 这里几次读进程 cwd 要读到同一个：拿住进程环境锁，别的用例
        // （`scoped().cwd(..)`）这时切不了目录。以前不拿，并发跑时偶发
        // 「左边是 crate 目录、右边是别人的临时目录」。
        let _env = cordis_base::test_env::scoped();
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

    /// 回归：工作区里 `/cd` 只换当前目录。会话还归档在项目下，工作区（限制模式的可写根、
    /// `AGENTS.md`）也还是整个项目——以前 `/cd sub` 之后新开的页、会话面板、定时任务都按
    /// sub 算，同一个项目的历史被切成两半。
    #[tokio::test]
    async fn cd_inside_the_workspace_keeps_the_project() {
        let _env = cordis_base::test_env::scoped().home();
        let proj = tempfile::tempdir().unwrap();
        let root = canon(proj.path());
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "proj rules").unwrap();
        let ctx = page(Some(&root));
        ctx.get::<Sessions>(SESSIONS).unwrap().attach_disk();

        let changed = change_dir(&ctx, Path::new("sub")).unwrap();
        assert!(changed.workspace.is_none(), "没出工作区");
        assert_eq!(session_cwd(&ctx), root.join("sub"));
        assert_eq!(session_space(&ctx), Space::in_place(&root));
        assert_eq!(session_home(&ctx), root);
        let paths = with_exec_ctx_async(ctx.clone(), async {
            crate::prompt::project_instructions::instruction_paths()
        })
        .await;
        assert!(
            paths.iter().any(|(_, p)| p == &root.join("AGENTS.md")),
            "项目的 AGENTS.md 还在：{paths:?}"
        );
    }

    /// `/cd` 出了工作区 = 换项目：还没聊过的页连归档一起换过去；聊过的那一份不搬家，
    /// 还记在原来的项目下。
    #[tokio::test]
    async fn cd_out_of_the_workspace_enters_the_target_project() {
        let _env = cordis_base::test_env::scoped().home();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (a, b) = (canon(a.path()), canon(b.path()));
        let ctx = page(Some(&a));
        let sessions = ctx.get::<Sessions>(SESSIONS).unwrap();
        sessions.attach_disk();

        let changed = change_dir(&ctx, &b).unwrap();
        assert_eq!(changed.workspace.as_deref(), Some(b.as_path()));
        assert_eq!(session_space(&ctx), Space::in_place(&b));
        assert_eq!(session_home(&ctx), b, "还没聊过：归档跟着换过去");

        sessions.append(cordis_base::types::LogEvent::User("hi".into()));
        change_dir(&ctx, &a).unwrap();
        assert_eq!(session_space(&ctx), Space::in_place(&a));
        assert_eq!(session_home(&ctx), b, "聊过的会话不搬家");
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
