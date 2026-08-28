use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use cordis::{plugin, Context, Inject, Plugin};

use crate::names::{SESSION_EVENT, SESSIONS};
use crate::types::LogEvent;

#[derive(Clone, Copy, Debug, Default)]
pub struct TokenUsage {
    pub prompt: u64,
    pub completion: u64,
    pub window: u64,
    pub official: bool,
}

impl TokenUsage {
    pub fn context_used(self) -> u64 {
        self.prompt.saturating_add(self.completion)
    }
}

/// A previous conversation, kept so `/resume` can restore it.
#[derive(Clone, Debug)]
pub struct ArchivedSession {
    pub id: String,
    pub title: String,
    pub events: Vec<LogEvent>,
    pub times: Vec<SystemTime>,
}

/// In-memory session log. DSH `ctx.sessions`; Grok conversation persistence.
#[derive(Clone)]
pub struct Sessions {
    ctx: Context,
    events: Arc<Mutex<Vec<LogEvent>>>,
    times: Arc<Mutex<Vec<SystemTime>>>,
    archive: Arc<Mutex<Vec<ArchivedSession>>>,
    next_id: Arc<Mutex<u64>>,
    usage: Arc<Mutex<TokenUsage>>,
    turn_started: Arc<Mutex<Option<Instant>>>,
    pending_images: Arc<Mutex<Vec<crate::types::UserImage>>>,
    user_images: Arc<Mutex<Vec<Vec<crate::types::UserImage>>>>,
    /// Child isolates skip `session/event` so the parent TUI is not flooded.
    emit: bool,
}

impl Sessions {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            events: Arc::new(Mutex::new(Vec::new())),
            times: Arc::new(Mutex::new(Vec::new())),
            archive: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(Mutex::new(1)),
            usage: Arc::new(Mutex::new(TokenUsage {
                window: 128_000,
                ..TokenUsage::default()
            })),
            turn_started: Arc::new(Mutex::new(None)),
            pending_images: Arc::new(Mutex::new(Vec::new())),
            user_images: Arc::new(Mutex::new(Vec::new())),
            emit: true,
        }
    }

    /// Nested subagent log: same API, no TUI events.
    pub fn isolated(ctx: Context) -> Self {
        let mut s = Self::new(ctx);
        s.emit = false;
        s
    }

    pub fn seed(&self, events: Vec<LogEvent>) {
        let n = events.len();
        *self.events.lock().unwrap() = events;
        *self.times.lock().unwrap() = vec![SystemTime::now(); n];
    }

    pub fn usage(&self) -> TokenUsage {
        *self.usage.lock().unwrap()
    }

    pub fn turn_elapsed(&self) -> Option<std::time::Duration> {
        self.turn_started.lock().unwrap().map(|t| t.elapsed())
    }

    pub fn set_window(&self, window: u64) {
        if window > 0 {
            self.usage.lock().unwrap().window = window;
        }
    }

    pub fn append(&self, event: LogEvent) {
        if matches!(event, LogEvent::User(_)) {
            let imgs = std::mem::take(&mut *self.pending_images.lock().unwrap());
            self.user_images.lock().unwrap().push(imgs);
        }
        self.events.lock().unwrap().push(event.clone());
        self.times.lock().unwrap().push(SystemTime::now());
        self.emit_session(event);
    }

    fn emit_session(&self, event: LogEvent) {
        if self.emit {
            self.ctx.emit(SESSION_EVENT, event);
        }
    }

    pub fn queue_user_images(&self, images: Vec<crate::types::UserImage>) {
        *self.pending_images.lock().unwrap() = images;
    }

    pub fn user_images(&self) -> Vec<Vec<crate::types::UserImage>> {
        self.user_images.lock().unwrap().clone()
    }

    pub fn events(&self) -> Vec<LogEvent> {
        self.events.lock().unwrap().clone()
    }

    pub fn times(&self) -> Vec<SystemTime> {
        self.times.lock().unwrap().clone()
    }

    /// Empty `llm/stream` slot the HTTP sampler fills via [`Self::apply_llm_delta`].
    pub fn begin_llm(&self) {
        {
            let mut usage = self.usage.lock().unwrap();
            usage.completion = 0;
            usage.official = false;
        }
        *self.turn_started.lock().unwrap() = Some(Instant::now());
        self.append(LogEvent::LlmStream(crate::types::LlmOutput::default()));
    }

    pub fn apply_llm_delta(&self, delta: &crate::stream_acc::StreamDelta) {
        match delta {
            crate::stream_acc::StreamDelta::Text(text) => {
                if text.is_empty() {
                    return;
                }
                let mut events = self.events.lock().unwrap();
                let Some(LogEvent::LlmStream(out)) = events.last_mut() else {
                    return;
                };
                out.text.push_str(text);
                {
                    let mut usage = self.usage.lock().unwrap();
                    if !usage.official {
                        usage.completion = Self::estimate_tokens(&out.text);
                    }
                }
                let event = events.last().cloned().unwrap();
                drop(events);
                self.emit_session(event);
            }
            crate::stream_acc::StreamDelta::Reasoning(text) => {
                if text.is_empty() {
                    return;
                }
                let mut events = self.events.lock().unwrap();
                let Some(LogEvent::LlmStream(out)) = events.last_mut() else {
                    return;
                };
                out.reasoning.push_str(text);
                let event = events.last().cloned().unwrap();
                drop(events);
                self.emit_session(event);
            }
            crate::stream_acc::StreamDelta::Usage {
                prompt,
                completion,
            } => {
                let mut usage = self.usage.lock().unwrap();
                if *prompt > 0 {
                    usage.prompt = *prompt;
                }
                if *completion > 0 {
                    usage.completion = *completion;
                    usage.official = true;
                }
                drop(usage);
                self.emit_session(LogEvent::LlmStream(crate::types::LlmOutput::default()));
            }
        }
    }

    pub fn finish_llm(&self, output: &crate::types::LlmOutput) {
        let mut events = self.events.lock().unwrap();
        let Some(LogEvent::LlmStream(out)) = events.last_mut() else {
            return;
        };
        if out.text.is_empty() {
            out.text = output.text.clone();
        }
        if out.reasoning.is_empty() {
            out.reasoning = output.reasoning.clone();
        }
        if out.reasoning_ms.is_none() && !out.reasoning.is_empty() {
            out.reasoning_ms = self
                .turn_started
                .lock()
                .unwrap()
                .map(|t| t.elapsed().as_millis() as u64);
        }
        out.tool_calls = output.tool_calls.clone();
        let event = events.last().cloned().unwrap();
        drop(events);
        self.emit_session(event);
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        self.events().iter().map(LogEvent::kind).collect()
    }

    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
        self.times.lock().unwrap().clear();
        *self.turn_started.lock().unwrap() = None;
        self.pending_images.lock().unwrap().clear();
        self.user_images.lock().unwrap().clear();
    }

    fn estimate_tokens(text: &str) -> u64 {
        let mut ascii = 0u64;
        let mut other = 0u64;
        for c in text.chars() {
            if c.is_ascii() {
                ascii += 1;
            } else {
                other += 1;
            }
        }
        other + ascii.saturating_add(3) / 4
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
        let times = self.times();
        let item = ArchivedSession {
            id,
            title,
            events,
            times,
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
        let user_n = item
            .events
            .iter()
            .filter(|e| matches!(e, LogEvent::User(_)))
            .count();
        *self.times.lock().unwrap() = item.times;
        *self.events.lock().unwrap() = item.events;
        *self.user_images.lock().unwrap() = vec![Vec::new(); user_n];
        self.pending_images.lock().unwrap().clear();
        if let Some(event) = last {
            self.emit_session(event);
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
        assert_eq!(sessions.times().len(), 1);
    }

    #[tokio::test]
    async fn empty_log_does_not_archive() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        assert!(sessions.archive_current().is_none());
        assert!(sessions.archived().is_empty());
    }

    #[tokio::test]
    async fn llm_deltas_patch_the_same_event() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.begin_llm();
        sessions.apply_llm_delta(&crate::stream_acc::StreamDelta::Text("Hel".into()));
        sessions.apply_llm_delta(&crate::stream_acc::StreamDelta::Text("lo".into()));
        sessions.finish_llm(&crate::types::LlmOutput {
            text: "Hello".into(),
            ..crate::types::LlmOutput::default()
        });
        assert_eq!(sessions.events().len(), 1);
        match &sessions.events()[0] {
            LogEvent::LlmStream(out) => assert_eq!(out.text, "Hello"),
            other => panic!("{other:?}"),
        }
    }
}
