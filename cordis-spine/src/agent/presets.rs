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

use crate::names::{AGENT_PRESETS, CONTEXT};
use crate::prompt::assemble::ORDER_PERSONA;
use crate::prompt::context_book::{own_sections, ContextBook};
use cordis_base::types::ToolSpec;

pub const DEFAULT_PRESET_ID: &str = "code";
pub const MINIMAL_PRESET_ID: &str = "minimal";

/// 唯一的 spawn 工具（`TOOLS.md`：只有 `task`）。它的 schema 随当前名册改写，
/// 所以在排序里不算「到处都允许」。
const SPAWN_TOOL_NAME: &str = "task";
pub const CORDIS_PRESET_ID: &str = "cordis";
pub const WARDEN_PRESET_ID: &str = "warden";

const PERSONA_MARK: &str = "# Agent 预设：";
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
        agent: include_str!("../../presets/code/agent.yml"),
        agents: &[
            (
                "general-purpose",
                include_str!("../../presets/code/agents/general-purpose.yml"),
            ),
            (
                "explore",
                include_str!("../../presets/code/agents/explore.yml"),
            ),
            ("plan", include_str!("../../presets/code/agents/plan.yml")),
        ],
    },
    ShippedMode {
        id: MINIMAL_PRESET_ID,
        agent: include_str!("../../presets/minimal/agent.yml"),
        agents: &[],
    },
    ShippedMode {
        id: CORDIS_PRESET_ID,
        agent: include_str!("../../presets/cordis/agent.yml"),
        agents: &[
            (
                "general-purpose",
                include_str!("../../presets/cordis/agents/general-purpose.yml"),
            ),
            (
                "explore",
                include_str!("../../presets/cordis/agents/explore.yml"),
            ),
            ("plan", include_str!("../../presets/cordis/agents/plan.yml")),
        ],
    },
    ShippedMode {
        id: WARDEN_PRESET_ID,
        agent: include_str!("../../presets/warden/agent.yml"),
        agents: &[
            ("岑", include_str!("../../presets/warden/agents/岑.yml")),
            ("锁", include_str!("../../presets/warden/agents/锁.yml")),
            ("甲", include_str!("../../presets/warden/agents/甲.yml")),
            ("乙", include_str!("../../presets/warden/agents/乙.yml")),
            ("丙", include_str!("../../presets/warden/agents/丙.yml")),
            ("衡", include_str!("../../presets/warden/agents/衡.yml")),
            ("验", include_str!("../../presets/warden/agents/验.yml")),
            ("观", include_str!("../../presets/warden/agents/观.yml")),
            ("突击", include_str!("../../presets/warden/agents/突击.yml")),
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
    /// Carry the skills / workflows listings into this child's system prompt.
    /// Off by default: a narrow child rarely loads a skill, and every
    /// concurrent child pays for the catalog again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub listings: bool,
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
            listings: self.listings,
            order: None,
            icon: None,
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
    /// Child-only: carry the skills / workflows listings. The main session
    /// always gets them; see [`SubagentDef::listings`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub listings: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    /// 客户端显示用的图标名（lucide 名，如 `rocket`）；TUI 不用。没写就按 id 猜。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
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
            listings: false,
            order: None,
            icon: None,
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

/// Write-path hints for the system prompt. Workspace-relative so a `/cd`
/// or different machine does not rewrite the prefix (provider prompt cache).
#[derive(Clone)]
struct WriteHintSnap {
    mode: String,
    /// `.dock/presets`
    presets_dir: String,
    /// `.dock/presets/{mode}/agents`
    agents_dir: String,
}

/// Named `agentPresets` service. TUI live-looks it; the loop filters specs
/// and execute at the call site.
#[derive(Clone)]
pub struct AgentPresets {
    inner: Arc<Mutex<Inner>>,
    write_hint: Arc<Mutex<Option<WriteHintSnap>>>,
    /// 从父会话继承下来的工具排序依据。子代理 overlay 只带**一个**角色预设，
    /// 自己算出来的「到处都允许」会变成「这个角色允许的全部」——和父会话的分组
    /// 不同，工具表就对不齐，前缀白排。见 [`Self::universal_tools`]。
    order: Option<Arc<UniversalTools>>,
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
            order: None,
        }
    }

    /// In-memory overlay for a child isolate. Never writes disk.
    pub fn overlay(preset: AgentPreset) -> Self {
        Self::overlay_inner(preset, None)
    }

    /// 同 [`Self::overlay`]，另外继承父会话的工具排序依据。
    ///
    /// 真正 spawn 子代理走这条：不继承的话，父子两边的工具表分组不同，子代理那张
    /// 表就不再是主会话那张的真前缀，公共头每次冷启动都要重付。预览类的调用
    /// （`/context` 占用估算）用 [`Self::overlay`] 就行，它们不发请求。
    pub fn overlay_with_order(preset: AgentPreset, order: Arc<UniversalTools>) -> Self {
        Self::overlay_inner(preset, Some(order))
    }

    fn overlay_inner(preset: AgentPreset, order: Option<Arc<UniversalTools>>) -> Self {
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
            order,
        }
    }

    /// 给新分页的一份：同样的层（内置 / 用户 / 项目）、同样的当前预设，之后两页
    /// 各切各的——一页 `/preset` 不再改掉所有页。
    ///
    /// 只读 overlay（旁问页、子代理）不能被复制成常驻页的预设，那会把只读工具集
    /// 带进一个全权页；这时按默认层重新加载一份。
    pub fn fork(&self) -> Self {
        let (user, project, current) = {
            let inner = self.inner.lock().unwrap();
            if !inner.persist {
                let (user, project) = default_layers();
                return Self::load_layers(user, project);
            }
            (
                inner.user_dir.clone(),
                inner.project_dir.clone(),
                inner.current.clone(),
            )
        };
        let forked = Self::load_layers(user, project);
        {
            let mut inner = forked.inner.lock().unwrap();
            if inner.presets.contains_key(&current) {
                inner.current = current;
            }
        }
        forked
    }

    /// `/cd`: point the project overlay at the new workspace and drop the
    /// write-path cache so the next assemble picks up the new mode dir.
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

    /// 只切这一份（按页 fork 出来的）预设，**不写** `roster.yml`：会话的预设在
    /// 创建时定下、跟着会话走，不该顺手改掉下次启动的默认。
    pub fn pin(&self, id: &str) -> Result<AgentPreset, String> {
        let mut inner = self.inner.lock().unwrap();
        let Some(preset) = inner.presets.get(id).cloned() else {
            return Err(format!("没有预设 {id}"));
        };
        if let Some(reason) = &preset.broken {
            return Err(reason.clone());
        }
        inner.current = id.to_string();
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

    /// 新建一个用户层预设（GUI「新建自定义预设」）：起名、可带图标，`based_on` 给了就照
    /// 它复制人设、工具名单和子代理名册。写 `~/.dock/presets/<id>/`，**不切**当前预设
    /// ——预设在开会话时选。
    pub fn create_custom(
        &self,
        name: &str,
        icon: Option<String>,
        description: &str,
        based_on: Option<&str>,
    ) -> Result<AgentPreset, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("预设名不能为空".into());
        }
        if let Some(icon) = &icon {
            check_icon(icon)?;
        }
        let mut inner = self.inner.lock().unwrap();
        let mut preset = match based_on {
            Some(from) => {
                let base = inner
                    .presets
                    .get(from)
                    .cloned()
                    .ok_or_else(|| format!("没有预设 {from}"))?;
                if base.broken.is_some() {
                    return Err("损坏的预设不能当底子".into());
                }
                base
            }
            None => AgentPreset::new(String::new()),
        };
        let id = mint_id(&inner.presets);
        preset.id = id.clone();
        preset.name = name.to_string();
        preset.description = description.trim().to_string();
        preset.icon = icon;
        preset.order = None;
        preset.broken = None;
        preset.origin = PresetOrigin::User;
        inner.presets.insert(id.clone(), preset.clone());
        persist_preset(&inner, &id)?;
        Ok(preset)
    }

    /// 整份改一个预设（GUI 预设编辑器）：基本信息、人设、工具名单、子代理名册一次写下。
    /// 内置预设写成用户层覆盖（同 TUI 画布）；损坏的预设整份重写、不再算损坏（「用默认
    /// 模板重建」）。名册里去掉的角色删掉它的文件；内置预设自带的角色删不掉（加载时会
    /// 从内置合回来），回错。
    pub fn update(&self, id: &str, edit: PresetEdit) -> Result<AgentPreset, String> {
        let name = edit.name.trim().to_string();
        if name.is_empty() {
            return Err("预设名不能为空".into());
        }
        if let Some(icon) = &edit.icon {
            check_icon(icon)?;
        }
        if edit.replace_prompt && edit.persona.trim().is_empty() {
            return Err("整份替换系统提示时，角色提示词不能为空".into());
        }
        let mut agents = IndexMap::new();
        for (role, mut def) in edit.agents {
            if agents.contains_key(&role) {
                return Err(format!("子代理 id 重复：{role}"));
            }
            if !valid_agent_type_id(&role) {
                return Err(format!("子代理 id 无效（ascii slug 或 1–16 汉字）: {role}"));
            }
            if def.replace_prompt && def.persona.trim().is_empty() {
                return Err(format!(
                    "子代理 {role}：整份替换系统提示时，角色提示词不能为空"
                ));
            }
            if def.name.trim().is_empty() {
                def.name = role.clone();
            }
            def.tools = def.tools.map(dedup);
            agents.insert(role, def);
        }
        if let Some(shipped) = parse_shipped(id) {
            if let Some(role) = shipped.agents.keys().find(|r| !agents.contains_key(*r)) {
                return Err(format!("内置子代理不能删除：{role}"));
            }
        }
        let removed: Vec<String> = {
            let mut inner = self.inner.lock().unwrap();
            let Some(preset) = inner.presets.get_mut(id) else {
                return Err(format!("没有预设 {id}"));
            };
            let removed = preset
                .agents
                .keys()
                .filter(|r| !agents.contains_key(*r))
                .cloned()
                .collect();
            if preset.broken.take().is_some() {
                preset.agents.clear();
            }
            preset.name = name;
            preset.description = edit.description.trim().to_string();
            preset.icon = edit.icon;
            preset.order = edit.order;
            preset.persona = edit.persona;
            preset.replace_prompt = edit.replace_prompt;
            preset.tools = edit.tools.map(dedup);
            preset.agents = agents;
            if preset.origin == PresetOrigin::Shipped {
                preset.origin = PresetOrigin::User;
            }
            persist_preset(&inner, id)?;
            removed
        };
        for role in removed {
            self.remove_role_file(id, &role);
        }
        self.get(id).ok_or_else(|| format!("没有预设 {id}"))
    }

    /// 预设落盘的 `agent.yml` 路径（内置未改过的没有文件，回 `None`）。
    pub fn file_of(&self, id: &str) -> Option<PathBuf> {
        let inner = self.inner.lock().unwrap();
        let preset = inner.presets.get(id)?;
        if preset.origin == PresetOrigin::Shipped {
            return None;
        }
        let path = file_path(&inner, preset);
        if path.is_file() {
            return Some(path);
        }
        let legacy = legacy_yml(&inner, preset);
        Some(if legacy.is_file() { legacy } else { path })
    }

    fn remove_role_file(&self, mode_id: &str, role_id: &str) {
        let inner = self.inner.lock().unwrap();
        if let Some(preset) = inner.presets.get(mode_id) {
            if preset.origin != PresetOrigin::Shipped && preset.broken.is_none() {
                let path = file_path(&inner, preset)
                    .parent()
                    .map(|p| p.join(AGENTS_DIR).join(format!("{role_id}.yml")));
                if let Some(path) = path {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
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

    /// Create a new `agents/<id>.yml` role under this mode. Returns `(role_id, def)`.
    pub fn create_subagent(&self, mode_id: &str) -> Result<(String, SubagentDef), String> {
        let mut minted = None;
        self.mutate(mode_id, |preset| {
            let id = mint_role_id(&preset.agents);
            let def = SubagentDef {
                name: id.clone(),
                ..SubagentDef::default()
            };
            preset.agents.insert(id.clone(), def.clone());
            minted = Some((id, def));
            Ok(())
        })?;
        minted.ok_or_else(|| "创建子代理失败".into())
    }

    pub fn delete_subagent(&self, mode_id: &str, role_id: &str) -> Result<(), String> {
        self.mutate(mode_id, |preset| {
            if preset.agents.shift_remove(role_id).is_none() {
                return Err(format!("没有子代理 {role_id}"));
            }
            Ok(())
        })?;
        // Remove only this role's YAML (do not scan-delete other disk files).
        let inner = self.inner.lock().unwrap();
        if let Some(preset) = inner.presets.get(mode_id) {
            if preset.origin != PresetOrigin::Shipped && preset.broken.is_none() {
                let path = file_path(&inner, preset)
                    .parent()
                    .map(|p| p.join(AGENTS_DIR).join(format!("{role_id}.yml")));
                if let Some(path) = path {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        Ok(())
    }

    pub fn set_subagent_persona(
        &self,
        mode_id: &str,
        role_id: &str,
        persona: String,
    ) -> Result<(), String> {
        self.mutate(mode_id, |preset| {
            let Some(def) = preset.agents.get_mut(role_id) else {
                return Err(format!("没有子代理 {role_id}"));
            };
            def.persona = persona;
            Ok(())
        })
    }

    pub fn add_subagent_tool(
        &self,
        mode_id: &str,
        role_id: &str,
        name: &str,
    ) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("工具名不能为空".into());
        }
        self.mutate(mode_id, |preset| {
            let Some(def) = preset.agents.get_mut(role_id) else {
                return Err(format!("没有子代理 {role_id}"));
            };
            match &mut def.tools {
                None => Ok(()),
                Some(list) if list.iter().any(|n| n == name) => Ok(()),
                Some(list) => {
                    list.push(name.to_string());
                    Ok(())
                }
            }
        })
    }

    pub fn remove_subagent_tool(
        &self,
        mode_id: &str,
        role_id: &str,
        name: &str,
        live: &[String],
    ) -> Result<(), String> {
        self.mutate(mode_id, |preset| {
            let Some(def) = preset.agents.get_mut(role_id) else {
                return Err(format!("没有子代理 {role_id}"));
            };
            match &def.tools {
                None => {
                    def.tools = Some(
                        live.iter()
                            .filter(|n| n.as_str() != name)
                            .cloned()
                            .collect(),
                    );
                }
                Some(_) => {
                    if let Some(list) = def.tools.as_mut() {
                        list.retain(|n| n != name);
                    }
                }
            }
            Ok(())
        })
    }

    pub fn assigned_subagent_tools(
        &self,
        mode_id: &str,
        role_id: &str,
        live: &[String],
    ) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let Some(preset) = inner.presets.get(mode_id) else {
            return live.to_vec();
        };
        let Some(def) = preset.agents.get(role_id) else {
            return live.to_vec();
        };
        match &def.tools {
            None => live.to_vec(),
            Some(list) => list.clone(),
        }
    }

    pub fn upsert_subagent(
        &self,
        mode_id: &str,
        role_id: &str,
        def: SubagentDef,
    ) -> Result<(), String> {
        if !valid_agent_type_id(role_id) {
            return Err(format!(
                "子代理 id 无效（ascii slug 或 1–16 汉字）: {role_id}"
            ));
        }
        self.mutate(mode_id, |preset| {
            preset.agents.insert(role_id.to_string(), def);
            Ok(())
        })
    }

    /// Rename a role id and/or display name. When `to_id` differs from `from`,
    /// migrates memory + deletes `agents/<from>.yml` (persist writes the new file).
    pub fn rename_subagent(
        &self,
        mode_id: &str,
        from: &str,
        to_id: &str,
        name: String,
    ) -> Result<(), String> {
        if !valid_agent_type_id(to_id) {
            return Err(format!(
                "子代理 id 无效（ascii slug 或 1–16 汉字）: {to_id}"
            ));
        }
        let id_changed = from != to_id;
        self.mutate(mode_id, |preset| {
            if id_changed {
                if preset.agents.contains_key(to_id) {
                    return Err(format!("子代理 id 已存在: {to_id}"));
                }
                let Some(mut def) = preset.agents.shift_remove(from) else {
                    return Err(format!("没有子代理 {from}"));
                };
                def.name = name;
                preset.agents.insert(to_id.to_string(), def);
            } else {
                let Some(def) = preset.agents.get_mut(from) else {
                    return Err(format!("没有子代理 {from}"));
                };
                def.name = name;
            }
            Ok(())
        })?;
        if id_changed {
            let inner = self.inner.lock().unwrap();
            if let Some(preset) = inner.presets.get(mode_id) {
                if preset.origin != PresetOrigin::Shipped && preset.broken.is_none() {
                    let path = file_path(&inner, preset)
                        .parent()
                        .map(|p| p.join(AGENTS_DIR).join(format!("{from}.yml")));
                    if let Some(path) = path {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        }
        Ok(())
    }

    /// 名册里所有白名单的快照，用来判断一个工具是不是**到处都允许**。
    ///
    /// 取的是**整份名册**（每个预设 + 它旗下每个角色），不是「当前预设」：子代理
    /// 的 `"agentPresets"` 是隔离的，它的 current 是角色而不是模式，按 current 算
    /// 出来的顺序父子两边对不上，那就白排了。整份名册是从同一批文件读出来的，父子
    /// 算出来一模一样。
    ///
    /// 坏掉的预设放行一切（见 [`Self::allows`]），不构成约束。
    pub fn universal_tools(&self) -> Arc<UniversalTools> {
        if let Some(order) = &self.order {
            return order.clone();
        }
        let inner = self.inner.lock().unwrap();
        let mut lists = Vec::new();
        for preset in inner.presets.values() {
            if preset.broken.is_some() {
                continue;
            }
            if let Some(allow) = &preset.tools {
                lists.push(allow.clone());
            }
            for def in preset.agents.values() {
                if let Some(allow) = &def.tools {
                    lists.push(allow.clone());
                }
            }
        }
        Arc::new(UniversalTools { lists })
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

    /// Appended to the model-facing `task` tool so it names this mode's roles.
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
                let desc = def.description.trim();
                let head = if name.is_empty() || name == id {
                    id.clone()
                } else {
                    format!("{id} ({name})")
                };
                if desc.is_empty() {
                    head
                } else {
                    format!("{head}: {desc}")
                }
            })
            .collect();
        format!(
            " subagent_type is a role id from this mode's agents/: {}. After writing a new agents/<id>.yml, call with reload_roster true (no spawn) to refresh this enum. Write new roles under {}/. New Agent modes go in {}/<id>/agent.yml (id [a-z0-9][a-z0-9-]*), then /preset apply; Han display-name dirs overlay a shipped mode and do not create one. Only ~/.dock/presets/ if the user asks to save globally.",
            parts.join("; "),
            self.workspace_agents_dir(),
            self.workspace_presets_dir()
        )
    }

    /// Workspace-relative `.dock/presets` (not an absolute `{cwd}/…` path).
    /// Same cache key as [`Self::workspace_agents_dir`] (current mode).
    pub fn workspace_presets_dir(&self) -> String {
        self.write_hint_snap().presets_dir
    }

    /// Workspace-relative `.dock/presets/{mode}/agents`.
    pub fn workspace_agents_dir(&self) -> String {
        self.write_hint_snap().agents_dir
    }

    fn write_hint_snap(&self) -> WriteHintSnap {
        let mode = self.current_id();
        {
            let snap = self.write_hint.lock().unwrap();
            if let Some(s) = snap.as_ref() {
                if s.mode == mode {
                    return s.clone();
                }
            }
        }
        let snap = WriteHintSnap {
            mode: mode.clone(),
            presets_dir: ".dock/presets".into(),
            agents_dir: format!(".dock/presets/{mode}/agents"),
        };
        *self.write_hint.lock().unwrap() = Some(snap.clone());
        snap
    }

    /// Re-read `agents/` and list callable `subagent_type` ids.
    /// Used by `task` when `reload_roster` is true.
    pub fn reload_roster_report(&self) -> String {
        self.resync();
        let preset = self.current();
        let mut lines = vec![format!(
            "Reloaded agents/ for mode {} ({}). Next model step's task subagent_type enum:",
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
        lines.push("Call task with one of these ids. Do not use a type that is not listed.".into());
        lines.push(format!(
            "New roles go in {}/<type>.yml. New Agent modes go in {}/<id>/agent.yml (id [a-z0-9][a-z0-9-]*), then /preset apply. Do not write ~/.dock/presets/ unless the user asks to save globally.",
            self.workspace_agents_dir(),
            self.workspace_presets_dir()
        ));
        lines.join("\n")
    }

    /// Close the `task` `subagent_type` to the live roster so the sampler
    /// cannot fall back to a trained three-type enum, and append the roster
    /// hint to its description.
    pub fn bind_spawn_schema(&self, spec: &mut ToolSpec) {
        if spec.name != SPAWN_TOOL_NAME {
            return;
        }
        spec.description.push_str(&self.subagent_role_hint());
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

    /// Whether this preset asked for the skills / workflows listings. Only
    /// consulted for child isolates — the main session always gets them.
    pub fn wants_listings(&self) -> bool {
        let preset = self.current();
        preset.broken.is_none() && preset.listings
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

/// [`AgentPresets::update`] 的整份内容。子代理名册按 id（`agents/<id>.yml` 的文件名）。
#[derive(Clone, Debug, Default)]
pub struct PresetEdit {
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub order: Option<i64>,
    pub persona: String,
    pub replace_prompt: bool,
    /// `None` = 全部已注册工具；`Some(vec![])` = 不用工具；其余是允许名单。
    pub tools: Option<Vec<String>>,
    /// 按顺序；id 不能重复。
    pub agents: Vec<(String, SubagentDef)>,
}

/// 内置预设自带的子代理 id（不是内置预设为空）。这些角色在覆盖层里删不掉。
pub fn shipped_roles(id: &str) -> Vec<String> {
    parse_shipped(id)
        .map(|p| p.agents.keys().cloned().collect())
        .unwrap_or_default()
}

fn check_icon(icon: &str) -> Result<(), String> {
    let ok = !icon.is_empty()
        && icon.len() <= 40
        && icon
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!("图标名不合法：{icon}"))
    }
}

fn dedup(list: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    list.into_iter()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty() && seen.insert(n.clone()))
        .collect()
}

pub fn blocked_tool_message() -> &'static str {
    BLOCKED
}

pub fn is_shipped(id: &str) -> bool {
    SHIPPED.iter().any(|m| m.id == id)
}

/// 根会话的预设层：`~/.dock/presets` + 启动目录的 `.dock/presets`。
fn default_layers() -> (PathBuf, Option<PathBuf>) {
    let user = cordis_base::config::dock_home().join("presets");
    let project = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".dock").join("presets"));
    (user, project)
}

pub fn agent_presets() -> Plugin {
    plugin("agent-presets", Inject::from([CONTEXT]), |ctx, _: &()| {
        let (user, project) = default_layers();
        let provided = ctx.provide(AGENT_PRESETS, AgentPresets::load_layers(user, project))?;
        let book = ctx.require::<ContextBook>(CONTEXT)?;
        own_sections(
            ctx,
            vec![
                book.replace_base("persona-replace", |exec| {
                    let presets = exec.get::<AgentPresets>(AGENT_PRESETS)?;
                    if !presets.replaces_prompt() {
                        return None;
                    }
                    let mut persona = String::new();
                    presets.merge_persona(&mut persona);
                    if persona.trim().is_empty() {
                        None
                    } else {
                        Some(persona)
                    }
                })?,
                book.section(ORDER_PERSONA, "persona", |exec| {
                    let presets = exec.get::<AgentPresets>(AGENT_PRESETS)?;
                    let mut persona = String::new();
                    presets.merge_persona(&mut persona);
                    if presets.replaces_prompt() {
                        return None;
                    }
                    let body = persona.strip_prefix("\n\n").unwrap_or(&persona);
                    if body.trim().is_empty() {
                        None
                    } else {
                        Some(body.to_string())
                    }
                })?,
            ],
        )?;
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
    ensure_mcp_discovery_tools(id, &mut overlay);
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
        // Do NOT wipe agents/*.yml missing from memory: handwritten roles that
        // were never reload_roster'd must survive /preset edits of the same mode.
        // Explicit delete_subagent removes that role's file instead.
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

/// 名册里所有工具白名单的快照。见 [`AgentPresets::universal_tools`]。
///
/// 存在的理由是前缀缓存：子代理那张工具表是主会话那张按角色白名单过滤出来的，
/// 被过滤掉的项若散落在数组中间，序列化出来的 `tools` 就会从第一个缺口处和主会话
/// 分叉。而 tools 排在整份 prompt 的最前面，一分叉，后面**整段**都得重算——这正是
/// 每个子代理第一次调用要整份满价的原因。把「到处都允许」的排前面，子代理的数组
/// 就成了主会话数组的真前缀，公共头能整段命中。
#[derive(Clone, Default)]
pub struct UniversalTools {
    lists: Vec<Vec<String>>,
}

impl UniversalTools {
    /// 名册里每一份白名单都放行这个工具？没有任何白名单时全都算（那时本来就没人
    /// 过滤，顺序维持原样）。
    pub fn allows_everywhere(&self, name: &str) -> bool {
        // `task` 例外：它的 schema 由 `bind_spawn_schema` 按当前名册改写，内容
        // 本身就随预设变，排在前面反而把分叉点提前了。
        if name == SPAWN_TOOL_NAME {
            return false;
        }
        self.lists.iter().all(|allow| tool_allowed(allow, name))
    }
}

fn tool_allowed(allow: &[String], name: &str) -> bool {
    if allow.iter().any(|n| n == name) {
        return true;
    }
    // `report` 已并进 `send_message`（子→父同一颗工具）。用户写过的
    // `agents/<type>.yml` 里还留着旧名，照旧名放行，免得升级后子代理回不了话。
    (name == "run_terminal_cmd" && allow.iter().any(|n| n == "bash"))
        || (name == "send_message" && allow.iter().any(|n| n == "report"))
}

/// Overlay YAML snapshots an allowlist. When crate adds `search_tool` /
/// `use_tool`, stale user/project copies would otherwise hide them from
/// `/preset` and `allows()`. Skip `minimal` and explicit empty lists.
fn ensure_mcp_discovery_tools(id: &str, preset: &mut AgentPreset) {
    if id == MINIMAL_PRESET_ID {
        return;
    }
    if !SHIPPED.iter().any(|m| m.id == id) {
        return;
    }
    append_mcp_discovery(&mut preset.tools);
    for def in preset.agents.values_mut() {
        append_mcp_discovery(&mut def.tools);
    }
}

fn append_mcp_discovery(tools: &mut Option<Vec<String>>) {
    let Some(list) = tools else {
        return;
    };
    if list.is_empty() {
        return;
    }
    for name in [
        crate::tools::mcp::SEARCH_TOOL_NAME,
        crate::tools::mcp::USE_TOOL_NAME,
    ] {
        if !list.iter().any(|t| t == name) {
            list.push(name.to_string());
        }
    }
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

fn mint_role_id(agents: &IndexMap<String, SubagentDef>) -> String {
    if !agents.contains_key("role") {
        return "role".into();
    }
    for n in 2..10_000 {
        let id = format!("role-{n}");
        if !agents.contains_key(&id) {
            return id;
        }
    }
    format!("role-{}", uuid::Uuid::now_v7().as_simple())
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
pub(crate) fn valid_agent_type_id(id: &str) -> bool {
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
    // 拼音 overlay 早于 spawn 工具面统一到单一 task：剥掉旧名（subagent 与
    // 一次性 task 套件的附属工具），并保证 task 在列 —— 迁移是换名，不是剥掉
    // spawn 面让用户两手空空。
    tools.retain(|t| {
        t != "subagent" && t != "get_task_output" && t != "wait_tasks" && t != "kill_task"
    });
    if !tools.iter().any(|t| t == "task") {
        tools.push("task".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::{AGENT_PRESETS, TOOLS};
    use crate::tools::registry::{tool_result, Tools};
    use cordis::Context;
    use cordis_base::types::ToolCall;
    use std::sync::Arc;

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
                    !p.tools.as_ref().unwrap().iter().any(|n| n == "job"),
                    "{}",
                    mode.id
                );
                assert!(
                    p.tools.as_ref().unwrap().iter().any(|n| n == "bash"),
                    "{}",
                    mode.id
                );
                assert!(
                    p.tools
                        .as_ref()
                        .unwrap()
                        .iter()
                        .any(|n| n == crate::tools::mcp::SEARCH_TOOL_NAME),
                    "{}",
                    mode.id
                );
                for id in ["岑", "锁", "甲", "乙", "丙", "衡", "验", "观", "突击"] {
                    let def = p.agents.get(id).unwrap_or_else(|| panic!("{id}"));
                    let tools = def.tools.as_ref().unwrap();
                    assert!(tools.iter().any(|n| n == "send_message"), "{id}");
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
                    .any(|n| n == "send_message"));
                assert!(
                    p.tools
                        .as_ref()
                        .unwrap()
                        .iter()
                        .any(|n| n == crate::tools::mcp::SEARCH_TOOL_NAME),
                    "{}",
                    mode.id
                );
            }
        }
    }

    /// `pin` 只切这一份，不写 `roster.yml`：重新加载还是原来的默认。
    #[test]
    fn pin_does_not_touch_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let root = AgentPresets::load(dir.path().to_path_buf());
        root.apply(CORDIS_PRESET_ID).unwrap();
        let page = root.fork();
        page.pin(WARDEN_PRESET_ID).unwrap();
        assert_eq!(page.current_id(), WARDEN_PRESET_ID);
        assert_eq!(root.current_id(), CORDIS_PRESET_ID);
        let reloaded = AgentPresets::load(dir.path().to_path_buf());
        assert_eq!(reloaded.current_id(), CORDIS_PRESET_ID, "默认不该被改");
        assert!(page.pin("no-such").is_err());
        assert_eq!(page.current_id(), WARDEN_PRESET_ID);
    }

    /// 回归：分页各持一份预设。第 2 页切预设、`/cd` 换项目层，都不能改到
    /// 第 1 页——以前所有常驻页共用根上那一份。
    #[test]
    fn forked_presets_switch_independently() {
        let dir = tempfile::tempdir().unwrap();
        let page1 = AgentPresets::load(dir.path().to_path_buf());
        page1.apply(CORDIS_PRESET_ID).unwrap();
        let page2 = page1.fork();
        assert_eq!(page2.current_id(), CORDIS_PRESET_ID, "新页照抄当前预设");

        page2.apply(MINIMAL_PRESET_ID).unwrap();
        assert_eq!(page1.current_id(), CORDIS_PRESET_ID, "第 1 页不该跟着切");

        let project = tempfile::tempdir().unwrap();
        page2.set_workspace_root(project.path());
        assert_eq!(
            page1.inner.lock().unwrap().project_dir,
            None,
            "第 1 页的项目层不该被第 2 页的 /cd 改掉"
        );
    }

    /// 只读 overlay（旁问页）不能被复制成常驻页的预设。
    #[test]
    fn forking_a_read_only_overlay_reloads_full_presets() {
        let overlay = AgentPresets::overlay(AgentPreset::new("aside"));
        let forked = overlay.fork();
        assert!(
            forked.inner.lock().unwrap().persist,
            "常驻页要能落盘的完整预设"
        );
        assert!(forked.get(DEFAULT_PRESET_ID).is_some());
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
        assert!(presets.allows("skill"));
        assert!(presets.allows("task"));
        assert!(presets.allows("send_message"));
        assert!(presets.allows("search_tool"));
        assert!(presets.allows("use_tool"));
        assert!(presets.allows("browser_open"));
        assert!(presets.allows("browser_snapshot"));
        assert!(!presets.allows("report"));
        assert!(!presets.allows("cordis_define"));
        assert!(!presets.allows("scheduler_create"));

        presets.apply(MINIMAL_PRESET_ID).unwrap();
        assert!(presets.allows("read_file"));
        assert!(!presets.allows("web_search"));
        assert!(!presets.allows("search_tool"));
        assert!(!presets.allows("use_tool"));
        assert!(!presets.allows("browser_open"));
        assert!(!presets.allows("cordis_run"));

        presets.apply(CORDIS_PRESET_ID).unwrap();
        assert!(presets.allows("cordis_inspect"));
        assert!(presets.allows("skill"));
        assert!(presets.allows("search_tool"));
        assert!(presets.allows("use_tool"));
        assert!(presets.allows("browser_open"));
        assert!(presets.allows("cordis_run"));
        assert!(presets.allows("cordis_promote"));
        assert!(!presets.allows("scheduler_create"));

        presets.apply(WARDEN_PRESET_ID).unwrap();
        assert!(presets.allows("bash"));
        assert!(presets.allows("run_terminal_cmd"));
        assert!(presets.allows("task"));
        assert!(presets.allows("search_tool"));
        assert!(presets.allows("use_tool"));
        assert!(!presets.allows("browser_open"));
        assert!(!presets.allows("job"));
        assert!(!presets.allows("kill_task"));
        assert!(!presets.allows("report"));
        assert_eq!(presets.role_label("岑").as_deref(), Some("岑"));
        assert_eq!(presets.role_label("观").as_deref(), Some("观"));
        assert!(presets.subagent("jia").is_none());
        assert!(!presets.allows("write_file"));
        assert!(!presets.allows("cordis_run"));
    }

    /// `report` 并进了 `send_message`。用户自己写的角色 YAML 里还列着旧名时，
    /// 照样放行 `send_message`，否则升级后这些子代理就回不了话。
    #[test]
    fn report_in_a_user_allowlist_still_allows_send_message() {
        let allow = vec!["read_file".to_string(), "report".to_string()];
        assert!(tool_allowed(&allow, "send_message"));
        assert!(!tool_allowed(&allow, "interrupt_agent"));
        assert!(!tool_allowed(&["read_file".to_string()], "send_message"));
    }

    #[test]
    fn stale_overlay_allowlist_gains_search_tool() {
        let dir = tempfile::tempdir().unwrap();
        let code = dir.path().join("code");
        std::fs::create_dir_all(code.join("agents")).unwrap();
        std::fs::write(
            code.join("agent.yml"),
            "name: 编码\ntools:\n  - bash\n  - read_file\n",
        )
        .unwrap();
        std::fs::write(
            code.join("agents").join("explore.yml"),
            "name: 探索\ntools:\n  - read_file\n  - report\n",
        )
        .unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        assert!(presets.allows("bash"));
        assert!(presets.allows("search_tool"));
        assert!(presets.allows("use_tool"));
        let explore = presets.subagent("explore").unwrap();
        let tools = explore.tools.unwrap();
        assert!(tools.iter().any(|n| n == "search_tool"), "{tools:?}");
        assert!(tools.iter().any(|n| n == "use_tool"), "{tools:?}");

        let ctx = Context::new();
        ctx.provide(AGENT_PRESETS, presets).unwrap();
        let tools_svc = Tools::echo(ctx.clone());
        let body: crate::tools::registry::ToolBody =
            Arc::new(|c| Box::pin(async move { tool_result(c, "ok") }));
        tools_svc
            .register(spec("search_tool"), body.clone())
            .unwrap();
        tools_svc.register(spec("use_tool"), body.clone()).unwrap();
        tools_svc.register(spec("bash"), body).unwrap();
        let model: Vec<String> = tools_svc
            .specs_for_model_on(&ctx)
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert!(model.iter().any(|n| n == "search_tool"), "{model:?}");
        assert!(model.iter().any(|n| n == "use_tool"), "{model:?}");
    }

    #[test]
    fn subagent_crud_roundtrip_via_api() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let mode = presets.create().unwrap();
        let (rid, def) = presets.create_subagent(&mode.id).unwrap();
        assert_eq!(rid, "role");
        assert_eq!(def.name, "role");
        presets
            .set_subagent_persona(&mode.id, &rid, "你是侦察".into())
            .unwrap();
        presets.add_subagent_tool(&mode.id, &rid, "bash").unwrap();
        // open allowlist starts as None → add is no-op; remove snapshots.
        let live = vec!["bash".into(), "read_file".into()];
        presets
            .remove_subagent_tool(&mode.id, &rid, "read_file", &live)
            .unwrap();
        let tools = presets.assigned_subagent_tools(&mode.id, &rid, &live);
        assert_eq!(tools, vec!["bash".to_string()]);
        presets
            .set_subagent_persona(&mode.id, &rid, "更新".into())
            .unwrap();
        let got = presets.get(&mode.id).unwrap();
        assert_eq!(got.agents.get(&rid).unwrap().persona, "更新");
        presets.delete_subagent(&mode.id, &rid).unwrap();
        assert!(presets.get(&mode.id).unwrap().agents.is_empty());
        let agents_dir = dir.path().join(&mode.id).join("agents");
        assert!(
            !agents_dir.join("role.yml").exists(),
            "deleted role.yml should be removed"
        );
    }

    #[test]
    fn upsert_subagent_custom_id_persists() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let mode = presets.create().unwrap();
        let def = SubagentDef {
            name: "侦察".into(),
            ..SubagentDef::default()
        };
        presets.upsert_subagent(&mode.id, "scout", def).unwrap();
        let got = presets.get(&mode.id).unwrap();
        assert!(got.agents.contains_key("scout"));
        assert_eq!(got.agents.get("scout").unwrap().name, "侦察");
        let yml = dir.path().join(&mode.id).join("agents").join("scout.yml");
        assert!(yml.is_file(), "custom id must land as agents/scout.yml");
    }

    #[test]
    fn rename_subagent_migrates_agents_yml() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let mode = presets.create().unwrap();
        presets
            .upsert_subagent(
                &mode.id,
                "old-role",
                SubagentDef {
                    name: "旧名".into(),
                    persona: "keep".into(),
                    ..SubagentDef::default()
                },
            )
            .unwrap();
        let agents_dir = dir.path().join(&mode.id).join("agents");
        assert!(agents_dir.join("old-role.yml").is_file());
        presets
            .rename_subagent(&mode.id, "old-role", "new-role", "新名".into())
            .unwrap();
        let got = presets.get(&mode.id).unwrap();
        assert!(!got.agents.contains_key("old-role"));
        assert_eq!(got.agents.get("new-role").unwrap().name, "新名");
        assert_eq!(got.agents.get("new-role").unwrap().persona, "keep");
        assert!(
            !agents_dir.join("old-role.yml").exists(),
            "old agents yml must be removed on id rename"
        );
        assert!(agents_dir.join("new-role.yml").is_file());
    }

    #[test]
    fn upsert_subagent_rejects_invalid_id() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let mode = presets.create().unwrap();
        let err = presets
            .upsert_subagent(
                &mode.id,
                "Bad_ID",
                SubagentDef {
                    name: "x".into(),
                    ..SubagentDef::default()
                },
            )
            .unwrap_err();
        assert!(err.contains("无效"), "{err}");
        let err = presets
            .rename_subagent(&mode.id, "missing", "also Bad", "n".into())
            .unwrap_err();
        assert!(err.contains("无效"), "{err}");
    }

    #[test]
    fn persist_keeps_handwritten_agents_yml_not_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let mode = presets.create().unwrap();
        let agents_dir = dir.path().join(&mode.id).join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("scout.yml"),
            "name: scout\npersona: 手写未 reload\n",
        )
        .unwrap();
        // Touch mode via /preset-like mutate without loading scout into memory.
        presets
            .set_persona(&mode.id, "改名 persona".into())
            .unwrap();
        assert!(
            agents_dir.join("scout.yml").exists(),
            "handwritten agents/scout.yml must survive persist without reload"
        );
        assert!(!presets.get(&mode.id).unwrap().agents.contains_key("scout"));
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
        // 迁移是换名不是剥面：旧 overlay 只写了 subagent，迁移后必须还有 task。
        assert!(presets.allows("task"), "{:?}", presets.current().tools);
        assert!(!presets.allows("subagent"), "{:?}", presets.current().tools);
        // 角色 id 现在只从 `task` 工具的 role hint 出去。
        let hint = presets.subagent_role_hint();
        assert!(!hint.contains("jia"), "{hint}");
        assert!(hint.contains("甲"), "{hint}");
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
        assert!(presets.allows("task"));
        assert!(!presets.allows("subagent"));
        assert!(!presets.allows("job"));
        assert!(!presets.allows("kill_task"));
        assert!(presets.allows("write_file"));
        assert!(presets.subagent("jia").is_none());
        let jia = presets.subagent("甲").unwrap();
        assert!(!jia.persona.contains("调度标签"), "{}", jia.persona);
        assert!(jia.persona.contains("只守住甲"), "{}", jia.persona);
        let hint = presets.subagent_role_hint();
        assert!(!hint.contains("jia"), "{hint}");
        // spawn 面统一后只教 task；旧一次性套件不出现在委派说明里。
        assert!(!hint.contains("kill_task"), "{hint}");
        assert!(hint.contains("甲"), "{hint}");
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
        // 名册与写路径归 `task` 工具的 role hint，不再在系统提示里重写一遍。
        assert!(!assembled.contains("send_message"), "{assembled}");
        assert!(!assembled.contains("reload_roster"), "{assembled}");
        assert_eq!(presets.role_label("explore").as_deref(), Some("探索"));
        let hint = presets.subagent_role_hint();
        assert!(hint.contains("explore"), "{hint}");
        assert!(hint.contains("general-purpose"), "{hint}");
        assert!(hint.contains("reload_roster"), "{hint}");
        assert!(hint.contains(".dock/presets/code/agents"), "{hint}");
        assert!(hint.contains(".dock/presets"), "{hint}");
        assert!(hint.contains("~/.dock/presets"), "{hint}");
        assert!(hint.contains("globally"), "{hint}");
        assert!(
            !hint.contains(&std::env::current_dir().unwrap().display().to_string()),
            "role hint must not embed an absolute cwd: {hint}"
        );
        // 角色说明现在也进 hint（原先只有 roster 段有）。
        assert!(hint.contains("只读探索代码库"), "{hint}");
    }

    #[test]
    fn roster_hint_uses_workspace_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let hint = presets.subagent_role_hint();
        assert!(hint.contains(".dock/presets/code/agents"), "{hint}");
        assert!(hint.contains("New Agent modes"), "{hint}");
        assert!(hint.contains("globally"), "{hint}");
        assert!(hint.contains(".dock/presets"), "{hint}");
        assert!(
            !hint.contains(&std::env::current_dir().unwrap().display().to_string()),
            "{hint}"
        );
    }

    #[test]
    fn write_hint_path_stable_until_mode_changes() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let a = presets.workspace_agents_dir();
        let b = presets.workspace_agents_dir();
        assert_eq!(a, b);
        assert_eq!(a, ".dock/presets/code/agents");
        let presets_dir = presets.workspace_presets_dir();
        assert_eq!(presets_dir, ".dock/presets");
        presets.apply(WARDEN_PRESET_ID).unwrap();
        let c = presets.workspace_agents_dir();
        assert_ne!(a, c);
        assert_eq!(c, ".dock/presets/warden/agents");
        assert_eq!(presets_dir, presets.workspace_presets_dir());
    }

    #[test]
    fn empty_mode_still_injects_write_paths() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        presets.apply(MINIMAL_PRESET_ID).unwrap();
        let hint = presets.subagent_role_hint();
        assert!(hint.contains("no agents/ roles"), "{hint}");
        assert!(hint.contains("new Agent mode"), "{hint}");
        assert!(hint.contains("[a-z0-9]"), "{hint}");
        assert!(hint.contains(".dock/presets"), "{hint}");
        assert!(hint.contains(".dock/presets/minimal/agents"), "{hint}");
        assert!(hint.contains("globally"), "{hint}");
        let report = presets.reload_roster_report();
        assert!(report.contains("empty roster"), "{report}");
        assert!(report.contains(".dock/presets"), "{report}");
    }

    /// `listings` is additive: absent from old YAML, and never written back
    /// when a preset is persisted, so `/preset` edits do not rewrite user files.
    #[test]
    fn listings_defaults_off_and_is_not_serialized() {
        let legacy: SubagentDef = serde_yaml::from_str("name: 探索\npersona: hi\n").unwrap();
        assert!(!legacy.listings, "absent listings must default to false");

        let opted: SubagentDef = serde_yaml::from_str("name: 通用\nlistings: true\n").unwrap();
        assert!(opted.listings);
        assert!(opted.to_preset("general-purpose").listings);

        let dumped = serde_yaml::to_string(&legacy).unwrap();
        assert!(!dumped.contains("listings"), "{dumped}");
        let dumped_on = serde_yaml::to_string(&opted).unwrap();
        assert!(dumped_on.contains("listings: true"), "{dumped_on}");
    }

    /// `listings: true` puts the skills / workflows catalogs into a child's
    /// system prompt, and both headers name their loader (`skill` /
    /// `workflow`). A role that advertises them without holding the tool burns
    /// a round: the sampler hides it, `search_tool` no longer lists it (those
    /// tools stopped being deferred), and `use_tool` hits the same allowlist.
    #[test]
    fn a_listing_role_holds_the_tools_its_listing_names() {
        let dir = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load(dir.path().to_path_buf());
        let mut checked = 0usize;
        for preset in presets.list() {
            if !preset.builtin() || preset.broken.is_some() {
                continue;
            }
            for (id, def) in &preset.agents {
                if !def.listings {
                    continue;
                }
                checked += 1;
                let overlay = AgentPresets::overlay(def.to_preset(id));
                assert!(overlay.wants_listings(), "{id} advertises listings");
                for tool in ["skill", "workflow"] {
                    assert!(
                        overlay.allows(tool),
                        "{}/{} advertises the {tool} listing without holding the tool: {:?}",
                        preset.id,
                        id,
                        def.tools
                    );
                }
            }
        }
        assert!(
            checked > 0,
            "no builtin role opts into listings — the assertion above is vacuous"
        );
    }

    /// The roster used to be a system-prompt section that repeated, in Chinese,
    /// what the `task` tool description already said in English. It is gone:
    /// role ids, write paths and the mailbox semantics now live on the tools.
    /// A child overlay therefore cannot inherit parent-only write hints.
    #[tokio::test]
    async fn roster_section_is_gone_from_the_assembly() {
        let dir = tempfile::tempdir().unwrap();
        let _env = cordis_base::test_env::scoped().home().cwd(dir.path());
        let ctx = cordis::Context::new();
        ctx.plugin(crate::prompt::context_book::context(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        ctx.plugin(agent_presets(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let book = ctx.get::<ContextBook>(CONTEXT).unwrap();
        let ids: Vec<String> = book
            .assemble()
            .inspect()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert!(!ids.iter().any(|id| id == "roster"), "{ids:?}");
        let rendered = book.assemble().render();
        assert!(!rendered.contains("本模式子代理"), "{rendered}");
        assert!(!rendered.contains("send_message"), "{rendered}");
        assert!(!rendered.contains("reload_roster"), "{rendered}");
    }

    /// GUI 的「新建自定义预设」：起名、带图标、照某个预设复制人设和工具；写用户层，
    /// 不切当前预设；重新加载后图标还在。
    #[test]
    fn create_custom_names_it_keeps_the_icon_and_leaves_current_alone() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let presets =
            AgentPresets::load_layers(home.path().to_path_buf(), Some(proj.path().to_path_buf()));
        let before = presets.current_id();
        let base = presets.get(MINIMAL_PRESET_ID).unwrap();

        let made = presets
            .create_custom(
                "我的助手",
                Some("rocket".into()),
                "说明",
                Some(MINIMAL_PRESET_ID),
            )
            .unwrap();
        assert_eq!(made.name, "我的助手");
        assert_eq!(made.icon.as_deref(), Some("rocket"));
        assert_eq!(made.tools, base.tools, "照 minimal 复制工具名单");
        assert_eq!(made.origin, PresetOrigin::User);
        assert!(home.path().join(&made.id).join(AGENT_FILE).is_file());
        assert!(!proj.path().join(&made.id).join(AGENT_FILE).is_file());
        assert_eq!(presets.current_id(), before, "新建不切当前预设");

        let reloaded =
            AgentPresets::load_layers(home.path().to_path_buf(), Some(proj.path().to_path_buf()));
        assert_eq!(
            reloaded.get(&made.id).unwrap().icon.as_deref(),
            Some("rocket")
        );

        assert!(presets.create_custom("  ", None, "", None).is_err());
        assert!(presets
            .create_custom("x", Some("../evil".into()), "", None)
            .is_err());
        assert!(presets.create_custom("x", None, "", Some("nope")).is_err());
    }

    #[test]
    fn update_rewrites_the_whole_preset_and_its_roster() {
        let home = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load_layers(home.path().to_path_buf(), None);
        let made = presets
            .create_custom("研究员", None, "", Some(MINIMAL_PRESET_ID))
            .unwrap();
        let role = |persona: &str| SubagentDef {
            name: String::new(),
            description: "查网页".into(),
            persona: persona.into(),
            tools: Some(vec!["web_fetch".into(), "web_fetch".into()]),
            replace_prompt: false,
            listings: true,
        };
        let edit = |agents: IndexMap<String, SubagentDef>| PresetEdit {
            agents: agents.into_iter().collect(),
            name: "深度研究员".into(),
            description: " 跨文档综合 ".into(),
            icon: Some("search".into()),
            order: Some(10),
            persona: "你是研究员。".into(),
            replace_prompt: false,
            tools: Some(vec!["read_file".into(), "grep".into()]),
        };
        let mut roster = IndexMap::new();
        roster.insert("web".to_string(), role("只查网页"));
        roster.insert("data".to_string(), role("只算数"));
        let saved = presets.update(&made.id, edit(roster.clone())).unwrap();
        assert_eq!(saved.name, "深度研究员");
        assert_eq!(saved.description, "跨文档综合");
        assert_eq!(saved.icon.as_deref(), Some("search"));
        assert_eq!(saved.order, Some(10));
        assert_eq!(saved.agents["web"].name, "web", "空名字用 id");
        assert_eq!(
            saved.agents["web"].tools.as_deref(),
            Some(&["web_fetch".to_string()][..]),
            "工具去重"
        );
        let agents_dir = home.path().join(&made.id).join(AGENTS_DIR);
        assert!(agents_dir.join("data.yml").is_file());

        // 名册里去掉的角色：文件也删掉，重读后不再回来。
        roster.shift_remove("data");
        presets.update(&made.id, edit(roster.clone())).unwrap();
        assert!(!agents_dir.join("data.yml").exists());
        let reloaded = AgentPresets::load_layers(home.path().to_path_buf(), None);
        let back = reloaded.get(&made.id).unwrap();
        assert_eq!(back.persona, "你是研究员。");
        assert_eq!(back.tools.as_ref().map(Vec::len), Some(2));
        assert_eq!(back.agents.keys().collect::<Vec<_>>(), vec!["web"]);
        assert!(back.agents["web"].listings);

        // 校验：空名、整份替换却没有提示词、坏的角色 id、坏的图标。
        let mut bad = edit(roster.clone());
        bad.name = " ".into();
        assert!(presets.update(&made.id, bad).is_err());
        let mut bad = edit(roster.clone());
        bad.replace_prompt = true;
        bad.persona = "  ".into();
        assert!(presets.update(&made.id, bad).is_err());
        let mut bad = edit(IndexMap::new());
        bad.agents.push(("Bad Id".into(), role("x")));
        assert!(presets.update(&made.id, bad).is_err());
        let mut bad = edit(roster);
        bad.icon = Some("../x".into());
        assert!(presets.update(&made.id, bad).is_err());
        assert!(presets.update("nope", edit(IndexMap::new())).is_err());
    }

    #[test]
    fn update_on_builtin_writes_an_overlay_and_keeps_shipped_roles() {
        let home = tempfile::tempdir().unwrap();
        let presets = AgentPresets::load_layers(home.path().to_path_buf(), None);
        let code = presets.get(DEFAULT_PRESET_ID).unwrap();
        assert!(
            presets.file_of(DEFAULT_PRESET_ID).is_none(),
            "没改过没有文件"
        );
        let edit = |agents: IndexMap<String, SubagentDef>| PresetEdit {
            agents: agents.into_iter().collect(),
            name: code.name.clone(),
            description: "改过".into(),
            icon: None,
            order: code.order,
            persona: code.persona.clone(),
            replace_prompt: false,
            tools: code.tools.clone(),
        };
        let mut dropped = code.agents.clone();
        let first = dropped.keys().next().unwrap().clone();
        dropped.shift_remove(&first);
        let err = presets
            .update(DEFAULT_PRESET_ID, edit(dropped))
            .unwrap_err();
        assert!(err.contains(&first), "{err}");

        let saved = presets
            .update(DEFAULT_PRESET_ID, edit(code.agents.clone()))
            .unwrap();
        assert_eq!(saved.origin, PresetOrigin::User);
        let file = presets.file_of(DEFAULT_PRESET_ID).unwrap();
        assert!(file.starts_with(home.path()) && file.is_file(), "{file:?}");
        let reloaded = AgentPresets::load_layers(home.path().to_path_buf(), None);
        assert_eq!(reloaded.get(DEFAULT_PRESET_ID).unwrap().description, "改过");
    }

    #[test]
    fn update_rebuilds_a_broken_preset() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(AGENT_FILE), "tools: [bash\n").unwrap();
        let presets = AgentPresets::load_layers(home.path().to_path_buf(), None);
        assert!(presets.get("broken").unwrap().broken.is_some());
        let fixed = presets
            .update(
                "broken",
                PresetEdit {
                    name: "broken".into(),
                    ..PresetEdit::default()
                },
            )
            .unwrap();
        assert!(fixed.broken.is_none());
        let reloaded = AgentPresets::load_layers(home.path().to_path_buf(), None);
        assert!(reloaded.get("broken").unwrap().broken.is_none());
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
        let hint = presets.subagent_role_hint();
        assert!(hint.contains("scout"), "{hint}");
        assert!(hint.contains("侦察"), "{hint}");
        assert!(
            hint.contains("新角色"),
            "role descriptions ride along: {hint}"
        );
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
            name: "task".into(),
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
