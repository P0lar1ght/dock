//! Named `"slash"` service：斜杠命令的唯一一张表。
//!
//! 两种条目：
//! - **命令**（[`SlashCommand`]）：各功能插件挂载时 [`Slash::register_command`]。
//!   [`SlashSurface::Host`] 带 handler，在 spine 里跑，TUI、网关、GUI 拿到的是同一个
//!   [`SlashOutcome`]，各自决定怎么画；[`SlashSurface::Terminal`] 只有声明它的终端
//!   会跑（开浮层、退出、复制……），别的客户端只列出来。
//! - **附加命令**（[`SlashEntry`]）：动态包、技能、工作流登记的 prompt / overlay /
//!   slot / tool 四种。只能加，不能盖掉命令：撞名的注册直接失败，命令后注册时
//!   把已有的同名附加命令藏起来——命令总是赢，跟谁先挂载无关。
//!
//! 内建命令的名字不再写死在一张保留表里：表里有谁，谁就是保留的。

use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use indexmap::IndexMap;
use serde_json::Value;

use crate::agent::runtime::BoxFuture;
use crate::names::{SLASH, TOOLS};
use crate::{ToolCall, Tools};

/// 命令在哪一端执行。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlashSurface {
    /// 宿主无关：handler 在 spine 里跑，任何客户端都能用。
    Host,
    /// 只有声明它的终端 UI 会跑；别的客户端列出来但跑不了。
    Terminal,
}

/// 子命令提示（`/goal pause` …），客户端补全用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlashHint {
    pub name: String,
    pub description: String,
    pub takes_args: bool,
}

/// 一次斜杠命令的结果。客户端按自己的方式呈现：TUI 闪一行 / 开文本浮层 / 填输入框，
/// 网关投成 dock.1 的 `applied` / `notice` / `filled` / `submitted` / `menu`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SlashOutcome {
    /// 做完了，一句话回执。
    Applied(String),
    /// 一段要给人看的正文。
    Notice { title: String, body: String },
    /// 填进输入框，发不发由人决定。
    Fill(String),
    /// 作为用户消息发出去。
    Submit(String),
    /// 让客户端打开它自己的选择器（`model` / `reasoning` / `context` / `goal`）。
    Menu(String),
    /// 打开动态包的终端插槽（`kind: slot`），载荷是插槽 id。
    OpenSlot(String),
    /// 只有终端能跑的命令（[`SlashSurface::Terminal`]），载荷是命令名。
    TerminalOnly(String),
}

impl SlashOutcome {
    pub fn notice(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self::Notice {
            title: title.into(),
            body: body.into(),
        }
    }
}

/// 命令体。收到的是**调用方那一页**的 ctx（TUI 当前页、网关按 `threadId` 解出的页），
/// 在里面 live-lookup 服务。
pub type SlashHandler =
    Arc<dyn Fn(Context, String) -> BoxFuture<'static, SlashOutcome> + Send + Sync>;

/// 同步命令体的便捷写法。
pub fn slash_handler(
    f: impl Fn(&Context, &str) -> SlashOutcome + Send + Sync + 'static,
) -> SlashHandler {
    Arc::new(move |ctx, args| {
        let out = f(&ctx, &args);
        Box::pin(async move { out })
    })
}

/// 表里的一条命令。
#[derive(Clone)]
pub struct SlashCommand {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub takes_args: bool,
    pub surface: SlashSurface,
    pub hints: Vec<SlashHint>,
    pub handler: Option<SlashHandler>,
}

impl std::fmt::Debug for SlashCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlashCommand")
            .field("name", &self.name)
            .field("aliases", &self.aliases)
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl SlashCommand {
    /// 宿主无关的命令：handler 在 spine 里跑。
    pub fn host(name: &str, description: &str, handler: SlashHandler) -> Self {
        Self {
            name: name.into(),
            aliases: Vec::new(),
            description: description.into(),
            takes_args: false,
            surface: SlashSurface::Host,
            hints: Vec::new(),
            handler: Some(handler),
        }
    }

    /// 只有终端会跑的命令：这里只登记名字与说明。
    pub fn terminal(name: &str, description: &str) -> Self {
        Self {
            name: name.into(),
            aliases: Vec::new(),
            description: description.into(),
            takes_args: false,
            surface: SlashSurface::Terminal,
            hints: Vec::new(),
            handler: None,
        }
    }

    pub fn aliases(mut self, aliases: &[&str]) -> Self {
        self.aliases = aliases.iter().map(|a| a.to_string()).collect();
        self
    }

    pub fn takes_args(mut self, takes_args: bool) -> Self {
        self.takes_args = takes_args;
        self
    }

    pub fn hint(mut self, name: &str, description: &str, takes_args: bool) -> Self {
        self.hints.push(SlashHint {
            name: name.into(),
            description: description.into(),
            takes_args,
        });
        self
    }

    pub fn display(&self) -> String {
        format!("/{}", self.name)
    }

    fn answers_to(&self, name: &str) -> bool {
        self.name == name || self.aliases.iter().any(|a| a == name)
    }
}

/// [`Slash::resolve`] 的结果。
#[derive(Clone, Debug)]
pub enum SlashResolved {
    Command(SlashCommand),
    Extra(SlashEntry),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtraSlashKind {
    Prompt,
    Overlay,
    Slot,
    /// Run a live model tool (`text` = tool name); typed args become JSON.
    Tool,
}

impl ExtraSlashKind {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            "prompt" => Ok(Self::Prompt),
            "overlay" => Ok(Self::Overlay),
            "slot" => Ok(Self::Slot),
            "tool" => Ok(Self::Tool),
            other => Err(format!(
                "slash kind must be \"prompt\", \"overlay\", \"slot\", or \"tool\", got {other:?}"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Overlay => "overlay",
            Self::Slot => "slot",
            Self::Tool => "tool",
        }
    }
}

/// Extra prompt-bar command. Additive only: name must not collide with builtins.
#[derive(Clone, Debug)]
pub struct SlashEntry {
    pub command: String,
    pub description: String,
    pub kind: ExtraSlashKind,
    pub text: String,
    pub title: String,
    pub send: bool,
}

impl SlashEntry {
    pub fn display(&self) -> String {
        format!("/{}", self.command)
    }

    pub fn expand(&self, args: &str) -> String {
        let args = args.trim();
        if self.text.contains("{args}") {
            self.text.replace("{args}", args)
        } else if args.is_empty() {
            self.text.clone()
        } else {
            format!("{} {args}", self.text)
        }
    }

    pub fn overlay_title(&self) -> String {
        if self.title.trim().is_empty() {
            self.display()
        } else {
            self.title.clone()
        }
    }

    pub fn tool_title(&self) -> String {
        if self.title.trim().is_empty() {
            format!("{} → {}", self.display(), self.text.trim())
        } else {
            self.title.clone()
        }
    }
}

/// Map typed slash args to a tool `arguments` JSON string.
/// Empty → `{}`. Leading `{` → raw JSON object. Otherwise `{"args":"<text>"}`.
pub fn tool_slash_arguments(args: &str) -> String {
    let args = args.trim();
    if args.is_empty() {
        return "{}".into();
    }
    if args.starts_with('{') {
        return args.to_string();
    }
    serde_json::json!({ "args": args }).to_string()
}

/// Named `slash` service. Duplicate names throw; extras cannot shadow commands.
/// Disposed with the calling fiber when the handle is owned.
pub struct Slash {
    commands: Arc<Mutex<IndexMap<String, SlashCommand>>>,
    extra: Arc<Mutex<IndexMap<String, SlashEntry>>>,
    pending_notice: Arc<Mutex<Option<(String, String)>>>,
}

impl Slash {
    pub fn new() -> Self {
        Self {
            commands: Arc::new(Mutex::new(IndexMap::new())),
            extra: Arc::new(Mutex::new(IndexMap::new())),
            pending_notice: Arc::new(Mutex::new(None)),
        }
    }

    /// 登记一条命令。名字或别名与已有命令撞了就失败；已有的同名附加命令被藏起来。
    pub fn register_command(&self, command: SlashCommand) -> cordis::Result<Disposable> {
        let name = command.name.trim().trim_start_matches('/').to_string();
        if name.is_empty() {
            return Err(cordis::Error::plugin("slash command must be non-empty"));
        }
        if command.surface == SlashSurface::Host && command.handler.is_none() {
            return Err(cordis::Error::plugin(format!(
                "host slash /{name} needs a handler"
            )));
        }
        // 别名和主名一样规整（`/m` → `m`），否则永远匹配不上。
        let aliases = command
            .aliases
            .iter()
            .map(|a| a.trim().trim_start_matches('/').to_string())
            .filter(|a| !a.is_empty())
            .collect();
        let command = SlashCommand {
            name: name.clone(),
            aliases,
            ..command
        };
        {
            let mut commands = self.commands.lock().unwrap();
            let tokens = std::iter::once(&command.name).chain(&command.aliases);
            for token in tokens {
                if commands.values().any(|c| c.answers_to(token)) {
                    return Err(cordis::Error::plugin(format!(
                        "duplicate builtin slash /{token}"
                    )));
                }
            }
            commands.insert(name.clone(), command);
        }
        let commands = self.commands.clone();
        Ok(Disposable::from_fn(move || {
            commands.lock().unwrap().shift_remove(&name);
        }))
    }

    /// 表里的命令，按登记顺序。
    pub fn commands(&self) -> Vec<SlashCommand> {
        self.commands.lock().unwrap().values().cloned().collect()
    }

    /// `name` 是不是某条命令的名字或别名（附加命令不算）。
    pub fn is_builtin(&self, name: &str) -> bool {
        let name = name.trim().trim_start_matches('/');
        self.commands
            .lock()
            .unwrap()
            .values()
            .any(|c| c.answers_to(name))
    }

    /// 按名字或别名找一条：命令优先，其次附加命令。
    pub fn resolve(&self, name: &str) -> Option<SlashResolved> {
        let name = name.trim().trim_start_matches('/');
        if let Some(command) = self
            .commands
            .lock()
            .unwrap()
            .values()
            .find(|c| c.answers_to(name))
        {
            return Some(SlashResolved::Command(command.clone()));
        }
        self.extra
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .map(SlashResolved::Extra)
    }

    /// 在 `page` 那一页上跑 `/name args`。不认识的名字回 `None`（当普通消息发）。
    pub async fn run(&self, page: &Context, name: &str, args: &str) -> Option<SlashOutcome> {
        let args = args.trim();
        match self.resolve(name)? {
            SlashResolved::Command(command) => Some(match command.handler {
                Some(handler) => handler(page.clone(), args.to_string()).await,
                None => SlashOutcome::TerminalOnly(command.name),
            }),
            SlashResolved::Extra(entry) => Some(run_extra(page, &entry, args).await),
        }
    }

    pub fn register(&self, entry: SlashEntry) -> cordis::Result<Disposable> {
        let command = normalize_command(&entry.command).map_err(cordis::Error::plugin)?;
        if self.is_builtin(&command) {
            return Err(cordis::Error::plugin(format!(
                "cannot shadow builtin slash /{command}"
            )));
        }
        if entry.kind == ExtraSlashKind::Tool && entry.text.trim().is_empty() {
            return Err(cordis::Error::plugin(
                "slash kind \"tool\" needs non-empty text (tool name)".to_string(),
            ));
        }
        let entry = SlashEntry {
            command: command.clone(),
            ..entry
        };
        {
            let mut extra = self.extra.lock().unwrap();
            if extra.contains_key(&command) {
                return Err(cordis::Error::plugin(format!("duplicate slash /{command}")));
            }
            extra.insert(command.clone(), entry);
        }
        let extra = self.extra.clone();
        Ok(Disposable::from_fn(move || {
            extra.lock().unwrap().shift_remove(&command);
        }))
    }

    /// `/help` 的正文：命令一行一条，附加命令跟在后面。
    pub fn help_body(&self) -> String {
        let mut lines: Vec<String> = self
            .commands()
            .iter()
            .map(|c| format!("{}  {}", c.display(), c.description))
            .collect();
        for entry in self.list() {
            lines.push(format!("{}  {}", entry.display(), entry.description));
        }
        lines.join("\n")
    }

    /// 附加命令（被同名命令盖住的不列）。
    pub fn list(&self) -> Vec<SlashEntry> {
        self.extra
            .lock()
            .unwrap()
            .values()
            .filter(|e| !self.is_builtin(&e.command))
            .cloned()
            .collect()
    }

    /// Update an existing overlay/prompt extra's description + body (e.g. /browser status).
    pub fn update_overlay(
        &self,
        command: &str,
        description: impl Into<String>,
        text: impl Into<String>,
        title: impl Into<String>,
    ) -> bool {
        let Ok(command) = normalize_command(command) else {
            return false;
        };
        let mut extra = self.extra.lock().unwrap();
        let Some(entry) = extra.get_mut(&command) else {
            return false;
        };
        entry.description = description.into();
        entry.text = text.into();
        entry.title = title.into();
        true
    }

    /// Queue a Notice for the TUI (e.g. after an async `kind: tool` run).
    pub fn queue_notice(&self, title: impl Into<String>, body: impl Into<String>) {
        *self.pending_notice.lock().unwrap() = Some((title.into(), body.into()));
    }

    pub fn take_notice(&self) -> Option<(String, String)> {
        self.pending_notice.lock().unwrap().take()
    }
}

impl Default for Slash {
    fn default() -> Self {
        Self::new()
    }
}

/// 附加命令的执行：prompt 填 / 发，overlay 给正文，slot 交给终端，tool 现跑一次工具。
async fn run_extra(page: &Context, entry: &SlashEntry, args: &str) -> SlashOutcome {
    match entry.kind {
        ExtraSlashKind::Prompt => {
            let text = entry.expand(args);
            if entry.send {
                SlashOutcome::Submit(text)
            } else {
                SlashOutcome::Fill(text)
            }
        }
        ExtraSlashKind::Overlay => SlashOutcome::notice(entry.overlay_title(), entry.expand(args)),
        ExtraSlashKind::Slot => SlashOutcome::OpenSlot(entry.text.trim().to_string()),
        ExtraSlashKind::Tool => {
            let Some(tools) = page.get::<Tools>(TOOLS) else {
                return SlashOutcome::notice(entry.tool_title(), "tools 未挂载");
            };
            let arguments = match crate::extra_tool_slash_arguments(entry, args) {
                Ok(arguments) => arguments,
                Err(body) => return SlashOutcome::notice(entry.tool_title(), body),
            };
            let result = tools
                .execute(ToolCall {
                    id: format!("slash-tool-{}", entry.command),
                    name: entry.text.trim().to_string(),
                    arguments,
                })
                .await;
            SlashOutcome::notice(entry.tool_title(), result.content)
        }
    }
}

pub fn normalize_command(raw: &str) -> Result<String, String> {
    let n = raw.trim().trim_start_matches('/');
    if n.is_empty() {
        return Err("slash command must be non-empty".into());
    }
    let bytes = n.as_bytes();
    if !(1..=32).contains(&bytes.len()) {
        return Err("slash command must be 1–32 characters".into());
    }
    if !bytes[0].is_ascii_lowercase() {
        return Err("slash command must start with a lowercase English letter".into());
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
    {
        return Err(
            "slash command may contain only lowercase English letters, digits, and hyphen".into(),
        );
    }
    Ok(n.to_string())
}

/// Parse define-time slash fields. `factory` must already be `"slash"`.
pub fn slash_entry_from_define(v: &Value, purpose: &str) -> Result<SlashEntry, String> {
    let command = v
        .get("command")
        .and_then(Value::as_str)
        .ok_or("factory \"slash\" needs `command`")?;
    let command = normalize_command(command)?;
    let kind = ExtraSlashKind::parse(v.get("kind").and_then(Value::as_str).ok_or(
        "factory \"slash\" needs `kind` (\"prompt\", \"overlay\", \"slot\", or \"tool\")",
    )?)?;
    let text = v
        .get("text")
        .and_then(Value::as_str)
        .ok_or("factory \"slash\" needs `text`")?
        .to_string();
    if text.is_empty() {
        return Err("factory \"slash\" needs non-empty `text`".into());
    }
    if kind == ExtraSlashKind::Tool && text.trim().is_empty() {
        return Err("factory \"slash\" kind \"tool\" needs text = tool name".into());
    }
    let description = v
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let purpose = purpose.trim();
            if purpose.is_empty() {
                "自定义命令".into()
            } else {
                purpose.to_string()
            }
        });
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let send = v
        .get("send")
        .and_then(Value::as_bool)
        .unwrap_or(kind == ExtraSlashKind::Prompt);
    Ok(SlashEntry {
        command,
        description,
        kind,
        text,
        title,
        send,
    })
}

pub fn contrib_fields_present(v: &Value) -> bool {
    v.get("command").is_some() || v.get("kind").is_some() || v.get("text").is_some()
}

/// 挂 `"slash"` 表，并登记 `/help`（列出表里的全部命令）。
pub fn slash() -> Plugin {
    plugin("slash", Inject::new(), |ctx, _: &()| {
        let slash = Slash::new();
        let help = slash.register_command(SlashCommand::host(
            "help",
            "显示斜杠命令",
            slash_handler(|page, _| {
                let body = page
                    .get::<Slash>(SLASH)
                    .map(|slash| slash.help_body())
                    .unwrap_or_default();
                SlashOutcome::notice("斜杠命令", body)
            }),
        ))?;
        ctx.provide(SLASH, slash)?;
        crate::tools::registry::own_registered(ctx, vec![help])?;
        Ok(None)
    })
}

/// 功能插件登记自己的命令：随插件 fiber 一起注销。
pub fn register_commands(ctx: &Context, commands: Vec<SlashCommand>) -> cordis::Result<()> {
    let slash = ctx.require::<Slash>(SLASH)?;
    let mut owned = Vec::with_capacity(commands.len());
    for command in commands {
        owned.push(slash.register_command(command)?);
    }
    crate::tools::registry::own_registered(ctx, owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(command: &str) -> SlashEntry {
        SlashEntry {
            command: command.into(),
            description: String::new(),
            kind: ExtraSlashKind::Prompt,
            text: format!("run {command}"),
            title: String::new(),
            send: true,
        }
    }

    #[test]
    fn normalize_rejects_uppercase() {
        assert!(normalize_command("Help").is_err());
        assert_eq!(normalize_command("/standup").unwrap(), "standup");
    }

    /// 命令总是赢：附加命令后来撞名直接失败；先来的附加命令被后来的命令藏起来。
    #[test]
    fn commands_win_regardless_of_mount_order() {
        let slash = Slash::new();
        let _early = slash.register(prompt("copy")).unwrap();
        let _cmd = slash
            .register_command(SlashCommand::terminal("copy", "复制").aliases(&["cp"]))
            .unwrap();
        assert!(slash.list().is_empty(), "被命令盖住的附加命令不该再列出来");
        assert!(matches!(
            slash.resolve("copy"),
            Some(SlashResolved::Command(_))
        ));
        assert!(slash.register(prompt("cp")).is_err(), "别名也算命令的名字");
        assert!(slash
            .register_command(SlashCommand::terminal("dup", "x").aliases(&["copy"]))
            .is_err());
        assert!(slash.is_builtin("/cp"));
        assert!(!slash.is_builtin("standup"));
    }

    #[test]
    fn aliases_are_normalized_like_names() {
        let slash = Slash::new();
        let _m = slash
            .register_command(SlashCommand::terminal("/model", "模型").aliases(&[" /m ", ""]))
            .unwrap();
        assert!(slash.is_builtin("m"));
        assert_eq!(slash.commands()[0].aliases, vec!["m".to_string()]);
    }

    #[test]
    fn disposing_a_command_frees_its_name() {
        let slash = Slash::new();
        let cmd = slash
            .register_command(SlashCommand::terminal("quit", "退出"))
            .unwrap();
        cmd.dispose_sync();
        assert!(!slash.is_builtin("quit"));
        assert!(slash.register(prompt("quit")).is_ok());
    }

    #[test]
    fn host_commands_need_a_handler() {
        let slash = Slash::new();
        let mut cmd = SlashCommand::terminal("x", "x");
        cmd.surface = SlashSurface::Host;
        assert!(slash.register_command(cmd).is_err());
    }

    #[tokio::test]
    async fn run_dispatches_commands_terminal_and_extras() {
        let ctx = Context::new();
        let slash = Slash::new();
        let _h = slash
            .register_command(
                SlashCommand::host(
                    "echo",
                    "回声",
                    slash_handler(|_, args| SlashOutcome::Applied(args.to_string())),
                )
                .aliases(&["e"]),
            )
            .unwrap();
        let _t = slash
            .register_command(SlashCommand::terminal("find", "搜索"))
            .unwrap();
        let _p = slash.register(prompt("standup")).unwrap();

        assert_eq!(
            slash.run(&ctx, "e", "  hi  ").await,
            Some(SlashOutcome::Applied("hi".into()))
        );
        assert_eq!(
            slash.run(&ctx, "find", "").await,
            Some(SlashOutcome::TerminalOnly("find".into()))
        );
        assert_eq!(
            slash.run(&ctx, "standup", "today").await,
            Some(SlashOutcome::Submit("run standup today".into()))
        );
        assert_eq!(slash.run(&ctx, "nope", "").await, None);
    }

    #[tokio::test]
    async fn help_lists_commands_and_extras() {
        let root = Context::new();
        root.plugin(slash(), ()).unwrap().wait().await.unwrap();
        let slash = root.get::<Slash>(SLASH).unwrap();
        let _p = slash.register(prompt("standup")).unwrap();
        let Some(SlashOutcome::Notice { body, .. }) = slash.run(&root, "help", "").await else {
            panic!("/help 该给一段正文");
        };
        assert!(body.contains("/help"), "{body}");
        assert!(body.contains("/standup"), "{body}");
    }

    #[test]
    fn tool_kind_and_arguments() {
        assert_eq!(ExtraSlashKind::parse("tool").unwrap().as_str(), "tool");
        assert_eq!(tool_slash_arguments(""), "{}");
        assert_eq!(tool_slash_arguments("  {\"x\":1}  "), r#"{"x":1}"#);
        assert_eq!(
            tool_slash_arguments("hello"),
            serde_json::json!({ "args": "hello" }).to_string()
        );
    }
}
