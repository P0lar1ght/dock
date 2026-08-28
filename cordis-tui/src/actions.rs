//! Copied variant names from grok-build/.../xai-grok-pager/src/app/actions.rs
//! Action = sync intent; Effect = async I/O.

use crate::slash::SlashCmd;

/// Synchronous, side-effect-free user intent.
#[derive(Debug)]
pub enum Action {
    Quit,
    NewSession,
    ResumePicker,
    Help,
    RestoreSession(String),
    SlashMove(i16),
    SlashAccept,
    SlashInsert,
    OverlayClose,
    OverlayMove(i16),
    OverlayAccept,
    OverlayChar(char),
    OverlayBackspace,
    OverlayPaste(String),
    OverlaySelect(usize),
    HistoryPicker,
    Find,
    SendPrompt(String),
    SendPromptNow { text: String },
    InsertChar(char),
    InsertText(String),
    Backspace,
    HistoryPrev,
    HistoryNext,
    Scroll(i16),
    ScrollPage(i16),
    Click { column: u16, row: u16 },
}

/// Produced by dispatch, consumed by the event loop.
#[derive(Debug)]
pub enum Effect {
    Quit,
    NewSession,
    ResumePicker,
    Help,
    HistoryPicker,
    Find,
    RestoreSession(String),
    InsertHistory(String),
    JumpToLine(usize),
    SendPrompt { text: String, send_now: bool },
}

pub fn effect_for_slash(cmd: SlashCmd) -> Effect {
    match cmd {
        SlashCmd::New => Effect::NewSession,
        SlashCmd::Resume => Effect::ResumePicker,
        SlashCmd::Help => Effect::Help,
        SlashCmd::History => Effect::HistoryPicker,
        SlashCmd::Find => Effect::Find,
        SlashCmd::Quit => Effect::Quit,
    }
}
