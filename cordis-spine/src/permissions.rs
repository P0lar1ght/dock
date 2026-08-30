//! Live-looked-up permission gate. Tools call [`Permissions::request`]; the TUI
//! resolves the front of the queue. Do not capture this Arc in a long-lived closure.

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use cordis::{Context, Inject, Plugin, plugin};
use tokio::sync::oneshot;

use crate::acp::PermissionOptionKind;
use crate::names::{PERMISSION_EVENT, PERMISSIONS, SETTINGS};
use crate::settings::{AppSettings, PermissionMode};

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
}

impl Permissions {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            queue: Mutex::new(VecDeque::new()),
            always: Mutex::new(HashSet::new()),
            never: Mutex::new(HashSet::new()),
        }
    }

    pub fn front(&self) -> Option<PermissionPrompt> {
        self.queue.lock().unwrap().front().map(|p| p.prompt.clone())
    }

    pub fn resolve(&self, kind: PermissionOptionKind) {
        let Some(pending) = self.queue.lock().unwrap().pop_front() else {
            return;
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
        let _ = pending.tx.send(kind);
        self.ctx.emit(PERMISSION_EVENT, ());
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
