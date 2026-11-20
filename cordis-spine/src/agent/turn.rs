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
    /// 这一轮上游给的粘性路由令牌（见 `WirePolicy::turn_state_header`）：
    /// `(谁给的, 值)`。「谁给的」是调用方拼的键（provider、账号、模型），换了就不回传。
    /// 只活一轮：[`Self::reset`] 清掉。
    routing: Mutex<Option<(String, String)>>,
}

impl TurnControl {
    pub fn new() -> Self {
        Self {
            token: Mutex::new(CancellationToken::new()),
            yield_requested: AtomicBool::new(false),
            routing: Mutex::new(None),
        }
    }

    /// New token for the next prompt. Previous holders keep the old token.
    pub fn reset(&self) {
        *self.token.lock().unwrap() = CancellationToken::new();
        self.clear_yield();
        *self.routing.lock().unwrap() = None;
    }

    /// 这一轮里 `key` 那家上游给过的路由令牌。
    pub fn routing_token(&self, key: &str) -> Option<String> {
        match &*self.routing.lock().unwrap() {
            Some((k, value)) if k == key => Some(value.clone()),
            _ => None,
        }
    }

    /// 记下上游给的路由令牌。一轮里**只认第一次给的**（之后的请求原样回传它，
    /// 不跟着换）；换了 provider / 账号 / 模型就换成新的。
    pub fn remember_routing_token(&self, key: &str, value: &str) {
        let mut slot = self.routing.lock().unwrap();
        if slot.as_ref().is_some_and(|(k, _)| k == key) {
            return;
        }
        *slot = Some((key.to_string(), value.to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 一轮里只认第一次给的令牌；换了上游就换；新一轮清空——跨轮回传会把下一轮
    /// 钉到上一轮的路由上。
    #[test]
    fn routing_token_lives_for_one_turn_and_one_upstream() {
        let turn = TurnControl::new();
        assert_eq!(turn.routing_token("a"), None);

        turn.remember_routing_token("a", "t1");
        turn.remember_routing_token("a", "t2");
        assert_eq!(turn.routing_token("a").as_deref(), Some("t1"), "只认第一次");
        assert_eq!(turn.routing_token("b"), None, "别家的不回传");

        turn.remember_routing_token("b", "u1");
        assert_eq!(turn.routing_token("b").as_deref(), Some("u1"));
        assert_eq!(turn.routing_token("a"), None, "换了上游，旧的作废");

        turn.reset();
        assert_eq!(turn.routing_token("b"), None, "新一轮从空开始");
    }
}
