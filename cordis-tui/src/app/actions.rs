//! Copied variant names from grok-build/.../xai-grok-pager/src/app/actions.rs
//! Action = sync intent; Effect = async I/O.

use std::path::PathBuf;

use crate::slash::{self, ArgKind, SlashCmd, SlashPick};
use crate::theme::ThemeKind;
use cordis_spine::{
    extra_tool_slash_arguments, goal_composer_fill, loop_composer_fill, loop_usage_message,
    lsp_composer_fill, workflow_command_arguments, ApiBackend, CuaAction, ExtraSlashKind,
    SlashEntry, GOAL_RESERVED_SUBCOMMANDS, WORKFLOW_TOOL_NAME,
};

/// Text submitted from the prompt, plus any A6 unbound-image toast.
///
/// `From<String>` / `From<&str>` keep `Action::SendPrompt("…".into())` working
/// for slash/tests (notice defaults to `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSend {
    pub text: String,
    pub unbound_image_notice: Option<String>,
}

impl PromptSend {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            unbound_image_notice: None,
        }
    }

    pub fn with_notice(text: impl Into<String>, notice: Option<String>) -> Self {
        Self {
            text: text.into(),
            unbound_image_notice: notice,
        }
    }
}

impl From<String> for PromptSend {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for PromptSend {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

/// Synchronous, side-effect-free user intent.
#[derive(Debug)]
pub enum Action {
    Quit,
    NewSession,
    /// `Ctrl+N`：开一页新会话（分页，不是换掉当前会话）。
    TabNew,
    /// `Alt+1..9` 或点标签：切到标签上那个号的页。
    TabGo(usize),
    /// `Ctrl+F`：从当前页分叉一页（带上下文快照）。
    TabFork,
    /// `Ctrl+B`：把这一页最近一条回复带回来源页的输入框。
    TabCarryBack,
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
    /// 重新读取当前 overlay 背后的外部状态（Ctrl+R；目前只有 `/mcps` 接）。
    OverlayRefresh,
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
    /// Idle Esc / `/undo`: peel last user send when that turn had no model output.
    UndoLastSend {
        announce_failure: bool,
    },
    FileSearchMove(i16),
    FileSearchAccept,
    FileSearchDismiss,
    CopyAssistant {
        n: usize,
        file: Option<PathBuf>,
    },
    CopyText(String),
    OpenImage(PathBuf),
    /// Submit the prompt buffer. `unbound_image_notice` is flashed *after*
    /// `Effect::SendPrompt` clears the status line (see take_send).
    SendPrompt(PromptSend),
    SendPromptNow(PromptSend),
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
    /// Async: 起一棵新的分页子树（会话 / 循环 / 视图各一份）。
    TabNew,
    /// 切到标签编号 `id` 的页。
    TabGo {
        id: usize,
    },
    /// Async: 关掉一页并 dispose 它整棵子树；`None` 关当前页。
    TabClose {
        id: Option<usize>,
    },
    /// Async: 从当前页分叉一页（带上下文快照，记来源）。
    TabFork,
    /// 把当前页最近一条回复填进来源页的输入框并切过去。
    TabCarryBack,
    /// Async: `/btw`：开一张只读分页问一句，不进主线上下文。
    AsideAsk {
        question: String,
    },
    /// Async: 把当前的只读旁问页转正成全权常驻页。
    TabPromote,
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
    SetProtocol(ApiBackend),
    SetEffort(String),
    ToggleTimestamps,
    ToggleThinking,
    Export(Option<PathBuf>),
    ChangeDir(PathBuf),
    SettingsModal,
    CancelTurn,
    /// Idle Esc / `/undo`: peel last user send when that turn had no model output.
    UndoLastSend {
        announce_failure: bool,
    },
    SendPrompt {
        text: String,
        send_now: bool,
        /// Captured before take_prompt; flashed after clear_notice.
        unbound_image_notice: Option<String>,
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
    ShowAgents,
    ToggleWorkflows,
    /// `/workflow stop <name|run_id>`：停一次在跑的 run。
    StopWorkflow {
        target: String,
    },
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
    MemoryFlush,
    MemoryDream,
    OpenMemoryBrowser,
    MemoryRemember {
        note: String,
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
    /// Async: 重读 `config.toml` 并与已连服务器对账（开 `/mcps` 时自动，Ctrl+R 手动）。
    /// `quiet` 时只有确实变了才 flash，免得每次开 pane 都刷一条。
    ReloadMcps {
        quiet: bool,
    },
    /// Async: 重新探测本机 cua-driver（开 `/computer` 时自动，Ctrl+R 手动）。
    CuaRefresh,
    /// Async: `/computer` 确认后执行安装（`i`）或授权（`p`）。
    CuaRun {
        action: CuaAction,
    },
    SetPrompt {
        text: String,
    },
}

/// `/tab` 的子命令。空参就是开新页——最常用的那个不该还要多打一个词。
/// `close` 不带参数关的是**当前页**；带数字关那一页（1 基，和标签上的号一致）。
fn tab_effect(args: &str) -> Effect {
    let mut parts = args.split_whitespace();
    match parts.next() {
        None | Some("new") | Some("n") => Effect::TabNew,
        Some("fork") | Some("f") => Effect::TabFork,
        Some("back") | Some("b") => Effect::TabCarryBack,
        Some("promote") | Some("p") => Effect::TabPromote,
        Some("close") | Some("c") | Some("x") => Effect::TabClose {
            id: parts.next().and_then(|n| n.parse::<usize>().ok()),
        },
        Some(other) => match other.parse::<usize>() {
            Ok(id) if id > 0 => Effect::TabGo { id },
            _ => Effect::ShowNotice {
                title: "分页".into(),
                body: "用法：/tab（新开）、/tab fork（带上下文分叉）、\
                       /tab back（把本页结论带回来源页）、/tab promote（旁问页转正）、\
                       /tab close [页号]、/tab <页号>\n\
                       快捷键：Ctrl+N 新开、Ctrl+F 分叉、Ctrl+B 带回、Alt+1..9 切换。\n"
                    .into(),
            },
        },
    }
}

pub fn effect_for_slash(cmd: SlashCmd, args: &str) -> Effect {
    let args = args.trim();
    match cmd {
        SlashCmd::New => Effect::NewSession,
        SlashCmd::Tab => tab_effect(args),
        SlashCmd::Btw => {
            if args.is_empty() {
                Effect::ShowNotice {
                    title: "旁问".into(),
                    body: "用法：/btw <问题>\n\
                           从当前页分叉一张**只读**分页问一句：主线那一轮照跑，答案也不进\n\
                           主线上下文。看完 Alt+1 回主线，/tab close 关掉，\n\
                           /tab promote 可以把它转正成全权页。\n"
                        .into(),
                }
            } else {
                Effect::AsideAsk {
                    question: args.to_string(),
                }
            }
        }
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
        // 名字打错就开菜单，不要猜一条协议发出去——猜错就是一次 404。
        SlashCmd::Protocol => match ApiBackend::from_name(args) {
            Some(backend) => Effect::SetProtocol(backend),
            None => Effect::ArgPicker {
                kind: ArgKind::Protocol,
                cmd,
            },
        },
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
        SlashCmd::Agents => Effect::ShowAgents,
        SlashCmd::Workflow => {
            if args.is_empty() || args.eq_ignore_ascii_case("runs") {
                Effect::ToggleWorkflows
            } else {
                let mut words = args.split_whitespace();
                let first = words.next().unwrap_or("");
                // `stop` 带名字就直接停；不带名字仍开 overlay 让用户挑一个。
                // `pause` / `resume` / `save` 还没接后端，一律开 overlay。
                if first == "stop" {
                    match words.next() {
                        Some(target) => Effect::StopWorkflow {
                            target: target.to_string(),
                        },
                        None => Effect::ToggleWorkflows,
                    }
                } else if matches!(first, "pause" | "resume" | "save") {
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
        SlashCmd::Flush => Effect::MemoryFlush,
        SlashCmd::Dream => Effect::MemoryDream,
        SlashCmd::Memory => Effect::OpenMemoryBrowser,
        SlashCmd::Remember => {
            let note = args.trim();
            if note.is_empty() {
                Effect::SetPrompt {
                    text: "用法: /remember <note>\n/remember ".into(),
                }
            } else {
                Effect::MemoryRemember {
                    note: note.to_string(),
                }
            }
        }
        SlashCmd::Undo => Effect::UndoLastSend {
            announce_failure: true,
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
                    unbound_image_notice: None,
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
        "protocol" | "proto" | "wire" => effect_for_slash(SlashCmd::Protocol, rest),
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

    /// `/btw` 要带问题；空参给用法而不是起一个空旁问。
    #[test]
    fn btw_slash_needs_a_question() {
        assert!(matches!(
            effect_for_slash(SlashCmd::Btw, "它卡在哪"),
            Effect::AsideAsk { question } if question == "它卡在哪"
        ));
        assert!(matches!(
            effect_for_slash(SlashCmd::Btw, "  "),
            Effect::ShowNotice { .. }
        ));
    }

    /// `/tab` 空参就是开新页：最常用的动作不该还要多打一个词。
    #[test]
    fn tab_slash_parses_subcommands() {
        assert!(matches!(tab_effect(""), Effect::TabNew));
        assert!(matches!(tab_effect("new"), Effect::TabNew));
        assert!(matches!(tab_effect("close"), Effect::TabClose { id: None }));
        assert!(matches!(
            tab_effect("close 3"),
            Effect::TabClose { id: Some(3) }
        ));
        assert!(matches!(tab_effect("2"), Effect::TabGo { id: 2 }));
        assert!(matches!(tab_effect("fork"), Effect::TabFork));
        assert!(matches!(tab_effect("back"), Effect::TabCarryBack));
        // 打错了给用法，不猜一个页号切过去。
        assert!(matches!(tab_effect("nope"), Effect::ShowNotice { .. }));
        assert!(matches!(tab_effect("0"), Effect::ShowNotice { .. }));
    }

    /// `/protocol` 认 config 里的写法与常见简写；空参数和打错的名字都开菜单，
    /// 不猜一条协议发出去。
    #[test]
    fn protocol_slash_parses_names_and_falls_back_to_the_picker() {
        for (args, want) in [
            ("responses", ApiBackend::Responses),
            ("resp", ApiBackend::Responses),
            ("chat_completions", ApiBackend::ChatCompletions),
            ("chat-completions", ApiBackend::ChatCompletions),
            ("Messages", ApiBackend::Messages),
            ("anthropic", ApiBackend::Messages),
        ] {
            match effect_for_slash(SlashCmd::Protocol, args) {
                Effect::SetProtocol(got) => assert_eq!(got, want, "{args}"),
                other => panic!("{args}: {other:?}"),
            }
        }
        for args in ["", "grpc"] {
            assert!(
                matches!(
                    effect_for_slash(SlashCmd::Protocol, args),
                    Effect::ArgPicker {
                        kind: ArgKind::Protocol,
                        ..
                    }
                ),
                "{args} 该开菜单"
            );
        }
    }

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
