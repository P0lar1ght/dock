//! Slash registry. Matching uses grok's nucleo `FuzzyMatcher`.
//! Dropdown chrome is copied from pager `views/slash_dropdown.rs`.

mod args;
mod dropdown;
mod interval;
mod matcher;

use cordis_spine::{load_catalog, ApiBackend, AppSettings, ModelChoice, SlashEntry};

pub use args::ArgItem;
pub use dropdown::{desired_item_rows, render_dropdown, SuggestionRow};
pub use interval::interval_to_human;
pub use matcher::FuzzyMatcher;

/// Copied from grok `slash::MAX_VISIBLE_SUGGESTIONS`.
pub const MAX_VISIBLE_SUGGESTIONS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCmd {
    New,
    Tab,
    Btw,
    Resume,
    Pair,
    Help,
    History,
    Find,
    Copy,
    Quit,
    Theme,
    Model,
    Protocol,
    Settings,
    Loop,
    Export,
    Cd,
    Timestamps,
    Thinking,
    Effort,
    Plan,
    ViewPlan,
    Goal,
    Tasks,
    Workflow,
    Mcps,
    Lsp,
    Cordis,
    Preset,
    Usage,
    Context,
    Compact,
}

/// Builtin catalog entry or a live extra from `"slash"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashPick {
    Builtin(SlashCmd),
    Extra(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    Tab,
    Theme,
    Model,
    Protocol,
    Settings,
    Effort,
    LoopInterval,
    Lsp,
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

/// Name / alias / description tokens for the TUI slash catalog.
///
/// Gateway `slash/list` iterates this so the browser autocomplete cannot
/// drift from the pager dropdown. Does not expose overlay `Effect`s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlashCatalogEntry {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub takes_args: bool,
}

impl SlashDef {
    fn catalog_entry(&self) -> SlashCatalogEntry {
        SlashCatalogEntry {
            name: self.name,
            aliases: self.aliases,
            description: self.description,
            takes_args: self.takes_args,
        }
    }
}

/// Builtin catalog in dropdown order (TUI `CATALOG`).
pub fn slash_catalog() -> impl Iterator<Item = SlashCatalogEntry> {
    CATALOG.iter().map(SlashDef::catalog_entry)
}

/// Resolve a name or alias to the canonical catalog entry.
pub fn resolve_slash(raw: &str) -> Option<SlashCatalogEntry> {
    lookup(raw).map(SlashDef::catalog_entry)
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
        cmd: SlashCmd::Tab,
        name: "tab",
        aliases: &["tabs"],
        display: "/tab",
        description: "分页：new / fork / back / close / 页号",
        takes_args: true,
        args_required: false,
        arg_kind: Some(ArgKind::Tab),
    },
    SlashDef {
        cmd: SlashCmd::Btw,
        name: "btw",
        aliases: &["aside", "旁问"],
        display: "/btw",
        description: "插一嘴：只读地问一句，不打断当前任务、不进主线上下文",
        takes_args: true,
        args_required: true,
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
        cmd: SlashCmd::Protocol,
        name: "protocol",
        aliases: &["proto", "wire"],
        display: "/protocol",
        description: "切换当前模型的推理协议",
        takes_args: true,
        args_required: false,
        arg_kind: Some(ArgKind::Protocol),
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
        cmd: SlashCmd::Pair,
        name: "pair",
        aliases: &["pairing"],
        display: "/pair",
        description: "浏览器配对（开启回环网关）",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Loop,
        name: "loop",
        aliases: &["cron"],
        display: "/loop",
        description: "安排循环提问",
        takes_args: true,
        args_required: false,
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
        cmd: SlashCmd::ViewPlan,
        name: "view-plan",
        aliases: &["show-plan", "plan-view"],
        display: "/view-plan",
        description: "查看或批准当前计划",
        takes_args: false,
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
        cmd: SlashCmd::Workflow,
        name: "workflow",
        aliases: &[],
        display: "/workflow",
        description: "查看工作流运行",
        takes_args: true,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Mcps,
        name: "mcps",
        aliases: &[],
        display: "/mcps",
        description: "MCP 服务器（Space 开关 · i 登录 · Enter 展开）",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Lsp,
        name: "lsp",
        aliases: &[],
        display: "/lsp",
        description: "探测并写入语言服务器（Tab 补全 status / setup / user）",
        takes_args: true,
        args_required: false,
        arg_kind: Some(ArgKind::Lsp),
    },
    SlashDef {
        cmd: SlashCmd::Cordis,
        name: "cordis",
        aliases: &["plugins"],
        display: "/cordis",
        description: "动态 / 永久 Cordis 插件",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Preset,
        name: "preset",
        aliases: &["presets", "agent", "agents"],
        display: "/preset",
        description: "组装 Agent 预设",
        takes_args: true,
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
        cmd: SlashCmd::Usage,
        name: "usage",
        aliases: &["cost"],
        display: "/usage",
        description: "查看本会话用量（Tab 切到占用）",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Context,
        name: "context",
        aliases: &[],
        display: "/context",
        description: "查看上下文占用",
        takes_args: false,
        args_required: false,
        arg_kind: None,
    },
    SlashDef {
        cmd: SlashCmd::Compact,
        name: "compact",
        aliases: &[],
        display: "/compact",
        description: "压缩旧对话",
        takes_args: true,
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
        cmd: SlashCmd::Thinking,
        name: "think",
        aliases: &["thinking"],
        display: "/think",
        description: "开关思考模式（推理过程）",
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
    CATALOG
        .iter()
        .find(|d| d.name == n || d.aliases.contains(&n))
}

#[derive(Debug, Clone)]
pub struct SlashSnapshot {
    pub open: bool,
    pub selected: usize,
    pub matches: Vec<SuggestionRow>,
    /// True while completing `/cmd args` — Enter should send, Tab still fills.
    pub completing_args: bool,
}

impl SlashSnapshot {
    pub fn current(&self) -> Option<&SuggestionRow> {
        self.matches.get(self.selected)
    }
}

/// `text` is the composer buffer. `selected` is the highlighted match index.
#[cfg(test)]
pub fn snapshot(text: &str, selected: usize) -> SlashSnapshot {
    snapshot_ex(text, selected, &[])
}

#[cfg(test)]
pub fn snapshot_ex(text: &str, selected: usize, extras: &[SlashEntry]) -> SlashSnapshot {
    snapshot_with_settings(text, selected, extras, None)
}

pub fn snapshot_with_settings(
    text: &str,
    selected: usize,
    extras: &[SlashEntry],
    settings: Option<&AppSettings>,
) -> SlashSnapshot {
    let closed = SlashSnapshot {
        open: false,
        selected: 0,
        matches: Vec::new(),
        completing_args: false,
    };
    let (matches, completing_args) = match slash_phase(text) {
        None => return closed,
        Some(SlashPhase::Command(query)) => (command_matches(query, extras), false),
        Some(SlashPhase::Args { name, query }) => (arg_matches(name, query, settings), true),
    };
    if matches.is_empty() {
        return closed;
    }
    let selected = selected.min(matches.len() - 1);
    SlashSnapshot {
        open: true,
        selected,
        matches,
        completing_args,
    }
}

fn command_matches(query: &str, extras: &[SlashEntry]) -> Vec<SuggestionRow> {
    let sources = rank_sources(extras);
    let mut matcher = FuzzyMatcher::new();
    let ranked = matcher.rank(&sources, query, sources.len(), |d| d.name.as_str());
    ranked
        .into_iter()
        .map(|(idx, _)| {
            let d = &sources[idx];
            SuggestionRow {
                display: d.display.clone(),
                description: d.description.clone(),
                pick: d.pick.clone(),
            }
        })
        .collect()
}

fn arg_matches(name: &str, query: &str, settings: Option<&AppSettings>) -> Vec<SuggestionRow> {
    let Some(def) = lookup(name) else {
        return Vec::new();
    };
    let Some(kind) = def.arg_kind else {
        return Vec::new();
    };
    filter_args(kind, query, settings)
        .into_iter()
        .map(|item| SuggestionRow {
            display: format!("{} {}", def.display, item.insert_text),
            description: item.description,
            pick: SlashPick::Builtin(def.cmd),
        })
        .collect()
}

struct RankSource {
    name: String,
    display: String,
    description: String,
    pick: SlashPick,
}

fn rank_sources(extras: &[SlashEntry]) -> Vec<RankSource> {
    let mut out: Vec<RankSource> = CATALOG
        .iter()
        .map(|d| RankSource {
            name: d.name.into(),
            display: d.display.into(),
            description: d.description.into(),
            pick: SlashPick::Builtin(d.cmd),
        })
        .collect();
    for extra in extras {
        if lookup(&extra.command).is_some() {
            continue;
        }
        out.push(RankSource {
            name: extra.command.clone(),
            display: extra.display(),
            description: extra.description.clone(),
            pick: SlashPick::Extra(extra.command.clone()),
        });
    }
    out
}

enum SlashPhase<'a> {
    /// `/ls` — still choosing a command name.
    Command(&'a str),
    /// `/lsp s` — command chosen; completing arguments when `arg_kind` is set.
    Args { name: &'a str, query: &'a str },
}

fn slash_phase(text: &str) -> Option<SlashPhase<'_>> {
    if text.contains('\n') {
        return None;
    }
    let rest = text.strip_prefix('/')?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    match parts.next() {
        None => Some(SlashPhase::Command(name)),
        Some(query) => Some(SlashPhase::Args { name, query }),
    }
}

#[cfg(test)]
pub fn command_for_submit(text: &str) -> Option<(SlashPick, String)> {
    command_for_submit_ex(text, &[])
}

pub fn command_for_submit_ex(text: &str, extras: &[SlashEntry]) -> Option<(SlashPick, String)> {
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
    if let Some(d) = lookup(query) {
        return Some((SlashPick::Builtin(d.cmd), args));
    }
    extras
        .iter()
        .find(|e| e.command == query)
        .map(|e| (SlashPick::Extra(e.command.clone()), args))
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
    settings.map(|s| s.catalog()).unwrap_or_else(load_catalog)
}

pub fn model_args(settings: Option<&AppSettings>) -> Vec<ArgItem> {
    let items = catalog_items(settings);
    // 目录来自 config.toml，没配就是空的（内置假条目已删）。给一条能照着做的
    // 提示，别让 /model 变成一个不解释为什么的空下拉。
    if items.is_empty() {
        // 这是一条说明，不是一个可选项：`insert_text` 留空，回车不会把提示文案
        // 当成模型 id 提交出去（`ArgItem::new` 默认 insert_text = display）。
        let mut hint = ArgItem::new(
            "（无模型）",
            "在 ~/.dock/config.toml 或 .dock/config.toml 里写 [model.<id>]，见 config.toml.example",
        );
        hint.insert_text = String::new();
        return vec![hint];
    }
    items
        .into_iter()
        .map(|m| {
            let desc = if m.description.is_empty() {
                m.name.clone()
            } else {
                m.description.clone()
            };
            // 声明了多条 wire 的端点在这里就说清楚，省得用户为了看协议再开一次
            // /protocol。只有一条时不占位置。
            let desc = if m.has_backend_choice() {
                let wires: Vec<&str> = m.api_backends.iter().map(|b| b.name()).collect();
                format!("{desc} · {}", wires.join(" / "))
            } else {
                desc
            };
            ArgItem::new(m.id, desc)
        })
        .collect()
}

/// 协议菜单来自当前模型的 `[model.<id>].api_backends`——端点没声明的那条切过去
/// 就是 404，所以这里只列声明过的。目录里没有当前模型才退回通用三条。
pub fn protocol_args(settings: Option<&AppSettings>) -> Vec<ArgItem> {
    let (choices, current) = match settings {
        Some(settings) => (settings.backend_choices(), Some(settings.backend())),
        None => (ApiBackend::ALL.to_vec(), None),
    };
    if choices.len() <= 1 {
        let only = choices.first().copied().unwrap_or_default();
        let mut hint = ArgItem::new(
            format!("（只声明了 {}）", only.name()),
            "在 config 里给这个模型写 api_backends = [\"responses\", \"chat_completions\"]",
        );
        hint.insert_text = String::new();
        return vec![hint];
    }
    choices
        .into_iter()
        .map(|backend| {
            let desc = if Some(backend) == current {
                format!("{} · 当前", backend.description())
            } else {
                backend.description().to_string()
            };
            ArgItem::new(backend.name(), desc)
        })
        .collect()
}

/// 档位来自当前模型的 `[model.<id>].reasoning_efforts`，没配才退回通用四档——
/// 各家认识的档位不一样，列死一份只会让人选到上游不认的值。
pub fn effort_args(settings: Option<&AppSettings>) -> Vec<ArgItem> {
    let Some(settings) = settings else {
        return cordis_spine::DEFAULT_EFFORT_CHOICES
            .iter()
            .map(|e| ArgItem::new(*e, "推理强度"))
            .collect();
    };
    let choices = settings.effort_choices();
    if choices.is_empty() {
        let mut hint = ArgItem::new(
            "（该模型无推理档）",
            "config 里这个模型写了 reasoning = false",
        );
        hint.insert_text = String::new();
        return vec![hint];
    }
    let current = settings.effort();
    choices
        .into_iter()
        .map(|e| {
            let desc = if e == current {
                "推理强度 · 当前"
            } else {
                "推理强度"
            };
            ArgItem::new(e, desc)
        })
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
        ArgItem::new("protocol", "推理协议"),
    ]
}

pub fn loop_interval_args() -> Vec<ArgItem> {
    ["30s", "1m", "5m", "15m", "1h"]
        .into_iter()
        .map(|t| ArgItem::new(t, interval_to_human(t)))
        .collect()
}

/// `/tab` 的子命令补全。页号不在这里 —— `args_for` 看不到 `"tui.tabs"`，
/// 而且数字查询本来就该让下拉关掉、直接把 `/tab 3` 提交出去。
pub fn tab_args() -> Vec<ArgItem> {
    vec![
        ArgItem::new("new", "开一张空白新页（Ctrl+N）"),
        ArgItem::new("fork", "带当前页上下文快照分叉一页（Ctrl+F）"),
        ArgItem::new("back", "把本页结论带回来源页的输入框（Ctrl+B）"),
        ArgItem::new("close", "关掉当前页；`close <页号>` 关那一页"),
    ]
}

pub fn lsp_args() -> Vec<ArgItem> {
    vec![
        ArgItem::new("status", "只看配置，不写文件"),
        ArgItem::new("setup", "写入项目 .dock/lsp.json"),
        ArgItem::new("user", "写入 ~/.dock/lsp.json"),
    ]
}

pub fn args_for(kind: ArgKind, settings: Option<&AppSettings>) -> Vec<ArgItem> {
    match kind {
        ArgKind::Tab => tab_args(),
        ArgKind::Theme => theme_args(),
        ArgKind::Model => model_args(settings),
        ArgKind::Protocol => protocol_args(settings),
        ArgKind::Settings => settings_args(settings),
        ArgKind::Effort => effort_args(settings),
        ArgKind::LoopInterval => loop_interval_args(),
        ArgKind::Lsp => lsp_args(),
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

    /// `/tab ` 之后补子命令；页号是自由输入，下拉该让开。
    #[test]
    fn tab_completes_subcommands_but_not_page_numbers() {
        let snap = snapshot("/tab ", 0);
        assert!(snap.open && snap.completing_args, "{snap:?}");
        let rows: Vec<&str> = snap.matches.iter().map(|m| m.display.as_str()).collect();
        for want in ["/tab new", "/tab fork", "/tab back", "/tab close"] {
            assert!(rows.contains(&want), "缺 {want}：{rows:?}");
        }

        let snap = snapshot("/tab fo", 0);
        assert_eq!(snap.matches[0].display, "/tab fork");

        // 数字不该被模糊匹配硬塞成某个子命令：下拉关掉，`/tab 3` 直接提交。
        let snap = snapshot("/tab 3", 0);
        assert!(!snap.open, "{snap:?}");
    }

    /// `/btw` 是自由文本，不该弹一堆补全把问题挡住。
    #[test]
    fn btw_does_not_complete_its_question() {
        let snap = snapshot("/btw 它卡在哪", 0);
        assert!(!snap.open, "{snap:?}");
    }

    #[test]
    fn alias_maps() {
        assert_eq!(lookup("t").map(|d| d.cmd), Some(SlashCmd::Theme));
        assert_eq!(lookup("cron").map(|d| d.cmd), Some(SlashCmd::Loop));
        assert_eq!(lookup("plan").map(|d| d.cmd), Some(SlashCmd::Plan));
        assert_eq!(lookup("goal").map(|d| d.cmd), Some(SlashCmd::Goal));
        assert_eq!(lookup("tasks").map(|d| d.cmd), Some(SlashCmd::Tasks));
        assert_eq!(lookup("workflow").map(|d| d.cmd), Some(SlashCmd::Workflow));
        assert_eq!(lookup("mcps").map(|d| d.cmd), Some(SlashCmd::Mcps));
        assert_eq!(lookup("lsp").map(|d| d.cmd), Some(SlashCmd::Lsp));
        assert_eq!(lookup("cordis").map(|d| d.cmd), Some(SlashCmd::Cordis));
        assert_eq!(lookup("plugins").map(|d| d.cmd), Some(SlashCmd::Cordis));
        assert_eq!(lookup("preset").map(|d| d.cmd), Some(SlashCmd::Preset));
        assert_eq!(lookup("agent").map(|d| d.cmd), Some(SlashCmd::Preset));
        assert_eq!(lookup("usage").map(|d| d.cmd), Some(SlashCmd::Usage));
        assert_eq!(lookup("cost").map(|d| d.cmd), Some(SlashCmd::Usage));
        assert_eq!(lookup("context").map(|d| d.cmd), Some(SlashCmd::Context));
        assert_eq!(lookup("compact").map(|d| d.cmd), Some(SlashCmd::Compact));
    }

    #[test]
    fn prose_is_not_slash() {
        assert!(!snapshot("hello /new", 0).open);
        assert!(command_for_submit("hello").is_none());
    }

    #[test]
    fn args_close_the_picker() {
        assert!(snapshot("/quit", 0).open);
        assert!(!snapshot("/quit ", 0).open);
        assert!(!snapshot("/quit now", 0).open);
        let extras = [extra("standup")];
        assert!(snapshot_ex("/standup", 0, &extras).open);
        assert!(!snapshot_ex("/standup ", 0, &extras).open);
    }

    #[test]
    fn lsp_args_stay_open_and_filter() {
        let cmd = snapshot("/lsp", 0);
        assert!(cmd.open && !cmd.completing_args, "{cmd:?}");

        let all = snapshot("/lsp ", 0);
        assert!(all.open && all.completing_args, "{all:?}");
        let displays: Vec<_> = all.matches.iter().map(|r| r.display.as_str()).collect();
        assert!(displays.contains(&"/lsp status"), "{displays:?}");
        assert!(displays.contains(&"/lsp setup"), "{displays:?}");
        assert!(displays.contains(&"/lsp user"), "{displays:?}");

        let filtered = snapshot("/lsp s", 0);
        assert!(filtered.open);
        let displays: Vec<_> = filtered
            .matches
            .iter()
            .map(|r| r.display.as_str())
            .collect();
        assert!(displays.contains(&"/lsp status"), "{displays:?}");
        assert!(displays.contains(&"/lsp setup"), "{displays:?}");

        let status = snapshot("/lsp sta", 0);
        assert_eq!(status.matches[0].display, "/lsp status");
        assert!(status.completing_args);
    }

    #[test]
    fn submit_maps_name() {
        assert_eq!(
            command_for_submit("/quit"),
            Some((SlashPick::Builtin(SlashCmd::Quit), String::new()))
        );
        assert_eq!(
            command_for_submit("/copy 2 out.txt"),
            Some((SlashPick::Builtin(SlashCmd::Copy), "2 out.txt".into()))
        );
        assert_eq!(
            command_for_submit("/theme grokday"),
            Some((SlashPick::Builtin(SlashCmd::Theme), "grokday".into()))
        );
    }

    fn extra(command: &str) -> SlashEntry {
        SlashEntry {
            command: command.into(),
            description: "自定义".into(),
            kind: cordis_spine::ExtraSlashKind::Prompt,
            text: "hello {args}".into(),
            title: String::new(),
            send: true,
        }
    }

    #[test]
    fn extras_append_and_cannot_shadow_help() {
        let extras = [extra("standup"), extra("help")];
        let snap = snapshot_ex("/", 0, &extras);
        assert!(snap.matches.iter().any(|r| r.display == "/standup"));
        assert!(!snap
            .matches
            .iter()
            .any(|r| matches!(&r.pick, SlashPick::Extra(n) if n == "help")));
        assert_eq!(
            command_for_submit_ex("/standup today", &extras),
            Some((SlashPick::Extra("standup".into()), "today".into()))
        );
        assert_eq!(
            command_for_submit_ex("/help", &extras),
            Some((SlashPick::Builtin(SlashCmd::Help), String::new()))
        );
    }

    #[test]
    fn public_catalog_tokens_match_internal() {
        let internal: Vec<_> = CATALOG
            .iter()
            .map(|d| (d.name, d.aliases, d.description, d.takes_args))
            .collect();
        let public: Vec<_> = slash_catalog()
            .map(|e| (e.name, e.aliases, e.description, e.takes_args))
            .collect();
        assert_eq!(internal, public);
        assert_eq!(resolve_slash("cron").map(|e| e.name), Some("loop"));
        assert_eq!(resolve_slash("cd").map(|e| e.name), Some("cd"));
    }

    #[test]
    fn catalog_names_and_aliases_unique() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for d in CATALOG {
            assert!(seen.insert(d.name), "duplicate catalog name {}", d.name);
            for alias in d.aliases {
                assert!(
                    seen.insert(*alias),
                    "duplicate catalog token {alias} (name or alias)"
                );
            }
        }
    }

    #[test]
    fn reserved_covers_catalog() {
        for d in CATALOG {
            assert!(
                cordis_spine::slash_name_reserved(d.name),
                "missing reserved {}",
                d.name
            );
            for alias in d.aliases {
                assert!(
                    cordis_spine::slash_name_reserved(alias),
                    "missing reserved alias {alias}"
                );
            }
        }
    }

    #[test]
    fn copy_args_parse() {
        assert_eq!(parse_copy_args("").unwrap(), (1, None));
        assert_eq!(parse_copy_args("2").unwrap().0, 2);
    }

    /// 空目录时给的是说明而不是选项：`insert_text` 必须为空，否则回车会把
    /// 「（无模型）」当成模型 id 提交出去。
    #[test]
    fn empty_catalog_hint_is_not_selectable() {
        for item in model_args(None).iter().chain(effort_args(None).iter()) {
            if item.display.starts_with('（') {
                assert!(
                    item.insert_text.is_empty(),
                    "提示项不能被填进输入框：{item:?}"
                );
            }
        }
    }

    /// 没有 settings 时退回通用四档（`/effort` 从浏览器 companion 过来就是这条路）。
    #[test]
    fn effort_args_fall_back_to_the_generic_ladder() {
        let args = effort_args(None);
        let values: Vec<&str> = args.iter().map(|a| a.display.as_str()).collect();
        assert_eq!(values, cordis_spine::DEFAULT_EFFORT_CHOICES);
        assert!(args.iter().all(|a| !a.insert_text.is_empty()));
    }

    /// 目录只来自 config（内置假条目已删），所以这里断言的是「与 live 目录
    /// 逐条对应」，而不是「非空」——没有 config.toml 时给的是写配置的指引。
    #[test]
    fn model_args_mirror_the_live_catalog() {
        let catalog = load_catalog();
        let args = model_args(None);
        if catalog.is_empty() {
            assert_eq!(args.len(), 1);
            assert!(args[0].description.contains("config.toml"), "{args:?}");
            return;
        }
        assert_eq!(args.len(), catalog.len());
        let ids: Vec<&str> = args.iter().map(|a| a.display.as_str()).collect();
        let want: Vec<&str> = catalog.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, want);
    }

    /// 没有 settings 时（浏览器 companion 走这条）列通用三条，插入的是 config
    /// 里写的那个名字——`/protocol responses` 得能原样再解析回来。
    #[test]
    fn protocol_args_list_every_wire_without_settings() {
        let args = protocol_args(None);
        let names: Vec<&str> = args.iter().map(|a| a.display.as_str()).collect();
        assert_eq!(names, vec!["responses", "chat_completions", "messages"]);
        for item in &args {
            assert_eq!(
                ApiBackend::from_name(&item.insert_text),
                Some(ApiBackend::from_name(&item.display).unwrap())
            );
        }
    }
}
