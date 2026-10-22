//! 白板 MCP 服务走一遍真协议（duplex 代替 stdio）：握手、工具表、界面资源、按项目读写。

use std::sync::Arc;

use cordis_apps::stdio::serve;
use cordis_apps::whiteboard::{Whiteboard, RESOURCE_URI};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Client {
    tx: tokio::io::DuplexStream,
    rx: tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    next: u64,
}

impl Client {
    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params });
        self.tx
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
        loop {
            let line = self.rx.next_line().await.unwrap().expect("server closed");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(self.next) {
                return v;
            }
        }
    }

    async fn tool(&mut self, name: &str, args: Value, cwd: &str) -> Value {
        let r = self
            .call(
                "tools/call",
                json!({ "name": name, "arguments": args, "_meta": { "dock/cwd": cwd, "dock/sessionId": "s1" } }),
            )
            .await;
        r["result"].clone()
    }
}

fn start(root: std::path::PathBuf) -> Client {
    let (client_w, server_r) = tokio::io::duplex(1 << 20);
    let (server_w, client_r) = tokio::io::duplex(1 << 20);
    tokio::spawn(serve(Arc::new(Whiteboard::new(root)), server_r, server_w));
    Client {
        tx: client_w,
        rx: BufReader::new(client_r).lines(),
        next: 0,
    }
}

#[tokio::test]
async fn whiteboard_speaks_mcp_apps_and_keeps_one_scene_per_project() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = start(dir.path().to_path_buf());

    let hello = c.call("server/discover", json!({})).await;
    assert!(hello["result"]["supportedVersions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "2026-07-28"));
    let init = c
        .call("initialize", json!({ "protocolVersion": "2025-06-18" }))
        .await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    let tools = c.call("tools/list", json!({})).await;
    let tools = tools["result"]["tools"].as_array().unwrap().clone();
    let by = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap().clone();
    assert_eq!(
        by("whiteboard_edit")["_meta"]["ui"]["resourceUri"],
        RESOURCE_URI
    );
    assert!(by("whiteboard_read").get("_meta").is_none(), "读不挂界面");
    assert_eq!(
        by("whiteboard_save")["_meta"]["ui"]["visibility"],
        json!(["app"])
    );

    let res = c
        .call("resources/read", json!({ "uri": RESOURCE_URI }))
        .await;
    let item = &res["result"]["contents"][0];
    assert_eq!(item["mimeType"], "text/html;profile=mcp-app");
    assert!(item["text"].as_str().unwrap().contains("<html"));
    assert!(item["_meta"]["ui"]["csp"]["resourceDomains"].is_array());
    let missing = c
        .call("resources/read", json!({ "uri": "ui://nope" }))
        .await;
    assert!(missing.get("error").is_some());

    // Agent 在项目 A 加图，项目 B 看不到。
    let edited = c
        .tool(
            "whiteboard_edit",
            json!({ "add": [{ "type": "rectangle", "id": "a", "text": "登录" }] }),
            "/work/a",
        )
        .await;
    assert_eq!(edited["isError"], false, "{edited}");
    assert_eq!(edited["structuredContent"]["added"], json!(["a"]));
    let other = c.tool("whiteboard_read", json!({}), "/work/b").await;
    assert!(
        other["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("空的"),
        "{other}"
    );

    // 界面载入、画好骨架存回来；之后 Agent 读到的是完整元素。
    let loaded = c.tool("whiteboard_load", json!({}), "/work/a").await;
    let rev = loaded["structuredContent"]["rev"].as_u64().unwrap();
    assert_eq!(loaded["structuredContent"]["pending"][0]["id"], "a");
    let unchanged = c
        .tool("whiteboard_load", json!({ "sinceRev": rev }), "/work/a")
        .await;
    assert_eq!(unchanged["structuredContent"]["unchanged"], true);
    let el = json!({ "id": "a", "type": "rectangle", "x": 100, "y": 100, "width": 180, "height": 80,
                     "version": 2, "versionNonce": 1, "isDeleted": false });
    let saved = c
        .tool(
            "whiteboard_save",
            json!({ "elements": [el], "consumed": ["a"] }),
            "/work/a",
        )
        .await;
    assert_eq!(saved["structuredContent"]["changed"], true, "{saved}");
    let read = c.tool("whiteboard_read", json!({}), "/work/a").await;
    let text = read["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("rectangle id=a") && !text.contains("待画"),
        "{text}"
    );

    let bad = c
        .tool("whiteboard_edit", json!({ "delete": ["ghost"] }), "/work/a")
        .await;
    assert_eq!(bad["isError"], true);
    assert!(dir.path().join("work-a/scene.json").exists());
}
