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
    // items 不再塞 13 个 properties，避免模型把数组包成 {item:...}
    let add_items = &by("whiteboard_edit")["inputSchema"]["properties"]["add"]["items"];
    assert_eq!(add_items["type"], "object");
    assert_eq!(add_items["additionalProperties"], true);
    assert!(add_items.get("properties").is_none(), "{add_items}");
    for name in [
        "whiteboard_group",
        "whiteboard_align",
        "whiteboard_style",
        "whiteboard_layer",
        "whiteboard_fit_text",
        "whiteboard_distribute",
        "whiteboard_layout",
        "whiteboard_place",
        "whiteboard_graph",
        "whiteboard_find",
        "whiteboard_duplicate",
    ] {
        assert!(tools.iter().any(|t| t["name"] == name), "缺工具 {name}");
    }
    let read_props = &by("whiteboard_read")["inputSchema"]["properties"];
    assert_eq!(read_props["overview"]["type"], "boolean");
    assert_eq!(read_props["ids"]["type"], "array");
    assert_eq!(read_props["ids"]["items"]["type"], "string");
    assert!(
        read_props["ids"].get("properties").is_none(),
        "{read_props}"
    );
    assert_eq!(read_props["related"]["type"], "string");
    let nodes_items = &by("whiteboard_graph")["inputSchema"]["properties"]["nodes"]["items"];
    assert_eq!(nodes_items["additionalProperties"], true);
    assert!(nodes_items.get("properties").is_none(), "{nodes_items}");

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

    // 恰好 {item:[...]} 的包装会被剥掉
    let wrapped = c
        .tool(
            "whiteboard_edit",
            json!({ "add": { "item": [{ "type": "ellipse", "id": "e1", "text": "圆" }] } }),
            "/work/a",
        )
        .await;
    assert_eq!(wrapped["isError"], false, "{wrapped}");
    let wtext = wrapped["content"][0]["text"].as_str().unwrap();
    assert!(wtext.contains("已自动去掉 item 包装层"), "{wtext}");
    assert_eq!(wrapped["structuredContent"]["unwrappedItem"], true);
    // 工具结果只讲这次改了什么（全量场景交给 whiteboard_read）。
    assert_eq!(wrapped["structuredContent"]["added"], json!(["e1"]));
    assert!(
        wtext.contains("ellipse id=e1"),
        "结果里要说清新图形：{wtext}"
    );
    assert!(
        !wrapped["structuredContent"]["shapes"].is_array(),
        "不再回全量场景：{wrapped}"
    );

    let read2 = c.tool("whiteboard_read", json!({}), "/work/a").await;
    assert!(
        read2["structuredContent"]["patches"]
            .as_array()
            .unwrap()
            .len()
            >= 2,
        "{read2}"
    );

    let bad = c
        .tool("whiteboard_edit", json!({ "delete": ["ghost"] }), "/work/a")
        .await;
    assert_eq!(bad["isError"], true);
    assert!(std::fs::read_dir(dir.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("work-a-")));
}

/// 终端和桌面端各拉起一个白板服务，同时往同一个项目写：读改写由文件锁串起来，谁的都不丢。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_servers_writing_the_same_project_do_not_lose_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = start(dir.path().to_path_buf());
    let mut b = start(dir.path().to_path_buf());
    let edits = |c: &'static str| {
        (0..15)
            .map(move |i| json!({ "add": [{ "type": "rectangle", "id": format!("{c}{i}"), "x": 0, "y": 0 }] }))
            .collect::<Vec<_>>()
    };
    let run_a = async {
        for e in edits("a") {
            let r = a.tool("whiteboard_edit", e, "/work/shared").await;
            assert_eq!(r["isError"], false, "{r}");
        }
        a
    };
    let run_b = async {
        for e in edits("b") {
            let r = b.tool("whiteboard_edit", e, "/work/shared").await;
            assert_eq!(r["isError"], false, "{r}");
        }
        b
    };
    let (mut a, _b) = tokio::join!(run_a, run_b);
    let read = a.tool("whiteboard_read", json!({}), "/work/shared").await;
    assert_eq!(
        read["structuredContent"]["shapes"]
            .as_array()
            .unwrap()
            .len(),
        30,
        "两边的 30 个都在"
    );
    assert_eq!(
        read["structuredContent"]["rev"], 30,
        "每次编辑一版，不会重号"
    );
}

/// 界面工具：画好的完整元素（版本压过服务端，模拟界面存回来的）。
fn ui_el(id: &str) -> Value {
    json!({ "id": id, "type": "rectangle", "x": 0, "y": 0, "width": 100, "height": 50,
            "version": 2, "versionNonce": 1, "isDeleted": false })
}

async fn three_shapes(c: &mut Client, cwd: &str) {
    let added = c
        .tool(
            "whiteboard_edit",
            json!({ "add": [
                { "type": "rectangle", "id": "a" },
                { "type": "rectangle", "id": "b" },
                { "type": "rectangle", "id": "c" },
            ] }),
            cwd,
        )
        .await;
    assert_eq!(added["isError"], false, "{added}");
    let els: Vec<Value> = ["a", "b", "c"].iter().map(|id| ui_el(id)).collect();
    let saved = c
        .tool(
            "whiteboard_save",
            json!({ "elements": els, "consumed": ["a", "b", "c"] }),
            cwd,
        )
        .await;
    assert_eq!(saved["isError"], false, "{saved}");
}

async fn load_ids(c: &mut Client, cwd: &str) -> Vec<String> {
    let loaded = c.tool("whiteboard_load", json!({}), cwd).await;
    loaded["structuredContent"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect()
}

/// 回归：layer 要真改「渲染顺序」（Excalidraw 按数组顺序画，index 只是持久化的 z 序键）。
#[tokio::test]
async fn whiteboard_layer_moves_array_order_and_index() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = start(dir.path().to_path_buf());
    let cwd = "/work/layer";
    three_shapes(&mut c, cwd).await;

    let layered = c
        .tool(
            "whiteboard_layer",
            json!({ "ids": ["a"], "to": "front" }),
            cwd,
        )
        .await;
    assert_eq!(layered["isError"], false, "{layered}");

    let ids = load_ids(&mut c, cwd).await;
    assert_eq!(
        ids,
        vec!["b", "c", "a"],
        "front 之后 a 要排在最后（数组顺序=渲染顺序）"
    );

    let loaded = c.tool("whiteboard_load", json!({}), cwd).await;
    let elements = loaded["structuredContent"]["elements"].as_array().unwrap();
    let idx = |id: &str| {
        elements.iter().find(|e| e["id"] == json!(id)).unwrap()["index"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(
        idx("a") > idx("b") && idx("a") > idx("c"),
        "a 的 index 要最大：a={} b={} c={}",
        idx("a"),
        idx("b"),
        idx("c")
    );

    // back 回到最前。
    let back = c
        .tool(
            "whiteboard_layer",
            json!({ "ids": ["a"], "to": "back" }),
            cwd,
        )
        .await;
    assert_eq!(back["isError"], false, "{back}");
    assert_eq!(load_ids(&mut c, cwd).await, vec!["a", "b", "c"]);
}

/// 回归：界面的数组顺序（= z 序）要能被服务端采纳，用户拖的层序才留得住。
#[tokio::test]
async fn whiteboard_save_keeps_ui_array_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = start(dir.path().to_path_buf());
    let cwd = "/work/order";
    three_shapes(&mut c, cwd).await;

    let saved = c
        .tool(
            "whiteboard_save",
            json!({ "elements": [ui_el("a"), ui_el("b"), ui_el("c")], "order": ["c", "a", "b"] }),
            cwd,
        )
        .await;
    assert_eq!(saved["isError"], false, "{saved}");
    assert_eq!(load_ids(&mut c, cwd).await, vec!["c", "a", "b"]);
}

/// 回归：复制时两端都在选区里的骨架箭头跟着走，绑定改到副本上，只沿放置方向平移。
#[tokio::test]
async fn whiteboard_duplicate_retargets_pending_arrow_and_shifts_by_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = start(dir.path().to_path_buf());
    let cwd = "/work/dup";
    let edited = c
        .tool(
            "whiteboard_edit",
            json!({ "add": [
                { "type": "rectangle", "id": "a", "x": 100, "y": 0 },
                { "type": "rectangle", "id": "b", "x": 400, "y": 0 },
                { "type": "arrow", "id": "edge", "from": "a", "to": "b" }
            ] }),
            cwd,
        )
        .await;
    assert_eq!(edited["isError"], false, "{edited}");
    let duplicated = c
        .tool(
            "whiteboard_duplicate",
            json!({ "ids": ["a", "b"], "place": "right" }),
            cwd,
        )
        .await;
    assert_eq!(duplicated["isError"], false, "{duplicated}");
    let loaded = c.tool("whiteboard_load", json!({}), cwd).await;
    let pending = loaded["structuredContent"]["pending"].as_array().unwrap();
    let find = |id: &str| {
        pending
            .iter()
            .find(|e| e["id"] == id)
            .unwrap_or_else(|| panic!("没有 {id}：{pending:?}"))
    };
    let edge = find("edge-copy");
    assert_eq!(edge["start"]["id"], "a-copy");
    assert_eq!(edge["end"]["id"], "b-copy");
    let a = find("a");
    let a_copy = find("a-copy");
    let ax = a["x"].as_f64().unwrap();
    let ay = a["y"].as_f64().unwrap();
    let cx = a_copy["x"].as_f64().unwrap();
    let cy = a_copy["y"].as_f64().unwrap();
    assert!(cx > ax + 100.0, "a-copy.x={cx} a.x={ax}");
    assert!((cy - ay).abs() < 1e-6, "y 不变：{cy} vs {ay}");
}
