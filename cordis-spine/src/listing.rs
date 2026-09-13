//! Shared budget-capped catalog listing for the system prompt.
//!
//! Skills and workflows both advertise a name + one-line description in the
//! system prompt and load the full body on demand. The rendering rules (budget,
//! truncation, degraded second pass, overflow footer) are identical, so they
//! live here once and each caller supplies only its header and path shape.
//!
//! [`wants_listing`] is the shared gate: a listing goes out only to a session
//! that wants it *and* can actually call the loader its header names.

use cordis::Context;

use crate::agent_presets::AgentPresets;
use crate::names::{AGENT_PRESETS, SESSIONS, TOOLS};
use crate::session::{Sessions, ROOT_IDENTITY};
use crate::tools::Tools;

/// Fraction of the context window (chars ≈ tokens×4) one listing may occupy.
///
/// Two listings ship today (skills + workflows), so the standing worst case is
/// twice this. Kept deliberately small: a listing is an index, the body is
/// loaded on demand.
pub(crate) const BUDGET_PERCENT: f64 = 0.03;

const DEFAULT_WINDOW_TOKENS: u64 = 128_000;
/// Longest per-entry description before truncation.
pub(crate) const MAX_ENTRY_DESC: usize = 400;
/// Below this a description carries no signal; fall back to the name.
pub(crate) const MIN_DESC: usize = 20;

pub(crate) fn budget_chars(window_tokens: u64) -> usize {
    let window = if window_tokens == 0 {
        DEFAULT_WINDOW_TOKENS
    } else {
        window_tokens
    };
    ((window as f64) * 4.0 * BUDGET_PERCENT) as usize
}

/// Whether this session should carry a catalog listing in its system prompt.
///
/// `loader` is the tool the listing header tells the model to call (`skill`,
/// `workflow`). Two independent gates:
///
/// 1. **Who is asking.** The main session wants a listing; a child isolate does
///    only when its `agents/<type>.yml` sets `listings: true` — a narrow child
///    rarely loads a skill, and every concurrent child would otherwise re-pay
///    for the catalog.
/// 2. **Can they act on it.** The loader has to be visible to this session's
///    sampler. A preset whose allowlist omits `skill` (`warden`) would
///    otherwise be handed a catalog it cannot use: the sampler filters the tool
///    out, `search_tool` does not index it (it is not deferred), and `use_tool`
///    hits the same allowlist — so the listing only buys a failed call.
///
/// Fails open on both: no `"sessions"` counts as main, and no `"tools"` (unit
/// tests, bare harnesses) counts as visible.
pub(crate) fn wants_listing(exec: &Context, loader: &str) -> bool {
    let wanted = exec
        .get::<Sessions>(SESSIONS)
        .is_none_or(|s| s.identity() == ROOT_IDENTITY)
        || exec
            .get::<AgentPresets>(AGENT_PRESETS)
            .is_some_and(|p| p.wants_listings());
    wanted && loader_visible(exec, loader)
}

/// Whether the sampler for this session will be handed `loader`.
///
/// Asks the live `"tools"` table rather than re-deriving the rule, so this
/// cannot drift from what `Tools::specs_for_model_on` actually sends or from
/// what `Tools::execute_on` will accept.
fn loader_visible(exec: &Context, loader: &str) -> bool {
    let Some(tools) = exec.get::<Tools>(TOOLS) else {
        return true;
    };
    tools
        .specs_for_model_on(exec)
        .iter()
        .any(|s| s.name == loader)
}

/// One catalog row. `listing_path` is workspace- or home-relative — never an
/// absolute `/tmp/...`, which would leak the scan root into the prompt.
pub(crate) trait ListEntry {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn when_to_use(&self) -> Option<&str>;
    /// Second line under the entry. `None` renders no path line.
    fn listing_path(&self) -> Option<String>;
    /// Short tag shown in the degraded (description-less) form.
    fn short_tag(&self) -> Option<&str> {
        None
    }
}

/// `header` then as many entries as `budget_chars` allows.
///
/// Described entries come first. If not even one fits, a second pass drops the
/// descriptions so bare names still make it in — a catalog of names the model
/// can load on demand beats a header with nothing under it. That fallback pass
/// force-includes the first row, so the listing is never just a header. Any
/// remainder is reported so the model knows the listing is not exhaustive.
pub(crate) fn render<E: ListEntry>(header: &str, entries: &[&E], budget_chars: usize) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let (mut body, mut included) = fill(header, entries, budget_chars, true, false);
    if included == 0 {
        (body, included) = fill(header, entries, budget_chars, false, true);
    }
    let rest = entries.len().saturating_sub(included);
    if rest > 0 {
        body.push_str(&format!("… 还有 {rest} 个未列入（超出占用预算）。\n"));
    }
    body
}

fn fill<E: ListEntry>(
    header: &str,
    entries: &[&E],
    budget_chars: usize,
    with_desc: bool,
    force_first: bool,
) -> (String, usize) {
    let mut body = String::from(header);
    let mut included = 0usize;
    for entry in entries {
        let text = format_entry(*entry, with_desc);
        if body.len() + text.len() > budget_chars && !(force_first && included == 0) {
            break;
        }
        body.push_str(&text);
        included += 1;
    }
    (body, included)
}

fn format_entry<E: ListEntry>(entry: &E, with_desc: bool) -> String {
    let mut out = if with_desc {
        format!("- `{}` — {}\n", entry.name(), entry_desc(entry))
    } else {
        match entry.short_tag() {
            Some(tag) => format!("- `{}` ({tag})\n", entry.name()),
            None => format!("- `{}`\n", entry.name()),
        }
    };
    if let Some(path) = entry.listing_path() {
        out.push_str("  ");
        out.push_str(&path);
        out.push('\n');
    }
    out
}

fn entry_desc<E: ListEntry>(entry: &E) -> String {
    let mut desc = entry.description().to_string();
    if let Some(when) = entry.when_to_use().filter(|w| !w.is_empty()) {
        desc.push_str(" Use when: ");
        desc.push_str(when);
    }
    if desc.chars().count() > MAX_ENTRY_DESC {
        desc = desc.chars().take(MAX_ENTRY_DESC).collect();
        desc.push('…');
    }
    if desc.chars().count() < MIN_DESC {
        desc = entry.name().to_string();
    }
    desc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Put a loader on the live `"tools"` table so `wants_listing` sees it.
    fn register_loader(ctx: &Context, name: &str) -> cordis::Disposable {
        let tools = ctx.get::<Tools>(TOOLS).expect("tools mounted");
        let body: crate::tools::ToolBody = std::sync::Arc::new(|call| {
            Box::pin(async move { crate::tools::tool_result(call, "") })
        });
        tools
            .register(
                crate::types::ToolSpec {
                    name: name.into(),
                    description: format!("{name} loader"),
                    parameters_json: r#"{"type":"object"}"#.into(),
                },
                body,
            )
            .unwrap()
    }

    struct Row {
        name: String,
        desc: String,
        when: Option<String>,
        path: Option<String>,
        tag: Option<String>,
    }

    impl Row {
        fn new(name: &str, desc: &str) -> Self {
            Self {
                name: name.into(),
                desc: desc.into(),
                when: None,
                path: Some(format!(".dock/skills/{name}/SKILL.md")),
                tag: Some("项目".into()),
            }
        }
    }

    impl ListEntry for Row {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            &self.desc
        }
        fn when_to_use(&self) -> Option<&str> {
            self.when.as_deref()
        }
        fn listing_path(&self) -> Option<String> {
            self.path.clone()
        }
        fn short_tag(&self) -> Option<&str> {
            self.tag.as_deref()
        }
    }

    #[test]
    fn budget_is_three_percent_of_window_chars() {
        assert_eq!(budget_chars(128_000), 15_360);
        assert_eq!(budget_chars(0), 15_360);
        assert_eq!(budget_chars(200_000), 24_000);
    }

    #[test]
    fn budget_drops_later_entries_and_reports_the_rest() {
        let rows: Vec<Row> = (0..40)
            .map(|i| Row::new(&format!("row-{i:02}"), &"d".repeat(300)))
            .collect();
        let refs: Vec<&Row> = rows.iter().collect();
        let text = render("H\n\n", &refs, 800);
        assert!(text.contains("`row-00`"), "{text}");
        assert!(!text.contains("`row-39`"), "{text}");
        assert!(text.contains("未列入"), "{text}");
    }

    /// Budget too small even for one described entry: drop descriptions rather
    /// than emitting a header with nothing under it.
    #[test]
    fn degraded_pass_keeps_names_when_nothing_fits() {
        let rows: Vec<Row> = (0..4)
            .map(|i| Row::new(&format!("row-{i}"), &"d".repeat(5_000)))
            .collect();
        let refs: Vec<&Row> = rows.iter().collect();
        let text = render("H\n\n", &refs, 120);
        assert!(text.contains("`row-0` (项目)"), "{text}");
        assert!(!text.contains("ddd"), "{text}");
    }

    #[test]
    fn long_description_truncates_and_short_one_falls_back_to_name() {
        let long = Row::new("long", &"x".repeat(MAX_ENTRY_DESC + 50));
        let text = render("H\n\n", &[&long], 10_000);
        assert!(text.contains('…'), "{text}");
        assert!(!text.contains(&"x".repeat(MAX_ENTRY_DESC + 1)), "{text}");

        let short = Row::new("terse", "hi");
        let text = render("H\n\n", &[&short], 10_000);
        assert!(text.contains("- `terse` — terse"), "{text}");
    }

    #[test]
    fn when_to_use_is_appended() {
        let mut row = Row::new("demo", "a reasonably long description here");
        row.when = Some("editing configs".into());
        let text = render("H\n\n", &[&row], 10_000);
        assert!(text.contains("Use when: editing configs"), "{text}");
    }

    #[tokio::test]
    async fn main_session_and_bare_harness_get_listings() {
        let ctx = Context::new();
        assert!(
            wants_listing(&ctx, "skill"),
            "no sessions and no tools at all must fail open"
        );
        crate::bundle::install_fakes(&ctx).await.unwrap();
        let _skill = register_loader(&ctx, "skill");
        assert!(
            wants_listing(&ctx, "skill"),
            "main session holding the loader gets listings"
        );
    }

    /// The header names a loader; a session that cannot call it must not be
    /// handed the catalog. Covers `warden` (allowlist omits `skill`) and any
    /// harness where the loader plugin is simply not mounted.
    #[tokio::test]
    async fn an_uncallable_loader_withholds_the_listing() {
        let ctx = Context::new();
        crate::bundle::install_fakes(&ctx).await.unwrap();
        let _skill = register_loader(&ctx, "skill");
        assert!(wants_listing(&ctx, "skill"));
        assert!(
            !wants_listing(&ctx, "workflow"),
            "an unregistered loader withholds its listing"
        );

        let mut narrow = crate::agent_presets::AgentPreset::new("narrow");
        narrow.tools = Some(vec!["read_file".into()]);
        let scoped = ctx.isolate("agentPresets");
        scoped
            .provide(AGENT_PRESETS, AgentPresets::overlay(narrow))
            .unwrap();
        assert!(
            !wants_listing(&scoped, "skill"),
            "an allowlist without the loader withholds its listing"
        );
    }

    #[tokio::test]
    async fn child_session_is_opt_in() {
        let ctx = Context::new();
        crate::bundle::install_fakes(&ctx).await.unwrap();
        let _skill = register_loader(&ctx, "skill");
        let child = ctx.isolate("sessions").isolate("agentPresets");
        child
            .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-1"))
            .unwrap();

        let mut narrow = crate::agent_presets::SubagentDef {
            name: "explore".into(),
            ..Default::default()
        };
        narrow.listings = false;
        child
            .provide(
                AGENT_PRESETS,
                AgentPresets::overlay(narrow.to_preset("explore")),
            )
            .unwrap();
        assert!(
            !wants_listing(&child, "skill"),
            "narrow child must not carry listings"
        );

        let wide_ctx = ctx.isolate("sessions").isolate("agentPresets");
        wide_ctx
            .provide(SESSIONS, Sessions::isolated_as(wide_ctx.clone(), "child-2"))
            .unwrap();
        let wide = crate::agent_presets::SubagentDef {
            name: "general-purpose".into(),
            listings: true,
            ..Default::default()
        };
        wide_ctx
            .provide(
                AGENT_PRESETS,
                AgentPresets::overlay(wide.to_preset("general-purpose")),
            )
            .unwrap();
        assert!(
            wants_listing(&wide_ctx, "skill"),
            "opted-in child holding the loader carries listings"
        );
    }

    #[test]
    fn empty_catalog_renders_nothing() {
        let rows: Vec<&Row> = Vec::new();
        assert!(render("H\n\n", &rows, 800).is_empty());
    }

    #[test]
    fn entry_without_path_renders_one_line() {
        let mut row = Row::new("flow", "a reasonably long description here");
        row.path = None;
        row.tag = None;
        let text = render("H\n\n", &[&row], 10_000);
        assert_eq!(text, "H\n\n- `flow` — a reasonably long description here\n");
    }
}
