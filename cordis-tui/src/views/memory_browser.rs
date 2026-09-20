//! `/memory` read-only browser (list + preview) rendered as Notice body.

use dock_memory::browse::list_memory_files;
use dock_memory::layout::MemoryRoot;

/// Build a markdown-ish text body: file list with inline previews.
pub fn render_text() -> String {
    let cfg = cordis_base::config::load_memory_config();
    if !cfg.enabled {
        return "Memory is disabled.\n\nEnable with `[memory] enabled = true` in config.toml\nor `DOCK_MEMORY=1`.\n\nLegacy files under ~/.dock/memory remain readable when enabled.".into();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let root = MemoryRoot::open_default(&cwd);
    let files = list_memory_files(&root);
    if files.is_empty() {
        return format!(
            "No memory files yet.\n\nLayout: {}\n  global/{{topics,observations}}/\n  workspace-{}/{{topics,observations}}/\n\nUse /remember, /flush, or /dream to create entries.",
            root.home.display(),
            root.slug
        );
    }
    let mut out = format!(
        "Memory root: {}\nWorkspace: workspace-{}\nFiles: {}\n\n",
        root.home.display(),
        root.slug,
        files.len()
    );
    for entry in &files {
        out.push_str(&format!("## {}\n", entry.label));
        out.push_str(&format!("`{}`\n\n", entry.path.display()));
        match std::fs::read_to_string(&entry.path) {
            Ok(body) => {
                let preview: String = body.chars().take(800).collect();
                out.push_str(&preview);
                if body.chars().count() > 800 {
                    out.push_str("\n…");
                }
                out.push_str("\n\n");
            }
            Err(e) => out.push_str(&format!("(unreadable: {e})\n\n")),
        }
    }
    out
}
