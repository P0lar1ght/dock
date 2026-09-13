//! Workspace engineering conventions: `AGENTS.md` into the system prompt.
//!
//! Two layers, user then project, both optional and both appended (the project
//! file adds to the user file, it does not shadow it):
//!
//! - `~/.dock/AGENTS.md` — conventions the user wants in every workspace
//! - `{cwd}/AGENTS.md` — this repository's own contract
//!
//! Only the workspace root is read. Nested `AGENTS.md` under subdirectories is
//! deliberately out of scope: the precedence rules get hard to predict and the
//! budget gets hard to bound. Read those with `read_file` when they matter.
//!
//! Registered on `"context"` at [`ORDER_PROJECT`] (last, since it is the only
//! cwd-dependent section). Fail-open: a missing or unreadable file contributes
//! nothing and the plugin still goes Active.

use std::path::PathBuf;

use cordis::{plugin, Context, Inject, Plugin};

use crate::config::dock_home;
use crate::context_book::{own_sections, ContextBook};
use crate::names::{CONTEXT, SESSIONS, SETTINGS};
use crate::prompt::ORDER_PROJECT;
use crate::session::Sessions;
use crate::settings::AppSettings;

pub const INSTRUCTIONS_FILE: &str = "AGENTS.md";

/// Fraction of the context window (chars ≈ tokens×4) the conventions may take.
/// Larger than a catalog listing — this is prose the user wrote on purpose —
/// but still bounded so one runaway file cannot crowd out the conversation.
const BUDGET_PERCENT: f64 = 0.05;
const DEFAULT_WINDOW_TOKENS: u64 = 128_000;

const HEADER: &str = "# 工作区工程规约\n\n以下是本工作区的约定，优先级高于你的通用习惯。与用户的当轮指令冲突时，以用户当轮指令为准。\n";
const TRUNCATED: &str = "\n…（规约超出占用预算，已从尾部截断；需要全文用 read_file 读该文件）\n";

fn budget_chars(window_tokens: u64) -> usize {
    let window = if window_tokens == 0 {
        DEFAULT_WINDOW_TOKENS
    } else {
        window_tokens
    };
    ((window as f64) * 4.0 * BUDGET_PERCENT) as usize
}

/// User layer then project layer. Both optional.
pub fn instruction_paths() -> Vec<(&'static str, PathBuf)> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    vec![
        ("~/.dock/AGENTS.md", dock_home().join(INSTRUCTIONS_FILE)),
        ("AGENTS.md", cwd.join(INSTRUCTIONS_FILE)),
    ]
}

fn window_tokens(exec: &Context) -> u64 {
    exec.get::<Sessions>(SESSIONS)
        .map(|s| s.usage().window)
        .filter(|w| *w > 0)
        .or_else(|| {
            exec.get::<AppSettings>(SETTINGS).and_then(|s| {
                s.catalog()
                    .into_iter()
                    .find(|m| m.id == s.model())
                    .and_then(|m| m.context_window)
            })
        })
        .unwrap_or(DEFAULT_WINDOW_TOKENS)
}

/// Rendered section body, or `None` when neither layer has readable content.
pub fn section_body(exec: &Context) -> Option<String> {
    let mut layers = Vec::new();
    for (label, path) in instruction_paths() {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        if raw.trim().is_empty() {
            continue;
        }
        layers.push(format!("## {label}\n\n{}", raw.trim_end()));
    }
    if layers.is_empty() {
        return None;
    }
    Some(clamp(
        &format!("{HEADER}\n{}", layers.join("\n\n")),
        budget_chars(window_tokens(exec)),
    ))
}

/// Truncate from the tail on a char boundary, leaving room for the notice.
fn clamp(body: &str, budget_chars: usize) -> String {
    if body.len() <= budget_chars {
        return body.to_string();
    }
    let room = budget_chars.saturating_sub(TRUNCATED.len());
    let mut end = room.min(body.len());
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATED}", &body[..end])
}

pub fn project_instructions() -> Plugin {
    plugin(
        "project-instructions",
        Inject::from([CONTEXT]),
        |ctx, _: &()| {
            let book = ctx.require::<ContextBook>(CONTEXT)?;
            own_sections(
                ctx,
                vec![book.section(ORDER_PROJECT, "project", section_body)?],
            )?;
            Ok(None)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::CONTEXT;

    #[tokio::test]
    async fn missing_file_is_fail_open() {
        let dir = tempfile::tempdir().unwrap();
        let _env = crate::test_env::scoped().home().cwd(dir.path());
        let ctx = Context::new();
        ctx.plugin(crate::context_book::context(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        ctx.plugin(project_instructions(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert!(section_body(&ctx).is_none());
        let book = ctx.get::<ContextBook>(CONTEXT).unwrap();
        assert!(!book.assemble().render().contains("工程规约"));
    }

    #[tokio::test]
    async fn reads_cwd_agents_md_into_the_section() {
        let dir = tempfile::tempdir().unwrap();
        let _env = crate::test_env::scoped().home().cwd(dir.path());
        std::fs::write(dir.path().join("AGENTS.md"), "改完要跑 cargo test。").unwrap();
        let ctx = Context::new();
        ctx.plugin(crate::context_book::context(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        ctx.plugin(project_instructions(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let rendered = ctx.get::<ContextBook>(CONTEXT).unwrap().assemble().render();
        assert!(rendered.contains("改完要跑 cargo test。"), "{rendered}");
        assert!(rendered.contains("## AGENTS.md"), "{rendered}");
    }

    #[test]
    fn user_layer_comes_before_project_layer() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let _env = crate::test_env::scoped()
            .set("DOCK_HOME", &home)
            .cwd(dir.path());
        std::fs::write(home.join("AGENTS.md"), "USER-LAYER").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "PROJECT-LAYER").unwrap();
        let ctx = Context::new();
        let body = section_body(&ctx).expect("both layers");
        let user = body.find("USER-LAYER").expect("user layer present");
        let project = body.find("PROJECT-LAYER").expect("project layer present");
        assert!(user < project, "project layer appends after user: {body}");
    }

    #[test]
    fn blank_file_contributes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let _env = crate::test_env::scoped().home().cwd(dir.path());
        std::fs::write(dir.path().join("AGENTS.md"), "   \n\n").unwrap();
        let ctx = Context::new();
        assert!(section_body(&ctx).is_none());
    }

    #[test]
    fn nested_agents_md_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let _env = crate::test_env::scoped().home().cwd(dir.path());
        let nested = dir.path().join("crate-a");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "ROOT-RULES").unwrap();
        std::fs::write(nested.join("AGENTS.md"), "NESTED-RULES").unwrap();
        let ctx = Context::new();
        let body = section_body(&ctx).expect("root layer");
        assert!(body.contains("ROOT-RULES"), "{body}");
        assert!(!body.contains("NESTED-RULES"), "{body}");
    }

    #[test]
    fn budget_is_five_percent_of_window_chars() {
        assert_eq!(budget_chars(128_000), 25_600);
        assert_eq!(budget_chars(0), 25_600);
    }

    #[test]
    fn over_budget_truncates_on_a_char_boundary() {
        let body = "件".repeat(20_000);
        let out = clamp(&body, 1_000);
        assert!(out.len() <= 1_000, "len={}", out.len());
        assert!(out.ends_with(TRUNCATED), "{out}");
        assert!(out.contains('件'));
    }

    #[test]
    fn under_budget_is_untouched() {
        assert_eq!(clamp("short", 1_000), "short");
    }

    /// Last section on purpose: it is the only body that tracks the cwd, so
    /// everything above it stays byte-identical across `/cd` (prefix cache).
    #[tokio::test]
    async fn project_section_renders_after_the_listings() {
        let dir = tempfile::tempdir().unwrap();
        let _env = crate::test_env::scoped().home().cwd(dir.path());
        std::fs::write(dir.path().join("AGENTS.md"), "PROJECT-RULES").unwrap();
        let ctx = Context::new();
        ctx.plugin(crate::context_book::context(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let book = ctx.get::<ContextBook>(CONTEXT).unwrap();
        let _skills = book
            .section(crate::prompt::ORDER_SKILLS, "skills", |_| {
                Some("SKILLS-LISTING".into())
            })
            .unwrap();
        let _workflows = book
            .section(crate::prompt::ORDER_WORKFLOWS, "workflows", |_| {
                Some("WORKFLOW-LISTING".into())
            })
            .unwrap();
        ctx.plugin(project_instructions(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();

        let rendered = book.assemble().render();
        let workflows = rendered.find("WORKFLOW-LISTING").expect("workflows");
        let skills = rendered.find("SKILLS-LISTING").expect("skills");
        let project = rendered.find("PROJECT-RULES").expect("project");
        assert!(workflows < skills, "{rendered}");
        assert!(skills < project, "project section is last: {rendered}");

        let ids: Vec<String> = book
            .assemble()
            .inspect()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids.last().map(String::as_str), Some("project"), "{ids:?}");
    }
}
