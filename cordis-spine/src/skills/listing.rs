//! Budget-capped skill listing for system-prompt / `/context`.

use super::discover::SkillInfo;

/// Fraction of the context window (in characters ≈ tokens×4) for the listing.
pub const SKILL_BUDGET_PERCENT: f64 = 0.08;
const MAX_ENTRY_DESC: usize = 400;
const MIN_DESC: usize = 20;

pub fn listing_budget_chars(window_tokens: u64) -> usize {
    let window = if window_tokens == 0 {
        128_000
    } else {
        window_tokens
    };
    ((window as f64) * 4.0 * SKILL_BUDGET_PERCENT) as usize
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
    if skills.is_empty() {
        return String::new();
    }
    let header = "可用技能（用 `/name` 或 `skill` 工具加载全文；listing 只有名称与说明）：\n\n";
    let mut body = String::from(header);
    let mut included = 0usize;
    for skill in skills {
        let entry = format_entry(skill, true);
        if body.len() + entry.len() > budget_chars && included > 0 {
            break;
        }
        body.push_str(&entry);
        included += 1;
    }
    if included == 0 {
        body = String::from(header);
        for skill in skills {
            let entry = format_entry(skill, false);
            if body.len() + entry.len() > budget_chars && included > 0 {
                break;
            }
            body.push_str(&entry);
            included += 1;
        }
    }
    let rest = skills.len().saturating_sub(included);
    if rest > 0 {
        body.push_str(&format!("… 还有 {rest} 个技能未列入（超出占用预算）。\n"));
    }
    body
}

fn format_entry(skill: &SkillInfo, with_desc: bool) -> String {
    if !with_desc {
        return format!(
            "- `{}` ({})\n  {}\n",
            skill.name,
            skill.scope.label(),
            skill.listing_path()
        );
    }
    let mut desc = skill.description.clone();
    if let Some(when) = &skill.when_to_use {
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
        desc = skill.name.clone();
    }
    format!("- `{}` — {desc}\n  {}\n", skill.name, skill.listing_path())
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
        let mut meta = skill.listing_path();
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
    fn budget_is_eight_percent_of_window_chars() {
        assert_eq!(listing_budget_chars(128_000), 40_960);
        assert_eq!(listing_budget_chars(0), 40_960);
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
}
