//! `dock serve` 冒烟：真的起二进制，按父进程（桌面 GUI）的用法走一遍——读
//! ready 行、要一张新 ticket、关 stdin 后进程自己退出。

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn dock() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dock"));
    let home = tempfile::tempdir().unwrap().keep();
    cmd.env("DOCK_HOME", &home)
        .env("DOCK_CUA_DRIVER", "off")
        .current_dir(&home);
    cmd
}

#[test]
fn serve_speaks_json_lines_and_exits_with_its_parent() {
    let mut child = dock()
        .args(["serve", "--origin", "tauri://localhost"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut read = || {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line)
            .unwrap_or_else(|e| panic!("stdout 只能是 JSON 行：{line:?} ({e})"))
    };

    let ready = read();
    assert_eq!(ready["event"], "ready", "{ready}");
    assert_eq!(ready["protocol"], "dock.1");
    let ws = ready["ws"].as_str().unwrap();
    assert!(ws.starts_with("ws://127.0.0.1:"), "只绑回环：{ws}");
    assert_eq!(ready["ticket"].as_str().unwrap().len(), 64);

    writeln!(stdin, "{{\"cmd\":\"ticket\"}}").unwrap();
    let renewed = read();
    assert_eq!(renewed["event"], "ticket");
    assert_ne!(renewed["ticket"], ready["ticket"]);

    // 父进程走了：stdin 关闭，dock 不能留成孤儿。
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("stdin 关闭后 dock serve 没有退出");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status:?}");
}

#[test]
fn serve_requires_an_origin() {
    let out = dock().arg("serve").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "错误不能写到 stdout（那是控制通道）");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--origin"), "{err}");
}
