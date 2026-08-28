use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};

use crate::names::{SESSION_EVENT, SESSIONS};
use crate::types::LogEvent;

/// A previous conversation, kept so `/resume` can restore it.
#[derive(Clone, Debug)]
pub struct ArchivedSession {
    pub id: String,
    pub title: String,
    pub events: Vec<LogEvent>,
}

/// In-memory session log. DSH `ctx.sessions`; Grok conversation persistence.
#[derive(Clone)]
pub struct Sessions {
    ctx: Context,
    events: Arc<Mutex<Vec<LogEvent>>>,
    archive: Arc<Mutex<Vec<ArchivedSession>>>,
    next_id: Arc<Mutex<u64>>,
}

impl Sessions {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            events: Arc::new(Mutex::new(Vec::new())),
            archive: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    pub fn append(&self, event: LogEvent) {
        self.events.lock().unwrap().push(event.clone());
        self.ctx.emit(SESSION_EVENT, event);
    }

    pub fn events(&self) -> Vec<LogEvent> {
        self.events.lock().unwrap().clone()
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        self.events().iter().map(LogEvent::kind).collect()
    }

    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }

    /// Snapshot the live log into the resume list. No-op when empty.
    pub fn archive_current(&self) -> Option<ArchivedSession> {
        let events = self.events();
        if events.is_empty() {
            return None;
        }
        let mut next = self.next_id.lock().unwrap();
        let id = format!("s{next}");
        *next += 1;
        let title = events
            .iter()
            .find_map(|e| match e {
                LogEvent::User(text) => {
                    let t = text.trim();
                    (!t.is_empty()).then(|| t.chars().take(40).collect())
                }
                _ => None,
            })
            .unwrap_or_else(|| id.clone());
        let item = ArchivedSession {
            id,
            title,
            events,
        };
        self.archive.lock().unwrap().insert(0, item.clone());
        Some(item)
    }

    pub fn archived(&self) -> Vec<ArchivedSession> {
        self.archive.lock().unwrap().clone()
    }

    /// Replace the live log (Grok `/resume`). Emits the last event so the TUI redraws.
    pub fn restore(&self, id: &str) -> bool {
        let Some(item) = self
            .archive
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .cloned()
        else {
            return false;
        };
        let last = item.events.last().cloned();
        *self.events.lock().unwrap() = item.events;
        if let Some(event) = last {
            self.ctx.emit(SESSION_EVENT, event);
        }
        true
    }
}

pub fn sessions() -> Plugin {
    plugin("sessions", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(SESSIONS, Sessions::new(ctx.clone()))?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LogEvent;

    #[tokio::test]
    async fn archive_then_restore() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("hello world".into()));
        let item = sessions.archive_current().expect("non-empty log archives");
        assert_eq!(item.title, "hello world");
        sessions.clear();
        assert!(sessions.events().is_empty());
        assert!(sessions.restore(&item.id));
        assert_eq!(sessions.events().len(), 1);
    }

    #[tokio::test]
    async fn empty_log_does_not_archive() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        assert!(sessions.archive_current().is_none());
        assert!(sessions.archived().is_empty());
    }
}
