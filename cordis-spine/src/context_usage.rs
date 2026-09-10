//! Live context-window occupancy. TUI live-looks [`snapshot_context`].
//!
//! Estimates match auto-compact (`ascii/4` + CJK chars as 1). Tool schemas
//! and images are added on top so the overlay matches what the next sample
//! actually sends. Not grok.com billing.

use cordis::Context;

use crate::compact::{
    estimate_context_tokens, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT, VISIBLE_NOTICE,
};
use crate::mcp::{is_mcp_public_name, split_mcp_public_name};
use crate::names::{SESSIONS, SETTINGS, SKILLS, SYSTEM_PROMPT, TOOLS};
use crate::prompt::{PromptAssembly, SystemPrompt};
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
    Deferred,
    Workflows,
    Skills,
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
            Self::Deferred => "本地按需",
            Self::Workflows => "工作流",
            Self::Skills => "技能",
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
    snapshot_with_parts(ctx, &assembled_parts(ctx))
}

/// Itemized breakdown for one occupancy slice (TUI detail pane).
pub fn occupancy_detail(ctx: &Context, kind: OccupancyKind) -> OccupancyDetail {
    let parts = assembled_parts(ctx);
    let snap = snapshot_with_parts(ctx, &parts);
    match kind {
        OccupancyKind::System => system_detail(&parts, &snap),
        OccupancyKind::Messages => messages_detail(ctx, &snap),
        OccupancyKind::Overhead => overhead_detail(ctx, &snap),
        OccupancyKind::Free => free_detail(&snap),
        OccupancyKind::Tools => tools_detail(ctx, &snap),
        OccupancyKind::Mcp => mcp_detail(ctx, &snap),
        OccupancyKind::Deferred => deferred_detail(ctx, &snap),
        OccupancyKind::Workflows => workflows_detail(&snap),
        OccupancyKind::Skills => skills_detail(ctx, &snap),
    }
}

fn assembled_parts(ctx: &Context) -> PromptAssembly {
    ctx.get::<SystemPrompt>(SYSTEM_PROMPT)
        .map(|p| p.assemble_parts_on(ctx))
        .unwrap_or_default()
}

fn snapshot_with_parts(ctx: &Context, parts: &PromptAssembly) -> ContextSnapshot {
    let system = parts.render();
    let system_prompt_tokens = estimate_text(&system);
    let sessions = ctx.get::<Sessions>(SESSIONS);
    let (base, reasoning, turn_count, tool_call_count, compaction_count) =
        if let Some(sessions) = sessions.as_ref() {
            let history = sessions.model_history();
            let display = sessions.events();
            (
                estimate_context_tokens(&system, &history),
                reasoning_tokens(&history),
                display
                    .iter()
                    .filter(|e| matches!(e, LogEvent::User(t) if !t.trim().is_empty()))
                    .count() as u64,
                display
                    .iter()
                    .filter(|e| matches!(e, LogEvent::ToolExecute { .. }))
                    .count() as u64,
                display
                    .iter()
                    .filter(|e| matches!(e, LogEvent::LlmStream(out) if out.text == VISIBLE_NOTICE))
                    .count() as u64,
            )
        } else {
            (system_prompt_tokens, 0, 0, 0, 0)
        };
    let message_tokens = base.saturating_sub(system_prompt_tokens);
    let image_tokens = sessions
        .as_ref()
        .map(|s| {
            s.model_user_image_count()
                .saturating_mul(IMAGE_TOKEN_ESTIMATE)
        })
        .unwrap_or(0);

    let (builtin_specs, _, _) = partition_model_specs(ctx);
    let tool_definitions_count = builtin_specs.len() as u64;
    let tool_definitions_tokens = estimate_tool_definitions(&builtin_specs);

    // Hidden extras (MCP + deferred local) stay registered for `use_tool`
    // but are omitted from the sampler tools array. Occupancy matches that send.
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
        categories: extra_categories(ctx, parts),
    }
}

fn system_detail(assembly: &PromptAssembly, snap: &ContextSnapshot) -> OccupancyDetail {
    let text = assembly.render();
    let chars = text.chars().count() as u64;
    let lines = if text.is_empty() {
        0
    } else {
        text.lines().count() as u64
    };
    let rows: Vec<DetailRow> = assembly
        .inspect()
        .into_iter()
        .map(|p| DetailRow {
            label: section_label(&p.id),
            tokens: Some(estimate_text(&p.body)),
            note: None,
        })
        .collect();
    OccupancyDetail {
        kind: OccupancyKind::System,
        tokens: snap.system_prompt_tokens,
        groups: vec![DetailGroup {
            heading: format!(
                "{} token · {chars} 字 · {lines} 行",
                snap.system_prompt_tokens
            ),
            rows,
        }],
        text: Some(if text.is_empty() {
            "（空）".into()
        } else {
            text
        }),
    }
}

fn section_label(id: &str) -> String {
    match id {
        "base" => "基座".into(),
        "cordis" => "Cordis".into(),
        "skills" => "技能".into(),
        "workflows" => "工作流".into(),
        "persona" | "persona-replace" => "人设".into(),
        "roster" => "子代理".into(),
        "plan" => "计划".into(),
        "goal" => "目标".into(),
        other => other.into(),
    }
}

fn messages_detail(ctx: &Context, snap: &ContextSnapshot) -> OccupancyDetail {
    let history = ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.model_history())
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
        .map(|s| s.model_history())
        .unwrap_or_default();
    let reasoning = reasoning_tokens(&history);
    let reasoning_n = history
        .iter()
        .filter(|e| matches!(e, LogEvent::LlmStream(o) if !o.reasoning.is_empty()))
        .count() as u64;
    let image_n = ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.model_user_image_count())
        .unwrap_or(0);
    let image_tok = image_n.saturating_mul(IMAGE_TOKEN_ESTIMATE);
    let overhead = snap.used.saturating_sub(
        snap.system_prompt_tokens
            .saturating_add(snap.message_tokens),
    );
    let rows = vec![
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
    ];
    OccupancyDetail {
        kind: OccupancyKind::Overhead,
        tokens: overhead,
        groups: vec![DetailGroup {
            heading: "菱形条开销格包含这些，工具定义不单独占色块".into(),
            rows,
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
    let (builtin_specs, _, _) = partition_model_specs(ctx);
    let mut rows: Vec<DetailRow> = builtin_specs
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
            heading: format!(
                "{} 个工具（模型可见，不含按需工具）",
                snap.tool_definitions_count
            ),
            rows,
        }],
        text: None,
    }
}

fn mcp_detail(ctx: &Context, snap: &ContextSnapshot) -> OccupancyDetail {
    let tokens = snap
        .categories
        .iter()
        .find(|c| c.label == "MCP 服务器")
        .map(|c| c.tokens)
        .unwrap_or(0);
    let (_, mcp_specs, _) = partition_model_specs(ctx);
    let mut by_server: std::collections::BTreeMap<String, Vec<ToolSpec>> =
        std::collections::BTreeMap::new();
    for spec in mcp_specs {
        let server = split_mcp_public_name(&spec.name)
            .map(|(s, _)| s.to_string())
            .unwrap_or_else(|| spec.name.clone());
        by_server.entry(server).or_default().push(spec);
    }
    let mut groups: Vec<DetailGroup> = vec![DetailGroup {
        heading: "未发给模型 · 经 search_tool / use_tool 发现".into(),
        rows: Vec::new(),
    }];
    for (server, specs) in by_server {
        let mut rows: Vec<DetailRow> = specs
            .into_iter()
            .map(|s| {
                let local = split_mcp_public_name(&s.name)
                    .map(|(_, t)| t.to_string())
                    .unwrap_or_else(|| s.name.clone());
                DetailRow {
                    label: local,
                    tokens: None,
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
        rows.sort_by(|a, b| a.label.cmp(&b.label));
        groups.push(DetailGroup {
            heading: format!("{server} · {} 个工具", rows.len()),
            rows,
        });
    }
    OccupancyDetail {
        kind: OccupancyKind::Mcp,
        tokens,
        groups: if groups.len() == 1 {
            vec![DetailGroup {
                heading: "没有已连接的 MCP 工具".into(),
                rows: Vec::new(),
            }]
        } else {
            groups
        },
        text: None,
    }
}

fn deferred_detail(ctx: &Context, snap: &ContextSnapshot) -> OccupancyDetail {
    let tokens = snap
        .categories
        .iter()
        .find(|c| c.label == "本地按需")
        .map(|c| c.tokens)
        .unwrap_or(0);
    let (_, _, deferred) = partition_model_specs(ctx);
    let mut rows: Vec<DetailRow> = deferred
        .into_iter()
        .map(|s| DetailRow {
            label: s.name,
            tokens: None,
            note: {
                let d = s.description.trim();
                if d.is_empty() {
                    None
                } else {
                    Some(preview(d, 48))
                }
            },
        })
        .collect();
    rows.sort_by(|a, b| a.label.cmp(&b.label));
    OccupancyDetail {
        kind: OccupancyKind::Deferred,
        tokens,
        groups: vec![DetailGroup {
            heading: if rows.is_empty() {
                "没有按需本地工具".into()
            } else {
                format!("未发给模型 · {} 个 · 经 search_tool 发现", rows.len())
            },
            rows,
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

fn skills_detail(ctx: &Context, snap: &ContextSnapshot) -> OccupancyDetail {
    let tokens = snap
        .categories
        .iter()
        .find(|c| c.label == "技能")
        .map(|c| c.tokens)
        .unwrap_or(0);
    let rows: Vec<DetailRow> = ctx
        .get::<crate::skills::Skills>(SKILLS)
        .map(|s| {
            s.occupancy_rows()
                .into_iter()
                .map(|(label, tokens, desc)| DetailRow {
                    label,
                    tokens: Some(tokens),
                    note: {
                        let d = desc.trim();
                        if d.is_empty() {
                            None
                        } else {
                            Some(preview(d, 48))
                        }
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    OccupancyDetail {
        kind: OccupancyKind::Skills,
        tokens,
        groups: vec![DetailGroup {
            heading: format!("{} 个技能（listing，已计入系统提示）", rows.len()),
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

fn section_tokens(assembly: &PromptAssembly, id: &str) -> u64 {
    assembly
        .inspect()
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| estimate_text(&p.body))
        .unwrap_or(0)
}

fn extra_categories(ctx: &Context, assembly: &PromptAssembly) -> Vec<ContextCategory> {
    let mut rows = Vec::new();
    // Listings that already live in the system prompt come first so /context
    // does not clip 技能 under MCP / 本地按需 / 工作流.
    let skills = ctx.get::<crate::skills::Skills>(SKILLS);
    let skill_rows = skills
        .as_ref()
        .map(|s| s.occupancy_rows())
        .unwrap_or_default();
    let catalog_n = skills.as_ref().map(|s| s.catalog().len()).unwrap_or(0);
    let skills_section = section_tokens(assembly, "skills");
    if skills.is_some() || skills_section > 0 {
        let tokens = if skills_section > 0 {
            skills_section
        } else {
            skill_rows.iter().map(|(_, t, _)| *t).sum()
        };
        let n = skill_rows.len().max(catalog_n);
        rows.push(ContextCategory {
            label: "技能".into(),
            tokens,
            detail: Some(if tokens > 0 {
                if n > 0 {
                    format!("{n} 个 · 已计入系统提示")
                } else {
                    "已计入系统提示".into()
                }
            } else {
                count_detail(n as u64, "个")
            }),
        });
    }
    let workflows = crate::workflow::catalog_listing();
    let workflows_section = section_tokens(assembly, "workflows");
    if !workflows.is_empty() || workflows_section > 0 {
        let tokens = if workflows_section > 0 {
            workflows_section
        } else {
            let mut text = String::new();
            for (name, desc) in &workflows {
                text.push_str(name);
                text.push('\n');
                text.push_str(desc);
                text.push('\n');
            }
            estimate_text(&text)
        };
        rows.push(ContextCategory {
            label: "工作流".into(),
            tokens,
            detail: Some(if tokens > 0 {
                format!("{} 个 · 已计入系统提示", workflows.len().max(1))
            } else {
                count_detail(workflows.len() as u64, "个")
            }),
        });
    }
    let (_, mcp_specs, deferred_specs) = partition_model_specs(ctx);
    if !mcp_specs.is_empty() {
        let mut servers = std::collections::BTreeSet::new();
        for spec in &mcp_specs {
            if let Some((server, _)) = split_mcp_public_name(&spec.name) {
                servers.insert(server.to_string());
            }
        }
        rows.push(ContextCategory {
            label: "MCP 服务器".into(),
            tokens: 0,
            detail: Some(format!(
                "{} 台 · {} 个工具 · 未计入窗口",
                servers.len().max(1),
                mcp_specs.len()
            )),
        });
    }
    if !deferred_specs.is_empty() {
        rows.push(ContextCategory {
            label: "本地按需".into(),
            tokens: 0,
            detail: Some(format!("{} 个工具 · 未计入窗口", deferred_specs.len())),
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

fn partition_model_specs(ctx: &Context) -> (Vec<ToolSpec>, Vec<ToolSpec>, Vec<ToolSpec>) {
    let Some(tools) = ctx.get::<Tools>(TOOLS) else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let mut builtin = Vec::new();
    let mut mcp = Vec::new();
    let mut deferred = Vec::new();
    for spec in tools.specs_for_model_on(ctx) {
        builtin.push(spec);
    }
    for spec in tools.specs() {
        if tools.is_mcp(&spec.name) || is_mcp_public_name(&spec.name) {
            mcp.push(spec);
        } else if tools.is_deferred(&spec.name) {
            deferred.push(spec);
        }
    }
    (builtin, mcp, deferred)
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
        let text = d.text.clone().unwrap_or_default();
        assert!(
            text.contains("test agent") || text.contains("You are"),
            "text={text}"
        );
        assert!(d.tokens > 0);
        assert!(
            d.groups
                .iter()
                .flat_map(|g| &g.rows)
                .any(|r| r.label == "基座"),
            "{d:?}"
        );
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

    #[tokio::test]
    async fn mcp_schemas_are_listed_but_not_counted_in_used() {
        let ctx = Context::new();
        crate::bundle::install_fakes(&ctx).await.unwrap();
        let tools = ctx.get::<Tools>(TOOLS).unwrap();
        let body: crate::tools::ToolBody = std::sync::Arc::new(|call| {
            Box::pin(async move { crate::tools::tool_result(call, "") })
        });
        let mcp_spec = ToolSpec {
            name: "mcp_probe__ping".into(),
            description: "mcp occupancy probe with a longer description".into(),
            parameters_json: r#"{"type":"object","properties":{"q":{"type":"string"}}}"#.into(),
        };
        let hidden = estimate_tool_definitions(std::slice::from_ref(&mcp_spec));
        let _keep = tools.register_mcp(mcp_spec, body).unwrap();
        let snap = snapshot_context(&ctx);
        let mcp_cat = snap
            .categories
            .iter()
            .find(|c| c.label == "MCP 服务器")
            .expect("MCP legend");
        assert_eq!(mcp_cat.tokens, 0, "{mcp_cat:?}");
        assert!(
            mcp_cat
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("未计入窗口") && d.contains("1 个工具")),
            "{mcp_cat:?}"
        );
        let tools_detail = occupancy_detail(&ctx, OccupancyKind::Tools);
        assert!(
            !tools_detail
                .groups
                .iter()
                .flat_map(|g| &g.rows)
                .any(|r| r.label.contains("mcp_probe") || r.label == "ping"),
            "{tools_detail:?}"
        );
        let mcp = occupancy_detail(&ctx, OccupancyKind::Mcp);
        assert_eq!(mcp.tokens, 0);
        assert!(
            mcp.groups.iter().any(|g| g.heading.contains("probe")
                && g.rows
                    .iter()
                    .any(|r| r.label == "ping" && r.tokens.is_none())),
            "{mcp:?}"
        );
        assert_eq!(
            snap.used,
            snap.system_prompt_tokens
                .saturating_add(snap.message_tokens)
                .saturating_add(snap.tool_definitions_tokens)
        );
        assert!(hidden > 0);
    }
}
