//! 底部终端的 PTY。每个终端标签一个登录 shell，输出原样字节经 Channel 推给
//! xterm.js；shell 退出时同一 Channel 发一条 JSON `{"exit": code}`。
//! 应用退出时 `kill_all`；页面重载时 `kill_owned_by` 杀掉那个 webview 开的，不留孤儿 shell。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use tauri::ipc::{Channel, InvokeResponseBody};

struct Term {
    /// 开这个终端的 webview：它重载后旧页面的标签都没了，shell 要跟着收。
    owner: String,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

#[derive(Clone, Default)]
pub struct Terminals {
    next: Arc<AtomicU32>,
    terms: Arc<Mutex<HashMap<u32, Term>>>,
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(2),
        cols: cols.max(2),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// `~` 开头的目录展开到家目录；不存在的目录退回家目录，免得 shell 起不来。
fn resolve_cwd(cwd: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let path = match cwd.strip_prefix('~') {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(cwd),
    };
    if path.is_dir() {
        path
    } else {
        home
    }
}

impl Terminals {
    pub fn spawn(&self, owner: &str, cwd: &str, cols: u16, rows: u16, out: Channel) -> Result<u32, String> {
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|e| format!("打开终端失败：{e}"))?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.arg("-l");
        cmd.cwd(resolve_cwd(cwd));
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("启动 {shell} 失败：{e}"))?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.terms.lock().unwrap().insert(
            id,
            Term {
                owner: owner.to_string(),
                master: pair.master,
                writer,
                child,
            },
        );

        let terms = self.terms.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if out
                            .send(InvokeResponseBody::Raw(buf[..n].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            // 读到 EOF：shell 已退出（或被 kill）。收走子进程，报退出码。
            let term = terms.lock().unwrap().remove(&id);
            let code = term
                .and_then(|mut t| t.child.wait().ok())
                .map(|s| s.exit_code())
                .unwrap_or(0);
            let _ = out.send(InvokeResponseBody::Json(format!("{{\"exit\":{code}}}")));
        });
        Ok(id)
    }

    pub fn write(&self, id: u32, data: &str) -> Result<(), String> {
        let mut terms = self.terms.lock().unwrap();
        let term = terms.get_mut(&id).ok_or("终端已关闭")?;
        term.writer
            .write_all(data.as_bytes())
            .map_err(|e| e.to_string())
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        let terms = self.terms.lock().unwrap();
        let term = terms.get(&id).ok_or("终端已关闭")?;
        term.master
            .resize(size(cols, rows))
            .map_err(|e| e.to_string())
    }

    /// 杀掉 shell；读线程随后读到 EOF，自己清理并报退出。
    pub fn kill(&self, id: u32) {
        if let Some(term) = self.terms.lock().unwrap().get_mut(&id) {
            let _ = term.child.kill();
        }
    }

    /// 某个 webview 开的全部终端（页面开始重载时调：旧页面的标签没了，没人会再来 kill）。
    pub fn kill_owned_by(&self, owner: &str) {
        for term in self.terms.lock().unwrap().values_mut() {
            if term.owner == owner {
                let _ = term.child.kill();
            }
        }
    }

    pub fn kill_all(&self) {
        for term in self.terms.lock().unwrap().values_mut() {
            let _ = term.child.kill();
        }
    }
}
