//! `tool-canvas`：四颗工具经工具表跑一遍，落在会话目录的 `canvas/` 下。

use cordis::Context;
use cordis_spine::{Sessions, ToolCall, Tools, SESSIONS, TOOLS};

async fn call(tools: &Tools, name: &str, args: serde_json::Value) -> cordis_spine::ToolResult {
    tools
        .execute(ToolCall {
            id: format!("c-{name}"),
            name: name.into(),
            arguments: args.to_string(),
        })
        .await
}

#[tokio::test]
async fn canvas_tools_write_versions_into_the_session_dir() {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("DOCK_HOME", home.path());
    std::env::set_var("DOCK_CUA_DRIVER", "off");
    let root = Context::new();
    cordis_spine::install_app(&root).await.unwrap();
    let tools = root.require::<Tools>(TOOLS).unwrap();

    let model: Vec<String> = tools
        .specs_for_model()
        .into_iter()
        .map(|s| s.name)
        .collect();
    // 画布按需加载：不进模型工具表，模型经 `use_tool` 调用。
    for name in ["canvas_create", "canvas_edit", "canvas_data", "canvas_read"] {
        assert!(
            !model.iter().any(|n| n == name),
            "{name} 应按需加载：{model:?}"
        );
    }

    // 不落盘的会话没地方存。走 `use_tool`，和模型平时的调用路径一样。
    let r = call(
        &tools,
        "use_tool",
        serde_json::json!({"tool_name": "canvas_create", "tool_input": {"title": "t", "html": "<p>"}}),
    )
    .await;
    assert!(r.is_error, "{}", r.content);
    assert!(r.content.contains("不落盘"), "{}", r.content);

    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    sessions.attach_disk();
    let dir = sessions
        .disk_session_dir()
        .expect("attach_disk 之后有会话目录");

    let r = call(
        &tools,
        "canvas_create",
        serde_json::json!({"title": "销售", "html": "<h1>Q1</h1>", "data": {"q": [1, 2]}}),
    )
    .await;
    assert!(!r.is_error, "{}", r.content);
    assert!(r.content.contains("canvas-1"), "{}", r.content);
    assert!(dir.join("canvas/canvas-1/v1.html").is_file());
    assert!(dir.join("canvas/canvas-1/data.json").is_file());

    let r = call(
        &tools,
        "canvas_edit",
        serde_json::json!({"id": "canvas-1", "old_string": "Q1", "new_string": "Q2", "note": "改季度"}),
    )
    .await;
    assert!(r.content.contains("v2"), "{}", r.content);

    let r = call(
        &tools,
        "canvas_edit",
        serde_json::json!({"id": "canvas-1", "old_string": "Q9", "new_string": "x", "note": "n"}),
    )
    .await;
    assert!(r.is_error && r.content.contains("没找到"), "{}", r.content);

    let r = call(
        &tools,
        "canvas_data",
        serde_json::json!({"id": "canvas-1", "data": {"q": [3]}}),
    )
    .await;
    assert!(r.content.contains("仍是 v2"), "{}", r.content);

    let r = call(&tools, "canvas_read", serde_json::json!({"id": "canvas-1"})).await;
    assert!(r.content.contains("<h1>Q2</h1>"), "{}", r.content);
    assert!(r.content.contains(r#"{"q":[3]}"#), "{}", r.content);
    assert!(r.content.contains("v2 改季度"), "{}", r.content);

    let r = call(&tools, "canvas_read", serde_json::json!({})).await;
    assert!(r.content.contains("canvas-1「销售」"), "{}", r.content);
}
