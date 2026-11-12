//! GUI 自己开的 SSH 隧道：`ssh -N -L 127.0.0.1:<本机随机端口>:127.0.0.1:<远端端口> user@host`。
//!
//! - 每个已打开的远程连接一条（`ensure`），同一条连接的窗口共用；切换前台不拆别的。
//! - 「测试连接」另开临时的（`open_temp` / `close_temp`），不碰已经打开的。
//! - 隧道通了还要看远端 Dock 在不在：经隧道发一个 HTTP 请求，回 `HTTP/` 才算在。
//!   不在、又填了启动命令，就经 SSH 跑一次（如 `systemctl start dock`），再等它起来。
//! - 只用系统的 `ssh`：认证走 ssh-agent / 私钥文件 / ~/.ssh/config，`BatchMode` 不弹密码框。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::remote::{SshTarget, FATAL};

/// 隧道建起来（本机端口能连上）最多等这么久。
const TUNNEL_WAIT: Duration = Duration::from_secs(20);
/// 跑完启动命令后等远端 Dock 起来最多这么久（要挂整棵插件树）。
const START_WAIT: Duration = Duration::from_secs(40);
/// 启动命令本身最多跑这么久。
const COMMAND_WAIT: Duration = Duration::from_secs(30);

struct Running {
    /// 连接 id + 配置：配置改了就重开。
    key: String,
    child: Child,
    port: u16,
    stderr: Arc<Mutex<String>>,
}

#[derive(Clone, Default)]
pub struct Tunnels {
    /// 已打开的远程连接各一条。切换前台不拆别的隧道。
    open: Arc<Mutex<HashMap<String, Running>>>,
    temps: Arc<Mutex<HashMap<u16, Running>>>,
}

impl Tunnels {
    /// 这条连接的隧道：还活着、配置没变就复用，否则只重开这一条。回本机端口。
    pub fn ensure(&self, id: &str, ssh: &SshTarget) -> Result<u16, String> {
        let key = format!("{id}:{ssh:?}");
        let mut open = self.open.lock().unwrap();
        if let Some(running) = open.get_mut(id) {
            let alive = matches!(running.child.try_wait(), Ok(None));
            if alive && running.key == key {
                return Ok(running.port);
            }
            kill(running);
            open.remove(id);
        }
        let running = open_tunnel(key, ssh)?;
        let port = running.port;
        open.insert(id.to_string(), running);
        Ok(port)
    }

    /// 关掉这一条连接的隧道。别的连接不动。
    pub fn stop_id(&self, id: &str) {
        if let Some(mut running) = self.open.lock().unwrap().remove(id) {
            kill(&mut running);
        }
    }

    /// 「测试连接」用的临时隧道，回本机端口（也是关它用的 id）。
    pub fn open_temp(&self, ssh: &SshTarget) -> Result<u16, String> {
        let running = open_tunnel(String::new(), ssh)?;
        let port = running.port;
        self.temps.lock().unwrap().insert(port, running);
        Ok(port)
    }

    pub fn close_temp(&self, port: u16) {
        if let Some(mut running) = self.temps.lock().unwrap().remove(&port) {
            kill(&mut running);
        }
    }

    pub fn stop_all(&self) {
        for (_, mut running) in self.open.lock().unwrap().drain() {
            kill(&mut running);
        }
        for (_, mut running) in self.temps.lock().unwrap().drain() {
            kill(&mut running);
        }
    }
}

fn kill(running: &mut Running) {
    let _ = running.child.kill();
    let _ = running.child.wait();
}

/// 开一条隧道并确认远端 Dock 在（不在就按启动命令拉起来）。
fn open_tunnel(key: String, ssh: &SshTarget) -> Result<Running, String> {
    let port = free_port()?;
    let mut cmd = ssh_command(ssh);
    cmd.args([
        "-N",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
    ])
    .arg("-L")
    .arg(format!("127.0.0.1:{port}:127.0.0.1:{}", ssh.remote_port))
    .arg(destination(ssh))
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("启动 ssh 失败：{e}"))?;
    let stderr = Arc::new(Mutex::new(String::new()));
    if let Some(mut pipe) = child.stderr.take() {
        let sink = stderr.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
    }
    let mut running = Running {
        key,
        child,
        port,
        stderr,
    };

    // 等本机端口能连上；ssh 先退出了就把它的报错带回去。
    let deadline = Instant::now() + TUNNEL_WAIT;
    loop {
        if let Ok(Some(status)) = running.child.try_wait() {
            std::thread::sleep(Duration::from_millis(100));
            let said = running.stderr.lock().unwrap().clone();
            return Err(ssh_failure(&said, &status.to_string(), ssh));
        }
        if TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(300))
            .is_ok()
        {
            break;
        }
        if Instant::now() > deadline {
            kill(&mut running);
            return Err(format!(
                "SSH 隧道 {} 秒没建起来（{}）",
                TUNNEL_WAIT.as_secs(),
                destination(ssh)
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    if gateway_up(port) {
        return Ok(running);
    }
    let Some(start) = ssh.start_command.as_deref() else {
        kill(&mut running);
        return Err(format!(
            "{FATAL}远端 Dock 没在运行（{}:{}）。可以在连接里填启动命令，如 systemctl start dock",
            ssh.host, ssh.remote_port
        ));
    };
    if let Err(e) = run_remote(ssh, start) {
        kill(&mut running);
        return Err(format!("{FATAL}{e}"));
    }
    let deadline = Instant::now() + START_WAIT;
    while Instant::now() < deadline {
        if gateway_up(port) {
            return Ok(running);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    kill(&mut running);
    Err(format!(
        "{FATAL}跑了启动命令（{start}），{} 秒内远端 Dock 还是没起来",
        START_WAIT.as_secs()
    ))
}

/// 经隧道问一下 `/api/ws`：网关在会回 HTTP 响应（400，因为没升级）；远端端口没人听时
/// ssh 会直接关掉这条通道，读到 EOF。
fn gateway_up(port: u16) -> bool {
    let Ok(mut stream) =
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let request =
        format!("GET /api/ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut head = [0u8; 5];
    matches!(stream.read_exact(&mut head), Ok(())) && &head == b"HTTP/"
}

/// 经 SSH 在远端跑一条命令（启动 Dock）。非零退出就把输出带回去。
fn run_remote(ssh: &SshTarget, command: &str) -> Result<(), String> {
    let mut child = ssh_command(ssh)
        .arg(destination(ssh))
        .arg("--")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 ssh 失败：{e}"))?;
    let deadline = Instant::now() + COMMAND_WAIT;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "启动命令跑了 {} 秒还没结束：{command}",
                COMMAND_WAIT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    if status.success() {
        return Ok(());
    }
    let mut out = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut out);
    }
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut out);
    }
    Err(format!(
        "启动命令失败（{status}）：{}",
        last_line(&out).unwrap_or(command)
    ))
}

/// 隧道和远端命令共用的 ssh 参数：不弹密码框、新主机自动记下指纹、连不上 10 秒放弃。
fn ssh_command(ssh: &SshTarget) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ConnectTimeout=10",
    ]);
    if let Some(port) = ssh.port {
        cmd.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = &ssh.identity {
        cmd.arg("-i")
            .arg(expand_home(identity))
            .args(["-o", "IdentitiesOnly=yes"]);
    }
    cmd
}

fn destination(ssh: &SshTarget) -> String {
    format!("{}@{}", ssh.user, ssh.host)
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

/// 问系统要一个空闲端口再放掉给 ssh 用（中间被别人抢了的话 ssh 会 ExitOnForwardFailure）。
fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("找不到空闲端口：{e}"))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

fn last_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).rfind(|l| !l.is_empty())
}

/// ssh 退出了：认证 / 主机指纹 / 私钥问题再试也没用，标成 fatal；网络问题留给重连。
fn ssh_failure(stderr: &str, status: &str, ssh: &SshTarget) -> String {
    let line = last_line(stderr).unwrap_or(status).to_string();
    let fatal = [
        "Permission denied",
        "Host key verification failed",
        "Identity file",
        "no such identity",
        "Bad configuration",
    ]
    .iter()
    .any(|needle| stderr.contains(needle));
    let message = format!("SSH 隧道建不起来（{}）：{line}", destination(ssh));
    if fatal {
        format!("{FATAL}{message}")
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> SshTarget {
        SshTarget {
            host: "h.example.com".into(),
            user: "root".into(),
            port: None,
            identity: None,
            remote_port: 18990,
            start_command: None,
        }
    }

    #[test]
    fn auth_failures_are_fatal_network_ones_are_not() {
        let denied = ssh_failure(
            "root@h: Permission denied (publickey).\n",
            "exit status: 255",
            &target(),
        );
        assert!(denied.starts_with(FATAL), "{denied}");
        assert!(denied.contains("Permission denied"), "{denied}");
        let timeout = ssh_failure(
            "ssh: connect to host h port 22: Operation timed out\n",
            "exit status: 255",
            &target(),
        );
        assert!(!timeout.starts_with(FATAL), "{timeout}");
    }

    #[test]
    fn gateway_probe_tells_http_from_closed_channel() {
        // 回 HTTP 的「网关」。
        let http = TcpListener::bind("127.0.0.1:0").unwrap();
        let http_port = http.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = http.accept().unwrap();
            let mut buf = [0u8; 256];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
        });
        assert!(gateway_up(http_port));
        // 收下连接就关（ssh 在远端端口没人听时就是这样）。
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = closed.accept().unwrap();
            drop(s);
        });
        assert!(!gateway_up(closed_port));
    }
}
