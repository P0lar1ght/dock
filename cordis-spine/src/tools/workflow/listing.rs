//! Workflow listing for system-prompt / `/context`. Budget + rendering rules
//! are shared with skills in [`crate::prompt::listing`]; this file only supplies the
//! header and the workflow-shaped row.

use crate::prompt::listing::{self, ListEntry};

use super::registry::WorkflowListing;

const HEADER: &str =
    "可用工作流（用 `/name` 或 `workflow` 工具按注册名启动；脚本在启动时加载，listing 只有名称与说明）：\n\n";

pub fn listing_budget_chars(window_tokens: u64) -> usize {
    listing::budget_chars(window_tokens)
}

impl ListEntry for WorkflowListing {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn when_to_use(&self) -> Option<&str> {
        self.when_to_use.as_deref()
    }

    fn listing_path(&self) -> Option<String> {
        match self.source {
            "project" => Some(format!(".dock/workflows/{}.rhai", self.name)),
            "user" => Some(format!("~/.dock/workflows/{}.rhai", self.name)),
            "bundled" => Some(format!("~/.dock/bundled/workflows/{}.rhai", self.name)),
            _ => None,
        }
    }
}

pub fn render_listing(workflows: &[WorkflowListing], budget_chars: usize) -> String {
    let refs: Vec<&WorkflowListing> = workflows.iter().collect();
    listing::render(HEADER, &refs, budget_chars)
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
    fn budget_is_three_percent_of_window_chars() {
        assert_eq!(listing_budget_chars(128_000), 15_360);
        assert_eq!(listing_budget_chars(0), 15_360);
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

    #[test]
    fn listing_uses_generic_paths() {
        let workflows = vec![sample(
            "demo-flow",
            "a reasonably long description for listing",
        )];
        let text = render_listing(&workflows, 800);
        assert!(text.contains(".dock/workflows/demo-flow.rhai"), "{text}");
        assert!(!text.contains("/tmp/"), "{text}");
    }

    /// The header names `workflow`, so that tool has to be on the sampler table.
    #[test]
    fn header_points_at_a_sampler_tool() {
        let workflows = vec![sample("demo-flow", "a reasonably long description here")];
        let text = render_listing(&workflows, 800);
        assert!(text.contains("`workflow` 工具"), "{text}");
    }
}
