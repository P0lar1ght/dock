//! Mailbox tools for the independent `subagent` grain.

use serde::Deserialize;

use crate::session::Sessions;
use crate::tools::tool_result;
use crate::types::{ToolCall, ToolResult, ToolSpec};

use super::store::InterruptOutcome;
use super::Subagents;

pub(super) fn send_spec() -> ToolSpec {
    ToolSpec {
        name: "send_message".into(),
        description: "Send a follow-up to a continuable subagent started with the subagent tool. Does not return the child's reply; use list_agents to confirm status.\n\
- subagent_id: id from subagent / list_agents.\n\
- message: text for the child.\n\
- priority: queued (default) waits if the child is running; urgent is send-now and steers a running child (not interrupt-then-send). If the child is idle, both queued and urgent start the next turn immediately. The tool result says idle→starting now vs running→queued vs running→steer."
            .into(),
        parameters_json: r#"{"type":"object","properties":{"subagent_id":{"type":"string"},"message":{"type":"string"},"priority":{"type":"string","enum":["queued","urgent"],"description":"queued waits if running; urgent steers a running child now. Idle children start the next turn for either."}},"required":["subagent_id","message"]}"#.into(),
    }
}

pub(super) fn list_spec() -> ToolSpec {
    ToolSpec {
        name: "list_agents".into(),
        description: "List continuable subagents started from this session (running or idle). Shows pending queued/urgent inbox counts. One-shot/disposed ids are omitted. This is the status source of truth after send_message.".into(),
        parameters_json: r#"{"type":"object","properties":{}}"#.into(),
    }
}

pub(super) fn interrupt_spec() -> ToolSpec {
    ToolSpec {
        name: "interrupt_agent".into(),
        description: "Stop the subagent's current turn and park it idle. Queued messages are kept and will run next. Already-idle is a no-op — it does not start a turn. Use send_message to wake an idle child. Not a prerequisite for urgent send_message.".into(),
        parameters_json: r#"{"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}"#.into(),
    }
}

pub(super) fn report_spec() -> ToolSpec {
    ToolSpec {
        name: "report".into(),
        description: "Send a message to the agent that started you. This is the only channel the parent can see — assistant text, inbox files, and todos are not delivered. Call it for progress, findings, failures, empty results, or whenever the parent must act. You may call it many times in one turn and across turns. Does not end your turn.".into(),
        parameters_json: r#"{"type":"object","properties":{"output":{"type":"string","description":"Self-contained findings for the parent."}},"required":["output"]}"#.into(),
    }
}

#[derive(Debug, Deserialize)]
struct SendInput {
    subagent_id: String,
    message: String,
    #[serde(default)]
    priority: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InterruptInput {
    agent_id: String,
}

#[derive(Debug, Deserialize)]
struct ReportInput {
    output: String,
}

pub(super) async fn run_send(sub: &Subagents, call: ToolCall) -> ToolResult {
    let input: SendInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: invalid send_message arguments: {e}")),
    };
    if input.message.trim().is_empty() {
        return tool_result(call, "Error: message is required");
    }
    let urgent = input
        .priority
        .as_deref()
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("urgent"));
    match sub.send_message(&input.subagent_id, &input.message, urgent) {
        Ok(msg) => tool_result(call, msg),
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
}

pub(super) async fn run_list(sub: &Subagents, call: ToolCall) -> ToolResult {
    tool_result(call, sub.list_agents())
}

pub(super) async fn run_interrupt(sub: &Subagents, call: ToolCall) -> ToolResult {
    let input: InterruptInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => {
            return tool_result(
                call,
                format!("Error: invalid interrupt_agent arguments: {e}"),
            )
        }
    };
    tool_result(call, sub.interrupt_agent(&input.agent_id))
}

pub(super) async fn run_report(
    sub: &Subagents,
    sessions: Option<std::sync::Arc<Sessions>>,
    call: ToolCall,
) -> ToolResult {
    let input: ReportInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: invalid report arguments: {e}")),
    };
    if input.output.trim().is_empty() {
        return tool_result(call, "Error: output is required");
    }
    let Some(sessions) = sessions else {
        return tool_result(call, "Error: sessions is not mounted");
    };
    let from = sessions.identity();
    if from == "main" {
        return tool_result(
            call,
            "Error: report is for subagents; the parent uses list_agents / send_message",
        );
    }
    match sub.deliver_report(from, &input.output) {
        Ok(msg) => tool_result(call, msg),
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
}

impl Subagents {
    pub fn send_message(&self, id: &str, message: &str, urgent: bool) -> Result<String, String> {
        let was_running = if urgent {
            self.store.push_urgent(id, message.to_string())?
        } else {
            self.store.enqueue_queued(id, message.to_string())?
        };
        Ok(send_ack(id, urgent, was_running))
    }

    pub fn list_agents(&self) -> String {
        let rows: Vec<String> = self
            .store
            .list()
            .into_iter()
            .filter(|s| !s.done)
            .map(|s| {
                let status = if s.idle { "idle" } else { "running" };
                let (queued, urgent) = self.store.inbox_pending(&s.id);
                let mut inbox = String::new();
                if queued > 0 {
                    inbox.push_str(&format!(" queued={queued}"));
                }
                if urgent {
                    inbox.push_str(" urgent");
                }
                let label = if s.description.trim().is_empty() {
                    s.subagent_type.clone()
                } else {
                    format!("{} — {}", s.subagent_type, s.description)
                };
                format!("{} [{status}]{inbox} — {label}", s.id)
            })
            .collect();
        if rows.is_empty() {
            "(no subagents)".into()
        } else {
            rows.join("\n")
        }
    }

    pub fn interrupt_agent(&self, id: &str) -> String {
        match self.store.request_interrupt(id) {
            InterruptOutcome::Requested => format!("interrupt requested for agent {id}"),
            InterruptOutcome::Settled => format!(
                "agent {id} is already idle; interrupt does not start a turn — use send_message"
            ),
            InterruptOutcome::Missing => format!("unknown agent {id}"),
        }
    }

    pub fn deliver_report(&self, from_id: &str, output: &str) -> Result<String, String> {
        if self.store.snapshot(from_id).is_none() {
            return Err("direct parent is not live; report was not delivered".into());
        };
        self.store.push_report(from_id, output);
        Ok(format!(
            "report accepted by the agent that started you as message {from_id}"
        ))
    }

    /// Grok-style next-sample reminders. Bodies already wrapped in `<system-reminder>`.
    pub fn drain_parent_notices(&self) -> Vec<String> {
        self.store
            .drain_notices()
            .iter()
            .map(|n| super::format::wrap_reminder(&super::format::format_parent_notice(n)))
            .collect()
    }

    pub fn consume_completion(&self, id: &str) {
        self.store.consume_completion(id);
    }
}

fn send_ack(id: &str, urgent: bool, was_running: bool) -> String {
    match (urgent, was_running) {
        (true, true) => format!(
            "urgent message delivered to running subagent {id}; it will steer on the current turn"
        ),
        (true, false) => format!(
            "urgent message delivered to idle subagent {id}; the next turn is starting now"
        ),
        (false, true) => format!(
            "queued message accepted for running subagent {id}; it will run after the current turn ends"
        ),
        (false, false) => format!(
            "queued message delivered to idle subagent {id}; the next turn is starting now"
        ),
    }
}
