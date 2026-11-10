//! 远程 Dock（`dock serve --remote`）的连接配置。
//!
//! - 名单和「当前用哪个」在 App 数据目录的 `remotes.json`：`{ active, remotes: [{ id, name, url, ssh? }] }`。
//!   `active` 为空 = 用本机 Dock（GUI 自己拉起的 `dock serve`）。
//! - 两种连法：直连 `url`（反向代理 / Tailscale / 自己开的隧道），或 `ssh`：GUI 自己开 SSH 隧道
//!   （见 tunnel.rs），远端 Dock 没在跑时还能经 SSH 跑一条启动命令。
//! - 设备令牌只进 macOS 钥匙串（service `dock-gui`，account 是连接 id），不落盘、不进前端存储。
//!   读过一次就缓存在进程里：重连不再碰钥匙串（签名变了的开发构建每读一次都可能弹授权框）。
//! - `url` 存用户填的样子；连的时候规范成 `wss://…/api/ws`。和 SDK 同一条规矩：
//!   只认 `https`，`http` 只允许回环（SSH 隧道、本机测试）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Url};

const KEYCHAIN_SERVICE: &str = "dock-gui";
const WS_PATH: &str = "/api/ws";
/// 不会自己好的错误带这个前缀（拿不到令牌、SSH 认证失败、远端 Dock 起不来）：前端认得出来，
/// 不再自动重连——再试只会再弹授权框 / 再撞一次认证。
pub const FATAL: &str = "[fatal] ";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Remote {
    pub id: String,
    pub name: String,
    /// 直连地址；走 SSH 的连接是空串。
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshTarget>,
}

/// 经 SSH 隧道连远端 Dock：`ssh -L 本机随机端口:127.0.0.1:remote_port user@host`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTarget {
    pub host: String,
    pub user: String,
    #[serde(default)]
    pub port: Option<u16>,
    /// 私钥文件；空着走 ssh-agent / ~/.ssh/config。
    #[serde(default)]
    pub identity: Option<String>,
    /// 远端 `dock serve --remote --bind 127.0.0.1:<这个>`。
    pub remote_port: u16,
    /// 远端 Dock 没在跑时经 SSH 跑这条（如 `systemctl start dock`）；空着就只报错。
    #[serde(default)]
    pub start_command: Option<String>,
}

impl SshTarget {
    /// 去掉空白、空串当没填；必填项不全就报错。
    pub fn normalized(mut self) -> Result<Self, String> {
        self.host = self.host.trim().to_string();
        self.user = self.user.trim().to_string();
        if self.host.is_empty() {
            return Err("SSH 主机不能为空".into());
        }
        if self.user.is_empty() {
            return Err("SSH 用户不能为空".into());
        }
        if self.remote_port == 0 {
            return Err("远端端口不能为空".into());
        }
        let blank = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        self.identity = blank(self.identity);
        self.start_command = blank(self.start_command);
        Ok(self)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Remotes {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub remotes: Vec<Remote>,
}

impl Remotes {
    pub fn active_remote(&self) -> Option<&Remote> {
        let id = self.active.as_deref()?;
        self.remotes.iter().find(|r| r.id == id)
    }
}

#[derive(Clone, Default)]
pub struct RemoteStore {
    inner: Arc<Mutex<Option<PathBuf>>>,
    /// 连接 id → 令牌（钥匙串里读出来 / 刚存进去的）。
    tokens: Arc<Mutex<HashMap<String, String>>>,
}

impl RemoteStore {
    pub fn init(&self, app: &AppHandle) {
        *self.inner.lock().unwrap() = app.path().app_data_dir().ok().map(|d| d.join("remotes.json"));
    }

    fn file(&self) -> Result<PathBuf, String> {
        self.inner.lock().unwrap().clone().ok_or_else(|| "找不到 App 数据目录".into())
    }

    /// 读名单。文件不在 = 没配过；坏了也当没配过（只是连接名单，不值得卡住启动）。
    pub fn load(&self) -> Remotes {
        let Ok(file) = self.file() else { return Remotes::default() };
        std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<Remotes>(&s).ok())
            .map(|mut r| {
                // 指着已经删掉的连接：退回本机。
                if r.active_remote().is_none() {
                    r.active = None;
                }
                r
            })
            .unwrap_or_default()
    }

    fn store(&self, remotes: &Remotes) -> Result<(), String> {
        let file = self.file()?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("建不了数据目录：{e}"))?;
        }
        let text = serde_json::to_string_pretty(remotes).map_err(|e| e.to_string())?;
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("写连接名单失败：{e}"))?;
        std::fs::rename(&tmp, &file).map_err(|e| format!("写连接名单失败：{e}"))
    }

    /// 新建（`id` 空）或改一个连接。`token` 空着 = 改的时候保留原来的令牌；新建必须给。
    /// 给了 `ssh` 就走隧道，`url` 不用。
    pub fn save(
        &self,
        id: Option<String>,
        name: String,
        url: String,
        ssh: Option<SshTarget>,
        token: Option<String>,
    ) -> Result<Remote, String> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("名字不能为空".into());
        }
        let ssh = ssh.map(SshTarget::normalized).transpose()?;
        let url = if ssh.is_some() { String::new() } else { url.trim().to_string() };
        if ssh.is_none() {
            ws_url(&url)?;
        }
        let token = token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let mut remotes = self.load();
        if remotes.remotes.iter().any(|r| r.name == name && Some(&r.id) != id.as_ref()) {
            return Err(format!("已经有叫「{name}」的连接了"));
        }
        let remote = match id {
            Some(id) => {
                let found = remotes
                    .remotes
                    .iter_mut()
                    .find(|r| r.id == id)
                    .ok_or_else(|| "这个连接已经不在了".to_string())?;
                found.name = name;
                found.url = url;
                found.ssh = ssh;
                found.clone()
            }
            None => {
                if token.is_none() {
                    return Err("要填设备令牌（在远端跑 dock device add 拿到）".into());
                }
                let remote = Remote { id: new_id(), name, url, ssh };
                remotes.remotes.push(remote.clone());
                remote
            }
        };
        if let Some(token) = token {
            keychain(&remote.id)?
                .set_password(&token)
                .map_err(|e| format!("存进钥匙串失败：{e}"))?;
            self.tokens.lock().unwrap().insert(remote.id.clone(), token);
        }
        self.store(&remotes)?;
        Ok(remote)
    }

    /// 删一个连接，连同钥匙串里的令牌。删的是正在用的就退回本机，回 `true`。
    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let mut remotes = self.load();
        let was_active = remotes.active.as_deref() == Some(id);
        remotes.remotes.retain(|r| r.id != id);
        if was_active {
            remotes.active = None;
        }
        self.store(&remotes)?;
        self.tokens.lock().unwrap().remove(id);
        if let Ok(entry) = keychain(id) {
            let _ = entry.delete_credential();
        }
        Ok(was_active)
    }

    /// 换当前连接：`None` 是本机。
    pub fn set_active(&self, id: Option<String>) -> Result<Remotes, String> {
        let mut remotes = self.load();
        if let Some(id) = &id {
            if !remotes.remotes.iter().any(|r| &r.id == id) {
                return Err("这个连接已经不在了".into());
            }
        }
        remotes.active = id;
        self.store(&remotes)?;
        Ok(remotes)
    }

    pub fn get(&self, id: &str) -> Result<Remote, String> {
        self.load()
            .remotes
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| "这个连接已经不在了".to_string())
    }

    /// 这个连接的设备令牌：先看进程里的缓存，没有再读钥匙串。
    pub fn token(&self, id: &str) -> Result<String, String> {
        if let Some(token) = self.tokens.lock().unwrap().get(id) {
            return Ok(token.clone());
        }
        let token = keychain(id)?
            .get_password()
            .map_err(|e| match e {
                keyring::Error::NoEntry => format!("{FATAL}钥匙串里没有这个连接的令牌，重新填一次"),
                e => format!("{FATAL}读钥匙串失败（{e}）"),
            })?;
        self.tokens.lock().unwrap().insert(id.to_string(), token.clone());
        Ok(token)
    }
}

fn keychain(id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, id).map_err(|e| format!("打不开钥匙串：{e}"))
}

fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("r{nanos:x}")
}

/// 用户填的地址 → WS 地址。`https://host[/前缀]` → `wss://host[/前缀]/api/ws`；
/// 已经写到 `/api/ws` 的不再追加。`http` / `ws` 只允许回环。
pub fn ws_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("地址不能为空".into());
    }
    let mut url = Url::parse(raw).map_err(|_| format!("看不懂的地址：{raw}"))?;
    let secure = match url.scheme() {
        "https" | "wss" => true,
        "http" | "ws" => false,
        other => return Err(format!("地址要用 https://（不认 {other}://）")),
    };
    let host = url.host_str().unwrap_or("").to_string();
    if host.is_empty() {
        return Err(format!("地址里没有主机名：{raw}"));
    }
    if !secure && !is_loopback(&host) {
        return Err("远程地址要用 https://；http:// 只允许 127.0.0.1 / localhost（SSH 隧道）".into());
    }
    url.set_scheme(if secure { "wss" } else { "ws" })
        .map_err(|_| format!("看不懂的地址：{raw}"))?;
    let path = url.path().trim_end_matches('/').to_string();
    if !path.ends_with(WS_PATH) {
        url.set_path(&format!("{path}{WS_PATH}"));
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::ws_url;

    #[test]
    fn ws_url_normalizes_and_guards_plain_http() {
        assert_eq!(ws_url("https://dock.example.com").unwrap(), "wss://dock.example.com/api/ws");
        assert_eq!(ws_url("https://dock.example.com/").unwrap(), "wss://dock.example.com/api/ws");
        assert_eq!(ws_url("https://h.example.com/dock/").unwrap(), "wss://h.example.com/dock/api/ws");
        assert_eq!(ws_url("wss://h.example.com/api/ws").unwrap(), "wss://h.example.com/api/ws");
        assert_eq!(ws_url("http://127.0.0.1:18990").unwrap(), "ws://127.0.0.1:18990/api/ws");
        assert_eq!(ws_url("http://localhost:18990").unwrap(), "ws://localhost:18990/api/ws");
        assert_eq!(ws_url("http://[::1]:18990").unwrap(), "ws://[::1]:18990/api/ws");
        assert!(ws_url("http://dock.example.com").is_err());
        assert!(ws_url("ws://10.0.0.2:18990").is_err());
        assert!(ws_url("ftp://dock.example.com").is_err());
        assert!(ws_url("dock.example.com").is_err());
        assert!(ws_url("").is_err());
    }
}
