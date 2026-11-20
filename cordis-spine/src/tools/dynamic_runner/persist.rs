//! Disk-backed Cordis plugins. Directory name is the stable pluginId.
//!
//! `{project}/.dock/plugins/<id>/` overlays `~/.dock/plugins/<id>/`. 这是模型侧
//! **唯一**的插件生命周期（对齐 DSH 的 Plugin Manager）：写文件 →
//! `cordis_plugin reload` → 用新能力验证。每次 reload 把 `source.rhai` 读成一个
//! 不可变的新 Package；跑不起来就回到上一版继续跑，原因留给 `cordis_inspect`。

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::json;

use super::registry::{PersistScope, PluginOrigin, Surface};
use super::{AttemptStatus, DefineReceipt, DefineTarget, DynamicRunner, RunMode, SourceInput};
use crate::host::slash::{slash_entry_from_define, SlashEntry};
use crate::tools::dynamic_runner::factories::RHAI_FACTORY;
use crate::tools::dynamic_runner::rhai_host::{FILE_TOO_LARGE, MAX_FILE_SOURCE};
use cordis_base::config::dock_home;

/// Session id of autoloaded disk plugins. Visible to every chat session.
pub const PERSIST_SESSION: &str = "*";

/// `plugin.toml` 里认得的键。认不得的不报错（老插件可能写了别的），但 reload
/// 回执里会提一句——`[permission]` 少个 s 这种错不该静默吞掉。
const MANIFEST_KEYS: &[&str] = &[
    "name",
    "purpose",
    "factory",
    "enabled",
    "command",
    "kind",
    "text",
    "title",
    "send",
    "description",
    "permissions",
];

#[derive(Clone, Debug, Default, Deserialize)]
struct PluginManifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    factory: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    send: Option<bool>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    permissions: ManifestPermissions,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ManifestPermissions {
    /// 免逐次确认的 HTTP host：`api.example.com` 或 `*.example.com`。
    #[serde(default)]
    http: Vec<String>,
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
    /// `[permissions] http`，已规范成小写。
    pub http_allow: Vec<String>,
    /// 读 `plugin.toml` 时发现的小问题（认不得的键）。不挡加载。
    pub warnings: Vec<String>,
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

/// 用户级在前、项目级在后：扫描时后面的覆盖前面的（同 id 项目级胜出）。
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

/// `host` 在不在声明单里。`*.example.com` 只配子域（`a.example.com`），不配
/// `example.com` 本身——要两个都放就两行都写。
pub fn http_host_allowed(allow: &[String], host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    allow.iter().any(|entry| match entry.strip_prefix("*.") {
        Some(suffix) => host.len() > suffix.len() + 1 && host.ends_with(&format!(".{suffix}")),
        None => *entry == host,
    })
}

/// 一行声明合不合法：只能是主机名（可带一个 `*.` 前缀），不许 scheme、端口、路径，
/// 也不许光一个 `*`——那等于放开全网，批准框上也看不出来。
fn normalize_http_entry(raw: &str) -> Result<String, String> {
    let entry = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let bare = entry.strip_prefix("*.").unwrap_or(&entry);
    let ok = !bare.is_empty()
        && bare.contains('.')
        && bare
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && !bare.starts_with('.')
        && !bare.contains("..");
    if ok || entry == "localhost" {
        Ok(entry)
    } else {
        Err(format!(
            "plugin.toml [permissions] http 只能写主机名（如 api.example.com 或 *.example.com），不能写 {raw:?}"
        ))
    }
}

/// 源码里 `host.on("…")` 挂的钩子名（只认字面量）。给批准框和回执看的说明，不是
/// 安全边界——真正的门是「加载这个插件」本身要用户点头。
pub fn declared_hooks(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("host.on(") {
        rest = &rest[at + "host.on(".len()..];
        let trimmed = rest.trim_start();
        let Some(body) = trimmed.strip_prefix('"') else {
            continue;
        };
        let Some(end) = body.find('"') else {
            break;
        };
        let name = &body[..end];
        if !name.is_empty() && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// 读插件目录里的源文件：规范化后必须还在这个插件目录里（不许经软链读到别处），
/// 不超过 1MiB。
pub fn read_source_file(path: &Path, max_bytes: usize, dir: &Path) -> Result<String, String> {
    let canon = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if !canon.is_file() {
        return Err(format!("{} 不是文件", canon.display()));
    }
    let dir_canon = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    if !canon.starts_with(&dir_canon) {
        return Err(format!(
            "源文件要在插件目录 {} 里，实际指向 {}",
            dir_canon.display(),
            canon.display()
        ));
    }
    let meta = std::fs::metadata(&canon).map_err(|e| format!("read {}: {e}", canon.display()))?;
    if meta.len() as usize > max_bytes {
        return Err(FILE_TOO_LARGE.into());
    }
    let text =
        std::fs::read_to_string(&canon).map_err(|e| format!("read {}: {e}", canon.display()))?;
    if text.len() > max_bytes {
        return Err(FILE_TOO_LARGE.into());
    }
    Ok(text)
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
            // 既没有 plugin.toml 也没有 source.rhai 的目录不是插件（可能是别的东西）。
            if !path.join("plugin.toml").is_file() && !path.join("source.rhai").is_file() {
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

/// 按 id 找插件目录：给了 `scope` 只看那一层，否则项目级优先（和扫描时的覆盖顺序
/// 一致）。返回找到的那份，以及被它盖住的另一份（如果有）。
pub fn find_disk(
    id: &str,
    scope: Option<PersistScope>,
    roots: &[(PersistScope, PathBuf)],
) -> Result<(DiskSpec, Option<PathBuf>), String> {
    let id = id.trim();
    if !valid_disk_id(id) {
        return Err(format!(
            "插件 id {id:?} 不合法：2–32 位，小写字母开头，只含小写字母、数字和 -（它就是 .dock/plugins/ 下的目录名）"
        ));
    }
    let mut ordered: Vec<&(PersistScope, PathBuf)> = roots
        .iter()
        .filter(|(s, _)| scope.is_none_or(|want| want == *s))
        .collect();
    // 项目级优先。
    ordered.sort_by_key(|(s, _)| match s {
        PersistScope::Project => 0,
        PersistScope::User => 1,
    });
    let mut found: Vec<(PersistScope, PathBuf)> = Vec::new();
    for (s, root) in ordered {
        let dir = root.join(id);
        if dir.is_dir() && !found.iter().any(|(_, d)| same_dir(d, &dir)) {
            found.push((*s, dir));
        }
    }
    let Some((s, dir)) = found.first().cloned() else {
        let places: Vec<String> = roots
            .iter()
            .filter(|(s, _)| scope.is_none_or(|want| want == *s))
            .map(|(s, root)| format!("{} {}", s.as_str(), root.join(id).display()))
            .collect();
        return Err(format!(
            "没找到插件目录 {id}。先把 source.rhai（和可选的 plugin.toml）写进其中一处：{}",
            places.join("；")
        ));
    };
    let spec = load_dir(&dir, id, s).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok((spec, found.get(1).map(|(_, d)| d.clone())))
}

/// 改磁盘插件 `plugin.toml` 的 `enabled`（就地改，其余键不动）。没有 `plugin.toml`
/// 的插件（只有 `source.rhai`）就建一个只写这一行的。
pub fn write_disk_enabled(dir: &Path, enabled: bool) -> Result<(), String> {
    let path = dir.join("plugin.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read plugin.toml: {e}")),
    };
    let mut doc = raw
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("plugin.toml: {e}"))?;
    doc["enabled"] = toml_edit::value(enabled);
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("write plugin.toml: {e}"))
}

/// 读一个插件目录。`plugin.toml` 可以没有（只有 `source.rhai` 就是一个 Rhai 插件，
/// 名字取目录名）；有的话每个键都可选。
fn load_dir(dir: &Path, id: &str, scope: PersistScope) -> Result<DiskSpec, String> {
    let manifest_path = dir.join("plugin.toml");
    let mut warnings = Vec::new();
    let man: PluginManifest = match std::fs::read_to_string(&manifest_path) {
        Ok(raw) => {
            let table: toml::Table =
                toml::from_str(&raw).map_err(|e| format!("plugin.toml: {e}"))?;
            let unknown: Vec<&str> = table
                .keys()
                .map(String::as_str)
                .filter(|k| !MANIFEST_KEYS.contains(k))
                .collect();
            if !unknown.is_empty() {
                warnings.push(format!(
                    "plugin.toml 里有认不得的键：{}（认得的：{}）",
                    unknown.join("、"),
                    MANIFEST_KEYS.join("、")
                ));
            }
            toml::from_str(&raw).map_err(|e| format!("plugin.toml: {e}"))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => PluginManifest::default(),
        Err(e) => return Err(format!("read plugin.toml: {e}")),
    };
    let factory = man
        .factory
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .unwrap_or(RHAI_FACTORY)
        .to_string();
    let source_path = if factory == RHAI_FACTORY {
        let p = dir.join("source.rhai");
        if !p.is_file() {
            return Err("Rhai 插件要有 source.rhai".into());
        }
        Some(p)
    } else {
        None
    };
    let name = man
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(id)
        .to_string();
    let purpose = man.purpose.as_deref().unwrap_or("").trim().to_string();
    let contrib = if factory == "slash" {
        let v = json!({
            "command": man.command,
            "kind": man.kind,
            "text": man.text,
            "title": man.title,
            "send": man.send,
            "description": man.description,
        });
        Some(slash_entry_from_define(&v, &purpose)?)
    } else if man.command.is_some() || man.kind.is_some() || man.text.is_some() {
        return Err("command/kind/text are only for factory \"slash\"".into());
    } else {
        None
    };
    let http_allow = man
        .permissions
        .http
        .iter()
        .map(|raw| normalize_http_entry(raw))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DiskSpec {
        id: id.to_string(),
        path: dir.to_path_buf(),
        scope,
        name,
        purpose,
        factory,
        enabled: man.enabled.unwrap_or(true),
        source_path,
        contrib,
        http_allow,
        warnings,
    })
}

/// 把插件目录读成一份源码快照（Rhai 插件），给 define 用。
fn snapshot(spec: &DiskSpec) -> Result<Option<SourceInput>, String> {
    let Some(path) = spec.source_path.as_ref() else {
        return Ok(None);
    };
    Ok(Some(SourceInput::Snapshot {
        text: read_source_file(path, MAX_FILE_SOURCE, &spec.path)?,
        dir: spec.path.clone(),
        http_allow: spec.http_allow.clone(),
    }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReloadStatus {
    Running,
    /// 起来了，但 `inject` 的名字还没挂上。
    Waiting,
    Failed,
}

impl ReloadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Failed => "failed",
        }
    }
}

/// `cordis_plugin reload` 的结果。
#[derive(Clone, Debug)]
pub struct ReloadReceipt {
    pub id: String,
    pub dir: PathBuf,
    pub scope: PersistScope,
    pub status: ReloadStatus,
    /// 跑不起来的原因（编译错带行列号）。
    pub error: Option<String>,
    /// 新版没起来、自动回到并仍在跑的上一版（Package id）。
    pub kept_previous: Option<String>,
    /// 这一版（或回滚后的那一版）实际登记的东西。
    pub surface: Surface,
    pub waiting_for: Vec<String>,
    pub warnings: Vec<String>,
    /// `plugin.toml` 原来写着 `enabled = false`，这次改成了 true。
    pub enabled_now: bool,
    pub http_allow: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct DisableReceipt {
    pub id: String,
    pub dir: PathBuf,
    pub was_running: bool,
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
        self.define_persistent(
            &spec.id,
            &spec.name,
            &spec.purpose,
            &spec.factory,
            spec.contrib.clone(),
            snapshot(spec)?,
            PluginOrigin::Disk {
                path: spec.path.clone(),
                scope: spec.scope,
            },
        )
    }

    /// 从插件目录装上 / 热换一个插件（`cordis_plugin reload`、设置页启用）。
    ///
    /// - 第一次：读目录、编译、跑。
    /// - 已经装着（同一个目录）：读成新 Package 换上去；新版跑不起来就回到上一版
    ///   继续跑——改坏一行不该让用户的工具一起消失。
    /// - `plugin.toml` 写着 `enabled = false` 的，顺手改成 true（reload 就是启用）。
    ///
    /// 只有「同名插件已经从别的目录装上了」这种没法开始的情况才回 `Err`；编译错、
    /// apply 抛错都在回执里（`status: failed`）。
    pub async fn reload_disk(&self, spec: &DiskSpec) -> Result<ReloadReceipt, String> {
        let (exists, prev_running, prev_pkg) = {
            let inner = self.inner.lock().unwrap();
            match inner.registry.get(&spec.id) {
                None => (false, false, None),
                Some(r) if disk_path_of(r).is_some_and(|p| same_dir(p, &spec.path)) => {
                    (true, r.run.is_some(), r.current_package_id.clone())
                }
                Some(r) => {
                    let other = disk_path_of(r)
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "程序侧定义的同名插件".into());
                    return Err(format!(
                        "已经装着同名插件 {}（{other}）。先 cordis_plugin disable 它（scope 指到那一层），或者给这个目录换个名字",
                        spec.id
                    ));
                }
            }
        };
        let enabled_now = !spec.enabled;
        if enabled_now {
            write_disk_enabled(&spec.path, true)?;
        }
        let mut receipt = ReloadReceipt {
            id: spec.id.clone(),
            dir: spec.path.clone(),
            scope: spec.scope,
            status: ReloadStatus::Failed,
            error: None,
            kept_previous: None,
            surface: Surface::default(),
            waiting_for: Vec::new(),
            warnings: spec.warnings.clone(),
            enabled_now,
            http_allow: spec.http_allow.clone(),
        };
        let defined = snapshot(spec).and_then(|source| {
            if exists {
                self.define(
                    PERSIST_SESSION,
                    super::PluginSel::Existing {
                        plugin_id: spec.id.clone(),
                    },
                    &spec.name,
                    &spec.purpose,
                    &spec.factory,
                    spec.contrib.clone(),
                    source,
                )
            } else {
                self.install_disk(spec)
            }
        });
        let outcome = match defined {
            // 编译不过：什么都没动，上一版（如果在跑）照跑。
            Err(e) => Err(e),
            Ok(defined) => {
                let mode = if prev_pkg.is_some() {
                    RunMode::Update
                } else {
                    RunMode::Run
                };
                match self
                    .run(PERSIST_SESSION, &spec.id, &defined.package_id, mode)
                    .await
                {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        // `update` 起新版之前已经把旧版的 fiber 拆了；current 指针
                        // 只在成功时才动，所以它还指着上一版，照原样再起一次。
                        if prev_running {
                            if let Some(prev) = prev_pkg.as_deref() {
                                if self
                                    .run(PERSIST_SESSION, &spec.id, prev, RunMode::Run)
                                    .await
                                    .is_ok()
                                {
                                    receipt.kept_previous = Some(prev.to_string());
                                }
                            }
                        }
                        Err(e)
                    }
                }
            }
        };
        if outcome.is_err() && prev_running && receipt.kept_previous.is_none() {
            // 编译期就失败的：上一版根本没被动过。
            let still = self
                .inspect_plugin(PERSIST_SESSION, &spec.id)
                .ok()
                .and_then(|r| r.active_run.map(|run| run.package_id));
            receipt.kept_previous = still;
        }
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(rec) = inner.registry.get_mut(&spec.id) {
                rec.reload_error = outcome.as_ref().err().cloned();
            }
        }
        if let Ok(row) = self.inspect_plugin(PERSIST_SESSION, &spec.id) {
            if let Some(run) = row.active_run {
                receipt.surface = run.surface;
                receipt.waiting_for = run.waiting_for;
            }
        }
        receipt.status = match &outcome {
            Err(e) => {
                receipt.error = Some(e.clone());
                ReloadStatus::Failed
            }
            Ok(()) if receipt.waiting_for.is_empty() => ReloadStatus::Running,
            Ok(()) => ReloadStatus::Waiting,
        };
        Ok(receipt)
    }

    /// 停用一个磁盘插件：`plugin.toml` 写 `enabled = false`（下次启动不装），装着
    /// 的话立刻停掉。文件都留着。
    pub async fn disable_disk(&self, spec: &DiskSpec) -> Result<DisableReceipt, String> {
        write_disk_enabled(&spec.path, false)?;
        let ours = {
            let inner = self.inner.lock().unwrap();
            inner
                .registry
                .get(&spec.id)
                .is_some_and(|r| disk_path_of(r).is_some_and(|p| same_dir(p, &spec.path)))
        };
        let was_running = if ours {
            self.stop_any(&spec.id).await?.was_running
        } else {
            false
        };
        if ours {
            let mut inner = self.inner.lock().unwrap();
            if let Some(rec) = inner.registry.get_mut(&spec.id) {
                if let Some(latest) = rec.latest.as_mut() {
                    latest.status = AttemptStatus::Stopped;
                }
            }
        }
        Ok(DisableReceipt {
            id: spec.id.clone(),
            dir: spec.path.clone(),
            was_running,
        })
    }

    pub fn overlay_listing(&self, session_id: &str) -> String {
        self.overlay_listing_from(session_id, &plugin_roots())
    }

    pub fn overlay_listing_from(
        &self,
        _session_id: &str,
        roots: &[(PersistScope, PathBuf)],
    ) -> String {
        let views = self.disk_plugins_in(roots);
        let mut lines = vec![
            "插件就是一个目录：.dock/plugins/<id>/（项目级，覆盖同名的用户级）或 ~/.dock/plugins/<id>/。".into(),
            "启动时自动装上；改了文件让 agent 用 cordis_plugin reload 重载，停用写 enabled = false。".into(),
            String::new(),
        ];
        if views.is_empty() {
            lines.push("（还没有插件）".into());
            for (scope, root) in roots {
                lines.push(format!("  {} {}", scope.as_str(), root.display()));
            }
        } else {
            for v in &views {
                let state = if v.running {
                    "运行中"
                } else if v.error.is_some() {
                    "出错"
                } else if !v.enabled {
                    "已停用"
                } else if v.shadowed_by.is_some() {
                    "被同名插件盖住"
                } else {
                    "未装载"
                };
                let purpose = if v.purpose.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", v.purpose)
                };
                lines.push(format!(
                    "  {} [{}] {}{purpose} ({state})",
                    v.id,
                    v.scope.as_str(),
                    v.name
                ));
                lines.push(format!("    {}", v.path.display()));
                if let Some(err) = &v.error {
                    lines.push(format!("    错误：{}", first_line(err)));
                }
            }
        }
        lines.join("\n")
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
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
    /// 读不起来、启动失败、或最近一次 reload 失败的原因。
    pub error: Option<String>,
}

/// 两个路径是不是同一个目录：先比字面，不同再比规范化后的真实路径（符号链接、
/// macOS 的 `/var` ↔ `/private/var`）。界面传来的是规范化后的路径，注册表里记的
/// 是装载时的写法，只比字面会把同一个插件当成「别处的同名插件」。
pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(x), Ok(y)) if x == y
        )
}

pub(crate) fn disk_path_of(rec: &super::registry::PluginRec) -> Option<&Path> {
    match &rec.origin {
        PluginOrigin::Disk { path, .. } => Some(path.as_path()),
        PluginOrigin::Session => None,
    }
}

fn failure(rec: &super::registry::PluginRec) -> Option<String> {
    rec.reload_error.clone().or_else(|| {
        rec.latest
            .as_ref()
            .filter(|a| a.status == AttemptStatus::Failed)
            .map(|a| a.host_error.clone().unwrap_or_else(|| "启动失败".into()))
    })
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
                        .unwrap_or_else(|| PathBuf::from("（程序侧定义的插件）"))
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

    /// 设置页启用 / 停用一个磁盘插件：启用 = reload（读最新的文件跑起来），
    /// 停用 = 写 `enabled = false` 并停掉。
    pub async fn set_disk_enabled_at(&self, dir: &Path, enabled: bool) -> Result<(), String> {
        let spec = Self::disk_spec_at(dir)?;
        if !enabled {
            return self.disable_disk(&spec).await.map(|_| ());
        }
        let receipt = self.reload_disk(&spec).await?;
        match (receipt.error, receipt.kept_previous) {
            (Some(e), Some(prev)) => Err(format!("{e}\n（新版没跑起来，仍在运行上一版 {prev}）")),
            (Some(e), None) => Err(e),
            (None, _) => Ok(()),
        }
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

    /// 桌面 GUI：进程 cwd 是 App 数据目录，每个会话钉着自己的项目目录。项目级插件根
    /// 要跟着**这个会话的**项目走，不是进程 cwd。
    #[tokio::test]
    async fn project_root_follows_the_running_session_not_the_process() {
        // 断言里读进程 cwd：拿住进程环境锁，别的用例切 cwd 时不会读到两个不同的目录。
        let _env = cordis_base::test_env::scoped();
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
    fn valid_disk_id_shape() {
        assert!(valid_disk_id("ab"));
        assert!(valid_disk_id("memo"));
        assert!(valid_disk_id("session-memo"));
        assert!(valid_disk_id("cyberspace"));
        assert!(!valid_disk_id("a"));
        assert!(!valid_disk_id("Memo"));
        assert!(!valid_disk_id("1abc"));
        assert!(!valid_disk_id(""));
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_source_file_alone_is_a_plugin_named_after_its_directory() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "memo/source.rhai",
            "#{ inject: [], apply: |host| {} }",
        );
        let specs = scan(&[(PersistScope::Project, root.path().to_path_buf())]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "memo");
        assert_eq!(specs[0].name, "memo");
        assert_eq!(specs[0].purpose, "");
        assert_eq!(specs[0].factory, RHAI_FACTORY);
        assert!(specs[0].enabled);
        assert!(specs[0].warnings.is_empty());
    }

    #[test]
    fn manifest_keys_are_optional_and_unknown_ones_are_reported() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "api/source.rhai",
            "#{ inject: [], apply: |host| {} }",
        );
        write(
            root.path(),
            "api/plugin.toml",
            "name = \"接口\"\n[permission]\nhttp = [\"api.example.com\"]\n",
        );
        let (spec, _) = find_disk(
            "api",
            None,
            &[(PersistScope::Project, root.path().to_path_buf())],
        )
        .unwrap();
        assert_eq!(spec.name, "接口");
        assert!(spec.http_allow.is_empty(), "拼错的段不该生效");
        assert!(
            spec.warnings[0].contains("permission"),
            "{:?}",
            spec.warnings
        );
    }

    #[test]
    fn declared_http_hosts_are_normalized_and_matched() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "api/source.rhai",
            "#{ inject: [], apply: |host| {} }",
        );
        write(
            root.path(),
            "api/plugin.toml",
            "[permissions]\nhttp = [\"API.Example.com\", \"*.qianxin.com\"]\n",
        );
        let (spec, _) = find_disk(
            "api",
            None,
            &[(PersistScope::Project, root.path().to_path_buf())],
        )
        .unwrap();
        assert_eq!(spec.http_allow, vec!["api.example.com", "*.qianxin.com"]);
        assert!(http_host_allowed(&spec.http_allow, "api.example.com"));
        assert!(http_host_allowed(&spec.http_allow, "hunter.qianxin.com"));
        assert!(!http_host_allowed(&spec.http_allow, "qianxin.com"));
        assert!(!http_host_allowed(&spec.http_allow, "evil-api.example.com"));
        assert!(!http_host_allowed(&spec.http_allow, "example.com"));
    }

    #[test]
    fn a_bare_wildcard_or_a_url_is_not_a_host() {
        for bad in ["*", "https://api.example.com", "api.example.com/v1", "a:80"] {
            assert!(normalize_http_entry(bad).is_err(), "{bad}");
        }
        assert_eq!(normalize_http_entry("localhost").unwrap(), "localhost");
    }

    #[test]
    fn declared_hooks_reads_string_literals() {
        let src = r#"#{ apply: |host| {
            host.on("tools/pre-execute", |c| { () });
            host.on( "agent/turn-end", |t| { () });
            host.on("tools/pre-execute", |c| { () });
            host.on(name, |x| { () });
        } }"#;
        assert_eq!(
            declared_hooks(src),
            vec!["tools/pre-execute", "agent/turn-end"]
        );
    }

    #[test]
    fn project_overlays_user() {
        let user = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        write(
            user.path(),
            "echo/plugin.toml",
            "name = \"home\"\nfactory = \"echo\"\n",
        );
        write(
            project.path(),
            "echo/plugin.toml",
            "name = \"proj\"\nfactory = \"echo\"\n",
        );
        let roots = [
            (PersistScope::User, user.path().to_path_buf()),
            (PersistScope::Project, project.path().to_path_buf()),
        ];
        let specs = scan(&roots);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "proj");
        assert_eq!(specs[0].scope, PersistScope::Project);
        // 按 id 找也是项目级优先，并报出被盖住的那份；给了 scope 就只看那一层。
        let (found, shadowed) = find_disk("echo", None, &roots).unwrap();
        assert_eq!(found.name, "proj");
        assert_eq!(shadowed.unwrap(), user.path().join("echo"));
        let (found, shadowed) = find_disk("echo", Some(PersistScope::User), &roots).unwrap();
        assert_eq!(found.name, "home");
        assert!(shadowed.is_none());
    }

    #[test]
    fn a_missing_directory_says_where_to_write_it() {
        let root = tempfile::tempdir().unwrap();
        let err = find_disk(
            "nope",
            None,
            &[(PersistScope::Project, root.path().to_path_buf())],
        )
        .unwrap_err();
        assert!(err.contains("source.rhai"), "{err}");
        assert!(
            err.contains(&root.path().join("nope").display().to_string()),
            "{err}"
        );
        let err = find_disk("Bad_Id", None, &[]).unwrap_err();
        assert!(err.contains("不合法"), "{err}");
    }

    #[test]
    fn write_disk_enabled_creates_a_manifest_when_there_is_none() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "memo/source.rhai",
            "#{ inject: [], apply: |host| {} }",
        );
        let dir = root.path().join("memo");
        write_disk_enabled(&dir, false).unwrap();
        let (spec, _) = find_disk(
            "memo",
            None,
            &[(PersistScope::Project, root.path().to_path_buf())],
        )
        .unwrap();
        assert!(!spec.enabled);
    }

    /// 源文件换成指向插件目录外的软链：读快照时要拒，也不能把外面的内容带出来。
    #[test]
    fn read_source_file_refuses_a_symlink_out_of_the_plugin_directory() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "demo/source.rhai",
            "#{ inject: [], apply: |host| {} }",
        );
        let dir = root.path().join("demo");
        let file = dir.join("source.rhai");
        assert!(read_source_file(&file, MAX_FILE_SOURCE, &dir).is_ok());

        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), "TOP SECRET").unwrap();
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(outside.path(), &file).unwrap();
        let err = read_source_file(&file, MAX_FILE_SOURCE, &dir).unwrap_err();
        assert!(err.contains("插件目录"), "{err}");
        assert!(!err.contains("TOP SECRET"), "{err}");
    }
}
