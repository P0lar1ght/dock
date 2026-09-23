//! Model-facing `web_search` + `web_fetch`. DSH `tool-web`: one plugin, two registers.

mod config;
mod domain;
pub(crate) mod error;
mod fetch;
mod http;
pub(crate) mod ssrf;

use cordis::{plugin, Inject, Plugin};

use crate::names::TOOLS;
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

pub use config::WebFetchParams;
use fetch::{fetch_url, search_web};

const FETCH_PARAMS: &str = r#"{"type":"object","properties":{"url":{"type":"string","description":"The URL to fetch content from."}},"required":["url"]}"#;
const SEARCH_PARAMS: &str = r#"{"type":"object","properties":{"query":{"type":"string","description":"The search query to perform."},"allowed_domains":{"type":"array","items":{"type":"string"},"description":"Optional list of domains to restrict search to."}},"required":["query"]}"#;

/// 组合根便利函数：`[toolset.web_fetch]` → 运行时参数。
///
/// 读磁盘的是这里，不是插件体——`tool-web` 拿到的是已经定好的 `WebFetchParams`，
/// 自己不 live-read 环境（和 `tool-task` 收 `TaskConfig` 同一个形状）。
pub fn web_fetch_params() -> WebFetchParams {
    WebFetchParams::from_tool_config(&cordis_base::config::load_web_fetch_config())
}

pub fn tool_web() -> Plugin {
    plugin(
        "tool-web",
        Inject::from([TOOLS]),
        |ctx, params: &WebFetchParams| {
            let tools = ctx.require::<Tools>(TOOLS)?;
            let fetch_params = params.clone();
            let search_params = params.clone();
            let fetch: ToolBody = std::sync::Arc::new(move |call| {
                let params = fetch_params.clone();
                Box::pin(async move { web_fetch(call, &params).await })
            });
            let search: ToolBody = std::sync::Arc::new(move |call| {
                let params = search_params.clone();
                Box::pin(async move { web_search(call, &params).await })
            });
            own_registered(
            ctx,
            vec![
                tools.register(
                    ToolSpec {
                        name: "web_fetch".into(),
                        description: "Fetch the content of a specific URL and return it as markdown.\n\nIMPORTANT: web_fetch WILL FAIL for authenticated or private URLs (e.g. Google Docs, Confluence, Jira, GitHub private repos). Use specialized MCP tools for those instead.\n\nUsage notes:\n  - HTTP URLs will be automatically upgraded to HTTPS\n  - Long pages will be truncated to fit your context window".into(),
                        parameters_json: FETCH_PARAMS.into(),
                    },
                    fetch,
                )?,
                tools.register(
                    ToolSpec {
                        name: "web_search".into(),
                        description: "Search the web for up-to-date information, tailored for coding and software development tasks.".into(),
                        parameters_json: SEARCH_PARAMS.into(),
                    },
                    search,
                )?,
            ],
        )?;
            Ok(None)
        },
    )
}

fn arg_url(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get("url")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn arg_query(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    if let Some(q) = v
        .get("query")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
    {
        return Some(q.to_string());
    }
    v.get("queries")
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

async fn web_fetch(call: ToolCall, params: &WebFetchParams) -> ToolResult {
    let Some(url) = arg_url(&call.arguments) else {
        return tool_result(call, "Error: url is required");
    };
    match fetch_url(&url, params).await {
        Ok(body) => tool_result(call, body),
        Err(e) => tool_result(call, e.to_string()),
    }
}

async fn web_search(call: ToolCall, params: &WebFetchParams) -> ToolResult {
    let Some(query) = arg_query(&call.arguments) else {
        return tool_result(call, "Error: query is required");
    };
    match search_web(&query, params).await {
        Ok(body) => tool_result(call, body),
        Err(e) => tool_result(call, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::capability::CapabilityMode;
    use crate::names::CAPABILITY;
    use cordis::Context;

    /// 只读子代理（`deep-research` 的 researcher 就是）要能直接调 web_search / web_fetch。
    ///
    /// 只读档挡 `use_tool`（它能派发到写盘的按需工具），所以这两颗一旦改成
    /// `register_deferred`，只读子代理就既看不到、也叫不动它们，联网调研整条断掉。
    #[tokio::test]
    async fn read_only_children_keep_the_web_tools_on_their_sampler() {
        let _env = cordis_base::test_env::scoped().home();
        let ctx = Context::new();
        crate::install_without_llm(&ctx).await.unwrap();
        ctx.plugin(tool_web(), web_fetch_params())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let child = ctx.isolate(CAPABILITY);
        let _cap = child.provide(CAPABILITY, CapabilityMode::ReadOnly).unwrap();
        assert!(
            !CapabilityMode::ReadOnly.allows("use_tool"),
            "前提：只读档不给 use_tool"
        );

        let tools = ctx.require::<Tools>(TOOLS).unwrap();
        let names: Vec<String> = tools
            .specs_for_model_on(&child)
            .into_iter()
            .map(|s| s.name)
            .collect();
        for name in ["web_search", "web_fetch"] {
            assert!(
                names.iter().any(|n| n == name),
                "只读子代理必须直接看得到 {name}：{names:?}"
            );
        }
    }
}
