//! 订阅通道的接线：`/auth` 命令、鉴权服务、provider 插件。
//!
//! 这条命令是**宿主无关**的：终端与桌面端跑的是同一个命令体，所以它必须
//! 真的登记进 `"slash"` 服务——挂了插件却在命令表里找不到，用户就没法用。

use cordis::Context;

#[tokio::test]
async fn auth_command_and_provider_are_mounted() {
    // 隔离的 DOCK_HOME：`install_app` 挂上 chatgpt provider 时会看活动账号、按需
    // 重拉模型列表——不能读到开发机上真的 ChatGPT 凭据、去打 OpenAI、改真的
    // `~/.dock/auth_models.json`。本二进制只有这一个测试，直接设环境变量就行。
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("DOCK_HOME", home.path());
    std::env::set_var("DOCK_CUA_DRIVER", "off");

    let ctx = Context::new();
    cordis_spine::install_app(&ctx).await.unwrap();

    let slash = ctx
        .get::<cordis_spine::Slash>(cordis_spine::SLASH)
        .expect("slash 服务");
    let names: Vec<String> = slash.commands().iter().map(|c| c.name.clone()).collect();
    assert!(
        names.contains(&"auth".to_string()),
        "命令表里没有 /auth：{names:?}"
    );
    let cmd = slash
        .commands()
        .into_iter()
        .find(|c| c.name == "auth")
        .unwrap();
    assert!(cmd.takes_args, "/auth 要能带子命令");
    // 描述与终端命令表（cordis-tui 的 CATALOG）一字不差，TUI 有测试守着这条。
    assert_eq!(
        cmd.description,
        "订阅通道：登录 / 登出 / 状态 / 账号 / 模型（直接回车 = 登录）"
    );

    let manager = ctx
        .get::<cordis_spine::AuthManager>(cordis_spine::AUTH)
        .expect("鉴权服务");
    assert!(
        manager.provider_ids().contains(&"chatgpt".to_string()),
        "chatgpt provider 没注册进来"
    );
}
