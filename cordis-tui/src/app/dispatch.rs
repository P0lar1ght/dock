//! Copied from grok-build/.../xai-grok-pager/src/app/dispatch/prompt.rs:
//! "The prompt is always pushed to the queue first."

use super::actions::{effect_for_pick, Action, Effect};
use crate::slash;
use crate::views::prompt::PromptWidget;

pub fn dispatch(action: Action, prompt: &PromptWidget) -> Vec<Effect> {
    match action {
        Action::Quit => vec![Effect::Quit],
        Action::NewSession => vec![Effect::NewSession],
        Action::TabNew => vec![Effect::TabNew],
        Action::TabGo(id) => vec![Effect::TabGo { id }],
        Action::TabFork => vec![Effect::TabFork],
        Action::TabCarryBack => vec![Effect::TabCarryBack],
        Action::ResumePicker => vec![Effect::ResumePicker],
        Action::Help => vec![Effect::Help],
        Action::RestoreSession(id) => vec![Effect::RestoreSession(id)],
        Action::SlashMove(delta) => {
            prompt.slash_move(delta);
            Vec::new()
        }
        Action::SlashAccept | Action::SlashInsert => {
            let snap = prompt.slash_snapshot();
            if let Some(row) = snap.current() {
                prompt.apply_slash_insert(&format!("{} ", row.display));
            }
            Vec::new()
        }
        Action::OverlayClose
        | Action::OverlayMove(_)
        | Action::OverlayAccept
        | Action::OverlayChar(_)
        | Action::OverlayBackspace
        | Action::OverlayPaste(_)
        | Action::OverlaySelect(_)
        | Action::OverlaySpace
        | Action::OverlayTab
        | Action::OverlayRefresh
        | Action::OverlayNavH(_)
        | Action::PermissionAccept
        | Action::PermissionReject
        | Action::MouseMove { .. }
        | Action::MouseDown { .. }
        | Action::MouseDrag { .. }
        | Action::MouseUp { .. }
        | Action::PasteClipboard
        | Action::CycleMode
        | Action::ToggleGoalDetail
        | Action::ToggleTodoFold => Vec::new(),
        Action::CancelTurn => vec![Effect::CancelTurn],
        Action::UndoLastSend { announce_failure } => {
            vec![Effect::UndoLastSend { announce_failure }]
        }
        Action::PromoteQueued { id } => vec![Effect::PromoteQueued { id }],
        Action::EditQueued { id } => vec![Effect::EditQueued { id }],
        Action::SettingsModal => vec![Effect::SettingsModal],
        Action::HistoryPicker => vec![Effect::HistoryPicker],
        Action::Find => vec![Effect::Find],
        Action::FileSearchMove(delta) => {
            prompt.file_search_move(delta);
            Vec::new()
        }
        Action::FileSearchAccept => {
            prompt.accept_file_search();
            Vec::new()
        }
        Action::FileSearchDismiss => {
            prompt.file_search_dismiss();
            Vec::new()
        }
        Action::CopyAssistant { n, file } => vec![Effect::CopyAssistant { n, file }],
        Action::CopyText(text) => vec![Effect::CopyText(text)],
        Action::OpenImage(path) => vec![Effect::OpenImage(path)],
        Action::SendPrompt(text) => {
            let text = text.trim().to_string();
            prompt.clear();
            if text.is_empty() {
                return Vec::new();
            }
            let extras = prompt.slash_extras();
            if let Some((pick, args)) = slash::command_for_submit_ex(&text, &extras) {
                return vec![effect_for_pick(pick, &args, &extras)];
            }
            vec![Effect::SendPrompt {
                text,
                send_now: false,
            }]
        }
        Action::SendPromptNow { text } => {
            let text = text.trim().to_string();
            prompt.clear();
            if text.is_empty() {
                return Vec::new();
            }
            vec![Effect::SendPrompt {
                text,
                send_now: true,
            }]
        }
        Action::InsertChar(c) => {
            prompt.push(c);
            Vec::new()
        }
        Action::InsertText(s) => {
            prompt.handle_paste(&s);
            Vec::new()
        }
        Action::Backspace => {
            prompt.backspace();
            Vec::new()
        }
        Action::Delete => {
            prompt.delete();
            Vec::new()
        }
        Action::MoveLeft => {
            prompt.move_left();
            Vec::new()
        }
        Action::MoveRight => {
            prompt.move_right();
            Vec::new()
        }
        Action::MoveHome => {
            prompt.move_home();
            Vec::new()
        }
        Action::MoveEnd => {
            prompt.move_end();
            Vec::new()
        }
        Action::MoveBufferStart => {
            prompt.move_buffer_start();
            Vec::new()
        }
        Action::MoveBufferEnd => {
            prompt.move_buffer_end();
            Vec::new()
        }
        Action::HistoryPrev => {
            prompt.history_prev();
            Vec::new()
        }
        Action::HistoryNext => {
            prompt.history_next();
            Vec::new()
        }
        Action::Scroll(_) | Action::ScrollPage(_) | Action::Click { .. } => Vec::new(),
    }
}
