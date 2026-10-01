//! 线上协议：两代握手、tools/list、tools/call 的 isError、未知方法、坏行。
//! 不需要 Chrome —— 只打到「还没 browser_open」的那条路径。

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Client {
    input: tokio::io::DuplexStream,
    output: tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Client {
    fn start() -> Self {
        let (input, server_in) = tokio::io::duplex(64 * 1024);
        let (server_out, output) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(cordis_browser::serve(server_in, server_out));
        Self {
            input,
            output: BufReader::new(output).lines(),
            server,
        }
    }

    async fn send(&mut self, v: Value) {
        let mut line = v.to_string();
        line.push('\n');
        self.input.write_all(line.as_bytes()).await.unwrap();
    }

    async fn send_raw(&mut self, raw: &str) {
        self.input.write_all(raw.as_bytes()).await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), self.output.next_line())
            .await
            .expect("server answered in time")
            .unwrap()
            .expect("server still writing");
        serde_json::from_str(&line).unwrap()
    }

    async fn close(self) {
        drop(self.input);
        tokio::time::timeout(std::time::Duration::from_secs(5), self.server)
            .await
            .expect("server exits when stdin closes")
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn modern_discover_then_list() {
    let mut c = Client::start();
    c.send(json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}))
        .await;
    let v = c.recv().await;
    assert_eq!(v["id"], 1);
    let versions = v["result"]["supportedVersions"].as_array().unwrap();
    assert_eq!(versions[0], "2026-07-28");
    assert_eq!(v["result"]["serverInfo"]["name"], "dock-browser");

    c.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        .await;
    let v = c.recv().await;
    let tools = v["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 21);
    assert!(tools.iter().any(|t| t["name"] == "browser_open"));
    assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
    c.close().await;
}

#[tokio::test]
async fn legacy_initialize_echoes_version() {
    let mut c = Client::start();
    c.send(json!({
        "jsonrpc":"2.0","id":"a","method":"initialize",
        "params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}
    }))
    .await;
    let v = c.recv().await;
    assert_eq!(v["id"], "a");
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
    assert!(v["result"]["capabilities"]["tools"].is_object());
    // initialized 是通知：不该有回复。紧跟一个 ping 验证下一条就是 ping 的结果。
    c.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .await;
    c.send(json!({"jsonrpc":"2.0","id":"p","method":"ping"}))
        .await;
    let v = c.recv().await;
    assert_eq!(v["id"], "p");
    assert_eq!(v["result"], json!({}));
    c.close().await;
}

#[tokio::test]
async fn call_before_open_is_an_error_result() {
    let mut c = Client::start();
    c.send(json!({
        "jsonrpc":"2.0","id":7,"method":"tools/call",
        "params":{"name":"browser_snapshot","arguments":{},"_meta":{"dock/sessionId":"s1"}}
    }))
    .await;
    let v = c.recv().await;
    assert_eq!(v["id"], 7);
    assert_eq!(v["result"]["isError"], true);
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("browser_open"), "{text}");
    c.close().await;
}

#[tokio::test]
async fn unknown_method_and_bad_line() {
    let mut c = Client::start();
    c.send_raw("{not json\n").await;
    let v = c.recv().await;
    assert_eq!(v["error"]["code"], -32700);
    assert_eq!(v["id"], Value::Null);

    c.send(json!({"jsonrpc":"2.0","id":3,"method":"resources/list"}))
        .await;
    let v = c.recv().await;
    assert_eq!(v["id"], 3);
    assert_eq!(v["error"]["code"], -32601);

    c.send(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{}}))
        .await;
    let v = c.recv().await;
    assert_eq!(v["error"]["code"], -32602);
    c.close().await;
}
