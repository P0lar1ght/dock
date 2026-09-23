//! Mailbox tools for the independent `subagent` grain.

use serde::Deserialize;

use crate::tools::registry::tool_result;
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

use super::store::InterruptOutcome;
use super::Subagents;

pub(super) const SEND_TOOL_NAME: &str = "send_message";

pub(super) fn send_spec() -> ToolSpec {
    ToolSpec {
        name: SEND_TOOL_NAME.into(),
        description: "Send a message to an adjacent agent: one of your direct subagents (agent_id from task / list_agents), or, when you are a subagent, the agent that started you (its id is in your first task). Siblings and yourself are rejected. A running target reads it at its next step; an idle subagent starts its next turn. Does not wait for a reply and does not end your turn."
            .into(),
        parameters_json: r#"{"type":"object","properties":{"agent_id":{"type":"string"},"message":{"type":"string"}},"required":["agent_id","message"]}"#.into(),
    }
}

pub(super) fn list_spec() -> ToolSpec {
    ToolSpec {
        name: "list_agents".into(),
        description: "List your subagents that can still be continued (running or idle), with queued message counts. Disposed ones are omitted. This is the status source of truth after send_message.".into(),
        parameters_json: r#"{"type":"object","properties":{}}"#.into(),
    }
}

pub(super) fn interrupt_spec() -> ToolSpec {
    ToolSpec {
        name: "interrupt_agent".into(),
        description: "Stop your subagent's current turn and park it idle. Queued messages are kept and will run next. Already-idle is a no-op — it does not start a turn; use send_message to wake an idle subagent.".into(),
        parameters_json: r#"{"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}"#.into(),
    }
}

#[derive(Debug, Deserialize)]
struct SendInput {
    /// 旧名 `subagent_id` 仍然收：`/resume` 回来的历史里全是它。
    #[serde(alias = "subagent_id")]
    agent_id: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct InterruptInput {
    /// 与 `send_message` 同名；旧名 `subagent_id` 仍然收。
    #[serde(alias = "subagent_id")]
    agent_id: String,
}

pub(super) async fn run_send(sub: &Subagents, sender: String, call: ToolCall) -> ToolResult {
    let input: SendInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: invalid send_message arguments: {e}")),
    };
    if input.message.trim().is_empty() {
        return tool_result(call, "Error: message is required");
    }
    match sub.send_between(&sender, input.agent_id.trim(), &input.message) {
        Ok(msg) => tool_result(call, msg),
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
}

pub(super) async fn run_list(sub: &Subagents, sender: String, call: ToolCall) -> ToolResult {
    tool_result(call, sub.list_agents_of(&sender))
}

pub(super) async fn run_interrupt(sub: &Subagents, sender: String, call: ToolCall) -> ToolResult {
    let input: InterruptInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => {
            return tool_result(
                call,
                format!("Error: invalid interrupt_agent arguments: {e}"),
            );
        }
    };
    let id = input.agent_id.trim();
    if let Err(e) = sub.ensure_child_of(&sender, id) {
        return tool_result(call, format!("Error: {e}"));
    }
    tool_result(call, sub.interrupt_agent(id))
}

impl Subagents {
    /// 用户（TUI 框底输入）发给子代理：不走相邻授权，`urgent` 打断本轮插话。
    pub fn send_message(&self, id: &str, message: &str, urgent: bool) -> Result<String, String> {
        let was_running = if urgent {
            self.store.push_urgent(id, message.to_string())?
        } else {
            self.store.enqueue_queued(id, message.to_string())?
        };
        Ok(send_ack(id, urgent, was_running))
    }

    /// 模型的 `send_message`：只走相邻的一条边。
    ///
    /// - 父 → 直接子：进孩子的队列。在跑就在下一步读到，idle 就开下一轮。
    /// - 子 → 启动它的会话：进父信箱（workflow 的孩子进 run 自己的队列）。
    ///
    /// 兄弟、隔代、自己一律拒绝。发送方身份由调用方的会话给出，模型填不了。
    pub fn send_between(
        &self,
        sender: &str,
        target: &str,
        message: &str,
    ) -> Result<String, String> {
        if target.is_empty() {
            return Err("agent_id 不能为空".into());
        }
        if target == sender {
            return Err("不能给自己发消息".into());
        }
        if self.store.get(target).is_some() {
            self.ensure_child_of(sender, target)?;
            let was_running = self.store.enqueue_queued(target, message.to_string())?;
            return Ok(if was_running {
                format!("message delivered to running agent {target}; it reads it at its next step")
            } else {
                format!("message delivered to idle agent {target}; its next turn is starting now")
            });
        }
        match self.store.get(sender) {
            Some(me) if me.parent == target => {
                self.deliver_report(sender, message)?;
                Ok(format!("message delivered to {target}"))
            }
            Some(me) => Err(format!(
                "{target} 不是启动你的代理；子代理只能发给 {}",
                me.parent
            )),
            None => Err(format!("未知的代理 {target}")),
        }
    }

    /// `target` 必须是 `sender` 的直接子代理。
    pub(super) fn ensure_child_of(&self, sender: &str, target: &str) -> Result<(), String> {
        match self.store.get(target) {
            None => Err(format!("未知的代理 {target}")),
            Some(slot) if slot.parent != sender => Err(format!(
                "{target} 不是你的直接子代理；只能发给自己启动的子代理或启动你的代理"
            )),
            Some(_) => Ok(()),
        }
    }

    /// `parent` 名下还能续的孩子。
    pub fn list_agents_of(&self, parent: &str) -> String {
        let rows: Vec<String> = self
            .store
            .children_of(parent)
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
            return Err("direct parent is not live; message was not delivered".into());
        };
        self.store.push_report(from_id, output);
        Ok("message delivered to the agent that started you".into())
    }

    /// 在跑的子代理 `id` 在步边界取走父级排队的消息，已包好 `<system-reminder>`。
    pub fn drain_child_inbox(&self, id: &str) -> Vec<String> {
        let Some(parent) = self.store.get(id).map(|s| s.parent.clone()) else {
            return Vec::new();
        };
        self.store
            .take_queued_for_step(id)
            .iter()
            .map(|m| super::format::wrap_reminder(&format!("Agent {parent} sent a message:\n{m}")))
            .collect()
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
        (true, false) => {
            format!("urgent message delivered to idle subagent {id}; the next turn is starting now")
        }
        (false, true) => format!(
            "queued message accepted for running subagent {id}; it reads it at its next step"
        ),
        (false, false) => {
            format!("queued message delivered to idle subagent {id}; the next turn is starting now")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同族工具用同一个参数名：`send_message` 与 `interrupt_agent` 都叫
    /// `agent_id`，否则模型照着前一颗的写法调后一颗就是一次参数错误。
    /// 旧名 `subagent_id` 继续收，`/resume` 回来的历史里已有的调用不至于失效。
    #[test]
    fn mailbox_tools_take_one_id_name() {
        for spec in [send_spec(), interrupt_spec()] {
            let params: serde_json::Value = serde_json::from_str(&spec.parameters_json).unwrap();
            assert!(
                params["required"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("agent_id")),
                "{}: {params}",
                spec.name
            );
            assert!(
                params["properties"].get("subagent_id").is_none(),
                "{params}"
            );
        }

        let new: InterruptInput = serde_json::from_str(r#"{"agent_id":"kid-1"}"#).unwrap();
        assert_eq!(new.agent_id, "kid-1");
        let old: InterruptInput = serde_json::from_str(r#"{"subagent_id":"kid-2"}"#).unwrap();
        assert_eq!(old.agent_id, "kid-2");
        let old: SendInput =
            serde_json::from_str(r#"{"subagent_id":"kid-3","message":"hi","priority":"urgent"}"#)
                .unwrap();
        assert_eq!(old.agent_id, "kid-3");
    }
}
