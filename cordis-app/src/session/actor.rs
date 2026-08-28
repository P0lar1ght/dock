//! Session actor run loop. TUI never holds the loop; it sends `SessionCommand`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cordis::Context;
use cordis_spine::{LoopHandle, TurnOutcome, AGENT_LOOP};
use tokio::sync::mpsc;

use super::commands::{PromptTurnResult, SessionCommand};

struct Pending {
    prompt_id: String,
    text: String,
    respond_to: tokio::sync::oneshot::Sender<PromptTurnResult>,
}

pub(super) async fn run_session(
    ctx: Context,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    current_prompt_id: Arc<Mutex<Option<String>>>,
) {
    let mut queue: VecDeque<Pending> = VecDeque::new();
    let mut current: Option<Pending> = None;

    loop {
        if current.is_none() {
            current = queue.pop_front();
        }
        if current.is_none() {
            match cmd_rx.recv().await {
                Some(SessionCommand::Shutdown) | None => return,
                Some(SessionCommand::Prompt {
                    prompt_id,
                    text,
                    send_now: _,
                    respond_to,
                }) => {
                    current = Some(Pending {
                        prompt_id,
                        text,
                        respond_to,
                    });
                }
            }
        }
        let Some(pending) = current.take() else {
            continue;
        };
        *current_prompt_id.lock().unwrap() = Some(pending.prompt_id.clone());
        let turn = run_turn(&ctx, pending.text.clone());
        tokio::pin!(turn);
        let result = loop {
            tokio::select! {
                outcome = &mut turn => break outcome,
                cmd = cmd_rx.recv() => match cmd {
                    Some(SessionCommand::Shutdown) | None => {
                        let _ = pending.respond_to.send(Err("shutdown".into()));
                        *current_prompt_id.lock().unwrap() = None;
                        return;
                    }
                    Some(SessionCommand::Prompt {
                        prompt_id,
                        text,
                        send_now,
                        respond_to,
                    }) => {
                        let next = Pending { prompt_id, text, respond_to };
                        if send_now {
                            queue.push_front(next);
                        } else {
                            queue.push_back(next);
                        }
                    }
                }
            }
        };
        *current_prompt_id.lock().unwrap() = None;
        let _ = pending.respond_to.send(result);
    }
}

async fn run_turn(ctx: &Context, text: String) -> PromptTurnResult {
    let handle = ctx
        .require::<LoopHandle>(AGENT_LOOP)
        .map_err(|e| e.to_string())?;
    match handle.run(text).await {
        Ok(TurnOutcome::Text(reply)) => Ok(reply),
        Err(err) => Err(err.to_string()),
    }
}
