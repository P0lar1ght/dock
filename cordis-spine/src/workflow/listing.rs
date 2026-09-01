//! Budget-capped workflow listing for system-prompt / `/context`.

use super::registry::WorkflowListing;

/// Fraction of the context window (in characters ≈ tokens×4) for the listing.
pub const WORKFLOW_BUDGET_PERCENT: f64 = 0.08;
const MAX_ENTRY_DESC: usize = 400;
const MIN_DESC: usize = 20;

pub fn listing_budget_chars(window_tokens: u64) -> usize {
    let window = if window_tokens == 0 {
        128_000
    } else {
        window_tokens
    };
    ((window as f64) * 4.0 * WORKFLOW_BUDGET_PERCENT) as usize
}

pub fn render_listing(workflows: &[WorkflowListing], budget_chars: usize) -> String {
    if workflows.is_empty() {
        return String::new();
    }
    let header =
        "可用工作流（用 `/name` 或 `workflow` 工具按注册名启动；脚本在启动时加载，listing 只有名称与说明）：\n\n";
    let mut body = String::from(header);
    let mut included = 0usize;
    for workflow in workflows {
        let entry = format_entry(workflow, true);
        if body.len() + entry.len() > budget_chars && included > 0 {
            break;
        }
        body.push_str(&entry);
        included += 1;
    }
    if included == 0 {
        body = String::from(header);
        for workflow in workflows {
            let entry = format_entry(workflow, false);
            if body.len() + entry.len() > budget_chars && included > 0 {
                break;
            }
            body.push_str(&entry);
            included += 1;
        }
    }
    let rest = workflows.len().saturating_sub(included);
    if rest > 0 {
        body.push_str(&format!("… 还有 {rest} 个工作流未列入（超出占用预算）。\n"));
    }
    body
}

fn format_entry(workflow: &WorkflowListing, with_desc: bool) -> String {
    if !with_desc {
        return format!("- `{}`\n", workflow.name);
    }
    let mut desc = workflow.description.clone();
    if let Some(when) = &workflow.when_to_use {
        if !when.is_empty() {
            desc.push_str(" Use when: ");
            desc.push_str(when);
        }
    }
    if desc.chars().count() > MAX_ENTRY_DESC {
        desc = desc.chars().take(MAX_ENTRY_DESC).collect();
        desc.push('…');
    }
    if desc.chars().count() < MIN_DESC {
        desc = workflow.name.clone();
    }
    let mut out = format!("- `{}` — {desc}\n", workflow.name);
    if let Some(path) = workflow.path.as_deref().filter(|p| !p.is_empty()) {
        out.push_str("  ");
        out.push_str(path);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str, desc: &str) -> WorkflowListing {
        WorkflowListing {
            name: name.into(),
            description: desc.into(),
            when_to_use: None,
            source: "project",
            path: Some(format!("/tmp/{name}.rhai")),
        }
    }

    #[test]
    fn budget_is_eight_percent_of_window_chars() {
        assert_eq!(listing_budget_chars(128_000), 40_960);
        assert_eq!(listing_budget_chars(0), 40_960);
    }

    #[test]
    fn budget_drops_later_entries() {
        let workflows: Vec<WorkflowListing> = (0..40)
            .map(|i| sample(&format!("wf-{i:02}"), &"d".repeat(300)))
            .collect();
        let text = render_listing(&workflows, 800);
        assert!(text.contains("`wf-00`"), "{text}");
        assert!(text.contains("未列入"), "{text}");
        assert!(!text.contains("`wf-39`"), "{text}");
    }

    #[test]
    fn empty_catalog_is_blank() {
        assert!(render_listing(&[], 800).is_empty());
    }
}
