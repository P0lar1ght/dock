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

use std::path::PathBuf;

use cordis::Context;

use crate::names::SESSIONS;
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
