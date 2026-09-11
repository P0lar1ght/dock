//! Copied variant names from grok-build/.../xai-grok-pager/src/app/actions.rs
//! Action = sync intent; Effect = async I/O.

use std::path::PathBuf;

use crate::slash::{self, ArgKind, SlashCmd, SlashPick};
use crate::theme::ThemeKind;
use cordis_spine::{
    extra_tool_slash_arguments, goal_composer_fill, loop_composer_fill, loop_usage_message,
    lsp_composer_fill, workflow_command_arguments, ExtraSlashKind, SlashEntry,
    GOAL_RESERVED_SUBCOMMANDS, WORKFLOW_TOOL_NAME,
};

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
    OverlayTab,
    /// Horizontal nav inside an overlay (Ask multi-question ←/→).
    OverlayNavH(i16),
    SettingsModal,
    PermissionAccept,
    PermissionReject,
    MouseMove {
        column: u16,
        row: u16,
    },
    MouseDown {
        column: u16,
        row: u16,
    },
    MouseDrag {
        column: u16,
        row: u16,
    },
    MouseUp {
        column: u16,
        row: u16,
    },
    HistoryPicker,
    Find,
    CancelTurn,
    FileSearchMove(i16),
    FileSearchAccept,
    FileSearchDismiss,
    CopyAssistant {
        n: usize,
        file: Option<PathBuf>,
    },
    CopyText(String),
    OpenImage(PathBuf),
    SendPrompt(String),
    SendPromptNow {
        text: String,
    },
    PromoteQueued {
        id: Option<String>,
    },
    EditQueued {
        id: Option<String>,
    },
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
    Click {
        column: u16,
        row: u16,
    },
    CycleMode,
    ToggleGoalDetail,
    /// Cycle the scrollback todo card fold (Ctrl+t).
    ToggleTodoFold,
}

/// Produced by dispatch, consumed by the event loop.
#[derive(Debug)]
pub enum Effect {
    Quit,
    NewSession,
    ResumePicker,
    PairingManage,
    Help,
    HistoryPicker,
    Find,
    RestoreSession(String),
    InsertHistory(String),
    JumpToLine(usize),
    CopyAssistant {
        n: usize,
        file: Option<PathBuf>,
    },
    CopyText(String),
    OpenImage(PathBuf),
    ArgPicker {
        kind: ArgKind,
        cmd: SlashCmd,
    },
    SetTheme(ThemeKind),
    SetModel(String),
    SetEffort(String),
    ToggleTimestamps,
    ToggleThinking,
    Export(Option<PathBuf>),
    ChangeDir(PathBuf),
    SettingsModal,
    CancelTurn,
    SendPrompt {
        text: String,
        send_now: bool,
    },
    PromoteQueued {
        id: Option<String>,
    },
    EditQueued {
        id: Option<String>,
    },
    FillPrompt {
        text: String,
    },
    EnterPlan {
        description: Option<String>,
    },
    ViewPlan,
    EnterGoal {
        objective: Option<String>,
    },
    EnterLoop {
        args: String,
    },
    ShowGoal {
        editing: bool,
    },
    GoalPause,
    GoalResume,
    GoalClear,
    ShowTasks,
    ToggleWorkflows,
    ShowMcps,
    /// `/lsp`：空/`setup` 写项目配置，`user` 写用户配置，`status` 只看不写。
    ShowLsp {
        write: bool,
        user: bool,
    },
    ShowCordis,
    /// Live `/browser` cockpit overlay (status / tabs / screenshot / 审批).
    ShowBrowser,
    /// Live `/computer` thin cockpit (cua-driver MCP status / 审批). No embedded desktop.
    ShowComputer,
    ShowPresets {
        focus: Option<String>,
    },
    ShowUsage,
    ShowContext,
    Compact {
        context: String,
    },
    ShowNotice {
        title: String,
        body: String,
    },
    OpenSlot {
        id: String,
    },
    /// Async: TUI spawns `Tools::execute` then queues a Notice on `"slash"`.
    RunTool {
        name: String,
        arguments: String,
        title: String,
    },
    /// Async: persist + connect/disconnect an MCP server (`Space` on `/mcps` server row).
    ToggleMcpServer {
        name: String,
        enabled: bool,
    },
    /// Async: persist + register/unregister one MCP tool (`Space` on `/mcps` tool row).
    ToggleMcpTool {
        server: String,
        tool: String,
        enabled: bool,
    },
    /// Async: browser PKCE for an HTTP MCP server (`i` on `/mcps`).
    McpAuth {
        name: String,
    },
    SetPrompt {
        text: String,
    },
}

pub fn effect_for_slash(cmd: SlashCmd, args: &str) -> Effect {
    let args = args.trim();
    match cmd {
        SlashCmd::New => Effect::NewSession,
        SlashCmd::Resume => Effect::ResumePicker,
        SlashCmd::Pair => Effect::PairingManage,
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
                Effect::FillPrompt {
                    text: loop_composer_fill(),
                }
            } else {
                Effect::EnterLoop {
                    args: args.to_string(),
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
        SlashCmd::Thinking => Effect::ToggleThinking,
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
        SlashCmd::ViewPlan => Effect::ViewPlan,
        SlashCmd::Goal => goal_effect(args),
        SlashCmd::Tasks => Effect::ShowTasks,
        SlashCmd::Workflow => {
            if args.is_empty() || args.eq_ignore_ascii_case("runs") {
                Effect::ToggleWorkflows
            } else {
                let first = args.split_whitespace().next().unwrap_or("");
                if matches!(first, "pause" | "resume" | "stop" | "save") {
                    Effect::ToggleWorkflows
                } else {
                    match workflow_command_arguments(args) {
                        Ok((name, arguments)) => Effect::RunTool {
                            name: WORKFLOW_TOOL_NAME.into(),
                            arguments,
                            title: format!("/{name}"),
                        },
                        Err(body) => Effect::ShowNotice {
                            title: "工作流".into(),
                            body,
                        },
                    }
                }
            }
        }
        SlashCmd::Mcps => Effect::ShowMcps,
        SlashCmd::Lsp => lsp_effect(args),
        SlashCmd::Cordis => Effect::ShowCordis,
        SlashCmd::Preset => {
            let focus = if args.is_empty() {
                None
            } else {
                Some(args.to_string())
            };
            Effect::ShowPresets { focus }
        }
        SlashCmd::Usage => Effect::ShowUsage,
        SlashCmd::Context => Effect::ShowContext,
        SlashCmd::Compact => Effect::Compact {
            context: args.to_string(),
        },
    }
}

pub fn effect_for_pick(pick: SlashPick, args: &str, extras: &[SlashEntry]) -> Effect {
    match pick {
        SlashPick::Builtin(cmd) => effect_for_slash(cmd, args),
        SlashPick::Extra(name) => extras
            .iter()
            .find(|e| e.command == name)
            .map(|e| effect_for_extra(e, args))
            .unwrap_or_else(|| {
                let args = args.trim();
                Effect::SetPrompt {
                    text: if args.is_empty() {
                        format!("/{name}")
                    } else {
                        format!("/{name} {args}")
                    },
                }
            }),
    }
}

pub fn effect_for_extra(entry: &SlashEntry, args: &str) -> Effect {
    let text = entry.expand(args);
    match entry.kind {
        ExtraSlashKind::Prompt => {
            if entry.send {
                Effect::SendPrompt {
                    text,
                    send_now: true,
                }
            } else {
                Effect::SetPrompt { text }
            }
        }
        ExtraSlashKind::Overlay if entry.command == "browser" => Effect::ShowBrowser,
        ExtraSlashKind::Overlay if entry.command == "computer" => Effect::ShowComputer,
        ExtraSlashKind::Overlay => Effect::ShowNotice {
            title: entry.overlay_title(),
            body: text,
        },
        ExtraSlashKind::Slot => Effect::OpenSlot { id: text },
        ExtraSlashKind::Tool => match extra_tool_slash_arguments(entry, args) {
            Ok(arguments) => Effect::RunTool {
                name: entry.text.trim().to_string(),
                arguments,
                title: entry.tool_title(),
            },
            Err(body) => Effect::ShowNotice {
                title: entry.tool_title(),
                body,
            },
        },
    }
}

fn apply_settings_arg(args: &str) -> Effect {
    let mut parts = args.splitn(2, char::is_whitespace);
    let key = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match key {
        "timestamps" => Effect::ToggleTimestamps,
        "think" | "thinking" => Effect::ToggleThinking,
        "theme" => effect_for_slash(SlashCmd::Theme, rest),
        "model" => effect_for_slash(SlashCmd::Model, rest),
        "effort" => effect_for_slash(SlashCmd::Effort, rest),
        _ => Effect::ArgPicker {
            kind: ArgKind::Settings,
            cmd: SlashCmd::Settings,
        },
    }
}

fn lsp_effect(args: &str) -> Effect {
    match args.trim() {
        "" | "setup" | "auto" => Effect::ShowLsp {
            write: true,
            user: false,
        },
        "user" => Effect::ShowLsp {
            write: true,
            user: true,
        },
        "status" => Effect::ShowLsp {
            write: false,
            user: false,
        },
        _ => Effect::FillPrompt {
            text: lsp_composer_fill(),
        },
    }
}

fn goal_effect(args: &str) -> Effect {
    let args = args.trim();
    if args.is_empty() {
        return Effect::FillPrompt {
            text: goal_composer_fill(),
        };
    }
    let first = args.split_whitespace().next().unwrap_or("");
    match first {
        "status" => Effect::ShowGoal { editing: false },
        "edit" => Effect::ShowGoal { editing: true },
        "pause" => Effect::GoalPause,
        "resume" => Effect::GoalResume,
        "clear" => Effect::GoalClear,
        _ => Effect::EnterGoal {
            objective: Some(args.to_string()),
        },
    }
}

#[derive(Debug)]
pub enum GoalComposer {
    Stub,
    Objective(String),
    Slash(String),
}

/// After bare `/goal`, the composer holds usage + `/goal `. Sending that
/// blob (or a replacement) becomes the objective.
#[cfg(test)]
pub fn interpret_goal_composer(text: &str) -> GoalComposer {
    interpret_goal_composer_ex(text, &[])
}

pub fn interpret_goal_composer_ex(text: &str, extras: &[SlashEntry]) -> GoalComposer {
    let usage = cordis_spine::goal_usage_message();
    let mut body = text.trim();
    if let Some(rest) = body.strip_prefix(usage) {
        body = rest.trim();
    }
    if body.is_empty() {
        return GoalComposer::Stub;
    }
    let line = body.lines().last().unwrap_or(body).trim();
    if let Some((pick, args)) = slash::command_for_submit_ex(line, extras) {
        if matches!(pick, SlashPick::Builtin(SlashCmd::Goal)) {
            let args = args.trim();
            if args.is_empty() || args == "<目标>" {
                return GoalComposer::Stub;
            }
            let first = args.split_whitespace().next().unwrap_or("");
            if GOAL_RESERVED_SUBCOMMANDS.contains(&first) {
                return GoalComposer::Slash(line.to_string());
            }
            return GoalComposer::Objective(args.to_string());
        }
        return GoalComposer::Slash(line.to_string());
    }
    if let Some(rest) = body.strip_prefix("/goal") {
        let rest = rest.trim();
        if rest.is_empty() || rest == "<目标>" {
            return GoalComposer::Stub;
        }
        return GoalComposer::Objective(rest.to_string());
    }
    GoalComposer::Objective(body.to_string())
}

#[derive(Debug)]
pub enum LoopComposer {
    Stub,
    Schedule(String),
    Other,
}

/// After bare `/loop`, the composer holds usage + `/loop `. Sending that
/// blob (or a filled last line) becomes `EnterLoop`.
pub fn interpret_loop_composer(text: &str) -> LoopComposer {
    let usage = loop_usage_message();
    let mut body = text.trim();
    if let Some(rest) = body.strip_prefix(usage) {
        body = rest.trim();
    } else if !body.contains("/loop") {
        return LoopComposer::Other;
    }
    if body.is_empty() {
        return LoopComposer::Stub;
    }
    let line = body.lines().last().unwrap_or(body).trim();
    if let Some((pick, args)) = slash::command_for_submit_ex(line, &[]) {
        if matches!(pick, SlashPick::Builtin(SlashCmd::Loop)) {
            let args = args.trim();
            if args.is_empty() {
                return LoopComposer::Stub;
            }
            return LoopComposer::Schedule(args.to_string());
        }
        return LoopComposer::Other;
    }
    if let Some(rest) = line.strip_prefix("/loop") {
        let rest = rest.trim();
        if rest.is_empty() {
            return LoopComposer::Stub;
        }
        return LoopComposer::Schedule(rest.to_string());
    }
    if body.contains(usage) || text.contains(usage) {
        LoopComposer::Stub
    } else {
        LoopComposer::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpret_stub_and_objective() {
        assert!(matches!(
            interpret_goal_composer(&goal_composer_fill()),
            GoalComposer::Stub
        ));
        assert!(matches!(
            interpret_goal_composer("/goal"),
            GoalComposer::Stub
        ));
        assert!(matches!(
            interpret_goal_composer("/goal <目标>"),
            GoalComposer::Stub
        ));
        match interpret_goal_composer("/goal ship the lsp") {
            GoalComposer::Objective(s) => assert_eq!(s, "ship the lsp"),
            other => panic!("{other:?}"),
        }
        match interpret_goal_composer("实现 lsp") {
            GoalComposer::Objective(s) => assert_eq!(s, "实现 lsp"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            interpret_goal_composer("/goal status"),
            GoalComposer::Slash(_)
        ));
        assert!(matches!(
            interpret_goal_composer("/plan"),
            GoalComposer::Slash(_)
        ));
    }

    #[test]
    fn interpret_loop_stub_and_schedule() {
        assert!(matches!(
            interpret_loop_composer(&loop_composer_fill()),
            LoopComposer::Stub
        ));
        assert!(matches!(
            interpret_loop_composer("/loop"),
            LoopComposer::Stub
        ));
        match interpret_loop_composer("/loop 5m check deploy") {
            LoopComposer::Schedule(s) => assert_eq!(s, "5m check deploy"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            interpret_loop_composer("just a normal prompt"),
            LoopComposer::Other
        ));
    }

    #[test]
    fn browser_extra_overlay_live_looks_cockpit() {
        let entry = SlashEntry {
            command: "browser".into(),
            description: "浏览器驾驶舱（未连接）".into(),
            kind: ExtraSlashKind::Overlay,
            text: "未连接".into(),
            title: "浏览器".into(),
            send: false,
        };
        assert!(matches!(effect_for_extra(&entry, ""), Effect::ShowBrowser));
        let other = SlashEntry {
            command: "skills".into(),
            description: "技能".into(),
            kind: ExtraSlashKind::Overlay,
            text: "body".into(),
            title: "技能".into(),
            send: false,
        };
        match effect_for_extra(&other, "") {
            Effect::ShowNotice { title, body } => {
                assert_eq!(title, "技能");
                assert_eq!(body, "body");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn computer_extra_overlay_opens_show_computer() {
        let entry = SlashEntry {
            command: "computer".into(),
            description: "电脑".into(),
            kind: ExtraSlashKind::Overlay,
            text: String::new(),
            title: "电脑".into(),
            send: false,
        };
        assert!(matches!(effect_for_extra(&entry, ""), Effect::ShowComputer));
    }

    #[test]
    fn extra_slash_slot_opens_registered_id() {
        let entry = SlashEntry {
            command: "memo".into(),
            description: "便签".into(),
            kind: ExtraSlashKind::Slot,
            text: "memo".into(),
            title: String::new(),
            send: false,
        };
        match effect_for_extra(&entry, "") {
            Effect::OpenSlot { id } => assert_eq!(id, "memo"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn extra_slash_tool_runs_named_tool() {
        let entry = SlashEntry {
            command: "test".into(),
            description: "试调".into(),
            kind: ExtraSlashKind::Tool,
            text: "dyn_echo".into(),
            title: String::new(),
            send: false,
        };
        match effect_for_extra(&entry, "") {
            Effect::RunTool {
                name,
                arguments,
                title,
            } => {
                assert_eq!(name, "dyn_echo");
                assert_eq!(arguments, "{}");
                assert_eq!(title, "/test → dyn_echo");
            }
            other => panic!("{other:?}"),
        }
        match effect_for_extra(&entry, r#"{"x":1}"#) {
            Effect::RunTool { arguments, .. } => assert_eq!(arguments, r#"{"x":1}"#),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn extra_slash_workflow_builds_named_source() {
        let entry = SlashEntry {
            command: "deep-research".into(),
            description: "调研".into(),
            kind: ExtraSlashKind::Tool,
            text: WORKFLOW_TOOL_NAME.into(),
            title: "deep-research".into(),
            send: false,
        };
        match effect_for_extra(&entry, "why rust") {
            Effect::RunTool {
                name,
                arguments,
                title,
            } => {
                assert_eq!(name, WORKFLOW_TOOL_NAME);
                assert_eq!(title, "deep-research");
                let v: serde_json::Value = serde_json::from_str(&arguments).unwrap();
                assert_eq!(v["source"]["type"], "name");
                assert_eq!(v["source"]["name"], "deep-research");
                assert_eq!(v["args"]["query"], "why rust");
            }
            other => panic!("{other:?}"),
        }
    }
}
