//! LSP server configuration from `~/.dock/lsp.json` and `<cwd>/.dock/lsp.json`.

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_STARTUP_TIMEOUT_MS: u64 = 15_000;
pub const DEFAULT_SHUTDOWN_TIMEOUT_MS: u64 = 5_000;
pub const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Load LSP servers from user/project config, merge plugin-provided configs, and
/// return the [`ConfigSource`](crate::lsp::config_source::ConfigSource) of each.
///
/// Plugin configs fill gaps (new server names) but never override user/project config.
/// This is the canonical merge function — both session startup and `grok inspect` call it.
/// Accepts both file-based `.lsp.json` paths and inline `lspServers` JSON values
/// from plugin manifests (`plugin.json`).
pub fn load_servers_with_plugins_sourced(
    cwd: &Path,
    plugin_lsp_paths: &[PathBuf],
    plugin_inline_lsp: &[&serde_json::Value],
    plugin_names: &[&str],
    inline_plugin_names: &[&str],
) -> BTreeMap<String, (LspServerConfig, crate::lsp::config_source::ConfigSource)> {
    use crate::lsp::config_source::ConfigSource;

    debug_assert!(
        plugin_names.is_empty() || plugin_names.len() == plugin_lsp_paths.len(),
        "plugin_names must be empty or parallel to plugin_lsp_paths"
    );

    let user_path = crate::config::dock_home().join("lsp.json");
    let project_path = cwd.join(".dock").join("lsp.json");

    // User-level servers
    let mut servers: BTreeMap<String, (LspServerConfig, ConfigSource)> = load_file(&user_path)
        .into_iter()
        .map(|(name, cfg)| {
            (
                name,
                (
                    cfg,
                    ConfigSource::User {
                        path: user_path.clone(),
                    },
                ),
            )
        })
        .collect();

    // Project-level overrides
    for (name, cfg) in load_file(&project_path) {
        servers.insert(
            name,
            (
                cfg,
                ConfigSource::Project {
                    path: project_path.clone(),
                },
            ),
        );
    }

    // Plugin file-based configs
    for (i, lsp_path) in plugin_lsp_paths.iter().enumerate() {
        let pname = plugin_names.get(i).copied().unwrap_or("unknown");
        for (name, cfg) in load_file(lsp_path) {
            servers.entry(name).or_insert_with(|| {
                (
                    cfg,
                    ConfigSource::Plugin {
                        plugin_name: pname.to_string(),
                        path: lsp_path.clone(),
                    },
                )
            });
        }
    }

    // Plugin inline configs
    for (i, inline) in plugin_inline_lsp.iter().enumerate() {
        let pname = inline_plugin_names.get(i).copied().unwrap_or("unknown");
        match serde_json::from_value::<BTreeMap<String, LspServerConfig>>((*inline).clone()) {
            Ok(parsed) => {
                for (name, cfg) in parsed {
                    servers.entry(name).or_insert_with(|| {
                        (
                            cfg,
                            ConfigSource::Plugin {
                                plugin_name: pname.to_string(),
                                path: PathBuf::new(),
                            },
                        )
                    });
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse inline lspServers from plugin manifest");
            }
        }
    }

    servers
}

/// Drop repo-local (project-scoped) LSP servers from a sourced map when the
/// workspace is untrusted; keep user/plugin. Warns per drop. The trust verdict is
/// passed in (the folder-trust engine lives in the shell, out of this crate).
///
/// Single source of truth for the folder-trust LSP load gate, shared by the
/// workspace build path and the shell's per-session gate.
pub fn filter_project_lsp_when_untrusted(
    sourced: BTreeMap<String, (LspServerConfig, crate::lsp::config_source::ConfigSource)>,
    project_trusted: bool,
) -> BTreeMap<String, LspServerConfig> {
    use crate::lsp::config_source::ConfigSource;
    sourced
        .into_iter()
        .filter_map(|(name, (cfg, source))| {
            if !project_trusted && matches!(source, ConfigSource::Project { .. }) {
                tracing::warn!(
                    server = %name,
                    "folder untrusted: skipping repo-local (project-scoped) LSP server"
                );
                None
            } else {
                Some((name, cfg))
            }
        })
        .collect()
}

/// Load LSP server configs from `~/.dock/lsp.json` and `<cwd>/.dock/lsp.json`.
/// Project config overrides user config for the same server name.
pub fn load_servers(cwd: &Path) -> BTreeMap<String, LspServerConfig> {
    let user_path = crate::config::dock_home().join("lsp.json");
    let project_path = cwd.join(".dock").join("lsp.json");

    let mut merged = load_file(&user_path);
    let project = load_file(&project_path);

    if !merged.is_empty() {
        tracing::info!(
            source = "user",
            path = %user_path.display(),
            servers = ?merged.keys().collect::<Vec<_>>(),
            "loaded user lsp.json"
        );
    }
    if !project.is_empty() {
        tracing::info!(
            source = "project",
            path = %project_path.display(),
            servers = ?project.keys().collect::<Vec<_>>(),
            "loaded project lsp.json"
        );
    }

    for (key, val) in project {
        merged.insert(key, val);
    }

    let mut ext_owners: HashMap<&str, &str> = HashMap::new();
    for (server_name, server_cfg) in &merged {
        for ext in server_cfg.extensions.keys() {
            if let Some(prev) = ext_owners.insert(ext.as_str(), server_name.as_str()) {
                tracing::warn!(
                    extension = ext,
                    server_a = prev,
                    server_b = server_name,
                    "extension claimed by multiple LSP servers; \
                     '{prev}' will handle it (first alphabetically)"
                );
            }
        }
    }

    if merged.is_empty() {
        tracing::info!(
            user = %user_path.display(),
            project = %project_path.display(),
            "no LSP servers configured"
        );
    }
    merged
}

/// User/project `lsp.json`, then fill gaps with language servers found on PATH
/// for the current workspace (Cargo.toml → rust-analyzer, etc.).
///
/// Called at first LSP start, not plugin mount, so `/cd` before the first
/// `lsp` call still sees the right root.
pub fn load_servers_or_defaults(cwd: &Path) -> BTreeMap<String, LspServerConfig> {
    let mut merged = load_servers(cwd);
    for (name, cfg) in detect_defaults(cwd) {
        merged.entry(name).or_insert(cfg);
    }
    merged
}

/// File-relative or absolute path → absolute `file://` URI.
/// Canonicalized so macOS `/var` vs `/private/var` matches `workspace_root`.
pub fn resolve_tool_path(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p.canonicalize().unwrap_or(p)
    } else {
        let joined = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p);
        joined.canonicalize().unwrap_or(joined)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LspJsonFile {
    Map(BTreeMap<String, LspServerConfig>),
    Wrapped {
        #[serde(rename = "lspServers")]
        lsp_servers: BTreeMap<String, LspServerConfig>,
    },
}

/// Load LSP server configs from a JSON file. Returns empty map on missing/invalid file.
///
/// Accepts a flat `{ "rust-analyzer": { "command": ... } }` map or the plugin
/// wrapper `{ "lspServers": { ... } }`.
pub fn load_file(path: &Path) -> BTreeMap<String, LspServerConfig> {
    let s = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BTreeMap::new(),
        Err(e) => {
            tracing::warn!(error = %e,"failed to read lsp.json");
            return BTreeMap::new();
        }
    };

    match serde_json::from_str::<LspJsonFile>(&s) {
        Ok(LspJsonFile::Map(m)) => m,
        Ok(LspJsonFile::Wrapped { lsp_servers }) => lsp_servers,
        Err(e) => {
            tracing::warn!(?e, "failed to parse lsp.json");
            BTreeMap::new()
        }
    }
}

pub(crate) struct DefaultServer {
    pub name: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    /// Workspace files that mean this language is in play.
    pub markers: &'static [&'static str],
    pub extensions: &'static [(&'static str, &'static str)],
}

pub(crate) const DEFAULT_SERVERS: &[DefaultServer] = &[
    DefaultServer {
        name: "rust-analyzer",
        command: "rust-analyzer",
        args: &[],
        markers: &["Cargo.toml"],
        extensions: &[(".rs", "rust")],
    },
    DefaultServer {
        name: "typescript-language-server",
        command: "typescript-language-server",
        args: &["--stdio"],
        markers: &["package.json", "tsconfig.json", "jsconfig.json"],
        extensions: &[
            (".ts", "typescript"),
            (".tsx", "typescriptreact"),
            (".js", "javascript"),
            (".jsx", "javascriptreact"),
            (".mjs", "javascript"),
            (".cjs", "javascript"),
        ],
    },
    DefaultServer {
        name: "gopls",
        command: "gopls",
        args: &[],
        markers: &["go.mod"],
        extensions: &[(".go", "go")],
    },
    DefaultServer {
        name: "pyright",
        command: "pyright-langserver",
        args: &["--stdio"],
        markers: &["pyproject.toml", "setup.py", "requirements.txt"],
        extensions: &[(".py", "python"), (".pyi", "python")],
    },
];

pub(crate) const SKIP_DIR_NAMES: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "dist",
    ".dock",
    ".hg",
    ".svn",
    "build",
    "out",
    "__pycache__",
    ".venv",
    "venv",
];

const MARKER_MAX_DEPTH: u32 = 4;

pub(crate) fn config_from_spec(spec: &DefaultServer) -> LspServerConfig {
    let mut extensions = HashMap::new();
    for (ext, lang) in spec.extensions {
        extensions.insert((*ext).to_string(), (*lang).to_string());
    }
    LspServerConfig {
        command: spec.command.to_string(),
        args: spec.args.iter().map(|s| (*s).to_string()).collect(),
        extensions,
        ..LspServerConfig::default()
    }
}

/// `cwd` or a nearby subdirectory has a language marker (skips `node_modules` / `target`).
pub(crate) fn workspace_has_marker(cwd: &Path, markers: &[&str]) -> bool {
    if markers.iter().any(|m| cwd.join(m).is_file()) {
        return true;
    }
    walk_for_markers(cwd, markers, 0)
}

fn walk_for_markers(dir: &Path, markers: &[&str], depth: u32) -> bool {
    if depth >= MARKER_MAX_DEPTH {
        return false;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_dir() {
            continue;
        }
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIR_NAMES.contains(&name.as_ref()) {
            continue;
        }
        if markers.iter().any(|m| path.join(m).is_file()) {
            return true;
        }
        if walk_for_markers(&path, markers, depth + 1) {
            return true;
        }
    }
    false
}

/// Language servers on PATH whose workspace markers exist under `cwd`.
pub fn detect_defaults(cwd: &Path) -> BTreeMap<String, LspServerConfig> {
    let mut out = BTreeMap::new();
    for spec in DEFAULT_SERVERS {
        if !command_on_path(spec.command) {
            continue;
        }
        if !workspace_has_marker(cwd, spec.markers) {
            continue;
        }
        out.insert(spec.name.to_string(), config_from_spec(spec));
    }
    out
}

pub fn command_on_path(name: &str) -> bool {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return p.is_file();
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    let sep = if cfg!(windows) { ';' } else { ':' };
    path.split(sep).any(|dir| {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return true;
        }
        cfg!(windows) && Path::new(dir).join(format!("{name}.exe")).is_file()
    })
}

/// Resolve which LSP server handles a file based on extension.
pub fn resolve_server(
    servers: &BTreeMap<String, LspServerConfig>,
    path: &Path,
) -> Option<(String, String)> {
    let ext = path.extension()?.to_str()?;
    let dot_ext = format!(".{ext}");
    for (server_name, server_cfg) in servers {
        if let Some(lang_id) = server_cfg.extensions.get(&dot_ext) {
            return Some((server_name.clone(), lang_id.clone()));
        }
    }
    None
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LspTransport {
    #[default]
    Stdio,
    Socket,
}

/// Which solution or projects the server should load once it is running.
///
/// Some servers do not derive their workspace from `rootUri`/`workspaceFolders`
/// and instead load it through a protocol extension. Roslyn is the notable one:
/// left alone it treats every file as a loose "miscellaneous file" and reports
/// no project-level diagnostics at all, until it is sent `solution/open` or
/// `project/open`. Wrappers such as `roslyn-language-server` do this for you; a
/// bare `Microsoft.CodeAnalysis.LanguageServer` does not.
///
/// Paths may be absolute or relative to the workspace root.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOpen {
    /// A single solution file, sent as `solution/open`.
    #[serde(default)]
    pub solution: Option<String>,
    /// Project files, sent as `project/open`.
    #[serde(default)]
    pub projects: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LspServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub transport: LspTransport,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(
        default,
        alias = "extensionToLanguage",
        alias = "extensionToLanguageId"
    )]
    pub extensions: HashMap<String, String>,
    #[serde(default, alias = "initializationOptions")]
    pub initialization_options: Option<serde_json::Value>,
    #[serde(default)]
    pub settings: Option<serde_json::Value>,
    #[serde(default, alias = "workspaceFolder")]
    pub workspace_folder: Option<String>,
    #[serde(default, alias = "workspaceOpen")]
    pub workspace_open: Option<WorkspaceOpen>,
    #[serde(default, alias = "startupTimeout")]
    pub startup_timeout: Option<u64>,
    #[serde(default, alias = "shutdownTimeout")]
    pub shutdown_timeout: Option<u64>,
    #[serde(default, alias = "restartOnCrash")]
    pub restart_on_crash: Option<bool>,
    #[serde(default, alias = "maxRestarts")]
    pub max_restarts: Option<u32>,
}

impl LspServerConfig {
    pub fn startup_timeout_ms(&self) -> u64 {
        self.startup_timeout.unwrap_or(DEFAULT_STARTUP_TIMEOUT_MS)
    }

    pub fn shutdown_timeout_ms(&self) -> u64 {
        self.shutdown_timeout.unwrap_or(DEFAULT_SHUTDOWN_TIMEOUT_MS)
    }

    pub fn restart_on_crash(&self) -> bool {
        self.restart_on_crash.unwrap_or(false)
    }

    /// Maximum restart attempts across the lifetime of a server monitor.
    /// This is a lifetime restart budget, not a per-crash-episode counter.
    pub fn max_restarts(&self) -> u32 {
        self.max_restarts.unwrap_or(3)
    }

    /// The directory this server should treat as its workspace: the per-server
    /// override if there is one, otherwise the session cwd. Everything that
    /// needs to name the server's root — `rootUri`, `workspaceFolders`,
    /// `workspaceOpen` — resolves it here so they cannot drift apart.
    pub fn effective_root<'a>(
        &'a self,
        workspace_root: &'a std::path::Path,
    ) -> &'a std::path::Path {
        self.workspace_folder
            .as_deref()
            .map(std::path::Path::new)
            .unwrap_or(workspace_root)
    }
}

#[cfg(test)]
mod tests {
    use super::{filter_project_lsp_when_untrusted, LspServerConfig};
    use crate::lsp::config_source::ConfigSource;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn sourced() -> BTreeMap<String, (LspServerConfig, ConfigSource)> {
        let mut m = BTreeMap::new();
        m.insert(
            "proj".to_string(),
            (
                LspServerConfig::default(),
                ConfigSource::Project {
                    path: PathBuf::from("/repo/.grok/lsp.json"),
                },
            ),
        );
        m.insert(
            "usr".to_string(),
            (
                LspServerConfig::default(),
                ConfigSource::User {
                    path: PathBuf::from("/home/.grok/lsp.json"),
                },
            ),
        );
        m.insert(
            "plug".to_string(),
            (
                LspServerConfig::default(),
                ConfigSource::Plugin {
                    plugin_name: "p".to_string(),
                    path: PathBuf::from("/plug/lsp.json"),
                },
            ),
        );
        m
    }

    #[test]
    fn untrusted_drops_only_project_keeps_user_and_plugin() {
        let kept = filter_project_lsp_when_untrusted(sourced(), false);
        assert_eq!(kept.len(), 2);
        assert!(!kept.contains_key("proj"));
        assert!(kept.contains_key("usr"));
        assert!(kept.contains_key("plug"));
    }

    #[test]
    fn trusted_keeps_all_including_project() {
        let kept = filter_project_lsp_when_untrusted(sourced(), true);
        assert_eq!(kept.len(), 3);
        assert!(kept.contains_key("proj"));
        assert!(kept.contains_key("usr"));
        assert!(kept.contains_key("plug"));
    }

    #[test]
    fn load_file_accepts_flat_map_and_lsp_servers_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let flat = dir.path().join("flat.json");
        std::fs::write(
            &flat,
            r#"{"rust-analyzer":{"command":"rust-analyzer","extensionToLanguage":{".rs":"rust"}}}"#,
        )
        .unwrap();
        let loaded = super::load_file(&flat);
        assert_eq!(loaded["rust-analyzer"].command, "rust-analyzer");
        assert_eq!(
            loaded["rust-analyzer"].extensions.get(".rs").unwrap(),
            "rust"
        );

        let wrapped = dir.path().join("wrapped.json");
        std::fs::write(
            &wrapped,
            r#"{"lspServers":{"gopls":{"command":"gopls","extensions":{".go":"go"}}}}"#,
        )
        .unwrap();
        let loaded = super::load_file(&wrapped);
        assert_eq!(loaded["gopls"].command, "gopls");
    }

    #[test]
    fn detect_defaults_requires_workspace_marker() {
        let dir = tempfile::tempdir().unwrap();
        let none = super::detect_defaults(dir.path());
        assert!(
            !none.contains_key("rust-analyzer"),
            "no Cargo.toml → no rust-analyzer default"
        );
        if super::command_on_path("rust-analyzer") {
            std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"t\"\n").unwrap();
            let found = super::detect_defaults(dir.path());
            assert!(
                found.contains_key("rust-analyzer"),
                "Cargo.toml + rust-analyzer on PATH"
            );
        }
    }

    #[test]
    fn resolve_tool_path_keeps_absolute() {
        let abs = if cfg!(windows) {
            r"C:\tmp\lib.rs"
        } else {
            "/tmp/lib.rs"
        };
        let resolved = super::resolve_tool_path(abs);
        assert!(resolved.is_absolute());
        assert!(resolved.ends_with("lib.rs"));
    }

    #[test]
    fn nested_package_json_counts_as_typescript_marker() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("embed-sdk");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("package.json"), "{}\n").unwrap();
        assert!(super::workspace_has_marker(
            dir.path(),
            &["package.json", "tsconfig.json", "jsconfig.json"]
        ));
        assert!(!super::workspace_has_marker(dir.path(), &["go.mod"]));
        let skipped = dir.path().join("node_modules").join("pkg");
        std::fs::create_dir_all(&skipped).unwrap();
        std::fs::write(skipped.join("go.mod"), "module x\n").unwrap();
        assert!(
            !super::workspace_has_marker(dir.path(), &["go.mod"]),
            "node_modules must not count as a workspace marker"
        );
    }

    #[test]
    fn user_config_wins_over_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"t\"\n").unwrap();
        let mut merged = BTreeMap::new();
        merged.insert(
            "rust-analyzer".into(),
            LspServerConfig {
                command: "/custom/ra".into(),
                ..LspServerConfig::default()
            },
        );
        for (name, cfg) in super::detect_defaults(dir.path()) {
            merged.entry(name).or_insert(cfg);
        }
        assert_eq!(merged["rust-analyzer"].command, "/custom/ra");
    }
}
