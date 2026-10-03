//! Disk-backed Cordis plugins. Directory name is the stable pluginId.
//!
//! `{project}/.dock/plugins/<id>/` overlays `~/.dock/plugins/<id>/`. DSH does
//! not persist dynamic packages; this is Dock's Host-only stand-in for "write
//! a real plugin" when the user cannot ship a cordis-rust crate.

use std::path::{Component, Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::registry::{Package, PersistScope, PluginOrigin};
use super::{DefineReceipt, DefineTarget, DynamicRunner, RunMode, SourceInput};
use crate::host::slash::{slash_entry_from_define, SlashEntry};
use crate::tools::dynamic_runner::factories::RHAI_FACTORY;
use crate::tools::dynamic_runner::rhai_host::{
    FILE_TOO_LARGE, INLINE_TOO_LARGE, MAX_FILE_SOURCE, MAX_INLINE_SOURCE,
};
use cordis_base::config::dock_home;

/// Session id of autoloaded disk plugins. Visible to every chat session.
pub const PERSIST_SESSION: &str = "*";

/// Tail of the promote-vs-session-Plugin collision error. A const so
/// `every_cordis_name_in_model_text_is_a_live_tool` covers it: the first
/// version said "undefine it".
pub const PROMOTE_SESSION_COLLISION_HINT: &str =
    "cordis_stop it with drop:true, or pass a different `id`";

#[derive(Clone, Debug)]
pub struct PromoteReceipt {
    pub plugin_id: String,
    pub package_id: String,
    pub path: PathBuf,
    pub scope: PersistScope,
    pub running: bool,
    pub source_plugin_id: String,
    pub stopped_source: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PluginManifest {
    name: String,
    purpose: String,
    factory: String,
    #[serde(default = "enabled_true")]
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    send: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

fn enabled_true() -> bool {
    true
}

#[derive(Clone, Debug)]
pub struct DiskSpec {
    pub id: String,
    pub path: PathBuf,
    pub scope: PersistScope,
    pub name: String,
    pub purpose: String,
    pub factory: String,
    pub enabled: bool,
    pub source_path: Option<PathBuf>,
    pub contrib: Option<SlashEntry>,
}

/// 项目级 = **正在跑的那个会话**的工作目录（桌面 GUI 每个会话一个项目）；不在任何
/// 一轮里（启动、TUI 视图）就是进程 cwd——TUI 从哪启动就认哪的项目插件，和以前一样。
pub fn persist_root(scope: PersistScope) -> PathBuf {
    match scope {
        PersistScope::User => dock_home().join("plugins"),
        PersistScope::Project => project_root(&crate::session::cwd::current_cwd()),
    }
}

/// 某个项目目录下的插件根：`<项目>/.dock/plugins`。
pub fn project_root(project: &Path) -> PathBuf {
    project.join(".dock").join("plugins")
}

pub fn plugin_roots() -> Vec<(PersistScope, PathBuf)> {
    vec![
        (PersistScope::User, persist_root(PersistScope::User)),
        (PersistScope::Project, persist_root(PersistScope::Project)),
    ]
}

pub fn valid_disk_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (2..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Resolve `cordis_define` `source_path` under project or user plugin roots.
/// Rejects `..`, paths outside the roots, and non-files.
pub fn resolve_source_path(raw: &str) -> Result<PathBuf, String> {
    resolve_source_path_in(raw, &plugin_roots())
}

pub fn resolve_source_path_in(
    raw: &str,
    roots: &[(PersistScope, PathBuf)],
) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("source_path must be non-empty".into());
    }
    let path = PathBuf::from(raw);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("source_path must not contain '..'".into());
    }
    let abs = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    assert_under_plugin_root_in(&abs, roots)
}

/// Re-canonicalize `path` and verify it still resolves under a plugin root.
/// Call before every re-read of a stored `source_path` (run/update/promote).
pub fn assert_under_plugin_root(path: &Path) -> Result<PathBuf, String> {
    assert_under_plugin_root_in(path, &plugin_roots())
}

pub fn assert_under_plugin_root_in(
    path: &Path,
    roots: &[(PersistScope, PathBuf)],
) -> Result<PathBuf, String> {
    let canon = path
        .canonicalize()
        .map_err(|e| format!("source_path {}: {e}", path.display()))?;
    if !canon.is_file() {
        return Err(format!("source_path is not a file: {}", canon.display()));
    }
    let under = roots.iter().any(|(_, root)| match root_canonicalize(root) {
        Some(root_canon) => path_under_plugin_root(&canon, &root_canon),
        None => false,
    });
    if !under {
        return Err(format!(
            "source_path must resolve under .dock/plugins/<id>/ or ~/.dock/plugins/<id>/; got {}",
            canon.display()
        ));
    }
    Ok(canon)
}

fn root_canonicalize(root: &Path) -> Option<PathBuf> {
    if !root.exists() {
        return None;
    }
    root.canonicalize().ok()
}

fn path_under_plugin_root(canon: &Path, root_canon: &Path) -> bool {
    let Ok(rel) = canon.strip_prefix(root_canon) else {
        return false;
    };
    let mut comps = rel.components();
    let Some(Component::Normal(id)) = comps.next() else {
        return false;
    };
    let Some(id) = id.to_str() else {
        return false;
    };
    valid_disk_id(id) && comps.next().is_some()
}

fn source_too_large_msg(max_bytes: usize) -> String {
    match max_bytes {
        MAX_INLINE_SOURCE => INLINE_TOO_LARGE.into(),
        MAX_FILE_SOURCE => FILE_TOO_LARGE.into(),
        n => format!("rhai source exceeds {n} bytes"),
    }
}

/// 重读前再确认一次路径**仍然**落在授权范围内，防的是 define 之后把文件换成
/// 软链的那一手。
///
/// `allowed_dir` 是这次授权的边界：
/// - `None` —— 走全局 [`plugin_roots`]，`cordis_define` 传进来的 `source_path` 用这条。
/// - `Some(dir)` —— 只认这个插件自己的目录。磁盘插件走这条，因为扫描用的 root
///   可以是调用方传进来的（[`DynamicRunner::boot_disk_from`]），写死全局 root 会
///   让它**静默**加载不上。
fn assert_still_authorized(path: &Path, allowed_dir: Option<&Path>) -> Result<PathBuf, String> {
    match allowed_dir {
        None => assert_under_plugin_root(path),
        Some(dir) => {
            let canon = path
                .canonicalize()
                .map_err(|e| format!("source_path {}: {e}", path.display()))?;
            if !canon.is_file() {
                return Err(format!("source_path is not a file: {}", canon.display()));
            }
            let dir_canon = dir
                .canonicalize()
                .map_err(|e| format!("source_path {}: {e}", dir.display()))?;
            if !canon.starts_with(&dir_canon) {
                return Err(format!(
                    "source_path must resolve under {}; got {}",
                    dir_canon.display(),
                    canon.display()
                ));
            }
            Ok(canon)
        }
    }
}

/// Read a path-backed Rhai source after re-validating it. See
/// [`assert_still_authorized`] for what `allowed_dir` means.
pub fn read_source_file(
    path: &Path,
    max_bytes: usize,
    allowed_dir: Option<&Path>,
) -> Result<String, String> {
    let path = assert_still_authorized(path, allowed_dir)?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if meta.len() as usize > max_bytes {
        return Err(source_too_large_msg(max_bytes));
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if text.len() > max_bytes {
        return Err(source_too_large_msg(max_bytes));
    }
    Ok(text)
}

pub fn package_source_text(
    pkg: &Package,
    allowed_dir: Option<&Path>,
) -> Result<(String, usize), String> {
    match (&pkg.source, &pkg.source_path) {
        (Some(s), None) => Ok((s.clone(), MAX_INLINE_SOURCE)),
        (None, Some(p)) => Ok((
            read_source_file(p, MAX_FILE_SOURCE, allowed_dir)?,
            MAX_FILE_SOURCE,
        )),
        (Some(_), Some(_)) => Err("rhai package has both source and source_path".into()),
        (None, None) => Err("rhai package has no source to read".into()),
    }
}

fn source_input_for_package(
    pkg: &Package,
    allowed_dir: Option<&Path>,
) -> Result<Option<SourceInput>, String> {
    match (&pkg.source, &pkg.source_path) {
        (Some(s), None) => Ok(Some(SourceInput::Inline(s.clone()))),
        (None, Some(p)) => Ok(Some(SourceInput::ResolvedPath {
            path: p.clone(),
            dir: allowed_dir.map(|d| d.to_path_buf()),
        })),
        (Some(_), Some(_)) => Err("rhai package has both source and source_path".into()),
        (None, None) => Ok(None),
    }
}

/// 磁盘插件的授权边界就是它自己那个目录。
pub(super) fn origin_source_dir(origin: &PluginOrigin) -> Option<&Path> {
    match origin {
        PluginOrigin::Disk { path, .. } => Some(path.as_path()),
        _ => None,
    }
}

pub fn default_disk_id(plugin_id: &str) -> String {
    if let Some((head, tail)) = plugin_id.rsplit_once('-') {
        if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) && valid_disk_id(head) {
            return head.to_string();
        }
    }
    plugin_id.to_string()
}

pub fn scan(roots: &[(PersistScope, PathBuf)]) -> Vec<DiskSpec> {
    let (specs, broken) = scan_report(roots);
    for b in broken {
        eprintln!("[cordis] skip disk plugin {}: {}", b.id, b.error);
    }
    specs
}

/// 读不起来的磁盘插件目录（设置页要把原因列出来，不能只打到 stderr）。
#[derive(Clone, Debug)]
pub struct BrokenDisk {
    pub id: String,
    pub path: PathBuf,
    pub scope: PersistScope,
    pub error: String,
}

/// [`scan`] 的完整版：能加载的 + 加载失败的。同 id 后面的根（项目）覆盖前面的
/// （用户）；读不起来的不覆盖已经读起来的那份（和以前只打日志时一样）。
pub fn scan_report(roots: &[(PersistScope, PathBuf)]) -> (Vec<DiskSpec>, Vec<BrokenDisk>) {
    let mut by_id: IndexMap<String, DiskSpec> = IndexMap::new();
    let mut broken = Vec::new();
    for (scope, root) in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for ent in entries.flatten() {
            let path = ent.path();
            if !path.is_dir() {
                continue;
            }
            let Some(id) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if !valid_disk_id(id) {
                continue;
            }
            match load_dir(&path, id, *scope) {
                Ok(spec) => {
                    by_id.insert(id.to_string(), spec);
                }
                Err(error) => broken.push(BrokenDisk {
                    id: id.to_string(),
                    path: path.clone(),
                    scope: *scope,
                    error,
                }),
            }
        }
    }
    (by_id.into_values().collect(), broken)
}

/// 改磁盘插件 `plugin.toml` 的 `enabled`（就地改，其余键不动）。
pub fn write_disk_enabled(dir: &Path, enabled: bool) -> Result<(), String> {
    let path = dir.join("plugin.toml");
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read plugin.toml: {e}"))?;
    let mut doc = raw
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("plugin.toml: {e}"))?;
    doc["enabled"] = toml_edit::value(enabled);
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("write plugin.toml: {e}"))
}

fn load_dir(dir: &Path, id: &str, scope: PersistScope) -> Result<DiskSpec, String> {
    let raw = std::fs::read_to_string(dir.join("plugin.toml"))
        .map_err(|e| format!("read plugin.toml: {e}"))?;
    let man: PluginManifest = toml::from_str(&raw).map_err(|e| format!("plugin.toml: {e}"))?;
    if man.name.trim().is_empty() || man.purpose.trim().is_empty() {
        return Err("plugin.toml needs name and purpose".into());
    }
    let source_path = if man.factory == RHAI_FACTORY {
        let p = dir.join("source.rhai");
        if !p.is_file() {
            return Err("rhai plugin needs source.rhai".into());
        }
        Some(p)
    } else {
        None
    };
    let contrib = if man.factory == "slash" {
        let v = json!({
            "command": man.command,
            "kind": man.kind,
            "text": man.text,
            "title": man.title,
            "send": man.send,
            "description": man.description,
        });
        Some(slash_entry_from_define(&v, &man.purpose)?)
    } else if man.command.is_some() || man.kind.is_some() || man.text.is_some() {
        return Err("command/kind/text are only for factory \"slash\"".into());
    } else {
        None
    };
    Ok(DiskSpec {
        id: id.to_string(),
        path: dir.to_path_buf(),
        scope,
        name: man.name,
        purpose: man.purpose,
        factory: man.factory,
        enabled: man.enabled,
        source_path,
        contrib,
    })
}

fn write_plugin(root: &Path, id: &str, pkg: &Package, enabled: bool) -> Result<PathBuf, String> {
    if !valid_disk_id(id) {
        return Err(format!(
            "disk plugin id {id:?} must be 2–32 chars, start with a-z, then a-z0-9-"
        ));
    }
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let mut man = PluginManifest {
        name: pkg.name.clone(),
        purpose: pkg.purpose.clone(),
        factory: pkg.factory.clone(),
        enabled,
        command: None,
        kind: None,
        text: None,
        title: None,
        send: None,
        description: None,
    };
    if let Some(c) = &pkg.contrib {
        man.command = Some(c.command.clone());
        man.kind = Some(c.kind.as_str().to_string());
        man.text = Some(c.text.clone());
        if !c.title.is_empty() {
            man.title = Some(c.title.clone());
        }
        man.send = Some(c.send);
        man.description = Some(c.description.clone());
    }
    let toml = toml::to_string_pretty(&man).map_err(|e| format!("encode plugin.toml: {e}"))?;
    std::fs::write(dir.join("plugin.toml"), toml).map_err(|e| format!("write plugin.toml: {e}"))?;
    if pkg.factory == RHAI_FACTORY {
        // promote 读的是会话包，它的 source_path 当初是按全局 root 授权的。
        let (src, _) = package_source_text(pkg, None)?;
        std::fs::write(dir.join("source.rhai"), src)
            .map_err(|e| format!("write source.rhai: {e}"))?;
    }
    Ok(dir)
}

impl DynamicRunner {
    pub async fn boot_disk(&self) {
        self.boot_disk_from(&plugin_roots()).await;
    }

    pub async fn boot_disk_from(&self, roots: &[(PersistScope, PathBuf)]) {
        for spec in scan(roots) {
            if self.inner.lock().unwrap().registry.get(&spec.id).is_some() {
                continue;
            }
            match self.install_disk(&spec) {
                Ok(defined) => {
                    if !spec.enabled {
                        continue;
                    }
                    if let Err(e) = self
                        .run(
                            PERSIST_SESSION,
                            &defined.plugin_id,
                            &defined.package_id,
                            RunMode::Run,
                        )
                        .await
                    {
                        eprintln!(
                            "[cordis] disk plugin {} defined but did not start: {e}",
                            spec.id
                        );
                    }
                }
                Err(e) => eprintln!("[cordis] disk plugin {}: {e}", spec.id),
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // 绘制 / 布局 / 注册参数天然多，抽结构体只是把参数搬个家，留给需要时再拆
    pub fn define_persistent(
        &self,
        plugin_id: &str,
        name: &str,
        purpose: &str,
        factory: &str,
        contrib: Option<SlashEntry>,
        source: Option<SourceInput>,
        origin: PluginOrigin,
    ) -> Result<DefineReceipt, String> {
        self.define_at(
            PERSIST_SESSION,
            DefineTarget::Exact {
                plugin_id: plugin_id.to_string(),
                origin,
            },
            name,
            purpose,
            factory,
            contrib,
            source,
        )
    }

    fn install_disk(&self, spec: &DiskSpec) -> Result<DefineReceipt, String> {
        // 授权边界是这个插件自己的目录，不是全局 root——扫描用的 root 可能是
        // `boot_disk_from` 传进来的。
        let source = spec
            .source_path
            .as_ref()
            .map(|p| SourceInput::ResolvedPath {
                path: p.clone(),
                dir: Some(spec.path.clone()),
            });
        self.define_persistent(
            &spec.id,
            &spec.name,
            &spec.purpose,
            &spec.factory,
            spec.contrib.clone(),
            source,
            PluginOrigin::Disk {
                path: spec.path.clone(),
                scope: spec.scope,
            },
        )
    }

    pub async fn promote(
        &self,
        session_id: &str,
        plugin_id: &str,
        disk_id: Option<&str>,
        scope: PersistScope,
    ) -> Result<PromoteReceipt, String> {
        self.promote_into(session_id, plugin_id, disk_id, scope, persist_root(scope))
            .await
    }

    pub async fn promote_into(
        &self,
        session_id: &str,
        plugin_id: &str,
        disk_id: Option<&str>,
        scope: PersistScope,
        root: PathBuf,
    ) -> Result<PromoteReceipt, String> {
        let wanted = disk_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| default_disk_id(plugin_id));
        if !valid_disk_id(&wanted) {
            return Err(format!(
                "cordis_promote id {wanted:?} must be 2–32 chars, start with a-z, then a-z0-9-; pass `id` explicitly"
            ));
        }

        let pkg = {
            let inner = self.inner.lock().unwrap();
            let rec = super::owned(&inner.registry, session_id, plugin_id)?;
            rec.current_package_id
                .as_ref()
                .and_then(|id| rec.packages.get(id))
                .or_else(|| rec.packages.last().map(|(_, p)| p))
                .cloned()
                .ok_or_else(|| format!("plugin \"{plugin_id}\" has no Package to promote"))?
        };
        let path = write_plugin(&root, &wanted, &pkg, true)?;
        let origin = PluginOrigin::Disk {
            path: path.clone(),
            scope,
        };

        if plugin_id == wanted {
            {
                let mut inner = self.inner.lock().unwrap();
                if let Some(rec) = inner.registry.get_mut(plugin_id) {
                    rec.session_id = PERSIST_SESSION.into();
                    rec.origin = origin;
                }
            }
            let running = self
                .inspect_plugin(PERSIST_SESSION, plugin_id)
                .ok()
                .and_then(|r| r.active_run)
                .is_some();
            if !running {
                if let Err(e) = self
                    .run(PERSIST_SESSION, plugin_id, &pkg.package_id, RunMode::Run)
                    .await
                {
                    return Err(format!(
                        "wrote {} but autostart failed: {e}",
                        path.display()
                    ));
                }
            }
            return Ok(PromoteReceipt {
                plugin_id: wanted,
                package_id: pkg.package_id,
                path,
                scope,
                running: true,
                source_plugin_id: plugin_id.to_string(),
                stopped_source: false,
            });
        }

        let existing_session = {
            let inner = self.inner.lock().unwrap();
            inner.registry.get(&wanted).map(|r| r.session_id.clone())
        };
        let stopped_source = self
            .stop(session_id, plugin_id)
            .await
            .map(|r| r.was_running)
            .unwrap_or(false);
        let package_id = match existing_session.as_deref() {
            None => {
                let defined = self.define_persistent(
                    &wanted,
                    &pkg.name,
                    &pkg.purpose,
                    &pkg.factory,
                    pkg.contrib.clone(),
                    source_input_for_package(&pkg, None)?,
                    origin,
                )?;
                self.run(
                    PERSIST_SESSION,
                    &defined.plugin_id,
                    &defined.package_id,
                    RunMode::Run,
                )
                .await
                .map_err(|e| format!("wrote {} but autostart failed: {e}", path.display()))?;
                defined.package_id
            }
            Some(PERSIST_SESSION) => {
                let defined = self.define(
                    PERSIST_SESSION,
                    super::PluginSel::Existing {
                        plugin_id: wanted.clone(),
                    },
                    &pkg.name,
                    &pkg.purpose,
                    &pkg.factory,
                    pkg.contrib.clone(),
                    source_input_for_package(&pkg, None)?,
                )?;
                let mode = {
                    let inner = self.inner.lock().unwrap();
                    match inner
                        .registry
                        .get(&wanted)
                        .and_then(|r| r.current_package_id.as_deref())
                    {
                        Some(cur) if cur != defined.package_id => RunMode::Update,
                        _ => RunMode::Run,
                    }
                };
                self.run(PERSIST_SESSION, &wanted, &defined.package_id, mode)
                    .await
                    .map_err(|e| format!("wrote {} but reload failed: {e}", path.display()))?;
                defined.package_id
            }
            Some(_) => {
                return Err(format!(
                    "wrote {} but \"{wanted}\" is already a session Plugin; {PROMOTE_SESSION_COLLISION_HINT}",
                    path.display()
                ));
            }
        };

        Ok(PromoteReceipt {
            plugin_id: wanted,
            package_id,
            path,
            scope,
            running: true,
            source_plugin_id: plugin_id.to_string(),
            stopped_source,
        })
    }

    pub fn overlay_listing(&self, session_id: &str) -> String {
        self.overlay_listing_from(session_id, &plugin_roots())
    }

    pub fn overlay_listing_from(
        &self,
        session_id: &str,
        roots: &[(PersistScope, PathBuf)],
    ) -> String {
        let live = self.snapshot(session_id);
        let disk = scan(roots);
        let mut lines = vec![
            "永久插件写在磁盘上，重启后自动加载（启动不走权限 overlay）。".into(),
            "会话插件只在当前进程：cordis_define → cordis_run。写成永久：cordis_promote。".into(),
            String::new(),
            "永久（磁盘）：".into(),
        ];
        if disk.is_empty() {
            lines.push("  （还没有）".into());
            lines.push("  .dock/plugins/<id>/".into());
            lines.push("  ~/.dock/plugins/<id>/".into());
        } else {
            for spec in &disk {
                let row = live.iter().find(|r| r.plugin_id == spec.id);
                let state = match row.and_then(|r| r.active_run.as_ref()) {
                    Some(_) => "运行中",
                    None if !spec.enabled => "已禁用",
                    None => "已停止",
                };
                lines.push(format!(
                    "  {} [{}] {} — {} ({state})",
                    spec.id,
                    spec.scope.as_str(),
                    spec.name,
                    spec.purpose
                ));
                lines.push(format!("    {}", spec.path.display()));
            }
        }
        lines.push(String::new());
        lines.push("会话（仅内存）：".into());
        let session_rows: Vec<_> = live
            .iter()
            .filter(|r| !matches!(r.origin, PluginOrigin::Disk { .. }))
            .collect();
        if session_rows.is_empty() {
            lines.push("  （无）".into());
        } else {
            for row in session_rows {
                let state = if row.active_run.is_some() {
                    "运行中"
                } else {
                    "已停止"
                };
                let name = row
                    .packages
                    .last()
                    .map(|p| p.name.as_str())
                    .unwrap_or(row.plugin_id.as_str());
                lines.push(format!("  {} {name} ({state})", row.plugin_id));
            }
        }
        lines.push(String::new());
        lines.push(
            "host.on(\"session/event\", |line| { … }) 观察 user / assistant / tool 行。".into(),
        );
        lines.join("\n")
    }
}

/// 设置页「永久插件」的一行：磁盘上的一个插件目录。按**目录**认，不按 id——
/// 不同项目里可以有同名插件，注册表里同一时间只能装其中一个。
#[derive(Clone, Debug)]
pub struct DiskPluginView {
    pub id: String,
    pub name: String,
    pub purpose: String,
    pub scope: PersistScope,
    /// 插件根（`~/.dock/plugins` 或 `<项目>/.dock/plugins`）。
    pub root: PathBuf,
    pub path: PathBuf,
    /// `plugin.toml` 的 `enabled`。
    pub enabled: bool,
    /// 注册表里装的就是这个目录（项目还没开过会话时是 false）。
    pub loaded: bool,
    pub running: bool,
    /// 同名插件已经从别的目录装上了：这一份装不进来。
    pub shadowed_by: Option<PathBuf>,
    /// 读不起来或启动失败的原因。
    pub error: Option<String>,
}

/// 动态插件：agent 在会话里用 `cordis_define` 定义、只在内存里的插件。
#[derive(Clone, Debug)]
pub struct SessionPluginView {
    pub plugin_id: String,
    /// 定义它的会话（`Sessions::identity`，如 `main#3`）。
    pub session_id: String,
    pub name: String,
    pub purpose: String,
    pub factory: String,
    pub running: bool,
    pub error: Option<String>,
}

/// 两个路径是不是同一个目录：先比字面，不同再比规范化后的真实路径（符号链接、
/// macOS 的 `/var` ↔ `/private/var`）。界面传来的是规范化后的路径，注册表里记的
/// 是装载时的写法，只比字面会把同一个插件当成「别处的同名插件」。
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(x), Ok(y)) if x == y
        )
}

fn disk_path_of(rec: &super::registry::PluginRec) -> Option<&Path> {
    match &rec.origin {
        PluginOrigin::Disk { path, .. } => Some(path.as_path()),
        PluginOrigin::Session => None,
    }
}

fn failure(rec: &super::registry::PluginRec) -> Option<String> {
    rec.latest
        .as_ref()
        .filter(|a| a.status == super::AttemptStatus::Failed)
        .map(|a| a.host_error.clone().unwrap_or_else(|| "启动失败".into()))
}

impl DynamicRunner {
    /// `roots` 里每个根下的插件目录（含读不起来的），各自装没装、跑没跑。
    /// 不合并同名：每个根单独扫，界面按根分组。
    pub fn disk_plugins_in(&self, roots: &[(PersistScope, PathBuf)]) -> Vec<DiskPluginView> {
        let inner = self.inner.lock().unwrap();
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (scope, root) in roots {
            if !seen.insert(root.clone()) {
                continue;
            }
            let (specs, broken) = scan_report(&[(*scope, root.clone())]);
            for spec in specs {
                let rec = inner.registry.get(&spec.id);
                let here = rec
                    .and_then(disk_path_of)
                    .is_some_and(|p| same_dir(p, &spec.path));
                let shadowed_by = rec.filter(|_| !here).map(|r| {
                    disk_path_of(r)
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| PathBuf::from("（会话插件）"))
                });
                out.push(DiskPluginView {
                    id: spec.id,
                    name: spec.name,
                    purpose: spec.purpose,
                    scope: spec.scope,
                    root: root.clone(),
                    path: spec.path,
                    enabled: spec.enabled,
                    loaded: here,
                    running: here && rec.is_some_and(|r| r.run.is_some()),
                    shadowed_by,
                    error: if here { rec.and_then(failure) } else { None },
                });
            }
            out.extend(broken.into_iter().map(|b| DiskPluginView {
                name: b.id.clone(),
                id: b.id,
                purpose: String::new(),
                scope: b.scope,
                root: root.clone(),
                path: b.path,
                enabled: false,
                loaded: false,
                running: false,
                shadowed_by: None,
                error: Some(b.error),
            }));
        }
        out
    }

    /// 动态插件（会话里定义、没写成永久的）。
    pub fn session_plugins(&self) -> Vec<SessionPluginView> {
        let inner = self.inner.lock().unwrap();
        inner
            .registry
            .all()
            .filter(|r| matches!(r.origin, PluginOrigin::Session))
            .map(|r| {
                let pkg = r
                    .current_package_id
                    .as_ref()
                    .and_then(|id| r.packages.get(id))
                    .or_else(|| r.packages.values().last());
                SessionPluginView {
                    plugin_id: r.plugin_id.clone(),
                    session_id: r.session_id.clone(),
                    name: pkg
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| r.plugin_id.clone()),
                    purpose: pkg.map(|p| p.purpose.clone()).unwrap_or_default(),
                    factory: pkg.map(|p| p.factory.clone()).unwrap_or_default(),
                    running: r.run.is_some(),
                    error: failure(r),
                }
            })
            .collect()
    }

    /// 按目录找一个磁盘插件（`dir` 必须在某个插件根下）。
    fn disk_spec_at(dir: &Path) -> Result<DiskSpec, String> {
        let id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("不是插件目录")?;
        let root = dir.parent().ok_or("不是插件目录")?;
        let scope = if same_dir(root, &persist_root(PersistScope::User)) {
            PersistScope::User
        } else {
            PersistScope::Project
        };
        load_dir(dir, id, scope)
    }

    /// 启用 / 停用一个磁盘插件：写 `plugin.toml` 的 `enabled`，并立刻装上跑 / 停掉。
    pub async fn set_disk_enabled_at(&self, dir: &Path, enabled: bool) -> Result<(), String> {
        let spec = Self::disk_spec_at(dir)?;
        let loaded = {
            let inner = self.inner.lock().unwrap();
            match inner.registry.get(&spec.id) {
                Some(r) if disk_path_of(r).is_some_and(|p| same_dir(p, dir)) => Some((
                    r.run.is_some(),
                    r.current_package_id
                        .clone()
                        .or_else(|| r.packages.keys().last().cloned()),
                )),
                Some(r) => {
                    if enabled {
                        let other = disk_path_of(r)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "会话里定义的同名插件".into());
                        return Err(format!("已经装着同名插件 {}（{other}），先停用它", spec.id));
                    }
                    None
                }
                None => None,
            }
        };
        write_disk_enabled(&spec.path, enabled)?;
        if !enabled {
            if loaded.is_some() {
                self.stop_any(&spec.id).await?;
            }
            return Ok(());
        }
        let package_id = match loaded {
            Some((true, _)) => return Ok(()),
            Some((false, Some(pkg))) => pkg,
            _ => self.install_disk(&spec)?.package_id,
        };
        self.run(PERSIST_SESSION, &spec.id, &package_id, RunMode::Run)
            .await
            .map(|_| ())
    }

    /// 删一个磁盘插件：装着就先停、从注册表摘掉，再删目录。
    pub async fn delete_disk_at(&self, dir: &Path) -> Result<(), String> {
        let id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|id| valid_disk_id(id))
            .ok_or("不是插件目录")?
            .to_string();
        if !dir.join("plugin.toml").is_file() && !dir.is_dir() {
            return Err(format!("{} 不存在", dir.display()));
        }
        let ours = {
            let inner = self.inner.lock().unwrap();
            inner
                .registry
                .get(&id)
                .is_some_and(|r| disk_path_of(r).is_some_and(|p| same_dir(p, dir)))
        };
        if ours {
            self.stop_any(&id).await?;
            self.inner.lock().unwrap().registry.delete(&id);
        }
        std::fs::remove_dir_all(dir).map_err(|e| format!("删除 {} 失败：{e}", dir.display()))
    }

    /// 把一个动态插件写成永久插件，写进 `root`（用户级或某个项目的插件根）。
    pub async fn promote_session(
        &self,
        plugin_id: &str,
        scope: PersistScope,
        root: PathBuf,
    ) -> Result<PromoteReceipt, String> {
        let session = self.session_owner(plugin_id)?;
        self.promote_into(&session, plugin_id, None, scope, root)
            .await
    }

    /// 丢掉一个动态插件：停掉并从注册表删掉（它本来就只在内存里）。
    pub async fn discard_session(&self, plugin_id: &str) -> Result<(), String> {
        let session = self.session_owner(plugin_id)?;
        self.undefine(&session, plugin_id).await.map(|_| ())
    }

    fn session_owner(&self, plugin_id: &str) -> Result<String, String> {
        let inner = self.inner.lock().unwrap();
        let rec = inner
            .registry
            .get(plugin_id)
            .ok_or_else(|| format!("没有叫 {plugin_id} 的插件"))?;
        if !matches!(rec.origin, PluginOrigin::Session) {
            return Err(format!("{plugin_id} 已经是永久插件"));
        }
        Ok(rec.session_id.clone())
    }

    /// 打开某个项目的会话时装上它的永久插件（同名已装的跳过）。和 TUI 从这个目录
    /// 启动时一样自动跑，不走权限确认。
    pub async fn boot_project(&self, project: &Path) {
        self.boot_disk_from(&[(PersistScope::Project, project_root(project))])
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 桌面 GUI：进程 cwd 是 App 数据目录，每个会话钉着自己的项目目录。agent 在会话里
    /// `cordis_promote` 到项目级，要写进**这个会话的**项目，不是进程 cwd。
    #[tokio::test]
    async fn project_root_follows_the_running_session_not_the_process() {
        let project = tempfile::tempdir().unwrap();
        let ctx = cordis::Context::new();
        let sessions = crate::session::log::Sessions::tab(ctx.clone(), 2);
        sessions.pin_workspace_cwd(project.path());
        let _reg = ctx.provide(crate::names::SESSIONS, sessions).unwrap();
        let root = crate::tools::registry::with_exec_ctx_async(ctx, async {
            persist_root(PersistScope::Project)
        })
        .await;
        assert_eq!(root, project.path().join(".dock").join("plugins"));
        // 不在任何一轮里（启动时）照旧是进程 cwd：TUI 从哪启动就认哪的项目插件。
        assert_eq!(
            persist_root(PersistScope::Project),
            std::env::current_dir()
                .unwrap()
                .join(".dock")
                .join("plugins")
        );
    }

    #[test]
    fn default_disk_id_strips_counter() {
        assert_eq!(default_disk_id("echo-1"), "echo");
        assert_eq!(default_disk_id("memo-12"), "memo");
        assert_eq!(default_disk_id("session-memo"), "session-memo");
        assert_eq!(default_disk_id("hold-1"), "hold");
    }

    #[test]
    fn valid_disk_id_shape() {
        assert!(valid_disk_id("ab"));
        assert!(valid_disk_id("memo"));
        assert!(valid_disk_id("session-memo"));
        assert!(!valid_disk_id("a"));
        assert!(!valid_disk_id("Memo"));
        assert!(!valid_disk_id("1abc"));
        assert!(!valid_disk_id(""));
    }

    #[test]
    fn write_and_scan_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = Package {
            package_id: "pkg-1".into(),
            name: "便签".into(),
            purpose: "memo bag".into(),
            factory: RHAI_FACTORY.into(),
            provides: Vec::new(),
            inject: vec!["tools".into()],
            tools: Vec::new(),
            contrib: None,
            source: Some("#{ inject: [\"tools\"], apply: |host| { host.log(\"hi\") } }".into()),
            source_path: None,
        };
        let written = write_plugin(dir.path(), "memo", &pkg, true).unwrap();
        assert!(written.join("plugin.toml").exists());
        assert!(written.join("source.rhai").exists());
        let specs = scan(&[(PersistScope::Project, dir.path().to_path_buf())]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "memo");
        assert_eq!(specs[0].name, "便签");
        assert_eq!(specs[0].factory, RHAI_FACTORY);
        let src = std::fs::read_to_string(specs[0].source_path.as_ref().unwrap()).unwrap();
        assert!(src.contains("apply"));
    }

    #[test]
    fn project_overlays_user() {
        let user = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let pkg = |name: &str| Package {
            package_id: "pkg-1".into(),
            name: name.into(),
            purpose: "p".into(),
            factory: "echo".into(),
            provides: Vec::new(),
            inject: Vec::new(),
            tools: Vec::new(),
            contrib: None,
            source: None,
            source_path: None,
        };
        write_plugin(user.path(), "echo", &pkg("home"), true).unwrap();
        write_plugin(project.path(), "echo", &pkg("proj"), true).unwrap();
        let specs = scan(&[
            (PersistScope::User, user.path().to_path_buf()),
            (PersistScope::Project, project.path().to_path_buf()),
        ]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "proj");
        assert_eq!(specs[0].scope, PersistScope::Project);
    }

    #[test]
    fn resolve_source_path_under_root() {
        let root = tempfile::tempdir().unwrap();
        let plug = root.path().join("demo");
        std::fs::create_dir_all(&plug).unwrap();
        let file = plug.join("source.rhai");
        std::fs::write(&file, "#{ inject: [], apply: |host| { } }").unwrap();
        let roots = vec![(PersistScope::Project, root.path().to_path_buf())];
        let got = resolve_source_path_in(file.to_str().unwrap(), &roots).unwrap();
        assert_eq!(got, file.canonicalize().unwrap());
    }

    #[test]
    fn resolve_source_path_rejects_dotdot() {
        let roots = vec![(PersistScope::Project, PathBuf::from("/tmp"))];
        let err = resolve_source_path_in("../etc/passwd", &roots).unwrap_err();
        assert!(err.contains(".."), "{err}");
    }

    #[test]
    fn resolve_source_path_rejects_outside_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let roots = vec![(PersistScope::Project, root.path().to_path_buf())];
        let err = resolve_source_path_in(outside.path().to_str().unwrap(), &roots).unwrap_err();
        assert!(err.contains("must resolve under"), "{err}");
    }

    #[test]
    fn assert_under_plugin_root_rejects_symlink_swap_after_resolve() {
        let root = tempfile::tempdir().unwrap();
        let plug = root.path().join("demo");
        std::fs::create_dir_all(&plug).unwrap();
        let file = plug.join("source.rhai");
        std::fs::write(&file, "#{ inject: [], apply: |host| { } }").unwrap();
        let roots = vec![(PersistScope::Project, root.path().to_path_buf())];
        let canon = resolve_source_path_in(file.to_str().unwrap(), &roots).unwrap();
        assert_eq!(assert_under_plugin_root_in(&canon, &roots).unwrap(), canon);

        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), "TOP SECRET").unwrap();
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(outside.path(), &file).unwrap();

        let err = assert_under_plugin_root_in(&canon, &roots).unwrap_err();
        assert!(err.contains("must resolve under"), "{err}");
        assert!(!err.contains("TOP SECRET"), "{err}");
    }
}
