//! Named `"agentPresets"`: roster + persona + tool allowlist.
//!
//! Each mode is a directory: `agent.yml` plus optional `agents/<type>.yml`
//! (that type's persona + Dock tool allowlist). Layers (later wins): crate
//! shipped < `~/.dock/presets` < project `.dock/presets`. Legacy `<id>.yml`
//! still loads. Drop a directory — no code change.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Inject, Plugin};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::names::AGENT_PRESETS;
use crate::types::ToolSpec;

pub const DEFAULT_PRESET_ID: &str = "code";
pub const MINIMAL_PRESET_ID: &str = "minimal";
pub const CORDIS_PRESET_ID: &str = "cordis";
pub const WARDEN_PRESET_ID: &str = "warden";

const PERSONA_MARK: &str = "# Agent 预设：";
const ROSTER_MARK: &str = "# 本模式子代理";
const BLOCKED: &str = "当前 Agent 预设未包含此工具。用 /preset 调整工具集。";
const ROSTER_FILE: &str = "roster.yml";
const AGENT_FILE: &str = "agent.yml";
const AGENTS_DIR: &str = "agents";
/// Leftover warden overlay filenames (`jia.yml`). Load as the Han id (`甲`).
const WARDEN_PINYIN_ALIASES: &[(&str, &str)] = &[
    ("cen", "岑"),
    ("suo", "锁"),
    ("jia", "甲"),
    ("yi", "乙"),
    ("bing", "丙"),
    ("heng", "衡"),
    ("yan", "验"),
    ("guan", "观"),
    ("strike", "突击"),
];
const FILE_HEADER: &str = "# Dock agent 预设。目录名即 id，须匹配 [a-z0-9][a-z0-9-]*。\n\
# 也可用内置模式的显示名做目录（如 创造/ 叠到 cordis，编码/ 叠到 code）。\n\
# 省略 tools = 当前进程全部已注册工具；[] = 空；列表 = 允许名单（Dock 工具名）。\n\
# replace_prompt: true 时 persona 就是整份系统提示。\n\
# 子代理写在 agents/<type>.yml（ascii 或汉字），不要写进本文件。\n";

struct ShippedMode {
    id: &'static str,
    agent: &'static str,
    agents: &'static [(&'static str, &'static str)],
}

const SHIPPED: &[ShippedMode] = &[
    ShippedMode {
        id: DEFAULT_PRESET_ID,
        agent: include_str!("../presets/code/agent.yml"),
        agents: &[
            (
                "general-purpose",
                include_str!("../presets/code/agents/general-purpose.yml"),
            ),
            (
                "explore",
                include_str!("../presets/code/agents/explore.yml"),
            ),
            ("plan", include_str!("../presets/code/agents/plan.yml")),
        ],
    },
    ShippedMode {
        id: MINIMAL_PRESET_ID,
        agent: include_str!("../presets/minimal/agent.yml"),
        agents: &[],
    },
    ShippedMode {
        id: CORDIS_PRESET_ID,
        agent: include_str!("../presets/cordis/agent.yml"),
        agents: &[
            (
                "general-purpose",
                include_str!("../presets/cordis/agents/general-purpose.yml"),
            ),
            (
                "explore",
                include_str!("../presets/cordis/agents/explore.yml"),
            ),
            ("plan", include_str!("../presets/cordis/agents/plan.yml")),
        ],
    },
    ShippedMode {
        id: WARDEN_PRESET_ID,
        agent: include_str!("../presets/warden/agent.yml"),
        agents: &[
            ("岑", include_str!("../presets/warden/agents/岑.yml")),
            ("锁", include_str!("../presets/warden/agents/锁.yml")),
            ("甲", include_str!("../presets/warden/agents/甲.yml")),
            ("乙", include_str!("../presets/warden/agents/乙.yml")),
            ("丙", include_str!("../presets/warden/agents/丙.yml")),
            ("衡", include_str!("../presets/warden/agents/衡.yml")),
            ("验", include_str!("../presets/warden/agents/验.yml")),
            ("观", include_str!("../presets/warden/agents/观.yml")),
            ("突击", include_str!("../presets/warden/agents/突击.yml")),
        ],
    },
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresetOrigin {
    Shipped,
    #[default]
    User,
    Project,
}

/// One YAML-defined child under a mode (`agents/<id>.yml`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubagentDef {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub persona: String,
    /// `None` = every live registered tool. `Some` = allowlist (order kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace_prompt: bool,
}

impl SubagentDef {
    pub fn to_preset(&self, id: &str) -> AgentPreset {
        AgentPreset {
            id: id.to_string(),
            name: if self.name.trim().is_empty() {
                id.to_string()
            } else {
                self.name.clone()
            },
            description: self.description.clone(),
            persona: self.persona.clone(),
            tools: self.tools.clone(),
            replace_prompt: self.replace_prompt,
            order: None,
            agents: IndexMap::new(),
            broken: None,
            origin: PresetOrigin::User,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentPreset {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub persona: String,
    /// `None` = every live registered tool. `Some` = allowlist (order kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Persona replaces the assembled system prompt instead of appending.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace_prompt: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    /// Child roster. Stored in `agents/*.yml`, not in `agent.yml`.
    #[serde(default, skip_serializing)]
    pub agents: IndexMap<String, SubagentDef>,
    #[serde(skip)]
    pub broken: Option<String>,
    #[serde(skip)]
    pub origin: PresetOrigin,
}

impl AgentPreset {
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            description: String::new(),
            persona: String::new(),
            tools: None,
            replace_prompt: false,
            order: None,
            agents: IndexMap::new(),
            broken: None,
            origin: PresetOrigin::User,
        }
    }

    pub fn builtin(&self) -> bool {
        self.origin == PresetOrigin::Shipped
    }
}

#[derive(Serialize, Deserialize)]
struct RosterFile {
    current: String,
}

struct Inner {
    user_dir: PathBuf,
    project_dir: Option<PathBuf>,
    current: String,
    presets: IndexMap<String, AgentPreset>,
    persist: bool,
}

/// Byte-stable write paths while cwd + mode stay the same (prompt-cache prefix).
#[derive(Clone)]
struct WriteHintSnap {
    cwd: PathBuf,
    mode: String,
    /// `{cwd}/.dock/presets` — stable across mode switches in the same workspace.
    presets_dir: String,
    /// `{cwd}/.dock/presets/{mode}/agents`
    agents_dir: String,
}

/// Named `agentPresets` service. TUI live-looks it; the loop filters specs
/// and execute at the call site.
#[derive(Clone)]
pub struct AgentPresets {
    inner: Arc<Mutex<Inner>>,
    write_hint: Arc<Mutex<Option<WriteHintSnap>>>,
}

impl AgentPresets {
    pub fn load(user_dir: PathBuf) -> Self {
        Self::load_layers(user_dir, None)
    }

    pub fn load_layers(user_dir: PathBuf, project_dir: Option<PathBuf>) -> Self {
        let inner = load_inner(user_dir, project_dir);
        Self {
            inner: Arc::new(Mutex::new(inner)),
            write_hint: Arc::new(Mutex::new(None)),
        }
    }

    /// In-memory overlay for a child isolate. Never writes disk.
    pub fn overlay(preset: AgentPreset) -> Self {
        let id = preset.id.clone();
        let mut presets = IndexMap::new();
        presets.insert(id.clone(), preset);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                user_dir: PathBuf::new(),
                project_dir: None,
                current: id,
                presets,
                persist: false,
            })),
            write_hint: Arc::new(Mutex::new(None)),
        }
    }

    /// `/cd`: point the project overlay at the new workspace and drop the
    /// write-path cache so the next assemble injects the new absolute dir.
    pub fn set_workspace_root(&self, cwd: &Path) {
        {
            let mut inner = self.inner.lock().unwrap();
            if !inner.persist {
                return;
            }
            inner.project_dir = Some(cwd.join(".dock").join("presets"));
        }
        *self.write_hint.lock().unwrap() = None;
        self.resync();
    }

    /// Re-read shipped + user + project YAML so a newly written
    /// `agents/<type>.yml` is spawnable in this session. Child overlays
    /// (`persist: false`) are a snapshot and are not reloaded. Keeps the
    /// in-memory current id when that preset still exists.
    pub fn resync(&self) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.persist {
            return;
        }
        let user = inner.user_dir.clone();
        let project = inner.project_dir.clone();
        let keep = inner.current.clone();
        let mut loaded = load_inner(user, project);
        if loaded.presets.contains_key(&keep) {
            loaded.current = keep;
        }
        *inner = loaded;
    }

    pub fn current_id(&self) -> String {
        self.inner.lock().unwrap().current.clone()
    }

    pub fn current(&self) -> AgentPreset {
        let inner = self.inner.lock().unwrap();
        inner
            .presets
            .get(&inner.current)
            .cloned()
            .unwrap_or_else(|| parse_shipped(DEFAULT_PRESET_ID).expect("shipped code"))
    }

    pub fn list(&self) -> Vec<AgentPreset> {
        self.inner
            .lock()
            .unwrap()
            .presets
            .values()
            .cloned()
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<AgentPreset> {
        self.inner.lock().unwrap().presets.get(id).cloned()
    }

    pub fn apply(&self, id: &str) -> Result<AgentPreset, String> {
        let mut inner = self.inner.lock().unwrap();
        let Some(preset) = inner.presets.get(id).cloned() else {
            return Err(format!("没有预设 {id}"));
        };
        if let Some(reason) = &preset.broken {
            return Err(reason.clone());
        }
        inner.current = id.to_string();
        persist_roster(&inner)?;
        Ok(preset)
    }

    pub fn create(&self) -> Result<AgentPreset, String> {
        let mut inner = self.inner.lock().unwrap();
        let id = mint_id(&inner.presets);
        let preset = AgentPreset {
            id: id.clone(),
            name: "未命名".into(),
            origin: persist_origin(&inner),
            ..AgentPreset::new(id.clone())
        };
        inner.presets.insert(id.clone(), preset.clone());
        inner.current = id.clone();
        persist_roster(&inner)?;
        persist_preset(&inner, &id)?;
        Ok(preset)
    }

    pub fn duplicate(&self, from: &str) -> Result<AgentPreset, String> {
        let mut inner = self.inner.lock().unwrap();
        let Some(mut preset) = inner.presets.get(from).cloned() else {
            return Err(format!("没有预设 {from}"));
        };
        if preset.broken.is_some() {
            return Err("损坏的预设不能复制".into());
        }
        let id = mint_id(&inner.presets);
        preset.id = id.clone();
        preset.name = format!("{} 副本", preset.name);
        preset.broken = None;
        preset.origin = persist_origin(&inner);
        preset.order = None;
        inner.presets.insert(id.clone(), preset.clone());
        inner.current = id.clone();
        persist_roster(&inner)?;
        persist_preset(&inner, &id)?;
        Ok(preset)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let Some(preset) = inner.presets.get(id).cloned() else {
            return Err(format!("没有预设 {id}"));
        };
        if preset.origin == PresetOrigin::Shipped {
            return Err("不能删除内置预设".into());
        }
        let dest = file_path(&inner, &preset);
        if let Some(mode_dir) = dest.parent() {
            let _ = std::fs::remove_dir_all(mode_dir);
        }
        let _ = std::fs::remove_file(legacy_yml(&inner, &preset));
        let _ = std::fs::remove_file(legacy_yml(&inner, &preset).with_extension("toml"));
        if is_shipped(id) {
            let mut restored = parse_shipped(id).expect("shipped");
            // Project overlay deleted: home file may still exist.
            if let Some(home) = read_dir_preset(&inner.user_dir, id, PresetOrigin::User) {
                restored = home;
            }
            inner.presets.insert(id.to_string(), restored);
        } else {
            inner.presets.shift_remove(id);
            if inner.current == id {
                inner.current = DEFAULT_PRESET_ID.into();
            }
        }
        persist_roster(&inner)
    }

    pub fn set_persona(&self, id: &str, persona: String) -> Result<(), String> {
        self.mutate(id, |preset| {
            preset.persona = persona;
            Ok(())
        })
    }

    pub fn add_tool(&self, id: &str, name: &str) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("工具名不能为空".into());
        }
        self.mutate(id, |preset| match &mut preset.tools {
            None => Ok(()),
            Some(list) if list.iter().any(|n| n == name) => Ok(()),
            Some(list) => {
                list.push(name.to_string());
                Ok(())
            }
        })
    }

    /// `live` is the current `"tools"` catalog. Removing from an open
    /// (all-tools) preset snapshots that catalog minus `name`.
    pub fn remove_tool(&self, id: &str, name: &str, live: &[String]) -> Result<(), String> {
        self.mutate(id, |preset| {
            match &preset.tools {
                None => {
                    preset.tools = Some(
                        live.iter()
                            .filter(|n| n.as_str() != name)
                            .cloned()
                            .collect(),
                    );
                }
                Some(_) => {
                    if let Some(list) = preset.tools.as_mut() {
                        list.retain(|n| n != name);
                    }
                }
            }
            Ok(())
        })
    }

    pub fn assigned_tools(&self, id: &str, live: &[String]) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let Some(preset) = inner.presets.get(id) else {
            return live.to_vec();
        };
        match &preset.tools {
            None => live.to_vec(),
            Some(list) => list.clone(),
        }
    }

    pub fn allows(&self, name: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        let Some(preset) = inner.presets.get(&inner.current) else {
            return true;
        };
        if preset.broken.is_some() {
            return true;
        }
        match &preset.tools {
            None => true,
            Some(allow) => tool_allowed(allow, name),
        }
    }

    pub fn filter_specs(&self, specs: Vec<ToolSpec>) -> Vec<ToolSpec> {
        let inner = self.inner.lock().unwrap();
        let Some(preset) = inner.presets.get(&inner.current) else {
            return specs;
        };
        if preset.broken.is_some() {
            return specs;
        }
        match &preset.tools {
            None => specs,
            Some(allow) => specs
                .into_iter()
                .filter(|s| tool_allowed(allow, &s.name))
                .collect(),
        }
    }

    pub fn persona_addon(&self) -> Option<String> {
        let preset = self.current();
        if preset.broken.is_some() || preset.replace_prompt {
            return None;
        }
        let text = preset.persona.trim();
        if text.is_empty() {
            return None;
        }
        Some(format!("{PERSONA_MARK}{}\n\n{text}", preset.name))
    }

    pub fn merge_persona(&self, assembled: &mut String) {
        self.resync();
        let preset = self.current();
        if preset.broken.is_some() {
            return;
        }
        let text = preset.persona.trim();
        if text.is_empty() {
            return;
        }
        if preset.replace_prompt {
            *assembled = text.to_string();
            return;
        }
        if assembled.contains(PERSONA_MARK) {
            return;
        }
        assembled.push_str("\n\n");
        assembled.push_str(&format!("{PERSONA_MARK}{}\n\n{text}", preset.name));
    }

    /// Parent-only: list YAML subagent types so the model knows `subagent_type` ids.
    /// Empty roster still injects write paths (new mode / first role).
    pub fn merge_subagent_roster(&self, assembled: &mut String) {
        self.resync();
        if !self.inner.lock().unwrap().persist {
            return;
        }
        let preset = self.current();
        if preset.broken.is_some() {
            return;
        }
        if assembled.contains(ROSTER_MARK) {
            return;
        }
        assembled.push_str("\n\n");
        assembled.push_str(ROSTER_MARK);
        assembled.push_str("\n用 subagent 委派并持续交流（send_message / list_agents / interrupt_agent）。idle 时 queued 与 urgent 都会立刻开下一轮；urgent 只在 running 时才是 send-now。interrupt_agent 不能叫醒 idle。子代理用 report 与你交流（可多轮多次），你看不到它们的助手正文。若收到「未调用 report」代转发，当作该轮消息，send_message 追问或改派，不要空等。");
        let has_task = match &preset.tools {
            None => true,
            Some(t) => t.iter().any(|n| n == "task"),
        };
        if has_task {
            assembled.push_str("一次性收集结果用 task + get_task_output。");
        }
        assembled.push_str(" 本轮给当前模式加了 agents/<id>.yml 后，先 subagent（reload_roster: true）再 spawn，enum 才会带上新 id。新建模式写完后用 /preset 应用该 id，不要指望 reload_roster 切模式。");
        assembled.push_str(&self.new_mode_write_hint());
        assembled.push_str(&self.new_role_write_hint());
        if preset.agents.is_empty() {
            assembled.push_str(" 当前模式尚无子代理。\n");
            return;
        }
        assembled.push_str(" subagent_type 为下列 id。\n");
        for (id, def) in &preset.agents {
            let name = def.name.trim();
            let desc = def.description.trim();
            if name.is_empty() || name == id {
                if desc.is_empty() {
                    assembled.push_str(&format!("- {id}\n"));
                } else {
                    assembled.push_str(&format!("- {id}：{desc}\n"));
                }
            } else if desc.is_empty() {
                assembled.push_str(&format!("- {id}：{name}\n"));
            } else {
                assembled.push_str(&format!("- {id}（{name}）：{desc}\n"));
            }
        }
    }

    pub fn current_roster(&self) -> IndexMap<String, SubagentDef> {
        self.resync();
        self.current().agents
    }

    pub fn subagent(&self, type_id: &str) -> Option<SubagentDef> {
        self.resync();
        self.current().agents.get(type_id).cloned()
    }

    /// Display name for a roster role (`agents/<id>.yml` `name:`).
    pub fn role_label(&self, type_id: &str) -> Option<String> {
        let preset = self.current();
        let def = preset.agents.get(type_id)?;
        let name = def.name.trim();
        Some(if name.is_empty() {
            type_id.to_string()
        } else {
            name.to_string()
        })
    }

    /// Appended to the model-facing `subagent` tool so it names this mode's roles.
    pub fn subagent_role_hint(&self) -> String {
        let roster = self.current_roster();
        if roster.is_empty() {
            return format!(
                " This Agent mode has no agents/ roles; add agents/<id>.yml under {}, then call with reload_roster true. To add a new Agent mode (not a role), write {}/<id>/agent.yml with id matching [a-z0-9][a-z0-9-]* then apply that id with /preset. Do not write ~/.dock/presets/ unless the user asks to save globally.",
                self.workspace_agents_dir(),
                self.workspace_presets_dir()
            );
        }
        let parts: Vec<String> = roster
            .iter()
            .map(|(id, def)| {
                let name = def.name.trim();
                if name.is_empty() || name == id {
                    id.clone()
                } else {
                    format!("{id} ({name})")
                }
            })
            .collect();
        format!(
            " subagent_type is a role id from this mode's agents/: {}. After writing a new agents/<id>.yml, call with reload_roster true (no spawn) to refresh this enum. Write new roles under {}/. New Agent modes go in {}/<id>/agent.yml (id [a-z0-9][a-z0-9-]*), then /preset apply; Han display-name dirs overlay a shipped mode and do not create one. Only ~/.dock/presets/ if the user asks to save globally.",
            parts.join(", "),
            self.workspace_agents_dir(),
            self.workspace_presets_dir()
        )
    }

    /// Absolute `{cwd}/.dock/presets` for the live workspace.
    /// Same cache key as [`Self::workspace_agents_dir`] (cwd + mode).
    pub fn workspace_presets_dir(&self) -> String {
        self.write_hint_snap().presets_dir
    }

    /// Absolute `{cwd}/.dock/presets/{mode}/agents` for the live workspace.
    /// Cached while cwd + mode are unchanged so the system prefix stays
    /// byte-identical across samples (provider prompt cache).
    pub fn workspace_agents_dir(&self) -> String {
        self.write_hint_snap().agents_dir
    }

    fn write_hint_snap(&self) -> WriteHintSnap {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mode = self.current_id();
        {
            let snap = self.write_hint.lock().unwrap();
            if let Some(s) = snap.as_ref() {
                if s.cwd == cwd && s.mode == mode {
                    return s.clone();
                }
            }
        }
        let root = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
        let presets_dir = root.join(".dock").join("presets").display().to_string();
        let agents_dir = root
            .join(".dock")
            .join("presets")
            .join(&mode)
            .join(AGENTS_DIR)
            .display()
            .to_string();
        let snap = WriteHintSnap {
            cwd,
            mode,
            presets_dir,
            agents_dir,
        };
        *self.write_hint.lock().unwrap() = Some(snap.clone());
        snap
    }

    /// Cwd-stable: how to add a new Agent mode. Placed before the current-mode
    /// agents path so a mode switch only changes the suffix (prompt cache).
    fn new_mode_write_hint(&self) -> String {
        let dir = self.workspace_presets_dir();
        format!(
            "新建 Agent 模式写到 {dir}/<id>/agent.yml，子代理写同目录 agents/<type>.yml。模式 id 必须是 [a-z0-9][a-z0-9-]*（如 review），不要用汉字做模式目录名（汉字目录只会叠到已有同名内置模式）。目录不存在就创建。写完后用 /preset 应用该 id。不要写 ~/.dock/presets/，除非用户明确要求保存到全局。"
        )
    }

    fn new_role_write_hint(&self) -> String {
        let dir = self.workspace_agents_dir();
        let id = self.current_id();
        format!(
            "新建人设必须写到当前工作区这个绝对路径（当前模式 {id}），不要猜测目录：{dir}/<type>.yml。目录不存在就创建。不要写 ~/.dock/presets/，除非用户明确要求保存到全局。"
        )
    }

    /// Re-read `agents/` and list callable `subagent_type` ids.
    /// Used by `subagent` when `reload_roster` is true.
    pub fn reload_roster_report(&self) -> String {
        self.resync();
        let preset = self.current();
        let mut lines = vec![format!(
            "Reloaded agents/ for mode {} ({}). Next model step's subagent / task subagent_type enum:",
            preset.id, preset.name
        )];
        if preset.agents.is_empty() {
            lines.push(format!(
                "(empty roster — add agents/<id>.yml under {}/, then call again. New Agent modes go in {}/<id>/agent.yml then /preset apply that id.)",
                self.workspace_agents_dir(),
                self.workspace_presets_dir()
            ));
        } else {
            for (id, def) in &preset.agents {
                let name = def.name.trim();
                let desc = def.description.trim();
                let line = if name.is_empty() || name == id {
                    if desc.is_empty() {
                        format!("- {id}")
                    } else {
                        format!("- {id}: {desc}")
                    }
                } else if desc.is_empty() {
                    format!("- {id} ({name})")
                } else {
                    format!("- {id} ({name}): {desc}")
                };
                lines.push(line);
            }
        }
        lines.push(
            "Call subagent with one of these ids. Do not use a type that is not listed.".into(),
        );
        lines.push(format!(
            "New roles go in {}/<type>.yml. New Agent modes go in {}/<id>/agent.yml (id [a-z0-9][a-z0-9-]*), then /preset apply. Do not write ~/.dock/presets/ unless the user asks to save globally.",
            self.workspace_agents_dir(),
            self.workspace_presets_dir()
        ));
        lines.join("\n")
    }

    /// Close `subagent` / `task` `subagent_type` to the live roster so the
    /// sampler cannot fall back to a trained three-type enum.
    pub fn bind_spawn_schema(&self, spec: &mut ToolSpec) {
        if spec.name != "subagent" && spec.name != "task" {
            return;
        }
        if spec.name == "subagent" {
            spec.description.push_str(&self.subagent_role_hint());
        }
        let roster = self.current_roster();
        if roster.is_empty() {
            return;
        }
        let ids: Vec<String> = roster.keys().cloned().collect();
        if let Some(json) = inject_subagent_type_enum(&spec.parameters_json, &ids) {
            spec.parameters_json = json;
        }
    }

    pub fn replaces_prompt(&self) -> bool {
        let preset = self.current();
        preset.broken.is_none() && preset.replace_prompt
    }

    fn mutate(
        &self,
        id: &str,
        f: impl FnOnce(&mut AgentPreset) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        {
            let Some(preset) = inner.presets.get_mut(id) else {
                return Err(format!("没有预设 {id}"));
            };
            if preset.broken.is_some() {
                return Err("损坏的预设不能编辑".into());
            }
            f(preset)?;
            if preset.origin == PresetOrigin::Shipped {
                preset.origin = PresetOrigin::User;
            }
        }
        persist_roster(&inner)?;
        persist_preset(&inner, id)
    }
}

pub fn blocked_tool_message() -> &'static str {
    BLOCKED
}

pub fn is_shipped(id: &str) -> bool {
    SHIPPED.iter().any(|m| m.id == id)
}

pub fn agent_presets() -> Plugin {
    plugin("agent-presets", Inject::new(), |ctx, _: &()| {
        let user = crate::config::dock_home().join("presets");
        let project = std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join(".dock").join("presets"));
        let provided = ctx.provide(AGENT_PRESETS, AgentPresets::load_layers(user, project))?;
        Ok(Some(provided))
    })
}

fn parse_yaml_preset(raw: &str, id: &str, origin: PresetOrigin) -> AgentPreset {
    match serde_yaml::from_str::<serde_yaml::Value>(raw) {
        Ok(serde_yaml::Value::Mapping(_)) => {}
        Ok(_) => {
            let mut preset = AgentPreset::new(id);
            preset.origin = origin;
            preset.broken = Some("YAML 必须是预设字段的映射".into());
            return preset;
        }
        Err(e) => {
            let mut preset = AgentPreset::new(id);
            preset.origin = origin;
            preset.broken = Some(format!("不是合法 YAML: {e}"));
            return preset;
        }
    }
    match serde_yaml::from_str::<AgentPreset>(raw) {
        Ok(mut preset) => {
            preset.id = id.to_string();
            preset.origin = origin;
            preset.broken = None;
            if preset.name.trim().is_empty() {
                preset.name = id.to_string();
            }
            preset
        }
        Err(e) => {
            let mut preset = AgentPreset::new(id);
            preset.origin = origin;
            preset.broken = Some(format!("无法解析预设: {e}"));
            preset
        }
    }
}

fn parse_shipped(id: &str) -> Option<AgentPreset> {
    SHIPPED.iter().find(|m| m.id == id).map(parse_shipped_mode)
}

fn parse_shipped_mode(mode: &ShippedMode) -> AgentPreset {
    let mut preset = parse_yaml_preset(mode.agent, mode.id, PresetOrigin::Shipped);
    for (tid, raw) in mode.agents {
        let def = parse_subagent(raw, tid);
        preset.agents.insert((*tid).to_string(), def);
    }
    preset
}

fn parse_subagent(raw: &str, id: &str) -> SubagentDef {
    match serde_yaml::from_str::<SubagentDef>(raw) {
        Ok(mut def) => {
            if def.name.trim().is_empty() {
                def.name = id.to_string();
            }
            def
        }
        Err(_) => SubagentDef {
            name: id.to_string(),
            ..SubagentDef::default()
        },
    }
}

fn load_inner(user_dir: PathBuf, project_dir: Option<PathBuf>) -> Inner {
    let mut presets: IndexMap<String, AgentPreset> = IndexMap::new();
    for mode in SHIPPED {
        presets.insert(mode.id.to_string(), parse_shipped_mode(mode));
    }
    let mut current = DEFAULT_PRESET_ID.to_string();
    let mut migrated = false;
    if user_dir.is_dir() {
        if let Some(c) = read_roster(&user_dir) {
            current = c;
        }
        migrated |= migrate_legacy_toml(&user_dir, &mut presets);
        if user_dir.join("roster.toml").is_file() {
            migrated = true;
        }
        load_dir(&user_dir, PresetOrigin::User, &mut presets);
    }
    if let Some(project) = project_dir.as_ref() {
        if project.is_dir() {
            if let Some(c) = read_roster(project) {
                current = c;
            }
            load_dir(project, PresetOrigin::Project, &mut presets);
        }
    }
    sort_presets(&mut presets);
    if current == "default" || !presets.contains_key(&current) {
        current = DEFAULT_PRESET_ID.into();
    }
    let inner = Inner {
        user_dir,
        project_dir,
        current,
        presets,
        persist: true,
    };
    if migrated {
        let _ = persist_roster(&inner);
        for id in inner
            .presets
            .iter()
            .filter(|(_, p)| p.origin == PresetOrigin::User && p.broken.is_none())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
        {
            let _ = persist_preset(&inner, &id);
        }
    }
    inner
}

fn load_dir(dir: &Path, origin: PresetOrigin, presets: &mut IndexMap<String, AgentPreset>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut alias_dirs: Vec<PathBuf> = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() {
            match path.file_name().and_then(|s| s.to_str()) {
                Some(name) if valid_id(name) => dirs.push(path),
                Some(name) if is_display_name_dir(name) => alias_dirs.push(path),
                _ => {}
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) == Some("yml")
            && path.file_stem().and_then(|s| s.to_str()) != Some("roster")
        {
            files.push(path);
        }
    }
    files.sort();
    dirs.sort();
    for path in files {
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        if dir.join(id).is_dir() {
            continue;
        }
        let Some(mut preset) = read_path_preset(&path, id, origin) else {
            continue;
        };
        if preset.agents.is_empty() {
            if let Some(base) = presets.get(id) {
                preset.agents = base.agents.clone();
            }
        }
        presets.insert(id.to_string(), preset);
    }
    for path in dirs {
        let Some(id) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(overlay) = read_mode_dir(&path, id, origin) else {
            continue;
        };
        absorb_mode_overlay(presets, id, overlay, origin);
    }
    alias_dirs.sort();
    for path in alias_dirs {
        let Some(label) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(target) = preset_id_for_display_name(presets, label) else {
            continue;
        };
        let Some(overlay) = read_mode_dir(&path, &target, origin) else {
            continue;
        };
        absorb_mode_overlay(presets, &target, overlay, origin);
    }
}

fn is_display_name_dir(name: &str) -> bool {
    !valid_id(name) && valid_agent_type_id(name)
}

fn preset_id_for_display_name(
    presets: &IndexMap<String, AgentPreset>,
    label: &str,
) -> Option<String> {
    let mut fallback = None;
    for (id, preset) in presets {
        if preset.name != label {
            continue;
        }
        if is_shipped(id) {
            return Some(id.clone());
        }
        fallback = Some(id.clone());
    }
    fallback
}

fn absorb_mode_overlay(
    presets: &mut IndexMap<String, AgentPreset>,
    id: &str,
    mut overlay: AgentPreset,
    origin: PresetOrigin,
) {
    if let Some(base) = presets.get(id).cloned() {
        if overlay.persona.is_empty() && overlay.tools.is_none() && overlay.description.is_empty() {
            overlay.name = base.name.clone();
            overlay.description = base.description.clone();
            overlay.persona = base.persona.clone();
            overlay.tools = base.tools.clone();
            overlay.replace_prompt = base.replace_prompt;
            overlay.order = base.order;
        }
        let mut merged = base.agents.clone();
        for (k, v) in overlay.agents {
            let key = canonical_agent_type_id(id, &k);
            if id == WARDEN_PRESET_ID && warden_child_persona_is_pinyin_stale(&v.persona) {
                continue;
            }
            merged.insert(key, v);
        }
        overlay.agents = merged;
        if id == WARDEN_PRESET_ID {
            sanitize_stale_warden_overlay(&mut overlay, &base);
        }
    }
    overlay.origin = origin;
    overlay.id = id.to_string();
    presets.insert(id.to_string(), overlay);
}

fn read_mode_dir(dir: &Path, id: &str, origin: PresetOrigin) -> Option<AgentPreset> {
    let agent_path = dir.join(AGENT_FILE);
    let agents_dir = dir.join(AGENTS_DIR);
    if !agent_path.is_file() && !agents_dir.is_dir() {
        return None;
    }
    let mut preset = if agent_path.is_file() {
        read_path_preset(&agent_path, id, origin)?
    } else {
        let mut p = AgentPreset::new(id);
        p.origin = origin;
        p
    };
    if agents_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("yml"))
                .collect();
            files.sort();
            let mut pending: Vec<(String, String, SubagentDef)> = Vec::new();
            let mut native: HashSet<String> = HashSet::new();
            for path in files {
                let Some(tid) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if !valid_agent_type_id(tid) {
                    continue;
                }
                let canon = canonical_agent_type_id(id, tid);
                if !valid_agent_type_id(&canon) {
                    continue;
                }
                let Ok(raw) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let def = parse_subagent(&raw, &canon);
                if tid == canon {
                    native.insert(tid.to_string());
                }
                pending.push((tid.to_string(), canon, def));
            }
            for (tid, canon, def) in pending {
                if tid != canon && native.contains(&canon) {
                    continue;
                }
                preset.agents.insert(canon, def);
            }
        }
    }
    Some(preset)
}

fn read_dir_preset(dir: &Path, id: &str, origin: PresetOrigin) -> Option<AgentPreset> {
    let mode = dir.join(id);
    if mode.is_dir() {
        return read_mode_dir(&mode, id, origin);
    }
    let path = dir.join(format!("{id}.yml"));
    read_path_preset(&path, id, origin)
}

fn read_path_preset(path: &Path, id: &str, origin: PresetOrigin) -> Option<AgentPreset> {
    let raw = std::fs::read_to_string(path).ok()?;
    Some(parse_yaml_preset(&raw, id, origin))
}

fn migrate_legacy_toml(dir: &Path, presets: &mut IndexMap<String, AgentPreset>) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut found = false;
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        if path.file_stem().and_then(|s| s.to_str()) == Some("roster") {
            continue;
        }
        let Some(mut preset) = read_toml_preset(&path) else {
            continue;
        };
        found = true;
        preset.origin = PresetOrigin::User;
        presets.insert(preset.id.clone(), preset);
    }
    found
}

fn read_toml_preset(path: &Path) -> Option<AgentPreset> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut preset: AgentPreset = toml::from_str(&raw).ok()?;
    if preset.id.trim().is_empty() {
        preset.id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(DEFAULT_PRESET_ID)
            .to_string();
    }
    if !valid_id(&preset.id) {
        return None;
    }
    if preset.name.trim().is_empty() {
        preset.name = preset.id.clone();
    }
    preset.broken = None;
    Some(preset)
}

fn read_roster(dir: &Path) -> Option<String> {
    let yml = dir.join(ROSTER_FILE);
    if let Ok(raw) = std::fs::read_to_string(&yml) {
        if let Ok(roster) = serde_yaml::from_str::<RosterFile>(&raw) {
            if !roster.current.trim().is_empty() {
                return Some(roster.current);
            }
        }
    }
    let toml_path = dir.join("roster.toml");
    if let Ok(raw) = std::fs::read_to_string(toml_path) {
        #[derive(Deserialize)]
        struct Legacy {
            current: String,
        }
        if let Ok(roster) = toml::from_str::<Legacy>(&raw) {
            if !roster.current.trim().is_empty() {
                return Some(roster.current);
            }
        }
    }
    None
}

fn persist_roster(inner: &Inner) -> Result<(), String> {
    if !inner.persist {
        return Ok(());
    }
    std::fs::create_dir_all(&inner.user_dir).map_err(|e| e.to_string())?;
    let roster = serde_yaml::to_string(&RosterFile {
        current: inner.current.clone(),
    })
    .map_err(|e| e.to_string())?;
    std::fs::write(inner.user_dir.join(ROSTER_FILE), roster).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(inner.user_dir.join("roster.toml"));
    Ok(())
}

fn persist_preset(inner: &Inner, id: &str) -> Result<(), String> {
    if !inner.persist {
        return Ok(());
    }
    let Some(preset) = inner.presets.get(id) else {
        return Ok(());
    };
    if preset.broken.is_some() || preset.origin == PresetOrigin::Shipped {
        return Ok(());
    }
    let dest = file_path(inner, preset);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(parent.join(AGENTS_DIR)).map_err(|e| e.to_string())?;
    }
    let yaml = render_preset(preset)?;
    std::fs::write(&dest, yaml).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(dest.with_extension("toml"));
    let _ = std::fs::remove_file(legacy_yml(inner, preset));
    let _ = std::fs::remove_file(legacy_yml(inner, preset).with_extension("toml"));
    if let Some(parent) = dest.parent() {
        let agents_dir = parent.join(AGENTS_DIR);
        for (tid, def) in &preset.agents {
            let body = serde_yaml::to_string(def).map_err(|e| e.to_string())?;
            std::fs::write(agents_dir.join(format!("{tid}.yml")), body)
                .map_err(|e| e.to_string())?;
        }
        if id == WARDEN_PRESET_ID {
            for (from, _) in WARDEN_PINYIN_ALIASES {
                let _ = std::fs::remove_file(agents_dir.join(format!("{from}.yml")));
            }
        }
    }
    Ok(())
}

fn render_preset(preset: &AgentPreset) -> Result<String, String> {
    let body = serde_yaml::to_string(preset).map_err(|e| e.to_string())?;
    Ok(format!("{FILE_HEADER}{body}"))
}

fn file_path(inner: &Inner, preset: &AgentPreset) -> PathBuf {
    base_dir(inner, preset).join(&preset.id).join(AGENT_FILE)
}

fn legacy_yml(inner: &Inner, preset: &AgentPreset) -> PathBuf {
    base_dir(inner, preset).join(format!("{}.yml", preset.id))
}

fn base_dir<'a>(inner: &'a Inner, preset: &AgentPreset) -> &'a Path {
    match preset.origin {
        PresetOrigin::Project => inner
            .project_dir
            .as_ref()
            .unwrap_or(&inner.user_dir)
            .as_path(),
        _ => inner.user_dir.as_path(),
    }
}

/// New modes (`/preset` n / d) land in the workspace when a project layer exists.
/// Editing a shipped preset via the canvas still flips origin to User overlay.
fn persist_origin(inner: &Inner) -> PresetOrigin {
    if inner.project_dir.is_some() {
        PresetOrigin::Project
    } else {
        PresetOrigin::User
    }
}

fn sort_presets(presets: &mut IndexMap<String, AgentPreset>) {
    let mut items: Vec<_> = presets.drain(..).collect();
    items.sort_by(|a, b| {
        match (a.1.order, b.1.order) {
            (Some(x), Some(y)) if x != y => return x.cmp(&y),
            (Some(_), None) => return std::cmp::Ordering::Less,
            (None, Some(_)) => return std::cmp::Ordering::Greater,
            _ => {}
        }
        match (shipped_index(&a.0), shipped_index(&b.0)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.0.cmp(&b.0),
        }
    });
    for (k, v) in items {
        presets.insert(k, v);
    }
}

fn shipped_index(id: &str) -> Option<usize> {
    SHIPPED.iter().position(|m| m.id == id)
}

fn tool_allowed(allow: &[String], name: &str) -> bool {
    if allow.iter().any(|n| n == name) {
        return true;
    }
    name == "run_terminal_cmd" && allow.iter().any(|n| n == "bash")
}

fn mint_id(presets: &IndexMap<String, AgentPreset>) -> String {
    if !presets.contains_key("custom") {
        return "custom".into();
    }
    for n in 2..10_000 {
        let id = format!("custom-{n}");
        if !presets.contains_key(&id) {
            return id;
        }
    }
    format!("custom-{}", uuid::Uuid::now_v7().as_simple())
}

fn inject_subagent_type_enum(parameters_json: &str, ids: &[String]) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let mut v: Value = serde_json::from_str(parameters_json).ok()?;
    let field = v
        .get_mut("properties")?
        .get_mut("subagent_type")?
        .as_object_mut()?;
    field.insert(
        "enum".into(),
        Value::Array(ids.iter().cloned().map(Value::String).collect()),
    );
    Some(v.to_string())
}

fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    if !(1..=32).contains(&b.len()) {
        return false;
    }
    let first = b[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    b.iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}'
    )
}

/// Roster role id: ascii slug *or* 1–16 CJK characters (`甲`, `突击`).
fn valid_agent_type_id(id: &str) -> bool {
    if valid_id(id) {
        return true;
    }
    let n = id.chars().count();
    (1..=16).contains(&n) && id.chars().all(is_cjk)
}

fn canonical_agent_type_id(preset_id: &str, type_id: &str) -> String {
    if preset_id == WARDEN_PRESET_ID {
        if let Some((_, han)) = WARDEN_PINYIN_ALIASES
            .iter()
            .find(|(from, _)| *from == type_id)
        {
            return (*han).to_string();
        }
    }
    type_id.to_string()
}

fn warden_child_persona_is_pinyin_stale(persona: &str) -> bool {
    persona.contains("调度标签")
        || WARDEN_PINYIN_ALIASES.iter().any(|(from, _)| {
            persona.contains(&format!("inbox/{from}-"))
                || persona.contains(&format!("只守住 {from}"))
        })
}

fn warden_parent_persona_is_pinyin_stale(persona: &str) -> bool {
    persona.contains("jia/yi/bing")
        || persona.contains("名册 id：cen")
        || persona.contains("先 task 一个 guan")
        || persona.contains("guan 观察")
        || persona.contains("并列多个 task")
}

fn sanitize_stale_warden_overlay(overlay: &mut AgentPreset, base: &AgentPreset) {
    if warden_parent_persona_is_pinyin_stale(&overlay.persona) {
        overlay.persona = base.persona.clone();
    }
    let Some(tools) = overlay.tools.as_mut() else {
        return;
    };
    tools
        .retain(|t| t != "task" && t != "get_task_output" && t != "wait_tasks" && t != "kill_task");
    if !tools.iter().any(|t| t == "subagent") {
        tools.push("subagent".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::TOOLS;
    use crate::tools::Tools;
    use crate::types::ToolCall;
    use cordis::Context;

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: name.into(),
            parameters_json: "{}".into(),
        }
    }

    #[test]
    fn shipped_yaml_parses() {
        for mode in SHIPPED {
            let p = parse_shipped_mode(mode);
            assert!(p.broken.is_none(), "{}: {:?}", mode.id, p.broken);
            assert!(!p.name.is_empty());
            assert!(p.tools.as_ref().is_some_and(|t| !t.is_empty()));
            if mode.id == MINIMAL_PRESET_ID {
                assert!(p.agents.is_empty(), "{}", mode.id);
            } else if mode.id == WARDEN_PRESET_ID {
                assert!(
                    !p.tools.as_ref().unwrap().iter().any(|n| n == "report"),
                    "{}",
                    mode.id
                );
                assert!(
                    !p.tools
                        .as_ref()
                        .unwrap()
                        .iter()
                        .any(|n| n == "get_task_output"),
                    "{}",
                    mode.id
                );
                assert!(
                    p.tools.as_ref().unwrap().iter().any(|n| n == "bash"),
                    "{}",
                    mode.id
                );
                for id in ["岑", "锁", "甲", "乙", "丙", "衡", "验", "观", "突击"] {
                    let def = p.agents.get(id).unwrap_or_else(|| panic!("{id}"));
                    let tools = def.tools.as_ref().unwrap();
                    assert!(tools.iter().any(|n| n == "report"), "{id}");
                    assert!(tools.iter().any(|n| n == "bash"), "{id}");
                    assert!(!tools.iter().any(|n| n == "task"), "{id}");
                    assert!(!tools.iter().any(|n| n == "subagent"), "{id}");
                }
            } else {
                assert!(p.agents.contains_key("explore"), "{}", mode.id);
                assert!(p.agents.contains_key("plan"), "{}", mode.id);
                assert!(p.agents.contains_key("general-purpose"), "{}", mode.id);
                let explore = p.agents.get("explore").unwrap();
                assert!(!explore
                    .tools
                    .as_ref()
                    .unwrap()
                    .iter()
                    .any(|n| n == "write_file"));
                assert!(!explore.tools.as_ref().unwrap().iter().any(|n| n == "bash"));
                assert!(explore
                    .tools
                    .as_ref()
                    .unwrap()
                    .iter()
                    .any(|n| n == "report"));
            }
        }
    }

    #[test]
    fn shipped_modes_filter_dock_tools() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let ids: Vec<_> = presets.list().into_iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            vec![
                DEFAULT_PRESET_ID,
                MINIMAL_PRESET_ID,
                CORDIS_PRESET_ID,
                WARDEN_PRESET_ID,
            ]
        );
        assert_eq!(presets.current_id(), DEFAULT_PRESET_ID);
        assert!(presets.allows("bash"));
        assert!(presets.allows("run_terminal_cmd"));
        assert!(presets.allows("lsp"));
        assert!(presets.allows("task"));
        assert!(presets.allows("subagent"));
        assert!(presets.allows("send_message"));
        assert!(!presets.allows("report"));
        assert!(!presets.allows("cordis_define"));
        assert!(!presets.allows("scheduler_create"));

        presets.apply(MINIMAL_PRESET_ID).unwrap();
        assert!(presets.allows("read_file"));
        assert!(!presets.allows("web_search"));
        assert!(!presets.allows("cordis_run"));

        presets.apply(CORDIS_PRESET_ID).unwrap();
        assert!(presets.allows("cordis_inspect"));
        assert!(presets.allows("cordis_run"));
        assert!(presets.allows("cordis_promote"));
        assert!(!presets.allows("scheduler_create"));

        presets.apply(WARDEN_PRESET_ID).unwrap();
        assert!(presets.allows("bash"));
        assert!(presets.allows("run_terminal_cmd"));
        assert!(presets.allows("subagent"));
        assert!(!presets.allows("task"));
        assert!(!presets.allows("get_task_output"));
        assert!(!presets.allows("wait_tasks"));
        assert!(!presets.allows("kill_task"));
        assert!(!presets.allows("report"));
        assert_eq!(presets.role_label("岑").as_deref(), Some("岑"));
        assert_eq!(presets.role_label("观").as_deref(), Some("观"));
        assert!(presets.subagent("jia").is_none());
        assert!(!presets.allows("write_file"));
        assert!(!presets.allows("cordis_run"));
    }

    #[test]
    fn yaml_drop_in_is_enough() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("review.yml"),
            "name: 评审\npersona: 只读代码。\ntools:\n  - read_file\n  - grep\n",
        )
        .unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let got = presets.get("review").unwrap();
        assert_eq!(got.name, "评审");
        assert_eq!(got.tools, Some(vec!["read_file".into(), "grep".into()]));
        presets.apply("review").unwrap();
        assert!(presets.allows("read_file"));
        assert!(!presets.allows("bash"));
    }

    #[test]
    fn project_yaml_overrides_user() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("review.yml"),
            "name: 家\ntools:\n  - bash\n",
        )
        .unwrap();
        std::fs::write(
            proj.path().join("review.yml"),
            "name: 仓\ntools:\n  - read_file\n",
        )
        .unwrap();
        let presets =
            AgentPresets::load_layers(home.path().to_path_buf(), Some(proj.path().to_path_buf()));
        let got = presets.get("review").unwrap();
        assert_eq!(got.name, "仓");
        assert_eq!(got.origin, PresetOrigin::Project);
        assert_eq!(got.tools, Some(vec!["read_file".into()]));
    }

    #[test]
    fn editing_shipped_writes_user_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        presets
            .set_persona(DEFAULT_PRESET_ID, "覆盖人设。".into())
            .unwrap();
        assert!(dir.path().join("code").join(AGENT_FILE).is_file());
        let got = presets.get(DEFAULT_PRESET_ID).unwrap();
        assert_eq!(got.origin, PresetOrigin::User);
        assert!(got.persona.contains("覆盖人设"));
        assert!(got.agents.contains_key("explore"));
        presets.delete(DEFAULT_PRESET_ID).unwrap();
        let restored = presets.get(DEFAULT_PRESET_ID).unwrap();
        assert_eq!(restored.origin, PresetOrigin::Shipped);
        assert!(restored.persona.contains("编码助手"));
        assert!(!dir.path().join("code").join(AGENT_FILE).is_file());
    }

    #[test]
    fn cannot_delete_unmodified_shipped() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        assert!(presets.delete(DEFAULT_PRESET_ID).is_err());
    }

    #[test]
    fn remove_snapshots_allowlist_on_user_copy() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let copy = presets.duplicate(MINIMAL_PRESET_ID).unwrap();
        let live = vec!["list_dir".into(), "read_file".into(), "bash".into()];
        presets.remove_tool(&copy.id, "bash", &live).unwrap();
        presets.apply(&copy.id).unwrap();
        assert!(!presets.allows("bash"));
        assert!(presets.allows("read_file"));
        presets.add_tool(&copy.id, "bash").unwrap();
        assert!(presets.allows("bash"));
        assert!(dir.path().join("custom").join(AGENT_FILE).is_file());
    }

    #[test]
    fn persist_roundtrip_writes_user_yaml_not_shipped_blob() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        {
            let presets = AgentPresets::load(path.clone());
            let copy = presets.duplicate(DEFAULT_PRESET_ID).unwrap();
            presets
                .set_persona(&copy.id, "你是安全研究员。".into())
                .unwrap();
            presets.remove_tool(&copy.id, "bash", &[]).unwrap();
            assert_eq!(presets.current_id(), copy.id);
        }
        let yaml = std::fs::read_to_string(path.join("custom").join(AGENT_FILE)).unwrap();
        assert!(yaml.contains("你是安全研究员"), "{yaml}");
        assert!(!yaml.contains("agent.cordis"), "{yaml}");
        assert!(path.join(ROSTER_FILE).is_file());
        let loaded = AgentPresets::load(path);
        assert_eq!(loaded.current_id(), "custom");
        let current = loaded.current();
        assert!(current.persona.contains("安全研究员"));
        assert!(!current.tools.as_ref().unwrap().iter().any(|n| n == "bash"));
        loaded.delete(&current.id).unwrap();
        assert_eq!(loaded.current_id(), DEFAULT_PRESET_ID);
    }

    #[test]
    fn empty_tools_list_is_not_all_tools() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("silent.yml"), "name: 静默\ntools: []\n").unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        presets.apply("silent").unwrap();
        assert!(!presets.allows("bash"));
    }

    #[test]
    fn omit_tools_means_all_live() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("wide.yml"), "name: 全开\npersona: hi\n").unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let got = presets.get("wide").unwrap();
        assert!(got.tools.is_none());
        presets.apply("wide").unwrap();
        assert!(presets.allows("scheduler_create"));
        assert!(presets.allows("cordis_define"));
    }

    #[test]
    fn minimal_replaces_system_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        presets.apply(MINIMAL_PRESET_ID).unwrap();
        assert!(presets.replaces_prompt());
        let mut assembled = "You are a test agent.".into();
        presets.merge_persona(&mut assembled);
        assert!(!assembled.contains("test agent"), "{assembled}");
        assert!(assembled.contains("list_dir"), "{assembled}");
    }

    #[test]
    fn agent_type_id_allows_han() {
        assert!(valid_agent_type_id("explore"));
        assert!(valid_agent_type_id("general-purpose"));
        assert!(valid_agent_type_id("甲"));
        assert!(valid_agent_type_id("突击"));
        assert!(!valid_id("甲"));
        assert!(!valid_agent_type_id(""));
        assert!(!valid_agent_type_id("jia甲"));
    }

    #[test]
    fn user_han_agent_yml_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let mode = dir.path().join("review");
        std::fs::create_dir_all(mode.join("agents")).unwrap();
        std::fs::write(mode.join("agent.yml"), "name: 评\ntools:\n  - read_file\n").unwrap();
        std::fs::write(
            mode.join("agents").join("甲.yml"),
            "name: 甲\ndescription: 猎洞\ntools:\n  - report\n",
        )
        .unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let got = presets.get("review").unwrap();
        assert!(got.agents.contains_key("甲"), "{got:?}");
        assert_eq!(got.agents.get("甲").unwrap().name, "甲");
    }

    #[test]
    fn warden_pinyin_overlay_is_aliased_to_han() {
        let dir = tempfile::tempdir().unwrap();
        let mode = dir.path().join("warden");
        std::fs::create_dir_all(mode.join("agents")).unwrap();
        std::fs::write(mode.join("agent.yml"), "name: 守望\ntools:\n  - subagent\n").unwrap();
        std::fs::write(
            mode.join("agents").join("jia.yml"),
            "name: 甲旧\ndescription: 覆盖\ntools:\n  - report\n",
        )
        .unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        presets.apply(WARDEN_PRESET_ID).unwrap();
        assert!(presets.subagent("jia").is_none());
        assert_eq!(presets.subagent("甲").unwrap().name, "甲旧");
        assert!(presets.subagent("观").is_some());
        let mut assembled = String::new();
        presets.merge_subagent_roster(&mut assembled);
        assert!(!assembled.contains("jia"), "{assembled}");
        assert!(assembled.contains("甲"), "{assembled}");
    }

    #[test]
    fn warden_han_yml_wins_over_pinyin_alias_file() {
        let dir = tempfile::tempdir().unwrap();
        let mode = dir.path().join("warden");
        std::fs::create_dir_all(mode.join("agents")).unwrap();
        std::fs::write(mode.join("agent.yml"), "name: 守望\ntools:\n  - subagent\n").unwrap();
        std::fs::write(
            mode.join("agents").join("jia.yml"),
            "name: 拼音甲\ndescription: skip\n",
        )
        .unwrap();
        std::fs::write(
            mode.join("agents").join("甲.yml"),
            "name: 甲\ndescription: 汉字\n",
        )
        .unwrap();
        let warden = presets_warden(&dir);
        let def = warden.agents.get("甲").unwrap();
        assert_eq!(def.name, "甲");
        assert_eq!(def.description, "汉字");
        assert!(warden.agents.get("jia").is_none());
    }

    #[test]
    fn stale_pinyin_warden_overlay_uses_shipped_han() {
        let dir = tempfile::tempdir().unwrap();
        let mode = dir.path().join("warden");
        std::fs::create_dir_all(mode.join("agents")).unwrap();
        std::fs::write(
            mode.join("agent.yml"),
            "name: 守望\npersona: |\n  用 task 委派，subagent_type 为名册 id：cen 测绘、jia/yi/bing。\n  先 task 一个 guan，description 固定为「guan 观察」。\n  并列多个 task。\ntools:\n  - task\n  - send_message\n  - write_file\n",
        )
        .unwrap();
        std::fs::write(
            mode.join("agents").join("jia.yml"),
            "name: 甲\npersona: |\n  你是甲（调度标签 jia），只守住 jia。\n  写 inbox/jia-guan-1.md。\ntools:\n  - report\n",
        )
        .unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        presets.apply(WARDEN_PRESET_ID).unwrap();
        let current = presets.current();
        assert!(
            !current.persona.contains("jia/yi/bing"),
            "{}",
            current.persona
        );
        assert!(current.persona.contains("甲/乙/丙"), "{}", current.persona);
        assert!(presets.allows("subagent"));
        assert!(!presets.allows("task"));
        assert!(!presets.allows("get_task_output"));
        assert!(presets.allows("write_file"));
        assert!(presets.subagent("jia").is_none());
        let jia = presets.subagent("甲").unwrap();
        assert!(!jia.persona.contains("调度标签"), "{}", jia.persona);
        assert!(jia.persona.contains("只守住甲"), "{}", jia.persona);
        let mut roster = String::new();
        presets.merge_subagent_roster(&mut roster);
        assert!(!roster.contains("jia"), "{roster}");
        assert!(!roster.contains("get_task_output"), "{roster}");
        assert!(roster.contains("- 甲"), "{roster}");
    }

    fn presets_warden(dir: &tempfile::TempDir) -> AgentPreset {
        AgentPresets::load(dir.path().to_path_buf())
            .get(WARDEN_PRESET_ID)
            .unwrap()
    }

    #[test]
    fn code_persona_appends_once() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let mut assembled = "You are a test agent.".into();
        presets.merge_persona(&mut assembled);
        presets.merge_persona(&mut assembled);
        assert_eq!(assembled.matches(PERSONA_MARK).count(), 1);
        assert!(assembled.contains("编码助手"));
        presets.merge_subagent_roster(&mut assembled);
        presets.merge_subagent_roster(&mut assembled);
        assert_eq!(assembled.matches(ROSTER_MARK).count(), 1);
        assert!(assembled.contains("explore"));
        assert!(assembled.contains("subagent"));
        assert!(assembled.contains("send_message"));
        assert!(assembled.contains("urgent"));
        assert!(assembled.contains("reload_roster"));
        let cwd = std::env::current_dir().unwrap();
        let root = cwd.canonicalize().unwrap_or(cwd);
        let expect = root
            .join(".dock")
            .join("presets")
            .join("code")
            .join("agents");
        assert!(
            assembled.contains(&expect.display().to_string()),
            "{assembled}"
        );
        assert!(assembled.contains("不要猜测"), "{assembled}");
        assert!(assembled.contains("全局"), "{assembled}");
        assert!(assembled.contains("~/.dock/presets"), "{assembled}");
        assert_eq!(presets.role_label("explore").as_deref(), Some("探索"));
        assert!(presets.subagent_role_hint().contains("explore"));
        assert!(presets.subagent_role_hint().contains("general-purpose"));
        assert!(presets.subagent_role_hint().contains("reload_roster"));
        assert!(presets
            .subagent_role_hint()
            .contains(&expect.display().to_string()));
    }

    #[test]
    fn roster_hint_uses_live_workspace_path() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let mut assembled = String::new();
        presets.merge_subagent_roster(&mut assembled);
        let cwd = std::env::current_dir().unwrap();
        let root = cwd.canonicalize().unwrap_or(cwd);
        let expect = root
            .join(".dock")
            .join("presets")
            .join("code")
            .join("agents");
        assert!(
            assembled.contains(&expect.display().to_string()),
            "{assembled}"
        );
        assert!(assembled.contains("当前模式 code"), "{assembled}");
        assert!(assembled.contains("新建 Agent 模式"), "{assembled}");
        assert!(assembled.contains("全局"), "{assembled}");
        let presets_dir = root.join(".dock").join("presets");
        assert!(
            assembled.contains(&presets_dir.display().to_string()),
            "{assembled}"
        );
    }

    #[test]
    fn write_hint_path_stable_until_mode_changes() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let a = presets.workspace_agents_dir();
        let b = presets.workspace_agents_dir();
        assert_eq!(a, b);
        assert!(
            a.ends_with("/.dock/presets/code/agents")
                || a.ends_with("\\.dock\\presets\\code\\agents"),
            "{a}"
        );
        let presets_dir = presets.workspace_presets_dir();
        presets.apply(WARDEN_PRESET_ID).unwrap();
        let c = presets.workspace_agents_dir();
        assert_ne!(a, c);
        assert!(
            c.contains("/.dock/presets/warden/agents")
                || c.contains("\\.dock\\presets\\warden\\agents"),
            "{c}"
        );
        assert_eq!(presets_dir, presets.workspace_presets_dir());
    }

    #[test]
    fn empty_mode_still_injects_write_paths() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        presets.apply(MINIMAL_PRESET_ID).unwrap();
        let mut assembled = String::new();
        presets.merge_subagent_roster(&mut assembled);
        assert!(assembled.contains(ROSTER_MARK), "{assembled}");
        assert!(assembled.contains("当前模式尚无子代理"), "{assembled}");
        assert!(assembled.contains("新建 Agent 模式"), "{assembled}");
        assert!(assembled.contains("[a-z0-9]"), "{assembled}");
        let cwd = std::env::current_dir().unwrap();
        let root = cwd.canonicalize().unwrap_or(cwd);
        let presets_dir = root.join(".dock").join("presets");
        let agents_dir = presets_dir.join("minimal").join("agents");
        assert!(
            assembled.contains(&presets_dir.display().to_string()),
            "{assembled}"
        );
        assert!(
            assembled.contains(&agents_dir.display().to_string()),
            "{assembled}"
        );
        assert!(assembled.contains("当前模式 minimal"), "{assembled}");
        assert!(presets.subagent_role_hint().contains("new Agent mode"));
        let report = presets.reload_roster_report();
        assert!(report.contains("empty roster"), "{report}");
        assert!(
            report.contains(&presets_dir.display().to_string()),
            "{report}"
        );
    }

    #[test]
    fn overlay_skips_parent_roster_write_hints() {
        let def = SubagentDef {
            name: "探".into(),
            ..SubagentDef::default()
        };
        let overlay = AgentPresets::overlay(def.to_preset("explore"));
        let mut assembled = String::new();
        overlay.merge_subagent_roster(&mut assembled);
        assert!(assembled.is_empty(), "{assembled}");
    }

    #[test]
    fn create_and_duplicate_write_project_dir_when_layered() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let presets =
            AgentPresets::load_layers(home.path().to_path_buf(), Some(proj.path().to_path_buf()));
        let created = presets.create().unwrap();
        assert_eq!(created.origin, PresetOrigin::Project);
        assert!(proj.path().join(&created.id).join(AGENT_FILE).is_file());
        assert!(!home.path().join(&created.id).join(AGENT_FILE).is_file());
        let copy = presets.duplicate(DEFAULT_PRESET_ID).unwrap();
        assert_eq!(copy.origin, PresetOrigin::Project);
        assert!(proj.path().join(&copy.id).join(AGENT_FILE).is_file());
        assert!(!home.path().join(&copy.id).join(AGENT_FILE).is_file());
    }

    #[test]
    fn editing_shipped_stays_user_overlay_when_layered() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let presets =
            AgentPresets::load_layers(home.path().to_path_buf(), Some(proj.path().to_path_buf()));
        presets
            .set_persona(DEFAULT_PRESET_ID, "覆盖人设。".into())
            .unwrap();
        assert!(home.path().join("code").join(AGENT_FILE).is_file());
        assert!(!proj.path().join("code").join(AGENT_FILE).is_file());
        assert_eq!(
            presets.get(DEFAULT_PRESET_ID).unwrap().origin,
            PresetOrigin::User
        );
    }

    #[test]
    fn new_subagent_yml_is_live_this_session() {
        let dir = tempfile::tempdir().unwrap();
        let mode = dir.path().join("review");
        std::fs::create_dir_all(mode.join("agents")).unwrap();
        std::fs::write(mode.join("agent.yml"), "name: 评\ntools:\n  - subagent\n").unwrap();
        std::fs::write(
            mode.join("agents").join("explore.yml"),
            "name: 探\ntools:\n  - read_file\n",
        )
        .unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        presets.apply("review").unwrap();
        assert!(presets.get("review").unwrap().agents.get("scout").is_none());

        std::fs::write(
            mode.join("agents").join("scout.yml"),
            "name: 侦察\ndescription: 新角色\ntools:\n  - read_file\n",
        )
        .unwrap();

        let def = presets
            .subagent("scout")
            .expect("new yml must be spawnable now");
        assert_eq!(def.name, "侦察");
        assert!(presets.current_roster().contains_key("scout"));
        let mut assembled = String::new();
        presets.merge_subagent_roster(&mut assembled);
        assert!(assembled.contains("scout"), "{assembled}");
        assert!(assembled.contains("侦察"), "{assembled}");
        assert!(presets.subagent_role_hint().contains("scout"));
        let report = presets.reload_roster_report();
        assert!(report.contains("scout"), "{report}");
        assert!(report.contains("侦察"), "{report}");
    }

    #[test]
    fn overlay_resync_does_not_replace_child_snapshot() {
        let def = SubagentDef {
            name: "探".into(),
            tools: Some(vec!["read_file".into()]),
            ..SubagentDef::default()
        };
        let overlay = AgentPresets::overlay(def.to_preset("explore"));
        overlay.resync();
        assert_eq!(overlay.current_id(), "explore");
        assert_eq!(overlay.current().name, "探");
        assert_eq!(overlay.current().tools, Some(vec!["read_file".into()]));
        assert!(overlay.get("code").is_none());
        assert!(overlay.get("warden").is_none());
    }

    #[test]
    fn display_name_dir_overlays_shipped_roster() {
        let dir = tempfile::tempdir().unwrap();
        let alias = dir.path().join("创造");
        std::fs::create_dir_all(alias.join("agents")).unwrap();
        std::fs::write(
            alias.join("agents").join("review.yml"),
            "name: Review\ndescription: 评审\ntools:\n  - read_file\n  - report\n",
        )
        .unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        assert!(presets.get("创造").is_none());
        presets.apply(CORDIS_PRESET_ID).unwrap();
        let roster = presets.current_roster();
        assert!(roster.contains_key("review"), "{roster:?}");
        assert!(roster.contains_key("explore"));
        assert_eq!(roster.get("review").unwrap().name, "Review");
        assert!(presets.subagent("review").is_some());

        let mut spec = ToolSpec {
            name: "subagent".into(),
            description: "Delegate".into(),
            parameters_json: r#"{"type":"object","properties":{"subagent_type":{"type":"string"}},"required":["subagent_type"]}"#.into(),
        };
        presets.bind_spawn_schema(&mut spec);
        let v: serde_json::Value = serde_json::from_str(&spec.parameters_json).unwrap();
        let ids: Vec<&str> = v["properties"]["subagent_type"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str())
            .collect();
        assert!(ids.contains(&"review"), "{ids:?}");
        assert!(ids.contains(&"explore"), "{ids:?}");
        assert!(ids.contains(&"plan"), "{ids:?}");
        assert!(ids.contains(&"general-purpose"), "{ids:?}");
        assert!(spec.description.contains("review"));
    }

    #[test]
    fn inject_enum_skips_tools_without_subagent_type() {
        let json = inject_subagent_type_enum(
            r#"{"type":"object","properties":{"x":{"type":"string"}}}"#,
            &["review".into()],
        );
        assert!(json.is_none());
    }

    #[test]
    fn broken_yaml_lists_but_refuses_apply() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("leaky.yml"), "- just a list\n").unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let got = presets.get("leaky").unwrap();
        assert!(got.broken.is_some(), "{got:?}");
        assert!(presets.apply("leaky").is_err());
        presets.delete("leaky").unwrap();
        assert!(presets.get("leaky").is_none());
    }

    #[test]
    fn resync_picks_up_hand_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let presets = AgentPresets::load(path.clone());
        std::fs::write(path.join("hand.yml"), "name: 手改\ntools:\n  - bash\n").unwrap();
        assert!(presets.get("hand").is_none());
        presets.resync();
        let got = presets.get("hand").unwrap();
        assert_eq!(got.name, "手改");
        assert_eq!(got.tools, Some(vec!["bash".into()]));
    }

    #[test]
    fn migrates_legacy_toml_into_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("daily.toml"),
            "id = \"daily\"\nname = \"日常\"\npersona = \"守住。\"\ntools = [\"bash\"]\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("roster.toml"), "current = \"daily\"\n").unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let got = presets.get("daily").unwrap();
        assert_eq!(got.name, "日常");
        assert_eq!(got.tools, Some(vec!["bash".into()]));
        assert_eq!(presets.current_id(), "daily");
        assert!(dir.path().join("daily").join(AGENT_FILE).is_file());
        assert!(!dir.path().join("daily.yml").is_file());
        assert!(!dir.path().join("daily.toml").is_file());
    }

    #[test]
    fn filter_specs_keeps_allowlisted_order() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        presets.apply(MINIMAL_PRESET_ID).unwrap();
        let names: Vec<_> = presets
            .filter_specs(vec![
                spec("web_search"),
                spec("bash"),
                spec("read_file"),
                spec("cordis_define"),
            ])
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["bash", "read_file"]);
    }

    #[test]
    fn directory_preset_and_legacy_yml_both_load() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("review").join("agents")).unwrap();
        std::fs::write(
            dir.path().join("review").join(AGENT_FILE),
            "name: 评审\ntools:\n  - read_file\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("review").join("agents").join("explore.yml"),
            "name: 探\ntools:\n  - read_file\n  - grep\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("legacy.yml"),
            "name: 旧\ntools:\n  - bash\n",
        )
        .unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let review = presets.get("review").unwrap();
        assert_eq!(review.name, "评审");
        assert!(review.agents.contains_key("explore"));
        assert_eq!(presets.get("legacy").unwrap().name, "旧");
    }

    #[test]
    fn project_agents_overlay_merges_shipped_roster() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join("code").join("agents")).unwrap();
        std::fs::write(
            proj.path().join("code").join("agents").join("review.yml"),
            "name: 审\ndescription: 只读评审\ntools:\n  - read_file\n",
        )
        .unwrap();
        let presets =
            AgentPresets::load_layers(home.path().to_path_buf(), Some(proj.path().to_path_buf()));
        let code = presets.get(DEFAULT_PRESET_ID).unwrap();
        assert_eq!(code.origin, PresetOrigin::Project);
        assert!(code.agents.contains_key("explore"));
        assert!(code.agents.contains_key("review"));
        assert_eq!(code.agents.get("review").unwrap().name, "审");
    }

    #[test]
    fn directory_wins_over_legacy_yml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("review.yml"),
            "name: 文件\ntools:\n  - bash\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("review")).unwrap();
        std::fs::write(
            dir.path().join("review").join(AGENT_FILE),
            "name: 目录\ntools:\n  - read_file\n",
        )
        .unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        assert_eq!(presets.get("review").unwrap().name, "目录");
    }

    #[tokio::test]
    async fn execute_blocks_tools_off_the_preset() {
        let ctx = Context::new();
        crate::install_without_llm(&ctx).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let user = presets.create().unwrap();
        presets
            .remove_tool(&user.id, "ghost", &["keep".into(), "ghost".into()])
            .unwrap();
        ctx.provide(AGENT_PRESETS, presets).unwrap();
        let tools = ctx.require::<Tools>(TOOLS).unwrap();
        let blocked = tools
            .execute(ToolCall {
                id: "1".into(),
                name: "ghost".into(),
                arguments: "nope".into(),
            })
            .await;
        assert!(
            blocked.content.contains("预设") && blocked.content.contains("/preset"),
            "{}",
            blocked.content
        );
        let allowed = tools
            .execute(ToolCall {
                id: "2".into(),
                name: "keep".into(),
                arguments: "ok".into(),
            })
            .await;
        assert_eq!(allowed.content, "ok");
    }
}
