//! Number projected notifications for history/subscribe. Does not change spine events.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::sync::broadcast;

use cordis_spine::{
    Ask, ElicitPrompt, LogEvent, PermissionOptionKind, PermissionPrompt, PlanApprovalPrompt,
    PlanDecision, Sessions,
};

use crate::protocol::LIVE_THREAD_ID;

const BROADCAST_CAP: usize = 256;

#[derive(Clone, Debug)]
pub struct ProjectedEvent {
    pub seq: u64,
    pub method: String,
    pub thread_id: String,
    pub turn_id: String,
    pub timestamp: String,
    pub payload: Value,
}

impl ProjectedEvent {
    pub fn as_notification(&self) -> Value {
        let mut params = flatten_payload(&self.payload);
        params.insert("seq".into(), json!(self.seq));
        params.insert("transcriptSeq".into(), json!(self.seq));
        params.insert("threadId".into(), json!(self.thread_id));
        params.insert("turnId".into(), json!(self.turn_id));
        params.insert("timestamp".into(), json!(self.timestamp));
        json!({
            "method": self.method,
            "params": params
        })
    }

    pub fn as_history_item(&self) -> Value {
        let mut payload = flatten_payload(&self.payload);
        payload.insert("threadId".into(), json!(self.thread_id));
        payload.insert("turnId".into(), json!(self.turn_id));
        json!({
            "timestamp": self.timestamp,
            "threadId": self.thread_id,
            "turnId": self.turn_id,
            "seq": self.seq,
            "method": self.method,
            "payload": payload
        })
    }
}

fn flatten_payload(payload: &Value) -> serde_json::Map<String, Value> {
    match payload {
        Value::Object(map) => map.clone(),
        other => {
            let mut m = serde_json::Map::new();
            m.insert("value".into(), other.clone());
            m
        }
    }
}

struct Projector {
    seq: u64,
    turn_n: u64,
    turn_id: String,
    last_text: String,
    seen_tools: HashSet<String>,
    pending_tools: HashSet<String>,
    turn_open: bool,
    perm_id: Option<String>,
    ask_id: Option<String>,
    plan_id: Option<String>,
    elicit_id: Option<String>,
}

impl Default for Projector {
    fn default() -> Self {
        Self {
            seq: 0,
            turn_n: 0,
            turn_id: String::new(),
            last_text: String::new(),
            seen_tools: HashSet::new(),
            pending_tools: HashSet::new(),
            turn_open: false,
            perm_id: None,
            ask_id: None,
            plan_id: None,
            elicit_id: None,
        }
    }
}

pub struct Transcript {
    events: Vec<ProjectedEvent>,
    projector: Projector,
    tx: broadcast::Sender<ProjectedEvent>,
}

impl Transcript {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Self {
            events: Vec::new(),
            projector: Projector::default(),
            tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProjectedEvent> {
        self.tx.subscribe()
    }

    pub fn latest_seq(&self) -> u64 {
        self.projector.seq
    }

    pub fn history_since(&self, since_seq: u64) -> Vec<ProjectedEvent> {
        self.events
            .iter()
            .filter(|e| e.seq > since_seq)
            .cloned()
            .collect()
    }

    pub fn reset_from_sessions(&mut self, sessions: &Sessions) {
        self.events.clear();
        self.projector = Projector::default();
        for event in sessions.events() {
            self.ingest_log(event);
        }
    }

    pub fn ingest_log(&mut self, event: LogEvent) {
        match event {
            LogEvent::User(text) => {
                self.complete_turn_if_open();
                self.projector.turn_n += 1;
                self.projector.turn_id = format!("t{}", self.projector.turn_n);
                self.projector.last_text.clear();
                self.projector.seen_tools.clear();
                self.projector.pending_tools.clear();
                self.projector.turn_open = true;
                let turn_id = self.projector.turn_id.clone();
                self.push("turn/started", json!({ "status": "running" }));
                self.push(
                    "item/user_message",
                    json!({ "content": text, "turnId": turn_id }),
                );
            }
            LogEvent::LlmStream(out) => {
                if !self.projector.turn_open {
                    self.ensure_turn();
                }
                if out.text.len() > self.projector.last_text.len() {
                    let delta = out.text[self.projector.last_text.len()..].to_string();
                    self.projector.last_text = out.text.clone();
                    if !delta.is_empty() {
                        self.push("item/message_delta", json!({ "delta": delta }));
                    }
                }
                for call in &out.tool_calls {
                    if self.projector.seen_tools.insert(call.id.clone()) {
                        self.projector.pending_tools.insert(call.id.clone());
                        let arguments: Value =
                            serde_json::from_str(&call.arguments).unwrap_or(json!({}));
                        self.push(
                            "item/tool_started",
                            json!({
                                "toolCallId": call.id,
                                "itemId": call.id,
                                "toolName": call.name,
                                "title": call.name,
                                "arguments": arguments
                            }),
                        );
                    }
                }
                if out.tool_calls.is_empty()
                    && self.projector.pending_tools.is_empty()
                    && !self.projector.last_text.is_empty()
                {
                    self.complete_turn_if_open();
                }
            }
            LogEvent::ToolExecute {
                id,
                name,
                arguments,
                content,
            } => {
                if !self.projector.turn_open {
                    self.ensure_turn();
                }
                if self.projector.seen_tools.insert(id.clone()) {
                    let args: Value = serde_json::from_str(&arguments).unwrap_or(json!({}));
                    self.push(
                        "item/tool_started",
                        json!({
                            "toolCallId": id,
                            "itemId": id,
                            "toolName": name,
                            "title": name,
                            "arguments": args
                        }),
                    );
                }
                self.projector.pending_tools.remove(&id);
                self.push(
                    "item/tool_completed",
                    json!({
                        "toolCallId": id,
                        "itemId": id,
                        "toolName": name,
                        "title": name,
                        "output": content,
                        "status": "completed"
                    }),
                );
            }
            LogEvent::PreStep | LogEvent::Prompt(_) | LogEvent::SystemReminder(_) => {}
        }
    }

    pub fn permission_requested(&mut self, prompt: &PermissionPrompt) {
        if !self.projector.turn_open {
            self.ensure_turn();
        }
        let id = format!("perm-{}", self.projector.seq + 1);
        self.projector.perm_id = Some(id.clone());
        self.push(
            "permission/requested",
            json!({
                "requestId": id,
                "toolName": prompt.tool,
                "title": prompt.tool,
                "summary": prompt.summary,
                "reason": prompt.summary
            }),
        );
    }

    pub fn permission_resolved(&mut self, kind: PermissionOptionKind) {
        let Some(id) = self.projector.perm_id.take() else {
            return;
        };
        let decision = if kind.is_allow() { "approve" } else { "deny" };
        self.push(
            "permission/resolved",
            json!({
                "requestId": id,
                "decision": decision,
                "always": matches!(kind, PermissionOptionKind::AllowAlways | PermissionOptionKind::RejectAlways)
            }),
        );
    }

    pub fn interaction_requested(&mut self, ask: &Ask) {
        let Some(front) = ask.front() else {
            return;
        };
        if !self.projector.turn_open {
            self.ensure_turn();
        }
        let id = format!("ask-{}", self.projector.seq + 1);
        self.projector.ask_id = Some(id.clone());
        let questions: Vec<Value> = front
            .questions
            .iter()
            .enumerate()
            .map(|(i, q)| {
                let options: Vec<Value> = q
                    .options
                    .iter()
                    .map(|o| {
                        json!({
                            "label": o.label,
                            "description": if o.description.is_empty() { o.label.as_str() } else { o.description.as_str() },
                            "recommended": false
                        })
                    })
                    .collect();
                json!({
                    "id": q.id.clone().unwrap_or_else(|| format!("question_{}", i + 1)),
                    "header": q.question.chars().take(80).collect::<String>(),
                    "question": q.question,
                    "options": options
                })
            })
            .collect();
        self.push(
            "interaction/requested",
            json!({
                "interactionId": id,
                "kind": "user_input",
                "questions": questions
            }),
        );
    }

    pub fn interaction_resolved(&mut self) {
        let Some(id) = self.projector.ask_id.take() else {
            return;
        };
        self.push(
            "interaction/resolved",
            json!({ "interactionId": id, "ok": true }),
        );
    }

    pub fn plan_requested(&mut self, prompt: &PlanApprovalPrompt) {
        if !self.projector.turn_open {
            self.ensure_turn();
        }
        let id = format!("plan-{}", self.projector.seq + 1);
        self.projector.plan_id = Some(id.clone());
        self.push(
            "plan/requested",
            json!({
                "planId": id,
                "path": prompt.path,
                "body": prompt.body,
                "empty": prompt.empty
            }),
        );
    }

    pub fn plan_resolved(&mut self, decision: PlanDecision) {
        let Some(id) = self.projector.plan_id.take() else {
            return;
        };
        let decision = match decision {
            PlanDecision::Approve => "approve",
            PlanDecision::Revise => "revise",
            PlanDecision::Quit => "quit",
        };
        self.push(
            "plan/resolved",
            json!({ "planId": id, "decision": decision }),
        );
    }

    pub fn elicit_requested(&mut self, prompt: &ElicitPrompt) {
        if !self.projector.turn_open {
            self.ensure_turn();
        }
        let id = format!("elicit-{}", self.projector.seq + 1);
        self.projector.elicit_id = Some(id.clone());
        self.push(
            "elicit/requested",
            json!({
                "elicitId": id,
                "server": prompt.server,
                "message": prompt.message,
                "heading": prompt.heading
            }),
        );
    }

    pub fn elicit_resolved(&mut self) {
        let Some(id) = self.projector.elicit_id.take() else {
            return;
        };
        self.push("elicit/resolved", json!({ "elicitId": id, "ok": true }));
    }

    fn ensure_turn(&mut self) {
        if self.projector.turn_open {
            return;
        }
        self.projector.turn_n += 1;
        self.projector.turn_id = format!("t{}", self.projector.turn_n);
        self.projector.turn_open = true;
        self.push("turn/started", json!({ "status": "running" }));
    }

    fn complete_turn_if_open(&mut self) {
        if !self.projector.turn_open {
            return;
        }
        self.projector.turn_open = false;
        self.push("turn/completed", json!({ "status": "completed" }));
    }

    fn push(&mut self, method: &str, mut payload: Value) {
        self.projector.seq += 1;
        let seq = self.projector.seq;
        let turn_id = self.projector.turn_id.clone();
        if let Some(obj) = payload.as_object_mut() {
            obj.entry("turnId")
                .or_insert_with(|| Value::String(turn_id.clone()));
            obj.entry("threadId")
                .or_insert_with(|| Value::String(LIVE_THREAD_ID.into()));
        }
        let event = ProjectedEvent {
            seq,
            method: method.into(),
            thread_id: LIVE_THREAD_ID.into(),
            turn_id,
            timestamp: iso_now(),
            payload,
        };
        self.events.push(event.clone());
        let _ = self.tx.send(event);
    }
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

fn iso_now() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{ms}")
}
