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
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

pub use questions::{AskUserQuestionInput, Question, QuestionOption};
pub use types::QuestionAnnotation;

pub struct AskPrompt {
    pub questions: Vec<Question>,
    pub index: usize,
    /// Wire labels already saved for the question at [`index`], if any.
    pub current_labels: Vec<String>,
    /// Freeform notes saved with an "Other" answer for the current question.
    pub current_notes: Option<String>,
    /// Highest question index the user may navigate to (answered prefix + frontier).
    pub max_index: usize,
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
        self.queue.lock().unwrap().front().map(|p| {
            let q = p.questions.get(p.index);
            let current_labels = q
                .and_then(|q| p.answers.get(&q.question).cloned())
                .unwrap_or_default();
            let current_notes =
                q.and_then(|q| p.annotations.get(&q.question).and_then(|a| a.notes.clone()));
            AskPrompt {
                questions: p.questions.clone(),
                index: p.index,
                current_labels,
                current_notes,
                max_index: max_reachable(p),
            }
        })
    }

    pub fn cancel(&self) {
        if let Some(pending) = self.queue.lock().unwrap().pop_front() {
            let _ = pending.tx.send(format::CANCEL_TEXT.to_string());
        }
        self.ctx.emit(ASK_EVENT, ());
    }

    /// Move between questions without submitting. Returns `true` if the index changed.
    pub fn navigate(&self, delta: i32) -> bool {
        let mut queue = self.queue.lock().unwrap();
        let Some(pending) = queue.front_mut() else {
            return false;
        };
        if pending.questions.len() <= 1 {
            return false;
        }
        let max = max_reachable(pending) as i32;
        let next = (pending.index as i32 + delta).clamp(0, max) as usize;
        if next == pending.index {
            return false;
        }
        pending.index = next;
        drop(queue);
        self.ctx.emit(ASK_EVENT, ());
        true
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
                q.question.clone(),
                QuestionAnnotation {
                    preview: None,
                    notes,
                },
            );
        } else {
            pending.annotations.remove(&q.question);
        }
        // Advance to the next unanswered question when possible; otherwise step
        // forward one so Left can still revisit earlier answers.
        if let Some(next) = first_unanswered(pending) {
            pending.index = next;
        } else {
            pending.index = pending.questions.len();
        }
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

    /// Dual-resolve from the web gateway. Matches questions by id, text, or
    /// projector `question_{n}` ids. Empty queue is an error so the second
    /// resolver cannot silently succeed.
    pub fn respond(&self, answers: &[(String, Vec<String>, Option<String>)]) -> Result<(), String> {
        let mut queue = self.queue.lock().unwrap();
        let Some(pending) = queue.front_mut() else {
            return Err("no pending question".into());
        };
        if answers.is_empty() {
            return Err("answers are required".into());
        }
        for (key, labels, notes) in answers {
            let matched = pending
                .questions
                .iter()
                .enumerate()
                .find(|(i, q)| {
                    q.id.as_deref() == Some(key.as_str())
                        || q.question == *key
                        || *key == format!("question_{}", i + 1)
                })
                .map(|(_, q)| q.clone());
            let Some(q) = matched else {
                continue;
            };
            pending.answers.insert(q.question.clone(), labels.clone());
            if notes.is_some() {
                pending.annotations.insert(
                    q.question.clone(),
                    QuestionAnnotation {
                        preview: None,
                        notes: notes.clone(),
                    },
                );
            } else {
                pending.annotations.remove(&q.question);
            }
        }
        if let Some(next) = first_unanswered(pending) {
            pending.index = next;
        } else {
            pending.index = pending.questions.len();
        }
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
        Ok(())
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

/// Highest index the user may open: end of the answered prefix, or the first
/// unanswered slot (so they can keep filling after going back).
fn max_reachable(pending: &Pending) -> usize {
    let n = pending.questions.len();
    if n == 0 {
        return 0;
    }
    match first_unanswered(pending) {
        Some(i) => i,
        None => n.saturating_sub(1),
    }
}

fn first_unanswered(pending: &Pending) -> Option<usize> {
    pending
        .questions
        .iter()
        .position(|q| !pending.answers.contains_key(&q.question))
}

pub fn tool_ask_user() -> Plugin {
    plugin("tool-ask-user", Inject::from([TOOLS]), |ctx, _: &()| {
        ctx.provide(ASK, Ask::new(ctx.clone()))?;
        let tools = ctx.require::<Tools>(TOOLS)?;
        let body: ToolBody =
            std::sync::Arc::new(|call| Box::pin(async move { ask_user(call).await }));
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

async fn ask_user(call: ToolCall) -> ToolResult {
    let input: AskUserQuestionInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: invalid ask_user_question args ({e})")),
    };
    // 工具体注册在根上。提问队列按页隔离，必须问执行期那一页，不能问捕获的根。
    let Some(ctx) = crate::tools::registry::exec_ctx() else {
        return tool_result(call, format::NO_OPERATOR_TEXT);
    };
    let Some(ask) = ctx.get::<Ask>(ASK) else {
        return tool_result(call, format::NO_OPERATOR_TEXT);
    };
    let text = ask.ask(input.questions).await;
    tool_result(call, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str, labels: &[&str]) -> Question {
        Question {
            question: text.into(),
            options: labels
                .iter()
                .map(|l| QuestionOption {
                    label: (*l).into(),
                    description: String::new(),
                    preview: None,
                    id: None,
                })
                .collect(),
            multi_select: None,
            id: None,
        }
    }

    #[test]
    fn navigate_left_revisits_answered_question() {
        let ask = Ask::new(cordis::Context::new());
        let (tx, _rx) = oneshot::channel();
        ask.queue.lock().unwrap().push_back(Pending {
            questions: vec![q("Q1", &["A", "B"]), q("Q2", &["C", "D"]), q("Q3", &["E"])],
            answers: IndexMap::new(),
            annotations: HashMap::new(),
            index: 0,
            tx,
        });
        ask.answer_current(vec!["A".into()], None);
        assert_eq!(ask.front().unwrap().index, 1);
        assert!(ask.navigate(-1));
        let front = ask.front().unwrap();
        assert_eq!(front.index, 0);
        assert_eq!(front.current_labels, vec!["A".to_string()]);
        assert!(!ask.navigate(-1));
        assert!(ask.navigate(1));
        assert_eq!(ask.front().unwrap().index, 1);
    }

    #[test]
    fn reanswer_overwrites_and_jumps_to_first_unanswered() {
        let ask = Ask::new(cordis::Context::new());
        let (tx, _rx) = oneshot::channel();
        ask.queue.lock().unwrap().push_back(Pending {
            questions: vec![q("Q1", &["A"]), q("Q2", &["B"]), q("Q3", &["C"])],
            answers: IndexMap::new(),
            annotations: HashMap::new(),
            index: 0,
            tx,
        });
        ask.answer_current(vec!["A".into()], None);
        ask.answer_current(vec!["B".into()], None);
        assert_eq!(ask.front().unwrap().index, 2);
        assert!(ask.navigate(-2));
        ask.answer_current(vec!["A2".into()], None);
        let front = ask.front().unwrap();
        assert_eq!(front.index, 2);
        assert_eq!(
            ask.queue.lock().unwrap().front().unwrap().answers.get("Q1"),
            Some(&vec!["A2".to_string()])
        );
    }

    #[test]
    fn respond_matches_id_or_text_and_errors_when_empty() {
        let ask = Ask::new(cordis::Context::new());
        assert!(ask
            .respond(&[("Q1".into(), vec!["A".into()], None)])
            .is_err());
        let (tx, mut rx) = oneshot::channel();
        ask.queue.lock().unwrap().push_back(Pending {
            questions: vec![q("Q1", &["A"])],
            answers: IndexMap::new(),
            annotations: HashMap::new(),
            index: 0,
            tx,
        });
        ask.respond(&[("question_1".into(), vec!["A".into()], None)])
            .unwrap();
        assert!(ask.front().is_none());
        assert!(rx.try_recv().is_ok());
    }
}
