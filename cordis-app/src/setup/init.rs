//! `dock init`：幂等创建 `$DOCK_HOME`，仅在缺省时写最小 config 骨架。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cordis_base::config::{dock_home, load_catalog, ModelChoice};

/// 最小骨架：只用 env_key 占位，禁止明文 api_key；完整样例见仓库 config.toml.example。
const CONFIG_SKELETON: &str = r#"# 由 `dock init` 生成。完整样例与字段说明：
#   https://github.com/P0lar1ght/dock/blob/main/config.toml.example
#
# 密钥只用 env_key（环境变量）或 $DOCK_HOME/secrets.json，不要写 api_key。
# 取消下面注释并改成你的端点后，再 export 对应环境变量，然后运行 `dock doctor`。

[models]
# default = "my-model"

# [model."my-model"]
# name = "我的模型"
# description = "按需修改"
# api_base_url = "https://<your-endpoint>/v1"
# api_backends = ["responses"]
# api_model = "<上游真实模型名>"
# env_key = "YOUR_API_KEY_ENV"
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitOutcome {
    /// 新建了 DOCK_HOME 与 config 骨架。
    Created { home: PathBuf, config: PathBuf },
    /// DOCK_HOME 已在，config 也已在：未覆盖。
    AlreadyConfigured { home: PathBuf, config: PathBuf },
    /// 建了目录，但 config 已存在（未覆盖）。
    HomeReadyConfigKept { home: PathBuf, config: PathBuf },
}

impl InitOutcome {
    pub fn home(&self) -> &Path {
        match self {
            Self::Created { home, .. }
            | Self::AlreadyConfigured { home, .. }
            | Self::HomeReadyConfigKept { home, .. } => home,
        }
    }

    pub fn config(&self) -> &Path {
        match self {
            Self::Created { config, .. }
            | Self::AlreadyConfigured { config, .. }
            | Self::HomeReadyConfigKept { config, .. } => config,
        }
    }

    pub fn wrote_config(&self) -> bool {
        matches!(self, Self::Created { .. })
    }
}

pub fn run_init() -> io::Result<InitOutcome> {
    let home = dock_home();
    let config = home.join("config.toml");
    let home_existed = home.exists();
    let config_existed = config.exists();

    fs::create_dir_all(&home)?;
    // 常用子目录：会话 / 密钥库占位目录语义由其它命令创建；这里只保证 home 本身。
    let _ = fs::create_dir_all(home.join("sessions"));

    let outcome = if config_existed {
        if home_existed {
            InitOutcome::AlreadyConfigured {
                home: home.clone(),
                config: config.clone(),
            }
        } else {
            InitOutcome::HomeReadyConfigKept {
                home: home.clone(),
                config: config.clone(),
            }
        }
    } else {
        // 原子写：先 .tmp 再 rename，避免半截文件。
        let staged = home.join("config.toml.init-new");
        fs::write(&staged, CONFIG_SKELETON)?;
        fs::rename(&staged, &config)?;
        InitOutcome::Created {
            home: home.clone(),
            config: config.clone(),
        }
    };

    Ok(outcome)
}

/// 打印中文结果；配不齐模型时给出下一步（不静默成功）。
pub fn print_init_report(outcome: &InitOutcome) {
    let home = outcome.home();
    let config = outcome.config();
    match outcome {
        InitOutcome::Created { .. } => {
            println!("已初始化 Dock 主目录：{}", home.display());
            println!("已写入最小配置骨架：{}", config.display());
            println!("（未写入任何 api_key；请用 env_key 或 secrets.json）");
        }
        InitOutcome::AlreadyConfigured { .. } => {
            println!(
                "配置已存在，跳过写入：{}（不会覆盖已有 config.toml）",
                config.display()
            );
        }
        InitOutcome::HomeReadyConfigKept { .. } => {
            println!("已确保主目录：{}", home.display());
            println!(
                "配置已存在，跳过写入：{}（不会覆盖已有 config.toml）",
                config.display()
            );
        }
    }

    print_model_next_steps(config);
}

fn print_model_next_steps(config: &Path) {
    let catalog = load_catalog();
    if catalog_ready(&catalog) {
        println!(
            "模型目录已可用（{} 条）。下一步：运行 dock 或 dock doctor。",
            catalog.len()
        );
        return;
    }

    println!();
    println!("下一步（模型尚未配齐，现在还不能直接对话）：");
    println!("  1. 编辑 {}", config.display());
    println!("     参考完整样例：");
    println!("     https://github.com/P0lar1ght/dock/blob/main/config.toml.example");
    println!("  2. 在模型条目里设置 env_key（例如 YOUR_API_KEY_ENV），");
    println!("     然后在 shell 里：export YOUR_API_KEY_ENV=sk-...");
    println!(
        "     插件密钥也可放进 {}/secrets.json（扁平 name→string）。",
        dock_home().display()
    );
    println!("  3. 运行 dock doctor 确认 PATH / 配置 / 模型目录。");
    if catalog.is_empty() {
        println!("  （当前模型目录为空：没有可用的 [[models.catalog]] / [model.*] 条目）");
    } else {
        println!(
            "  （当前有 {} 条目录，但缺少可用端点或密钥——请检查 api_base_url 与 env_key）",
            catalog.len()
        );
    }
}

/// 至少有一条「配了 HTTP 端点或密钥来源」的模型，才算能继续。
fn catalog_ready(catalog: &[ModelChoice]) -> bool {
    catalog.iter().any(ModelChoice::has_http)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_base::test_env;

    #[test]
    fn init_creates_skeleton_without_api_key() {
        let _env = test_env::scoped().home();
        let home = dock_home();
        assert!(!home.join("config.toml").exists());

        let once = run_init().unwrap();
        assert!(once.wrote_config());
        let text = fs::read_to_string(once.config()).unwrap();
        assert!(text.contains("env_key"));
        assert!(!text.lines().any(|l| {
            let t = l.trim();
            t.starts_with("api_key") && !t.starts_with('#')
        }));
        assert!(text.contains("config.toml.example") || text.contains("P0lar1ght/dock"));
    }

    #[test]
    fn init_idempotent_does_not_overwrite() {
        let _env = test_env::scoped().home();
        run_init().unwrap();
        let config = dock_home().join("config.toml");
        let marker = "# user-marker-do-not-touch\n";
        fs::write(&config, marker).unwrap();

        let again = run_init().unwrap();
        assert!(!again.wrote_config());
        assert_eq!(fs::read_to_string(&config).unwrap(), marker);
    }
}
