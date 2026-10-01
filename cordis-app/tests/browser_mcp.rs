//! 内置浏览器 MCP 的端到端：登记真的 `dock` 二进制，`install_app` 按内置行拉起
//! `dock mcp browser`，工具经 `search_tool` 找得到、经 `use_tool` 调得通，服务器的
//! `isError` 落到 [`ToolResult::is_error`]。不需要 Chrome（只打「还没 browser_open」）。

use std::time::{Duration, Instant};

use cordis_spine::{
    install_app, Browser, BrowserState, Mcp, ToolCall, Tools, BROWSER, BROWSER_MCP_SERVER, MCP,
    TOOLS,
};

fn isolated_home() {
    let home = tempfile::tempdir().unwrap().keep();
    std::env::set_var("DOCK_HOME", &home);
    std::env::set_var("DOCK_CUA_DRIVER", "off");
    std::env::remove_var("DOCK_BROWSER_MCP");
}

async fn wait_connected(root: &cordis::Context) {
    let mcp = root.require::<Mcp>(MCP).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let list = mcp.list();
        if let Some(row) = list.iter().find(|s| s.name == BROWSER_MCP_SERVER) {
            if row.ok && !row.tools.is_empty() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "browser MCP never connected: {} ({})",
                row.detail,
                row.command
            );
        } else {
            assert!(Instant::now() < deadline, "no [mcp_servers.browser] row");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn builtin_browser_mcp_connects_and_reports_errors() {
    isolated_home();
    cordis_spine::register_builtin_browser_mcp(env!("CARGO_BIN_EXE_dock").into());
    let root = cordis::Context::new();
    install_app(&root).await.unwrap();
    wait_connected(&root).await;

    let browser = root.require::<Browser>(BROWSER).unwrap();
    assert_eq!(browser.state(), BrowserState::Connected { tools: 21 });

    let tools = root.require::<Tools>(TOOLS).unwrap();
    // 模型那边看不见（和别的 MCP 一样经 search_tool），搜得到。
    assert!(!tools
        .specs_for_model()
        .iter()
        .any(|s| s.name.starts_with("mcp_browser__")));
    let found = tools
        .execute(ToolCall {
            id: "st".into(),
            name: "search_tool".into(),
            arguments: r#"{"query":"browser snapshot accessibility refs","limit":5}"#.into(),
        })
        .await;
    assert!(
        found.content.contains("mcp_browser__browser_snapshot"),
        "{}",
        found.content
    );

    // 还没 browser_open：服务器回 isError，结果标失败，不靠猜文本。
    let snap = tools
        .execute(ToolCall {
            id: "ut".into(),
            name: "use_tool".into(),
            arguments: r#"{"tool_name":"mcp_browser__browser_snapshot","tool_input":{}}"#.into(),
        })
        .await;
    assert!(snap.is_error, "{}", snap.content);
    assert!(snap.content.contains("browser_open"), "{}", snap.content);
}
