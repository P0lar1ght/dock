//! Scan Dock skill directories and parse SKILL.md frontmatter.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

pub const MAX_NAME_LEN: usize = 64;
pub const MAX_DESCRIPTION_LEN: usize = 1024;
pub const MAX_WALK_DEPTH: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SkillScope {
    /// 编译期嵌入、启动时物化到 `$DOCK_HOME/bundled/skills/`（最低优先级）
    Builtin,
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
            Self::Builtin => "内置",
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
    /// Agent Skills 规范的可选 `license`：许可名，或指向同目录的许可文件。
    pub license: Option<String>,
    pub paths: Option<Vec<String>>,
    pub user_invocable: bool,
    pub disable_model_invocation: bool,
    pub path: PathBuf,
    pub dir: PathBuf,
    pub scope: SkillScope,
}

impl SkillInfo {
    /// Workspace- or home-relative path for model-facing listings.
    ///
    /// Deliberately not named `listing_path`: that is the
    /// [`ListEntry`](crate::prompt::listing::ListEntry) trait method in
    /// `skills/listing.rs`, and its impl delegates here. Same-named inherent +
    /// trait methods resolve to the inherent one, so a future rename here
    /// would silently turn that delegation into infinite recursion.
    pub fn display_path(&self) -> String {
        match self.scope {
            SkillScope::Builtin => format!("bundled/skills/{}/SKILL.md", self.name),
            SkillScope::Bundled => format!("skills/{}/SKILL.md", self.name),
            SkillScope::User => format!("~/.dock/skills/{}/SKILL.md", self.name),
            SkillScope::Agents => format!(".agents/skills/{}/SKILL.md", self.name),
            SkillScope::Project => format!(".dock/skills/{}/SKILL.md", self.name),
        }
    }
}

pub fn scan_all() -> Vec<SkillInfo> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut map = IndexMap::new();
    // 内置最先合并：同名技能被任何更高层覆盖（对齐 Grok bundled 语义）。
    merge_scope(
        &mut map,
        scan_dir(&materialize_bundled(), SkillScope::Builtin),
    );
    merge_scope(&mut map, scan_dir(&cwd.join("skills"), SkillScope::Bundled));
    merge_scope(
        &mut map,
        scan_dir(
            &cordis_base::config::dock_home().join("skills"),
            SkillScope::User,
        ),
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

/// 把编译期嵌入的内置技能物化到 `$DOCK_HOME/bundled/skills/`，返回缓存目录。
/// 已存在的文件不覆盖（用户可以直接改缓存；同名技能本就被更高层覆盖）。
fn materialize_bundled() -> PathBuf {
    let root = cordis_base::config::dock_home()
        .join("bundled")
        .join("skills");
    for (rel, content) in crate::tools::skills::builtin::BUILTIN_FILES {
        let path = root.join(rel);
        if path.is_file() {
            continue;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, content);
    }
    root
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
        license: fm.license,
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
    license: Option<String>,
    paths: Option<Vec<String>>,
    user_invocable: bool,
    disable_model_invocation: bool,
}

fn parse_frontmatter(raw: &str) -> Frontmatter {
    let mut out = Frontmatter {
        name: None,
        description: None,
        when_to_use: None,
        license: None,
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
    out.name = scalar(map.get(yaml_key("name")));
    out.description = scalar(map.get(yaml_key("description")));
    out.when_to_use = scalar(map.get(yaml_key("when-to-use")))
        .or_else(|| scalar(map.get(yaml_key("when_to_use"))));
    out.license = scalar(map.get(yaml_key("license")));
    out.paths = string_list(map.get(yaml_key("paths")));
    if map.contains_key(yaml_key("user-invocable")) || map.contains_key(yaml_key("user_invocable"))
    {
        out.user_invocable = yaml_true(map.get(yaml_key("user-invocable")))
            || yaml_true(map.get(yaml_key("user_invocable")));
    }
    out.disable_model_invocation = yaml_true(map.get(yaml_key("disable-model-invocation")))
        || yaml_true(map.get(yaml_key("disable_model_invocation")));
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

/// Agent Skills 规范：1-64 字符，仅小写字母/数字/连字符，不得以连字符开头
/// 或结尾，不得有连续连字符。
pub fn valid_skill_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if !(1..=MAX_NAME_LEN).contains(&bytes.len()) {
        return false;
    }
    bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && !bytes.starts_with(b"-")
        && !bytes.ends_with(b"-")
        && !name.contains("--")
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

/// gitignore 风格的受限 glob，支持 `*`（单段内任意）、`**`（跨任意段，可为零段）、
/// `?`（单个非 `/` 字符）。
/// - 不含 `/` 的模式：按路径段名匹配（文件名或目录名，目录即其下任意文件）。
/// - 含 `/` 的模式：按段比对，但从**每个段边界**都试一次 —— 触碰路径常是
///   绝对路径，从段边界起试才能让 `docs/**` 命中 `/abs/cwd/docs/a.md`。
///   这是有意放宽，不是 gitignore 的严格 anchored 语义（`src/*.rs` 也会命中
///   `/other/src/main.rs`）。
fn globish_match(pattern: &str, path: &str) -> bool {
    let pat = pattern.trim().trim_start_matches("./");
    if pat.is_empty() {
        return false;
    }
    let pat = pat.trim_end_matches('/');
    let path = path.trim_start_matches("./");
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return false;
    }
    if !pat.contains('/') {
        // 文件/目录名（可含通配）：匹配任意层级的单段。
        return segs.iter().any(|s| match_segment(pat, s));
    }
    let pat_segs: Vec<&str> = pat.split('/').filter(|s| !s.is_empty()).collect();
    (0..segs.len()).any(|i| match_segments(&pat_segs, &segs[i..]))
}

fn match_segments(pat: &[&str], segs: &[&str]) -> bool {
    match pat.split_first() {
        None => segs.is_empty(),
        Some((p, rest)) if *p == "**" => (0..=segs.len()).any(|i| match_segments(rest, &segs[i..])),
        Some((p, rest)) => match segs.split_first() {
            Some((s, srest)) => match_segment(p, s) && match_segments(rest, srest),
            None => false,
        },
    }
}

/// 单段内通配：`*` 任意（可空）、`?` 单字符。
fn match_segment(pat: &str, seg: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = seg.chars().collect();
    fn go(p: &[char], s: &[char]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some(('?', rest)) => !s.is_empty() && go(rest, &s[1..]),
            Some(('*', rest)) => (0..=s.len()).any(|i| go(rest, &s[i..])),
            Some((c, rest)) => s.first() == Some(c) && go(rest, &s[1..]),
        }
    }
    go(&p, &s)
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

    #[test]
    fn frontmatter_license_is_parsed() {
        let raw = "---\nname: mcp-builder\ndescription: d\nlicense: Complete terms in LICENSE.txt\n---\nBody.\n";
        assert_eq!(
            parse_frontmatter(raw).license.as_deref(),
            Some("Complete terms in LICENSE.txt")
        );
        let raw = "---\nname: x\ndescription: d\n---\n";
        assert_eq!(parse_frontmatter(raw).license, None);
    }

    /// 内置与仓库技能必须满足 Agent Skills 规范的最小要求：name 合法且与目录
    /// 同名、description 非空且不超 1024 字符。
    #[test]
    fn shipped_skills_conform_to_spec() {
        let _env = cordis_base::test_env::scoped().home();
        for (rel, content) in crate::tools::skills::builtin::BUILTIN_FILES {
            if !rel.ends_with("SKILL.md") {
                continue;
            }
            assert_conforms(
                rel,
                rel.trim_end_matches("/SKILL.md")
                    .rsplit('/')
                    .next()
                    .unwrap(),
                content,
            );
        }

        let repo_skills = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("skills");
        let mut repo_checked = 0usize;
        for entry in std::fs::read_dir(&repo_skills)
            .into_iter()
            .flatten()
            .flatten()
        {
            let dir = entry.path();
            let skill_md = dir.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            let raw = std::fs::read_to_string(&skill_md).unwrap();
            let dir_name = dir.file_name().unwrap().to_str().unwrap().to_string();
            assert_conforms(&dir_name, &dir_name, &raw);
            repo_checked += 1;
        }
        assert!(
            repo_checked >= 4,
            "expected the four repo skills, got {repo_checked}"
        );
    }

    /// name 必须合法且与目录同名；description 必填且不超 1024 字符。
    /// 规范里 name 可选（缺省取目录名），所以只校验「写了就必须对」。
    fn assert_conforms(dir_name: &str, expected_name: &str, raw: &str) {
        let fm = parse_frontmatter(raw);
        if let Some(name) = fm.name {
            assert!(valid_skill_name(&name), "{dir_name}: invalid name {name}");
            assert_eq!(name, expected_name, "{dir_name}: name must match dir");
        }
        let desc = fm.description.unwrap_or_default();
        assert!(!desc.trim().is_empty(), "{dir_name}: missing description");
        assert!(
            desc.chars().count() <= MAX_DESCRIPTION_LEN,
            "{dir_name}: description over {MAX_DESCRIPTION_LEN} chars"
        );
    }

    #[test]
    fn skill_name_rules_follow_agent_skills_spec() {
        assert!(valid_skill_name("pdf-processing"));
        assert!(valid_skill_name("a"));
        assert!(!valid_skill_name("PDF"));
        assert!(!valid_skill_name("-pdf"));
        assert!(!valid_skill_name("pdf-"));
        assert!(!valid_skill_name("pdf--processing"));
        assert!(!valid_skill_name(""));
        assert!(!valid_skill_name(&"x".repeat(65)));
    }

    #[test]
    fn glob_star_does_not_cross_segments_or_suffixes() {
        // `*.rs` 无斜杠：按单段匹配、任意层级（gitignore 语义）。
        assert!(globish_match("*.rs", "main.rs"));
        assert!(globish_match("*.rs", "src/main.rs"));
        assert!(!globish_match("*.rs", "main.rsx"));
        // 单段内的 `*` 不跨 `/`：`src/*.rs` 不进孙子目录。
        assert!(globish_match("src/*.rs", "src/main.rs"));
        assert!(!globish_match("src/*.rs", "src/deep/main.rs"));
        assert!(globish_match("**/*.rs", "src/deep/main.rs"));
        assert!(globish_match("**/*.rs", "main.rs"), "** 可匹配零段");
    }

    #[test]
    fn glob_double_star_and_question() {
        assert!(globish_match("src/**/*.rs", "src/a/b.rs"));
        assert!(globish_match("src/**/*.rs", "src/b.rs"), "src/**/ 可为零段");
        assert!(!globish_match("src/**/*.rs", "lib/b.rs"));
        assert!(globish_match("test_?.py", "test_1.py"));
        assert!(!globish_match("test_?.py", "test_10.py"));
        assert!(
            globish_match("docs", "docs/guide.md"),
            "目录模式命中其下文件"
        );
        assert!(globish_match("docs", "docs"));
    }

    #[test]
    fn glob_literal_with_slash_anchors_from_segment_boundary() {
        assert!(globish_match("docs/guide.md", "/abs/cwd/docs/guide.md"));
        assert!(globish_match(
            ".dock/skills/x/SKILL.md",
            "repo/.dock/skills/x/SKILL.md"
        ));
        assert!(!globish_match("docs/guide.md", "/abs/cwd/other/guide.md"));
    }

    #[test]
    fn builtin_materializes_and_yields_lowest_priority() {
        let _env = cordis_base::test_env::scoped().home();
        let root = materialize_bundled();
        for name in crate::tools::skills::builtin::builtin_skill_dirs() {
            assert!(root.join(name).join("SKILL.md").is_file(), "{name}");
        }
        // 幂等：再次物化不覆盖、不报错。
        let _ = materialize_bundled();
        let skills = scan_all();
        let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"dock-guide"), "{names:?}");
        assert!(names.contains(&"dock-config"), "{names:?}");
        // `workflow` 工具的描述直接指着 create-workflow：装了 dock 就得有这一份，
        // 不能只在仓库 `skills/` 里躺着。
        assert!(names.contains(&"create-workflow"), "{names:?}");
        let sc = skills.iter().find(|s| s.name == "dock-guide").unwrap();
        assert_eq!(sc.scope, SkillScope::Builtin);
        // 同名用户技能覆盖内置。
        let user_dir = cordis_base::config::dock_home()
            .join("skills")
            .join("dock-guide");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(
            user_dir.join("SKILL.md"),
            "---\nname: dock-guide\ndescription: user override\n---\nUser body.\n",
        )
        .unwrap();
        let skills = scan_all();
        let sc = skills.iter().find(|s| s.name == "dock-guide").unwrap();
        assert_eq!(sc.scope, SkillScope::User);
        assert_eq!(sc.description, "user override");
    }
}
