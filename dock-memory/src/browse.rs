//! Read-only listing for `/memory` overlay.

use std::path::{Path, PathBuf};

use crate::layout::{MemoryRoot, MemoryScope};

#[derive(Debug, Clone)]
pub struct MemoryFileEntry {
    pub scope: MemoryScope,
    pub kind: &'static str,
    pub path: PathBuf,
    pub label: String,
}

/// List browsable markdown under global + workspace topics/observations.
pub fn list_memory_files(root: &MemoryRoot) -> Vec<MemoryFileEntry> {
    let mut out = Vec::new();
    for (scope, paths) in [
        (MemoryScope::Global, &root.global),
        (MemoryScope::Workspace, &root.workspace),
    ] {
        let md = paths.root.join("MEMORY.md");
        if md.is_file() {
            let scope_s = match scope {
                MemoryScope::Global => "global",
                MemoryScope::Workspace => "workspace",
            };
            out.push(MemoryFileEntry {
                scope,
                kind: "index",
                label: format!("{scope_s}/MEMORY.md"),
                path: md,
            });
        }
        collect_dir(&mut out, scope, "topics", &paths.topics);
        // Inbox lives at observations/_inbox/; include that segment in labels
        // (list + delete-confirm flash), matching on-disk layout.
        collect_dir(&mut out, scope, "observations/_inbox", &paths.inbox);
        // Legacy flat observations/*.md (pre-migrate / collision left behind).
        collect_flat_obs(&mut out, scope, &paths.observations);
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

fn collect_dir(out: &mut Vec<MemoryFileEntry>, scope: MemoryScope, kind: &'static str, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dir(out, scope, kind, &path);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let scope_s = match scope {
            MemoryScope::Global => "global",
            MemoryScope::Workspace => "workspace",
        };
        out.push(MemoryFileEntry {
            scope,
            kind,
            label: format!("{scope_s}/{kind}/{name}"),
            path,
        });
    }
}

fn collect_flat_obs(out: &mut Vec<MemoryFileEntry>, scope: MemoryScope, observations: &Path) {
    let Ok(entries) = std::fs::read_dir(observations) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let scope_s = match scope {
            MemoryScope::Global => "global",
            MemoryScope::Workspace => "workspace",
        };
        let label = format!("{scope_s}/observations/{name}");
        if out.iter().any(|e| e.label == label || e.path == path) {
            continue;
        }
        out.push(MemoryFileEntry {
            scope,
            kind: "observations",
            label,
            path,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_topics_and_observations() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/browse-proj"));
        root.ensure_layout().unwrap();
        std::fs::write(root.global.topics.join("prefs.md"), "# prefs\n").unwrap();
        std::fs::write(root.workspace.inbox.join("flush-1.md"), "## x\n").unwrap();
        let list = list_memory_files(&root);
        // 2 notes + MEMORY.md for each scope created by ensure_layout.
        assert!(list.len() >= 2, "len={}", list.len());
        assert!(list.iter().any(|e| e.label.contains("prefs")));
        assert!(list.iter().any(|e| e.label.contains("flush-1")));
        assert!(list.iter().any(|e| e.label.ends_with("MEMORY.md")));
        let inbox = list
            .iter()
            .find(|e| e.label.contains("flush-1"))
            .expect("inbox observation");
        assert!(
            inbox.label.contains("observations/_inbox/"),
            "inbox label must include _inbox segment, got {}",
            inbox.label
        );
    }
}
