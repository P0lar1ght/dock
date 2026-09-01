pub mod capabilities;
pub mod client;
pub mod config;
pub mod config_source;
pub mod diagnostics;
pub mod dispatch;
pub mod documents;
pub mod format;
pub mod manager;
pub mod notify;
pub mod pending;
mod process;
pub mod pull;
pub mod refresh;
pub mod restart;
pub mod setup;
mod types;
pub mod workspace_open;

pub use dispatch::LspBackendAdapter;
#[allow(unused_imports)]
pub use manager::{drain_lsp_diagnostics, DiagnosticsSummary, LspManager};
pub use restart::restart_monitor;
pub use setup::{
    auto_setup as lsp_auto_setup, composer_fill as lsp_composer_fill,
    status_report as lsp_status_report, LspSetupReport, LspSetupScope,
};
#[allow(unused_imports)]
pub use types::{
    DiagnosticEntry, DiagnosticSeverityLevel, FileDiagnosticEntry, LspBackend, LspConfig,
    LspOperation, LspToolInput, LspToolResult,
};

// ── Shared types used across submodules ─────────────────────────────────

use std::path::Path;
use std::sync::Arc;

use async_lsp::lsp_types::{Position, TextDocumentIdentifier, TextDocumentPositionParams, Url};

/// How long a reader will wait for diagnostics to arrive after an edit before
/// reporting what it has.
///
/// This is the budget the whole after-edit diagnostics path is sized against:
/// anything scheduled to happen later than this — a pull retry, say — answers
/// after the reader has already given up. Kept here, next to the pieces that
/// have to agree on it, rather than as a number at the call site.
pub const DIAGNOSTICS_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("failed to spawn LSP server: {0}")]
    SpawnFailed(String),
    #[error("LSP server '{0}' timed out after {1:?}")]
    Timeout(String, std::time::Duration),
    #[error("LSP initialization failed: {0}")]
    InitFailed(String),
    #[error("LSP request failed: {0}")]
    RequestFailed(String),
    #[error("invalid file path")]
    InvalidPath,
}

pub type DiagnosticsNotify = Arc<tokio::sync::Notify>;
pub type LspMainLoop = async_lsp::MainLoop<async_lsp::router::Router<()>>;

pub fn file_uri(path: &Path) -> Result<Url, LspError> {
    Url::from_file_path(path).map_err(|_| LspError::InvalidPath)
}

pub fn text_document_position(
    path: &Path,
    line: u32,
    column: u32,
) -> Result<TextDocumentPositionParams, LspError> {
    Ok(TextDocumentPositionParams {
        text_document: TextDocumentIdentifier {
            uri: file_uri(path)?,
        },
        position: Position {
            line,
            character: column,
        },
    })
}

/// Cordis plugin: Grok `LspTool::run` over `"tools"`. Fail-open if no servers.
const LSP_UNAVAILABLE: &str = "LSP tool is unavailable. Configure ~/.dock/lsp.json or <cwd>/.dock/lsp.json and ensure the language server can start.";

pub fn tool_lsp() -> cordis::Plugin {
    use crate::names::{LSP, TOOLS, TOOLS_EXECUTE};
    use crate::tools::{own_registered, ToolBody, Tools};
    use crate::types::{ToolResult, ToolSpec};
    use cordis::{plugin, Inject};
    use notify::ToolNotificationHandle;
    use tokio::sync::Mutex as TokioMutex;

    const DESC: &str = "Code intelligence via language servers. Prefer over grep/read_file for understanding code.\n\
Operations: goToDefinition (jump to where a symbol is defined), findReferences (all usages of a symbol), hover (type info/docs at a position), goToImplementation (trait/interface implementations), documentSymbol (list all symbols in a file), workspaceSymbol (search symbols by name across the workspace — requires query parameter, not file_path).\n\
Requires file_path + line + character for position-based operations. line/character are 0-based (read_file's N→ markers are 1-based: subtract 1). file_path may be relative to cwd.";
    const PARAMS: &str = r#"{"type":"object","properties":{"operation":{"type":"string","enum":["goToDefinition","findReferences","hover","goToImplementation","documentSymbol","workspaceSymbol"],"description":"LSP operation."},"file_path":{"type":"string","description":"Path to the file. Absolute, or relative to cwd."},"line":{"type":"integer","description":"0-based line. read_file N→ is 1-based: subtract 1."},"character":{"type":"integer","description":"0-based UTF-16 column."},"query":{"type":"string","description":"Symbol query for workspaceSymbol."}},"required":["operation"]}"#;

    plugin("tool-lsp", Inject::from([TOOLS]), |ctx, _: &()| {
        // Servers are loaded at first use (live cwd / lsp.json / PATH defaults).
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let mgr = std::sync::Arc::new(TokioMutex::new(manager::LspManager::new(
            Default::default(),
            cwd,
            true,
            ToolNotificationHandle::noop(),
        )));
        let backend = dispatch::LspBackendAdapter::new(mgr);
        ctx.provide(LSP, backend)?;
        let _ = ctx.on_waterfall(TOOLS_EXECUTE, {
            let ctx = ctx.clone();
            move |result: ToolResult, _| {
                notify_edit_if_needed(&ctx, &result);
                result
            }
        })?;
        let tools = ctx.require::<Tools>(TOOLS)?;
        let body: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { lsp_run(&ctx, call).await })
            })
        };
        own_registered(
            ctx,
            vec![tools.register(
                ToolSpec {
                    name: "lsp".into(),
                    description: DESC.into(),
                    parameters_json: PARAMS.into(),
                },
                body,
            )?],
        )?;
        Ok(None)
    })
}

fn notify_edit_if_needed(ctx: &cordis::Context, result: &crate::types::ToolResult) {
    if !matches!(result.name.as_str(), "search_replace" | "write_file") {
        return;
    }
    let Some(path) = edited_path_from_result(&result.content) else {
        return;
    };
    let Some(handle) = ctx.get::<LspBackendAdapter>(crate::names::LSP) else {
        return;
    };
    handle.ensure_started_background();
    tokio::spawn(async move {
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            handle.notify_file_changed(&path, &content).await;
        }
    });
}

fn edited_path_from_result(content: &str) -> Option<std::path::PathBuf> {
    let line = content.lines().next()?;
    for prefix in ["updated ", "created ", "wrote "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            if rest.starts_with("Error") {
                continue;
            }
            return Some(config::resolve_tool_path(rest));
        }
    }
    None
}

async fn lsp_run(ctx: &cordis::Context, call: crate::types::ToolCall) -> crate::types::ToolResult {
    use crate::names::LSP;
    use crate::tools::tool_result;
    // Copied from Grok `LspTool::run`.
    let Some(handle) = ctx.get::<LspBackendAdapter>(LSP) else {
        return tool_result(call, LSP_UNAVAILABLE);
    };
    let input: types::LspToolInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: {e}")),
    };
    let result = LspBackend::dispatch(&*handle, &input).await;
    tool_result(call, result.text)
}
