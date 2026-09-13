//! Skill listing for system-prompt / `/context`. Budget + rendering rules are
//! shared with workflows in [`crate::listing`]; this file only supplies the
//! header and the skill-shaped row.

use crate::listing::{self, ListEntry};

use super::discover::SkillInfo;

const HEADER: &str = "可用技能（用 `/name` 或 `skill` 工具加载全文；listing 只有名称与说明）：\n\n";

pub fn listing_budget_chars(window_tokens: u64) -> usize {
    listing::budget_chars(window_tokens)
}

impl ListEntry for SkillInfo {
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
        Some(self.display_path())
    }

    fn short_tag(&self) -> Option<&str> {
        Some(self.scope.label())
    }
}

pub fn listable<'a>(
    skills: &'a [SkillInfo],
    activated: &std::collections::HashSet<String>,
) -> Vec<&'a SkillInfo> {
    skills
        .iter()
        .filter(|s| {
            if s.disable_model_invocation {
                return false;
            }
            if let Some(paths) = s.paths.as_ref() {
                if !paths.is_empty() && !activated.contains(&s.name) {
                    return false;
                }
            }
            true
        })
        .collect()
}

pub fn render_listing(skills: &[&SkillInfo], budget_chars: usize) -> String {
    listing::render(HEADER, skills, budget_chars)
}

pub fn overlay_body(skills: &[SkillInfo]) -> String {
    if skills.is_empty() {
        return "没有发现技能。\n把 SKILL.md 放到 skills/、~/.dock/skills/、.dock/skills/ 或 .agents/skills/。".into();
    }
    let mut lines = vec![format!("{} 个技能", skills.len()), String::new()];
    for skill in skills {
        let inv = if skill.user_invocable {
            format!("/{}", skill.name)
        } else {
            "（不可斜杠）".into()
        };
        lines.push(format!(
            "{}  ·  {}  ·  {}",
            inv,
            skill.scope.label(),
            skill.description
        ));
        let mut meta = skill.display_path();
        if let Some(license) = skill.license.as_deref().filter(|l| !l.is_empty()) {
            meta.push_str("  ·  ");
            meta.push_str(license);
        }
        lines.push(format!("  {meta}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::discover::{SkillInfo, SkillScope};
    use std::path::PathBuf;

    fn sample(name: &str, desc: &str) -> SkillInfo {
        SkillInfo {
            name: name.into(),
            description: desc.into(),
            when_to_use: None,
            license: None,
            paths: None,
            user_invocable: true,
            disable_model_invocation: false,
            path: PathBuf::from(format!("/tmp/{name}/SKILL.md")),
            dir: PathBuf::from(format!("/tmp/{name}")),
            scope: SkillScope::Project,
        }
    }

    #[test]
    fn budget_is_three_percent_of_window_chars() {
        assert_eq!(listing_budget_chars(128_000), 15_360);
        assert_eq!(listing_budget_chars(0), 15_360);
    }

    #[test]
    fn budget_drops_later_entries() {
        let skills: Vec<SkillInfo> = (0..40)
            .map(|i| sample(&format!("skill-{i:02}"), &"d".repeat(300)))
            .collect();
        let refs: Vec<&SkillInfo> = skills.iter().collect();
        let text = render_listing(&refs, 800);
        assert!(text.contains("`skill-00`"), "{text}");
        assert!(text.contains("未列入"), "{text}");
        assert!(!text.contains("`skill-39`"), "{text}");
    }

    #[test]
    fn disable_model_invocation_omits_from_listing() {
        let mut hidden = sample("hidden", "secret workflow");
        hidden.disable_model_invocation = true;
        let shown = sample("shown", "visible workflow for tests");
        let skills = [hidden, shown];
        let activated = std::collections::HashSet::new();
        let list = listable(&skills, &activated);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "shown");
    }

    #[test]
    fn overlay_shows_license_when_present() {
        let mut licensed = sample("mcp-builder", "MCP 服务器开发指南");
        licensed.license = Some("Complete terms in LICENSE.txt".into());
        let plain = sample("demo", "没有 license 的技能");
        let text = overlay_body(&[licensed, plain]);
        assert!(text.contains("Complete terms in LICENSE.txt"), "{text}");
        // 没写 license 的技能，路径行不带额外的 ` · `。
        let demo_path = text
            .lines()
            .find(|l| l.trim().ends_with(".dock/skills/demo/SKILL.md"))
            .expect("demo path line");
        assert_eq!(demo_path.trim(), ".dock/skills/demo/SKILL.md");
    }

    #[test]
    fn listing_uses_generic_paths() {
        let skill = sample("demo", "a reasonably long description for listing");
        let text = render_listing(&[&skill], 800);
        assert!(text.contains(".dock/skills/demo/SKILL.md"), "{text}");
        assert!(!text.contains("/tmp/"), "{text}");
    }

    /// The header names `skill`, so that tool has to be on the sampler table.
    #[test]
    fn header_points_at_a_sampler_tool() {
        let skill = sample("demo", "a reasonably long description for listing");
        let text = render_listing(&[&skill], 800);
        assert!(text.contains("`skill` 工具"), "{text}");
    }
}
