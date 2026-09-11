//! Task-family control-op cards — `get_task_output` / `wait_tasks` /
//! `kill_task` / `interrupt_agent` / `send_message` / `list_agents` /
//! `report`.
//! The spawn call (`task`) has its own card in [`super::subagent`]; the
//! `skill` card lives in [`super::skill`]. These render the follow-up ops:
//! status verb + target label + result preview.
//! A click on a target card opens the child conversation (`sub:<id>`) or job
//! output (`job:<id>`) overlay, same as the spawn card.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::grok::line_utils::truncate_line;
use crate::scrollback::live;
use crate::theme::Theme;
use cordis_spine::{JobSnapshot, SubagentSnap};

pub fn is_task_op(name: &str) -> bool {
    matches!(
        name,
        "get_task_output"
            | "wait_tasks"
            | "kill_task"
            | "interrupt_agent"
            | "send_message"
            | "list_agents"
            | "report"
    )
}

/// Target ids referenced by the call, in argument order.
fn target_ids(name: &str, arguments: &str) -> Vec<String> {
    if name == "list_agents" || name == "report" {
        return Vec::new();
    }
    let v: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let mut ids = Vec::new();
    if let Some(arr) = v.get("task_ids").and_then(|x| x.as_array()) {
        ids.extend(
            arr.iter()
                .filter_map(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
        );
    }
    for key in ["task_id", "subagent_id", "agent_id"] {
        if let Some(s) = v
            .get(key)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            ids.push(s.to_string());
        }
    }
    ids
}

/// `sub:<id>` when the first target is a roster subagent, `job:<id>` when it
/// is a background command; None when the target is unresolved or the call is
/// roster-wide (`list_agents` / `report`) — those cards are not clickable.
pub fn header_id(
    name: &str,
    arguments: &str,
    agents: &[SubagentSnap],
    jobs: &[JobSnapshot],
) -> Option<String> {
    if let Some((inner, inner_args)) = unwrap_task_op(name, arguments) {
        return header_id(&inner, &inner_args, agents, jobs);
    }
    let id = target_ids(name, arguments).into_iter().next()?;
    if agents.iter().any(|a| a.id == id) {
        return Some(format!("sub:{id}"));
    }
    if jobs.iter().any(|j| j.id == id) {
        return Some(format!("job:{id}"));
    }
    None
}

/// `use_tool` 包装（deferred 工具都走它）：内层是本族操作时，返回
/// (内层工具名, `tool_input` 序列化)，让操作卡片直接按内层渲染。
fn unwrap_task_op(name: &str, arguments: &str) -> Option<(String, String)> {
    super::unwrap_use_tool(name, arguments).filter(|(inner, _)| is_task_op(inner))
}

pub fn lines(
    name: &str,
    arguments: &str,
    content: &str,
    agents: &[SubagentSnap],
    job_snaps: &[JobSnapshot],
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    if let Some((inner, inner_args)) = unwrap_task_op(name, arguments) {
        return lines(
            &inner,
            &inner_args,
            content,
            agents,
            job_snaps,
            theme,
            width,
        );
    }
    let ids = target_ids(name, arguments);
    let snap = ids
        .first()
        .and_then(|id| agents.iter().find(|a| &a.id == id));
    let job = ids
        .first()
        .and_then(|id| job_snaps.iter().find(|j| &j.id == id));
    let pending = content.trim().is_empty();
    let err = content.trim_start().starts_with("Error");
    let (verb, detail) = if err {
        (verb_error(name), None)
    } else {
        verb_ok(name, content, pending)
    };
    // 技能卡搬到 [`super::skill`]：本族卡片只关心 target id 与结果正文。
    let running = pending && !err;
    let desc = label_for(&ids, snap, job, content, arguments, name);
    let preview = preview_for(name, arguments, content);

    let label_style = if running {
        theme.primary().add_modifier(Modifier::BOLD)
    } else {
        theme.muted().add_modifier(Modifier::BOLD)
    };
    let body_style = if running {
        theme.primary()
    } else {
        theme.muted()
    };
    let mut spans = vec![Span::styled(verb.to_string(), label_style)];
    if let Some(desc) = &desc {
        spans.push(Span::styled(format!(" \u{201c}{desc}\u{201d}"), body_style));
    }
    if let Some(detail) = detail {
        spans.push(Span::styled(format!(" \u{00b7} {detail}"), theme.muted()));
    }
    if running {
        if let Some(started) = snap.map(|s| s.started_at) {
            spans.push(Span::styled(live::elapsed_instant(started), theme.muted()));
        }
    }
    let clickable = header_id(name, arguments, agents, job_snaps).is_some();
    if clickable {
        spans.push(Span::styled("  （点击查看）".to_string(), theme.dim()));
    }
    let mut line = Line::from(spans);
    let accent = if err {
        theme.accent_error
    } else if running {
        theme.accent_running
    } else {
        theme.accent_thinking
    };
    line.spans.insert(0, live::diamond(theme, accent, running));
    if width > 0 {
        line = truncate_line(line, width);
    }
    let mut out = vec![line];
    if let (Some(preview), false) = (preview, running) {
        let mut prev = Line::from(Span::styled(format!("  {preview}"), theme.dim()));
        if width > 0 {
            prev = truncate_line(prev, width);
        }
        out.push(prev);
    }
    out
}

fn verb_error(name: &str) -> &'static str {
    match name {
        "send_message" => "发送失败",
        "kill_task" => "终止出错",
        "interrupt_agent" => "打断出错",
        "report" => "上报失败",
        "list_agents" => "名册出错",
        _ => "查询出错",
    }
}

/// (verb, optional detail) from the tool result body.
fn verb_ok(name: &str, content: &str, pending: bool) -> (&'static str, Option<&'static str>) {
    if pending {
        return (verb_pending(name), None);
    }
    match name {
        "get_task_output" => {
            if content.contains("not found") {
                ("未找到", None)
            } else if content.contains("No background tasks") {
                ("任务总览", Some("当前没有后台任务"))
            } else if content.contains("[running]") {
                ("任务输出", Some("部分任务仍在运行"))
            } else {
                ("任务输出", None)
            }
        }
        // wait_tasks 超时返回时正文里仍是 `[running]` 块——此时不能说「就绪」。
        "wait_tasks" => {
            if content.contains("[running]") {
                ("等待中", Some("超时返回，仍有任务在运行"))
            } else {
                ("任务就绪", None)
            }
        }
        "kill_task" => {
            if content.contains("already finished") {
                ("早已结束", None)
            } else if content.contains("not found") || content.contains("unknown") {
                ("未找到", None)
            } else {
                ("已终止", None)
            }
        }
        "interrupt_agent" => {
            if content.contains("interrupt requested") {
                ("已请求打断", Some("本轮停下后转 idle"))
            } else if content.contains("already idle") {
                ("本就空闲", None)
            } else {
                ("未找到", None)
            }
        }
        "send_message" => {
            if content.contains("urgent message delivered to running") {
                ("已插话", Some("运行中，本轮生效"))
            } else if content.contains("queued message accepted") {
                ("已排队", Some("本轮结束后执行"))
            } else if content.contains("delivered to idle") {
                ("已送达", Some("下一轮已开始"))
            } else {
                ("消息已发", None)
            }
        }
        "list_agents" => {
            if content.contains("(no subagents)") {
                ("名册为空", None)
            } else {
                ("子代理名册", None)
            }
        }
        "report" => ("已上报父代理", None),
        _ => ("任务", None),
    }
}

fn verb_pending(name: &str) -> &'static str {
    match name {
        "get_task_output" | "wait_tasks" => "查询任务",
        "kill_task" => "终止任务",
        "interrupt_agent" => "打断子代理",
        "send_message" => "发送消息",
        "list_agents" => "子代理名册",
        "report" => "上报父代理",
        _ => "任务",
    }
}

/// Target label: live roster description, else job description/command, else
/// the description line in the result block, else the raw ids.
fn label_for(
    ids: &[String],
    snap: Option<&SubagentSnap>,
    job: Option<&JobSnapshot>,
    content: &str,
    arguments: &str,
    name: &str,
) -> Option<String> {
    if name == "list_agents" {
        return Some(roster_summary(content));
    }
    if name == "report" {
        return None;
    }
    if let Some(s) = snap {
        let d = s.description.trim();
        if !d.is_empty() {
            return Some(d.to_string());
        }
        return Some(s.subagent_type.clone());
    }
    if let Some(j) = job {
        let d = j
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        return Some(d.unwrap_or(&j.command).to_string());
    }
    block_desc(content, ids.first().map(String::as_str))
        .or_else(|| json_str(arguments, "description"))
        .or_else(|| {
            ids.first().cloned().or_else(|| {
                if name == "get_task_output" || name == "wait_tasks" {
                    Some("全部任务".to_string())
                } else {
                    None
                }
            })
        })
}

/// Roster count + status split, e.g. `3 个 · 1 running · 2 idle`.
fn roster_summary(content: &str) -> String {
    let (mut running, mut idle) = (0usize, 0usize);
    for line in content.lines() {
        let t = line.trim();
        if t.contains("[running]") {
            running += 1;
        } else if t.contains("[idle]") {
            idle += 1;
        }
    }
    let total = running + idle;
    if total == 0 {
        return "0 个".to_string();
    }
    if running == 0 {
        format!("{total} 个 · 全部 idle")
    } else {
        format!("{total} 个 · {running} running · {idle} idle")
    }
}

/// Description from a `render_subagent` / `render_snap` result block:
/// `[status] <id> [type] <desc>` for subagents, `[status] <id> <command>` for jobs.
fn block_desc(content: &str, id: Option<&str>) -> Option<String> {
    let id = id?;
    let line = content
        .lines()
        .find(|l| l.trim().starts_with('[') && l.contains(id))?;
    let after_id = line.split_once(id)?.1.trim();
    // Subagent blocks carry a `[type]` tag after the id; job blocks don't.
    let after_type = match after_id.strip_prefix('[') {
        Some(rest) => rest.split_once(']')?.1.trim(),
        None => after_id,
    };
    if after_type.is_empty() {
        None
    } else {
        Some(first_line(after_type))
    }
}

fn preview_for(name: &str, arguments: &str, content: &str) -> Option<String> {
    match name {
        "send_message" => json_str(arguments, "message").map(|m| first_line(&m)),
        "report" => json_str(arguments, "output").map(|o| first_line(&o)),
        "get_task_output" | "wait_tasks" | "kill_task" | "interrupt_agent" => content
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .find(|l| !l.starts_with('['))
            .map(first_line),
        _ => None,
    }
}

fn json_str(arguments: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// First non-empty line, capped so the preview never wraps.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    let mut t: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        t.push('\u{2026}');
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn snap(id: &str, desc: &str) -> SubagentSnap {
        SubagentSnap {
            id: id.into(),
            description: desc.into(),
            subagent_type: "general-purpose".into(),
            done: false,
            idle: true,
            cancelled: false,
            output: String::new(),
            started_at: std::time::Instant::now(),
        }
    }

    #[test]
    fn send_message_card_shows_queue_state_and_preview() {
        let theme = Theme::current();
        let agents = vec![snap("kid-1", "修复 calc.py 的 add")];
        let card = lines(
            "send_message",
            r#"{"subagent_id":"kid-1","message":"再加一个 mod 函数\n并补测试"}"#,
            "queued message accepted for running subagent kid-1; it will run after the current turn ends",
            &agents,
            &[],
            &theme,
            120,
        );
        let text = flat(&card);
        assert!(text.contains("已排队"), "{text}");
        assert!(text.contains("本轮结束后执行"), "{text}");
        assert!(text.contains("修复 calc.py 的 add"), "{text}");
        assert!(text.contains("再加一个 mod 函数"), "{text}");
        assert!(text.contains("点击查看"), "{text}");
        assert_eq!(
            header_id("send_message", r#"{"subagent_id":"kid-1"}"#, &agents, &[]),
            Some("sub:kid-1".into())
        );
    }

    #[test]
    fn get_task_output_card_parses_result_block() {
        let theme = Theme::current();
        let body = "[idle] kid-1 [general-purpose] 修复 calc.py 的 add\n完成：add 修复，ALL PASS";
        let card = lines(
            "get_task_output",
            r#"{"task_ids":["kid-1"],"timeout_ms":0}"#,
            body,
            &[snap("kid-1", "修复 calc.py 的 add")],
            &[],
            &theme,
            120,
        );
        let text = flat(&card);
        assert!(text.contains("任务输出"), "{text}");
        assert!(text.contains("修复 calc.py 的 add"), "{text}");
        assert!(text.contains("完成：add 修复"), "{text}");
    }

    #[test]
    fn get_task_output_targets_job_overlay() {
        let job = JobSnapshot {
            id: "job-3".into(),
            command: "seq 3".into(),
            done: true,
            output: "3".into(),
            description: None,
            is_monitor: false,
            start_time: std::time::SystemTime::now(),
        };
        assert_eq!(
            header_id("get_task_output", r#"{"task_ids":["job-3"]}"#, &[], &[job]),
            Some("job:job-3".into())
        );
        assert_eq!(
            header_id("get_task_output", r#"{"task_ids":["ghost"]}"#, &[], &[]),
            None
        );
    }

    #[test]
    fn kill_and_interrupt_cards_reflect_result() {
        let theme = Theme::current();
        let killed = lines(
            "kill_task",
            r#"{"task_id":"kid-9"}"#,
            "killed kid-9",
            &[],
            &[],
            &theme,
            120,
        );
        assert!(flat(&killed).contains("已终止"), "{}", flat(&killed));
        // Disposed target: id shown, no click.
        assert!(!flat(&killed).contains("点击查看"));

        let idle = lines(
            "interrupt_agent",
            r#"{"agent_id":"kid-1"}"#,
            "agent kid-1 is already idle; interrupt does not start a turn — use send_message",
            &[snap("kid-1", "观察")],
            &[],
            &theme,
            120,
        );
        let text = flat(&idle);
        assert!(text.contains("本就空闲"), "{text}");
        assert!(text.contains("点击查看"), "{text}");
    }

    #[test]
    fn list_agents_card_summarizes_roster_without_click() {
        let theme = Theme::current();
        let body = "kid-1 [idle] — general-purpose — 修复 calc.py 的 add\n\
                    kid-2 [running] queued=1 — general-purpose — sleep 并汇报\n\
                    kid-3 [idle] — general-purpose — sleep 5 并汇报";
        let card = lines("list_agents", "{}", body, &[], &[], &theme, 120);
        let text = flat(&card);
        assert!(text.contains("子代理名册"), "{text}");
        assert!(text.contains("3 个 · 1 running · 2 idle"), "{text}");
        assert!(!text.contains("点击查看"), "{text}");

        let empty = lines("list_agents", "{}", "(no subagents)", &[], &[], &theme, 120);
        assert!(flat(&empty).contains("名册为空"));
    }

    #[test]
    fn report_card_previews_output() {
        let theme = Theme::current();
        let card = lines(
            "report",
            r#"{"output":"测试全过：ALL PASS\n明细……"}"#,
            "report accepted by the agent that started you as message main",
            &[],
            &[],
            &theme,
            120,
        );
        let text = flat(&card);
        assert!(text.contains("已上报父代理"), "{text}");
        assert!(text.contains("测试全过：ALL PASS"), "{text}");
    }

    #[test]
    fn use_tool_wrapper_renders_inner_op() {
        let theme = Theme::current();
        let agents = vec![snap("kid-1", "观察")];
        // 内层 task 族操作按内层卡片渲染，参数取 `tool_input`。
        let card = lines(
            "use_tool",
            r#"{"tool_name":"get_task_output","tool_input":{"task_ids":["kid-1"]}}"#,
            "[idle] kid-1 [general-purpose] 观察\n耗时=1s 退出码=0",
            &agents,
            &[],
            &theme,
            120,
        );
        let text = flat(&card);
        assert!(text.contains("任务输出"), "{text}");
        assert!(text.contains("观察"), "{text}");

        // 内层非本族操作不解包（`skill` 有自己的卡片，见 `super::skill`）。
        assert!(unwrap_task_op(
            "use_tool",
            r#"{"tool_name":"browser_open","tool_input":{}}"#
        )
        .is_none());
        assert!(unwrap_task_op(
            "use_tool",
            r#"{"tool_name":"skill","tool_input":{"name":"dock-config"}}"#
        )
        .is_none());
        assert!(unwrap_task_op("skill", "{}").is_none());

        // 内层 task 族的 target 解析穿透 tool_input。
        assert_eq!(
            header_id(
                "use_tool",
                r#"{"tool_name":"get_task_output","tool_input":{"task_ids":["kid-1"]}}"#,
                &agents,
                &[]
            ),
            Some("sub:kid-1".into())
        );
    }

    #[test]
    fn pending_call_renders_running_verb() {
        let theme = Theme::current();
        let card = lines(
            "wait_tasks",
            r#"{"task_ids":["kid-1"],"timeout_ms":8000}"#,
            "",
            &[snap("kid-1", "sleep 并汇报")],
            &[],
            &theme,
            120,
        );
        let text = flat(&card);
        assert!(text.contains("查询任务"), "{text}");
        assert!(text.contains("sleep 并汇报"), "{text}");
    }

    #[test]
    fn wait_tasks_distinguishes_ready_from_timeout() {
        let theme = Theme::current();
        let done = lines(
            "wait_tasks",
            r#"{"task_ids":["kid-1"]}"#,
            "[idle] kid-1 [general-purpose] sleep 并汇报\n耗时=5s 退出码=0",
            &[snap("kid-1", "sleep 并汇报")],
            &[],
            &theme,
            120,
        );
        assert!(flat(&done).contains("任务就绪"));

        let timeout = lines(
            "wait_tasks",
            r#"{"task_ids":["kid-1"],"timeout_ms":8000}"#,
            "[running] kid-1 [general-purpose] sleep 并汇报\n(still running)",
            &[],
            &[],
            &theme,
            120,
        );
        let text = flat(&timeout);
        assert!(text.contains("等待中"), "{text}");
        assert!(text.contains("仍有任务在运行"), "{text}");
    }

    #[test]
    fn get_task_output_snapshot_with_running_target_says_so() {
        let theme = Theme::current();
        let card = lines(
            "get_task_output",
            r#"{"task_ids":["kid-1"],"timeout_ms":0}"#,
            "[running] kid-1 [general-purpose] sleep 并汇报\n(still running)",
            &[],
            &[],
            &theme,
            120,
        );
        let text = flat(&card);
        assert!(text.contains("任务输出"), "{text}");
        assert!(text.contains("部分任务仍在运行"), "{text}");
    }

    #[test]
    fn error_verbs_are_per_tool() {
        let theme = Theme::current();
        let cases = [
            ("report", "Error: output is required", "上报失败"),
            ("list_agents", "Error: boom", "名册出错"),
            ("get_task_output", "Error: task_ids is required", "查询出错"),
            ("wait_tasks", "Error: task_ids is required", "查询出错"),
            ("send_message", "Error: message is required", "发送失败"),
            ("kill_task", "Error: task_id is required", "终止出错"),
            ("interrupt_agent", "Error: invalid arguments", "打断出错"),
        ];
        for (name, content, verb) in cases {
            let card = lines(name, "{}", content, &[], &[], &theme, 120);
            assert!(flat(&card).contains(verb), "{name}: {}", flat(&card));
        }
    }

    #[test]
    fn send_message_urgent_and_idle_delivery_verbs() {
        let theme = Theme::current();
        let steer = lines(
            "send_message",
            r#"{"subagent_id":"kid-1","message":"改方向"}"#,
            "urgent message delivered to running subagent kid-1; it will steer on the current turn",
            &[snap("kid-1", "观察")],
            &[],
            &theme,
            120,
        );
        let text = flat(&steer);
        assert!(text.contains("已插话"), "{text}");
        assert!(text.contains("运行中，本轮生效"), "{text}");

        let start = lines(
            "send_message",
            r#"{"subagent_id":"kid-1","message":"继续"}"#,
            "queued message delivered to idle subagent kid-1; the next turn is starting now",
            &[snap("kid-1", "观察")],
            &[],
            &theme,
            120,
        );
        let text = flat(&start);
        assert!(text.contains("已送达"), "{text}");
        assert!(text.contains("下一轮已开始"), "{text}");
    }

    #[test]
    fn interrupt_requested_and_kill_terminal_states() {
        let theme = Theme::current();
        let requested = lines(
            "interrupt_agent",
            r#"{"agent_id":"kid-1"}"#,
            "interrupt requested for agent kid-1",
            &[snap("kid-1", "观察")],
            &[],
            &theme,
            120,
        );
        let text = flat(&requested);
        assert!(text.contains("已请求打断"), "{text}");
        assert!(text.contains("本轮停下后转 idle"), "{text}");

        let finished = lines(
            "kill_task",
            r#"{"task_id":"kid-1"}"#,
            "kid-1 already finished",
            &[],
            &[],
            &theme,
            120,
        );
        assert!(flat(&finished).contains("早已结束"));

        let missing = lines(
            "kill_task",
            r#"{"task_id":"kid-9"}"#,
            "Task or subagent kid-9 not found. No background tasks or subagents exist in this session.",
            &[],
            &[],
            &theme,
            120,
        );
        assert!(flat(&missing).contains("未找到"));
    }

    #[test]
    fn multi_task_ids_bind_first_target_only() {
        let agents = vec![snap("kid-1", "第一个"), snap("kid-2", "第二个")];
        assert_eq!(
            header_id(
                "get_task_output",
                r#"{"task_ids":["kid-1","kid-2"]}"#,
                &agents,
                &[]
            ),
            Some("sub:kid-1".into())
        );
        let theme = Theme::current();
        let card = lines(
            "get_task_output",
            r#"{"task_ids":["kid-1","kid-2"]}"#,
            "[idle] kid-1 [general-purpose] 第一个\n输出一\n\n[idle] kid-2 [general-purpose] 第二个\n输出二",
            &agents,
            &[],
            &theme,
            120,
        );
        let text = flat(&card);
        assert!(text.contains("第一个"), "{text}");
    }
}
