//! Copied from grok-build/.../xai-grok-pager/src/app/dispatch/prompt.rs:
//! "The prompt is always pushed to the queue first."

use super::actions::{effect_for_slash, Action, Effect};
use super::prompt::PromptWidget;
use super::slash;

pub fn dispatch(action: Action, prompt: &PromptWidget) -> Vec<Effect> {
    match action {
        Action::Quit => vec![Effect::Quit],
        Action::NewSession => vec![Effect::NewSession],
        Action::ResumePicker => vec![Effect::ResumePicker],
        Action::Help => vec![Effect::Help],
        Action::RestoreSession(id) => vec![Effect::RestoreSession(id)],
        Action::SlashMove(delta) => {
            prompt.slash_move(delta);
            Vec::new()
        }
        Action::SlashAccept => {
            let snap = prompt.slash_snapshot();
            let Some(row) = snap.current() else {
                return Vec::new();
            };
            prompt.clear();
            vec![effect_for_slash(row.cmd, "")]
        }
        Action::SlashInsert => {
            let snap = prompt.slash_snapshot();
            if let Some(row) = snap.current() {
                prompt.apply_slash_insert(&format!("{} ", row.display));
            }
            Vec::new()
        }
        Action::OverlayClose | Action::OverlayMove(_) | Action::OverlayAccept
        | Action::OverlayChar(_) | Action::OverlayBackspace | Action::OverlayPaste(_)
        |         Action::OverlaySelect(_) | Action::OverlaySpace
        | Action::PermissionAccept | Action::PermissionReject
        | Action::MouseMove { .. } | Action::PasteClipboard | Action::CycleMode => Vec::new(),
        Action::CancelTurn => vec![Effect::CancelTurn],
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
            if let Some((cmd, args)) = slash::command_for_submit(&text) {
                return vec![effect_for_slash(cmd, &args)];
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
