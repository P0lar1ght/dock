//! 侧边聊天（只读旁问页）写进主线的那段笔记：让旁问页自己整理一段限长的结论。
//!
//! 只起草，不写入：起草出来的笔记交给用户看（GUI 预览 / TUI 确认），用户点了
//! 「写入」再经来源页的 [`crate::SessionRef::merge_side_note`] 落进主线。

use cordis::Context;
use cordis_base::types::{LogEvent, PromptRequest, SIDE_NOTE_MAX_CHARS};

use crate::llm::sampler::Llm;
use crate::names::{LLM, SESSIONS, SYSTEM_PROMPT, TOOLS};
use crate::prompt::assemble::SystemPrompt;
use crate::session::log::Sessions;
use crate::tools::registry::Tools;

/// 起草时追加在旁问历史末尾的那条要求（只进这一次请求，不进旁问页的日志）。
fn draft_instruction() -> String {
    format!(
        "<system-reminder>\n用户要把这段侧边聊天的结论写进主线（主会话）。请写一段给主线看的笔记：\n\
         - 只写这段旁问里得出的结论和依据（文件路径、行号、命令输出的要点）；\n\
         - 不要复述主线已经知道的内容，不要寒暄，不要提「侧边聊天」本身；\n\
         - 不要调用工具，直接写；\n\
         - 用中文，{SIDE_NOTE_MAX_CHARS} 字以内，可以用 markdown 列表。\n</system-reminder>"
    )
}

/// 让旁问页 `aside` 整理一段写进主线的笔记（≤ [`SIDE_NOTE_MAX_CHARS`] 字）。
///
/// 这次采样不进任何会话（隔离掉 `"sessions"`，同压缩、`/remember` 改写），旁问页
/// 的对话不多出一条。系统提示和工具表照旁问页平时的发，前缀缓存接得上；只靠
/// 那条要求让模型别调工具，调了也只取正文。
pub async fn draft_side_note(aside: &Context) -> Result<String, String> {
    let sessions = aside
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| "侧边聊天没有会话".to_string())?;
    let llm = aside
        .get::<Llm>(LLM)
        .ok_or_else(|| "模型服务没有挂载".to_string())?;
    let system = aside
        .get::<SystemPrompt>(SYSTEM_PROMPT)
        .map(|p| p.assemble_on(aside))
        .unwrap_or_default();
    let tools = aside
        .get::<Tools>(TOOLS)
        .map(|t| t.specs_for_model_on(aside))
        .unwrap_or_default();
    let mut history = sessions.model_history();
    history.push(LogEvent::User(draft_instruction()));
    let iso = aside.isolate("sessions");
    let output = llm
        .stream_observed(
            &iso,
            PromptRequest {
                system,
                history,
                tools,
            },
            |_| {},
        )
        .await;
    if let Some(error) = output.error {
        return Err(format!("模型请求失败：{error}"));
    }
    let text = output.text.trim();
    if text.is_empty() {
        return Err("模型没有写出结论".into());
    }
    Ok(clip_note(text))
}

/// 截到 [`SIDE_NOTE_MAX_CHARS`] 个字符，截了就补一个省略号。
pub(crate) fn clip_note(text: &str) -> String {
    if text.chars().count() <= SIDE_NOTE_MAX_CHARS {
        return text.to_string();
    }
    let mut out: String = text.chars().take(SIDE_NOTE_MAX_CHARS - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_note_is_clipped_to_the_cap() {
        let long = "字".repeat(SIDE_NOTE_MAX_CHARS + 10);
        let clipped = clip_note(&long);
        assert_eq!(clipped.chars().count(), SIDE_NOTE_MAX_CHARS);
        assert!(clipped.ends_with('…'));
        assert_eq!(clip_note("短"), "短");
    }
}
