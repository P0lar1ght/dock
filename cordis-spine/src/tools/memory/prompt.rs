//! `<memory>` block body (Dock paths). Injected as an
//! `agent/step-start` reminder, not a system-prompt section — see
//! `render_reminder_body` in the parent module.

use dock_memory::layout::{MemoryRoot, MemoryScope};
use dock_memory::manifest;

/// Soft cap per scope's injected MEMORY.md body (chars).
pub const MANIFEST_INJECT_CAP: usize = 6_000;

/// Build the `<memory>…</memory>` block when memory is enabled.
pub fn memory_section_body(root: &MemoryRoot) -> String {
    let global_path = root.global.root.display().to_string();
    let workspace_path = root.workspace.root.display().to_string();

    let mut body = String::new();
    body.push_str("<memory>\n");
    body.push_str(
        "记忆是用户控制的文件系统，记以前会话里学到的东西。下面注入的是每个范围生成的 `MEMORY.md` 索引，不要自己去读 `MEMORY.md`。开始一块工作前，先读标题对得上的主题文件。只有和过去的工作明显无关时才跳过记忆。本轮对话里的用户指令压过记忆；标成过去代理决定的笔记是记录，不是规则，要对照当前代码树核对。\n\n",
    );
    body.push_str("回忆优先用 `memory_search` / `memory_get`。普通读写工具只能在下面允许的路径里碰记忆文件。\n\n");
    body.push_str("全局记忆，所有工作区共用：\n");
    body.push_str(&format!(
        "- `{global_path}/topics/` — 维护中的 Markdown 笔记\n"
    ));
    body.push_str(&format!(
        "- `{global_path}/observations/_inbox/` — 新的 Markdown 观察\n"
    ));
    body.push_str(&format!(
        "- `{global_path}/MEMORY.md` — 生成的索引（只读）\n\n"
    ));
    body.push_str("工作区记忆，只属于当前工作区：\n");
    body.push_str(&format!(
        "- `{workspace_path}/topics/` — 维护中的 Markdown 笔记\n"
    ));
    body.push_str(&format!(
        "- `{workspace_path}/observations/_inbox/` — 新的 Markdown 观察\n"
    ));
    body.push_str(&format!(
        "- `{workspace_path}/MEMORY.md` — 生成的索引（只读）\n\n"
    ));
    body.push_str(
        "`topics/` 放可复用的稳定事实：偏好、约定、架构、决定、反复用的流程。`observations/_inbox/` 放以后可能整理进主题的新观察。`MEMORY.md` 是有长度上限的生成索引，下面已经注入，不要直接改。\n\n",
    );
    body.push_str(
        "只允许写 `topics/` 或 `observations/_inbox/` 下的 `.md`。生成的索引、归档、数据库和其他内部文件是保护的。\n\n",
    );
    body.push_str(
        "把记忆当历史，不当成当前事实。路径、命令、仓库状态、外部事实和其他会变的说法，先用工具核对再依赖。\n\n",
    );

    append_scope_manifest(&mut body, "全局", &root.global, MemoryScope::Global);
    body.push('\n');
    append_scope_manifest(&mut body, "工作区", &root.workspace, MemoryScope::Workspace);
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
    body.push_str(&format!("## {label}记忆索引\n"));
    body.push_str(&format!("**Scope root:** `{}`\n\n", scope.root.display()));
    match manifest::read_manifest_body(scope, MANIFEST_INJECT_CAP) {
        Some((text, truncated)) => {
            body.push_str(&text);
            if truncated {
                body.push_str("\n\n…（已截断；要看全文直接打开磁盘上的文件）\n");
            }
            body.push('\n');
        }
        None => {
            body.push_str(&format!(
                "（`{}` 还没有 MEMORY.md。记住、flush、dream 或建好目录之后才会有索引）\n",
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
        assert!(body.contains("全局记忆索引"));
        assert!(body.contains("工作区记忆索引"));
    }
}
