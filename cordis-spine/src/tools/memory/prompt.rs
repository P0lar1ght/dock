//! System-prompt `<memory>` section body (Dock paths; Grok-aligned copy).

use dock_memory::layout::{MemoryRoot, MemoryScope};
use dock_memory::manifest;

/// Soft cap per scope's injected MEMORY.md body (chars).
pub const MANIFEST_INJECT_CAP: usize = 6_000;

/// Build the `<memory>…</memory>` system section when memory is enabled.
pub fn memory_section_body(root: &MemoryRoot) -> String {
    let global_path = root.global.root.display().to_string();
    let workspace_path = root.workspace.root.display().to_string();

    let mut body = String::new();
    body.push_str("<memory>\n");
    body.push_str(
        "Memory is a user-controlled filesystem knowledge base of what earlier sessions learned. \
The memory index injected below is each scope's generated `MEMORY.md`, so never read `MEMORY.md` itself. \
Before starting work in an area, read the topic files whose titles cover it. Skip memory only for requests \
with no plausible overlap with past work. The user's instructions in this conversation override memory; \
a note marked as a past agent decision is a record, not a rule, so verify it against the current tree.\n\n",
    );
    body.push_str("Prefer `memory_search` / `memory_get` for recall. Ordinary read/write tools may touch memory paths only within the allowed bounds below.\n\n");
    body.push_str("Global memory, shared across workspaces:\n");
    body.push_str(&format!(
        "- `{global_path}/topics/` — maintained Markdown notes\n"
    ));
    body.push_str(&format!(
        "- `{global_path}/observations/_inbox/` — new Markdown observations\n"
    ));
    body.push_str(&format!(
        "- `{global_path}/MEMORY.md` — generated index (read-only)\n\n"
    ));
    body.push_str("Workspace memory, specific to this workspace:\n");
    body.push_str(&format!(
        "- `{workspace_path}/topics/` — maintained Markdown notes\n"
    ));
    body.push_str(&format!(
        "- `{workspace_path}/observations/_inbox/` — new Markdown observations\n"
    ));
    body.push_str(&format!(
        "- `{workspace_path}/MEMORY.md` — generated index (read-only)\n\n"
    ));
    body.push_str(
        "`topics/` holds durable preferences, conventions, architecture, decisions, recurring workflows, \
and other facts worth reusing. `observations/_inbox/` holds new observations that may later be consolidated into topics. \
`MEMORY.md` is a bounded generated index; it is already injected below, and you must NEVER edit it directly.\n\n",
    );
    body.push_str(
        "Writes are allowed only to `.md` files under `topics/` or `observations/_inbox/`; generated indexes, \
archives, databases, and other internals are protected.\n\n",
    );
    body.push_str(
        "Treat memory as historical context, not current truth. Verify paths, commands, repository state, \
external facts, and other changeable claims with live tools before relying on them.\n\n",
    );

    append_scope_manifest(&mut body, "Global", &root.global, MemoryScope::Global);
    body.push('\n');
    append_scope_manifest(
        &mut body,
        "Workspace",
        &root.workspace,
        MemoryScope::Workspace,
    );
    body.push_str("</memory>");
    body
}

fn append_scope_manifest(
    body: &mut String,
    label: &str,
    scope: &dock_memory::layout::ScopePaths,
    which: MemoryScope,
) {
    let path = manifest::memory_md_path(scope);
    body.push_str(&format!("## {label} memory manifest\n"));
    body.push_str(&format!("**Scope root:** `{}`\n\n", scope.root.display()));
    match manifest::read_manifest_body(scope, MANIFEST_INJECT_CAP) {
        Some((text, truncated)) => {
            body.push_str(&text);
            if truncated {
                body.push_str("\n\n…(truncated; open the file on disk for the full index)\n");
            }
            body.push('\n');
        }
        None => {
            body.push_str(&format!(
                "(no MEMORY.md yet at `{}` — index appears after remember/flush/dream or layout ensure)\n",
                path.display()
            ));
            let _ = which; // keep signature aligned for future scope-specific notes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn section_includes_paths_and_tags() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/mem-prompt"));
        root.ensure_layout().unwrap();
        let body = memory_section_body(&root);
        assert!(body.starts_with("<memory>"));
        assert!(body.ends_with("</memory>"));
        assert!(body.contains("memory_search"));
        assert!(body.contains("MEMORY.md"));
        assert!(body.contains("observations/_inbox"));
        assert!(body.contains("Global memory manifest"));
        assert!(body.contains("Workspace memory manifest"));
    }
}
