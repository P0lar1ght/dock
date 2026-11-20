//! hooks 桥接：把 Claude Code / Codex 格式的 command hook 挂到 Dock 的流程钩子上。
//! 用户已经写好的 hooks 配置拿来就能用，不用学 Rhai（DSH 的 `hooks-claude-code` /
//! `hooks-codex` 是同一个思路，语义照它们对齐）。
//!
//! | hook | Dock 的时机 | 能做什么 |
//! | --- | --- | --- |
//! | `SessionStart` | 一个会话第一次采样前（`agent/step-start`） | 附加上下文 |
//! | `UserPromptSubmit` | 用户消息进来、第一次采样前（`agent/pre-step`） | 拦下这一轮，或附加上下文 |
//! | `PreToolUse` | 工具跑之前（`tools/pre-execute`） | 拒绝、要用户确认（自动批准下也问） |
//! | `PostToolUse` | 工具跑完（`tools/execute`） | 往结果后面补一段反馈 / 上下文 |
//! | `Stop` / `SubagentStop` | 回合想结束时（`agent/turn-end`） | 让它接着干，理由给模型看 |
//!
//! 读哪些文件见 [`config`]；项目里的文件第一次跑之前要点头，见 [`trust`]。`allow`
//! 不会绕过 Dock 的权限门——只能收紧，不能放宽。
//!
//! hook 在回合里同步跑，会卡住这一步直到它返回（和 Claude Code 一样）；命令按配置顺序
//! 一条条跑，超时（默认 10 分钟，`timeout` 改）连同它起的子进程一起杀掉。

pub mod config;
pub mod run;
pub mod trust;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};
use serde_json::{json, Value};

use crate::host::permissions::Permissions;
use crate::host::slash::{register_commands, slash_handler, SlashCommand, SlashOutcome};
use crate::names::{
    PERMISSIONS, PRE_STEP, SESSIONS, SLASH, STEP_START, TOOLS_EXECUTE, TOOLS_PRE_EXECUTE, TURN_END,
};
use crate::session::log::Sessions;
use cordis_base::types::{PreExecute, PreStep, StepStart, ToolResult, TurnEnd};

use config::{claude_tool_input, claude_tool_name, CommandHook, Source};
use run::{Decision, Outcome};

/// 排在内建（todo 10、goal 20）和 Rhai 钩子（50）后面。
const ORDER: i32 = 55;

/// `Stop` hook 一轮里最多让它接着干几次（hook 自己该看 `stop_hook_active` 收手，这是兜底）。
const MAX_STOP_ROUNDS: usize = 8;

struct Bridge {
    ctx: Context,
    /// `UserPromptSubmit` / `SessionStart` 给的上下文，等这一页下一次采样前送进去。
    pending: Mutex<HashMap<String, Vec<String>>>,
    /// 上一次回合结束是被 `Stop` hook 拦回去的（给 hook 的 `stop_hook_active`）。
    stop_active: Mutex<HashSet<String>>,
    /// 已经跑过 `SessionStart` 的会话（live session id）。
    started: Mutex<HashSet<String>>,
    /// 这个进程里用户拒绝过的项目 hooks 文件（路径 + 内容指纹）：不再问。
    refused: Mutex<HashSet<(PathBuf, String)>>,
}

impl Bridge {
    /// 发起这一步的那一页；不在任何一轮里就是根。
    fn page(&self) -> Context {
        crate::tools::registry::exec_ctx().unwrap_or_else(|| self.ctx.clone())
    }

    fn home() -> PathBuf {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    fn sources(project: &Path) -> Vec<Source> {
        config::discover(
            &Self::home(),
            project,
            cordis_base::config::load_hooks_import(),
        )
        .into_iter()
        .filter_map(|r| match r {
            Ok(s) => Some(s),
            Err((path, e)) => {
                eprintln!("[hooks] {}: {e}", path.display());
                None
            }
        })
        .collect()
    }

    /// 这个事件要跑的命令（按配置顺序）。项目里的文件只有点过头的才算；还没点过头、
    /// 这次又真有命令要跑的，在这一页上问一次。
    fn jobs(
        &self,
        page: &Context,
        project: &Path,
        event: &str,
        names: &[&str],
    ) -> Vec<CommandHook> {
        let mut out = Vec::new();
        for source in Self::sources(project) {
            let hooks = source.select(event, names);
            if hooks.is_empty() {
                continue;
            }
            if source.project && !self.trusted(page, &source) {
                continue;
            }
            out.extend(hooks);
        }
        out
    }

    fn trusted(&self, page: &Context, source: &Source) -> bool {
        if trust::is_trusted(&source.path, &source.digest) {
            return true;
        }
        let key = (source.path.clone(), source.digest.clone());
        if self.refused.lock().unwrap().contains(&key) {
            return false;
        }
        let approved = ask_trust(page, source);
        if approved {
            if let Err(e) = trust::remember(&source.path, &source.digest) {
                eprintln!("[hooks] 记不下信任：{e}");
            }
        } else {
            self.refused.lock().unwrap().insert(key);
        }
        approved
    }

    /// 跑一个事件上所有匹配的 hook，合并结论。
    fn fire(&self, event: &str, names: &[&str], extra: Value) -> Outcome {
        let page = self.page();
        let project = crate::session::cwd::session_cwd(&page);
        let jobs = self.jobs(&page, &project, event, names);
        if jobs.is_empty() {
            return Outcome::default();
        }
        let session_id = page
            .get::<Sessions>(SESSIONS)
            .map(|s| s.live_session_id())
            .unwrap_or_default();
        let mut input = json!({
            "session_id": session_id,
            // Dock 的会话日志不是 Claude 的 transcript 格式，给空串（DSH 也是）。
            "transcript_path": "",
            "cwd": project.display().to_string(),
            "hook_event_name": event,
        });
        if let (Some(obj), Value::Object(extra)) = (input.as_object_mut(), extra) {
            obj.extend(extra);
        }
        let outcome = blocking(|| {
            let mut merged = Outcome::default();
            for job in &jobs {
                let r = run::run_command(&job.command, &input, &project, &project, job.timeout);
                merged.merge(run::decode(event, &job.command, &r));
            }
            merged
        });
        for e in &outcome.errors {
            eprintln!("[hooks] {event}: {e}");
        }
        if let Some(reason) = &outcome.stop {
            eprintln!("[hooks] {event}: hook 要求停下（continue: false）：{reason}");
        }
        outcome
    }

    fn queue_context(&self, identity: &str, context: Vec<String>) {
        if context.is_empty() {
            return;
        }
        self.pending
            .lock()
            .unwrap()
            .entry(identity.to_string())
            .or_default()
            .extend(context);
    }
}

/// 多线程运行时上让出 worker 再跑（hook 会阻塞）；单线程（测试）就地跑。
fn blocking<R>(f: impl FnOnce() -> R) -> R {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

/// 在这一页上问：要不要跑这个项目文件里的 hooks。自动批准、「始终允许」都不算数。
fn ask_trust(page: &Context, source: &Source) -> bool {
    let Some(perms) = page.get::<Permissions>(PERMISSIONS) else {
        return false;
    };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
        eprintln!(
            "[hooks] {} 还没确认过，这里问不了，先不跑",
            source.path.display()
        );
        return false;
    }
    let mut lines = vec![format!(
        "运行项目里的 hooks（{} 格式）：{}\n这些命令会在本机直接跑：",
        source.dialect.label(),
        source.path.display()
    )];
    let mut shown = 0;
    for (event, groups) in &source.events {
        for g in groups {
            for h in &g.hooks {
                if shown < 8 {
                    let matcher = if g.matcher.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", g.matcher)
                    };
                    lines.push(format!(
                        "{event}{matcher}: {}（超时 {} 秒）",
                        h.command,
                        h.timeout.as_secs_f64()
                    ));
                }
                shown += 1;
            }
        }
    }
    if shown > 8 {
        lines.push(format!("…另有 {} 条", shown - 8));
    }
    let key = format!("hooks {}", source.path.display());
    let summary = lines.join("\n");
    tokio::task::block_in_place(|| handle.block_on(perms.request_strict(&key, &summary, None)))
}

fn parse_args(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or(Value::Null)
}

pub fn hooks_bridge() -> Plugin {
    plugin("hooks-bridge", Inject::new(), |ctx, _: &()| {
        let bridge = Arc::new(Bridge {
            ctx: ctx.clone(),
            pending: Mutex::default(),
            stop_active: Mutex::default(),
            started: Mutex::default(),
            refused: Mutex::default(),
        });
        let mut owned = Vec::new();

        let b = bridge.clone();
        owned.push(
            ctx.on_waterfall(TOOLS_PRE_EXECUTE, move |pre: PreExecute, args| {
                let mut next = args.next::<PreExecute>().unwrap_or(pre);
                if next.denial().is_some() {
                    return next;
                }
                let dock = next.name().to_string();
                let claude = claude_tool_name(&dock);
                let input = json!({
                    "tool_name": claude,
                    "tool_input": claude_tool_input(&dock, &parse_args(next.arguments())),
                });
                let out = b.fire("PreToolUse", &[&claude, &dock], input);
                match (out.block, out.decision) {
                    (Some(reason), _) | (None, Decision::Deny(reason)) => {
                        next.deny(format!("{reason}（PreToolUse hook）"));
                    }
                    (None, Decision::Ask(reason)) => {
                        next.ask(format!("{reason}（PreToolUse hook）"))
                    }
                    _ => {}
                }
                next
            })?,
        );

        let b = bridge.clone();
        owned.push(
            ctx.on_waterfall(TOOLS_EXECUTE, move |result: ToolResult, args| {
                let mut next = args.next::<ToolResult>().unwrap_or(result);
                let dock = next.name.clone();
                let claude = claude_tool_name(&dock);
                let arguments = crate::tools::registry::call_arguments().unwrap_or_default();
                let input = json!({
                    "tool_name": claude,
                    "tool_input": claude_tool_input(&dock, &parse_args(&arguments)),
                    "tool_response": { "output": next.content, "is_error": next.is_error },
                });
                let out = b.fire("PostToolUse", &[&claude, &dock], input);
                for note in out.block.into_iter().chain(out.context) {
                    next.content
                        .push_str(&format!("\n\n[PostToolUse hook] {note}"));
                }
                next
            })?,
        );

        let b = bridge.clone();
        owned.push(ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
            let mut next = args.next::<PreStep>().unwrap_or(step);
            if !next.enter || next.denial().is_some() {
                return next;
            }
            b.stop_active.lock().unwrap().remove(&next.identity);
            let out = b.fire("UserPromptSubmit", &[], json!({ "prompt": next.user }));
            match out.block {
                Some(reason) => next.deny(format!("{reason}（UserPromptSubmit hook）")),
                None => b.queue_context(&next.identity, out.context),
            }
            next
        })?);

        let b = bridge.clone();
        owned.push(ctx.on_waterfall(STEP_START, move |start: StepStart, args| {
            let mut next = args.next::<StepStart>().unwrap_or(start);
            if next.step == 0 {
                let page = b.page();
                if let Some(sessions) = page.get::<Sessions>(SESSIONS) {
                    let sid = sessions.live_session_id();
                    if !sid.is_empty() && b.started.lock().unwrap().insert(sid) {
                        let turns = sessions.with_log(|events, _| {
                            events
                                .iter()
                                .filter(|e| matches!(e, cordis_base::types::LogEvent::User(_)))
                                .count()
                        });
                        let source = if turns > 1 { "resume" } else { "startup" };
                        let out = b.fire("SessionStart", &[source], json!({ "source": source }));
                        b.queue_context(&next.identity, out.context);
                    }
                }
            }
            if let Some(context) = b.pending.lock().unwrap().remove(&next.identity) {
                next.remind(
                    ORDER,
                    crate::tools::task::format::wrap_reminder(&format!(
                        "hooks 附加的上下文：\n{}",
                        context.join("\n\n")
                    )),
                );
            }
            next
        })?);

        let b = bridge.clone();
        owned.push(ctx.on_waterfall(TURN_END, move |end: TurnEnd, args| {
            let mut next = args.next::<TurnEnd>().unwrap_or(end);
            if next.queued_followups {
                return next;
            }
            let event = if cordis_base::types::is_main_identity(&next.identity) {
                "Stop"
            } else {
                "SubagentStop"
            };
            let active = b.stop_active.lock().unwrap().contains(&next.identity);
            let out = b.fire(
                event,
                &[],
                json!({ "stop_hook_active": active, "last_assistant_message": next.text }),
            );
            match out.block {
                Some(reason) if next.rounds < MAX_STOP_ROUNDS => {
                    b.stop_active.lock().unwrap().insert(next.identity.clone());
                    next.keep_working(
                        ORDER,
                        crate::tools::task::format::wrap_reminder(&format!(
                            "{event} hook 要求继续：{reason}"
                        )),
                    );
                }
                Some(_) => {
                    eprintln!("[hooks] {event}: 已经续跑 {MAX_STOP_ROUNDS} 次，不再续");
                    b.stop_active.lock().unwrap().remove(&next.identity);
                }
                None => {
                    b.stop_active.lock().unwrap().remove(&next.identity);
                }
            }
            next
        })?);

        ctx.effect("hooks-bridge", move |scope| {
            for d in owned {
                scope.own(d);
            }
            Ok(())
        })?;
        Ok(None)
    })
}

/// `/hooks`：这一页生效的 hooks 配置。
pub fn hooks_command() -> Plugin {
    plugin("command-hooks", Inject::from([SLASH]), |ctx, _: &()| {
        register_commands(
            ctx,
            vec![SlashCommand::host(
                "hooks",
                "列出这个项目生效的 hooks（Dock / Claude Code / Codex 格式）",
                slash_handler(|page, _| SlashOutcome::Notice {
                    title: "hooks".into(),
                    body: describe(&crate::session::cwd::session_cwd(page)),
                }),
            )],
        )?;
        Ok(None)
    })
}

/// 给人看的清单：每个文件、格式、信任状态、事件和命令数、警告，以及怎么打开别家的格式。
pub fn describe(project: &Path) -> String {
    let import = cordis_base::config::load_hooks_import();
    let mut lines = Vec::new();
    let found = config::discover(&Bridge::home(), project, import);
    if found.is_empty() {
        lines.push("没有 hooks 配置。".to_string());
    }
    for item in found {
        match item {
            Err((path, e)) => lines.push(format!("✗ {}：{e}", path.display())),
            Ok(s) => {
                let trust = if !s.project {
                    "用户目录，直接生效"
                } else if trust::is_trusted(&s.path, &s.digest) {
                    "项目文件，已确认"
                } else {
                    "项目文件，第一次触发时会问你要不要跑"
                };
                lines.push(format!(
                    "{}（{} 格式，{} 条命令，{trust}）",
                    s.path.display(),
                    s.dialect.label(),
                    s.command_count()
                ));
                for (event, groups) in &s.events {
                    let n: usize = groups.iter().map(|g| g.hooks.len()).sum();
                    lines.push(format!("  {event}：{n} 条命令"));
                }
                for w in &s.warnings {
                    lines.push(format!("  注意：{w}"));
                }
            }
        }
    }
    lines.push(String::new());
    lines.push("读哪些文件：~/.dock/hooks.json、<项目>/.dock/hooks.json 一直读。".into());
    lines.push(format!(
        "Claude Code（~/.claude/settings.json、<项目>/.claude/settings{{,.local}}.json）：{}",
        if import.claude_code {
            "已打开"
        } else {
            "没打开——在 ~/.dock/config.toml 写 [hooks] claude_code = true"
        }
    ));
    lines.push(format!(
        "Codex（~/.codex/hooks.json、<项目>/.codex/hooks.json）：{}",
        if import.codex {
            "已打开"
        } else {
            "没打开——在 ~/.dock/config.toml 写 [hooks] codex = true"
        }
    ));
    lines.join("\n")
}
