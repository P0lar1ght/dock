//! Copied variant names from grok-build/.../xai-grok-pager/src/app/actions.rs
//! Action = sync intent; Effect = async I/O.

use std::path::PathBuf;
use std::time::Duration;

use crate::slash::{self, ArgKind, SlashCmd};
use crate::theme::ThemeKind;

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
    OverlaySpace,
    SettingsModal,
    PermissionAccept,
    PermissionReject,
    MouseMove { column: u16, row: u16 },
    HistoryPicker,
    Find,
    CancelTurn,
    FileSearchMove(i16),
    FileSearchAccept,
    FileSearchDismiss,
    CopyAssistant { n: usize, file: Option<PathBuf> },
    CopyText(String),
    OpenImage(PathBuf),
    SendPrompt(String),
    SendPromptNow { text: String },
    PasteClipboard,
    InsertChar(char),
    InsertText(String),
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveHome,
    MoveEnd,
    MoveBufferStart,
    MoveBufferEnd,
    HistoryPrev,
    HistoryNext,
    Scroll(i16),
    ScrollPage(i16),
    Click { column: u16, row: u16 },
    CycleMode,
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
    CopyAssistant { n: usize, file: Option<PathBuf> },
    CopyText(String),
    OpenImage(PathBuf),
    ArgPicker { kind: ArgKind, cmd: SlashCmd },
    SetTheme(ThemeKind),
    SetModel(String),
    SetEffort(String),
    ToggleTimestamps,
    CronAdd { every: Duration, prompt: String },
    Export(Option<PathBuf>),
    ChangeDir(PathBuf),
    SettingsModal,
    CancelTurn,
    SendPrompt { text: String, send_now: bool },
    EnterPlan { description: Option<String> },
    EnterGoal { objective: Option<String> },
    ShowTasks,
    ShowMcps,
}

pub fn effect_for_slash(cmd: SlashCmd, args: &str) -> Effect {
    let args = args.trim();
    match cmd {
        SlashCmd::New => Effect::NewSession,
        SlashCmd::Resume => Effect::ResumePicker,
        SlashCmd::Help => Effect::Help,
        SlashCmd::History => Effect::HistoryPicker,
        SlashCmd::Find => Effect::Find,
        SlashCmd::Copy => match slash::parse_copy_args(args) {
            Ok((n, file)) => Effect::CopyAssistant { n, file },
            Err(_) => Effect::CopyAssistant { n: 1, file: None },
        },
        SlashCmd::Quit => Effect::Quit,
        SlashCmd::Theme => {
            if args.is_empty() {
                Effect::ArgPicker {
                    kind: ArgKind::Theme,
                    cmd,
                }
            } else if let Some(kind) = ThemeKind::from_name(args) {
                Effect::SetTheme(kind)
            } else {
                Effect::ArgPicker {
                    kind: ArgKind::Theme,
                    cmd,
                }
            }
        }
        SlashCmd::Model => {
            if args.is_empty() {
                Effect::ArgPicker {
                    kind: ArgKind::Model,
                    cmd,
                }
            } else {
                Effect::SetModel(args.to_string())
            }
        }
        SlashCmd::Settings => {
            if args.is_empty() {
                Effect::SettingsModal
            } else {
                apply_settings_arg(args)
            }
        }
        SlashCmd::Loop => {
            if args.is_empty() {
                Effect::ArgPicker {
                    kind: ArgKind::LoopInterval,
                    cmd,
                }
            } else {
                let (tok, prompt) = slash::parse_loop_args(args);
                match (tok.and_then(slash::token_to_duration), prompt) {
                    (Some(every), p) if !p.is_empty() => Effect::CronAdd {
                        every,
                        prompt: p.to_string(),
                    },
                    _ => Effect::ArgPicker {
                        kind: ArgKind::LoopInterval,
                        cmd,
                    },
                }
            }
        }
        SlashCmd::Export => Effect::Export(if args.is_empty() {
            None
        } else {
            Some(PathBuf::from(args))
        }),
        SlashCmd::Cd => Effect::ChangeDir(PathBuf::from(if args.is_empty() { "." } else { args })),
        SlashCmd::Timestamps => Effect::ToggleTimestamps,
        SlashCmd::Effort => {
            if args.is_empty() {
                Effect::ArgPicker {
                    kind: ArgKind::Effort,
                    cmd,
                }
            } else {
                Effect::SetEffort(args.to_string())
            }
        }
        SlashCmd::Plan => Effect::EnterPlan {
            description: if args.is_empty() {
                None
            } else {
                Some(args.to_string())
            },
        },
        SlashCmd::Goal => Effect::EnterGoal {
            objective: if args.is_empty() {
                None
            } else {
                Some(args.to_string())
            },
        },
        SlashCmd::Tasks => Effect::ShowTasks,
        SlashCmd::Mcps => Effect::ShowMcps,
    }
}

fn apply_settings_arg(args: &str) -> Effect {
    let mut parts = args.splitn(2, char::is_whitespace);
    let key = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match key {
        "timestamps" => Effect::ToggleTimestamps,
        "theme" => effect_for_slash(SlashCmd::Theme, rest),
        "model" => effect_for_slash(SlashCmd::Model, rest),
        _ => Effect::ArgPicker {
            kind: ArgKind::Settings,
            cmd: SlashCmd::Settings,
        },
    }
}
