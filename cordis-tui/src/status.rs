//! Status line values. The widget is the copied Grok `StatusBar`.

use cordis::Context;
use cordis_spine::{Sessions, SESSIONS};

use crate::names::SESSION_PORT;
use crate::session::SessionRef;

pub struct StatusLine {
    ctx: Context,
}

impl StatusLine {
    pub fn new(ctx: Context) -> Self {
        Self { ctx }
    }

    pub fn left(&self) -> String {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| ".".into())
    }

    pub fn center(&self) -> Option<String> {
        let sessions = self.ctx.require::<Sessions>(SESSIONS).ok()?;
        let turns = sessions
            .events()
            .iter()
            .filter(|e| matches!(e, cordis_spine::LogEvent::User(_)))
            .count();
        Some(format!("turn {turns}"))
    }

    pub fn right(&self) -> Option<String> {
        let working = self
            .ctx
            .get::<SessionRef>(SESSION_PORT)
            .is_some_and(|h| h.working());
        Some(if working {
            "working".into()
        } else {
            "idle".into()
        })
    }
}
