//! Named `"skills"` catalog + `skill` model tool. Fail-open.

mod builtin;
mod discover;
mod listing;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};

use crate::host::settings::AppSettings;
use crate::host::slash::{slash_name_reserved, ExtraSlashKind, Slash, SlashEntry};
use crate::names::{CONTEXT, PRE_STEP, SESSIONS, SETTINGS, SKILLS, SLASH, TOOLS, TOOLS_EXECUTE};
use crate::prompt::assemble::ORDER_SKILLS;
use crate::prompt::context_book::{own_sections, ContextBook};
use crate::prompt::listing::wants_listing;
use crate::session::log::Sessions;
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{LogEvent, PreStep, ToolCall, ToolResult, ToolSpec};

pub use discover::{apply_substitutions, extract_skill_body, SkillInfo, SkillScope};
pub use listing::{listable, listing_budget_chars, overlay_body, render_listing};

const SKILL_TOOL_DESC: &str = "Load a skill's SKILL.md body into this turn. Prefer `/name` (user slash) when the user invoked it; use this tool when a listed skill matches the task. `name` is the skill id from the system listing. Optional `args` fill $ARGUMENTS.";
const SKILL_TOOL_PARAMS: &str = r#"{"type":"object","properties":{"name":{"type":"string","description":"Skill id (slash name)."},"args":{"type":"string","description":"Optional arguments substituted for $ARGUMENTS."}},"required":["name"]}"#;

struct SkillsInner {
    catalog: Vec<SkillInfo>,
    announced: HashSet<String>,
    activated: HashSet<String>,
    extras: Vec<Disposable>,
    overlay: Option<Disposable>,
    /// First rendered listing, frozen so window/activation changes do not
    /// rewrite the system-prompt prefix (Grok never mutates system for skills).
    frozen_listing: Option<String>,
}

/// Named `"skills"` service. Live-look at the call site.
#[derive(Clone)]
pub struct Skills {
    ctx: Context,
    inner: Arc<Mutex<SkillsInner>>,
}

impl Skills {
    fn discover(ctx: Context) -> Self {
        let catalog = discover::scan_all();
        let announced = catalog.iter().map(|s| s.name.clone()).collect();
        let skills = Self {
            ctx,
            inner: Arc::new(Mutex::new(SkillsInner {
                catalog,
                announced,
                activated: HashSet::new(),
                extras: Vec::new(),
                overlay: None,
                frozen_listing: None,
            })),
        };
        skills.sync_slash();
        skills
    }

    pub fn catalog(&self) -> Vec<SkillInfo> {
        self.inner.lock().unwrap().catalog.clone()
    }

    pub fn listing_text(&self) -> String {
        {
            let inner = self.inner.lock().unwrap();
            if let Some(frozen) = inner.frozen_listing.clone() {
                return frozen;
            }
        }
        let window = self
            .ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.usage().window)
            .filter(|w| *w > 0)
            .or_else(|| {
                self.ctx.get::<AppSettings>(SETTINGS).and_then(|s| {
                    s.catalog()
                        .into_iter()
                        .find(|m| m.id == s.model())
                        .and_then(|m| m.context_window)
                })
            })
            .unwrap_or(128_000);
        let mut inner = self.inner.lock().unwrap();
        if let Some(frozen) = inner.frozen_listing.clone() {
            return frozen;
        }
        let list = listable(&inner.catalog, &inner.activated);
        let text = render_listing(&list, listing_budget_chars(window));
        inner.frozen_listing = Some(text.clone());
        text
    }

    /// Rows behind the `/context` 技能 slice.
    ///
    /// Derived from the *frozen* listing when one exists: the system prompt
    /// never rewrites its skills section, so re-running `listable` against the
    /// live activation set would report skills the model was never told about.
    pub fn occupancy_rows(&self) -> Vec<(String, u64, String)> {
        let inner = self.inner.lock().unwrap();
        let listed = inner
            .frozen_listing
            .as_ref()
            .map(|frozen| listed_names(frozen));
        let list = listable(&inner.catalog, &inner.activated);
        list.into_iter()
            .filter(|s| listed.as_ref().is_none_or(|names| names.contains(&s.name)))
            .map(|s| {
                let mut text = s.name.clone();
                text.push('\n');
                text.push_str(&s.description);
                (
                    s.name.clone(),
                    occupancy_estimate(&text),
                    s.description.clone(),
                )
            })
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<SkillInfo> {
        self.inner
            .lock()
            .unwrap()
            .catalog
            .iter()
            .find(|s| s.name == name)
            .cloned()
    }

    /// paths 渐进披露：匹配文件被触碰过才激活（对齐 Grok，激活前不进 listing
    /// 也不可被 `skill` 工具加载；`/name` 用户斜杠不受限）。
    pub fn is_activated(&self, name: &str) -> bool {
        self.inner.lock().unwrap().activated.contains(name)
    }

    pub fn load(&self, name: &str, args: &str) -> Result<(SkillInfo, String), String> {
        let skill = self
            .get(name)
            .ok_or_else(|| format!("unknown skill {name:?}"))?;
        let raw = std::fs::read_to_string(&skill.path)
            .map_err(|e| format!("read {}: {e}", skill.path.display()))?;
        let body = apply_substitutions(&extract_skill_body(&raw), args, &skill.dir);
        Ok((skill, body))
    }

    fn inject_slash(&self, user: &str) {
        let Some((name, args)) = parse_slash_invoke(user) else {
            return;
        };
        if name == "skills" || slash_name_reserved(name) {
            return;
        }
        let Some(skill) = self.get(name) else {
            return;
        };
        if !skill.user_invocable {
            return;
        }
        let Ok((skill, body)) = self.load(name, args) else {
            return;
        };
        let Some(sessions) = self.ctx.get::<Sessions>(SESSIONS) else {
            return;
        };
        sessions.append(LogEvent::SystemReminder(format_skill_block(
            &skill, args, &body,
        )));
    }

    fn notice_paths(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let mut discovered = Vec::new();
        for path in paths {
            if let Some(skill_md) = skill_md_near(path) {
                if let Some(skill) =
                    discover::parse_skill_file(&skill_md, discover::SkillScope::Project)
                {
                    discovered.push(skill);
                }
            }
            if path.is_dir() {
                for skill in discover::scan_dir(path, discover::SkillScope::Project) {
                    discovered.push(skill);
                }
            }
        }
        if discovered.is_empty() && !paths.iter().any(|p| discover::path_near_skills(p)) {
            self.activate_for_paths(paths);
            return;
        }
        let mut new_names = Vec::new();
        {
            let mut inner = self.inner.lock().unwrap();
            for skill in discovered {
                if inner.catalog.iter().any(|s| s.path == skill.path) {
                    continue;
                }
                if inner.catalog.iter().any(|s| s.name == skill.name) {
                    inner.catalog.retain(|s| s.name != skill.name);
                }
                if !inner.announced.contains(&skill.name) {
                    new_names.push(skill.name.clone());
                    inner.announced.insert(skill.name.clone());
                }
                inner.catalog.push(skill);
            }
        }
        self.activate_for_paths(paths);
        if !new_names.is_empty() {
            self.sync_slash();
            if let Some(sessions) = self.ctx.get::<Sessions>(SESSIONS) {
                sessions.append(LogEvent::SystemReminder(format!(
                    "发现新技能：{}。用 `/name` 或 skill 工具加载全文。",
                    new_names.join("、")
                )));
            }
        }
    }

    fn activate_for_paths(&self, paths: &[PathBuf]) {
        let mut newly = Vec::new();
        {
            let mut inner = self.inner.lock().unwrap();
            let names: Vec<String> = inner
                .catalog
                .iter()
                .filter(|skill| {
                    skill
                        .paths
                        .as_ref()
                        .is_some_and(|patterns| discover::paths_gate_match(patterns, paths))
                })
                .map(|skill| skill.name.clone())
                .collect();
            for name in names {
                if inner.activated.insert(name.clone()) {
                    newly.push(name);
                }
            }
        }
        if newly.is_empty() {
            return;
        }
        if let Some(sessions) = self.ctx.get::<Sessions>(SESSIONS) {
            sessions.append(LogEvent::SystemReminder(format!(
                "技能现已可用：{}。用 `/name` 或 skill 工具加载全文。",
                newly.join("、")
            )));
        }
    }

    fn sync_slash(&self) {
        let Some(slash) = self.ctx.get::<Slash>(SLASH) else {
            return;
        };
        let catalog = self.catalog();
        let overlay_text = overlay_body(&catalog);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.extras.clear();
            inner.overlay = None;
        }
        let overlay = slash
            .register(SlashEntry {
                command: "skills".into(),
                description: "列出已发现的技能".into(),
                kind: ExtraSlashKind::Overlay,
                text: overlay_text,
                title: "技能".into(),
                send: false,
            })
            .ok();
        let mut extras = Vec::new();
        for skill in &catalog {
            if !skill.user_invocable || skill.name == "skills" {
                continue;
            }
            if slash_name_reserved(&skill.name) {
                continue;
            }
            let entry = SlashEntry {
                command: skill.name.clone(),
                description: skill.description.clone(),
                kind: ExtraSlashKind::Prompt,
                text: format!("/{}", skill.name),
                title: String::new(),
                send: true,
            };
            if let Ok(d) = slash.register(entry) {
                extras.push(d);
            }
        }
        let mut inner = self.inner.lock().unwrap();
        inner.overlay = overlay;
        inner.extras = extras;
    }
}

/// Entry names actually present in a rendered listing (`- \`name\` …`).
fn listed_names(listing: &str) -> HashSet<String> {
    listing
        .lines()
        .filter_map(|line| line.strip_prefix("- `"))
        .filter_map(|rest| rest.split('`').next())
        .map(str::to_string)
        .collect()
}

fn occupancy_estimate(text: &str) -> u64 {
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

fn format_skill_block(skill: &SkillInfo, args: &str, body: &str) -> String {
    if args.is_empty() {
        format!(
            "<skill name=\"{}\" path=\"{}\">\n{body}\n</skill>",
            skill.name,
            skill.display_path()
        )
    } else {
        format!(
            "<skill name=\"{}\" args=\"{}\" path=\"{}\">\n{body}\n</skill>",
            skill.name,
            args,
            skill.display_path()
        )
    }
}

fn parse_slash_invoke(user: &str) -> Option<(&str, &str)> {
    let t = user.trim();
    let rest = t.strip_prefix('/')?;
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim()),
        None => (rest, ""),
    };
    if name.is_empty() {
        return None;
    }
    Some((name, args))
}

fn skill_md_near(path: &std::path::Path) -> Option<PathBuf> {
    if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") && path.is_file() {
        return Some(path.to_path_buf());
    }
    let mut dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    for _ in 0..6 {
        let candidate = dir.join("SKILL.md");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !discover::path_near_skills(&dir) {
            break;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => break,
        }
    }
    None
}

fn sibling_files(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == "SKILL.md" {
                None
            } else {
                Some(name)
            }
        })
        .collect();
    names.sort();
    names.truncate(10);
    names
}

pub fn skills() -> Plugin {
    plugin("skills", Inject::from([SLASH, CONTEXT]), |ctx, _: &()| {
        let handle = Skills::discover(ctx.clone());
        let book = ctx.require::<ContextBook>(CONTEXT)?;
        own_sections(
            ctx,
            vec![book.section(ORDER_SKILLS, "skills", |exec| {
                if !wants_listing(exec, "skill") {
                    return None;
                }
                exec.get::<Skills>(SKILLS).and_then(|skills| {
                    let listing = skills.listing_text();
                    if listing.trim().is_empty() {
                        None
                    } else {
                        Some(listing)
                    }
                })
            })?],
        )?;
        let ctx_pre = ctx.clone();
        let _ = ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
            let next = args.next::<PreStep>().unwrap_or(step);
            // Main session only: `/name args` comes from the prompt bar, and
            // the SKILL.md body can only be appended to the main session.
            if next.enter && next.is_main_session() {
                if let Some(skills) = ctx_pre.get::<Skills>(SKILLS) {
                    skills.inject_slash(&next.user);
                }
            }
            next
        });
        let ctx_exec = ctx.clone();
        let _ = ctx.on_waterfall(TOOLS_EXECUTE, move |result: ToolResult, args| {
            let result = args.next::<ToolResult>().unwrap_or(result);
            if let Some(skills) = ctx_exec.get::<Skills>(SKILLS) {
                let call_args = arguments_for(&ctx_exec, &result);
                let paths =
                    discover::extract_paths_from_tool(&result.name, &call_args, &result.content);
                if !paths.is_empty() {
                    skills.notice_paths(&paths);
                }
            }
            result
        });
        Ok(Some(ctx.provide(SKILLS, handle)?))
    })
}

fn arguments_for(ctx: &Context, result: &ToolResult) -> String {
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return String::new();
    };
    sessions
        .events()
        .into_iter()
        .rev()
        .find_map(|e| match e {
            LogEvent::ToolExecute { id, arguments, .. } if id == result.call_id => Some(arguments),
            _ => None,
        })
        .unwrap_or_default()
}

pub fn tool_skills() -> Plugin {
    plugin("tool-skills", Inject::from([TOOLS]), |ctx, _: &()| {
        let tools = ctx.require::<Tools>(TOOLS)?;
        let body: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { run_skill_tool(&ctx, call) })
            })
        };
        own_registered(
            ctx,
            // On the sampler table, not deferred: the system-prompt listing
            // names this tool, so it has to be callable without a
            // `search_tool` round-trip first.
            vec![tools.register(
                ToolSpec {
                    name: "skill".into(),
                    description: SKILL_TOOL_DESC.into(),
                    parameters_json: SKILL_TOOL_PARAMS.into(),
                },
                body,
            )?],
        )?;
        Ok(None)
    })
}

fn run_skill_tool(ctx: &Context, call: ToolCall) -> ToolResult {
    let v: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or_default();
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return tool_result(call, "Error: skill name is required");
    }
    let args = v
        .get("args")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let Some(skills) = ctx.get::<Skills>(SKILLS) else {
        return tool_result(call, "Error: skills plugin is not mounted");
    };
    let Some(meta) = skills.get(&name) else {
        let hint = skills
            .catalog()
            .into_iter()
            .map(|s| format!("{} ({})", s.name, s.display_path()))
            .take(8)
            .collect::<Vec<_>>()
            .join("\n");
        return tool_result(
            call,
            format!("Error: unknown skill {name:?}. Known:\n{hint}"),
        );
    };
    if meta.disable_model_invocation {
        return tool_result(
            call,
            format!("Error: skill {name:?} is user-invocable only (`/{name}`)"),
        );
    }
    if let Some(patterns) = meta.paths.as_ref().filter(|p| !p.is_empty()) {
        if !skills.is_activated(&name) {
            return tool_result(
                call,
                format!(
                    "Error: skill {name:?} is gated on paths [{}] and not active yet; it unlocks when a matching file is touched in this session.",
                    patterns.join(", ")
                ),
            );
        }
    }
    match skills.load(&name, &args) {
        Ok((skill, body)) => {
            let siblings = sibling_files(&skill.dir);
            let mut out = format_skill_block(&skill, &args, &body);
            if !siblings.is_empty() {
                out.push_str("\n\nFiles in skill dir:\n");
                for name in siblings {
                    out.push_str("- ");
                    out.push_str(&name);
                    out.push('\n');
                }
            }
            tool_result(call, out)
        }
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::assemble::{SystemPrompt, ORDER_SKILLS};
    use crate::prompt::context_usage::{occupancy_detail, snapshot_context, OccupancyKind};
    use cordis_base::types::PreStep;

    fn write_skill(root: &std::path::Path, name: &str, body: &str) {
        let skill_dir = root.join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), body).unwrap();
    }

    /// Mount both halves, like `install_app` does: `skills` owns the catalog
    /// and the listing, `tool-skills` registers the `skill` loader the listing
    /// header names. Without the loader the listing is (correctly) withheld.
    async fn mount_skills(ctx: &Context) {
        crate::install_without_llm(ctx).await.unwrap();
        ctx.plugin(crate::slash(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        ctx.plugin(skills(), ()).unwrap().wait().await.unwrap();
        ctx.plugin(tool_skills(), ()).unwrap().wait().await.unwrap();
    }

    #[test]
    fn order_skills_follows_roster() {
        const { assert!(ORDER_SKILLS > crate::prompt::assemble::ORDER_PERSONA) };
        const { assert!(ORDER_SKILLS > crate::prompt::assemble::ORDER_WORKFLOWS) };
    }

    #[tokio::test]
    async fn pre_step_injects_skill_body() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "demo-skill",
            "---\nname: demo-skill\ndescription: Demo skill for inject tests.\n---\n\nDo the demo with $ARGUMENTS.\n",
        );
        let _env = cordis_base::test_env::scoped().home().cwd(dir.path());
        let ctx = cordis::Context::new();
        mount_skills(&ctx).await;
        let skills = ctx.get::<Skills>(SKILLS).unwrap();
        assert!(skills.get("demo-skill").is_some());
        ctx.waterfall(
            PRE_STEP,
            PreStep::new("/demo-skill abc", true, crate::session::log::ROOT_IDENTITY),
            || PreStep::new("/demo-skill abc", true, crate::session::log::ROOT_IDENTITY),
        );
        let events = ctx.get::<Sessions>(SESSIONS).unwrap().events();
        let text = events
            .iter()
            .find_map(|e| match e {
                LogEvent::SystemReminder(t) => Some(t.as_str()),
                _ => None,
            })
            .expect("skill reminder");
        assert!(text.contains("<skill name=\"demo-skill\""), "{text}");
        assert!(text.contains("Do the demo with abc."), "{text}");
    }

    #[tokio::test]
    async fn reserved_slash_is_not_registered() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "help",
            "---\nname: help\ndescription: Must not shadow /help.\n---\n\nNope.\n",
        );
        let _env = cordis_base::test_env::scoped().home().cwd(dir.path());
        let ctx = cordis::Context::new();
        mount_skills(&ctx).await;
        let extras = ctx.get::<Slash>(SLASH).unwrap().list();
        assert!(extras.iter().any(|e| e.command == "skills"));
        assert!(!extras.iter().any(|e| e.command == "help"));
        assert!(extras
            .iter()
            .any(|e| e.command == "skills" && e.kind == ExtraSlashKind::Overlay));
    }

    #[tokio::test]
    async fn assemble_lists_skills_and_occupancy_does_not_double_count() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "demo-skill",
            "---\nname: demo-skill\ndescription: Demo skill for listing occupancy.\n---\n\nBody.\n",
        );
        let _env = cordis_base::test_env::scoped().home().cwd(dir.path());
        let ctx = cordis::Context::new();
        mount_skills(&ctx).await;
        let assembled = ctx
            .get::<SystemPrompt>(crate::names::SYSTEM_PROMPT)
            .unwrap()
            .assemble();
        assert!(assembled.contains("demo-skill"), "{assembled}");
        assert!(assembled.contains("可用技能"), "{assembled}");
        let snap = snapshot_context(&ctx);
        let extra = snap
            .categories
            .iter()
            .find(|c| c.label == "技能")
            .expect("技能 legend");
        assert!(extra.tokens > 0);
        assert!(
            extra
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("已计入系统提示")),
            "{extra:?}"
        );
        assert_eq!(
            snap.used,
            snap.system_prompt_tokens
                .saturating_add(snap.message_tokens)
                .saturating_add(snap.tool_definitions_tokens)
        );
        let detail = occupancy_detail(&ctx, OccupancyKind::Skills);
        assert_eq!(detail.kind, OccupancyKind::Skills);
        assert!(
            detail
                .groups
                .iter()
                .flat_map(|g| &g.rows)
                .any(|r| r.label == "demo-skill"),
            "{detail:?}"
        );
    }

    #[tokio::test]
    async fn occupancy_lists_skills_category_even_without_project_skills() {
        let dir = tempfile::tempdir().unwrap();
        let _env = cordis_base::test_env::scoped().home().cwd(dir.path());
        let ctx = cordis::Context::new();
        mount_skills(&ctx).await;
        let snap = snapshot_context(&ctx);
        let extra = snap
            .categories
            .iter()
            .find(|c| c.label == "技能")
            .expect("技能 legend even without project skills");
        // 项目层没有技能也要有图例；内置技能（dock-guide/dock-config）始终存在。
        assert!(extra.tokens > 0, "{extra:?}");
    }

    #[tokio::test]
    async fn skill_tool_returns_body_and_siblings() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "demo-skill",
            "---\nname: demo-skill\ndescription: Demo skill for the skill tool.\n---\n\nLoaded $ARGUMENTS.\n",
        );
        std::fs::write(
            dir.path()
                .join("skills")
                .join("demo-skill")
                .join("notes.md"),
            "x",
        )
        .unwrap();
        write_skill(
            dir.path(),
            "hidden-one",
            "---\nname: hidden-one\ndescription: User slash only.\ndisable-model-invocation: true\n---\n\nSecret.\n",
        );
        let _env = cordis_base::test_env::scoped().home().cwd(dir.path());
        let ctx = cordis::Context::new();
        mount_skills(&ctx).await;
        let tools = ctx.require::<Tools>(TOOLS).unwrap();
        let out = tools
            .execute(cordis_base::types::ToolCall {
                id: "s1".into(),
                name: "skill".into(),
                arguments: r#"{"name":"demo-skill","args":"zz"}"#.into(),
            })
            .await;
        assert!(
            out.content.contains("<skill name=\"demo-skill\""),
            "{}",
            out.content
        );
        assert!(out.content.contains("Loaded zz."), "{}", out.content);
        assert!(out.content.contains("notes.md"), "{}", out.content);
        let hidden = tools
            .execute(cordis_base::types::ToolCall {
                id: "s2".into(),
                name: "skill".into(),
                arguments: r#"{"name":"hidden-one"}"#.into(),
            })
            .await;
        assert!(
            hidden.content.contains("user-invocable only"),
            "{}",
            hidden.content
        );
    }

    #[tokio::test]
    async fn tools_execute_discovers_new_skill() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "demo-skill",
            "---\nname: demo-skill\ndescription: Already known.\n---\n\nOld.\n",
        );
        let _env = cordis_base::test_env::scoped().home().cwd(dir.path());
        let ctx = cordis::Context::new();
        mount_skills(&ctx).await;
        let late = dir.path().join(".dock").join("skills").join("late-skill");
        std::fs::create_dir_all(&late).unwrap();
        let skill_md = late.join("SKILL.md");
        std::fs::write(
            &skill_md,
            "---\nname: late-skill\ndescription: Discovered mid session.\n---\n\nLate body.\n",
        )
        .unwrap();
        let result = ToolResult {
            call_id: "w1".into(),
            name: "write_file".into(),
            content: format!("created {}", skill_md.display()),
            ..Default::default()
        };
        ctx.waterfall(TOOLS_EXECUTE, result.clone(), move || result);
        let skills = ctx.get::<Skills>(SKILLS).unwrap();
        assert!(skills.get("late-skill").is_some());
        let extras = ctx.get::<Slash>(SLASH).unwrap().list();
        assert!(
            extras.iter().any(|e| e.command == "late-skill"),
            "{extras:?}"
        );
        let events = ctx.get::<Sessions>(SESSIONS).unwrap().events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LogEvent::SystemReminder(t) if t.contains("late-skill"))),
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn gated_skill_requires_activation_before_tool_load() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "gated-one",
            "---\nname: gated-one\ndescription: Only for Cargo manifests.\npaths:\n  - \"**/Cargo.toml\"\n---\n\nCargo rules $ARGUMENTS.\n",
        );
        let _env = cordis_base::test_env::scoped().home().cwd(dir.path());
        let ctx = cordis::Context::new();
        mount_skills(&ctx).await;
        // 激活前：不进 listing，skill 工具拒绝加载。
        let listing = ctx.get::<Skills>(SKILLS).unwrap().listing_text();
        assert!(!listing.contains("gated-one"), "{listing}");
        let tools = ctx.require::<Tools>(TOOLS).unwrap();
        let blocked = tools
            .execute(cordis_base::types::ToolCall {
                id: "g1".into(),
                name: "skill".into(),
                arguments: r#"{"name":"gated-one"}"#.into(),
            })
            .await;
        assert!(
            blocked.content.contains("gated on paths"),
            "{}",
            blocked.content
        );
        // 触碰匹配路径 → 激活提示，之后工具可加载。
        let result = ToolResult {
            call_id: "g2".into(),
            name: "edit_file".into(),
            content: "updated Cargo.toml".into(),
            ..Default::default()
        };
        ctx.waterfall(TOOLS_EXECUTE, result.clone(), move || result);
        let events = ctx.get::<Sessions>(SESSIONS).unwrap().events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LogEvent::SystemReminder(t) if t.contains("gated-one"))),
            "{events:?}"
        );
        let ok = tools
            .execute(cordis_base::types::ToolCall {
                id: "g3".into(),
                name: "skill".into(),
                arguments: r#"{"name":"gated-one","args":"now"}"#.into(),
            })
            .await;
        assert!(ok.content.contains("Cargo rules now"), "{}", ok.content);
    }

    #[tokio::test]
    async fn builtin_skill_is_listed_and_loadable() {
        let dir = tempfile::tempdir().unwrap();
        // 一次 scoped() 只持一把进程锁：home + cwd 必须链在同一 guard 上。
        let _env = cordis_base::test_env::scoped().home().cwd(dir.path());
        let ctx = cordis::Context::new();
        mount_skills(&ctx).await;
        let skills = ctx.get::<Skills>(SKILLS).unwrap();
        let sc = skills.get("dock-guide").expect("builtin dock-guide");
        assert_eq!(sc.scope, discover::SkillScope::Builtin);
        let listing = skills.listing_text();
        assert!(listing.contains("dock-guide"), "{listing}");
        let tools = ctx.require::<Tools>(TOOLS).unwrap();
        let out = tools
            .execute(cordis_base::types::ToolCall {
                id: "b1".into(),
                name: "skill".into(),
                arguments: r#"{"name":"dock-guide"}"#.into(),
            })
            .await;
        assert!(
            out.content.contains("<skill name=\"dock-guide\""),
            "{}",
            out.content
        );
    }

    /// The system prompt freezes its skills section on first render, so the
    /// `/context` breakdown must report that frozen set — not whatever the
    /// live activation set has grown to since.
    #[test]
    fn listed_names_parses_rendered_entries() {
        let listing =
            "可用技能：\n\n- `alpha` — first\n  .dock/skills/alpha/SKILL.md\n- `beta` (项目)\n";
        let names = listed_names(listing);
        assert!(names.contains("alpha"), "{names:?}");
        assert!(names.contains("beta"), "{names:?}");
        assert_eq!(names.len(), 2, "{names:?}");
    }
}
