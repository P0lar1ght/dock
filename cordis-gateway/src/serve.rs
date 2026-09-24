//! `dock serve`：由父进程（桌面 GUI）拉起的无头网关。
//!
//! 和 TUI 里 `/pair` 开的网关是同一个 `GatewayHandle`，差别只在鉴权入口：这里
//! 不走 Origin 配对，而是把 ticket 经 **stdout 管道**直接交给父进程。控制通道
//! 是一行一个 JSON：
//!
//! - 启动后 → `{"event":"ready","protocol":"dock.1","version":…,"http":…,"ws":…,"ticket":…,"expiresAtMs":…}`
//! - `{"cmd":"ticket"}` → `{"event":"ticket","ticket":…,"expiresAtMs":…}`（续签，ticket 有效期见 `TICKET_TTL`）
//! - 看不懂的行 → `{"event":"error","message":…}`
//! - stdin 关闭（父进程退出）→ 控制循环返回，`dock serve` 随之退出，不留孤儿
//!
//! 所以 serve 模式下 stdout 只能写这些行；其它诊断一律走 stderr。

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::handle::GatewayHandle;
use crate::pairing::IssuedTicket;
use crate::protocol::{PROTOCOL_VERSION, SERVER_VERSION, WS_PATH};

/// Named service `"gateway.serve"`：只在 `gateway_serve` 挂载时存在。
pub const GATEWAY_SERVE: &str = "gateway.serve";

/// `gateway_serve` 的配置：ticket 签给哪个应用、哪个 Origin（GUI webview 的
/// Origin，WS 握手时要对得上）。
#[derive(Clone, Debug)]
pub struct ServeConfig {
    pub application: String,
    pub origin: String,
}

/// `"gateway.serve"` 服务：驱动 stdin / stdout 控制通道。
#[derive(Clone)]
pub struct ServeControl {
    inner: Arc<Inner>,
}

struct Inner {
    handle: GatewayHandle,
    config: ServeConfig,
    addr: SocketAddr,
}

impl ServeControl {
    pub(crate) fn new(handle: GatewayHandle, config: ServeConfig, addr: SocketAddr) -> Self {
        Self {
            inner: Arc::new(Inner {
                handle,
                config,
                addr,
            }),
        }
    }

    /// 网关实际监听的地址（端口可能是 OS 分配的）。
    pub fn addr(&self) -> SocketAddr {
        self.inner.addr
    }

    /// 先写 ready 行，然后逐行处理命令，直到 `reader` 读到 EOF 才返回。
    pub async fn run<R, W>(&self, reader: R, mut writer: W) -> std::io::Result<()>
    where
        R: AsyncBufRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let ready = match self.issue() {
            Ok(ticket) => {
                let mut line = ticket_json("ready", &ticket);
                line["protocol"] = json!(PROTOCOL_VERSION);
                line["version"] = json!(SERVER_VERSION);
                line["http"] = json!(format!("http://{}", self.inner.addr));
                line["ws"] = json!(format!("ws://{}{WS_PATH}", self.inner.addr));
                line
            }
            Err(message) => error_json(message),
        };
        write_line(&mut writer, &ready).await?;

        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let reply = self.handle_command(line);
            write_line(&mut writer, &reply).await?;
        }
        Ok(())
    }

    fn handle_command(&self, line: &str) -> Value {
        let cmd = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|v| v.get("cmd").and_then(Value::as_str).map(str::to_string));
        match cmd.as_deref() {
            Some("ticket") => match self.issue() {
                Ok(ticket) => ticket_json("ticket", &ticket),
                Err(message) => error_json(message),
            },
            Some(other) => error_json(format!("未知命令：{other}")),
            None => error_json("控制命令应为一行 JSON，如 {\"cmd\":\"ticket\"}".into()),
        }
    }

    fn issue(&self) -> Result<IssuedTicket, String> {
        let config = &self.inner.config;
        self.inner
            .handle
            .issue_trusted_ticket(&config.application, &config.origin)
            .map_err(|e| format!("签发 ticket 失败：{e}"))
    }
}

fn ticket_json(event: &str, ticket: &IssuedTicket) -> Value {
    json!({
        "event": event,
        "ticket": ticket.token,
        "expiresAtMs": ticket.expires_unix_ms,
    })
}

fn error_json(message: String) -> Value {
    json!({ "event": "error", "message": message })
}

async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> std::io::Result<()> {
    let mut line = value.to_string();
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, BufReader};

    const ORIGIN: &str = "tauri://localhost";

    fn control() -> ServeControl {
        let handle = GatewayHandle::idle(cordis::Context::new(), "127.0.0.1:0".parse().unwrap());
        ServeControl::new(
            handle,
            ServeConfig {
                application: "dock-gui".into(),
                origin: ORIGIN.into(),
            },
            "127.0.0.1:4567".parse().unwrap(),
        )
    }

    async fn transcript(control: &ServeControl, input: &str) -> Vec<Value> {
        let (mut out_w, mut out_r) = tokio::io::duplex(64 * 1024);
        control
            .run(BufReader::new(input.as_bytes()), &mut out_w)
            .await
            .unwrap();
        drop(out_w);
        let mut text = String::new();
        out_r.read_to_string(&mut text).await.unwrap();
        text.lines()
            .map(|l| serde_json::from_str(l).expect("每行都是 JSON"))
            .collect()
    }

    /// ready 行带地址与一张可用的 ticket；续签给新的一张；看不懂的行回中文错误；
    /// stdin EOF 时 `run` 返回（`dock serve` 靠这个跟着父进程退出）。
    #[tokio::test]
    async fn ready_then_tickets_until_eof() {
        let control = control();
        let lines = transcript(
            &control,
            "{\"cmd\":\"ticket\"}\n\nnot json\n{\"cmd\":\"nope\"}\n",
        )
        .await;
        assert_eq!(lines.len(), 4, "{lines:?}");

        let ready = &lines[0];
        assert_eq!(ready["event"], "ready");
        assert_eq!(ready["protocol"], PROTOCOL_VERSION);
        assert_eq!(ready["ws"], "ws://127.0.0.1:4567/api/ws");
        let first = ready["ticket"].as_str().unwrap();
        control
            .inner
            .handle
            .authenticate(first, ORIGIN)
            .expect("ready 里的 ticket 能用");

        assert_eq!(lines[1]["event"], "ticket");
        let second = lines[1]["ticket"].as_str().unwrap();
        assert_ne!(first, second);
        control.inner.handle.authenticate(second, ORIGIN).unwrap();

        assert_eq!(lines[2]["event"], "error");
        assert_eq!(lines[3]["event"], "error");
        assert!(lines[3]["message"].as_str().unwrap().contains("nope"));
    }
}
