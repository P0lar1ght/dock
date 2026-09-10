//! Named `"slash"` service. TUI live-looks it; dynamic factories register
//! extra commands. Builtins stay in the TUI catalog — extras cannot replace them.

use std::sync::{Arc, Mutex};

use cordis::{plugin, Disposable, Inject, Plugin};
use indexmap::IndexMap;
use serde_json::Value;

use crate::names::SLASH;

/// Keep in sync with `cordis-tui` `CATALOG` names and aliases. A TUI test
/// asserts every catalog token is listed here.
pub const RESERVED_SLASH: &[&str] = &[
    "settings",
    "config",
    "prefs",
    "new",
    "model",
    "m",
    "resume",
    "pair",
    "pairing",
    "loop",
    "cron",
    "plan",
    "view-plan",
    "show-plan",
    "plan-view",
    "goal",
    "tasks",
    "workflow",
    "mcps",
    "lsp",
    "cordis",
    "plugins",
    "preset",
    "presets",
    "agent",
    "agents",
    "history",
    "copy",
    "find",
    "usage",
    "cost",
    "context",
    "compact",
    "theme",
    "t",
    "timestamps",
    "think",
    "thinking",
    "effort",
    "export",
    "cd",
    "help",
    "quit",
    "exit",
];

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

/// Named `slash` service. Duplicate extra names throw. Builtins cannot be
/// shadowed. Disposed with the calling fiber when the handle is owned.
pub struct Slash {
    extra: Arc<Mutex<IndexMap<String, SlashEntry>>>,
    pending_notice: Arc<Mutex<Option<(String, String)>>>,
}

impl Slash {
    pub fn new() -> Self {
        Self {
            extra: Arc::new(Mutex::new(IndexMap::new())),
            pending_notice: Arc::new(Mutex::new(None)),
        }
    }

    pub fn register(&self, entry: SlashEntry) -> cordis::Result<Disposable> {
        let command = normalize_command(&entry.command).map_err(cordis::Error::plugin)?;
        if slash_name_reserved(&command) {
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

    pub fn list(&self) -> Vec<SlashEntry> {
        self.extra.lock().unwrap().values().cloned().collect()
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

pub fn slash_name_reserved(name: &str) -> bool {
    let n = name.trim().trim_start_matches('/');
    RESERVED_SLASH.contains(&n)
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
    if slash_name_reserved(n) {
        return Err(format!("cannot shadow builtin slash /{n}"));
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

pub fn slash() -> Plugin {
    plugin("slash", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(SLASH, Slash::new())?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_includes_help_and_aliases() {
        assert!(slash_name_reserved("help"));
        assert!(slash_name_reserved("/quit"));
        assert!(slash_name_reserved("exit"));
        assert!(slash_name_reserved("view-plan"));
        assert!(!slash_name_reserved("standup"));
    }

    #[test]
    fn normalize_rejects_uppercase_and_reserved() {
        assert!(normalize_command("Help").is_err());
        assert!(normalize_command("help").is_err());
        assert_eq!(normalize_command("/standup").unwrap(), "standup");
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
