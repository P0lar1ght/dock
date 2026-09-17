//! Disk-backed Cordis plugins. Directory name is the stable pluginId.
//!
//! `{project}/.dock/plugins/<id>/` overlays `~/.dock/plugins/<id>/`. DSH does
//! not persist dynamic packages; this is Dock's Host-only stand-in for "write
//! a real plugin" when the user cannot ship a cordis-rust crate.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::registry::{Package, PersistScope, PluginOrigin};
use super::{DefineReceipt, DefineTarget, DynamicRunner, RunMode};
use crate::dynamic_runner::factories::RHAI_FACTORY;
use crate::slash::{slash_entry_from_define, SlashEntry};
use cordis_base::config::dock_home;

/// Session id of autoloaded disk plugins. Visible to every chat session.
pub const PERSIST_SESSION: &str = "*";

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
    pub source: Option<String>,
    pub contrib: Option<SlashEntry>,
}

pub fn persist_root(scope: PersistScope) -> PathBuf {
    match scope {
        PersistScope::User => dock_home().join("plugins"),
        PersistScope::Project => std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".dock")
            .join("plugins"),
    }
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

pub fn default_disk_id(plugin_id: &str) -> String {
    if let Some((head, tail)) = plugin_id.rsplit_once('-') {
        if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) && valid_disk_id(head) {
            return head.to_string();
        }
    }
    plugin_id.to_string()
}

pub fn scan(roots: &[(PersistScope, PathBuf)]) -> Vec<DiskSpec> {
    let mut by_id: IndexMap<String, DiskSpec> = IndexMap::new();
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
                Err(e) => eprintln!("[cordis] skip disk plugin {id}: {e}"),
            }
        }
    }
    by_id.into_values().collect()
}

fn load_dir(dir: &Path, id: &str, scope: PersistScope) -> Result<DiskSpec, String> {
    let raw = std::fs::read_to_string(dir.join("plugin.toml"))
        .map_err(|e| format!("read plugin.toml: {e}"))?;
    let man: PluginManifest = toml::from_str(&raw).map_err(|e| format!("plugin.toml: {e}"))?;
    if man.name.trim().is_empty() || man.purpose.trim().is_empty() {
        return Err("plugin.toml needs name and purpose".into());
    }
    let source = if man.factory == RHAI_FACTORY {
        Some(
            std::fs::read_to_string(dir.join("source.rhai"))
                .map_err(|e| format!("read source.rhai: {e}"))?,
        )
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
        source,
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
        let src = pkg
            .source
            .as_deref()
            .ok_or("rhai package has no source to promote")?;
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
        source: Option<String>,
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
        self.define_persistent(
            &spec.id,
            &spec.name,
            &spec.purpose,
            &spec.factory,
            spec.contrib.clone(),
            spec.source.clone(),
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
                    pkg.source.clone(),
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
                    pkg.source.clone(),
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
                    "wrote {} but \"{wanted}\" is already a session Plugin; undefine it or pass a different `id`",
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

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let written = write_plugin(dir.path(), "memo", &pkg, true).unwrap();
        assert!(written.join("plugin.toml").exists());
        assert!(written.join("source.rhai").exists());
        let specs = scan(&[(PersistScope::Project, dir.path().to_path_buf())]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "memo");
        assert_eq!(specs[0].name, "便签");
        assert_eq!(specs[0].factory, RHAI_FACTORY);
        assert!(specs[0].source.as_deref().unwrap().contains("apply"));
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
}
