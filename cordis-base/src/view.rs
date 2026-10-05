//! 插件视图（dock.view.1）：插件交给界面的声明式视图树。契约见 `docs/PLUGIN-VIEWS.md`。
//!
//! 这里只有数据：宽松解析（不认识的节点留个占位、超限截断，不报错）、规范化的 JSON
//! （网关原样投给 GUI）、纯文本降级，以及树里所有动作 id（网关据此校验点击）。
//! 怎么画由各端自己定：TUI 在 `cordis-tui`，GUI 在 `dock-gui`。

use serde_json::{json, Map, Value};

/// 最多几层。
pub const MAX_DEPTH: usize = 8;
/// 最多几个节点。
pub const MAX_NODES: usize = 500;
/// 单个字符串最长多少字符。
pub const MAX_TEXT: usize = 20_000;

/// 色调。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Default,
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
}

impl Tone {
    /// 从 JSON 字符串认色调，不认识的回默认。
    pub fn from_json(v: Option<&Value>) -> Self {
        Self::parse(v)
    }

    fn parse(v: Option<&Value>) -> Self {
        match v.and_then(Value::as_str).unwrap_or("") {
            "muted" => Self::Muted,
            "accent" => Self::Accent,
            "success" => Self::Success,
            "warning" => Self::Warning,
            "danger" => Self::Danger,
            _ => Self::Default,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Muted => "muted",
            Self::Accent => "accent",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Danger => "danger",
        }
    }
}

/// 竖排间距 / 文字大小共用的三档。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    S,
    #[default]
    M,
    L,
}

impl Size {
    fn parse(v: Option<&Value>) -> Self {
        match v.and_then(Value::as_str).unwrap_or("") {
            "s" => Self::S,
            "l" => Self::L,
            _ => Self::M,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::S => "s",
            Self::M => "m",
            Self::L => "l",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonStyle {
    Primary,
    #[default]
    Secondary,
    Danger,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    pub text: String,
    pub tone: Tone,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KvItem {
    pub label: String,
    pub value: String,
    pub tone: Tone,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub badge: Option<Badge>,
    pub action: Option<String>,
}

/// 一个节点。字段含义见 `docs/PLUGIN-VIEWS.md`。
#[derive(Clone, Debug, PartialEq)]
pub enum ViewNode {
    Stack {
        children: Vec<ViewNode>,
        gap: Size,
    },
    Row {
        children: Vec<ViewNode>,
        between: bool,
    },
    Section {
        title: String,
        children: Vec<ViewNode>,
        collapsed: bool,
    },
    Text {
        text: String,
        tone: Tone,
        bold: bool,
        size: Size,
    },
    Markdown {
        text: String,
    },
    Code {
        text: String,
        lang: Option<String>,
    },
    Kv {
        items: Vec<KvItem>,
    },
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    List {
        items: Vec<ListItem>,
    },
    Badge(Badge),
    Progress {
        /// 0–1；`None` = 不确定进度。
        value: Option<f64>,
        label: Option<String>,
    },
    Button {
        label: String,
        action: String,
        style: ButtonStyle,
    },
    Link {
        label: String,
        url: String,
    },
    Divider,
    Empty {
        title: String,
        text: Option<String>,
        action: Option<String>,
        label: Option<String>,
    },
    /// 不认识的节点：各端画一行「不支持的视图：<type>」。
    Unsupported {
        kind: String,
    },
}

struct Budget {
    nodes: usize,
    truncated: bool,
}

fn text_of(v: Option<&Value>) -> String {
    let s = match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    };
    clip(s)
}

fn opt_text(v: Option<&Value>) -> Option<String> {
    let s = text_of(v);
    (!s.is_empty()).then_some(s)
}

fn clip(s: String) -> String {
    if s.chars().count() <= MAX_TEXT {
        s
    } else {
        let mut out: String = s.chars().take(MAX_TEXT).collect();
        out.push('…');
        out
    }
}

/// `kv` / `table` / `list` 的条目也算进节点预算：一张几万行的表不该绕过限额。
fn take_items<'a>(v: Option<&'a Value>, budget: &mut Budget) -> Vec<&'a Value> {
    let Some(items) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    let n = items.len().min(budget.nodes);
    if n < items.len() {
        budget.truncated = true;
    }
    budget.nodes -= n;
    items[..n].iter().collect()
}

/// 链接只放行 http / https / mailto；别的（`file:`、自定义协议）不能交给系统去开。
fn safe_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|p| lower.starts_with(p))
}

fn badge_of(v: Option<&Value>) -> Option<Badge> {
    let v = v?;
    let text = text_of(v.get("text"));
    (!text.is_empty()).then(|| Badge {
        text,
        tone: Tone::parse(v.get("tone")),
    })
}

impl ViewNode {
    /// 宽松解析一棵树：不认识的节点变 [`ViewNode::Unsupported`]，超过
    /// [`MAX_DEPTH`] / [`MAX_NODES`] 的部分丢掉并在末尾加一行提示。从不失败。
    pub fn parse(value: &Value) -> ViewNode {
        let mut budget = Budget {
            nodes: MAX_NODES,
            truncated: false,
        };
        let root = Self::parse_at(value, 1, &mut budget).unwrap_or(ViewNode::Stack {
            children: Vec::new(),
            gap: Size::M,
        });
        if !budget.truncated {
            return root;
        }
        ViewNode::Stack {
            children: vec![
                root,
                ViewNode::Text {
                    text: "（视图过大，后面的部分没有显示）".into(),
                    tone: Tone::Muted,
                    bold: false,
                    size: Size::S,
                },
            ],
            gap: Size::M,
        }
    }

    fn parse_children(v: Option<&Value>, depth: usize, budget: &mut Budget) -> Vec<ViewNode> {
        v.and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| Self::parse_at(item, depth + 1, budget))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn parse_at(v: &Value, depth: usize, budget: &mut Budget) -> Option<ViewNode> {
        if depth > MAX_DEPTH || budget.nodes == 0 {
            budget.truncated = true;
            return None;
        }
        budget.nodes -= 1;
        // 纯字符串当一段文字，写起来省事。
        if let Some(s) = v.as_str() {
            return Some(ViewNode::Text {
                text: clip(s.to_string()),
                tone: Tone::Default,
                bold: false,
                size: Size::M,
            });
        }
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        let node = match kind {
            "stack" => ViewNode::Stack {
                children: Self::parse_children(v.get("children"), depth, budget),
                gap: Size::parse(v.get("gap")),
            },
            "row" => ViewNode::Row {
                children: Self::parse_children(v.get("children"), depth, budget),
                between: v.get("align").and_then(Value::as_str) == Some("between"),
            },
            "section" => ViewNode::Section {
                title: text_of(v.get("title")),
                children: Self::parse_children(v.get("children"), depth, budget),
                collapsed: v.get("collapsed").and_then(Value::as_bool).unwrap_or(false),
            },
            "text" => ViewNode::Text {
                text: text_of(v.get("text")),
                tone: Tone::parse(v.get("tone")),
                bold: v.get("weight").and_then(Value::as_str) == Some("bold"),
                size: Size::parse(v.get("size")),
            },
            "markdown" => ViewNode::Markdown {
                text: text_of(v.get("text")),
            },
            "code" => ViewNode::Code {
                text: text_of(v.get("text")),
                lang: opt_text(v.get("lang")),
            },
            "kv" => ViewNode::Kv {
                items: take_items(v.get("items"), budget)
                    .into_iter()
                    .map(|i| KvItem {
                        label: text_of(i.get("label")),
                        value: text_of(i.get("value")),
                        tone: Tone::parse(i.get("tone")),
                    })
                    .collect(),
            },
            "table" => ViewNode::Table {
                columns: v
                    .get("columns")
                    .and_then(Value::as_array)
                    .map(|c| c.iter().map(|x| text_of(Some(x))).collect())
                    .unwrap_or_default(),
                rows: take_items(v.get("rows"), budget)
                    .into_iter()
                    .map(|r| {
                        r.as_array()
                            .map(|cells| cells.iter().map(|c| text_of(Some(c))).collect())
                            .unwrap_or_default()
                    })
                    .collect(),
            },
            "list" => ViewNode::List {
                items: take_items(v.get("items"), budget)
                    .into_iter()
                    .map(|i| ListItem {
                        title: text_of(i.get("title")),
                        subtitle: opt_text(i.get("subtitle")),
                        badge: badge_of(i.get("badge")),
                        action: opt_text(i.get("action")),
                    })
                    .collect(),
            },
            "badge" => ViewNode::Badge(Badge {
                text: text_of(v.get("text")),
                tone: Tone::parse(v.get("tone")),
            }),
            "progress" => ViewNode::Progress {
                value: v
                    .get("value")
                    .and_then(Value::as_f64)
                    .map(|x| x.clamp(0.0, 1.0)),
                label: opt_text(v.get("label")),
            },
            "button" => ViewNode::Button {
                label: text_of(v.get("label")),
                action: text_of(v.get("action")),
                style: match v.get("style").and_then(Value::as_str).unwrap_or("") {
                    "primary" => ButtonStyle::Primary,
                    "danger" => ButtonStyle::Danger,
                    _ => ButtonStyle::Secondary,
                },
            },
            "link" => {
                let label = text_of(v.get("label"));
                let url = text_of(v.get("url"));
                if safe_url(&url) {
                    ViewNode::Link { label, url }
                } else {
                    ViewNode::Text {
                        text: format!("{label}（已拦下不安全的链接）"),
                        tone: Tone::Muted,
                        bold: false,
                        size: Size::M,
                    }
                }
            }
            "divider" => ViewNode::Divider,
            "empty" => ViewNode::Empty {
                title: text_of(v.get("title")),
                text: opt_text(v.get("text")),
                action: opt_text(v.get("action")),
                label: opt_text(v.get("label")),
            },
            other => ViewNode::Unsupported {
                kind: if other.is_empty() {
                    "（缺 type）".into()
                } else {
                    other.chars().take(40).collect()
                },
            },
        };
        Some(node)
    }

    /// 规范化的 JSON：只带认识的字段，网关原样投给 GUI。
    pub fn to_value(&self) -> Value {
        let kids = |c: &[ViewNode]| Value::Array(c.iter().map(ViewNode::to_value).collect());
        let badge = |b: &Badge| json!({ "text": b.text, "tone": b.tone.as_str() });
        match self {
            ViewNode::Stack { children, gap } => {
                json!({ "type": "stack", "gap": gap.as_str(), "children": kids(children) })
            }
            ViewNode::Row { children, between } => json!({
                "type": "row",
                "align": if *between { "between" } else { "start" },
                "children": kids(children),
            }),
            ViewNode::Section {
                title,
                children,
                collapsed,
            } => json!({
                "type": "section",
                "title": title,
                "collapsed": collapsed,
                "children": kids(children),
            }),
            ViewNode::Text {
                text,
                tone,
                bold,
                size,
            } => json!({
                "type": "text",
                "text": text,
                "tone": tone.as_str(),
                "weight": if *bold { "bold" } else { "normal" },
                "size": size.as_str(),
            }),
            ViewNode::Markdown { text } => json!({ "type": "markdown", "text": text }),
            ViewNode::Code { text, lang } => json!({ "type": "code", "text": text, "lang": lang }),
            ViewNode::Kv { items } => json!({
                "type": "kv",
                "items": items
                    .iter()
                    .map(|i| json!({ "label": i.label, "value": i.value, "tone": i.tone.as_str() }))
                    .collect::<Vec<_>>(),
            }),
            ViewNode::Table { columns, rows } => {
                json!({ "type": "table", "columns": columns, "rows": rows })
            }
            ViewNode::List { items } => json!({
                "type": "list",
                "items": items
                    .iter()
                    .map(|i| {
                        let mut m = Map::new();
                        m.insert("title".into(), json!(i.title));
                        if let Some(s) = &i.subtitle {
                            m.insert("subtitle".into(), json!(s));
                        }
                        if let Some(b) = &i.badge {
                            m.insert("badge".into(), badge(b));
                        }
                        if let Some(a) = &i.action {
                            m.insert("action".into(), json!(a));
                        }
                        Value::Object(m)
                    })
                    .collect::<Vec<_>>(),
            }),
            ViewNode::Badge(b) => {
                let mut v = badge(b);
                v["type"] = json!("badge");
                v
            }
            ViewNode::Progress { value, label } => {
                json!({ "type": "progress", "value": value, "label": label })
            }
            ViewNode::Button {
                label,
                action,
                style,
            } => json!({
                "type": "button",
                "label": label,
                "action": action,
                "style": match style {
                    ButtonStyle::Primary => "primary",
                    ButtonStyle::Secondary => "secondary",
                    ButtonStyle::Danger => "danger",
                },
            }),
            ViewNode::Link { label, url } => json!({ "type": "link", "label": label, "url": url }),
            ViewNode::Divider => json!({ "type": "divider" }),
            ViewNode::Empty {
                title,
                text,
                action,
                label,
            } => json!({
                "type": "empty",
                "title": title,
                "text": text,
                "action": action,
                "label": label,
            }),
            // 原样带回不认识的 type：GUI 同样画「不支持的视图：<type>」。
            ViewNode::Unsupported { kind } => json!({ "type": kind }),
        }
    }

    /// 树里所有可点的动作 id（按钮、列表行、空状态按钮），按出现顺序、去重。
    /// 包括折叠段里的（GUI 能展开）；网关据此校验点击。
    pub fn actions(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_actions(&mut out, true);
        out
    }

    /// 同 [`Self::actions`]，但跳过折叠段里的：终端不画折叠段的内容，给看不见的
    /// 动作编号会让数字键点到藏起来的按钮。
    pub fn visible_actions(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_actions(&mut out, false);
        out
    }

    fn collect_actions(&self, out: &mut Vec<String>, hidden: bool) {
        let mut push = |a: &str| {
            if !a.is_empty() && !out.iter().any(|x| x == a) {
                out.push(a.to_string());
            }
        };
        match self {
            ViewNode::Section {
                collapsed: true, ..
            } if !hidden => {}
            ViewNode::Stack { children, .. }
            | ViewNode::Row { children, .. }
            | ViewNode::Section { children, .. } => {
                for c in children {
                    c.collect_actions(out, hidden);
                }
            }
            ViewNode::Button { action, .. } => push(action),
            ViewNode::List { items } => {
                for a in items.iter().filter_map(|i| i.action.as_deref()) {
                    push(a);
                }
            }
            ViewNode::Empty {
                action: Some(a), ..
            } => push(a),
            _ => {}
        }
    }

    /// 纯文本降级（底栏一行、只认文本的客户端）。
    pub fn to_plain(&self) -> String {
        let mut lines = Vec::new();
        self.plain_into(&mut lines);
        lines.join("\n")
    }

    fn plain_into(&self, out: &mut Vec<String>) {
        match self {
            ViewNode::Stack { children, .. } | ViewNode::Section { children, .. } => {
                if let ViewNode::Section { title, .. } = self {
                    out.push(title.clone());
                }
                for c in children {
                    c.plain_into(out);
                }
            }
            ViewNode::Row { children, .. } => {
                let mut parts = Vec::new();
                for c in children {
                    let mut sub = Vec::new();
                    c.plain_into(&mut sub);
                    parts.push(sub.join(" "));
                }
                out.push(parts.join("  "));
            }
            ViewNode::Text { text, .. }
            | ViewNode::Markdown { text }
            | ViewNode::Code { text, .. } => out.push(text.clone()),
            ViewNode::Kv { items } => {
                for i in items {
                    out.push(format!("{}：{}", i.label, i.value));
                }
            }
            ViewNode::Table { columns, rows } => {
                out.push(columns.join(" | "));
                for r in rows {
                    out.push(r.join(" | "));
                }
            }
            ViewNode::List { items } => {
                for i in items {
                    let mut line = format!("• {}", i.title);
                    if let Some(b) = &i.badge {
                        line.push_str(&format!(" [{}]", b.text));
                    }
                    out.push(line);
                    if let Some(s) = &i.subtitle {
                        out.push(format!("  {s}"));
                    }
                }
            }
            ViewNode::Badge(b) => out.push(format!("[{}]", b.text)),
            ViewNode::Progress { value, label } => {
                let pct = value
                    .map(|v| format!("{:.0}%", v * 100.0))
                    .unwrap_or_else(|| "…".into());
                out.push(match label {
                    Some(l) => format!("{l} {pct}"),
                    None => pct,
                });
            }
            ViewNode::Button { label, .. } => out.push(format!("[{label}]")),
            ViewNode::Link { label, url } => out.push(format!("{label} <{url}>")),
            ViewNode::Divider => out.push("—".into()),
            ViewNode::Empty { title, text, .. } => {
                out.push(title.clone());
                if let Some(t) = text {
                    out.push(t.clone());
                }
            }
            ViewNode::Unsupported { kind } => out.push(format!("不支持的视图：{kind}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_nodes_and_keeps_unknown_as_placeholder() {
        let v = json!({
            "type": "stack",
            "children": [
                { "type": "kv", "items": [{ "label": "环境", "value": "production", "tone": "success" }] },
                { "type": "progress", "value": 1.7, "label": "上传中" },
                { "type": "chart", "data": [1, 2] },
                "裸字符串",
                { "type": "row", "align": "between", "children": [
                    { "type": "button", "label": "部署", "action": "deploy", "style": "primary" },
                    { "type": "button", "label": "回滚", "action": "rollback" }
                ]},
                { "type": "list", "items": [{ "title": "v2.4.1", "action": "open:1", "badge": { "text": "成功", "tone": "success" } }] }
            ]
        });
        let node = ViewNode::parse(&v);
        let ViewNode::Stack { children, .. } = &node else {
            panic!("{node:?}");
        };
        assert!(matches!(&children[1], ViewNode::Progress { value: Some(v), .. } if *v == 1.0));
        assert_eq!(
            children[2],
            ViewNode::Unsupported {
                kind: "chart".into()
            }
        );
        assert!(matches!(&children[3], ViewNode::Text { text, .. } if text == "裸字符串"));
        assert_eq!(node.actions(), vec!["deploy", "rollback", "open:1"]);
        // 规范化后再解析，结果不变。
        assert_eq!(ViewNode::parse(&node.to_value()), node);
        assert!(node.to_plain().contains("环境：production"));
        assert!(node.to_plain().contains("不支持的视图：chart"));
    }

    /// 折叠段里的动作：网关照样认（GUI 能展开），终端不编号（看不见就点不到）。
    #[test]
    fn collapsed_sections_hide_actions_from_the_terminal() {
        let node = ViewNode::parse(&json!({ "type": "stack", "children": [
            { "type": "section", "title": "危险", "collapsed": true, "children": [
                { "type": "button", "label": "删除", "action": "delete", "style": "danger" }
            ]},
            { "type": "button", "label": "部署", "action": "deploy" }
        ]}));
        assert_eq!(node.actions(), vec!["delete", "deploy"]);
        assert_eq!(node.visible_actions(), vec!["deploy"]);
    }

    #[test]
    fn unsafe_links_are_not_links() {
        let ok =
            ViewNode::parse(&json!({ "type": "link", "label": "日志", "url": "https://x.dev" }));
        assert!(matches!(ok, ViewNode::Link { .. }));
        for url in ["file:///etc/passwd", "javascript:alert(1)", "vscode://x"] {
            let node = ViewNode::parse(&json!({ "type": "link", "label": "日志", "url": url }));
            assert!(
                matches!(&node, ViewNode::Text { text, .. } if text.contains("不安全")),
                "{url}: {node:?}"
            );
        }
    }

    #[test]
    fn oversized_trees_are_cut_with_a_note() {
        let mut deep = json!({ "type": "text", "text": "底" });
        for _ in 0..20 {
            deep = json!({ "type": "stack", "children": [deep] });
        }
        let node = ViewNode::parse(&deep);
        assert!(node.to_plain().contains("视图过大"));

        let wide = json!({ "type": "stack", "children": vec![json!("x"); MAX_NODES + 50] });
        let ViewNode::Stack { children, .. } = ViewNode::parse(&wide) else {
            panic!()
        };
        let ViewNode::Stack { children: kept, .. } = &children[0] else {
            panic!()
        };
        assert_eq!(kept.len(), MAX_NODES - 1, "根节点自己占一个名额");

        // 条目也算预算：一张超大的表被截短并提示。
        let rows = vec![json!(["a", "b"]); MAX_NODES * 4];
        let big = ViewNode::parse(&json!({ "type": "table", "columns": ["x", "y"], "rows": rows }));
        assert!(big.to_plain().contains("视图过大"));
        let ViewNode::Stack { children, .. } = &big else {
            panic!()
        };
        let ViewNode::Table { rows, .. } = &children[0] else {
            panic!()
        };
        assert_eq!(rows.len(), MAX_NODES - 1);

        let long = ViewNode::parse(&json!({ "type": "text", "text": "a".repeat(MAX_TEXT + 10) }));
        let ViewNode::Text { text, .. } = long else {
            panic!()
        };
        assert_eq!(text.chars().count(), MAX_TEXT + 1);
    }
}
