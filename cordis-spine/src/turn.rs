//! Cooperative turn cancel. Grok uses `CancellationToken` on the sampler
//! request; dock live-looks this up from `"turn"` so HTTP / bash can abort
//! without capturing the token in a long-lived TUI closure.

use std::sync::Mutex;

use cordis::{Inject, Plugin, plugin};
use tokio_util::sync::CancellationToken;

use crate::names::TURN;

pub struct TurnControl {
    token: Mutex<CancellationToken>,
}

impl TurnControl {
    pub fn new() -> Self {
        Self {
            token: Mutex::new(CancellationToken::new()),
        }
    }

    /// New token for the next prompt. Previous holders keep the old token.
    pub fn reset(&self) {
        *self.token.lock().unwrap() = CancellationToken::new();
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

pub fn turn() -> Plugin {
    plugin("turn", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(TURN, TurnControl::new())?))
    })
}
