use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use cordis::{plugin, Disposable, Inject, Plugin};
use cordis_spine::{AGENT_LOOP, SESSIONS, TURN};
use cordis_tui::{QueuedItem, SessionPort, SessionRef, SESSION_PORT};
use tokio::sync::mpsc;

use crate::names::SESSION;

use super::actor::run_session;
use super::commands::SessionCommand;
use super::handle::SessionHandle;

pub fn session_actor() -> Plugin {
    plugin(
        "session",
        Inject::from([AGENT_LOOP, SESSIONS, TURN]),
        |ctx, _: &()| {
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
            let current_prompt_id = Arc::new(Mutex::new(None));
            let queued = Arc::new(AtomicUsize::new(0));
            let queued_prompts = Arc::new(Mutex::new(Vec::<QueuedItem>::new()));
            let handle = SessionHandle {
                cmd_tx: cmd_tx.clone(),
                current_prompt_id: current_prompt_id.clone(),
                queued: queued.clone(),
                queued_prompts: queued_prompts.clone(),
            };
            tokio::spawn(run_session(
                ctx.clone(),
                cmd_rx,
                current_prompt_id,
                queued,
                queued_prompts,
            ));
            ctx.effect("session-actor-task", move |scope| {
                scope.own(Disposable::from_fn(move || {
                    let _ = cmd_tx.send(SessionCommand::Shutdown);
                }));
                Ok(())
            })?;
            let port = SessionRef::new(Arc::new(handle.clone()) as Arc<dyn SessionPort>);
            ctx.provide(SESSION_PORT, port)?;
            Ok(Some(ctx.provide(SESSION, handle)?))
        },
    )
}
