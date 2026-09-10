//! Copied from `xai-tty-utils` `ProcessScope` / unix `ProcessGroup`.
//! `killpg` uses libc instead of nix. `detach_std_command` is the unix `setsid` path.

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone)]
pub struct ProcessScope {
    inner: Arc<ScopeInner>,
}

struct ScopeInner {
    groups: Mutex<Vec<Weak<ProcessGroup>>>,
    closed: AtomicBool,
}

impl ProcessScope {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ScopeInner {
                groups: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
            }),
        }
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }

    pub fn register(&self, group: &Arc<ProcessGroup>) -> bool {
        let mut groups = self.inner.groups.lock().unwrap_or_else(|e| e.into_inner());
        if self.inner.closed.load(Ordering::Relaxed) {
            if !group.wants_hangup() {
                let _ = group.kill();
            }
            return false;
        }
        groups.retain(|w| w.strong_count() > 0);
        groups.push(Arc::downgrade(group));
        true
    }
}

pub struct ProcessGroup {
    #[cfg(unix)]
    leader: Option<u32>,
}

impl ProcessGroup {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            leader: None,
        })
    }

    pub fn attach_std(&mut self, child: &std::process::Child) -> io::Result<()> {
        self.attach_pid(child.id())
    }

    pub fn attach_pid(&mut self, pid: u32) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.leader = Some(pid);
        }
        let _ = pid;
        Ok(())
    }

    pub fn wants_hangup(&self) -> bool {
        false
    }

    pub fn kill(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let Some(leader) = self.leader else {
                return Ok(());
            };
            let rc = unsafe { libc::killpg(leader as i32, libc::SIGKILL) };
            if rc == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(not(unix))]
        Ok(())
    }
}

/// Copied from `xai_tty_utils::detach_std_command` (unix `setsid`, EPERM → `setpgid`).
pub fn detach_std_command(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() >= 0 {
                    return Ok(());
                }
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
                    && libc::setpgid(0, 0) == 0
                {
                    return Ok(());
                }
                Err(std::io::Error::last_os_error())
            });
        }
    }
    let _ = cmd;
}

/// Copied from `xai_tty_utils::pager_env` (subset used by LSP spawn).
pub fn pager_env() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::from([
        ("PAGER".to_string(), "cat".to_string()),
        ("GIT_PAGER".to_string(), "cat".to_string()),
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ("GPG_TTY".to_string(), String::new()),
    ])
}
