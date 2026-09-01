//! `/lsp` auto-config: merge detected language servers into lsp.json.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::config::{
    command_on_path, config_from_spec, load_file, load_servers, workspace_has_marker,
    DefaultServer, LspServerConfig, DEFAULT_SERVERS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspSetupScope {
    /// `<cwd>/.dock/lsp.json` — servers that match this workspace.
    Project,
    /// `~/.dock/lsp.json` — every default server found on PATH.
    User,
}

impl LspSetupScope {
    pub fn path(self, cwd: &Path) -> PathBuf {
        match self {
            Self::Project => cwd.join(".dock").join("lsp.json"),
            Self::User => crate::config::dock_home().join("lsp.json"),
        }
    }

    fn require_marker(self) -> bool {
        matches!(self, Self::Project)
    }
}

#[derive(Debug, Clone)]
pub struct LspSetupReport {
    pub path: PathBuf,
    pub added: Vec<String>,
    pub kept: Vec<String>,
    pub skipped_no_command: Vec<String>,
    pub skipped_no_marker: Vec<String>,
    pub wrote: bool,
}

impl LspSetupReport {
    pub fn display(&self) -> String {
        let mut lines = vec![format!("{}", self.path.display()), String::new()];
        if !self.kept.is_empty() {
            lines.push("已保留：".into());
            for name in &self.kept {
                lines.push(format!("- {name}"));
            }
            lines.push(String::new());
        }
        if !self.added.is_empty() {
            lines.push("已添加：".into());
            for name in &self.added {
                lines.push(format!("- {name}"));
            }
            lines.push(String::new());
        }
        if !self.skipped_no_command.is_empty() {
            lines.push("PATH 上没有：".into());
            for name in &self.skipped_no_command {
                lines.push(format!("- {name}"));
            }
            lines.push(String::new());
        }
        if !self.skipped_no_marker.is_empty() {
            lines.push("已安装但本工作区没有对应标记（未写入项目配置）：".into());
            for name in &self.skipped_no_marker {
                lines.push(format!("- {name}"));
            }
            lines.push(String::new());
        }
        if self.added.is_empty() && self.kept.is_empty() {
            lines.push(
                "没有可写入的语言服务器。请安装 rust-analyzer / typescript-language-server / gopls / pyright-langserver。"
                    .into(),
            );
        } else if self.wrote {
            lines.push("已写入。当前会话会尝试启动新服务器；lsp 工具下次调用会用这份配置。".into());
        } else {
            lines.push("已是最新，未改文件。".into());
        }
        lines.join("\n")
    }
}

pub fn composer_fill() -> String {
    "用法: /lsp [status|setup|user]\n\
     探测 PATH 与工作区标记，把缺的语言服务器写入 lsp.json。\n\
     /lsp "
        .into()
}

pub fn status_report(cwd: &Path) -> String {
    let user = crate::config::dock_home().join("lsp.json");
    let project = cwd.join(".dock").join("lsp.json");
    let configured = load_servers(cwd);
    let mut lines = vec!["LSP 配置".into(), String::new()];
    lines.push(format!(
        "用户: {}{}",
        user.display(),
        if user.is_file() { "" } else { "（无）" }
    ));
    lines.push(format!(
        "项目: {}{}",
        project.display(),
        if project.is_file() { "" } else { "（无）" }
    ));
    lines.push(String::new());
    if configured.is_empty() {
        lines.push("当前没有已加载的服务器。".into());
    } else {
        lines.push("已配置：".into());
        for (name, cfg) in &configured {
            lines.push(format!("- {name} ({})", cfg.command));
        }
    }
    lines.push(String::new());
    lines.push("探测：".into());
    for spec in DEFAULT_SERVERS {
        let on_path = command_on_path(spec.command);
        let marker = workspace_has_marker(cwd, spec.markers);
        let in_file = configured.contains_key(spec.name);
        let state = match (in_file, on_path, marker) {
            (true, _, _) => "已配置",
            (false, true, true) => "可写入（PATH + 工作区）",
            (false, true, false) => "PATH 上有，工作区无标记",
            (false, false, _) => "PATH 上没有",
        };
        lines.push(format!("- {} — {state}", spec.name));
    }
    lines.push(String::new());
    lines.push("空命令写入项目 .dock/lsp.json；/lsp user 写入 ~/.dock/lsp.json。".into());
    lines.join("\n")
}

pub fn auto_setup(cwd: &Path, scope: LspSetupScope) -> Result<LspSetupReport, String> {
    let path = scope.path(cwd);
    let require_marker = scope.require_marker();
    let (mut root, wrapped) = read_or_empty(&path)?;
    let existing = load_file(&path);
    let mut added = Vec::new();
    let mut kept: Vec<String> = existing.keys().cloned().collect();
    kept.sort();
    let mut skipped_no_command = Vec::new();
    let mut skipped_no_marker = Vec::new();
    let mut to_insert: Vec<&DefaultServer> = Vec::new();

    for spec in DEFAULT_SERVERS {
        if existing.contains_key(spec.name) {
            continue;
        }
        if !command_on_path(spec.command) {
            skipped_no_command.push(spec.name.to_string());
            continue;
        }
        if require_marker && !workspace_has_marker(cwd, spec.markers) {
            skipped_no_marker.push(format!(
                "{}（需要 {}）",
                spec.name,
                spec.markers.join(" / ")
            ));
            continue;
        }
        to_insert.push(spec);
        added.push(spec.name.to_string());
    }

    if to_insert.is_empty() {
        return Ok(LspSetupReport {
            path,
            added,
            kept,
            skipped_no_command,
            skipped_no_marker,
            wrote: false,
        });
    }

    let servers = servers_map(&mut root, wrapped)?;
    for spec in to_insert {
        servers.insert(spec.name.to_string(), spec_json(spec));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建 {}: {e}", parent.display()))?;
    }
    let body =
        serde_json::to_string_pretty(&root).map_err(|e| format!("无法序列化 lsp.json: {e}"))?;
    std::fs::write(&path, format!("{body}\n"))
        .map_err(|e| format!("无法写入 {}: {e}", path.display()))?;

    Ok(LspSetupReport {
        path,
        added,
        kept,
        skipped_no_command,
        skipped_no_marker,
        wrote: true,
    })
}

/// Servers that should be live after a successful `/lsp` write (file + PATH defaults).
pub fn merged_after_setup(cwd: &Path) -> BTreeMap<String, LspServerConfig> {
    let mut merged = load_servers(cwd);
    for spec in DEFAULT_SERVERS {
        if !command_on_path(spec.command) {
            continue;
        }
        if !workspace_has_marker(cwd, spec.markers) {
            continue;
        }
        merged
            .entry(spec.name.to_string())
            .or_insert_with(|| config_from_spec(spec));
    }
    merged
}

fn spec_json(spec: &DefaultServer) -> Value {
    let mut ext = Map::new();
    for (e, lang) in spec.extensions {
        ext.insert((*e).to_string(), json!(lang));
    }
    let mut entry = Map::new();
    entry.insert("command".into(), json!(spec.command));
    if !spec.args.is_empty() {
        entry.insert("args".into(), json!(spec.args));
    }
    entry.insert("extensionToLanguage".into(), Value::Object(ext));
    Value::Object(entry)
}

fn read_or_empty(path: &Path) -> Result<(Value, bool), String> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok((json!({}), false)),
        Ok(s) => parse_root(&s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((json!({}), false)),
        Err(e) => Err(format!("无法读取 {}: {e}", path.display())),
    }
}

fn parse_root(s: &str) -> Result<(Value, bool), String> {
    let v: Value = serde_json::from_str(s).map_err(|e| format!("无法解析 lsp.json: {e}"))?;
    let Some(obj) = v.as_object() else {
        return Err("lsp.json 必须是对象".into());
    };
    if obj.contains_key("lspServers") {
        if !obj["lspServers"].is_object() {
            return Err("lsp.json 的 lspServers 必须是对象".into());
        }
        return Ok((v, true));
    }
    Ok((v, false))
}

fn servers_map(root: &mut Value, wrapped: bool) -> Result<&mut Map<String, Value>, String> {
    if wrapped {
        root.get_mut("lspServers")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "lsp.json 的 lspServers 必须是对象".into())
    } else {
        root.as_object_mut()
            .ok_or_else(|| "lsp.json 必须是对象".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_setup_keeps_existing_and_skips_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let dock = dir.path().join(".dock");
        std::fs::create_dir(&dock).unwrap();
        let path = dock.join("lsp.json");
        std::fs::write(
            &path,
            r#"{
  "rust-analyzer": {
    "command": "/custom/ra",
    "extensionToLanguage": { ".rs": "rust" },
    "initializationOptions": { "cargo": { "allFeatures": true } }
  }
}
"#,
        )
        .unwrap();
        let report = auto_setup(dir.path(), LspSetupScope::Project).unwrap();
        assert!(report.kept.iter().any(|n| n == "rust-analyzer"));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("/custom/ra"));
        assert!(raw.contains("allFeatures"));
    }

    #[test]
    fn auto_setup_refuses_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let dock = dir.path().join(".dock");
        std::fs::create_dir(&dock).unwrap();
        let path = dock.join("lsp.json");
        std::fs::write(&path, "{not json").unwrap();
        let err = auto_setup(dir.path(), LspSetupScope::Project).unwrap_err();
        assert!(err.contains("解析"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
    }

    #[test]
    fn auto_setup_adds_path_server_when_marker_nested() {
        if !command_on_path("typescript-language-server") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("embed-sdk");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("package.json"), "{}\n").unwrap();
        let report = auto_setup(dir.path(), LspSetupScope::Project).unwrap();
        assert!(
            report
                .added
                .iter()
                .any(|n| n == "typescript-language-server"),
            "{report:?}"
        );
        assert!(report.wrote);
        let loaded = load_file(&report.path);
        assert_eq!(
            loaded["typescript-language-server"].command,
            "typescript-language-server"
        );
        assert_eq!(
            loaded["typescript-language-server"].args,
            vec!["--stdio".to_string()]
        );
    }

    #[test]
    fn composer_fill_leaves_slash() {
        let fill = composer_fill();
        assert!(fill.contains("/lsp "));
        assert!(fill.contains("status"));
    }
}
