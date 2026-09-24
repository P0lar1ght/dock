//! MCP `elicitation/create` (form + url) and `notifications/elicitation/complete`.
//!
//! Transport calls [`Elicitation::create`] / [`Elicitation::complete_url`].
//! TUI live-looks [`Elicitation::front`] and resolves with accept / decline / cancel.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use cordis::Context;
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use crate::names::MCP_ELICIT_EVENT;

pub const MAX_ELICIT_FIELDS: usize = 32;
pub const MAX_ELICIT_MESSAGE_CHARS: usize = 4096;
pub const MAX_ELICIT_URL_CHARS: usize = 2048;
pub const MAX_ELICIT_ID_CHARS: usize = 128;
pub const MAX_ELICIT_NAME_CHARS: usize = 64;
pub const MAX_ELICIT_TITLE_CHARS: usize = 128;
pub const MAX_ELICIT_SCHEMA_BYTES: usize = 64 * 1024;
pub const MAX_ELICIT_ENUM_VALUES: usize = 32;
pub const MAX_ELICIT_ENUM_VALUE_CHARS: usize = 128;
pub const MAX_ELICIT_DRAFT_CHARS: usize = 4096;

const METHOD_CREATE: &str = "elicitation/create";
const METHOD_COMPLETE: &str = "notifications/elicitation/complete";
const OTHER_LABEL: &str = "其他";
const OTHER_VALUE: &str = "Other";

#[derive(Debug, Clone)]
pub struct ElicitOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub enum ElicitFieldKind {
    String {
        default: Option<String>,
    },
    Number {
        default: Option<String>,
    },
    Integer {
        default: Option<String>,
    },
    Boolean {
        default: bool,
    },
    SingleSelect {
        options: Vec<ElicitOption>,
        default_index: Option<usize>,
    },
    MultiSelect {
        options: Vec<ElicitOption>,
        default_indexes: Vec<usize>,
    },
    Unsupported {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct ElicitFieldSpec {
    pub name: String,
    pub title: String,
    pub required: bool,
    pub kind: ElicitFieldKind,
}

/// What the overlay should paint for the current step.
#[derive(Debug, Clone)]
pub struct ElicitPrompt {
    pub server: String,
    pub message: String,
    pub heading: String,
    pub options: Vec<String>,
    pub typing: bool,
    pub multi: bool,
    pub draft: String,
    /// Index of the freeform 「其他」 row, if this step is a select.
    pub other_index: Option<usize>,
    pub selected: usize,
    pub picked: Vec<bool>,
}

enum Kind {
    Url {
        url: String,
        elicitation_id: String,
        opened: bool,
    },
    Form {
        fields: Vec<ElicitFieldSpec>,
        index: usize,
        answers: Map<String, Value>,
        picked: Vec<bool>,
    },
}

struct Job {
    /// 入队序号，见 [`Elicitation::front_seq_for`]。
    seq: u64,
    server: String,
    message: String,
    kind: Kind,
    /// `main` / `main#N` of the call that was in flight. `None` shows on
    /// whichever page is open.
    origin: Option<String>,
    tx: oneshot::Sender<Value>,
}

struct Inner {
    ctx: Context,
    queue: Mutex<VecDeque<Job>>,
    /// In-flight MCP calls and the page each belongs to, innermost last. The
    /// read loop that receives `elicitation/create` is not the tool task, so
    /// it cannot see `exec_ctx`; the tool task pushes here for the duration of
    /// `tools/call` and its guard removes its own entry on return.
    callers: Mutex<Vec<Caller>>,
    /// `tools/call` JSON-RPC id → page, in the order they were sent. A question
    /// that arrives on that request's own HTTP response is looked up by id, so
    /// two pages can call the same server at once and each keep its own prompt.
    inflight: Mutex<Vec<(u64, Option<String>)>>,
    next_caller: AtomicU64,
    next_job: AtomicU64,
}

/// One in-flight `tools/call`. The id lets a returning call drop its own entry
/// instead of the stack top — two pages calling concurrently return in any
/// order, and a blind pop would leave the other page's entry behind.
struct Caller {
    id: u64,
    page: Option<String>,
}

/// Removes this call's own entry when the MCP tool returns.
pub struct PageGuard {
    inner: Arc<Inner>,
    id: u64,
}

impl Drop for PageGuard {
    fn drop(&mut self) {
        let mut callers = self.inner.callers.lock().unwrap();
        callers.retain(|c| c.id != self.id);
    }
}

/// Removes this `tools/call` from the in-flight map when the RPC returns.
pub struct RequestGuard {
    inner: Arc<Inner>,
    request_id: u64,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.inner
            .inflight
            .lock()
            .unwrap()
            .retain(|(id, _)| *id != self.request_id);
    }
}

/// Which page should see this question.
///
/// A related `tools/call` id wins. Otherwise the oldest in-flight page that
/// does not already have a prompt — never `callers.last()`, which is whichever
/// page happened to start second.
fn pick_origin(
    inflight: &[(u64, Option<String>)],
    callers: &[Caller],
    queue: &VecDeque<Job>,
    related: Option<u64>,
) -> Option<String> {
    if let Some(id) = related {
        if let Some(page) = inflight
            .iter()
            .find(|(rid, _)| *rid == id)
            .and_then(|(_, page)| page.clone())
        {
            return Some(page);
        }
    }
    let pages: Vec<String> = if inflight.iter().any(|(_, page)| page.is_some()) {
        inflight
            .iter()
            .filter_map(|(_, page)| page.clone())
            .collect()
    } else {
        callers.iter().filter_map(|c| c.page.clone()).collect()
    };
    pages
        .iter()
        .find(|page| {
            !queue
                .iter()
                .any(|job| job.origin.as_deref() == Some(page.as_str()))
        })
        .cloned()
        .or_else(|| pages.into_iter().next())
}

fn index_of(queue: &VecDeque<Job>, page: Option<&str>) -> Option<usize> {
    match page {
        None => (!queue.is_empty()).then_some(0),
        Some(page) => queue
            .iter()
            .position(|job| job.origin.as_deref().is_none_or(|origin| origin == page)),
    }
}

/// Named queue on `"mcp"`. Clone shares the same jobs.
#[derive(Clone)]
pub struct Elicitation {
    inner: Arc<Inner>,
}

impl Elicitation {
    pub fn new(ctx: Context) -> Self {
        Self {
            inner: Arc::new(Inner {
                ctx,
                queue: Mutex::new(VecDeque::new()),
                callers: Mutex::new(Vec::new()),
                inflight: Mutex::new(Vec::new()),
                next_caller: AtomicU64::new(0),
                next_job: AtomicU64::new(0),
            }),
        }
    }

    pub fn is_create(method: &str) -> bool {
        method == METHOD_CREATE
    }

    pub fn is_complete(method: &str) -> bool {
        method == METHOD_COMPLETE
    }

    pub async fn create(&self, server: &str, params: Value) -> Value {
        self.create_for(server, params, None).await
    }

    /// `related` is the client `tools/call` id this question belongs to.
    ///
    /// HTTP carries the question on that call's own POST response, so the id
    /// is known and the prompt stays on the page that sent it. A shared stdio
    /// stream has no such channel: with one call in flight it takes that page,
    /// with several it takes the oldest call that does not already have a
    /// prompt, instead of whoever pushed last.
    pub async fn create_for(&self, server: &str, params: Value, related: Option<u64>) -> Value {
        let (kind, message) = match parse_params(&params) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(server, "elicitation declined: {e}");
                return decline_value();
            }
        };
        let (tx, rx) = oneshot::channel();
        {
            let inflight = self.inner.inflight.lock().unwrap();
            let callers = self.inner.callers.lock().unwrap();
            let mut q = self.inner.queue.lock().unwrap();
            let origin = pick_origin(&inflight, &callers, &q, related);
            q.push_back(Job {
                seq: self
                    .inner
                    .next_job
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1,
                server: server.to_string(),
                message,
                kind,
                origin,
                tx,
            });
        }
        self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
        rx.await.unwrap_or_else(|_| cancel_value())
    }

    /// Bind a `tools/call` JSON-RPC id to the page that sent it. Held until the
    /// call returns, including while its elicitation is on screen.
    pub fn track_request(&self, request_id: u64, page: Option<String>) -> RequestGuard {
        self.inner.inflight.lock().unwrap().push((request_id, page));
        RequestGuard {
            inner: Arc::clone(&self.inner),
            request_id,
        }
    }

    /// Remember which page's tool call is in flight so a server elicitation
    /// opened on the read loop lands on that page.
    pub fn scope_caller(&self) -> PageGuard {
        let page = crate::tools::registry::exec_ctx()
            .as_ref()
            .and_then(crate::session::log::Sessions::page_of);
        self.scope_page(page)
    }

    pub fn scope_page(&self, page: Option<String>) -> PageGuard {
        let id = self
            .inner
            .next_caller
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.callers.lock().unwrap().push(Caller {
            id,
            page: page.clone(),
        });
        PageGuard {
            inner: Arc::clone(&self.inner),
            id,
        }
    }

    pub fn front(&self) -> Option<ElicitPrompt> {
        self.front_for(None)
    }

    /// First prompt that belongs on `page`. `None` is the raw head (tests).
    pub fn front_for(&self, page: Option<&str>) -> Option<ElicitPrompt> {
        let q = self.inner.queue.lock().unwrap();
        index_of(&q, page).and_then(|i| q.get(i).map(job_prompt))
    }

    /// `front_for(page)` 那一条的入队序号（单调递增），投影方用它去重。
    pub fn front_seq_for(&self, page: Option<&str>) -> Option<u64> {
        let q = self.inner.queue.lock().unwrap();
        index_of(&q, page).and_then(|i| q.get(i).map(|job| job.seq))
    }

    pub fn cancel(&self) {
        self.cancel_for(None);
    }

    pub fn cancel_for(&self, page: Option<&str>) {
        let mut q = self.inner.queue.lock().unwrap();
        if let Some(i) = index_of(&q, page) {
            let job = q.remove(i).unwrap();
            let _ = job.tx.send(cancel_value());
        }
        drop(q);
        self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
    }

    pub fn decline(&self) {
        self.decline_for(None);
    }

    pub fn decline_for(&self, page: Option<&str>) {
        let mut q = self.inner.queue.lock().unwrap();
        if let Some(i) = index_of(&q, page) {
            let job = q.remove(i).unwrap();
            let _ = job.tx.send(decline_value());
        }
        drop(q);
        self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
    }

    /// URL mode: open the browser. Does not finish the JSON-RPC request.
    pub fn open_url(&self) -> Result<String, String> {
        self.open_url_on(None)
    }

    pub fn open_url_on(&self, page: Option<&str>) -> Result<String, String> {
        let url = {
            let mut q = self.inner.queue.lock().unwrap();
            let Some(i) = index_of(&q, page) else {
                return Err("没有待处理的 elicitation".into());
            };
            let job = q.get_mut(i).unwrap();
            let Kind::Url { url, opened, .. } = &mut job.kind else {
                return Err("当前不是链接 elicitation".into());
            };
            *opened = true;
            url.clone()
        };
        webbrowser::open(&url).map_err(|e| e.to_string())?;
        self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
        Ok(url)
    }

    pub fn accept_option(
        &self,
        selected: usize,
        picked: &[bool],
        draft: &str,
    ) -> Result<(), String> {
        self.accept_option_on(None, selected, picked, draft)
    }

    pub fn accept_option_on(
        &self,
        page: Option<&str>,
        selected: usize,
        picked: &[bool],
        draft: &str,
    ) -> Result<(), String> {
        let mut q = self.inner.queue.lock().unwrap();
        let Some(i) = index_of(&q, page) else {
            return Ok(());
        };
        let job = q.get_mut(i).unwrap();
        match &mut job.kind {
            Kind::Url { .. } => {
                if selected == 0 {
                    drop(q);
                    self.open_url_on(page).map(|_| ())
                } else {
                    let job = q.remove(i).unwrap();
                    let _ = job.tx.send(decline_value());
                    drop(q);
                    self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
                    Ok(())
                }
            }
            Kind::Form {
                fields,
                index,
                answers,
                picked: slot,
            } => {
                let field = fields
                    .get(*index)
                    .ok_or_else(|| "没有当前字段".to_string())?;
                let value = option_value(field, selected, picked, draft)?;
                answers.insert(field.name.clone(), value);
                *index += 1;
                if *index >= fields.len() {
                    let job = q.remove(i).unwrap();
                    let Kind::Form { answers, .. } = job.kind else {
                        unreachable!();
                    };
                    let _ = job.tx.send(accept_value(Value::Object(answers)));
                    drop(q);
                    self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
                    return Ok(());
                }
                *slot = default_picked(&fields[*index]);
                drop(q);
                self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
                Ok(())
            }
        }
    }

    pub fn accept_text(&self, text: &str) -> Result<(), String> {
        self.accept_text_on(None, text)
    }

    pub fn accept_text_on(&self, page: Option<&str>, text: &str) -> Result<(), String> {
        let mut q = self.inner.queue.lock().unwrap();
        let Some(i) = index_of(&q, page) else {
            return Ok(());
        };
        let job = q.get_mut(i).unwrap();
        let Kind::Form {
            fields,
            index,
            answers,
            picked,
        } = &mut job.kind
        else {
            return Err("当前不是表单 elicitation".into());
        };
        let field = fields
            .get(*index)
            .ok_or_else(|| "没有当前字段".to_string())?;
        let value = text_value(field, text)?;
        answers.insert(field.name.clone(), value);
        *index += 1;
        if *index >= fields.len() {
            let job = q.remove(i).unwrap();
            let Kind::Form { answers, .. } = job.kind else {
                unreachable!();
            };
            let _ = job.tx.send(accept_value(Value::Object(answers)));
            drop(q);
            self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
            return Ok(());
        }
        *picked = default_picked(&fields[*index]);
        drop(q);
        self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
        Ok(())
    }

    /// Dual-resolve from the web gateway. Empty queue is an error.
    pub fn resolve(&self, action: &str, content: Option<Value>) -> Result<(), String> {
        self.resolve_on(None, action, content)
    }

    pub fn resolve_on(
        &self,
        page: Option<&str>,
        action: &str,
        content: Option<Value>,
    ) -> Result<(), String> {
        let mut q = self.inner.queue.lock().unwrap();
        let Some(i) = index_of(&q, page) else {
            return Err("no pending elicitation".into());
        };
        let job = q.remove(i).unwrap();
        let payload = match action {
            "accept" | "approve" => accept_value(content.unwrap_or_else(|| json!({}))),
            "decline" | "deny" => decline_value(),
            _ => cancel_value(),
        };
        let _ = job.tx.send(payload);
        drop(q);
        self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
        Ok(())
    }

    pub fn complete_url(&self, params: &Value) {
        let id = params
            .get("elicitationId")
            .or_else(|| params.get("elicitation_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if id.is_empty() || !chars_within(id, MAX_ELICIT_ID_CHARS) {
            return;
        }
        let mut q = self.inner.queue.lock().unwrap();
        if let Some(pos) = q.iter().position(|job| match &job.kind {
            Kind::Url { elicitation_id, .. } => elicitation_id == id,
            _ => false,
        }) {
            let job = q.remove(pos).unwrap();
            let _ = job.tx.send(accept_value(json!({})));
        }
        drop(q);
        self.inner.ctx.emit(MCP_ELICIT_EVENT, ());
    }
}

fn job_prompt(job: &Job) -> ElicitPrompt {
    match &job.kind {
        Kind::Url { url, opened, .. } => ElicitPrompt {
            server: job.server.clone(),
            message: job.message.clone(),
            heading: if *opened {
                format!("已打开 {url}，等待服务器确认")
            } else {
                url.clone()
            },
            options: vec!["打开链接".into(), "拒绝".into()],
            typing: false,
            multi: false,
            draft: String::new(),
            other_index: None,
            selected: 0,
            picked: Vec::new(),
        },
        Kind::Form {
            fields,
            index,
            picked,
            ..
        } => {
            let field = fields.get(*index);
            let heading = field
                .map(|f| format!("{}（{}/{}）", f.title, *index + 1, fields.len().max(1)))
                .unwrap_or_default();
            let (options, typing, multi, draft, other_index) =
                field
                    .map(field_ui)
                    .unwrap_or((Vec::new(), false, false, String::new(), None));
            let selected = field.map(default_selected).unwrap_or(0);
            ElicitPrompt {
                server: job.server.clone(),
                message: job.message.clone(),
                heading,
                options,
                typing,
                multi,
                draft,
                other_index,
                selected,
                picked: picked.clone(),
            }
        }
    }
}

fn field_ui(field: &ElicitFieldSpec) -> (Vec<String>, bool, bool, String, Option<usize>) {
    match &field.kind {
        ElicitFieldKind::Boolean { default } => (
            vec!["是".into(), "否".into()],
            false,
            false,
            if *default {
                "是".into()
            } else {
                String::new()
            },
            None,
        ),
        ElicitFieldKind::SingleSelect { options, .. } => (
            options.iter().map(|o| o.label.clone()).collect(),
            false,
            false,
            String::new(),
            other_index(options),
        ),
        ElicitFieldKind::MultiSelect { options, .. } => (
            options.iter().map(|o| o.label.clone()).collect(),
            false,
            true,
            String::new(),
            other_index(options),
        ),
        ElicitFieldKind::String { default }
        | ElicitFieldKind::Number { default }
        | ElicitFieldKind::Integer { default } => (
            Vec::new(),
            true,
            false,
            default.clone().unwrap_or_default(),
            None,
        ),
        ElicitFieldKind::Unsupported { reason } => {
            (vec![reason.clone()], false, false, String::new(), None)
        }
    }
}

fn default_selected(field: &ElicitFieldSpec) -> usize {
    match &field.kind {
        ElicitFieldKind::Boolean { default } => {
            if *default {
                0
            } else {
                1
            }
        }
        ElicitFieldKind::SingleSelect { default_index, .. } => default_index.unwrap_or(0),
        _ => 0,
    }
}

fn default_picked(field: &ElicitFieldSpec) -> Vec<bool> {
    match &field.kind {
        ElicitFieldKind::MultiSelect {
            options,
            default_indexes,
        } => options
            .iter()
            .enumerate()
            .map(|(i, _)| default_indexes.contains(&i))
            .collect(),
        _ => Vec::new(),
    }
}

fn option_value(
    field: &ElicitFieldSpec,
    selected: usize,
    picked: &[bool],
    draft: &str,
) -> Result<Value, String> {
    match &field.kind {
        ElicitFieldKind::Boolean { .. } => Ok(Value::Bool(selected == 0)),
        ElicitFieldKind::SingleSelect { options, .. } => {
            let option = options
                .get(selected)
                .ok_or_else(|| "无效选项".to_string())?;
            if is_other_option(option) {
                other_content(draft)
            } else {
                Ok(Value::String(option.value.clone()))
            }
        }
        ElicitFieldKind::MultiSelect { options, .. } => {
            let mut values = Vec::new();
            for (i, option) in options.iter().enumerate() {
                if !picked.get(i).copied().unwrap_or(false) {
                    continue;
                }
                if is_other_option(option) {
                    values.push(other_content(draft)?);
                } else {
                    values.push(Value::String(option.value.clone()));
                }
            }
            if field.required && values.is_empty() {
                return Err("请至少选一项".into());
            }
            Ok(Value::Array(values))
        }
        ElicitFieldKind::Unsupported { reason } => Err(reason.clone()),
        _ => Err("当前字段需要输入文字".into()),
    }
}

fn other_content(draft: &str) -> Result<Value, String> {
    let text = draft.trim();
    if text.is_empty() {
        return Err("请输入具体内容".into());
    }
    if !chars_within(text, MAX_ELICIT_DRAFT_CHARS) {
        return Err("输入过长".into());
    }
    Ok(Value::String(text.to_string()))
}

fn text_value(field: &ElicitFieldSpec, text: &str) -> Result<Value, String> {
    let text = text.trim();
    if field.required && text.is_empty() {
        return Err("此项必填".into());
    }
    if !chars_within(text, MAX_ELICIT_DRAFT_CHARS) {
        return Err("输入过长".into());
    }
    match &field.kind {
        ElicitFieldKind::String { .. } => Ok(Value::String(text.to_string())),
        ElicitFieldKind::Number { .. } => {
            let n: f64 = text.parse().map_err(|_| "请输入数字".to_string())?;
            Ok(json!(n))
        }
        ElicitFieldKind::Integer { .. } => {
            let n: i64 = text.parse().map_err(|_| "请输入整数".to_string())?;
            Ok(json!(n))
        }
        _ => Err("当前字段不是文字输入".into()),
    }
}

fn parse_params(params: &Value) -> Result<(Kind, String), String> {
    let message = params
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    if !chars_within(&message, MAX_ELICIT_MESSAGE_CHARS) {
        return Err("message too long".into());
    }
    let mode = params.get("mode").and_then(|m| m.as_str()).unwrap_or("");
    if mode == "url" || params.get("url").is_some() {
        let url = params
            .get("url")
            .and_then(|u| u.as_str())
            .ok_or("url elicitation needs url")?
            .to_string();
        let elicitation_id = params
            .get("elicitationId")
            .or_else(|| params.get("elicitation_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !chars_within(&url, MAX_ELICIT_URL_CHARS)
            || !chars_within(&elicitation_id, MAX_ELICIT_ID_CHARS)
        {
            return Err("url elicitation fields too long".into());
        }
        if url.is_empty() {
            return Err("url elicitation needs url".into());
        }
        return Ok((
            Kind::Url {
                url,
                elicitation_id,
                opened: false,
            },
            message,
        ));
    }
    let schema = params
        .get("requestedSchema")
        .or_else(|| params.get("requested_schema"))
        .cloned()
        .unwrap_or_else(|| json!({"type":"object","properties":{}}));
    let fields = parse_form_schema(&schema)?;
    if fields
        .iter()
        .any(|f| matches!(f.kind, ElicitFieldKind::Unsupported { .. }))
    {
        return Err("unsupported elicitation field".into());
    }
    let picked = fields.first().map(default_picked).unwrap_or_default();
    Ok((
        Kind::Form {
            fields,
            index: 0,
            answers: Map::new(),
            picked,
        },
        message,
    ))
}

pub fn parse_form_schema(schema: &Value) -> Result<Vec<ElicitFieldSpec>, String> {
    let Some(obj) = schema.as_object() else {
        return Err("requestedSchema must be an object".into());
    };
    let type_ok = obj
        .get("type")
        .and_then(|t| t.as_str())
        .is_none_or(|t| t == "object");
    if !type_ok {
        return Err("requestedSchema.type must be \"object\"".into());
    }
    let bytes = serde_json::to_vec(schema)
        .map(|b| b.len())
        .unwrap_or(usize::MAX);
    if bytes > MAX_ELICIT_SCHEMA_BYTES {
        return Err("requestedSchema too large".into());
    }
    let required: HashSet<String> = obj
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let Some(props) = obj.get("properties").and_then(|p| p.as_object()) else {
        return Ok(Vec::new());
    };
    if props.len() > MAX_ELICIT_FIELDS {
        return Err("too many elicitation fields".into());
    }
    let mut fields = Vec::new();
    for (name, prop) in props {
        if !chars_within(name, MAX_ELICIT_NAME_CHARS) {
            return Err("property name too long".into());
        }
        fields.push(field_from_schema(name, prop, required.contains(name))?);
    }
    Ok(fields)
}

fn field_from_schema(name: &str, prop: &Value, required: bool) -> Result<ElicitFieldSpec, String> {
    let title = prop
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or(name)
        .to_string();
    if !chars_within(&title, MAX_ELICIT_TITLE_CHARS) {
        return Err("title too long".into());
    }
    let default_str = prop.get("default").map(|d| match d {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    });
    let kind = field_kind(prop, default_str)?;
    Ok(ElicitFieldSpec {
        name: name.to_string(),
        title,
        required,
        kind,
    })
}

fn field_kind(prop: &Value, default_str: Option<String>) -> Result<ElicitFieldKind, String> {
    if let Some(values) = prop.get("enum").and_then(|e| e.as_array()) {
        let names = prop.get("enumNames").and_then(|n| n.as_array());
        let options: Vec<ElicitOption> = values
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let value = json_scalar(v)?;
                let label = names
                    .and_then(|n| n.get(i))
                    .and_then(|l| l.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| value.clone());
                Some(ElicitOption { value, label })
            })
            .collect();
        check_options(&options)?;
        let mut options = options;
        ensure_other(&mut options);
        let default_index = default_str
            .as_deref()
            .and_then(|d| options.iter().position(|o| o.value == d));
        return Ok(ElicitFieldKind::SingleSelect {
            options,
            default_index,
        });
    }
    let ty = prop
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("string");
    Ok(match ty {
        "string" => ElicitFieldKind::String {
            default: default_str,
        },
        "number" => ElicitFieldKind::Number {
            default: default_str,
        },
        "integer" => ElicitFieldKind::Integer {
            default: default_str,
        },
        "boolean" => ElicitFieldKind::Boolean {
            default: prop
                .get("default")
                .and_then(|d| d.as_bool())
                .unwrap_or(false),
        },
        "array" => multi_select(prop)?,
        other => ElicitFieldKind::Unsupported {
            reason: format!("unsupported type {other:?}"),
        },
    })
}

fn multi_select(prop: &Value) -> Result<ElicitFieldKind, String> {
    let Some(items) = prop.get("items") else {
        return Ok(ElicitFieldKind::Unsupported {
            reason: "array without items".into(),
        });
    };
    let options: Vec<ElicitOption> =
        if let Some(values) = items.get("enum").and_then(|e| e.as_array()) {
            values
                .iter()
                .filter_map(|v| {
                    let value = json_scalar(v)?;
                    Some(ElicitOption {
                        label: value.clone(),
                        value,
                    })
                })
                .collect()
        } else {
            return Ok(ElicitFieldKind::Unsupported {
                reason: "array without enum items".into(),
            });
        };
    check_options(&options)?;
    let mut options = options;
    ensure_other(&mut options);
    let default_indexes = prop
        .get("default")
        .and_then(|d| d.as_array())
        .map(|defaults| {
            defaults
                .iter()
                .filter_map(|d| d.as_str())
                .filter_map(|d| options.iter().position(|o| o.value == d))
                .collect()
        })
        .unwrap_or_default();
    Ok(ElicitFieldKind::MultiSelect {
        options,
        default_indexes,
    })
}

pub fn is_other_label(label: &str) -> bool {
    let t = label.trim();
    t.eq_ignore_ascii_case(OTHER_VALUE) || t == OTHER_LABEL
}

fn is_other_option(option: &ElicitOption) -> bool {
    is_other_label(&option.label) || is_other_label(&option.value)
}

fn other_index(options: &[ElicitOption]) -> Option<usize> {
    options.iter().position(is_other_option)
}

/// Last row like Ask: a freeform 「其他」 the user types into. Not submitted
/// as the sentinel — typed text is the value.
fn ensure_other(options: &mut Vec<ElicitOption>) {
    if options.is_empty() {
        return;
    }
    if options.iter().any(is_other_option) {
        return;
    }
    if options.len() >= MAX_ELICIT_ENUM_VALUES {
        return;
    }
    options.push(ElicitOption {
        value: OTHER_VALUE.into(),
        label: OTHER_LABEL.into(),
    });
}

fn check_options(options: &[ElicitOption]) -> Result<(), String> {
    if options.len() > MAX_ELICIT_ENUM_VALUES {
        return Err("too many enum values".into());
    }
    if options.iter().any(|o| {
        !chars_within(&o.value, MAX_ELICIT_ENUM_VALUE_CHARS)
            || !chars_within(&o.label, MAX_ELICIT_ENUM_VALUE_CHARS)
    }) {
        return Err("enum value too long".into());
    }
    Ok(())
}

fn json_scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub fn chars_within(s: &str, max: usize) -> bool {
    s.chars().count() <= max
}

pub fn accept_value(content: Value) -> Value {
    json!({ "action": "accept", "content": content })
}

pub fn decline_value() -> Value {
    json!({ "action": "decline" })
}

pub fn cancel_value() -> Value {
    json!({ "action": "cancel" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_form_enum_and_string() {
        let schema = json!({
            "type": "object",
            "properties": {
                "env": { "title": "环境", "enum": ["dev", "prod"], "enumNames": ["开发", "生产"] },
                "note": { "type": "string", "title": "备注" }
            },
            "required": ["env"]
        });
        let fields = parse_form_schema(&schema).unwrap();
        assert_eq!(fields.len(), 2);
        assert!(matches!(
            fields[0].kind,
            ElicitFieldKind::SingleSelect { .. }
        ));
        assert!(matches!(fields[1].kind, ElicitFieldKind::String { .. }));
    }

    #[test]
    fn parse_params_url() {
        let (kind, msg) = parse_params(&json!({
            "mode": "url",
            "message": "登录",
            "url": "https://example.com/auth",
            "elicitationId": "e1"
        }))
        .unwrap();
        assert_eq!(msg, "登录");
        match kind {
            Kind::Url {
                url,
                elicitation_id,
                ..
            } => {
                assert_eq!(url, "https://example.com/auth");
                assert_eq!(elicitation_id, "e1");
            }
            _ => panic!("expected url"),
        }
    }

    #[test]
    fn oversized_message_declines() {
        let params = json!({
            "message": "m".repeat(MAX_ELICIT_MESSAGE_CHARS + 1),
            "requestedSchema": { "type": "object", "properties": {} }
        });
        assert!(parse_params(&params).is_err());
    }

    #[tokio::test]
    async fn create_and_accept_boolean() {
        let root = Context::new();
        let elicit = Elicitation::new(root);
        let params = json!({
            "message": "继续？",
            "requestedSchema": {
                "type": "object",
                "properties": { "ok": { "type": "boolean", "title": "确认" } }
            }
        });
        let handle = tokio::spawn({
            let elicit = elicit.clone();
            async move { elicit.create("local", params).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let front = elicit.front().unwrap();
        assert_eq!(front.options, ["是", "否"]);
        elicit.accept_option(0, &[], "").unwrap();
        let result = handle.await.unwrap();
        assert_eq!(result["action"], "accept");
        assert_eq!(result["content"]["ok"], true);
    }

    #[tokio::test]
    async fn complete_url_accepts() {
        let root = Context::new();
        let elicit = Elicitation::new(root);
        let params = json!({
            "mode": "url",
            "message": "登录",
            "url": "https://example.com",
            "elicitationId": "e1"
        });
        let handle = tokio::spawn({
            let elicit = elicit.clone();
            async move { elicit.create("http", params).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        elicit.complete_url(&json!({ "elicitationId": "e1" }));
        let result = handle.await.unwrap();
        assert_eq!(result["action"], "accept");
    }

    #[test]
    fn method_names() {
        assert!(Elicitation::is_create("elicitation/create"));
        assert!(Elicitation::is_complete(
            "notifications/elicitation/complete"
        ));
    }

    #[test]
    fn enum_appends_other() {
        let schema = json!({
            "type": "object",
            "properties": {
                "env": { "title": "环境", "enum": ["dev", "prod"], "enumNames": ["开发", "生产"] }
            }
        });
        let fields = parse_form_schema(&schema).unwrap();
        match &fields[0].kind {
            ElicitFieldKind::SingleSelect { options, .. } => {
                assert_eq!(options.len(), 3);
                assert_eq!(options[2].label, OTHER_LABEL);
                assert_eq!(options[2].value, OTHER_VALUE);
            }
            other => panic!("expected single select, got {other:?}"),
        }
    }

    #[test]
    fn does_not_append_other_twice() {
        let schema = json!({
            "type": "object",
            "properties": {
                "env": { "enum": ["dev", "Other"] }
            }
        });
        let fields = parse_form_schema(&schema).unwrap();
        match &fields[0].kind {
            ElicitFieldKind::SingleSelect { options, .. } => {
                assert_eq!(options.len(), 2);
                assert!(is_other_option(&options[1]));
            }
            other => panic!("expected single select, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_prompt_stamped_for_one_page_stays_off_the_other() {
        let elicit = Elicitation::new(Context::new());
        let params = json!({
            "message": "环境？",
            "requestedSchema": {
                "type": "object",
                "properties": { "env": { "enum": ["dev"] } }
            }
        });
        let handle = tokio::spawn({
            let elicit = elicit.clone();
            async move {
                let _page = elicit.scope_page(Some("main#2".into()));
                elicit.create("local", params).await
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(elicit.front_for(Some("main")).is_none());
        assert!(elicit.front_for(Some("main#2")).is_some());
        elicit.cancel_for(Some("main"));
        assert!(elicit.front_for(Some("main#2")).is_some());
        elicit.cancel_for(Some("main#2"));
        assert!(elicit.front().is_none());
        let _ = handle.await;
    }

    #[tokio::test]
    async fn a_returning_call_does_not_drop_the_other_pages_caller() {
        // 两页同时调同一个 MCP 服务器时，先返回的那一页不能把后返回那页的来源页
        // 条目一起弹掉，否则它的提问会盖到不存在的页上。
        let elicit = Elicitation::new(Context::new());
        let first = elicit.scope_page(Some("main#2".into()));
        let _second = elicit.scope_page(Some("main#3".into()));
        drop(first);
        {
            let callers = elicit.inner.callers.lock().unwrap();
            assert_eq!(
                callers.last().map(|c| c.page.as_deref()),
                Some(Some("main#3")),
                "后返回那页的条目该还在"
            );
            assert_eq!(callers.len(), 1);
        }

        let params = json!({ "message": "环境？" });
        let handle = tokio::spawn({
            let elicit = elicit.clone();
            async move { elicit.create("local", params).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let origin = elicit
            .inner
            .queue
            .lock()
            .unwrap()
            .front()
            .and_then(|job| job.origin.clone());
        assert_eq!(
            origin.as_deref(),
            Some("main#3"),
            "提问该盖在还在跑的那一页上"
        );
        elicit.cancel();
        let _ = handle.await;
    }

    /// 后发出的那次调用先被服务器提问时，框仍留在它自己的页上，不盖到先发出的那页。
    #[tokio::test]
    async fn a_later_calls_question_stays_on_its_own_page() {
        let elicit = Elicitation::new(Context::new());
        let _first = elicit.track_request(1, Some("main".into()));
        let _second = elicit.track_request(2, Some("main#2".into()));
        let params = json!({ "message": "后一页" });
        let handle = tokio::spawn({
            let elicit = elicit.clone();
            async move { elicit.create_for("local", params, Some(2)).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(
            elicit.front_for(Some("main#2")).map(|p| p.message),
            Some("后一页".into())
        );
        assert!(elicit.front_for(Some("main")).is_none());
        elicit.cancel_for(Some("main#2"));
        let _ = handle.await;
    }

    #[tokio::test]
    async fn other_requires_typed_content() {
        let root = Context::new();
        let elicit = Elicitation::new(root);
        let params = json!({
            "message": "环境？",
            "requestedSchema": {
                "type": "object",
                "properties": { "env": { "enum": ["dev", "prod"] } }
            }
        });
        let handle = tokio::spawn({
            let elicit = elicit.clone();
            async move { elicit.create("local", params).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let front = elicit.front().unwrap();
        let other = front.other_index.expect("other row");
        assert_eq!(
            elicit.accept_option(other, &[], "").unwrap_err(),
            "请输入具体内容"
        );
        elicit.accept_option(other, &[], " staging ").unwrap();
        let result = handle.await.unwrap();
        assert_eq!(result["action"], "accept");
        assert_eq!(result["content"]["env"], "staging");
    }

    /// 可选文本字段留空必须能提交 —— 否则表单走不完，MCP 请求永远不 resolve，
    /// 工具调用和整轮对话一起卡死。必填的仍要挡住。
    ///
    /// TUI 侧不再自己预判空输入，就靠这里的两条分支；改这里要同步想清楚
    /// `accept_elicit` 会把哪条错误闪给用户。
    #[tokio::test]
    async fn optional_text_accepts_empty_but_required_does_not() {
        let root = Context::new();
        let elicit = Elicitation::new(root);
        let params = json!({
            "message": "备注",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "must": { "type": "string", "title": "必填" },
                    "note": { "type": "string", "title": "可选备注" }
                },
                "required": ["must"]
            }
        });
        let handle = tokio::spawn({
            let elicit = elicit.clone();
            async move { elicit.create("local", params).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // 字段顺序跟 schema 的 properties 走：先 must，后 note。
        assert_eq!(elicit.accept_text("").unwrap_err(), "此项必填");
        elicit.accept_text("有值").unwrap();
        // 可选字段留空直接过。
        elicit.accept_text("").unwrap();

        let result = handle.await.unwrap();
        assert_eq!(result["action"], "accept");
        assert_eq!(result["content"]["must"], "有值");
        assert_eq!(result["content"]["note"], "");
    }

    #[tokio::test]
    async fn other_multi_replaces_sentinel_with_draft() {
        let root = Context::new();
        let elicit = Elicitation::new(root);
        let params = json!({
            "message": "选环境",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "env": {
                        "type": "array",
                        "items": { "enum": ["dev", "prod"] }
                    }
                },
                "required": ["env"]
            }
        });
        let handle = tokio::spawn({
            let elicit = elicit.clone();
            async move { elicit.create("local", params).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let front = elicit.front().unwrap();
        let other = front.other_index.expect("other row");
        let mut picked = vec![false; front.options.len()];
        picked[0] = true;
        picked[other] = true;
        elicit.accept_option(other, &picked, "staging").unwrap();
        let result = handle.await.unwrap();
        assert_eq!(result["content"]["env"], json!(["dev", "staging"]));
    }
}
