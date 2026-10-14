//! `dock doctor`：中文汇报本机安装与配置健康状况。

use std::path::{Path, PathBuf};
use std::process::Command;

use cordis_base::config::{dock_home, load_catalog, ModelChoice};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone)]
pub struct CheckItem {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct DoctorReport {
    pub items: Vec<CheckItem>,
}

impl DoctorReport {
    pub fn ok(&self) -> bool {
        self.items.iter().all(|i| i.status != CheckStatus::Fail)
    }

    /// 全 Pass → 0；任一 Fail / Warn → 1（「有问题非 0」）。
    pub fn exit_code(&self) -> i32 {
        if self
            .items
            .iter()
            .any(|i| matches!(i.status, CheckStatus::Fail | CheckStatus::Warn))
        {
            1
        } else {
            0
        }
    }
}

pub fn run_doctor() -> DoctorReport {
    let mut report = DoctorReport::default();
    report.items.push(check_version());
    report.items.push(check_binary_path());
    report.items.push(check_config());
    report.items.push(check_catalog());
    report.items.push(check_quarantine());
    if cfg!(target_os = "linux") {
        report.items.push(check_glibc_hint());
    }
    report
}

pub fn print_doctor_report(report: &DoctorReport) {
    println!("Dock 诊断（DOCK_HOME={}）", dock_home().display());
    println!();
    for item in &report.items {
        let mark = match item.status {
            CheckStatus::Pass => "✓",
            CheckStatus::Warn => "!",
            CheckStatus::Fail => "✗",
        };
        println!("{mark} {}: {}", item.name, item.detail);
    }
    println!();
    if report.ok() && report.exit_code() == 0 {
        println!("全部通过。");
    } else {
        println!("存在问题，请按上方提示处理后再试。");
    }
}

fn check_version() -> CheckItem {
    let ver = env!("CARGO_PKG_VERSION");
    match Command::new(std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dock")))
        .arg("--version")
        .output()
    {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            CheckItem {
                name: "版本自检".into(),
                status: CheckStatus::Pass,
                detail: if text.is_empty() {
                    format!("dock {ver}")
                } else {
                    text
                },
            }
        }
        Ok(out) => CheckItem {
            name: "版本自检".into(),
            status: CheckStatus::Fail,
            detail: format!(
                "--version 失败：{}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        },
        Err(e) => CheckItem {
            name: "版本自检".into(),
            status: CheckStatus::Fail,
            detail: format!("无法执行自身 --version：{e}"),
        },
    }
}

fn check_binary_path() -> CheckItem {
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "（未知）".into());
    let on_path = which_dock();
    match on_path {
        Some(p) => CheckItem {
            name: "PATH / 二进制".into(),
            status: CheckStatus::Pass,
            detail: format!("PATH 命中 {}；当前进程 {}", p.display(), exe),
        },
        None => CheckItem {
            name: "PATH / 二进制".into(),
            status: CheckStatus::Warn,
            detail: format!(
                "PATH 上找不到 dock；当前进程 {}。把安装目录加进 PATH，或用绝对路径调用。",
                exe
            ),
        },
    }
}

fn which_dock() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("dock");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn check_config() -> CheckItem {
    let path = dock_home().join("config.toml");
    if path.is_file() {
        CheckItem {
            name: "配置文件".into(),
            status: CheckStatus::Pass,
            detail: format!("存在 {}", path.display()),
        }
    } else {
        CheckItem {
            name: "配置文件".into(),
            status: CheckStatus::Fail,
            detail: format!(
                "缺少 {}。运行 dock init 生成骨架，或复制 config.toml.example。",
                path.display()
            ),
        }
    }
}

fn check_catalog() -> CheckItem {
    let catalog = load_catalog();
    if catalog.is_empty() {
        return CheckItem {
            name: "模型目录".into(),
            status: CheckStatus::Fail,
            detail: "目录为空。在 config.toml 里配置 [model.*] 或 [[models.catalog]]，参考 config.toml.example。".into(),
        };
    }
    let usable = catalog.iter().filter(|m| ModelChoice::has_http(m)).count();
    if usable == 0 {
        CheckItem {
            name: "模型目录".into(),
            status: CheckStatus::Fail,
            detail: format!(
                "有 {} 条条目，但都缺少 api_base_url / env_key（或等价协议块）。无法发起请求。",
                catalog.len()
            ),
        }
    } else {
        let keyed = catalog
            .iter()
            .filter(|m| m.resolved_api_key().is_some())
            .count();
        let detail = if keyed == 0 {
            format!(
                "{} 条可用端点，但当前环境解析不到密钥（检查 env_key 对应的环境变量或 secrets）。",
                usable
            )
        } else {
            format!("{} 条目录，其中 {} 条已解析到密钥。", catalog.len(), keyed)
        };
        let status = if keyed == 0 {
            CheckStatus::Warn
        } else {
            CheckStatus::Pass
        };
        CheckItem {
            name: "模型目录".into(),
            status,
            detail,
        }
    }
}

fn check_quarantine() -> CheckItem {
    #[cfg(target_os = "macos")]
    {
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(_) => {
                return CheckItem {
                    name: "macOS quarantine".into(),
                    status: CheckStatus::Warn,
                    detail: "无法定位当前二进制，跳过 quarantine 检查。".into(),
                };
            }
        };
        match Command::new("xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(&exe)
            .output()
        {
            Ok(out) if out.status.success() => CheckItem {
                name: "macOS quarantine".into(),
                status: CheckStatus::Warn,
                detail: format!(
                    "带有 com.apple.quarantine。若 Gatekeeper 拦启动，执行：\n  xattr -d com.apple.quarantine \"{}\"",
                    exe.display()
                ),
            },
            _ => CheckItem {
                name: "macOS quarantine".into(),
                status: CheckStatus::Pass,
                detail: "未检测到 quarantine 标记。".into(),
            },
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        CheckItem {
            name: "macOS quarantine".into(),
            status: CheckStatus::Pass,
            detail: "非 macOS，跳过。".into(),
        }
    }
}

fn check_glibc_hint() -> CheckItem {
    // 预编译 Linux 产物要求较新 glibc；这里只做提示性检查。
    match Command::new("ldd").arg("--version").output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let line = text.lines().next().unwrap_or("").trim();
            CheckItem {
                name: "glibc".into(),
                status: CheckStatus::Pass,
                detail: format!(
                    "{line}（预编译产物通常需要 glibc ≥ 2.39 与 OpenSSL 3；过旧请从源码构建）"
                ),
            }
        }
        Err(_) => CheckItem {
            name: "glibc".into(),
            status: CheckStatus::Warn,
            detail:
                "无法运行 ldd --version；若 dock 启动报 GLIBC_ 缺符号，请升级系统或从源码构建。"
                    .into(),
        },
    }
}

#[allow(dead_code)]
fn path_exists(p: &Path) -> bool {
    p.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_base::test_env;
    use std::fs;

    #[test]
    fn doctor_fails_without_config() {
        let _env = test_env::scoped().home();
        let report = run_doctor();
        assert!(
            report
                .items
                .iter()
                .any(|i| i.name == "配置文件" && i.status == CheckStatus::Fail),
            "{:?}",
            report.items
        );
        assert_ne!(report.exit_code(), 0);
    }

    #[test]
    fn doctor_catalog_fail_when_empty_config() {
        let _env = test_env::scoped().home();
        let home = dock_home();
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("config.toml"), "# empty\n").unwrap();
        let report = run_doctor();
        assert!(
            report
                .items
                .iter()
                .any(|i| i.name == "模型目录" && i.status == CheckStatus::Fail),
            "{:?}",
            report.items
        );
        assert_ne!(report.exit_code(), 0);
    }

    #[test]
    fn doctor_pass_path_when_model_and_key_ready() {
        let dir = tempfile::tempdir().unwrap();
        let _env = test_env::scoped()
            .set("DOCK_HOME", dir.path())
            .set("DOCK_DOCTOR_TEST_KEY", "sk-test");
        fs::write(
            dir.path().join("config.toml"),
            r#"
[models]
default = "m"

[model.m]
name = "m"
api_base_url = "https://example.test/v1"
api_backends = ["responses"]
env_key = "DOCK_DOCTOR_TEST_KEY"
"#,
        )
        .unwrap();
        let report = run_doctor();
        let catalog = report
            .items
            .iter()
            .find(|i| i.name == "模型目录")
            .expect("catalog check");
        assert_eq!(catalog.status, CheckStatus::Pass, "{catalog:?}");
        let config = report
            .items
            .iter()
            .find(|i| i.name == "配置文件")
            .expect("config check");
        assert_eq!(config.status, CheckStatus::Pass, "{config:?}");
    }
}
