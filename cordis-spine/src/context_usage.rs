//! Live context-window occupancy. TUI live-looks [`snapshot_context`].
//!
//! Estimates match auto-compact (`ascii/4` + CJK chars as 1). Tool schemas
//! and images are added on top so the overlay matches what the next sample
//! actually sends. Not grok.com billing.

use cordis::Context;

use crate::compact::{
    estimate_context_tokens, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT, VISIBLE_NOTICE,
};
use crate::mcp::Mcp;
use crate::names::{MCP, SESSIONS, SETTINGS, SYSTEM_PROMPT, TOOLS};
use crate::prompt::SystemPrompt;
use crate::session::{Sessions, TokenUsage};
use crate::settings::AppSettings;
use crate::tools::Tools;
use crate::types::{LogEvent, ToolSpec};

/// Per-image patch cost (Grok `IMAGE_TOKEN_ESTIMATE`).
pub const IMAGE_TOKEN_ESTIMATE: u64 = 765;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccupancyKind {
    System,
    Messages,
    Overhead,
    Free,
    Tools,
    Mcp,
    Workflows,
}

impl OccupancyKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::System => "系统提示",
            Self::Messages => "消息",
            Self::Overhead => "推理/开销",
            Self::Free => "空闲",
            Self::Tools => "工具定义",
            Self::Mcp => "MCP 服务器",
            Self::Workflows => "工作流",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccupancyDetail {
    pub kind: OccupancyKind,
    pub tokens: u64,
    pub groups: Vec<DetailGroup>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailGroup {
    pub heading: String,
    pub rows: Vec<DetailRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailRow {
    pub label: String,
    pub tokens: Option<u64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextCategory {
    pub label: String,
    pub tokens: u64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSnapshot {
    pub used: u64,
    pub total: u64,
    pub model: String,
    pub system_prompt_tokens: u64,
    pub message_tokens: u64,
    pub tool_definitions_tokens: u64,
    pub tool_definitions_count: u64,
    pub free_tokens: u64,
    pub usage_pct: u8,
    pub auto_compact_threshold_percent: u8,
    pub turn_count: u64,
    pub tool_call_count: u64,
    pub compaction_count: u64,
    pub categories: Vec<ContextCategory>,
}

impl ContextSnapshot {
    pub fn remaining_to_compact(&self) -> u64 {
        if self.total == 0 {
            return 0;
        }
        let threshold = self
            .total
            .saturating_mul(self.auto_compact_threshold_percent as u64)
            .div_ceil(100);
        threshold.saturating_sub(self.used)
    }
}

/// Assemble occupancy from live `"sessions"` / `"systemPrompt"` / `"tools"`.
pub fn snapshot_context(ctx: &Context) -> ContextSnapshot {
    snapshot_with_system(ctx, &assembled_system(ctx))
}

/// Itemized breakdown for one occupancy slice (TUI detail pane).
pub fn occupancy_detail(ctx: &Context, kind: OccupancyKind) -> OccupancyDetail {
    let system = assembled_system(ctx);
    let snap = snapshot_with_system(ctx, &system);
    match kind {
        OccupancyKind::System => system_detail(&system, &snap),
        OccupancyKind::Messages => messages_detail(ctx, &snap),
        OccupancyKind::Overhead => overhead_detail(ctx, &snap),
        OccupancyKind::Free => free_detail(&snap),
        OccupancyKind::Tools => tools_detail(ctx, &snap),
        OccupancyKind::Mcp => mcp_detail(ctx),
        OccupancyKind::Workflows => workflows_detail(&snap),
    }
}

fn assembled_system(ctx: &Context) -> String {
    ctx.get::<SystemPrompt>(SYSTEM_PROMPT)
        .map(|p| p.assemble_on(ctx))
        .unwrap_or_default()
}

fn snapshot_with_system(ctx: &Context, system: &str) -> ContextSnapshot {
    let system_prompt_tokens = estimate_text(system);
    let sessions = ctx.get::<Sessions>(SESSIONS);
    let (base, reasoning, turn_count, tool_call_count, compaction_count) = if let Some(sessions) =
        sessions.as_ref()
    {
        sessions.with_log(|history, _| {
            (
                estimate_context_tokens(system, history),
                reasoning_tokens(history),
                history
                    .iter()
                    .filter(|e| matches!(e, LogEvent::User(t) if !t.trim().is_empty()))
                    .count() as u64,
                history
                    .iter()
                    .filter(|e| matches!(e, LogEvent::ToolExecute { .. }))
                    .count() as u64,
                history
                    .iter()
                    .filter(|e| matches!(e, LogEvent::LlmStream(out) if out.text == VISIBLE_NOTICE))
                    .count() as u64,
            )
        })
    } else {
        (system_prompt_tokens, 0, 0, 0, 0)
    };
    let message_tokens = base.saturating_sub(system_prompt_tokens);
    let image_tokens = sessions
        .as_ref()
        .map(|s| s.user_image_count().saturating_mul(IMAGE_TOKEN_ESTIMATE))
        .unwrap_or(0);

    let specs = ctx
        .get::<Tools>(TOOLS)
        .map(|t| t.specs_for_model_on(ctx))
        .unwrap_or_default();
    let tool_definitions_count = specs.len() as u64;
    let tool_definitions_tokens = estimate_tool_definitions(&specs);

    let estimate = base
        .saturating_add(tool_definitions_tokens)
        .saturating_add(image_tokens)
        .saturating_add(reasoning);
    let usage = sessions.as_ref().map(|s| s.usage()).unwrap_or_default();
    let total = window_size(usage.window, ctx);
    let used = used_tokens(usage, estimate).min(total.saturating_mul(4));
    let usage_pct = usage_percentage_u8(used, total);

    ContextSnapshot {
        used,
        total,
        model: model_label(ctx),
        system_prompt_tokens,
        message_tokens,
        tool_definitions_tokens,
        tool_definitions_count,
        free_tokens: total.saturating_sub(used),
        usage_pct,
        auto_compact_threshold_percent: DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT,
        turn_count,
        tool_call_count,
        compaction_count,
        categories: extra_categories(ctx),
    }
}

fn system_detail(text: &str, snap: &ContextSnapshot) -> OccupancyDetail {
    let chars = text.chars().count() as u64;
    let lines = if text.is_empty() {
        0
    } else {
        text.lines().count() as u64
    };
    OccupancyDetail {
        kind: OccupancyKind::System,
        tokens: snap.system_prompt_tokens,
        groups: vec![DetailGroup {
            heading: format!(
                "{} token · {chars} 字 · {lines} 行",
                snap.system_prompt_tokens
            ),
            rows: Vec::new(),
        }],
        text: Some(if text.is_empty() {
            "（空）".into()
        } else {
            text.to_string()
        }),
    }
}

fn messages_detail(ctx: &Context, snap: &ContextSnapshot) -> OccupancyDetail {
    let history = ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.events())
        .unwrap_or_default();
    let mut user_n = 0u64;
    let mut user_tok = 0u64;
    let mut asst_n = 0u64;
    let mut asst_tok = 0u64;
    let mut tool_n = 0u64;
    let mut tool_tok = 0u64;
    let mut remind_n = 0u64;
    let mut remind_tok = 0u64;
    let mut rows = Vec::new();
    for event in &history {
        match event {
            LogEvent::User(t) if !t.trim().is_empty() => {
                let tok = estimate_text(t);
                user_n += 1;
                user_tok += tok;
                rows.push(DetailRow {
                    label: "用户".into(),
                    tokens: Some(tok),
                    note: Some(preview(t, 48)),
                });
            }
            LogEvent::LlmStream(out) => {
                if out.text.trim().is_empty() && out.tool_calls.is_empty() {
                    continue;
                }
                let mut tok = estimate_text(&out.text);
                for call in &out.tool_calls {
                    tok = tok
                        .saturating_add(estimate_text(&call.name))
                        .saturating_add(estimate_text(&call.arguments));
                }
                asst_n += 1;
                asst_tok += tok;
                let note = if out.text == VISIBLE_NOTICE {
                    "已压缩上下文。".into()
                } else if out.text.trim().is_empty() {
                    format!("{} 个工具调用", out.tool_calls.len())
                } else {
                    preview(&out.text, 48)
                };
                rows.push(DetailRow {
                    label: "助手".into(),
                    tokens: Some(tok),
                    note: Some(note),
                });
            }
            LogEvent::ToolExecute {
                name,
                arguments,
                content,
                ..
            } => {
                let tok = estimate_text(name)
                    .saturating_add(estimate_text(arguments))
                    .saturating_add(estimate_text(content));
                tool_n += 1;
                tool_tok += tok;
                rows.push(DetailRow {
                    label: "工具".into(),
                    tokens: Some(tok),
                    note: Some(name.clone()),
                });
            }
            LogEvent::SystemReminder(t) => {
                let tok = estimate_text(t);
                remind_n += 1;
                remind_tok += tok;
                rows.push(DetailRow {
                    label: "提醒".into(),
                    tokens: Some(tok),
                    note: Some(preview(t, 48)),
                });
            }
            _ => {}
        }
    }
    let mut groups = vec![DetailGroup {
        heading: format!("用户 {user_n} · 助手 {asst_n} · 工具结果 {tool_n} · 提醒 {remind_n}"),
        rows: vec![
            DetailRow {
                label: "用户".into(),
                tokens: Some(user_tok),
                note: Some(format!("{user_n} 条")),
            },
            DetailRow {
                label: "助手".into(),
                tokens: Some(asst_tok),
                note: Some(format!("{asst_n} 条")),
            },
            DetailRow {
                label: "工具结果".into(),
                tokens: Some(tool_tok),
                note: Some(format!("{tool_n} 条")),
            },
            DetailRow {
                label: "提醒".into(),
                tokens: Some(remind_tok),
                note: Some(format!("{remind_n} 条")),
            },
        ],
    }];
    if !rows.is_empty() {
        groups.push(DetailGroup {
            heading: "逐条".into(),
            rows,
        });
    }
    OccupancyDetail {
        kind: OccupancyKind::Messages,
        tokens: snap.message_tokens,
        groups,
        text: None,
    }
}

fn overhead_detail(ctx: &Context, snap: &ContextSnapshot) -> OccupancyDetail {
    let history = ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.events())
        .unwrap_or_default();
    let reasoning = reasoning_tokens(&history);
    let reasoning_n = history
        .iter()
        .filter(|e| matches!(e, LogEvent::LlmStream(o) if !o.reasoning.is_empty()))
        .count() as u64;
    let image_n = ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.user_image_count())
        .unwrap_or(0);
    let image_tok = image_n.saturating_mul(IMAGE_TOKEN_ESTIMATE);
    let overhead = snap.used.saturating_sub(
        snap.system_prompt_tokens
            .saturating_add(snap.message_tokens),
    );
    OccupancyDetail {
        kind: OccupancyKind::Overhead,
        tokens: overhead,
        groups: vec![DetailGroup {
            heading: "菱形条开销格包含这些，工具定义不单独占色块".into(),
            rows: vec![
                DetailRow {
                    label: "推理".into(),
                    tokens: Some(reasoning),
                    note: Some(format!("{reasoning_n} 段")),
                },
                DetailRow {
                    label: "图片".into(),
                    tokens: Some(image_tok),
                    note: Some(format!("{image_n} 张 × {IMAGE_TOKEN_ESTIMATE}")),
                },
                DetailRow {
                    label: "工具定义".into(),
                    tokens: Some(snap.tool_definitions_tokens),
                    note: Some(format!("{} 个", snap.tool_definitions_count)),
                },
            ],
        }],
        text: None,
    }
}

fn free_detail(snap: &ContextSnapshot) -> OccupancyDetail {
    OccupancyDetail {
        kind: OccupancyKind::Free,
        tokens: snap.free_tokens,
        groups: vec![DetailGroup {
            heading: format!(
                "窗口 {} · 已用 {} · {}%",
                snap.total, snap.used, snap.usage_pct
            ),
            rows: vec![
                DetailRow {
                    label: "空闲".into(),
                    tokens: Some(snap.free_tokens),
                    note: None,
                },
                DetailRow {
                    label: format!("{}% 自动压缩", snap.auto_compact_threshold_percent),
                    tokens: Some(snap.remaining_to_compact()),
                    note: Some("距阈值还剩".into()),
                },
            ],
        }],
        text: None,
    }
}

fn tools_detail(ctx: &Context, snap: &ContextSnapshot) -> OccupancyDetail {
    let mut rows: Vec<DetailRow> = ctx
        .get::<Tools>(TOOLS)
        .map(|t| t.specs_for_model_on(ctx))
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            let tok = estimate_text(&s.name)
                .saturating_add(estimate_text(&s.description))
                .saturating_add(estimate_text(&s.parameters_json));
            DetailRow {
                label: s.name,
                tokens: Some(tok),
                note: {
                    let d = s.description.trim();
                    if d.is_empty() {
                        None
                    } else {
                        Some(preview(d, 48))
                    }
                },
            }
        })
        .collect();
    rows.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.label.cmp(&b.label)));
    OccupancyDetail {
        kind: OccupancyKind::Tools,
        tokens: snap.tool_definitions_tokens,
        groups: vec![DetailGroup {
            heading: format!("{} 个工具（模型可见）", snap.tool_definitions_count),
            rows,
        }],
        text: None,
    }
}

fn mcp_detail(ctx: &Context) -> OccupancyDetail {
    let rows = extra_categories(ctx)
        .into_iter()
        .find(|c| c.label == "MCP 服务器");
    let tokens = rows.as_ref().map(|c| c.tokens).unwrap_or(0);
    let mut detail_rows = Vec::new();
    if let Some(mcp) = ctx.get::<Mcp>(MCP) {
        for s in mcp.list().into_iter().filter(|s| s.enabled) {
            let enabled: Vec<_> = s.tools.iter().filter(|t| t.enabled).collect();
            let mut text = s.name.clone();
            text.push('\n');
            for t in &enabled {
                text.push_str(&t.name);
                text.push('\n');
            }
            detail_rows.push(DetailRow {
                label: s.name,
                tokens: Some(estimate_text(&text)),
                note: Some(format!("{} 个工具", enabled.len())),
            });
        }
    }
    OccupancyDetail {
        kind: OccupancyKind::Mcp,
        tokens,
        groups: vec![DetailGroup {
            heading: if detail_rows.is_empty() {
                "没有已启用的 MCP 服务器".into()
            } else {
                format!("{} 台已启用", detail_rows.len())
            },
            rows: detail_rows,
        }],
        text: None,
    }
}

fn workflows_detail(snap: &ContextSnapshot) -> OccupancyDetail {
    let tokens = snap
        .categories
        .iter()
        .find(|c| c.label == "工作流")
        .map(|c| c.tokens)
        .unwrap_or(0);
    let listing = crate::workflow::catalog_listing();
    let rows: Vec<DetailRow> = listing
        .iter()
        .map(|(name, desc)| {
            let mut text = name.clone();
            text.push('\n');
            text.push_str(desc);
            DetailRow {
                label: name.clone(),
                tokens: Some(estimate_text(&text)),
                note: {
                    let d = desc.trim();
                    if d.is_empty() {
                        None
                    } else {
                        Some(preview(d, 48))
                    }
                },
            }
        })
        .collect();
    OccupancyDetail {
        kind: OccupancyKind::Workflows,
        tokens,
        groups: vec![DetailGroup {
            heading: format!("{} 个工作流", rows.len()),
            rows,
        }],
        text: None,
    }
}

fn preview(text: &str, max_chars: usize) -> String {
    let first = text.lines().next().unwrap_or("").trim();
    let n = first.chars().count();
    if n <= max_chars {
        first.to_string()
    } else {
        let mut s: String = first.chars().take(max_chars.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

fn used_tokens(usage: TokenUsage, estimate: u64) -> u64 {
    if usage.official {
        usage.prompt.max(estimate)
    } else {
        estimate
    }
}

fn window_size(session_window: u64, ctx: &Context) -> u64 {
    if session_window > 0 {
        return session_window;
    }
    ctx.get::<AppSettings>(SETTINGS)
        .and_then(|st| {
            let id = st.model();
            st.catalog()
                .into_iter()
                .find(|m| m.id == id)
                .and_then(|m| m.context_window)
        })
        .filter(|n| *n > 0)
        .unwrap_or(128_000)
}

fn extra_categories(ctx: &Context) -> Vec<ContextCategory> {
    let mut rows = Vec::new();
    if let Some(mcp) = ctx.get::<Mcp>(MCP) {
        let servers: Vec<_> = mcp.list().into_iter().filter(|s| s.enabled).collect();
        if !servers.is_empty() {
            let mut text = String::new();
            for s in &servers {
                text.push_str(&s.name);
                text.push('\n');
                for t in &s.tools {
                    if t.enabled {
                        text.push_str(&t.name);
                        text.push('\n');
                    }
                }
            }
            rows.push(ContextCategory {
                label: "MCP 服务器".into(),
                tokens: estimate_text(&text),
                detail: Some(count_detail(servers.len() as u64, "个")),
            });
        }
    }
    let workflows = crate::workflow::catalog_listing();
    if !workflows.is_empty() {
        let mut text = String::new();
        for (name, desc) in &workflows {
            text.push_str(name);
            text.push('\n');
            text.push_str(desc);
            text.push('\n');
        }
        rows.push(ContextCategory {
            label: "工作流".into(),
            tokens: estimate_text(&text),
            detail: Some(count_detail(workflows.len() as u64, "个")),
        });
    }
    rows
}

fn reasoning_tokens(history: &[LogEvent]) -> u64 {
    history
        .iter()
        .map(|e| match e {
            LogEvent::LlmStream(out) => estimate_text(&out.reasoning),
            _ => 0,
        })
        .sum()
}

fn estimate_tool_definitions(specs: &[ToolSpec]) -> u64 {
    specs
        .iter()
        .map(|s| {
            estimate_text(&s.name)
                .saturating_add(estimate_text(&s.description))
                .saturating_add(estimate_text(&s.parameters_json))
        })
        .sum()
}

fn estimate_text(text: &str) -> u64 {
    let mut ascii = 0u64;
    let mut other = 0u64;
    for c in text.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
    }
    other + ascii.saturating_add(3) / 4
}

fn model_label(ctx: &Context) -> String {
    let Some(settings) = ctx.get::<AppSettings>(SETTINGS) else {
        return String::new();
    };
    let id = settings.model();
    settings
        .catalog()
        .into_iter()
        .find(|m| m.id == id)
        .map(|m| {
            if m.name.trim().is_empty() {
                m.id
            } else {
                m.name
            }
        })
        .unwrap_or(id)
}

fn count_detail(n: u64, unit: &str) -> String {
    format!("{n} {unit}")
}

fn usage_percentage_u8(used: u64, total: u64) -> u8 {
    if total == 0 {
        0
    } else {
        ((used as f64) / (total as f64) * 100.0).round().min(100.0) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_cjk_estimates() {
        assert_eq!(estimate_text(""), 0);
        assert_eq!(estimate_text("abcd"), 1);
        assert_eq!(estimate_text("中"), 1);
        assert_eq!(estimate_text("中文ab"), 2 + 2u64.saturating_add(3) / 4);
    }

    #[test]
    fn remaining_uses_div_ceil_threshold() {
        let snap = ContextSnapshot {
            used: 80_000,
            total: 100_000,
            model: String::new(),
            system_prompt_tokens: 1,
            message_tokens: 1,
            tool_definitions_tokens: 0,
            tool_definitions_count: 0,
            free_tokens: 20_000,
            usage_pct: 80,
            auto_compact_threshold_percent: 85,
            turn_count: 1,
            tool_call_count: 0,
            compaction_count: 0,
            categories: Vec::new(),
        };
        assert_eq!(snap.remaining_to_compact(), 5_000);
    }

    #[test]
    fn official_prompt_wins_when_larger() {
        let estimate = 100;
        assert_eq!(
            used_tokens(
                TokenUsage {
                    prompt: 500,
                    official: true,
                    window: 1_000,
                    ..TokenUsage::default()
                },
                estimate
            ),
            500
        );
        assert_eq!(
            used_tokens(
                TokenUsage {
                    prompt: 50,
                    official: true,
                    window: 1_000,
                    ..TokenUsage::default()
                },
                estimate
            ),
            100
        );
        assert_eq!(
            used_tokens(
                TokenUsage {
                    prompt: 999,
                    official: false,
                    window: 1_000,
                    ..TokenUsage::default()
                },
                estimate
            ),
            100
        );
    }

    #[test]
    fn empty_window_falls_back() {
        assert_eq!(window_size(0, &Context::new()), 128_000);
        assert_eq!(window_size(204_800, &Context::new()), 204_800);
    }

    #[tokio::test]
    async fn snapshot_counts_user_message() {
        let ctx = Context::new();
        crate::bundle::install_fakes(&ctx).await.unwrap();
        let sessions = ctx.get::<Sessions>(SESSIONS).unwrap();
        sessions.append(LogEvent::User("hello dock".into()));
        let snap = snapshot_context(&ctx);
        assert!(
            snap.message_tokens > 0,
            "message_tokens={}",
            snap.message_tokens
        );
        assert_eq!(snap.turn_count, 1);
        assert!(snap.total > 0);
        assert!(snap.system_prompt_tokens > 0);
        assert!(snap.used >= snap.system_prompt_tokens + snap.message_tokens);
    }

    #[tokio::test]
    async fn detail_system_includes_assembled_prompt() {
        let ctx = Context::new();
        crate::bundle::install_fakes(&ctx).await.unwrap();
        let d = occupancy_detail(&ctx, OccupancyKind::System);
        assert_eq!(d.kind, OccupancyKind::System);
        let text = d.text.unwrap_or_default();
        assert!(
            text.contains("test agent") || text.contains("You are"),
            "text={text}"
        );
        assert!(d.tokens > 0);
    }

    #[tokio::test]
    async fn detail_messages_lists_user() {
        let ctx = Context::new();
        crate::bundle::install_fakes(&ctx).await.unwrap();
        ctx.get::<Sessions>(SESSIONS)
            .unwrap()
            .append(LogEvent::User("hello dock".into()));
        let d = occupancy_detail(&ctx, OccupancyKind::Messages);
        assert!(
            d.groups
                .iter()
                .flat_map(|g| &g.rows)
                .any(|r| r.note.as_deref() == Some("hello dock")),
            "{d:?}"
        );
    }

    #[tokio::test]
    async fn detail_tools_lists_model_specs() {
        let ctx = Context::new();
        crate::bundle::install_fakes(&ctx).await.unwrap();
        let tools = ctx.get::<Tools>(TOOLS).unwrap();
        let body: crate::tools::ToolBody = std::sync::Arc::new(|call| {
            Box::pin(async move { crate::tools::tool_result(call, "") })
        });
        let _keep = tools
            .register(
                ToolSpec {
                    name: "probe".into(),
                    description: "occupancy probe".into(),
                    parameters_json: r#"{"type":"object"}"#.into(),
                },
                body,
            )
            .unwrap();
        let d = occupancy_detail(&ctx, OccupancyKind::Tools);
        assert!(
            d.groups
                .iter()
                .flat_map(|g| &g.rows)
                .any(|r| r.label == "probe"),
            "{d:?}"
        );
        assert!(d.tokens > 0);
    }
}
