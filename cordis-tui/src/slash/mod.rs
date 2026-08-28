//! Slash registry. Matching uses grok's nucleo `FuzzyMatcher`.
//! Dropdown chrome is copied from pager `views/slash_dropdown.rs`.

mod args;
mod dropdown;
mod interval;
mod matcher;

use cordis_spine::{load_catalog, AppSettings, ModelChoice};

pub use args::ArgItem;
pub use dropdown::{desired_item_rows, render_dropdown, SuggestionRow};
pub use interval::{interval_to_human, parse_loop_args, token_to_duration};
pub use matcher::FuzzyMatcher;

/// Copied from grok `slash::MAX_VISIBLE_SUGGESTIONS`.
pub const MAX_VISIBLE_SUGGESTIONS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCmd {
    New,
    Resume,
    Help,
    History,
    Find,
    Copy,
    Quit,
    Theme,
    Model,
    Settings,
    Loop,
    Export,
    Cd,
    Timestamps,
    Effort,
    Plan,
    Goal,
    Tasks,
    Mcps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    Theme,
    Model,
    Settings,
    Effort,
    LoopInterval,
}

pub struct SlashDef {
    pub cmd: SlashCmd,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub display: &'static str,
    pub description: &'static str,
    #[allow(dead_code)]
    pub takes_args: bool,
    #[allow(dead_code)]
    pub args_required: bool,
    #[allow(dead_code)]
    pub arg_kind: Option<ArgKind>,
}

/// Menu order copied from grok `builtin_commands()` (pager-local subset).
pub const CATALOG: &[SlashDef] = &[
    SlashDef {
        cmd: SlashCmd::Settings,
        name: "settings",
        aliases: &["config", "prefs"],
        display: "/settings",
        description: "打开设置",
        takes_args: true,
        args_required: false,
        arg_kind: Some(ArgKind::Settings),
    },
    SlashDef {
        cmd: SlashCmd::New,
        name: "new",
        aliases: &[],
        display: "/new",
        description: "开始新会话",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Model,
        name: "model",
        aliases: &["m"],
        display: "/model",
        description: "切换当前模型",
        takes_args: true,
        args_required: true,
        arg_kind: Some(ArgKind::Model),
    },
    SlashDef {
        cmd: SlashCmd::Resume,
        name: "resume",
        aliases: &[],
        display: "/resume",
        description: "恢复上次会话",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Loop,
        name: "loop",
        aliases: &["cron"],
        display: "/loop",
        description: "按间隔循环提问",
        takes_args: true,
        args_required: true,
        arg_kind: Some(ArgKind::LoopInterval),
    },
    SlashDef {
        cmd: SlashCmd::Plan,
        name: "plan",
        aliases: &[],
        display: "/plan",
        description: "进入计划模式",
        takes_args: true,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Goal,
        name: "goal",
        aliases: &[],
        display: "/goal",
        description: "开始或查看目标",
        takes_args: true,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Tasks,
        name: "tasks",
        aliases: &[],
        display: "/tasks",
        description: "列出后台任务与定时任务",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Mcps,
        name: "mcps",
        aliases: &[],
        display: "/mcps",
        description: "显示 MCP 服务器状态",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::History,
        name: "history",
        aliases: &[],
        display: "/history",
        description: "搜索提示词历史",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Copy,
        name: "copy",
        aliases: &[],
        display: "/copy",
        description: "把上一条回复复制到剪贴板或文件",
        takes_args: true,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Find,
        name: "find",
        aliases: &[],
        display: "/find",
        description: "搜索对话",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Theme,
        name: "theme",
        aliases: &["t"],
        display: "/theme",
        description: "切换配色",
        takes_args: true,
        args_required: false,
        arg_kind: Some(ArgKind::Theme),
    },
    SlashDef {
        cmd: SlashCmd::Timestamps,
        name: "timestamps",
        aliases: &[],
        display: "/timestamps",
        description: "开关滚动区时间戳",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Effort,
        name: "effort",
        aliases: &[],
        display: "/effort",
        description: "设置推理强度",
        takes_args: true,
        args_required: true,
        arg_kind: Some(ArgKind::Effort),
    },
    SlashDef {
        cmd: SlashCmd::Export,
        name: "export",
        aliases: &[],
        display: "/export",
        description: "把对话导出到文件",
        takes_args: true,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Cd,
        name: "cd",
        aliases: &[],
        display: "/cd",
        description: "切换工作目录",
        takes_args: true,
        args_required: true,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Help,
        name: "help",
        aliases: &[],
        display: "/help",
        description: "显示斜杠命令",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Quit,
        name: "quit",
        aliases: &["exit"],
        display: "/quit",
        description: "退出",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
];

#[allow(dead_code)]
pub fn def_for(cmd: SlashCmd) -> &'static SlashDef {
    CATALOG.iter().find(|d| d.cmd == cmd).unwrap()
}

pub fn lookup(name: &str) -> Option<&'static SlashDef> {
    let n = name.trim_start_matches('/');
    CATALOG.iter().find(|d| d.name == n || d.aliases.iter().any(|a| *a == n))
}

#[derive(Debug, Clone)]
pub struct SlashSnapshot {
    pub open: bool,
    pub selected: usize,
    pub matches: Vec<SuggestionRow>,
}

impl SlashSnapshot {
    pub fn current(&self) -> Option<&SuggestionRow> {
        self.matches.get(self.selected)
    }
}

/// `text` is the composer buffer. `selected` is the highlighted match index.
pub fn snapshot(text: &str, selected: usize) -> SlashSnapshot {
    let Some(query) = slash_query(text) else {
        return SlashSnapshot {
            open: false,
            selected: 0,
            matches: Vec::new(),
        };
    };
    let mut matcher = FuzzyMatcher::new();
    let ranked = matcher.rank(CATALOG, query, CATALOG.len(), |d| d.name);
    let matches: Vec<SuggestionRow> = ranked
        .into_iter()
        .map(|(idx, _)| {
            let d = &CATALOG[idx];
            SuggestionRow {
                display: d.display.to_string(),
                description: d.description.to_string(),
                cmd: d.cmd,
            }
        })
        .collect();
    let selected = if matches.is_empty() {
        0
    } else {
        selected.min(matches.len() - 1)
    };
    SlashSnapshot {
        open: !matches.is_empty(),
        selected,
        matches,
    }
}

fn slash_query(text: &str) -> Option<&str> {
    if text.contains('\n') {
        return None;
    }
    let rest = text.strip_prefix('/')?;
    Some(rest.split(char::is_whitespace).next().unwrap_or(""))
}

pub fn command_for_submit(text: &str) -> Option<(SlashCmd, String)> {
    let rest = text.strip_prefix('/')?;
    if text.contains('\n') {
        return None;
    }
    let mut parts = rest.splitn(2, char::is_whitespace);
    let query = parts.next().unwrap_or("");
    if query.is_empty() {
        return None;
    }
    let args = parts.next().unwrap_or("").trim().to_string();
    lookup(query).map(|d| (d.cmd, args))
}

/// `/copy [N] [file]` — copied from grok `slash::commands::copy`.
pub fn parse_copy_args(args: &str) -> Result<(usize, Option<std::path::PathBuf>), String> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok((1, None));
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("");
    let rest = parts.next().map(str::trim).filter(|s| !s.is_empty());
    match first.parse::<usize>() {
        Ok(0) => Err("Usage: /copy [N] [file] where N is 1 (latest), 2, 3, ...".into()),
        Ok(n) => Ok((n, rest.map(std::path::PathBuf::from))),
        Err(_) => Ok((1, Some(std::path::PathBuf::from(trimmed)))),
    }
}

pub fn theme_args() -> Vec<ArgItem> {
    crate::theme::ThemeKind::ALL
        .iter()
        .map(|k| ArgItem::new(k.display_name(), "配色"))
        .collect()
}

fn catalog_items(settings: Option<&AppSettings>) -> Vec<ModelChoice> {
    settings
        .map(|s| s.catalog())
        .unwrap_or_else(load_catalog)
}

pub fn model_args(settings: Option<&AppSettings>) -> Vec<ArgItem> {
    catalog_items(settings)
        .into_iter()
        .map(|m| {
            let desc = if m.description.is_empty() {
                m.name
            } else {
                m.description
            };
            ArgItem::new(m.id, desc)
        })
        .collect()
}

pub fn effort_args() -> Vec<ArgItem> {
    ["low", "medium", "high", "xhigh"]
        .into_iter()
        .map(|e| ArgItem::new(e, "推理强度"))
        .collect()
}

pub fn settings_args(settings: Option<&AppSettings>) -> Vec<ArgItem> {
    let model_id = settings
        .map(|s| s.model())
        .filter(|id| !id.trim().is_empty())
        .or_else(|| catalog_items(settings).into_iter().next().map(|m| m.id))
        .unwrap_or_else(|| "grok-4".into());
    vec![
        ArgItem::new("timestamps", "开关时间戳"),
        ArgItem::new("theme groknight", "GrokNight 配色"),
        ArgItem::new("theme grokday", "GrokDay 配色"),
        ArgItem::new("theme tokyonight", "TokyoNight 配色"),
        ArgItem::new(format!("model {model_id}"), "默认模型"),
    ]
}

pub fn loop_interval_args() -> Vec<ArgItem> {
    ["30s", "1m", "5m", "15m", "1h"]
        .into_iter()
        .map(|t| ArgItem::new(t, interval_to_human(t)))
        .collect()
}

pub fn args_for(kind: ArgKind, settings: Option<&AppSettings>) -> Vec<ArgItem> {
    match kind {
        ArgKind::Theme => theme_args(),
        ArgKind::Model => model_args(settings),
        ArgKind::Settings => settings_args(settings),
        ArgKind::Effort => effort_args(),
        ArgKind::LoopInterval => loop_interval_args(),
    }
}

pub fn filter_args(kind: ArgKind, query: &str, settings: Option<&AppSettings>) -> Vec<ArgItem> {
    let items = args_for(kind, settings);
    let mut matcher = FuzzyMatcher::new();
    let ranked = matcher.rank(&items, query, items.len(), |a| a.match_text.as_str());
    ranked.into_iter().map(|(i, _)| items[i].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slash_lists_catalog() {
        let snap = snapshot("/", 0);
        assert!(snap.open);
        assert_eq!(snap.matches.len(), CATALOG.len());
    }

    #[test]
    fn nucleo_ranks_resume() {
        let snap = snapshot("/re", 0);
        assert_eq!(snap.matches[0].display, "/resume");
    }

    #[test]
    fn alias_maps() {
        assert_eq!(lookup("t").map(|d| d.cmd), Some(SlashCmd::Theme));
        assert_eq!(lookup("cron").map(|d| d.cmd), Some(SlashCmd::Loop));
        assert_eq!(lookup("plan").map(|d| d.cmd), Some(SlashCmd::Plan));
        assert_eq!(lookup("goal").map(|d| d.cmd), Some(SlashCmd::Goal));
        assert_eq!(lookup("tasks").map(|d| d.cmd), Some(SlashCmd::Tasks));
        assert_eq!(lookup("mcps").map(|d| d.cmd), Some(SlashCmd::Mcps));
    }

    #[test]
    fn prose_is_not_slash() {
        assert!(!snapshot("hello /new", 0).open);
        assert!(command_for_submit("hello").is_none());
    }

    #[test]
    fn submit_maps_name() {
        assert_eq!(
            command_for_submit("/quit"),
            Some((SlashCmd::Quit, String::new()))
        );
        assert_eq!(
            command_for_submit("/copy 2 out.txt"),
            Some((SlashCmd::Copy, "2 out.txt".into()))
        );
        assert_eq!(
            command_for_submit("/theme grokday"),
            Some((SlashCmd::Theme, "grokday".into()))
        );
    }

    #[test]
    fn copy_args_parse() {
        assert_eq!(parse_copy_args("").unwrap(), (1, None));
        assert_eq!(parse_copy_args("2").unwrap().0, 2);
    }

    #[test]
    fn model_args_come_from_catalog() {
        assert!(!model_args(None).is_empty());
    }
}
