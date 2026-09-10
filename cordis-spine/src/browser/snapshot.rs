//! Lean accessibility snapshot + stable refs (agent-browser shape, not a dep).
//!
//! Output lines look like:
//!   - button "Submit" [ref=e1]
//!   - textbox "Email" [ref=e2] value="a@b.com"
//! Refs are `@eN` / `eN` / `ref=eN` for click/type.

use std::collections::{HashMap, HashSet};

use chromiumoxide::cdp::browser_protocol::accessibility::AxNode;
use chromiumoxide::cdp::browser_protocol::dom::BackendNodeId;

/// Stored target for a snapshot ref.
#[derive(Debug, Clone)]
pub struct RefEntry {
    pub role: String,
    #[allow(dead_code)] // kept for future role+name re-query
    pub name: String,
    pub backend_dom_node_id: Option<i64>,
}

/// Result of a lean snapshot.
#[derive(Debug, Clone)]
pub struct LeanSnapshot {
    pub text: String,
    pub refs: HashMap<String, RefEntry>,
}

const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "slider",
    "spinbutton",
    "switch",
    "tab",
    "treeitem",
];

const STRUCTURAL_KEEP: &[&str] = &[
    "heading",
    "img",
    "image",
    "navigation",
    "main",
    "banner",
    "contentinfo",
    "form",
    "dialog",
    "alertdialog",
    "alert",
    "status",
    "table",
    "row",
    "cell",
    "columnheader",
    "rowheader",
    "list",
    "listitem",
];

const NOISE_ROLES: &[&str] = &[
    "none",
    "presentation",
    "InlineTextBox",
    "generic",
    "LineBreak",
    "paragraph",
    "StaticText",
    "RootWebArea",
    "WebArea",
    "ScrollArea",
    "ignored",
];

fn ax_string(v: &Option<chromiumoxide::cdp::browser_protocol::accessibility::AxValue>) -> String {
    v.as_ref()
        .and_then(|ax| ax.value.as_ref())
        .map(|j| match j {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string().trim_matches('"').to_string(),
        })
        .unwrap_or_default()
}

fn normalize_role(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return "generic".into();
    }
    // CDP often returns Role casing like "StaticText"; lower for matching.
    t.to_string()
}

fn role_key(role: &str) -> String {
    role.to_ascii_lowercase()
}

fn is_interactive(role: &str) -> bool {
    let k = role_key(role);
    INTERACTIVE_ROLES.iter().any(|r| *r == k)
}

fn is_noise(role: &str, name: &str, interactive_only: bool) -> bool {
    let k = role_key(role);
    if NOISE_ROLES.iter().any(|r| role_key(r) == k) {
        return true;
    }
    if interactive_only {
        return !is_interactive(role);
    }
    // Keep interactive + a few structural / named nodes; skip empty generics.
    if is_interactive(role) || STRUCTURAL_KEEP.iter().any(|r| *r == k.as_str()) {
        return false;
    }
    name.trim().is_empty()
}

/// Parse ref argument: `@e1`, `ref=e1`, or bare `e1`.
pub fn parse_ref(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('@') {
        return Some(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("ref=") {
        return Some(rest.to_string());
    }
    if s.starts_with('e') && s[1..].chars().all(|c| c.is_ascii_digit()) {
        return Some(s.to_string());
    }
    Some(s.to_string())
}

/// Build a lean tree from CDP `Accessibility.getFullAXTree` nodes.
pub fn build_lean_snapshot(nodes: &[AxNode], interactive_only: bool) -> LeanSnapshot {
    let by_id: HashMap<&str, &AxNode> = nodes
        .iter()
        .map(|n| (n.node_id.inner().as_str(), n))
        .collect();

    // Pick roots: nodes with no parent or parent missing.
    let child_set: HashSet<&str> = nodes
        .iter()
        .filter_map(|n| n.child_ids.as_ref())
        .flatten()
        .map(|id| id.inner().as_str())
        .collect();
    let roots: Vec<&AxNode> = nodes
        .iter()
        .filter(|n| !child_set.contains(n.node_id.inner().as_str()))
        .collect();

    let mut refs = HashMap::new();
    let mut lines = Vec::new();
    let mut next_ref = 1u32;

    fn walk(
        node: &AxNode,
        by_id: &HashMap<&str, &AxNode>,
        depth: usize,
        interactive_only: bool,
        next_ref: &mut u32,
        refs: &mut HashMap<String, RefEntry>,
        lines: &mut Vec<String>,
    ) {
        if node.ignored {
            // Still walk children — ignored wrappers often contain real nodes.
            if let Some(children) = &node.child_ids {
                for cid in children {
                    if let Some(child) = by_id.get(cid.inner().as_str()) {
                        walk(child, by_id, depth, interactive_only, next_ref, refs, lines);
                    }
                }
            }
            return;
        }

        let role = normalize_role(&ax_string(&node.role));
        let name = ax_string(&node.name);
        let value = ax_string(&node.value);
        let emit = !is_noise(&role, &name, interactive_only);

        if emit {
            let indent = "  ".repeat(depth);
            let mut line = format!("{indent}- {role}");
            if !name.is_empty() {
                line.push_str(&format!(" \"{}\"", name.replace('"', "'")));
            }
            let want_ref = is_interactive(&role) || (!name.is_empty() && !interactive_only);
            if want_ref {
                let id = format!("e{next_ref}");
                *next_ref += 1;
                line.push_str(&format!(" [ref={id}]"));
                let backend = node.backend_dom_node_id.as_ref().map(|b| *b.inner());
                refs.insert(
                    id,
                    RefEntry {
                        role: role_key(&role),
                        name: name.clone(),
                        backend_dom_node_id: backend,
                    },
                );
            }
            if !value.is_empty() && is_interactive(&role) {
                line.push_str(&format!(" value=\"{}\"", value.replace('"', "'")));
            }
            lines.push(line);
        }

        let child_depth = if emit { depth + 1 } else { depth };
        if let Some(children) = &node.child_ids {
            for cid in children {
                if let Some(child) = by_id.get(cid.inner().as_str()) {
                    walk(
                        child,
                        by_id,
                        child_depth,
                        interactive_only,
                        next_ref,
                        refs,
                        lines,
                    );
                }
            }
        }
    }

    if roots.is_empty() {
        for n in nodes {
            walk(
                n,
                &by_id,
                0,
                interactive_only,
                &mut next_ref,
                &mut refs,
                &mut lines,
            );
        }
    } else {
        for r in roots {
            walk(
                r,
                &by_id,
                0,
                interactive_only,
                &mut next_ref,
                &mut refs,
                &mut lines,
            );
        }
    }

    // Deduplicate accidental double-walk if CDP returns flat + nested oddly:
    // (roots path already exclusive). Keep a soft cap for model context.
    const MAX_LINES: usize = 400;
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        lines.push("… (snapshot truncated)".into());
    }

    LeanSnapshot {
        text: if lines.is_empty() {
            "(empty accessibility tree)".into()
        } else {
            lines.join("\n")
        },
        refs,
    }
}

/// Resolve backend id helper for session actions.
pub fn backend_id(entry: &RefEntry) -> Option<BackendNodeId> {
    entry.backend_dom_node_id.map(BackendNodeId::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chromiumoxide::cdp::browser_protocol::accessibility::{
        AxNode, AxNodeId, AxValue, AxValueType,
    };

    fn ax_val(s: &str) -> AxValue {
        AxValue {
            r#type: AxValueType::String,
            value: Some(serde_json::Value::String(s.into())),
            related_nodes: None,
            sources: None,
        }
    }

    fn node(id: &str, role: &str, name: &str, children: &[&str], backend: Option<i64>) -> AxNode {
        AxNode {
            node_id: AxNodeId::new(id),
            ignored: false,
            ignored_reasons: None,
            role: Some(ax_val(role)),
            chrome_role: None,
            name: Some(ax_val(name)),
            description: None,
            value: None,
            properties: None,
            parent_id: None,
            child_ids: if children.is_empty() {
                None
            } else {
                Some(children.iter().map(|c| AxNodeId::new(*c)).collect())
            },
            backend_dom_node_id: backend.map(BackendNodeId::new),
            frame_id: None,
        }
    }

    #[test]
    fn parse_ref_formats() {
        assert_eq!(parse_ref("@e1").as_deref(), Some("e1"));
        assert_eq!(parse_ref("ref=e2").as_deref(), Some("e2"));
        assert_eq!(parse_ref("e3").as_deref(), Some("e3"));
    }

    #[test]
    fn lean_interactive_assigns_refs() {
        let nodes = vec![
            node("1", "RootWebArea", "", &["2", "3"], None),
            node("2", "button", "Submit", &[], Some(10)),
            node("3", "textbox", "Email", &[], Some(11)),
            node("4", "StaticText", "noise", &[], None),
        ];
        // 4 is not linked as child of root — still shouldn't get a ref in interactive mode
        // when walked only via roots; unlinked StaticText becomes a root but is noise.
        let snap = build_lean_snapshot(&nodes, true);
        assert!(
            snap.text.contains("button \"Submit\" [ref=e1]"),
            "{}",
            snap.text
        );
        assert!(
            snap.text.contains("textbox \"Email\" [ref=e2]"),
            "{}",
            snap.text
        );
        assert!(!snap.text.contains("StaticText"), "{}", snap.text);
        assert_eq!(snap.refs["e1"].backend_dom_node_id, Some(10));
        assert_eq!(snap.refs["e2"].name, "Email");
    }
}
