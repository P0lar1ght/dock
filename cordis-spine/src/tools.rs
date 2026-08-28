use std::sync::Arc;

use cordis::{plugin, Context, Inject, Plugin};

use crate::names::{TOOLS, TOOLS_EXECUTE, TOOLS_MCP};
use crate::runtime::BoxFuture;
use crate::types::{ToolCall, ToolResult, ToolSpec};

/// Extra `"tools.mcp"` dispatcher. Looked up live from `"tools"`; do not store the handle.
pub trait ExtraTools: Send + Sync {
    fn handles(&self, name: &str) -> bool;
    fn specs(&self) -> Vec<ToolSpec>;
    fn execute(&self, call: ToolCall) -> BoxFuture<'_, ToolResult>;
}

/// Named service wrapper so `"tools.mcp"` can be `get::<ExtraToolsHandle>`.
#[derive(Clone)]
pub struct ExtraToolsHandle(Arc<dyn ExtraTools>);

impl ExtraToolsHandle {
    pub fn new(inner: impl ExtraTools + 'static) -> Self {
        Self(Arc::new(inner))
    }

    pub fn handles(&self, name: &str) -> bool {
        self.0.handles(name)
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.0.specs()
    }

    pub fn execute(&self, call: ToolCall) -> BoxFuture<'_, ToolResult> {
        self.0.execute(call)
    }
}

/// Named `tools` service. Echo backend only in this standalone tree.
/// MCP is a live `"tools.mcp"` lookup, not captured here.
#[derive(Clone)]
pub struct Tools {
    ctx: Context,
}

impl Tools {
    pub fn echo(ctx: Context) -> Self {
        Self { ctx }
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs = Vec::new();
        if let Some(extra) = self.ctx.get::<ExtraToolsHandle>(TOOLS_MCP) {
            specs.extend(extra.specs());
        }
        specs
    }

    pub async fn execute(&self, call: ToolCall) -> ToolResult {
        let result = if let Some(extra) = self.ctx.get::<ExtraToolsHandle>(TOOLS_MCP) {
            if extra.handles(&call.name) {
                extra.execute(call).await
            } else {
                self.dispatch_local(call)
            }
        } else {
            self.dispatch_local(call)
        };
        self.ctx
            .waterfall(TOOLS_EXECUTE, result.clone(), move || result)
    }

    fn dispatch_local(&self, call: ToolCall) -> ToolResult {
        ToolResult {
            call_id: call.id,
            name: call.name,
            content: call.arguments,
        }
    }
}

pub fn tools() -> Plugin {
    plugin("tools", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(TOOLS, Tools::echo(ctx.clone()))?))
    })
}
