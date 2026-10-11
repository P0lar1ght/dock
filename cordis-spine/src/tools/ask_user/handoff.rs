//! `handoff_to_user`：请用户在 Dock 的浏览器面板里接手一步（登录、验证码、二次验证、
//! 只该本人做的确认），做完再继续。参考 Codex 浏览器的 `requestManualHandoff`。
//!
//! 就是一道只有两个选项的提问，走 `"ask"` 队列（和 `ask_user_question` 同一条路，按页
//! 隔离、TUI / GUI 都能答）。问题 id 是 [`HANDOFF_QUESTION_ID`]：网关据此在
//! `interaction/requested` 上带 `browserHandoff: true`，GUI 看到就把这个会话的浏览器
//! 面板打开，人在面板里直接操作同一个标签页。

use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

use super::{Ask, Question, QuestionOption};
use crate::names::ASK;
use crate::tools::registry::tool_result;

/// 接手请求那道题的 id（网关和客户端靠它认出这是浏览器接手）。
pub const HANDOFF_QUESTION_ID: &str = "browser_handoff";
const DONE: &str = "已完成";
const NO_OPERATOR_TEXT: &str = "没有用户可以接手（非交互会话）：不要自己尝试登录或绕过验证，在回复里说明这一步需要用户本人在浏览器里完成。";
const CANNOT: &str = "做不到";

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "handoff_to_user".into(),
        description: "Ask the user to take over the browser for a step only they should or can do (log in, solve a CAPTCHA, 2FA, confirm a payment), then wait until they say it is done. Dock opens its browser panel on this session's tab so the user acts on the same page. Call it after the page that needs the user is open in this session (browser_open / browser_js). Then check the page (snapshot) before continuing. Do not use it for questions you can answer yourself.".into(),
        parameters_json: r#"{"type":"object","properties":{"reason":{"type":"string","description":"What the user should do, in the user's language, one short sentence (e.g. 「登录淘宝账号」)."}},"required":["reason"]}"#.into(),
    }
}

/// 给用户看的那道题。
fn question(reason: &str) -> Question {
    Question {
        question: format!("需要你在浏览器面板里接手：{reason}\n做完选「{DONE}」，我接着做。"),
        options: vec![
            QuestionOption {
                label: DONE.into(),
                description: "我已经在浏览器里做完了".into(),
                preview: None,
                id: None,
            },
            QuestionOption {
                label: CANNOT.into(),
                description: "跳过这一步".into(),
                preview: None,
                id: None,
            },
        ],
        multi_select: Some(false),
        id: Some(HANDOFF_QUESTION_ID.into()),
    }
}

/// 回给模型的话：做完了就先确认页面再继续；没做完别再试这一步。
///
/// 回话是 `"题面"="选项"`（后面可能跟用户备注）。按整对「题面 = 已完成」判断：理由是模型写的、
/// 嵌在题面里，备注是用户写的，都可能含「已完成」三个字。
fn outcome(reason: &str, answer: &str) -> String {
    let done = format!("\"{}\"=\"{DONE}\"", question(reason).question);
    if answer.contains(&done) {
        format!(
            "用户说已经在浏览器面板里完成：{reason}。先看一眼页面（snapshot）确认状态，再继续。"
        )
    } else {
        format!(
            "用户没有完成：{reason}。不要再自己试这一步，告诉用户卡在哪里、还需要什么。\n（用户的回答：{answer}）"
        )
    }
}

pub async fn run(call: ToolCall) -> ToolResult {
    let reason = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .ok()
        .and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(str::to_string))
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    let Some(reason) = reason else {
        return tool_result(call, "Error: reason is required");
    };
    // 和 ask_user_question 一样：提问队列按页隔离，问执行期那一页。
    // 没有人能接手（非交互会话）：不能沿用提问那句「按你的判断继续」——登录、验证码这类
    // 场景下那等于叫模型自己去硬闯。
    let Some(ask) = crate::tools::registry::exec_ctx().and_then(|c| c.get::<Ask>(ASK)) else {
        return tool_result(call, NO_OPERATOR_TEXT);
    };
    let answer = ask.ask(vec![question(&reason)]).await;
    tool_result(call, outcome(&reason, &answer))
}

#[cfg(test)]
mod tests {
    use super::super::format;
    use super::*;
    use indexmap::IndexMap;

    fn answered(label: &str) -> String {
        let q = question("登录");
        let mut answers = IndexMap::new();
        answers.insert(q.question.clone(), vec![label.to_string()]);
        format::format_accepted_tool_result(&answers, &None)
    }

    #[test]
    fn done_and_cannot_read_back_from_the_formatted_answer() {
        let done = outcome("登录", &answered(DONE));
        assert!(done.contains("已经在浏览器面板里完成"), "{done}");
        let cannot = outcome("登录", &answered(CANNOT));
        assert!(cannot.contains("没有完成"), "{cannot}");
        let cancelled = outcome("登录", format::CANCEL_TEXT);
        assert!(cancelled.contains("没有完成"), "{cancelled}");
    }

    /// 审阅指出：以前在整段回话里找 `"已完成"`。理由是模型写的、会嵌进题面，用户备注也会拼
    /// 进去——里面带着 `"已完成"` 时，选了「做不到」也会读成完成。
    #[test]
    fn done_is_read_from_the_choice_not_from_the_reason_or_notes() {
        let reason = "登录后页面出现\"已完成\"字样";
        let q = question(reason);
        let mut answers = IndexMap::new();
        answers.insert(q.question.clone(), vec![CANNOT.to_string()]);
        let mut notes = std::collections::HashMap::new();
        notes.insert(
            q.question.clone(),
            super::super::QuestionAnnotation {
                preview: None,
                notes: Some("我看到\"已完成\"但其实没登上".into()),
            },
        );
        let answer = format::format_accepted_tool_result(&answers, &Some(notes));
        let read = outcome(reason, &answer);
        assert!(read.contains("没有完成"), "{read}");
    }

    #[test]
    fn the_question_is_marked_for_the_browser_panel() {
        let q = question("登录淘宝");
        assert_eq!(q.id.as_deref(), Some(HANDOFF_QUESTION_ID));
        assert!(q.question.contains("登录淘宝"));
        assert_eq!(q.options.len(), 2);
    }
}
