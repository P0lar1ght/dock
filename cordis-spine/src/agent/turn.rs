//! Cooperative turn cancel. Grok uses `CancellationToken` on the sampler
//! request; dock live-looks this up from `"turn"` so HTTP / bash can abort
//! without capturing the token in a long-lived TUI closure.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use cordis::{plugin, Context, Inject, Plugin};
use tokio_util::sync::CancellationToken;

use crate::names::{SESSIONS, TURN};
use crate::session::log::Sessions;

pub struct TurnControl {
    token: Mutex<CancellationToken>,
    /// 软信号：有插话在等下一个步骤边界（见 `Sessions::push_steer`）。不取消
    /// 任何东西；能提前交还控制权的长工具（前台 bash 转后台）看它，循环取走插话
    /// 后清掉。
    yield_requested: AtomicBool,
}

impl TurnControl {
    pub fn new() -> Self {
        Self {
            token: Mutex::new(CancellationToken::new()),
            yield_requested: AtomicBool::new(false),
        }
    }

    /// New token for the next prompt. Previous holders keep the old token.
    pub fn reset(&self) {
        *self.token.lock().unwrap() = CancellationToken::new();
        self.clear_yield();
    }

    /// 请在下一个步骤边界交还控制权（插话到了）。和 [`Self::cancel`] 不同：正在
    /// 进行的采样和工具照常跑完。
    pub fn request_yield(&self) {
        self.yield_requested.store(true, Ordering::Relaxed);
    }

    pub fn yield_requested(&self) -> bool {
        self.yield_requested.load(Ordering::Relaxed)
    }

    pub fn clear_yield(&self) {
        self.yield_requested.store(false, Ordering::Relaxed);
    }

    pub fn cancel(&self) {
        self.token.lock().unwrap().cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.lock().unwrap().is_cancelled()
    }

    pub fn token(&self) -> CancellationToken {
        self.token.lock().unwrap().clone()
    }
}

impl Default for TurnControl {
    fn default() -> Self {
        Self::new()
    }
}

/// 插话：把 `text` 交给**正在跑**的这一轮，在下一个步骤边界送达（采样和工具
/// 都不打断），并请能提前交还控制权的长工具让路。返回插话 id；没有会话服务时
/// `None`。调用方自己保证这一页确实有一轮在跑——空闲时插话没人取，应当直接
/// 当一条新消息发。
pub fn steer(
    ctx: &Context,
    text: String,
    images: Vec<cordis_base::types::UserImage>,
) -> Option<String> {
    let sessions = ctx.get::<Sessions>(SESSIONS)?;
    let id = sessions.push_steer(text, images);
    if let Some(turn) = ctx.get::<TurnControl>(TURN) {
        turn.request_yield();
    }
    Some(id)
}

pub fn turn() -> Plugin {
    plugin("turn", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(TURN, TurnControl::new())?))
    })
}
