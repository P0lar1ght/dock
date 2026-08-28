//! `ask_user_question` over `"ask"` (DSH `ctx.userQuestions`). Format strings copied from Grok.

mod format;
mod questions;
mod timeout;
mod types;

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use cordis::{plugin, Context, Inject, Plugin};
use indexmap::IndexMap;
use tokio::sync::oneshot;

use crate::names::{ASK, ASK_EVENT, TOOLS};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

pub use format::{unanswered_text, CANCEL_TEXT, NO_OPERATOR_TEXT};
pub use questions::{AskUserQuestionInput, Question, QuestionOption};
pub use types::QuestionAnnotation;

pub struct AskPrompt {
    pub questions: Vec<Question>,
    pub index: usize,
}

struct Pending {
    questions: Vec<Question>,
    answers: IndexMap<String, Vec<String>>,
    annotations: HashMap<String, QuestionAnnotation>,
    index: usize,
    tx: oneshot::Sender<String>,
}

/// Named `"ask"` service. TUI resolves the front of the queue.
pub struct Ask {
    ctx: Context,
    queue: Mutex<VecDeque<Pending>>,
}

impl Ask {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            queue: Mutex::new(VecDeque::new()),
        }
    }

    pub fn front(&self) -> Option<AskPrompt> {
        self.queue.lock().unwrap().front().map(|p| AskPrompt {
            questions: p.questions.clone(),
            index: p.index,
        })
    }

    pub fn cancel(&self) {
        if let Some(pending) = self.queue.lock().unwrap().pop_front() {
            let _ = pending.tx.send(format::CANCEL_TEXT.to_string());
        }
        self.ctx.emit(ASK_EVENT, ());
    }

    pub fn answer_current(&self, labels: Vec<String>, notes: Option<String>) {
        let mut queue = self.queue.lock().unwrap();
        let Some(pending) = queue.front_mut() else {
            return;
        };
        let q = pending.questions[pending.index].clone();
        pending.answers.insert(q.question.clone(), labels);
        if notes.is_some() {
            pending.annotations.insert(
                q.question,
                QuestionAnnotation {
                    preview: None,
                    notes,
                },
            );
        }
        pending.index += 1;
        if pending.index >= pending.questions.len() {
            let pending = queue.pop_front().unwrap();
            let text = format::format_accepted_tool_result(
                &pending.answers,
                &Some(pending.annotations).filter(|m| !m.is_empty()),
            );
            let _ = pending.tx.send(text);
        }
        drop(queue);
        self.ctx.emit(ASK_EVENT, ());
    }

    pub async fn ask(&self, questions: Vec<Question>) -> String {
        if questions.is_empty() {
            return format::CANCEL_TEXT.to_string();
        }
        let (tx, rx) = oneshot::channel();
        self.queue.lock().unwrap().push_back(Pending {
            questions,
            answers: IndexMap::new(),
            annotations: HashMap::new(),
            index: 0,
            tx,
        });
        self.ctx.emit(ASK_EVENT, ());
        let timeout = timeout::response_timeout();
        tokio::select! {
            ok = rx => ok.unwrap_or_else(|_| format::CANCEL_TEXT.to_string()),
            _ = tokio::time::sleep(timeout) => format::unanswered_text(false).to_string(),
        }
    }
}

pub fn tool_ask_user() -> Plugin {
    plugin("tool-ask-user", Inject::from([TOOLS]), |ctx, _: &()| {
        ctx.provide(ASK, Ask::new(ctx.clone()))?;
        let tools = ctx.require::<Tools>(TOOLS)?;
        let body: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { ask_user(&ctx, call).await })
            })
        };
        own_registered(
            ctx,
            vec![tools.register(
                ToolSpec {
                    name: "ask_user_question".into(),
                    description: "Ask the user one or more multiple-choice questions.\n\n- Every question automatically gets an \"Other\" choice where the user can type their own answer.\n- Put your recommended option first and append \"(Recommended)\" to its label.".into(),
                    parameters_json: r#"{"type":"object","properties":{"questions":{"type":"array","items":{"type":"object","properties":{"question":{"type":"string"},"options":{"type":"array","items":{"type":"object","properties":{"label":{"type":"string"},"description":{"type":"string"}},"required":["label"]}},"multi_select":{"type":"boolean"}},"required":["question","options"]}}},"required":["questions"]}"#.into(),
                },
                body,
            )?],
        )?;
        Ok(None)
    })
}

async fn ask_user(ctx: &Context, call: ToolCall) -> ToolResult {
    let input: AskUserQuestionInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: invalid ask_user_question args ({e})")),
    };
    let Some(ask) = ctx.get::<Ask>(ASK) else {
        return tool_result(call, format::NO_OPERATOR_TEXT);
    };
    let text = ask.ask(input.questions).await;
    tool_result(call, text)
}
