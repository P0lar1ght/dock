//! `task` — spawn a nested Grok-shaped turn. Coordinator is `"subagents"`.
//!
//! Child sessions are a Cordis isolate of `"sessions"` + `"turn"` so the parent
//! transcript and TUI stay clean. The loop plugin is not forked: children run
//! [`crate::runtime::GrokStep`] through [`crate::runtime::LoopHandle`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use serde_json::Value;

use crate::names::{SESSIONS, SUBAGENTS, TOOLS, TURN};
use crate::runtime::{GrokStep, LoopHandle};
use crate::session::Sessions;
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::turn::TurnControl;
use crate::types::{LogEvent, ToolCall, ToolResult, ToolSpec, TurnOutcome};

/// Copied from Grok `MAX_SUBAGENT_DEPTH` (default when not injected).
pub const MAX_SUBAGENT_DEPTH: u32 = 1;

tokio::task_local! {
    static DEPTH: u32;
}

const TASK_PARAMS: &str = r#"{"type":"object","properties":{"prompt":{"type":"string","description":"The full task prompt for the subagent to execute."},"description":{"type":"string","description":"Short description of the task (3-5 words)."},"subagent_type":{"type":"string","description":"Built-in types: general-purpose, explore, plan."},"run_in_background":{"type":"boolean","description":"Returns immediately with a subagent_id. Use get_task_output to retrieve results. Default true."},"resume_from":{"type":"string","description":"Resume a completed subagent_id in this session."}},"required":["prompt","description"]}"#;

const TASK_DESC: &str = "Launch a subagent to handle a task autonomously.\n\
- prompt: the full task for the child.\n\
- description: 3–5 words.\n\
- subagent_type: general-purpose (default), explore (read-only codebase), plan.\n\
- run_in_background: default true. Returns subagent_id; use get_task_output with task_ids=[\"id\"] and a positive timeout_ms.\n\
- resume_from: completed subagent_id; same subagent_type; new prompt is appended.\n\
Max nesting depth is 1. Prefer explore for questions about the tree.";

const TYPES: &[&str] = &["general-purpose", "explore", "plan"];

#[derive(Clone, Debug)]
pub struct SubagentSnap {
    pub id: String,
    pub description: String,
    pub subagent_type: String,
    pub done: bool,
    pub cancelled: bool,
    pub output: String,
}

struct Slot {
    description: String,
    subagent_type: String,
    done: AtomicBool,
    cancelled: AtomicBool,
    output: Mutex<String>,
    events: Mutex<Vec<LogEvent>>,
    turn: Mutex<Option<Arc<TurnControl>>>,
    _hold: Mutex<Vec<Disposable>>,
}

/// Named `"subagents"` service. Live-lookup; do not capture the Arc.
pub struct Subagents {
    ctx: Context,
    seq: AtomicU64,
    inner: Mutex<HashMap<String, Arc<Slot>>>,
}

impl Subagents {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            seq: AtomicU64::new(1),
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn list(&self) -> Vec<SubagentSnap> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .map(|(id, s)| snap(id, s))
            .collect()
    }

    pub fn snapshot(&self, id: &str) -> Option<SubagentSnap> {
        self.inner.lock().unwrap().get(id).map(|s| snap(id, s))
    }

    pub async fn wait(&self, ids: &[String], timeout_ms: u64) -> Vec<SubagentSnap> {
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(timeout_ms.max(1));
        loop {
            let snaps: Vec<_> = ids.iter().filter_map(|id| self.snapshot(id)).collect();
            if snaps.iter().all(|s| s.done) || tokio::time::Instant::now() >= deadline {
                return if snaps.is_empty() {
                    ids.iter()
                        .map(|id| SubagentSnap {
                            id: id.clone(),
                            description: String::new(),
                            subagent_type: String::new(),
                            done: true,
                            cancelled: false,
                            output: format!(
                                "Task or subagent {id} not found. No background tasks or subagents exist in this session."
                            ),
                        })
                        .collect()
                } else {
                    snaps
                };
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    pub fn kill(&self, id: &str) -> Option<String> {
        let slot = self.inner.lock().unwrap().get(id).cloned()?;
        if slot.done.load(Ordering::Relaxed) {
            return Some(format!("{id} already finished"));
        }
        slot.cancelled.store(true, Ordering::Relaxed);
        if let Some(turn) = slot.turn.lock().unwrap().as_ref() {
            turn.cancel();
        }
        Some(format!("killed {id}"))
    }

    fn alloc(&self, description: String, subagent_type: String) -> (String, Arc<Slot>) {
        let id = format!("sa-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let slot = Arc::new(Slot {
            description,
            subagent_type,
            done: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            output: Mutex::new(String::new()),
            events: Mutex::new(Vec::new()),
            turn: Mutex::new(None),
            _hold: Mutex::new(Vec::new()),
        });
        self.inner.lock().unwrap().insert(id.clone(), slot.clone());
        (id, slot)
    }
}

fn snap(id: &str, s: &Slot) -> SubagentSnap {
    SubagentSnap {
        id: id.into(),
        description: s.description.clone(),
        subagent_type: s.subagent_type.clone(),
        done: s.done.load(Ordering::Relaxed),
        cancelled: s.cancelled.load(Ordering::Relaxed),
        output: s.output.lock().unwrap().clone(),
    }
}

struct SpawnReq {
    prompt: String,
    description: String,
    subagent_type: String,
    background: bool,
    resume_from: Option<String>,
}

pub fn tool_task() -> Plugin {
    plugin("tool-task", Inject::from([TOOLS]), |ctx, _: &()| {
        ctx.provide(SUBAGENTS, Subagents::new(ctx.clone()))?;
        let tools = ctx.require::<Tools>(TOOLS)?;
        let body: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { run_task(&ctx, call).await })
            })
        };
        own_registered(
            ctx,
            vec![tools.register(
                ToolSpec {
                    name: "task".into(),
                    description: TASK_DESC.into(),
                    parameters_json: TASK_PARAMS.into(),
                },
                body,
            )?],
        )?;
        Ok(None)
    })
}

fn current_depth() -> u32 {
    DEPTH.try_with(|d| *d).unwrap_or(0)
}

fn parse_spawn(raw: &str) -> Result<SpawnReq, String> {
    let v: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let prompt = v
        .get("prompt")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if prompt.is_empty() {
        return Err("Error: prompt is required".into());
    }
    let description = v
        .get("description")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("subagent")
        .to_string();
    let subagent_type = v
        .get("subagent_type")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("general-purpose")
        .to_string();
    if !TYPES.contains(&subagent_type.as_str()) {
        return Err(format!(
            "Unknown subagent_type {subagent_type:?}. Available: {}.",
            TYPES.join(", ")
        ));
    }
    let background = v
        .get("run_in_background")
        .and_then(|x| x.as_bool())
        .unwrap_or(true);
    let resume_from = v
        .get("resume_from")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_sentinel(s))
        .map(str::to_string);
    Ok(SpawnReq {
        prompt,
        description,
        subagent_type,
        background,
        resume_from,
    })
}

fn is_sentinel(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "" | "null" | "none" | "undefined"
    )
}

async fn run_task(ctx: &Context, call: ToolCall) -> ToolResult {
    let req = match parse_spawn(&call.arguments) {
        Ok(r) => r,
        Err(e) => return tool_result(call, e),
    };
    if current_depth() >= MAX_SUBAGENT_DEPTH {
        return tool_result(
            call,
            format!("Error: max subagent nesting depth is {MAX_SUBAGENT_DEPTH}"),
        );
    }
    let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
        return tool_result(call, "Error: subagents is not mounted");
    };
    if let Some(src) = req.resume_from.as_deref() {
        match sub.snapshot(src) {
            None => {
                return tool_result(call, format!("Error: resume_from {src} not found"));
            }
            Some(s) if !s.done => {
                return tool_result(call, format!("Error: resume_from {src} is still running"));
            }
            Some(s) if s.subagent_type != req.subagent_type => {
                return tool_result(
                    call,
                    format!(
                        "Error: resume_from type is {} but this call uses {}",
                        s.subagent_type, req.subagent_type
                    ),
                );
            }
            _ => {}
        }
    }
    let (id, slot) = sub.alloc(req.description.clone(), req.subagent_type.clone());
    let parent = sub.ctx.clone();
    let resume = req
        .resume_from
        .as_ref()
        .and_then(|src| {
            sub.inner
                .lock()
                .unwrap()
                .get(src)
                .map(|s| s.events.lock().unwrap().clone())
        })
        .unwrap_or_default();
    let prompt = child_prompt(&req);
    if req.background {
        let id_bg = id.clone();
        let typ = req.subagent_type.clone();
        let desc = req.description.clone();
        tokio::spawn(async move {
            run_child(parent, slot, resume, prompt).await;
        });
        return tool_result(
            call,
            format_subagent_started_background(&id_bg, &typ, &desc),
        );
    }
    run_child(parent, slot.clone(), resume, prompt).await;
    tool_result(call, format_completion(&id, &slot))
}

fn child_prompt(req: &SpawnReq) -> String {
    let role = match req.subagent_type.as_str() {
        "explore" => "You are a read-only explore subagent. Prefer list_dir, read_file, grep, glob, web_search, web_fetch, lsp. Do not write files or run destructive bash.",
        "plan" => "You are a planning subagent. Explore, write a plan, ask clarifying questions. Do not implement unless the prompt says to.",
        _ => "You are a general-purpose subagent. Complete the task and return a concise result.",
    };
    format!(
        "{role}\n\n[{}] {}\n\n{}",
        req.subagent_type, req.description, req.prompt
    )
}

async fn run_child(parent: Context, slot: Arc<Slot>, resume: Vec<LogEvent>, prompt: String) {
    let started = Instant::now();
    let child = parent.isolate("sessions").isolate("turn");
    let sessions = Sessions::isolated(child.clone());
    if !resume.is_empty() {
        sessions.seed(resume);
    }
    let turn = TurnControl::new();
    let mut hold = Vec::new();
    match child.provide(SESSIONS, sessions.clone()) {
        Ok(d) => hold.push(d),
        Err(e) => {
            *slot.output.lock().unwrap() = format!("Error: child sessions: {e}");
            slot.done.store(true, Ordering::Relaxed);
            return;
        }
    }
    match child.provide(TURN, turn) {
        Ok(d) => hold.push(d),
        Err(e) => {
            *slot.output.lock().unwrap() = format!("Error: child turn: {e}");
            slot.done.store(true, Ordering::Relaxed);
            return;
        }
    }
    if let Some(t) = child.get::<TurnControl>(TURN) {
        *slot.turn.lock().unwrap() = Some(t);
    }
    *slot._hold.lock().unwrap() = hold;
    let handle = LoopHandle::new(child, Arc::new(GrokStep));
    let outcome = DEPTH
        .scope(current_depth() + 1, handle.run(prompt))
        .await;
    let text = match outcome {
        Ok(TurnOutcome::Text(t)) => t,
        Err(e) => {
            if slot.cancelled.load(Ordering::Relaxed) || format!("{e}").contains("cancel") {
                slot.cancelled.store(true, Ordering::Relaxed);
                format!("cancelled: {e}")
            } else {
                format!("failed: {e}")
            }
        }
    };
    if let Some(s) = child.get::<Sessions>(SESSIONS) {
        *slot.events.lock().unwrap() = s.events();
    }
    let ms = started.elapsed().as_millis();
    let body = if slot.cancelled.load(Ordering::Relaxed) {
        text
    } else {
        text
    };
    let _ = ms;
    *slot.output.lock().unwrap() = body;
    slot.done.store(true, Ordering::Relaxed);
}

/// Copied from Grok `format_subagent_started_background` (canonical names).
fn format_subagent_started_background(id: &str, typ: &str, description: &str) -> String {
    format!(
        "Subagent started in background.\n\
         subagent_id: {id}\n\
         type: {typ}\n\
         description: {description}\n\n\
         When you need its result, use get_task_output with task_ids=[\"{id}\"] and a positive timeout_ms."
    )
}

fn format_completion(id: &str, slot: &Slot) -> String {
    let output = slot.output.lock().unwrap().clone();
    let typ = &slot.subagent_type;
    format!(
        "{output}\n\n<subagent_meta>id={id}, type={typ}</subagent_meta>\n\
         <subagent_result> resume_from={id} subagent_type={typ} </subagent_result>"
    )
}

pub fn render_subagent(s: &SubagentSnap) -> String {
    let status = if s.cancelled {
        "cancelled"
    } else if s.done {
        "done"
    } else {
        "running"
    };
    format!(
        "[{status}] {} [{}] {}\n{}",
        s.id, s.subagent_type, s.description, s.output
    )
}
