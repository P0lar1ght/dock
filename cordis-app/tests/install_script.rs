//! `install.sh`：静态检查 + 用本地 file:// 产物真跑一遍（假 dock，不碰真实 ~/.dock）。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn install_sh() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../install.sh")
}

/// macOS 的 `sh` 是 bash 3.2：`$var` 后面紧跟全角字符时，会把字节吃进变量名，
/// `set -u` 下直接 unbound variable 退出。必须写成 `${var}`。
#[test]
fn no_unbraced_variable_before_non_ascii() {
    let text = fs::read_to_string(install_sh()).unwrap();
    let mut bad = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '$'
                && chars
                    .get(i + 1)
                    .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
            {
                let mut j = i + 1;
                while chars
                    .get(j)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    j += 1;
                }
                if chars.get(j).is_some_and(|c| !c.is_ascii()) {
                    bad.push(format!("install.sh:{}: {}", n + 1, line.trim()));
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    assert!(
        bad.is_empty(),
        "变量后紧跟非 ASCII，要加花括号：\n{}",
        bad.join("\n")
    );
}

#[cfg(unix)]
fn fake_release(dir: &Path) {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    let stage = dir.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let bin = stage.join("dock");
    fs::write(
        &bin,
        "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'dock 9.9.9' ;;\n  init) echo 'fake init'; : > \"$DOCK_HOME/init-ran\" ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    // install.sh 按 uname 选 target：四个平台各放一份，同内容。
    for target in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
    ] {
        let file = format!("dock-{target}.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(dir.join(&file))
            .arg("-C")
            .arg(&stage)
            .arg("dock")
            .status()
            .unwrap();
        assert!(status.success());
        let sum = Sha256::digest(fs::read(dir.join(&file)).unwrap());
        fs::write(
            dir.join(format!("{file}.sha256")),
            format!("{sum:x}  {file}\n"),
        )
        .unwrap();
    }
}

#[cfg(unix)]
#[test]
fn install_script_runs_three_phases_and_inits() {
    let release = tempfile::tempdir().unwrap();
    let install = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fake_release(release.path());

    let out = Command::new("sh")
        .arg(install_sh())
        .env(
            "DOCK_BASE_URL",
            format!("file://{}", release.path().display()),
        )
        .env("DOCK_INSTALL_DIR", install.path())
        .env("DOCK_HOME", home.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");

    // 非 TTY：朴素输出，三阶段都走到。
    for phase in ["阶段 1/3", "阶段 2/3", "阶段 3/3"] {
        assert!(stdout.contains(phase), "缺 {phase}：\n{stdout}");
    }
    assert!(
        !stdout.contains('\u{1b}'),
        "非 TTY 不该有颜色码：\n{stdout}"
    );
    assert!(stdout.contains("已安装 dock 9.9.9"), "{stdout}");
    assert!(
        home.path().join("init-ran").is_file(),
        "没跑 dock init：\n{stdout}"
    );

    let names: Vec<_> = fs::read_dir(install.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        vec![std::ffi::OsString::from("dock")],
        "暂存文件没清掉"
    );
}
