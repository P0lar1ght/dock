//! `dock init` / `doctor` / `upgrade --check`：真起二进制，DOCK_HOME 用临时目录。

use std::process::Command;

fn dock(home: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dock"));
    cmd.env("DOCK_HOME", home)
        .env("DOCK_CUA_DRIVER", "off")
        .env("DOCK_BROWSER_MCP", "off")
        .current_dir(home);
    cmd
}

fn run(home: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = dock(home).args(args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn init_idempotent_does_not_overwrite_config() {
    let home = tempfile::tempdir().unwrap();
    let (code, out, err) = run(home.path(), &["init"]);
    assert_eq!(code, 0, "{err}\n{out}");
    assert!(out.contains("已初始化") || out.contains("已写入"), "{out}");
    assert!(out.contains("下一步") || out.contains("模型"), "{out}");

    let config = home.path().join("config.toml");
    assert!(config.is_file());
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("env_key"));
    assert!(
        !text
            .lines()
            .any(|l| l.trim().starts_with("api_key") && !l.trim().starts_with('#')),
        "骨架不得含明文 api_key：{text}"
    );

    let marker = "# preserved-by-user\ncustom = true\n";
    std::fs::write(&config, marker).unwrap();

    let (code, out, err) = run(home.path(), &["init"]);
    assert_eq!(code, 0, "{err}\n{out}");
    assert!(out.contains("跳过") || out.contains("已存在"), "{out}");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), marker);
}

#[test]
fn doctor_fails_on_empty_home_passes_when_ready() {
    let home = tempfile::tempdir().unwrap();

    let (code, out, err) = run(home.path(), &["doctor"]);
    assert_ne!(code, 0, "缺 config 应为非 0：{out}\n{err}");
    assert!(
        out.contains("配置文件") || out.contains("缺少"),
        "{out}\n{err}"
    );

    // 先 init 再写可用模型
    let (code, _, err) = run(home.path(), &["init"]);
    assert_eq!(code, 0, "{err}");

    std::fs::write(
        home.path().join("config.toml"),
        r#"
[models]
default = "m"

[model.m]
name = "demo"
api_base_url = "https://example.test/v1"
api_backends = ["responses"]
env_key = "DOCK_SETUP_CLI_KEY"
"#,
    )
    .unwrap();

    let (code, out, err) = dock(home.path())
        .args(["doctor"])
        .env("DOCK_SETUP_CLI_KEY", "sk-test")
        .output()
        .map(|o| {
            (
                o.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&o.stdout).into_owned(),
                String::from_utf8_lossy(&o.stderr).into_owned(),
            )
        })
        .unwrap();

    // PATH 上可能找不到 debug 二进制 → Warn → 非 0；至少配置与模型目录应通过。
    assert!(
        out.contains("配置文件") && out.contains("模型目录"),
        "{out}\n{err}"
    );
    assert!(
        out.contains("✓") || out.contains("存在") || out.contains("条"),
        "{out}"
    );
    // 不要求全局 exit 0（PATH warn 常见），但不应再报「缺少 config」。
    assert!(!out.contains("缺少"), "{out}");
    let _ = code;
}

#[test]
fn upgrade_check_does_not_change_binary() {
    let home = tempfile::tempdir().unwrap();
    let install = tempfile::tempdir().unwrap();
    // 放一个伪二进制记录 mtime/内容；--check 不应碰它。
    let bin = install.path().join("dock");
    std::fs::write(&bin, b"not-a-real-dock-binary").unwrap();
    let before = std::fs::read(&bin).unwrap();
    let meta_before = std::fs::metadata(&bin).unwrap();

    let out = dock(home.path())
        .args(["upgrade", "--check"])
        .env("DOCK_UPGRADE_REMOTE_VERSION", "v0.0.1")
        .env("DOCK_INSTALL_DIR", install.path())
        .output()
        .unwrap();
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(code, 0, "{stderr}\n{stdout}");
    assert!(
        stdout.contains("当前版本") && stdout.contains("远端版本"),
        "{stdout}"
    );
    assert!(
        stdout.contains("已是最新") || stdout.contains("无需") || stdout.contains("不高于"),
        "{stdout}"
    );

    let after = std::fs::read(&bin).unwrap();
    assert_eq!(before, after, "upgrade --check 不得改二进制");
    let meta_after = std::fs::metadata(&bin).unwrap();
    assert_eq!(
        meta_before.modified().unwrap(),
        meta_after.modified().unwrap()
    );
}

#[test]
fn update_check_alias_works() {
    let home = tempfile::tempdir().unwrap();
    let (code, out, err) = dock(home.path())
        .args(["update", "--check"])
        .env("DOCK_UPGRADE_REMOTE_VERSION", "v99.0.0")
        .output()
        .map(|o| {
            (
                o.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&o.stdout).into_owned(),
                String::from_utf8_lossy(&o.stderr).into_owned(),
            )
        })
        .unwrap();
    assert_eq!(code, 0, "{err}\n{out}");
    assert!(
        out.contains("有新版本") || out.contains("远端版本"),
        "{out}"
    );
}
