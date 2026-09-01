//! Scan Dock skill directories and parse SKILL.md frontmatter.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

pub const MAX_NAME_LEN: usize = 64;
pub const MAX_DESCRIPTION_LEN: usize = 1024;
pub const MAX_WALK_DEPTH: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SkillScope {
    /// `{cwd}/skills/`
    Bundled,
    /// `~/.dock/skills/`
    User,
    /// `{cwd}/.agents/skills/`
    Agents,
    /// `{cwd}/.dock/skills/` (wins on name collision)
    Project,
}

impl SkillScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bundled => "仓库",
            Self::User => "用户",
            Self::Agents => "agents",
            Self::Project => "项目",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub paths: Option<Vec<String>>,
    pub user_invocable: bool,
    pub disable_model_invocation: bool,
    pub path: PathBuf,
    pub dir: PathBuf,
    pub scope: SkillScope,
}

pub fn scan_all() -> Vec<SkillInfo> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut map = IndexMap::new();
    merge_scope(&mut map, scan_dir(&cwd.join("skills"), SkillScope::Bundled));
    merge_scope(
        &mut map,
        scan_dir(&crate::config::dock_home().join("skills"), SkillScope::User),
    );
    merge_scope(
        &mut map,
        scan_dir(&cwd.join(".agents").join("skills"), SkillScope::Agents),
    );
    merge_scope(
        &mut map,
        scan_dir(&cwd.join(".dock").join("skills"), SkillScope::Project),
    );
    map.into_values().collect()
}

fn merge_scope(map: &mut IndexMap<String, SkillInfo>, skills: Vec<SkillInfo>) {
    for skill in skills {
        map.insert(skill.name.clone(), skill);
    }
}

pub fn scan_dir(dir: &Path, scope: SkillScope) -> Vec<SkillInfo> {
    find_skill_md_paths(dir)
        .into_iter()
        .filter_map(|path| parse_skill_file(&path, scope))
        .collect()
}

pub fn find_skill_md_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let root = dir.join("SKILL.md");
    if root.is_file() {
        paths.push(root);
    }
    walk_for_skill_md(dir, &mut paths, 0);
    paths
}

fn walk_for_skill_md(dir: &Path, paths: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for path in dirs {
        let skill_md = path.join("SKILL.md");
        if skill_md.is_file() {
            paths.push(skill_md);
        }
        walk_for_skill_md(&path, paths, depth + 1);
    }
}

pub fn parse_skill_file(path: &Path, scope: SkillScope) -> Option<SkillInfo> {
    let raw = std::fs::read_to_string(path).ok()?;
    let dir = path.parent()?.to_path_buf();
    let fallback = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("skill")
        .to_string();
    let fm = parse_frontmatter(&raw);
    let mut name = fm
        .name
        .filter(|n| valid_skill_name(n))
        .unwrap_or_else(|| fallback.clone());
    if !valid_skill_name(&name) {
        name = fallback;
    }
    if !valid_skill_name(&name) {
        return None;
    }
    let mut description = fm.description.unwrap_or_default();
    if description.is_empty() {
        description = peek_description(&extract_skill_body(&raw)).unwrap_or_else(|| name.clone());
    }
    description.truncate(MAX_DESCRIPTION_LEN);
    Some(SkillInfo {
        name,
        description,
        when_to_use: fm.when_to_use,
        paths: fm.paths,
        user_invocable: fm.user_invocable,
        disable_model_invocation: fm.disable_model_invocation,
        path: path.to_path_buf(),
        dir,
        scope,
    })
}

struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    when_to_use: Option<String>,
    paths: Option<Vec<String>>,
    user_invocable: bool,
    disable_model_invocation: bool,
}

fn parse_frontmatter(raw: &str) -> Frontmatter {
    let mut out = Frontmatter {
        name: None,
        description: None,
        when_to_use: None,
        paths: None,
        user_invocable: true,
        disable_model_invocation: false,
    };
    let Some(yaml) = frontmatter_yaml(raw) else {
        return out;
    };
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(yaml) else {
        return out;
    };
    let Some(map) = value.as_mapping() else {
        return out;
    };
    out.name = scalar(map.get(&yaml_key("name")));
    out.description = scalar(map.get(&yaml_key("description")));
    out.when_to_use = scalar(map.get(&yaml_key("when-to-use")))
        .or_else(|| scalar(map.get(&yaml_key("when_to_use"))));
    out.paths = string_list(map.get(&yaml_key("paths")));
    if map.contains_key(&yaml_key("user-invocable"))
        || map.contains_key(&yaml_key("user_invocable"))
    {
        out.user_invocable = yaml_true(map.get(&yaml_key("user-invocable")))
            || yaml_true(map.get(&yaml_key("user_invocable")));
    }
    out.disable_model_invocation = yaml_true(map.get(&yaml_key("disable-model-invocation")))
        || yaml_true(map.get(&yaml_key("disable_model_invocation")));
    out
}

fn yaml_key(s: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(s.into())
}

fn scalar(value: Option<&serde_yaml::Value>) -> Option<String> {
    match value? {
        serde_yaml::Value::String(s) => {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn yaml_true(value: Option<&serde_yaml::Value>) -> bool {
    matches!(value, Some(serde_yaml::Value::Bool(true)))
        || matches!(value, Some(serde_yaml::Value::String(s)) if s == "true")
}

fn string_list(value: Option<&serde_yaml::Value>) -> Option<Vec<String>> {
    match value? {
        serde_yaml::Value::String(s) => {
            let items: Vec<String> = s
                .split([',', '\n'])
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect();
            (!items.is_empty()).then_some(items)
        }
        serde_yaml::Value::Sequence(seq) => {
            let items: Vec<String> = seq
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            (!items.is_empty()).then_some(items)
        }
        _ => None,
    }
}

fn frontmatter_yaml(raw: &str) -> Option<&str> {
    let s = raw.trim_start_matches('\u{feff}').trim_start();
    let rest = s.strip_prefix("---")?;
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

pub fn extract_skill_body(raw: &str) -> String {
    let s = raw.trim_start_matches('\u{feff}');
    let trimmed = s.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(idx) = rest.find("\n---") {
            let after = &rest[idx + 4..];
            return after
                .trim_start_matches('\r')
                .trim_start_matches('\n')
                .to_string();
        }
    }
    s.to_string()
}

fn peek_description(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut s = t.to_string();
        s.truncate(MAX_DESCRIPTION_LEN);
        return Some(s);
    }
    None
}

pub fn valid_skill_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if !(1..=MAX_NAME_LEN).contains(&bytes.len()) {
        return false;
    }
    bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

pub fn apply_substitutions(body: &str, args: &str, skill_dir: &Path) -> String {
    let dir = skill_dir.display().to_string();
    body.replace("$ARGUMENTS", args)
        .replace("${ARGUMENTS}", args)
        .replace("$SKILL_DIR", &dir)
        .replace("${SKILL_DIR}", &dir)
}

pub fn path_near_skills(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("skills") | Some(".agents") | Some(".dock")
        )
    }) || path
        .to_string_lossy()
        .split(['/', '\\'])
        .any(|seg| seg == "skills")
}

pub fn extract_paths_from_tool(name: &str, arguments: &str, content: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_json_paths(arguments, &mut out);
    for prefix in ["updated ", "created ", "wrote "] {
        if let Some(rest) = content.lines().next().and_then(|l| l.strip_prefix(prefix)) {
            if !rest.starts_with("Error") {
                out.push(PathBuf::from(rest.trim()));
            }
        }
    }
    if matches!(name, "list_dir" | "glob" | "grep" | "read_file") {
        for line in content.lines().take(80) {
            let line = line.trim();
            if line.is_empty() || line.starts_with("Error") {
                continue;
            }
            let candidate = line.split(':').next().unwrap_or(line);
            if candidate.contains("SKILL.md") || path_near_skills(Path::new(candidate)) {
                out.push(PathBuf::from(candidate));
            }
        }
    }
    out.into_iter()
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

fn collect_json_paths(raw: &str, out: &mut Vec<PathBuf>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    walk_json_strings(&v, out);
}

fn walk_json_strings(v: &serde_json::Value, out: &mut Vec<PathBuf>) {
    match v {
        serde_json::Value::String(s) if !s.is_empty() => out.push(PathBuf::from(s)),
        serde_json::Value::Array(items) => {
            for item in items {
                walk_json_strings(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values() {
                walk_json_strings(val, out);
            }
        }
        _ => {}
    }
}

pub fn paths_gate_match(patterns: &[String], touched: &[PathBuf]) -> bool {
    touched.iter().any(|path| {
        let display = path.to_string_lossy();
        patterns.iter().any(|pat| globish_match(pat, &display))
    })
}

fn globish_match(pattern: &str, path: &str) -> bool {
    let pat = pattern.trim();
    if pat.is_empty() {
        return false;
    }
    if !pat.contains('*') {
        return path.contains(pat);
    }
    let needle = pat.replace("**/", "").replace("**", "").replace('*', "");
    !needle.is_empty() && path.contains(&needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_defaults_and_strict_bool() {
        let raw = "---\nname: commit\ndescription: Write commits.\n---\n# Body\n";
        let fm = parse_frontmatter(raw);
        assert_eq!(fm.name.as_deref(), Some("commit"));
        assert_eq!(fm.description.as_deref(), Some("Write commits."));
        assert!(fm.user_invocable);
        assert!(!fm.disable_model_invocation);
        let raw = "---\nname: x\ndescription: d\nuser-invocable: yes\n---\n";
        let fm = parse_frontmatter(raw);
        assert!(!fm.user_invocable, "only YAML true / \"true\" count");
        let raw = "---\nname: x\ndescription: d\nuser-invocable: false\ndisable-model-invocation: true\n---\n";
        let fm = parse_frontmatter(raw);
        assert!(!fm.user_invocable);
        assert!(fm.disable_model_invocation);
        let raw = "---\nname: y\ndescription: d\nuser-invocable: true\n---\n";
        let fm = parse_frontmatter(raw);
        assert!(fm.user_invocable);
    }

    #[test]
    fn extracts_body_after_frontmatter() {
        let raw = "---\nname: n\ndescription: d\n---\n\nHello $ARGUMENTS\n";
        assert_eq!(extract_skill_body(raw).trim(), "Hello $ARGUMENTS");
    }

    #[test]
    fn substitutions_fill_args_and_dir() {
        let got = apply_substitutions("x $ARGUMENTS ${SKILL_DIR}", "fix", Path::new("/tmp/s"));
        assert_eq!(got, "x fix /tmp/s");
    }

    #[test]
    fn extract_paths_from_write_file_content() {
        let paths = extract_paths_from_tool(
            "write_file",
            "",
            "created /tmp/.dock/skills/late-skill/SKILL.md",
        );
        assert!(paths.iter().any(|p| p.ends_with("SKILL.md")), "{paths:?}");
    }
}
