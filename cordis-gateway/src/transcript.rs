//! Number projected notifications for history/subscribe. Does not change spine events.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::sync::broadcast;

use cordis_spine::{
    Ask, ElicitPrompt, LogEvent, PermissionOptionKind, PermissionPrompt, PlanApprovalPrompt,
    PlanDecision, Sessions, TurnEndStatus, UserImage, INTERRUPTED_TOOL_RESULT,
    PERMISSION_DENIED_TOOL_RESULT, ROOT_IDENTITY,
};

use crate::protocol::LIVE_THREAD_ID;

const BROADCAST_CAP: usize = 256;

#[derive(Clone, Debug)]
pub struct ProjectedEvent {
    pub seq: u64,
    pub method: String,
    /// 发出它的页（`main` / `main#N`）。订阅按页匹配，见 `ws.rs`。
    pub page: String,
    /// 推送时这一页在协议里的 `threadId`（第 1 页 `live`，其它页是会话 id）。
    pub thread_id: String,
    pub turn_id: String,
    pub timestamp: String,
    pub payload: Value,
}

impl ProjectedEvent {
    pub fn as_notification(&self) -> Value {
        self.as_notification_as(&self.thread_id)
    }

    /// 以订阅方用的 `threadId` 推送：第 1 页既可以按 `live` 也可以按会话 id 订。
    pub fn as_notification_as(&self, thread_id: &str) -> Value {
        let mut params = flatten_payload(&self.payload);
        params.insert("seq".into(), json!(self.seq));
        params.insert("transcriptSeq".into(), json!(self.seq));
        params.insert("threadId".into(), json!(thread_id));
        params.insert("turnId".into(), json!(self.turn_id));
        params.insert("timestamp".into(), json!(self.timestamp));
        json!({
            "method": self.method,
            "params": params
        })
    }

    pub fn as_history_item(&self) -> Value {
        self.as_history_item_as(&self.thread_id)
    }

    pub fn as_history_item_as(&self, thread_id: &str) -> Value {
        let mut payload = flatten_payload(&self.payload);
        payload.insert("threadId".into(), json!(thread_id));
        payload.insert("turnId".into(), json!(self.turn_id));
        json!({
            "timestamp": self.timestamp,
            "threadId": thread_id,
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

#[derive(Default)]
struct Projector {
    seq: u64,
    turn_n: u64,
    turn_id: String,
    last_text: String,
    seen_tools: HashSet<String>,
    pending_tools: HashSet<String>,
    turn_open: bool,
    /// 这一轮最后一次采样的 `LlmOutput::error`。只给没有 `turn-end` 行的旧会话
    /// 回放用（[`Transcript::close_legacy_turn`]）；新记录里 Dock 已经把它算进
    /// `TurnEnd` 的状态了。
    sample_error: Option<String>,
    perm_id: Option<String>,
    ask_id: Option<String>,
    plan_id: Option<String>,
    elicit_id: Option<String>,
    /// 已经报过 requested 的队首序号（`front_seq`）。同一条还挂着时再来事件不重报。
    perm_seq: Option<u64>,
    ask_seq: Option<u64>,
    plan_seq: Option<u64>,
    elicit_seq: Option<u64>,
}

/// 一页的投影。各页的 `Transcript` 共用网关的一条 broadcast，事件上带着页身份。
pub struct Transcript {
    events: Vec<ProjectedEvent>,
    projector: Projector,
    tx: broadcast::Sender<ProjectedEvent>,
    page: String,
    thread_id: String,
    /// 回放时给事件打的落盘时间（毫秒）；`None` 用当前时间。
    clock: Option<u128>,
}

impl Transcript {
    /// 第 1 页、自带一条 broadcast（单测用）。
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Self::for_page(ROOT_IDENTITY, LIVE_THREAD_ID, tx)
    }

    pub fn for_page(
        page: impl Into<String>,
        thread_id: impl Into<String>,
        tx: broadcast::Sender<ProjectedEvent>,
    ) -> Self {
        Self {
            events: Vec::new(),
            projector: Projector::default(),
            tx,
            page: page.into(),
            thread_id: thread_id.into(),
            clock: None,
        }
    }

    /// 网关共用的那条 broadcast 的容量。
    pub fn channel() -> broadcast::Sender<ProjectedEvent> {
        broadcast::channel(BROADCAST_CAP).0
    }

    /// 这一页换了会话（`thread/start` 等）之后，之后推的事件用新 id。
    pub fn set_thread_id(&mut self, thread_id: impl Into<String>) {
        self.thread_id = thread_id.into();
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
        self.replay(
            &sessions.events(),
            &sessions.times(),
            &sessions.user_images(),
        );
    }

    /// 从落盘事件重建投影：重开会话（`reset_from_sessions`）和看关着的会话
    /// （`thread/history`）走同一条路。时间戳用落盘时间；每轮按落盘的
    /// [`LogEvent::TurnEnd`] 收尾。更早的会话没有这一行，就在下一条用户消息前、
    /// 以及末尾按它最后一条事件的时间补一个（[`Self::close_legacy_turn`]）——重建
    /// 只发生在页空闲时（`thread/start` / restore 之后、关着的会话）。
    pub fn replay(&mut self, events: &[LogEvent], times: &[SystemTime], images: &[Vec<UserImage>]) {
        self.events.clear();
        self.projector = Projector::default();
        let ms = |i: usize| times.get(i).map(|t| unix_ms(*t));
        let mut user_i = 0usize;
        for (i, event) in events.iter().enumerate() {
            if matches!(event, LogEvent::User(_)) && i > 0 {
                self.clock = ms(i - 1);
                self.close_legacy_turn();
            }
            self.clock = ms(i);
            let attachments = if matches!(event, LogEvent::User(_)) {
                let row = images.get(user_i).cloned().unwrap_or_default();
                user_i += 1;
                attachment_values(&row)
            } else {
                Vec::new()
            };
            self.ingest_log_with(event.clone(), &attachments);
        }
        self.clock = events.len().checked_sub(1).and_then(ms);
        self.close_legacy_turn();
        self.clock = None;
    }

    #[cfg(test)]
    pub fn ingest_log(&mut self, event: LogEvent) {
        self.ingest_log_with(event, &[]);
    }

    pub fn ingest_log_with(&mut self, event: LogEvent, user_attachments: &[Value]) {
        match event {
            LogEvent::User(text) => {
                self.complete_turn_if_open();
                self.projector.turn_n += 1;
                self.projector.turn_id = format!("t{}", self.projector.turn_n);
                self.projector.last_text.clear();
                self.projector.sample_error = None;
                self.projector.seen_tools.clear();
                self.projector.pending_tools.clear();
                self.projector.turn_open = true;
                let turn_id = self.projector.turn_id.clone();
                self.push("turn/started", json!({ "status": "running" }));
                let mut payload = json!({ "content": text, "turnId": turn_id });
                if !user_attachments.is_empty() {
                    payload["attachments"] = Value::Array(user_attachments.to_vec());
                }
                self.push("item/user_message", payload);
            }
            LogEvent::LlmStream(out) => {
                if !self.projector.turn_open {
                    self.ensure_turn();
                }
                if let Some(delta) = stream_text_delta(&self.projector.last_text, &out.text) {
                    self.push("item/message_delta", json!({ "delta": delta }));
                }
                self.projector.last_text.clone_from(&out.text);
                self.projector.sample_error.clone_from(&out.error);
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
                // 不在这里结束一轮：流式每一段都是「有文本、没工具」，以前每段都发一
                // 对 turn/completed + turn/started。一轮结束只看 `session/turn-end`
                // （[`Transcript::turn_ended`]）。
            }
            LogEvent::ToolExecute {
                id,
                name,
                arguments,
                content,
                images: _,
                is_error,
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
                // 照 Dock 落下的标记报，不按输出猜。停止时补的中断结果算 cancelled，
                // 权限门拒绝的算 denied。
                let status = if !is_error {
                    "completed"
                } else if content.trim() == INTERRUPTED_TOOL_RESULT {
                    "cancelled"
                } else if content.trim() == PERMISSION_DENIED_TOOL_RESULT {
                    "denied"
                } else {
                    "failed"
                };
                self.push(
                    "item/tool_completed",
                    json!({
                        "toolCallId": id,
                        "itemId": id,
                        "toolName": name,
                        "title": name,
                        "output": content,
                        "status": status
                    }),
                );
            }
            LogEvent::TurnEnd(status) => self.turn_ended(&status),
            // Notice 只给 TUI 用户看，不进 dock.1 投影，也不该变成一条聊天消息。
            LogEvent::PreStep
            | LogEvent::Prompt(_)
            | LogEvent::SystemReminder(_)
            | LogEvent::Notice { .. } => {}
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
                    "options": options,
                    "multiSelect": q.multi_select.unwrap_or(false)
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

    /// 按权限队列对账：队首换了才报 `permission/requested`（先把被换下的那条报
    /// resolved），队空了报 resolved。队列事件的载荷是 `()`，别的页排队、同一条
    /// 还挂着都会再来一次事件——没有序号就会重复报。
    pub fn sync_permission(
        &mut self,
        front: Option<(u64, PermissionPrompt)>,
        last: Option<PermissionOptionKind>,
    ) {
        let seq = front.as_ref().map(|(s, _)| *s);
        if seq == self.projector.perm_seq {
            return;
        }
        self.permission_resolved(last.unwrap_or(PermissionOptionKind::RejectOnce));
        self.projector.perm_seq = seq;
        if let Some((_, prompt)) = front {
            self.permission_requested(&prompt);
        }
    }

    /// 同 [`Self::sync_permission`]，提问队列。翻题、作答也会发事件，序号不变就不重报。
    pub fn sync_interaction(&mut self, seq: Option<u64>, ask: &Ask) {
        if seq == self.projector.ask_seq {
            return;
        }
        self.interaction_resolved();
        self.projector.ask_seq = seq;
        if seq.is_some() {
            self.interaction_requested(ask);
        }
    }

    /// 同 [`Self::sync_permission`]，计划审批。
    pub fn sync_plan(
        &mut self,
        front: Option<(u64, PlanApprovalPrompt)>,
        last: Option<PlanDecision>,
    ) {
        let seq = front.as_ref().map(|(s, _)| *s);
        if seq == self.projector.plan_seq {
            return;
        }
        self.plan_resolved(last.unwrap_or(PlanDecision::Quit));
        self.projector.plan_seq = seq;
        if let Some((_, prompt)) = front {
            self.plan_requested(&prompt);
        }
    }

    /// 同 [`Self::sync_permission`]，MCP elicitation（按页取队首）。
    pub fn sync_elicit(&mut self, front: Option<(u64, ElicitPrompt)>) {
        let seq = front.as_ref().map(|(s, _)| *s);
        if seq == self.projector.elicit_seq {
            return;
        }
        self.elicit_resolved();
        self.projector.elicit_seq = seq;
        if let Some((_, prompt)) = front {
            self.elicit_requested(&prompt);
        }
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

    /// 一轮结束（[`LogEvent::TurnEnd`]，实时和回放同一条路）。`status` 是
    /// completed / cancelled / failed，照 Dock 记的报；失败带 `error`，否则客户端
    /// 只见一轮空结束。
    pub fn turn_ended(&mut self, status: &TurnEndStatus) {
        self.projector.sample_error = None;
        if !self.projector.turn_open {
            return;
        }
        self.projector.turn_open = false;
        let payload = match status {
            TurnEndStatus::Completed => json!({ "status": "completed" }),
            TurnEndStatus::Cancelled => json!({ "status": "cancelled" }),
            TurnEndStatus::Failed(error) => json!({ "status": "failed", "error": error }),
        };
        self.push("turn/completed", payload);
    }

    /// 旧会话没有 `turn-end` 行：按当时的规则补一个——最后一次采样带错误算失败，
    /// 否则算完成（停止在旧记录里看不出来）。这一轮已经收尾时什么都不做。
    fn close_legacy_turn(&mut self) {
        let status = match self.projector.sample_error.clone() {
            Some(error) => TurnEndStatus::Failed(error),
            None => TurnEndStatus::Completed,
        };
        self.turn_ended(&status);
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
                .or_insert_with(|| Value::String(self.thread_id.clone()));
        }
        let event = ProjectedEvent {
            seq,
            method: method.into(),
            page: self.page.clone(),
            thread_id: self.thread_id.clone(),
            turn_id,
            timestamp: self.clock.map_or_else(iso_now, |ms| ms.to_string()),
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

fn unix_ms(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

fn iso_now() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{ms}")
}

pub(crate) fn attachment_values(images: &[UserImage]) -> Vec<Value> {
    images
        .iter()
        .filter(|img| matches!(img.mime.as_str(), "image/png" | "image/jpeg" | "image/webp"))
        .filter(|img| img.width > 0 && img.height > 0 && !img.data.is_empty())
        .map(|img| {
            json!({
                "type": "image",
                "mimeType": img.mime,
                "width": img.width,
                "height": img.height,
                "byteLength": img.data.len()
            })
        })
        .collect()
}

/// Incremental assistant text for `item/message_delta`.
///
/// Spine re-emits the full accumulated `LlmOutput.text` on every token. A new
/// sample (empty `begin_llm`, or a rewritten buffer) is **not** a byte prefix of
/// the previous string — slicing at `previous.len()` panics inside a multibyte
/// char such as `）`.
fn stream_text_delta(previous: &str, current: &str) -> Option<String> {
    if current == previous {
        return None;
    }
    if let Some(delta) = current.strip_prefix(previous) {
        return if delta.is_empty() {
            None
        } else {
            Some(delta.to_string())
        };
    }
    if current.is_empty() {
        None
    } else {
        Some(current.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_spine::LlmOutput;

    fn deltas(t: &Transcript) -> Vec<String> {
        t.history_since(0)
            .into_iter()
            .filter(|e| e.method == "item/message_delta")
            .map(|e| {
                e.payload
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    fn methods(t: &Transcript) -> Vec<String> {
        t.history_since(0).into_iter().map(|e| e.method).collect()
    }

    fn prompt(summary: &str) -> PermissionPrompt {
        PermissionPrompt {
            tool: "bash".into(),
            summary: summary.into(),
        }
    }

    /// 回归：权限事件载荷是 `()`，别的页排队、同一条还挂着都会再来事件。按队首
    /// 序号对账：同一条不重报；换了一条先报旧的 resolved 再报新的 requested；
    /// 队空了报 resolved。以前第二条排队的请求会让第一条永远等不到 resolved。
    #[test]
    fn permission_sync_reports_each_request_once() {
        let mut t = Transcript::new();
        t.sync_permission(Some((1, prompt("ls"))), None);
        t.sync_permission(Some((1, prompt("ls"))), None);
        assert_eq!(
            methods(&t)
                .iter()
                .filter(|m| m.starts_with("permission/"))
                .count(),
            1,
            "同一条请求只报一次：{:?}",
            methods(&t)
        );

        t.sync_permission(
            Some((2, prompt("rm"))),
            Some(PermissionOptionKind::AllowOnce),
        );
        t.sync_permission(None, Some(PermissionOptionKind::RejectOnce));
        let perms: Vec<String> = methods(&t)
            .into_iter()
            .filter(|m| m.starts_with("permission/"))
            .collect();
        assert_eq!(
            perms,
            [
                "permission/requested",
                "permission/resolved",
                "permission/requested",
                "permission/resolved"
            ]
        );
    }

    /// 回归：翻题、作答也会发 `ask/pending`；以前每翻一次就重报一次
    /// `interaction/requested`（每次一个新 id）。
    #[tokio::test]
    async fn interaction_sync_ignores_navigation() {
        let ask = std::sync::Arc::new(Ask::new(cordis::Context::new()));
        let question = |text: &str| cordis_spine::Question {
            question: text.into(),
            options: vec![cordis_spine::QuestionOption {
                label: "好".into(),
                description: String::new(),
                preview: None,
                id: None,
            }],
            multi_select: None,
            id: None,
        };
        let waiting = ask.clone();
        let asked =
            tokio::spawn(
                async move { waiting.ask(vec![question("一？"), question("二？")]).await },
            );
        while ask.front_seq().is_none() {
            tokio::task::yield_now().await;
        }

        let mut t = Transcript::new();
        t.sync_interaction(ask.front_seq(), &ask);
        ask.answer_current(vec!["好".into()], None);
        ask.navigate(1);
        t.sync_interaction(ask.front_seq(), &ask);
        t.sync_interaction(ask.front_seq(), &ask);
        assert_eq!(methods(&t), ["turn/started", "interaction/requested"]);

        ask.cancel();
        t.sync_interaction(ask.front_seq(), &ask);
        assert_eq!(methods(&t).last().unwrap(), "interaction/resolved");
        asked.await.unwrap();
    }

    /// 多选题要把 `multiSelect` 带出去，客户端才知道能选几个；没写的算单选。
    #[tokio::test]
    async fn interaction_requested_carries_multi_select() {
        let ask = std::sync::Arc::new(Ask::new(cordis::Context::new()));
        let question = |text: &str, multi: Option<bool>| cordis_spine::Question {
            question: text.into(),
            options: vec![cordis_spine::QuestionOption {
                label: "好".into(),
                description: String::new(),
                preview: None,
                id: None,
            }],
            multi_select: multi,
            id: None,
        };
        let waiting = ask.clone();
        let asked = tokio::spawn(async move {
            waiting
                .ask(vec![question("多？", Some(true)), question("单？", None)])
                .await
        });
        while ask.front_seq().is_none() {
            tokio::task::yield_now().await;
        }
        let mut t = Transcript::new();
        t.sync_interaction(ask.front_seq(), &ask);
        let requested = t
            .history_since(0)
            .into_iter()
            .find(|e| e.method == "interaction/requested")
            .unwrap();
        let questions = requested.payload["questions"].as_array().unwrap();
        assert_eq!(questions[0]["multiSelect"], true);
        assert_eq!(questions[1]["multiSelect"], false);
        ask.cancel();
        asked.await.unwrap();
    }

    #[test]
    fn notice_is_not_projected() {
        let mut t = Transcript::new();
        t.ingest_log(LogEvent::User("hi".into()));
        let after_user = t.history_since(0).len();
        t.ingest_log(LogEvent::Notice {
            kind: cordis_spine::NoticeKind::WorkflowReport,
            title: "deep-research · planner 上报".into(),
            body: "阶段性结论".into(),
        });
        assert_eq!(
            t.history_since(0).len(),
            after_user,
            "Notice 不该变成 dock.1 事件：{:?}",
            t.history_since(0)
                .iter()
                .map(|e| &e.method)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn stream_text_delta_suffix_on_utf8_growth() {
        assert_eq!(stream_text_delta("你", "你好").as_deref(), Some("好"));
        assert_eq!(stream_text_delta("你好", "你好").as_deref(), None);
    }

    #[test]
    fn stream_text_delta_does_not_slice_inside_multibyte_char() {
        let previous = "a".repeat(45);
        let current = "bash 工具被权限拒 1m29 需要审批）。我用";
        assert!(!current.is_char_boundary(previous.len()));
        assert_eq!(
            stream_text_delta(&previous, current).as_deref(),
            Some(current)
        );
        assert_eq!(stream_text_delta(&previous, "").as_deref(), None);
    }

    #[test]
    fn llm_stream_rewrite_after_long_prefix_does_not_panic() {
        let mut t = Transcript::new();
        t.ingest_log(LogEvent::User("hi".into()));
        t.ingest_log(LogEvent::LlmStream(LlmOutput {
            text: "a".repeat(45),
            ..Default::default()
        }));
        t.ingest_log(LogEvent::LlmStream(LlmOutput {
            text: "bash 工具被权限拒 1m29 需要审批）。我用".into(),
            ..Default::default()
        }));
        assert!(deltas(&t).iter().any(|d| d.contains("需要审批）")));
    }

    #[test]
    fn llm_stream_empty_resets_prefix_before_next_sample() {
        let mut t = Transcript::new();
        t.ingest_log(LogEvent::User("hi".into()));
        t.ingest_log(LogEvent::LlmStream(LlmOutput {
            text: "a".repeat(45),
            ..Default::default()
        }));
        t.ingest_log(LogEvent::LlmStream(LlmOutput::default()));
        t.ingest_log(LogEvent::LlmStream(LlmOutput {
            text: "bash 工具被权限拒 1m29 需要审批）。我用".into(),
            ..Default::default()
        }));
        let got = deltas(&t);
        assert_eq!(
            got.last().map(String::as_str),
            Some("bash 工具被权限拒 1m29 需要审批）。我用")
        );
    }

    /// 回归：流式每一段都是「有文本、没工具」，以前每段都发一对
    /// turn/completed + turn/started（轮次 id 一路涨）。现在一轮只在
    /// `session/turn-end`（`turn_ended`）时结束一次；用量更新发的是原样副本，
    /// 完整回复只推一次。
    #[test]
    fn a_streamed_reply_is_one_turn_and_pushed_once() {
        let mut t = Transcript::new();
        t.ingest_log(LogEvent::User("hi".into()));
        t.ingest_log(LogEvent::LlmStream(LlmOutput::default()));
        for s in ["当前", "当前目录", "当前目录只有", "当前目录只有"] {
            t.ingest_log(LogEvent::LlmStream(LlmOutput {
                text: s.into(),
                ..Default::default()
            }));
        }
        assert!(
            !methods(&t).iter().any(|m| m == "turn/completed"),
            "流式中途不该结束：{:?}",
            methods(&t)
        );
        t.turn_ended(&TurnEndStatus::Completed);
        t.turn_ended(&TurnEndStatus::Completed);
        assert_eq!(deltas(&t).concat(), "当前目录只有");
        let m = methods(&t);
        assert_eq!(
            m.iter().filter(|m| *m == "turn/started").count(),
            1,
            "{m:?}"
        );
        assert_eq!(
            m.iter().filter(|m| *m == "turn/completed").count(),
            1,
            "{m:?}"
        );
        assert_eq!(m.last().map(String::as_str), Some("turn/completed"));
    }

    /// 回归：一轮出错（如模型请求失败）以前只回给 TUI，dock.1 客户端只看到
    /// `turn/completed {status: completed}`、没有任何产出，不知道出了错。
    #[test]
    fn turn_end_reports_failure_and_cancel() {
        let completed = |t: &Transcript| {
            t.history_since(0)
                .into_iter()
                .filter(|e| e.method == "turn/completed")
                .map(|e| e.payload)
                .collect::<Vec<_>>()
        };
        let mut t = Transcript::new();
        t.ingest_log(LogEvent::User("hi".into()));
        t.turn_ended(&TurnEndStatus::Failed("模型请求失败：HTTP 400".into()));
        t.ingest_log(LogEvent::User("again".into()));
        t.turn_ended(&TurnEndStatus::Cancelled);
        let ends = completed(&t);
        assert_eq!(ends.len(), 2, "{ends:?}");
        assert_eq!(ends[0]["status"], "failed");
        assert_eq!(ends[0]["error"], "模型请求失败：HTTP 400");
        assert_eq!(ends[1]["status"], "cancelled");
        assert!(ends[1].get("error").is_none(), "{:?}", ends[1]);
    }

    /// 回放照落盘的 `turn-end` 报每一轮的结果（停止、出错带原文，用它的时间），
    /// 不再一律补成完成；只有更早、没有这一行的会话才按旧规则补（最后一次采样
    /// 带错误算失败）。
    #[test]
    fn replay_reports_recorded_turn_ends_and_falls_back_for_old_rows() {
        let at = |s: u64| UNIX_EPOCH + std::time::Duration::from_secs(s);
        let events = vec![
            LogEvent::User("a".into()),
            LogEvent::LlmStream(LlmOutput {
                text: "partial".into(),
                ..Default::default()
            }),
            LogEvent::TurnEnd(TurnEndStatus::Cancelled),
            LogEvent::User("b".into()),
            LogEvent::TurnEnd(TurnEndStatus::Failed("HTTP 404".into())),
            // 旧会话接着往下聊：这一轮没有 turn-end。
            LogEvent::User("c".into()),
            LogEvent::LlmStream(LlmOutput {
                error: Some("HTTP 500".into()),
                ..Default::default()
            }),
        ];
        let times = vec![
            at(100),
            at(110),
            at(133),
            at(200),
            at(201),
            at(300),
            at(302),
        ];
        let mut t = Transcript::new();
        t.replay(&events, &times, &[]);
        let ends: Vec<_> = t
            .history_since(0)
            .into_iter()
            .filter(|e| e.method == "turn/completed")
            .collect();
        assert_eq!(ends.len(), 3, "{:?}", methods(&t));
        assert_eq!(ends[0].payload["status"], "cancelled");
        assert_eq!(ends[0].timestamp, "133000", "用 turn-end 自己的时间");
        assert_eq!(ends[1].payload["status"], "failed");
        assert_eq!(ends[1].payload["error"], "HTTP 404");
        assert_eq!(ends[2].payload["status"], "failed");
        assert_eq!(ends[2].payload["error"], "HTTP 500");
    }

    /// 回归：`item/tool_completed` 以前恒为 completed，客户端只能按输出猜失败。
    /// 现在照 Dock 落下的 `is_error` 报：失败 failed、停止时补的「已中断。」cancelled、
    /// 用户在权限门拒绝的 denied。
    #[test]
    fn tool_completed_status_follows_the_recorded_flag() {
        let tool = |id: &str, content: &str, is_error: bool| LogEvent::ToolExecute {
            id: id.into(),
            name: "bash".into(),
            arguments: "{}".into(),
            content: content.into(),
            images: Vec::new(),
            is_error,
        };
        let mut t = Transcript::new();
        t.ingest_log(LogEvent::User("hi".into()));
        t.ingest_log(tool("a", "ok", false));
        t.ingest_log(tool("b", "exit status: 1\nboom", true));
        t.ingest_log(tool("c", cordis_spine::INTERRUPTED_TOOL_RESULT, true));
        t.ingest_log(tool("d", cordis_spine::PERMISSION_DENIED_TOOL_RESULT, true));
        let statuses: Vec<_> = t
            .history_since(0)
            .into_iter()
            .filter(|e| e.method == "item/tool_completed")
            .map(|e| e.payload["status"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(statuses, ["completed", "failed", "cancelled", "denied"]);
    }

    /// 回归：从落盘事件重建投影（重开会话、看关着的会话）以前时间戳全是「现在」，
    /// 最后一轮只有 turn/started 没有 turn/completed —— 客户端当它还在跑。
    #[test]
    fn replay_closes_every_turn_and_keeps_recorded_times() {
        let at = |s: u64| UNIX_EPOCH + std::time::Duration::from_secs(s);
        let events = vec![
            LogEvent::User("a".into()),
            LogEvent::LlmStream(LlmOutput {
                text: "ok".into(),
                ..Default::default()
            }),
            LogEvent::User("b".into()),
            LogEvent::LlmStream(LlmOutput {
                error: Some("HTTP 500".into()),
                ..Default::default()
            }),
        ];
        let times = vec![at(100), at(130), at(200), at(203)];
        let mut t = Transcript::new();
        t.replay(&events, &times, &[]);
        let all = t.history_since(0);
        let ends: Vec<_> = all
            .iter()
            .filter(|e| e.method == "turn/completed")
            .collect();
        assert_eq!(ends.len(), 2, "每一轮都要收尾：{:?}", methods(&t));
        assert_eq!(ends[0].payload["status"], "completed");
        assert_eq!(ends[1].payload["status"], "failed");
        assert_eq!(ends[1].payload["error"], "HTTP 500");
        let started: Vec<_> = all.iter().filter(|e| e.method == "turn/started").collect();
        assert_eq!(started[0].timestamp, "100000", "用落盘的时间");
        assert_eq!(ends[0].timestamp, "130000");
        assert_eq!(ends[1].timestamp, "203000");
    }

    #[test]
    fn llm_stream_emits_char_suffix_when_text_grows() {
        let mut t = Transcript::new();
        t.ingest_log(LogEvent::User("hi".into()));
        t.ingest_log(LogEvent::LlmStream(LlmOutput {
            text: "你".into(),
            ..Default::default()
        }));
        t.ingest_log(LogEvent::LlmStream(LlmOutput {
            text: "你好".into(),
            ..Default::default()
        }));
        assert_eq!(deltas(&t), vec!["你".to_string(), "好".to_string()]);
    }

    #[test]
    fn user_message_includes_attachment_metadata() {
        let mut t = Transcript::new();
        t.ingest_log_with(
            LogEvent::User("[Image #1] 这是什么".into()),
            &[json!({
                "type": "image",
                "mimeType": "image/png",
                "width": 8,
                "height": 8,
                "byteLength": 32
            })],
        );
        let user = t
            .history_since(0)
            .into_iter()
            .find(|e| e.method == "item/user_message")
            .expect("user message");
        assert_eq!(user.payload["content"], "[Image #1] 这是什么");
        assert_eq!(user.payload["attachments"][0]["mimeType"], "image/png");
        assert!(user.payload["attachments"][0].get("data").is_none());
    }
}
