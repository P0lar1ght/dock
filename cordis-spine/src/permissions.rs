//! Live-looked-up permission gate. Tools call [`Permissions::request`]; the TUI
//! resolves the front of the queue. Do not capture this Arc in a long-lived closure.

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use cordis::{plugin, Context, Inject, Plugin};
use tokio::sync::oneshot;

use crate::names::{PERMISSIONS, PERMISSION_EVENT, SETTINGS};
use crate::settings::{AppSettings, PermissionMode};
use cordis_base::acp::PermissionOptionKind;

#[derive(Clone, Debug)]
pub struct PermissionPrompt {
    pub tool: String,
    pub summary: String,
}

struct Pending {
    prompt: PermissionPrompt,
    tx: oneshot::Sender<PermissionOptionKind>,
}

pub struct Permissions {
    ctx: Context,
    queue: Mutex<VecDeque<Pending>>,
    always: Mutex<HashSet<String>>,
    never: Mutex<HashSet<String>>,
    last_resolve: Mutex<Option<PermissionOptionKind>>,
}

impl Permissions {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            queue: Mutex::new(VecDeque::new()),
            always: Mutex::new(HashSet::new()),
            never: Mutex::new(HashSet::new()),
            last_resolve: Mutex::new(None),
        }
    }

    pub fn front(&self) -> Option<PermissionPrompt> {
        self.queue.lock().unwrap().front().map(|p| p.prompt.clone())
    }

    /// Last decision that actually popped the queue. Gateway projects this
    /// when `front()` is empty after a TUI (or web) resolve.
    pub fn last_resolve(&self) -> Option<PermissionOptionKind> {
        *self.last_resolve.lock().unwrap()
    }

    /// Resolve the front of the queue. Returns `false` when empty so a second
    /// resolver (TUI vs web) can fail closed instead of auto-allowing.
    pub fn resolve(&self, kind: PermissionOptionKind) -> bool {
        let Some(pending) = self.queue.lock().unwrap().pop_front() else {
            return false;
        };
        match kind {
            PermissionOptionKind::AllowAlways => {
                self.always
                    .lock()
                    .unwrap()
                    .insert(pending.prompt.tool.clone());
            }
            PermissionOptionKind::RejectAlways => {
                self.never
                    .lock()
                    .unwrap()
                    .insert(pending.prompt.tool.clone());
            }
            _ => {}
        }
        *self.last_resolve.lock().unwrap() = Some(kind);
        let _ = pending.tx.send(kind);
        self.ctx.emit(PERMISSION_EVENT, ());
        true
    }

    pub async fn request(&self, tool: &str, summary: &str) -> bool {
        if let Some(settings) = self.ctx.get::<AppSettings>(SETTINGS) {
            if settings.permission_mode() == PermissionMode::Allow {
                return true;
            }
        }
        if self.always.lock().unwrap().contains(tool) {
            return true;
        }
        if self.never.lock().unwrap().contains(tool) {
            return false;
        }
        let (tx, rx) = oneshot::channel();
        self.queue.lock().unwrap().push_back(Pending {
            prompt: PermissionPrompt {
                tool: tool.to_string(),
                summary: summary.to_string(),
            },
            tx,
        });
        self.ctx.emit(PERMISSION_EVENT, ());
        rx.await.ok().is_some_and(PermissionOptionKind::is_allow)
    }
}

pub fn permissions() -> Plugin {
    plugin("permissions", Inject::new(), |ctx, _: &()| {
        Ok(Some(
            ctx.provide(PERMISSIONS, Permissions::new(ctx.clone()))?,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis::Context;

    #[test]
    fn resolve_returns_false_when_empty() {
        let ctx = Context::new();
        let perms = Permissions::new(ctx);
        assert!(!perms.resolve(PermissionOptionKind::AllowOnce));
        assert!(perms.last_resolve().is_none());
    }
}
