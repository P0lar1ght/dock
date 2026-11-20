//! 找、读 hooks 配置。Dock 自己的 `hooks.json` 一直读；Claude Code / Codex 的按用户级
//! `config.toml` 的 `[hooks]` 开关读。三家的外形一样：
//!
//! ```json
//! { "hooks": { "PreToolUse": [ { "matcher": "Bash|Edit", "hooks": [
//!     { "type": "command", "command": "./check.sh", "timeout": 30 } ] } ] } }
//! ```
//!
//! 差别只在 matcher 怎么解读：Claude Code / Dock 是整串匹配（`Edit|Write`、`mcp__.*`），
//! Codex 是不锚定的正则。只跑 `command` 类型；`http` / `prompt` / `agent` / `async` 的
//! 跳过并记一句。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cordis_base::config::HooksImport;
use serde_json::Value;

/// 认得的事件（Claude Code 的名字）。
pub const EVENTS: [&str; 6] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "SubagentStop",
];

/// 没写 `timeout` 时等多久（Claude Code / Codex 的默认值）。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

// Hooks block an agent step: allow long checks, but never wait more than one day.
const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dialect {
    Dock,
    ClaudeCode,
    Codex,
}

impl Dialect {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dock => "Dock",
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CommandHook {
    pub command: String,
    pub timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct Group {
    /// 空 = 这一类事件全都匹配。
    pub matcher: String,
    pub hooks: Vec<CommandHook>,
}

/// 一份配置文件。
#[derive(Clone, Debug)]
pub struct Source {
    pub path: PathBuf,
    pub dialect: Dialect,
    /// 项目里的文件：第一次跑之前要用户点头（见 `trust`）。
    pub project: bool,
    /// 文件内容的 sha256：信任按它记，文件改了要重新点头。
    pub digest: String,
    pub events: BTreeMap<String, Vec<Group>>,
    pub warnings: Vec<String>,
}

impl Source {
    /// 这一份里配了几条命令。
    pub fn command_count(&self) -> usize {
        self.events
            .values()
            .flat_map(|groups| groups.iter())
            .map(|g| g.hooks.len())
            .sum()
    }

    /// 这个事件、这些名字匹配上的命令，按配置顺序。
    pub fn select(&self, event: &str, names: &[&str]) -> Vec<CommandHook> {
        self.events
            .get(event)
            .map(|groups| {
                groups
                    .iter()
                    .filter(|g| matches(&g.matcher, self.dialect, names))
                    .flat_map(|g| g.hooks.iter().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// 该读的文件，用户级在前、项目级在后。
pub fn candidates(
    home: &Path,
    project: &Path,
    import: HooksImport,
) -> Vec<(PathBuf, Dialect, bool)> {
    let dock_home = cordis_base::config::dock_home();
    let mut out = vec![(dock_home.join("hooks.json"), Dialect::Dock, false)];
    if import.claude_code {
        out.push((
            home.join(".claude/settings.json"),
            Dialect::ClaudeCode,
            false,
        ));
    }
    if import.codex {
        out.push((home.join(".codex/hooks.json"), Dialect::Codex, false));
    }
    out.push((project.join(".dock/hooks.json"), Dialect::Dock, true));
    if import.claude_code {
        out.push((
            project.join(".claude/settings.json"),
            Dialect::ClaudeCode,
            true,
        ));
        out.push((
            project.join(".claude/settings.local.json"),
            Dialect::ClaudeCode,
            true,
        ));
    }
    if import.codex {
        out.push((project.join(".codex/hooks.json"), Dialect::Codex, true));
    }
    // 项目就是 home（在家目录里开会话）：同一个文件别读两遍。
    let mut seen = std::collections::HashSet::new();
    out.retain(|(p, _, _)| seen.insert(p.clone()));
    out
}

/// 读所有存在的配置。读不起来的也返回（带原因），好让 `/hooks` 说出来。
pub fn discover(
    home: &Path,
    project: &Path,
    import: HooksImport,
) -> Vec<Result<Source, (PathBuf, String)>> {
    candidates(home, project, import)
        .into_iter()
        .filter_map(|(path, dialect, project)| {
            let raw = std::fs::read_to_string(&path).ok()?;
            Some(parse(&path, &raw, dialect, project).map_err(|e| (path, e)))
        })
        .collect()
}

pub fn parse(path: &Path, raw: &str, dialect: Dialect, project: bool) -> Result<Source, String> {
    let top: Value = serde_json::from_str(raw).map_err(|e| format!("不是合法的 JSON：{e}"))?;
    let hooks = match top.get("hooks") {
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return Err("hooks 要是一个对象".into()),
        // Claude Code 的 settings.json 没有 hooks 键 = 没配。
        None if dialect == Dialect::ClaudeCode => Default::default(),
        // hooks.json 也可以直接是事件表。
        None => match &top {
            Value::Object(map) if map.keys().any(|k| EVENTS.contains(&k.as_str())) => map.clone(),
            _ => Default::default(),
        },
    };
    let mut events = BTreeMap::new();
    let mut warnings = Vec::new();
    for (event, groups) in hooks {
        if !EVENTS.contains(&event.as_str()) {
            warnings.push(format!("{event}：Dock 没有对应的时机，跳过"));
            continue;
        }
        let Some(groups) = groups.as_array() else {
            warnings.push(format!("{event}：要是一个数组"));
            continue;
        };
        let mut parsed = Vec::new();
        for group in groups {
            let matcher = group
                .get("matcher")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if let Err(e) = compile(&matcher, dialect) {
                warnings.push(format!(
                    "{event}：matcher {matcher:?} 不是合法的正则（{e}），这一组不会触发"
                ));
                continue;
            }
            let mut hooks = Vec::new();
            for hook in group
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let kind = hook
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("command");
                if kind != "command" {
                    warnings.push(format!("{event}：只跑 command 类型的 hook，跳过 {kind}"));
                    continue;
                }
                if hook.get("async").and_then(Value::as_bool) == Some(true) {
                    warnings.push(format!("{event}：async hook 不支持，跳过"));
                    continue;
                }
                let Some(command) = hook
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                else {
                    warnings.push(format!("{event}：有一条 hook 没写 command"));
                    continue;
                };
                let timeout = match hook
                    .get("timeout")
                    .and_then(Value::as_f64)
                    .filter(|t| *t > 0.0)
                {
                    None => DEFAULT_TIMEOUT,
                    Some(seconds) => match Duration::try_from_secs_f64(seconds)
                        .ok()
                        .filter(|t| !t.is_zero() && *t <= MAX_TIMEOUT)
                    {
                        Some(timeout) => timeout,
                        None => {
                            warnings.push(format!(
                                "{event}：timeout 必须大于 0 且不超过 86400 秒（至少 1 纳秒），跳过命令 {command:?}"
                            ));
                            continue;
                        }
                    },
                };
                hooks.push(CommandHook {
                    command: command.to_string(),
                    timeout,
                });
            }
            if !hooks.is_empty() {
                parsed.push(Group { matcher, hooks });
            }
        }
        if !parsed.is_empty() {
            events.insert(event, parsed);
        }
    }
    Ok(Source {
        path: path.to_path_buf(),
        dialect,
        project,
        digest: super::trust::digest(raw),
        events,
        warnings,
    })
}

fn compile(matcher: &str, dialect: Dialect) -> Result<Option<regex::Regex>, regex::Error> {
    let m = matcher.trim();
    if m.is_empty() || m == "*" {
        return Ok(None);
    }
    let pattern = match dialect {
        Dialect::Codex => m.to_string(),
        Dialect::Dock | Dialect::ClaudeCode => format!("^(?:{m})$"),
    };
    regex::Regex::new(&pattern).map(Some)
}

/// `names` 里任何一个匹配上就算（工具既按 Claude 的名字 `Bash` 也按 Dock 的 `bash` 认）。
pub fn matches(matcher: &str, dialect: Dialect, names: &[&str]) -> bool {
    match compile(matcher, dialect) {
        Ok(None) => true,
        Ok(Some(re)) => names.iter().any(|n| re.is_match(n)),
        Err(_) => false,
    }
}

/// Dock 工具名 → Claude Code 的叫法（照 Claude 写的 matcher 和脚本能直接用）。
pub fn claude_tool_name(dock: &str) -> String {
    match dock {
        "bash" => "Bash".into(),
        "read_file" => "Read".into(),
        "write_file" => "Write".into(),
        "search_replace" => "Edit".into(),
        "glob" => "Glob".into(),
        "grep" => "Grep".into(),
        "list_dir" => "LS".into(),
        "web_fetch" => "WebFetch".into(),
        "web_search" => "WebSearch".into(),
        "task" => "Task".into(),
        "todo_write" => "TodoWrite".into(),
        // `mcp_<server>__<tool>` → `mcp__<server>__<tool>`
        other => match other.strip_prefix("mcp_") {
            Some(rest) if rest.contains("__") => format!("mcp__{rest}"),
            _ => other.to_string(),
        },
    }
}

/// 工具参数照 Claude Code 的键名补一份（原来的键留着）：`read_file` 的 `target_file`
/// → `file_path`，`write_file` 的 `contents` → `content`。
pub fn claude_tool_input(dock: &str, args: &Value) -> Value {
    let mut out = args.clone();
    let Some(obj) = out.as_object_mut() else {
        return out;
    };
    let alias = |obj: &mut serde_json::Map<String, Value>, from: &str, to: &str| {
        if !obj.contains_key(to) {
            if let Some(v) = obj.get(from).cloned() {
                obj.insert(to.to_string(), v);
            }
        }
    };
    match dock {
        "read_file" => alias(obj, "target_file", "file_path"),
        "write_file" => alias(obj, "contents", "content"),
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(raw: &str, dialect: Dialect) -> Source {
        parse(Path::new("/x/hooks.json"), raw, dialect, false).unwrap()
    }

    #[test]
    fn claude_matchers_match_the_whole_name_codex_matchers_do_not() {
        let names = ["Bash", "bash"];
        assert!(matches("Bash", Dialect::ClaudeCode, &names));
        assert!(matches("Edit|Bash", Dialect::ClaudeCode, &names));
        assert!(!matches("Bas", Dialect::ClaudeCode, &names));
        assert!(matches("Bas", Dialect::Codex, &names));
        assert!(matches("", Dialect::ClaudeCode, &names));
        assert!(matches("*", Dialect::Dock, &names));
        assert!(matches(
            "mcp__.*",
            Dialect::Dock,
            &["mcp__fs__read", "mcp_fs__read"]
        ));
    }

    #[test]
    fn parses_settings_and_bare_event_tables_and_skips_what_it_cannot_run() {
        let s = src(
            r#"{ "model": "x", "hooks": {
                "PreToolUse": [ { "matcher": "Bash", "hooks": [
                    { "type": "command", "command": "./a.sh", "timeout": 5 },
                    { "type": "prompt", "prompt": "?" },
                    { "type": "command", "command": "./b.sh", "async": true } ] } ],
                "Notification": [ { "hooks": [ { "type": "command", "command": "x" } ] } ],
                "Stop": [ { "matcher": "(", "hooks": [ { "type": "command", "command": "y" } ] } ]
            } }"#,
            Dialect::ClaudeCode,
        );
        let pre = s.select("PreToolUse", &["Bash"]);
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0].command, "./a.sh");
        assert_eq!(pre[0].timeout, Duration::from_secs(5));
        assert!(s.select("PreToolUse", &["Edit"]).is_empty());
        assert!(!s.events.contains_key("Stop"), "坏 matcher 的组不进来");
        assert_eq!(s.warnings.len(), 4, "{:?}", s.warnings);

        let bare = src(
            r#"{ "Stop": [ { "hooks": [ { "command": "./done.sh" } ] } ] }"#,
            Dialect::Dock,
        );
        assert_eq!(bare.select("Stop", &[]).len(), 1);
        assert_eq!(bare.select("Stop", &[])[0].timeout, DEFAULT_TIMEOUT);
        assert_eq!(
            src(r#"{ "model": "x" }"#, Dialect::ClaudeCode).command_count(),
            0
        );
    }

    /// 直接解析未信任的项目 JSON：旧版在 1e20 / 1e308 上先 panic，根本走不到确认。
    #[test]
    fn project_timeouts_are_checked_before_trust_without_panicking() {
        for dialect in [Dialect::Dock, Dialect::ClaudeCode, Dialect::Codex] {
            for literal in ["1e20", "1e308", "86400.001", "1e-20"] {
                let raw = format!(
                    r#"{{"hooks":{{"Stop":[{{"hooks":[
                        {{"command":"./bad.sh","timeout":{literal}}},
                        {{"command":"./good.sh","timeout":0.25}}
                    ]}}]}}}}"#
                );
                let source = parse(Path::new("/project/.dock/hooks.json"), &raw, dialect, true)
                    .expect("坏 timeout 只跳过这一条，不丢掉整份配置");
                assert!(source.project);
                let jobs = source.select("Stop", &[]);
                assert_eq!(jobs.len(), 1, "{dialect:?}: {literal}");
                assert_eq!(jobs[0].command, "./good.sh");
                assert_eq!(jobs[0].timeout, Duration::from_millis(250));
                assert_eq!(source.warnings, vec![
                    "Stop：timeout 必须大于 0 且不超过 86400 秒（至少 1 纳秒），跳过命令 \"./bad.sh\""
                ]);
            }
        }
    }

    #[test]
    fn hook_timeout_boundaries_and_legacy_defaults_are_preserved() {
        for (field, expected) in [
            ("", DEFAULT_TIMEOUT),
            (r#", "timeout": 0"#, DEFAULT_TIMEOUT),
            (r#", "timeout": -1"#, DEFAULT_TIMEOUT),
            (r#", "timeout": "bad""#, DEFAULT_TIMEOUT),
            (r#", "timeout": 0.000000001"#, Duration::from_nanos(1)),
            (r#", "timeout": 86400"#, MAX_TIMEOUT),
        ] {
            let raw = format!(r#"{{"Stop":[{{"hooks":[{{"command":"ok"{field}}}]}}]}}"#);
            let source = src(&raw, Dialect::Dock);
            let jobs = source.select("Stop", &[]);
            assert_eq!(jobs.len(), 1, "{field}");
            assert_eq!(jobs[0].timeout, expected, "{field}");
            assert_eq!(source.warnings, Vec::<String>::new(), "{field}");
        }
    }

    #[test]
    fn dock_tool_names_and_inputs_are_offered_in_claude_terms() {
        assert_eq!(claude_tool_name("bash"), "Bash");
        assert_eq!(claude_tool_name("search_replace"), "Edit");
        assert_eq!(
            claude_tool_name("mcp_github__create_issue"),
            "mcp__github__create_issue"
        );
        assert_eq!(claude_tool_name("cordis_plugin"), "cordis_plugin");
        let input = claude_tool_input("read_file", &serde_json::json!({ "target_file": "a.rs" }));
        assert_eq!(input["file_path"], "a.rs");
        assert_eq!(input["target_file"], "a.rs");
    }

    #[test]
    fn project_files_and_imports_follow_the_switches() {
        let home = Path::new("/h");
        let project = Path::new("/p");
        let none = candidates(home, project, HooksImport::default());
        assert!(none.iter().all(|(_, d, _)| *d == Dialect::Dock));
        assert!(none
            .iter()
            .any(|(p, _, proj)| *proj && p == Path::new("/p/.dock/hooks.json")));
        let all = candidates(
            home,
            project,
            HooksImport {
                claude_code: true,
                codex: true,
            },
        );
        assert!(all
            .iter()
            .any(|(p, _, proj)| !*proj && p == Path::new("/h/.claude/settings.json")));
        assert!(all
            .iter()
            .any(|(p, _, proj)| *proj && p == Path::new("/p/.claude/settings.local.json")));
        assert!(all
            .iter()
            .any(|(p, _, proj)| *proj && p == Path::new("/p/.codex/hooks.json")));
    }
}
