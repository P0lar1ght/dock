use cordis::Context;
use cordis_base::acp::PermissionOptionKind;
use cordis_spine::{
    agent_loop, agent_presets, dynamic_runner, llm, permissions, settings, slash, tool_cordis,
    tool_todo, tools, tui_slots, AgentPresets, DynEcho, DynamicRunner, LlmConfig, LlmMode,
    LogEvent, LoopHandle, PermissionMode, Permissions, PersistScope, PluginOrigin, PluginSel,
    PreStep, RhaiBag, RunMode, Sessions, Slash, SourceInput, ToolCall, Tools, TurnOutcome,
    AGENT_LOOP, AGENT_PRESETS, CORDIS_PRESET_ID, DYNAMIC_CORDIS_RUNNER, DYN_ECHO, DYN_ECHO_TOOL,
    PERMISSIONS, PRE_STEP, SESSIONS, SETTINGS, SLASH, TOOLS, TUI_SLOTS,
};
use serde_json::json;

const MAIN: &str = "main";

async fn boot() -> Context {
    boot_with(tools()).await
}

/// 带真权限门的工具表（`workspace_tools`）：`boot()` 用的 echo 工具表不过门。
async fn boot_gated() -> Context {
    boot_with(cordis_spine::workspace_tools()).await
}

async fn boot_with(tool_table: cordis::Plugin) -> Context {
    let root = Context::new();
    cordis_spine::install_core(&root).await.unwrap();
    root.plugin(tool_table, ()).unwrap().wait().await.unwrap();
    root.plugin(settings(), ()).unwrap().wait().await.unwrap();
    root.plugin(permissions(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(slash(), ()).unwrap().wait().await.unwrap();
    root.plugin(tui_slots(), ()).unwrap().wait().await.unwrap();
    for p in [cordis_spine::status_items(), cordis_spine::tool_views()] {
        root.plugin(p, ()).unwrap().wait().await.unwrap();
    }
    // 设置卡的值落在临时文件，不碰本机 `$DOCK_HOME`。
    let settings_file = std::env::temp_dir().join(format!(
        "dock-dynamic-plugin-settings-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = root.provide(
        cordis_spine::PLUGIN_SETTINGS,
        cordis_spine::PluginSettings::at(settings_file),
    );
    root.plugin(dynamic_runner(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(tool_cordis(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.get::<cordis_spine::AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Allow);
    // 根会话钉一个临时项目目录：`cordis_plugin` 在 `<它>/.dock/plugins/` 下找插件，
    // 不碰进程 cwd，也不碰真实项目。TempDir 挂在树上，树在它就在。
    let project = tempfile::tempdir().unwrap();
    root.require::<Sessions>(SESSIONS)
        .unwrap()
        .pin_workspace_cwd(project.path());
    let _ = root.provide("dynamicTestProject", project);
    root
}

fn runner(root: &Context) -> std::sync::Arc<DynamicRunner> {
    root.require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap()
}

/// 这棵树的项目级插件根：`<项目>/.dock/plugins`。
fn plugins_root(root: &Context) -> std::path::PathBuf {
    root.require::<Sessions>(SESSIONS)
        .unwrap()
        .workspace_cwd()
        .expect("boot pins a project")
        .join(".dock/plugins")
}

/// 往项目级插件目录里写文件（模型用 write_file 做的就是这一步）。
fn write_plugin(root: &Context, id: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = plugins_root(root).join(id);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir
}

async fn reload(root: &Context, id: &str) -> String {
    exec_json(root, "cordis_plugin", json!({"action": "reload", "id": id})).await
}

/// 模型的真实路径：写 `source.rhai`，`cordis_plugin reload`，要求跑起来。
async fn load_plugin(root: &Context, id: &str, source: &str) -> String {
    write_plugin(root, id, &[("source.rhai", source)]);
    let out = reload(root, id).await;
    assert!(out.contains("运行中"), "{out}");
    out
}

/// 程序侧 define（runner API 还在，测试和设置页用）：返回 (pluginId, packageId)。
fn define(
    root: &Context,
    plugin: PluginSel,
    factory: &str,
    source: Option<&str>,
) -> Result<(String, String), String> {
    runner(root)
        .define(
            MAIN,
            plugin,
            "Probe",
            "probe",
            factory,
            None,
            source.map(|s| SourceInput::Inline(s.to_string())),
        )
        .map(|r| (r.plugin_id, r.package_id))
}

fn new_plugin(prefix: &str) -> PluginSel {
    PluginSel::New {
        id_prefix: prefix.into(),
    }
}

fn tools_of(root: &Context) -> std::sync::Arc<Tools> {
    root.require::<Tools>(TOOLS).unwrap()
}

async fn exec(root: &Context, name: &str, args: &str) -> String {
    tools_of(root)
        .execute(ToolCall {
            id: "t".into(),
            name: name.into(),
            arguments: args.into(),
        })
        .await
        .content
}

#[tokio::test]
async fn define_does_not_mount() {
    let root = boot().await;
    let (plugin, package) = define(&root, new_plugin("echo"), "echo", None).unwrap();
    assert_eq!((plugin.as_str(), package.as_str()), ("echo-1", "pkg-1"));
    assert!(root.get::<DynEcho>(DYN_ECHO).is_none());
    let names: Vec<_> = tools_of(&root)
        .specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(!names.iter().any(|n| n == DYN_ECHO_TOOL), "{names:?}");
}

/// 主路径：写目录 → reload → 工具在、inspect 看得到登记了什么 → disable 撤下、
/// `plugin.toml` 写 `enabled = false`、文件还在。
#[tokio::test]
async fn reload_mounts_and_disable_unmounts_a_plugin_directory() {
    let root = boot().await;
    let dir = write_plugin(
        &root,
        "echo",
        &[(
            "plugin.toml",
            "name = \"Echo\"\npurpose = \"ping\"\nfactory = \"echo\"\n",
        )],
    );
    let ran = reload(&root, "echo").await;
    assert!(ran.contains("已重载插件 echo"), "{ran}");
    assert!(ran.contains("运行中"), "{ran}");
    assert!(
        ran.contains(&format!("登记：工具 {DYN_ECHO_TOOL}")),
        "{ran}"
    );
    assert!(ran.contains("use_tool"), "下一步该是试调：{ran}");
    assert!(root.get::<DynEcho>(DYN_ECHO).is_some());
    assert_eq!(exec(&root, DYN_ECHO_TOOL, "{}").await, "pong");

    let overview = exec(&root, "cordis_inspect", "{}").await;
    assert!(overview.contains("- echo [project] running"), "{overview}");
    assert!(
        overview.contains("more: cordis_inspect plugin"),
        "{overview}"
    );
    let detail = exec(&root, "cordis_inspect", r#"{"plugin":"echo"}"#).await;
    assert!(detail.contains("status: running"), "{detail}");
    assert!(
        detail.contains(&format!("registered: 工具 {DYN_ECHO_TOOL}")),
        "{detail}"
    );
    assert!(detail.contains("factory: echo"), "{detail}");
    let services = exec(&root, "cordis_inspect", r#"{"what":"services"}"#).await;
    assert!(services.contains("dynEcho"), "{services}");
    assert!(services.contains("dynamicCordisRunner"), "{services}");
    assert!(services.contains("host.register_tool"), "{services}");
    assert!(services.contains("cannot replace /agents"), "{services}");
    assert!(services.contains("host.open_slot"), "{services}");
    let tools_only = exec(&root, "cordis_inspect", r#"{"what":"tools"}"#).await;
    assert!(!tools_only.contains("host.register_tool"), "{tools_only}");

    let off = exec_json(
        &root,
        "cordis_plugin",
        json!({"action":"disable","id":"echo"}),
    )
    .await;
    assert!(off.contains("已停用插件 echo"), "{off}");
    assert!(root.get::<DynEcho>(DYN_ECHO).is_none());
    let names: Vec<_> = tools_of(&root)
        .specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(!names.iter().any(|n| n == DYN_ECHO_TOOL), "{names:?}");
    let manifest = std::fs::read_to_string(dir.join("plugin.toml")).unwrap();
    assert!(manifest.contains("enabled = false"), "{manifest}");
    let detail = exec(&root, "cordis_inspect", r#"{"plugin":"echo"}"#).await;
    assert!(detail.contains("status: disabled"), "{detail}");

    // reload 就是启用：开关改回 true，又跑起来。
    let back = reload(&root, "echo").await;
    assert!(back.contains("enabled = false，已改成 true"), "{back}");
    assert!(root.get::<DynEcho>(DYN_ECHO).is_some());
}

/// 两个插件抢同一个名字：后来的那个 reload 失败、回执点名怎么解决，先来的照跑。
#[tokio::test]
async fn second_echo_fails_and_does_not_leave_a_fiber() {
    let root = boot().await;
    let manifest = "factory = \"echo\"\n";
    write_plugin(&root, "echo-a", &[("plugin.toml", manifest)]);
    write_plugin(&root, "echo-b", &[("plugin.toml", manifest)]);
    reload(&root, "echo-a").await;
    let failed = reload(&root, "echo-b").await;
    assert!(
        failed.contains("has been registered") || failed.contains("already registered"),
        "{failed}"
    );
    assert!(failed.contains("cordis_plugin disable"), "{failed}");
    assert!(failed.contains("现在没有在跑的版本"), "{failed}");
    assert!(root.get::<DynEcho>(DYN_ECHO).is_some());
    let overview = exec(&root, "cordis_inspect", "{}").await;
    assert!(
        overview.contains("- echo-a [project] running"),
        "{overview}"
    );
    assert!(overview.contains("- echo-b [project] failed"), "{overview}");
    assert!(runner(&root)
        .inspect_plugin(MAIN, "echo-b")
        .unwrap()
        .active_run
        .is_none());
}

/// 不带参数是概览（插件在哪、各自什么状态），`plugin` 看一个，`what` 看参考段；
/// 问错了要报错而不是静默回目录。
#[tokio::test]
async fn inspect_overview_plugin_and_sections() {
    let root = boot().await;
    write_plugin(
        &root,
        "both",
        &[("plugin.toml", "name = \"Both\"\nfactory = \"note\"\n")],
    );

    let overview = exec(&root, "cordis_inspect", "{}").await;
    assert!(overview.contains("root project:"), "{overview}");
    assert!(
        overview.contains("- both [project] not loaded — Both"),
        "{overview}"
    );
    assert!(
        !overview.contains("host.register_tool"),
        "概览不该塞整份参考：{overview}"
    );

    let plugin = exec(&root, "cordis_inspect", r#"{"plugin":"both"}"#).await;
    assert!(plugin.contains("plugin: both"), "{plugin}");
    assert!(plugin.contains("factory: note"), "{plugin}");
    assert!(plugin.contains("status: not loaded"), "{plugin}");

    // plugin 比 what 窄，窄的赢。
    let both = exec(
        &root,
        "cordis_inspect",
        r#"{"what":"builtins","plugin":"both"}"#,
    )
    .await;
    assert!(both.contains("plugin: both"), "{both}");
    assert!(!both.contains("builtins:"), "{both}");

    let missing = exec(&root, "cordis_inspect", r#"{"plugin":"nope"}"#).await;
    assert!(missing.contains("没找到插件目录 nope"), "{missing}");
    let unknown = exec(&root, "cordis_inspect", r#"{"what":"fibers"}"#).await;
    assert!(unknown.contains("不认得"), "{unknown}");
    let events = exec(&root, "cordis_inspect", r#"{"what":"events"}"#).await;
    for hook in [
        "session/event",
        "agent/step-start",
        "agent/turn-end",
        "agent/pre-step",
        "system-prompt/assemble",
        "tools/pre-execute",
        "tools/execute",
        "llm/stream",
    ] {
        assert!(events.contains(hook), "{hook}: {events}");
    }
}

/// 程序侧 API：stop 保留版本，undefine 才丢定义；两者都不碰磁盘。
#[tokio::test]
async fn stop_keeps_the_definition_and_undefine_drops_it() {
    let root = boot().await;
    let (id, pkg) = define(&root, new_plugin("keep"), "echo", None).unwrap();
    let runner = runner(&root);
    runner.run(MAIN, &id, &pkg, RunMode::Run).await.unwrap();
    let stopped = runner.stop(MAIN, &id).await.unwrap();
    assert!(stopped.was_running);
    let row = runner.inspect_plugin(MAIN, &id).unwrap();
    assert_eq!(row.current_package_id.as_deref(), Some(pkg.as_str()));
    assert!(row.active_run.is_none());
    // 停下来的包可以再 run 起来，这正是不 undefine 的理由。
    runner.run(MAIN, &id, &pkg, RunMode::Run).await.unwrap();
    let dropped = runner.undefine(MAIN, &id).await.unwrap();
    assert!(dropped.was_running);
    assert!(runner.inspect_plugin(MAIN, &id).is_err());
}

/// 依赖没挂上：回执说「在等依赖」，点名等谁，下一步教怎么排查。
#[tokio::test]
async fn hold_stays_pending() {
    let root = boot().await;
    write_plugin(&root, "hold", &[("plugin.toml", "factory = \"hold\"\n")]);
    let ran = reload(&root, "hold").await;
    assert!(ran.contains("在等依赖"), "{ran}");
    assert!(ran.contains("dynHoldGate"), "{ran}");
    let runner = runner(&root);
    let row = runner.inspect_plugin(MAIN, "hold").unwrap();
    let run = row.active_run.expect("hold should keep a fiber");
    assert_eq!(run.waiting_for, ["dynHoldGate"]);
    let fiber = runner
        .live_fibers(MAIN)
        .into_iter()
        .find(|f| f.plugin_id.as_deref() == Some("hold"))
        .expect("hold fiber");
    assert_eq!(fiber.state, "pending");
    let detail = exec(&root, "cordis_inspect", r#"{"plugin":"hold"}"#).await;
    assert!(detail.contains("waiting for: dynHoldGate"), "{detail}");
}

#[tokio::test]
async fn update_mode_and_drop() {
    let root = boot().await;
    let (id, first) = define(&root, new_plugin("note"), "note", None).unwrap();
    let (_, second) = define(
        &root,
        PluginSel::Existing {
            plugin_id: id.clone(),
        },
        "note",
        None,
    )
    .unwrap();
    let runner = runner(&root);
    runner.run(MAIN, &id, &first, RunMode::Run).await.unwrap();
    let bad = runner
        .run(MAIN, &id, &second, RunMode::Run)
        .await
        .unwrap_err();
    assert!(bad.contains("update"), "{bad}");
    let updated = runner
        .run(MAIN, &id, &second, RunMode::Update)
        .await
        .unwrap();
    assert_eq!(updated.current_package_id.as_deref(), Some(second.as_str()));
    runner.undefine(MAIN, &id).await.unwrap();
    assert!(runner.snapshot(MAIN).iter().all(|r| r.plugin_id != id));
}

#[tokio::test]
async fn slash_factory_adds_and_stop_removes() {
    let root = boot().await;
    write_plugin(
        &root,
        "standup",
        &[(
            "plugin.toml",
            "name = \"Standup\"\npurpose = \"站会\"\nfactory = \"slash\"\ncommand = \"standup\"\nkind = \"prompt\"\ntext = \"写站会：{args}\"\nsend = true\n",
        )],
    );
    let ran = reload(&root, "standup").await;
    assert!(ran.contains("运行中"), "{ran}");
    assert!(ran.contains("斜杠 /standup"), "{ran}");
    let slash = root.require::<Slash>(SLASH).unwrap();
    assert!(
        slash.list().iter().any(|e| e.command == "standup"),
        "{:?}",
        slash
            .list()
            .iter()
            .map(|e| e.command.clone())
            .collect::<Vec<_>>()
    );
    let inspect = exec(&root, "cordis_inspect", r#"{"what":"services"}"#).await;
    assert!(inspect.contains("/standup"), "{inspect}");

    exec_json(
        &root,
        "cordis_plugin",
        json!({"action":"disable","id":"standup"}),
    )
    .await;
    assert!(slash.list().is_empty());
}

#[tokio::test]
async fn slash_tool_kind_binds_live_tool_name() {
    let root = boot().await;
    write_plugin(&root, "echo", &[("plugin.toml", "factory = \"echo\"\n")]);
    reload(&root, "echo").await;
    write_plugin(
        &root,
        "probe",
        &[(
            "plugin.toml",
            &format!(
                "name = \"Test\"\npurpose = \"probe\"\nfactory = \"slash\"\ncommand = \"probe\"\nkind = \"tool\"\ntext = \"{DYN_ECHO_TOOL}\"\n"
            ),
        )],
    );
    let ran = reload(&root, "probe").await;
    assert!(ran.contains("运行中"), "{ran}");
    let slash = root.require::<Slash>(SLASH).unwrap();
    let entry = slash
        .list()
        .into_iter()
        .find(|e| e.command == "probe")
        .unwrap_or_else(|| {
            panic!(
                "probe slash missing; list={:?}",
                slash
                    .list()
                    .iter()
                    .map(|e| (e.command.clone(), e.kind.as_str(), e.text.clone()))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(entry.kind.as_str(), "tool");
    assert_eq!(entry.text, DYN_ECHO_TOOL);

    slash.queue_notice("t", "body");
    assert_eq!(slash.take_notice(), Some(("t".into(), "body".into())));
    assert!(slash.take_notice().is_none());
}

#[tokio::test]
async fn slash_cannot_shadow_help() {
    let root = boot().await;
    write_plugin(
        &root,
        "nope",
        &[(
            "plugin.toml",
            "factory = \"slash\"\ncommand = \"help\"\nkind = \"overlay\"\ntext = \"x\"\n",
        )],
    );
    let out = reload(&root, "nope").await;
    assert!(out.contains("cannot shadow"), "{out}");
    assert!(root.require::<Slash>(SLASH).unwrap().list().is_empty());
}

#[tokio::test]
async fn echo_rejects_slash_fields() {
    let root = boot().await;
    write_plugin(
        &root,
        "echo",
        &[("plugin.toml", "factory = \"echo\"\ncommand = \"standup\"\n")],
    );
    let out = reload(&root, "echo").await;
    assert!(out.contains("only for factory"), "{out}");
}

async fn exec_json(root: &Context, name: &str, v: serde_json::Value) -> String {
    exec(root, name, &v.to_string()).await
}

const RHAI_MEMO: &str = r#"#{
    inject: ["tools", "tui.slots"],
    apply: |host| {
        host.provide("dynMemo", #{ text: "hi" });
        host.register_tool(#{
            name: "memo_get",
            description: "Read the memo bag",
            parameters: #{ type: "object", properties: #{} },
            execute: |args| { "memo-ok" },
            view: |tc| #{ type: "kv", items: [
                #{ label: "输出", value: tc.output },
                #{ label: "失败", value: tc.failed }
            ] }
        });
        host.register_tool_view("bash", |tc| ());
        host.register_slot(#{
            id: "memo",
            title: "便签",
            hud: true,
            render: || { "hello slot" },
            actions: [#{ id: "refresh", label: "刷新" }, #{ id: "clear" }],
            view: || #{ type: "stack", children: [
                #{ type: "kv", items: [#{ label: "条数", value: 3 }] },
                #{ type: "button", label: "归档", action: "archive" }
            ] }
        });
        host.register_slot(#{
            id: "later",
            title: "稍后",
            render: || { "还没准备好" },
            view: || ()
        });
        host.slot_changed("memo");
        host.register_settings(#{ title: "便签", fields: [
            #{ key: "limit", type: "number", min: 1, max: 50, "default": 10 },
            #{ key: "memo-token", type: "secret", label: "令牌" }
        ] });
        host.provide("dynMemoLimit", #{ limit: host.setting("limit"), none: host.setting("nope") });
        host.set_status(#{ id: "memo-count", text: "2 条", tone: "accent", surface: "memo" });
        host.set_status(#{ id: "memo-count", text: "3 条", tone: "success", surface: "memo" });
        host.open_slot("memo");
    }
}"#;

/// 编译不过：回执带原因和「按行号改、再 reload」，什么都没装上。
#[tokio::test]
async fn rhai_compile_reject_does_not_mint() {
    let root = boot().await;
    write_plugin(&root, "bad", &[("source.rhai", "this is not { rhai")]);
    let out = reload(&root, "bad").await;
    assert!(out.contains("这一版没跑起来"), "{out}");
    assert!(out.contains("source must be a map"), "{out}");
    assert!(out.contains("does not call apply"), "{out}");
    assert!(out.contains("cordis_plugin reload bad"), "{out}");
    assert!(out.contains("别另起一个插件目录"), "{out}");
    assert!(runner(&root).inspect_plugin(MAIN, "bad").is_err());
}

/// `inject` 进的是**内核**的 `Inject`，内核按名字查 store（类型擦除），所以一个
/// 脚本够不着的 spine 服务照样能满足它——fiber active、apply 跑过、工具可调。
/// 以前 `waiting for` 拿 `service_present` 那张只认几个名字的表单独判，于是这种包
/// 被报成 waiting，模型会去修一个没坏的插件。
#[tokio::test]
async fn injecting_a_mounted_spine_service_runs_and_is_not_reported_as_waiting() {
    let root = boot().await;
    let ran = load_plugin(
        &root,
        "gate",
        r#"
#{
  inject: ["tools", "settings", "permissions"],
  apply: |host| {
    host.register_tool(#{
      name: "gate_ping",
      description: "probe",
      parameters: #{ type: "object", properties: #{} },
      execute: |args| { "pong" }
    });
  }
}
"#,
    )
    .await;
    assert!(
        !ran.contains("在等依赖"),
        "a resolved inject is not a wait: {ran}"
    );
    let detail = exec(&root, "cordis_inspect", r#"{"plugin":"gate"}"#).await;
    assert!(detail.contains("status: running"), "{detail}");
    assert!(!detail.contains("waiting for"), "{detail}");

    // 真跑起来了，不只是状态好看。
    assert_eq!(exec(&root, "gate_ping", "{}").await, "pong");

    // 而脚本**仍然**够不着那两个名字：inject 是启动闸，不是授权。
    let services = exec(&root, "cordis_inspect", r#"{"what":"services"}"#).await;
    let (reachable, mounted) = services
        .split_once("also mounted")
        .expect("services must split reachable from merely mounted");
    for name in ["settings", "permissions", "context", "sessions"] {
        assert!(
            mounted.contains(name),
            "{name} belongs under 'also mounted': {services}"
        );
        assert!(
            !reachable.contains(&format!("- {name}")),
            "{name} must not be listed as reachable from Rhai: {services}"
        );
    }
    for name in ["tools", "slash", "tui.slots"] {
        assert!(
            reachable.contains(&format!("- {name}")),
            "{name} is reachable: {services}"
        );
    }
    assert!(
        reachable.contains("host.register_tool") && reachable.contains("host.call_tool"),
        "reachable names carry their host.* signatures: {services}"
    );
}

/// 超限时该说的是「写成插件目录里的 source.rhai」，不是「把脚本写短点」；
/// 文件本身超过 1MiB 也要说清怎么拆。
#[tokio::test]
async fn oversized_sources_say_where_to_go() {
    let root = boot().await;
    let big = format!(
        "#{{ inject: [], apply: |host| {{ }} }}\n// {}",
        "x".repeat(130 * 1024)
    );
    let err = define(&root, new_plugin("big"), "rhai", Some(&big)).unwrap_err();
    assert!(err.contains("128KiB"), "{err}");
    assert!(err.contains("source.rhai"), "{err}");
    assert!(err.contains("cordis_plugin reload"), "{err}");
    assert!(runner(&root)
        .snapshot(MAIN)
        .iter()
        .all(|r| !r.plugin_id.starts_with("big")));

    let huge = format!(
        "#{{ inject: [], apply: |host| {{ }} }}\n// {}",
        "x".repeat(1024 * 1024 + 1)
    );
    write_plugin(&root, "huge", &[("source.rhai", &huge)]);
    let out = reload(&root, "huge").await;
    assert!(out.contains("1MiB"), "{out}");
    assert!(out.contains("host.read_bytes"), "{out}");
}

/// 改坏一行不该让用户手上的工具一起消失：新版 apply 抛错时自动回到上一版继续跑，
/// 回执说清「这一版没起来、上一版还在」，inspect 记着最近的错误；修好再 reload
/// 换上新版、错误清掉。
#[tokio::test]
async fn a_failed_reload_keeps_the_previous_version_running() {
    let root = boot().await;
    let good = r#"#{ inject: ["tools"], apply: |host| {
        host.register_tool(#{ name: "boom_ping", parameters: #{ type: "object", properties: #{} }, execute: |a| { "v1" } });
    } }"#;
    load_plugin(&root, "boom", good).await;
    assert_eq!(exec(&root, "boom_ping", "{}").await, "v1");

    write_plugin(
        &root,
        "boom",
        &[(
            "source.rhai",
            r#"#{ inject: ["tools"], apply: |host| { throw "nope" } }"#,
        )],
    );
    let result = tools_of(&root)
        .execute(ToolCall {
            id: "t".into(),
            name: "cordis_plugin".into(),
            arguments: json!({"action": "reload", "id": "boom"}).to_string(),
        })
        .await;
    assert!(
        result.is_error,
        "失败的 reload 要标成错误：{}",
        result.content
    );
    let failed = result.content;
    assert!(failed.contains("这一版没跑起来"), "{failed}");
    assert!(failed.contains("nope"), "{failed}");
    assert!(failed.contains("仍在运行（已自动回滚）"), "{failed}");
    assert!(failed.contains("登记：工具 boom_ping"), "{failed}");
    assert!(failed.contains("cordis_plugin reload boom"), "{failed}");
    assert_eq!(exec(&root, "boom_ping", "{}").await, "v1", "上一版照跑");
    let detail = exec(&root, "cordis_inspect", r#"{"plugin":"boom"}"#).await;
    assert!(detail.contains("running (last reload failed)"), "{detail}");
    assert!(
        detail.contains("last error:") && detail.contains("nope"),
        "{detail}"
    );

    write_plugin(&root, "boom", &[("source.rhai", &good.replace("v1", "v2"))]);
    let fixed = reload(&root, "boom").await;
    assert!(fixed.contains("运行中"), "{fixed}");
    assert_eq!(exec(&root, "boom_ping", "{}").await, "v2");
    let detail = exec(&root, "cordis_inspect", r#"{"plugin":"boom"}"#).await;
    assert!(!detail.contains("last error"), "成功一次就清掉：{detail}");
}

/// reload 把文件读成快照：之后改文件不影响正在跑的版本，再 reload 才换上——
/// 「不可变的包」名副其实，回滚才真的回到上一版。inspect 报路径，不回源码。
#[tokio::test]
async fn reload_snapshots_the_source_until_the_next_reload() {
    let root = boot().await;
    let src = |answer: &str| {
        format!(
            r#"#{{ inject: ["tools"], apply: |host| {{
                host.register_tool(#{{ name: "snap_ping", description: "pong from file",
                    parameters: #{{ type: "object", properties: #{{}} }}, execute: |a| {{ "{answer}" }} }});
            }} }}"#
        )
    };
    load_plugin(&root, "snap", &src("one")).await;
    assert_eq!(exec(&root, "snap_ping", "{}").await, "one");
    write_plugin(&root, "snap", &[("source.rhai", &src("two"))]);
    assert_eq!(
        exec(&root, "snap_ping", "{}").await,
        "one",
        "没 reload 不换"
    );
    reload(&root, "snap").await;
    assert_eq!(exec(&root, "snap_ping", "{}").await, "two");

    let detail = exec(&root, "cordis_inspect", r#"{"plugin":"snap"}"#).await;
    assert!(detail.contains("source: "), "{detail}");
    assert!(detail.contains("snap/source.rhai"), "{detail}");
    assert!(!detail.contains("register_tool"), "不回源码：{detail}");
}

/// `source.rhai` 换成指向插件目录外的软链：reload 拒掉，外面的内容不进回执，
/// 工具也不登记。
#[tokio::test]
async fn reload_rejects_a_source_symlinked_out_of_the_plugin_directory() {
    let root = boot().await;
    let dir = write_plugin(
        &root,
        "swapprobe",
        &[("source.rhai", "#{ inject: [], apply: |host| {} }")],
    );
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        outside.path(),
        r#"#{ inject: ["tools"], apply: |host| { host.register_tool(#{ name: "swapprobe_ping", parameters: #{ type: "object", properties: #{} }, execute: |a| { "TOP SECRET" } }); } }"#,
    )
    .unwrap();
    std::fs::remove_file(dir.join("source.rhai")).unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.join("source.rhai")).unwrap();

    let ran = reload(&root, "swapprobe").await;
    assert!(ran.contains("插件目录"), "{ran}");
    assert!(!ran.contains("TOP SECRET"), "{ran}");
    let names: Vec<_> = tools_of(&root)
        .specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        !names.iter().any(|n| n == "swapprobe_ping"),
        "tool must not register after symlink escape: {names:?}"
    );
}

#[tokio::test]
async fn rhai_run_registers_tool_provide_and_slot_then_stop_unregisters() {
    let root = boot().await;
    assert!(root.get::<RhaiBag>("dynMemo").is_none());
    let ran = load_plugin(&root, "memo", RHAI_MEMO).await;
    // 回执把这一版登记的东西列全：模型下一步该试调哪颗，不用再去猜。
    assert!(
        ran.contains("登记：工具 memo_get；面板 memo、later；服务 dynMemo、dynMemoLimit；设置卡"),
        "{ran}"
    );
    let bag = root.get::<RhaiBag>("dynMemo").expect("provide dynMemo");
    assert_eq!(bag.get()["text"], "hi");
    assert_eq!(exec(&root, "memo_get", "{}").await, "memo-ok");

    let inspect = exec(&root, "cordis_inspect", r#"{"plugin":"memo"}"#).await;
    assert!(inspect.contains("registered: 工具 memo_get"), "{inspect}");

    let builtins = exec(&root, "cordis_inspect", r#"{"what":"builtins"}"#).await;
    assert!(builtins.contains("host.register_tool"), "{builtins}");
    assert!(builtins.contains("host.on"), "{builtins}");
    assert!(
        builtins.contains("JSON-schema map") || builtins.contains("parameters: #{"),
        "{builtins}"
    );
    assert!(builtins.contains("/agents"), "{builtins}");
    let slots = exec(&root, "cordis_inspect", r#"{"what":"slots"}"#).await;
    assert!(slots.contains("memo"), "{slots}");
    assert!(slots.contains("便签"), "{slots}");

    let tui = root.require::<cordis_spine::TuiSlots>(TUI_SLOTS).unwrap();
    assert_eq!(tui.take_open_request().as_deref(), Some("memo"));
    assert!(tui.render("memo").unwrap().contains("hello slot"));
    // 声明的动作原样交给 GUI；没写 label 的用 id。
    let actions: Vec<(String, String)> = tui
        .actions("memo")
        .into_iter()
        .map(|a| (a.id, a.label))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("refresh".to_string(), "刷新".to_string()),
            ("clear".to_string(), "clear".to_string())
        ]
    );
    // 视图树：脚本返回的 map 解析成节点；视图里的按钮也算能点的动作。
    let view = tui.view("memo").expect("脚本声明了 view");
    assert!(view.to_plain().contains("条数：3"), "{view:?}");
    assert_eq!(tui.action_ids("memo"), vec!["refresh", "clear", "archive"]);
    // `view` 返回 `()`：这次不给视图，回退到 `render()`。
    assert!(tui.view("later").is_none());
    assert_eq!(tui.render("later").as_deref(), Some("还没准备好"));

    // 工具卡视图：写在工具上的 `view` 和 register_tool_view 都登记进 `"tool.views"`；
    // 回 `()` 的是「这次不给视图」。
    let views = root
        .require::<cordis_spine::ToolViews>(cordis_spine::TOOL_VIEWS)
        .unwrap();
    let call = |name| cordis_spine::ToolViewInput {
        name,
        arguments: "{}",
        output: "memo-ok",
        failed: false,
    };
    let drawn = views.render(&call("memo_get")).expect("memo_get 有视图");
    assert!(drawn.to_plain().contains("输出：memo-ok"), "{drawn:?}");
    assert!(drawn.to_plain().contains("失败：false"), "{drawn:?}");
    assert!(views.has("bash"));
    assert!(views.render(&call("bash")).is_none());

    // 设置卡：按插件 id 登记；没写过的字段读到 default，没有的字段读到 ()。
    let settings = root
        .require::<cordis_spine::PluginSettings>(cordis_spine::PLUGIN_SETTINGS)
        .unwrap();
    assert_eq!(
        settings.list(),
        vec![("memo".to_string(), "便签".to_string())]
    );
    let bag = root.require::<RhaiBag>("dynMemoLimit").unwrap().get();
    assert_eq!(bag["limit"], serde_json::json!(10), "{bag}");
    assert!(bag.get("none").is_none_or(|v| v.is_null()), "{bag}");

    // 状态项：第一次登记、第二次改内容；包停了跟着消失。
    let status = root
        .require::<cordis_spine::StatusItems>(cordis_spine::STATUS_ITEMS)
        .unwrap();
    let item = status
        .list()
        .into_iter()
        .find(|i| i.id == "memo-count")
        .expect("set_status 登记了状态项");
    assert_eq!(item.text, "3 条");
    assert_eq!(item.tone, cordis_base::view::Tone::Success);
    assert_eq!(item.surface.as_deref(), Some("memo"));

    exec_json(
        &root,
        "cordis_plugin",
        json!({"action": "disable", "id": "memo"}),
    )
    .await;
    assert!(
        status.list().iter().all(|i| i.id != "memo-count"),
        "包停了状态项要跟着走"
    );
    assert!(views.names().is_empty(), "包停了卡片视图要跟着走");
    assert!(settings.list().is_empty(), "包停了设置卡要跟着走");
    assert!(root.get::<RhaiBag>("dynMemo").is_none());
    let names: Vec<_> = tools_of(&root)
        .specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(!names.iter().any(|n| n == "memo_get"), "{names:?}");
    assert!(tui.list().is_empty());
}

#[tokio::test]
async fn dynamic_tools_bypass_agent_preset_allowlist() {
    let home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path());
    let root = boot().await;
    root.plugin(cordis_spine::mcp_client(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(agent_presets(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.require::<AgentPresets>(AGENT_PRESETS)
        .unwrap()
        .apply(CORDIS_PRESET_ID)
        .unwrap();
    assert!(!root
        .require::<AgentPresets>(AGENT_PRESETS)
        .unwrap()
        .allows(DYN_ECHO_TOOL));

    write_plugin(&root, "echo", &[("plugin.toml", "factory = \"echo\"\n")]);
    reload(&root, "echo").await;

    // 动态工具按需加载：对采样器隐藏，不再占用模型上下文
    let model: Vec<_> = tools_of(&root)
        .specs_for_model()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        !model.iter().any(|n| n == DYN_ECHO_TOOL),
        "dynamic tool must be hidden from sampler: {model:?}"
    );

    // 但在全量 specs 中已注册为 hidden 工具
    let all: Vec<_> = tools_of(&root)
        .specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(all.iter().any(|n| n == DYN_ECHO_TOOL));
    assert!(tools_of(&root).is_hidden(DYN_ECHO_TOOL));

    // 可以通过 search_tool 搜索到
    let search_res = exec(&root, "search_tool", r#"{"query":"dynEcho"}"#).await;
    assert!(search_res.contains(DYN_ECHO_TOOL), "{search_res}");
    assert!(
        search_res.contains("\"server\":\"dynamic\""),
        "动态包工具在目录里归 dynamic 组：{search_res}"
    );

    // 可以通过 use_tool 调度执行（即使当前 AgentPreset 允许名单里没有该工具，Bypass 也依然生效）
    let use_res = exec(
        &root,
        "use_tool",
        &json!({
            "tool_name": DYN_ECHO_TOOL,
            "tool_input": {}
        })
        .to_string(),
    )
    .await;
    assert_eq!(use_res, "pong");

    assert_eq!(exec(&root, DYN_ECHO_TOOL, "{}").await, "pong");
}

/// 开一页自己的权限队列（询问档），根页是自动批准——和 GUI 里「每个会话一页、
/// 第 1 页没人看」一样。
async fn ask_page(root: &Context) -> (Context, std::sync::Arc<Permissions>) {
    let page = root.isolate(SETTINGS).isolate(PERMISSIONS);
    page.plugin(settings(), ()).unwrap().wait().await.unwrap();
    page.plugin(permissions(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    page.require::<cordis_spine::AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Ask);
    let perms = page.require::<Permissions>(PERMISSIONS).unwrap();
    (page, perms)
}

async fn wait_for_prompt(perms: &Permissions) -> cordis_spine::PermissionPrompt {
    for _ in 0..300 {
        if let Some(p) = perms.front() {
            return p;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("确认框没有出现在发起调用的那一页");
}

/// 回归：插件脚本里的 `host.call_tool` 要在**发起调用的那一页**上执行。以前它拿
/// 插件 fiber 所在的根算，确认框弹到第 1 页——GUI 里那一页没人看，调用一直挂着
/// （实测会话里 `bash git status` 挂了 307 秒）。这里根页是自动批准：修之前 bash
/// 直接在根上跑掉，发起调用的那一页根本不会出现确认框。
#[tokio::test(flavor = "multi_thread")]
async fn host_call_tool_asks_on_the_calling_page() {
    let root = boot_gated().await;
    load_plugin(
        &root,
        "relay",
        r#"#{ inject: ["tools"], apply: |host| {
            host.register_tool(#{ name: "relay_bash", parameters: #{ type: "object", properties: #{} },
                execute: |a| { host.call_tool("bash", #{ command: "echo relayed" }) } });
        } }"#,
    )
    .await;
    let (page, perms) = ask_page(&root).await;
    let tools = tools_of(&root);
    let caller = page.clone();
    let call = tokio::spawn(async move {
        tools
            .execute_on(
                &caller,
                ToolCall {
                    id: "t".into(),
                    name: "relay_bash".into(),
                    arguments: "{}".into(),
                },
            )
            .await
            .content
    });
    let prompt = wait_for_prompt(&perms).await;
    assert_eq!(prompt.tool, "bash");
    assert!(
        prompt.summary.contains("echo relayed"),
        "{}",
        prompt.summary
    );
    assert!(
        root.require::<Permissions>(PERMISSIONS)
            .unwrap()
            .front()
            .is_none(),
        "根页不该收到这个请求"
    );
    perms.resolve(PermissionOptionKind::RejectOnce);
    let out = call.await.unwrap();
    assert!(!out.contains("relayed"), "拒了就不该跑：{out}");
}

/// 同一个缺陷的 HTTP 那一半：没声明的 host 要在发起调用的那一页确认；
/// `plugin.toml` 里 `[permissions] http` 声明过的直接放行（批准加载时已经列给用户看过）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_http_asks_on_the_calling_page_unless_the_host_is_declared() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, seen) = spawn_api_server();
    let source = format!(
        r#"#{{ inject: ["tools"], apply: |host| {{
            host.register_tool(#{{ name: "net_issues", parameters: #{{ type: "object", properties: #{{}} }},
                execute: |a| {{ let r = http_request(#{{ url: "{base}/issues" }}); "HTTP " + r.status }} }});
        }} }}"#
    );
    load_plugin(&root, "net", &source).await;
    let (page, perms) = ask_page(&root).await;
    let run = |page: Context| {
        let tools = tools_of(&root);
        tokio::spawn(async move {
            tools
                .execute_on(
                    &page,
                    ToolCall {
                        id: "t".into(),
                        name: "net_issues".into(),
                        arguments: "{}".into(),
                    },
                )
                .await
                .content
        })
    };
    let call = run(page.clone());
    let prompt = wait_for_prompt(&perms).await;
    assert_eq!(prompt.tool, "http_request 127.0.0.1");
    assert!(prompt.summary.contains("插件 net"), "{}", prompt.summary);
    perms.resolve(PermissionOptionKind::AllowOnce);
    assert_eq!(call.await.unwrap(), "HTTP 200");

    write_plugin(
        &root,
        "net",
        &[("plugin.toml", "[permissions]\nhttp = [\"127.0.0.1\"]\n")],
    );
    let ran = reload(&root, "net").await;
    assert!(ran.contains("HTTP 免逐次确认：127.0.0.1"), "{ran}");
    let out = run(page.clone()).await.unwrap();
    assert_eq!(out, "HTTP 200", "声明过的 host 不再问");
    assert!(perms.front().is_none());
    assert_eq!(seen.lock().unwrap().len(), 2);
}

/// `cordis_plugin` 的确认按插件记：「始终允许」一个插件，迭代它不用每次点；换一个
/// 插件还会再问。确认框里列着它挂的钩子和声明的 HTTP host。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_approval_is_per_plugin_and_lists_hooks_and_hosts() {
    let root = boot_gated().await;
    root.require::<cordis_spine::AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Ask);
    let perms = root.require::<Permissions>(PERMISSIONS).unwrap();
    let hooks = r#"#{ inject: [], apply: |host| {
        host.on("tools/pre-execute", |c| { () });
        host.on("agent/turn-end", |e| { () });
    } }"#;
    write_plugin(
        &root,
        "guard",
        &[
            ("source.rhai", hooks),
            (
                "plugin.toml",
                "[permissions]\nhttp = [\"api.example.com\"]\n",
            ),
        ],
    );
    write_plugin(&root, "other", &[("source.rhai", hooks)]);
    let spawn_reload = |id: &'static str| {
        let root = root.clone();
        tokio::spawn(async move { reload(&root, id).await })
    };

    let first = spawn_reload("guard");
    let prompt = wait_for_prompt(&perms).await;
    assert_eq!(prompt.tool, "cordis_plugin guard");
    assert!(
        prompt.summary.contains("加载插件 guard"),
        "{}",
        prompt.summary
    );
    assert!(
        prompt
            .summary
            .contains("钩子：tools/pre-execute、agent/turn-end"),
        "{}",
        prompt.summary
    );
    assert!(
        prompt.summary.contains("HTTP 免逐次确认：api.example.com"),
        "{}",
        prompt.summary
    );
    perms.resolve(PermissionOptionKind::AllowAlways);
    assert!(first.await.unwrap().contains("运行中"));

    // 同一个插件再 reload：不问。
    let again = reload(&root, "guard").await;
    assert!(again.contains("运行中"), "{again}");
    assert!(perms.front().is_none());

    // 换一个插件：还问。
    let other = spawn_reload("other");
    let prompt = wait_for_prompt(&perms).await;
    assert_eq!(prompt.tool, "cordis_plugin other");
    perms.resolve(PermissionOptionKind::RejectOnce);
    let denied = other.await.unwrap();
    assert!(!denied.contains("运行中"), "{denied}");
    assert!(runner(&root).inspect_plugin(MAIN, "other").is_err());
}

#[tokio::test]
async fn at_plugin_id_pre_step_injects_identity_reminder() {
    let root = boot().await;
    write_plugin(
        &root,
        "echo",
        &[(
            "plugin.toml",
            "name = \"Echo\"\npurpose = \"ping\"\nfactory = \"echo\"\n",
        )],
    );
    let user = "@echo please inspect";
    root.waterfall(PRE_STEP, PreStep::new(user, true, MAIN), move || {
        PreStep::new(user, true, MAIN)
    });
    let events = root.require::<Sessions>(SESSIONS).unwrap().events();
    let reminder = events.iter().find_map(|e| match e {
        LogEvent::SystemReminder(text) => Some(text.as_str()),
        _ => None,
    });
    let text = reminder.expect("system reminder");
    assert!(text.contains("# @echo"), "{text}");
    assert!(text.contains(".dock/plugins/echo"), "{text}");
    assert!(text.contains("cordis_plugin reload echo"), "{text}");
    assert!(text.contains("不要另起一个插件目录"), "{text}");
    assert!(!text.to_lowercase().contains("source:"), "{text}");
}

#[tokio::test]
async fn other_session_cannot_see_owned_plugin() {
    let root = boot().await;
    let (id, _) = define(&root, new_plugin("echo"), "echo", None).unwrap();
    let runner = runner(&root);
    assert!(runner.inspect_plugin(MAIN, &id).is_ok());
    assert!(runner.inspect_plugin("child-other", &id).is_err());
    assert!(runner.reference("child-other", &id).is_err());
}

/// 先登记两颗工具和一个服务，再去盖命令表里的 `/help`（`slash` 插件自己登记的，
/// 不依赖终端是否挂着）——这一步失败，前面登记的都要回滚。
const RHAI_APPLY_THEN_RESERVED_SLASH: &str = r#"#{
    inject: ["tools", "slash"],
    apply: |host| {
        host.provide("dynOrphan", #{ text: "leak" });
        host.register_tool(#{
            name: "orb_list",
            description: "should roll back",
            parameters: #{ type: "object", properties: #{} },
            execute: |args| { "x" }
        });
        host.register_tool(#{
            name: "orb_list_get",
            description: "should roll back",
            parameters: #{ type: "object", properties: #{} },
            execute: |args| { "y" }
        });
        host.register_slash(#{
            command: "help",
            kind: "overlay",
            text: "nope"
        });
    }
}"#;

const RHAI_TWO_TOOLS: &str = r#"#{
    inject: ["tools", "slash"],
    apply: |host| {
        host.provide("dynAgentList", #{ n: 1 });
        host.register_tool(#{
            name: "aagt_list",
            description: "list",
            parameters: #{ type: "object", properties: #{} },
            execute: |args| { "ok" }
        });
        host.register_tool(#{
            name: "aagt_list_get",
            description: "get",
            parameters: #{ type: "object", properties: #{} },
            execute: |args| { "ok" }
        });
    }
}"#;

const RHAI_SLASH_ONLY: &str = r#"#{
    inject: ["slash"],
    apply: |host| {
        host.register_slash(#{
            command: "agls",
            kind: "overlay",
            text: "agents"
        });
    }
}"#;

fn extra_names(root: &Context) -> Vec<String> {
    tools_of(root).specs().into_iter().map(|s| s.name).collect()
}

#[tokio::test]
async fn rhai_apply_throw_rolls_back_register_tool_and_provide() {
    let root = boot().await;
    write_plugin(
        &root,
        "orb",
        &[("source.rhai", RHAI_APPLY_THEN_RESERVED_SLASH)],
    );
    let ran = reload(&root, "orb").await;
    assert!(ran.contains("cannot shadow"), "{ran}");
    assert!(ran.contains("pick a different command"), "{ran}");

    let names = extra_names(&root);
    assert!(!names.iter().any(|n| n == "orb_list"), "{names:?}");
    assert!(!names.iter().any(|n| n == "orb_list_get"), "{names:?}");
    assert!(root.get::<RhaiBag>("dynOrphan").is_none());
    let slash = root.require::<Slash>(SLASH).unwrap();
    assert!(
        !slash.list().iter().any(|e| e.command == "agents"),
        "{:?}",
        slash
            .list()
            .iter()
            .map(|e| e.command.clone())
            .collect::<Vec<_>>()
    );
    // 修好同一个目录再 reload：还是 orb 这个 id，不会多出第二个插件。
    write_plugin(&root, "orb", &[("source.rhai", RHAI_TWO_TOOLS)]);
    let fixed = reload(&root, "orb").await;
    assert!(fixed.contains("运行中"), "{fixed}");
    assert!(extra_names(&root).iter().any(|n| n == "aagt_list"));
    assert_eq!(
        runner(&root)
            .snapshot(MAIN)
            .iter()
            .filter(|r| r.plugin_id.starts_with("orb"))
            .count(),
        1
    );
}

#[tokio::test]
async fn update_replaces_entire_fiber_not_overlay() {
    let root = boot().await;
    load_plugin(&root, "aagt", RHAI_TWO_TOOLS).await;
    assert!(extra_names(&root).iter().any(|n| n == "aagt_list"));
    assert!(root.get::<RhaiBag>("dynAgentList").is_some());

    write_plugin(&root, "aagt", &[("source.rhai", RHAI_SLASH_ONLY)]);
    let updated = reload(&root, "aagt").await;
    assert!(updated.contains("登记：斜杠 /agls"), "{updated}");
    let names = extra_names(&root);
    assert!(!names.iter().any(|n| n == "aagt_list"), "{names:?}");
    assert!(!names.iter().any(|n| n == "aagt_list_get"), "{names:?}");
    assert!(root.get::<RhaiBag>("dynAgentList").is_none());
    let slash = root.require::<Slash>(SLASH).unwrap();
    assert!(
        slash.list().iter().any(|e| e.command == "agls"),
        "{:?}",
        slash
            .list()
            .iter()
            .map(|e| e.command.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn rhai_define_without_apply_teaches_and_does_not_mint() {
    let root = boot().await;
    write_plugin(
        &root,
        "noap",
        &[("source.rhai", "#{ inject: [\"tools\"] }")],
    );
    let out = reload(&root, "noap").await;
    assert!(out.contains("apply"), "{out}");
    assert!(out.contains("这一版没跑起来"), "{out}");
    assert!(runner(&root).inspect_plugin(MAIN, "noap").is_err());
}

const RHAI_STRING_PARAMS: &str = r#"#{
    inject: ["tools"],
    apply: |host| {
        host.register_tool(#{
            name: "str_params",
            description: "must fail",
            parameters: "{\"type\":\"object\",\"properties\":{}}",
            execute: |args| { "x" }
        });
    }
}"#;

const RHAI_BAD_REQUIRED: &str = r#"#{
    inject: ["tools"],
    apply: |host| {
        host.register_tool(#{
            name: "bad_req",
            description: "illegal required",
            parameters: #{ type: "object", properties: #{}, required: ["nope"] },
            execute: |args| { "x" }
        });
    }
}"#;

#[tokio::test]
async fn rhai_string_parameters_fail_loud_and_roll_back() {
    let root = boot().await;
    write_plugin(&root, "strp", &[("source.rhai", RHAI_STRING_PARAMS)]);
    let ran = reload(&root, "strp").await;
    assert!(ran.contains("must be a map"), "{ran}");
    assert!(ran.contains("not a JSON string"), "{ran}");
    let names = extra_names(&root);
    assert!(!names.iter().any(|n| n == "str_params"), "{names:?}");
}

#[tokio::test]
async fn rhai_required_unknown_property_fail_loud() {
    let root = boot().await;
    write_plugin(&root, "schm", &[("source.rhai", RHAI_BAD_REQUIRED)]);
    let ran = reload(&root, "schm").await;
    assert!(ran.contains("undeclared property"), "{ran}");
    assert!(ran.contains("nope"), "{ran}");
    let names = extra_names(&root);
    assert!(!names.iter().any(|n| n == "bad_req"), "{names:?}");
}

#[tokio::test]
async fn concurrent_identical_cordis_run_shares_in_flight() {
    let root = boot().await;
    define(&root, new_plugin("echo"), "echo", None).unwrap();
    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    let (hold_tx, _hold_rx) = tokio::sync::watch::channel(false);
    runner.test_set_start_hold(Some(hold_tx.clone()));

    let a_runner = runner.clone();
    let a = tokio::spawn(async move { a_runner.run(MAIN, "echo-1", "pkg-1", RunMode::Run).await });

    let mut starting = false;
    for _ in 0..200 {
        if runner.test_is_starting("echo-1") {
            starting = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(starting, "lead never recorded in-flight");

    let b_runner = runner.clone();
    let b = tokio::spawn(async move { b_runner.run(MAIN, "echo-1", "pkg-1", RunMode::Run).await });

    let mut waited = false;
    for _ in 0..200 {
        if runner.test_in_flight_waiters("echo-1") >= 1 {
            waited = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(waited, "second run did not join in-flight");

    hold_tx.send(true).unwrap();
    runner.test_set_start_hold(None);

    let (ra, rb) = tokio::join!(a, b);
    let ra = ra.expect("join a").expect("run a");
    let rb = rb.expect("join b").expect("run b");
    assert_eq!(ra.plugin_run_id, rb.plugin_run_id);
    let fibers: Vec<_> = runner
        .live_fibers(MAIN)
        .into_iter()
        .filter(|f| f.plugin_id.as_deref() == Some("echo-1"))
        .collect();
    assert_eq!(fibers.len(), 1, "{fibers:?}");
}

#[tokio::test]
async fn concurrent_different_package_is_already_starting() {
    let root = boot().await;
    define(&root, new_plugin("echo"), "echo", None).unwrap();
    define(
        &root,
        PluginSel::Existing {
            plugin_id: "echo-1".into(),
        },
        "note",
        None,
    )
    .unwrap();
    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    let (hold_tx, _hold_rx) = tokio::sync::watch::channel(false);
    runner.test_set_start_hold(Some(hold_tx.clone()));

    let a_runner = runner.clone();
    let a = tokio::spawn(async move { a_runner.run(MAIN, "echo-1", "pkg-1", RunMode::Run).await });

    for _ in 0..200 {
        if runner.test_is_starting("echo-1") {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(runner.test_is_starting("echo-1"));

    let other = runner.run(MAIN, "echo-1", "pkg-2", RunMode::Run).await;
    assert!(
        other
            .as_ref()
            .err()
            .is_some_and(|e| e.contains("already starting")),
        "{other:?}"
    );

    hold_tx.send(true).unwrap();
    runner.test_set_start_hold(None);
    a.await.expect("join a").expect("run a");
}

const RHAI_ON_EVENT: &str = r#"#{
    inject: ["tools"],
    apply: |host| {
        let store = #{ last: "" };
        host.on("session/event", |line| {
            store.last = line;
        });
        host.register_tool(#{
            name: "last_evt",
            description: "last session event line",
            parameters: #{ type: "object", properties: #{} },
            execute: |args| { store.last }
        });
    }
}"#;

#[tokio::test]
async fn host_on_session_event_sees_user_line() {
    let root = boot().await;
    let ran = load_plugin(&root, "obsv", RHAI_ON_EVENT).await;
    assert!(ran.contains("钩子 session/event"), "{ran}");

    root.require::<Sessions>(SESSIONS)
        .unwrap()
        .append(LogEvent::User("hello cordis".into()));

    let mut got = String::new();
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        got = exec(&root, "last_evt", "{}").await;
        if got.contains("user") && got.contains("hello cordis") {
            break;
        }
    }
    assert!(got.contains("user\thello cordis"), "{got}");

    let events = exec(&root, "cordis_inspect", r#"{"what":"events"}"#).await;
    assert!(events.contains("session/event"), "{events}");
}

#[tokio::test]
async fn disk_plugin_autoloads_and_is_visible_to_other_sessions() {
    let root = boot().await;
    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join("echo");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        "name = \"Echo\"\npurpose = \"disk echo\"\nfactory = \"echo\"\nenabled = true\n",
    )
    .unwrap();

    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    runner
        .boot_disk_from(&[(PersistScope::Project, dir.path().to_path_buf())])
        .await;

    assert!(root.get::<DynEcho>(DYN_ECHO).is_some());
    let row = runner
        .inspect_plugin(MAIN, "echo")
        .expect("visible to main");
    assert!(matches!(row.origin, PluginOrigin::Disk { .. }));
    assert!(runner.inspect_plugin("child-other", "echo").is_ok());

    let listing =
        runner.overlay_listing_from(MAIN, &[(PersistScope::Project, dir.path().to_path_buf())]);
    assert!(
        listing.contains("echo [project] Echo — disk echo (运行中)"),
        "{listing}"
    );
    assert!(listing.contains("cordis_plugin reload"), "{listing}");

    let gone = runner.undefine(MAIN, "echo").await.unwrap();
    assert!(gone.was_running);
    assert!(plugin_dir.join("plugin.toml").exists());
}

// —— scriptable waterfalls ————————————————————————————————————————————————

/// Registers on both scriptable waterfalls. `()` = no opinion.
const RHAI_HOOKS: &str = r#"#{
    inject: [],
    apply: |host| {
        host.on("agent/step-start", |s| {
            if s.step == 1 { "<system-reminder>脚本盯梢</system-reminder>" } else { () }
        });
        host.on("agent/turn-end", |e| {
            if e.rounds < 1 { "<system-reminder>脚本续跑</system-reminder>" } else { () }
        });
    }
}"#;

/// A handler that always throws. Fail-open: the turn must not notice.
const RHAI_THROWS: &str = r#"#{
    inject: [],
    apply: |host| {
        host.on("agent/step-start", |s| { throw "boom" });
        host.on("agent/turn-end", |e| { throw "boom" });
    }
}"#;

/// `boot()` plus a text-only sampler and the loop, so a turn can actually run.
async fn boot_with_loop() -> Context {
    let root = boot().await;
    root.plugin(
        llm(),
        LlmConfig {
            mode: LlmMode::Text,
            ..LlmConfig::default()
        },
    )
    .unwrap()
    .wait()
    .await
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root
}

async fn run_rhai(root: &Context, id: &str, source: &str) {
    load_plugin(root, id, source).await;
}

fn reminders(root: &Context) -> Vec<String> {
    root.require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .filter_map(|e| match e {
            LogEvent::SystemReminder(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn samples(root: &Context) -> usize {
    root.require::<Sessions>(SESSIONS)
        .unwrap()
        .kinds()
        .iter()
        .filter(|k| **k == "llm/stream")
        .count()
}

/// A disk-shaped Rhai package can now do what only compiled-in code could:
/// keep a turn going, and drop a reminder in before a sample.
#[tokio::test]
async fn rhai_can_intercept_step_start_and_turn_end() {
    let root = boot_with_loop().await;
    run_rhai(&root, "hook", RHAI_HOOKS).await;

    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert!(matches!(out, TurnOutcome::Text(ref t) if !t.is_empty()));
    assert_eq!(
        samples(&root),
        2,
        "the script's vote must buy a second sample"
    );
    let seen = reminders(&root);
    assert_eq!(
        seen,
        [
            "<system-reminder>脚本续跑</system-reminder>",
            "<system-reminder>脚本盯梢</system-reminder>"
        ],
        "turn-end reminder lands first, then step 1's"
    );
}

/// A throwing script is "no opinion", not a broken turn.
#[tokio::test]
async fn a_throwing_rhai_hook_does_not_break_the_turn() {
    let root = boot_with_loop().await;
    run_rhai(&root, "bang", RHAI_THROWS).await;

    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert!(matches!(out, TurnOutcome::Text(ref t) if !t.is_empty()));
    assert_eq!(samples(&root), 1, "a throw must not add a round");
    assert!(reminders(&root).is_empty(), "{:?}", reminders(&root));
}

/// The host always calls `args.next()` for the script, so a script cannot drop
/// the chain: the built-in todo bookkeeping still gets its vote. A script's
/// keep-working vote outranks bookkeeping (bookkeeping ends the turn and must
/// not swallow someone else's "not done yet"), so bookkeeping comes after it.
#[tokio::test]
async fn a_rhai_hook_cannot_swallow_the_todo_bookkeeping() {
    let root = boot_with_loop().await;
    // Script first, so the built-in handler is *inside* it on the chain: if the
    // host ever stopped calling `args.next()`, the bookkeeping vote would vanish.
    run_rhai(&root, "hook", RHAI_HOOKS).await;
    root.plugin(tool_todo(), ()).unwrap().wait().await.unwrap();
    exec(
        &root,
        "todo_write",
        r#"{"merge":false,"todos":[{"id":"a","content":"读代码","status":"in_progress"}]}"#,
    )
    .await;

    let _ = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    let seen = reminders(&root);
    let script = seen
        .iter()
        .position(|r| r.contains("脚本续跑"))
        .expect("the script's continuation ran");
    let bookkeeping: Vec<usize> = seen
        .iter()
        .enumerate()
        .filter(|(_, r)| r.contains(cordis_spine::TODO_BOOKKEEPING_SENTINEL))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(bookkeeping.len(), 1, "{seen:?}");
    assert!(script < bookkeeping[0], "{seen:?}");
}

/// Returns a non-string, which must not be stringified into the transcript.
const RHAI_NON_STRING: &str = r#"#{
    inject: [],
    apply: |host| {
        host.on("agent/step-start", |s| { 42 });
        host.on("agent/turn-end", |e| { #{ oops: true } });
    }
}"#;

/// `TurnEnd` says handlers must respect `queued_followups`, and here the host
/// *is* the handler — the rule cannot live in the script's documentation.
#[tokio::test]
async fn a_rhai_turn_end_hook_never_outvotes_a_queued_followup() {
    let root = boot_with_loop().await;
    run_rhai(&root, "hook", RHAI_HOOKS).await;
    root.require::<Sessions>(SESSIONS)
        .unwrap()
        .set_queued_followups(1);

    let _ = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert_eq!(samples(&root), 1, "the user is steering, not the script");
    assert!(
        !reminders(&root).iter().any(|t| t.contains("脚本续跑")),
        "{:?}",
        reminders(&root)
    );
}

/// A script hook returns a reminder or nothing. Anything else is a mistake, not
/// a reminder to be coerced into the model's history.
#[tokio::test]
async fn a_rhai_hook_returning_a_non_string_injects_nothing() {
    let root = boot_with_loop().await;
    run_rhai(&root, "junk", RHAI_NON_STRING).await;

    let _ = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert_eq!(samples(&root), 1);
    assert!(reminders(&root).is_empty(), "{:?}", reminders(&root));
}

/// （`install_disk` 的 Err 分支不打印）。
#[tokio::test]
async fn rhai_disk_plugin_autoloads_from_the_given_roots() {
    let root = boot().await;
    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join("probeplug");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        "name = \"Probe\"\npurpose = \"rhai from disk\"\nfactory = \"rhai\"\nenabled = true\n",
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("source.rhai"),
        r#"#{
            inject: ["tools"],
            apply: |host| {
                host.register_tool(#{
                    name: "probeplug_ping",
                    description: "pong",
                    parameters: #{ type: "object", properties: #{} },
                    execute: |args| { "probe-pong" }
                });
            }
        }"#,
    )
    .unwrap();

    root.plugin(cordis_spine::mcp_client(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();

    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    runner
        .boot_disk_from(&[(PersistScope::Project, dir.path().to_path_buf())])
        .await;

    let row = runner.inspect_plugin(MAIN, "probeplug");
    assert!(
        row.is_ok(),
        "rhai 磁盘插件应能从传入的 root 自动加载，实际：{:?}",
        row.err()
    );

    let tools = tools_of(&root);
    assert!(tools.is_hidden("probeplug_ping"));
    let model_specs: Vec<_> = tools
        .specs_for_model()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(!model_specs.iter().any(|n| n == "probeplug_ping"));
    let all_specs: Vec<_> = tools.specs().into_iter().map(|s| s.name).collect();
    assert!(all_specs.iter().any(|n| n == "probeplug_ping"));

    let use_res = exec(
        &root,
        "use_tool",
        &json!({
            "tool_name": "probeplug_ping",
            "tool_input": {}
        })
        .to_string(),
    )
    .await;
    assert_eq!(use_res, "probe-pong");
}

/// 最小多路由服务端。用裸 TcpListener 手写，别为测试拖一个 mock 框架进来。
/// 返回 `(base_url, 收到的请求原文)`。线程随进程退出。
fn spawn_api_server() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = seen.clone();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            if head.is_empty() {
                continue;
            }
            let mut body = vec![0u8; content_length];
            if content_length > 0 {
                let _ = reader.read_exact(&mut body);
            }
            let body = String::from_utf8_lossy(&body).to_string();
            let request_line = head.lines().next().unwrap_or_default().to_string();
            log.lock().unwrap().push(format!("{head}{body}"));

            let path = request_line.split_whitespace().nth(1).unwrap_or("/");
            let (status, ctype, payload) = match path {
                p if p.starts_with("/issues?") || p == "/issues" => (
                    "200 OK",
                    "application/json",
                    r#"{"total":2,"page":{"next":null},"items":[{"id":11,"title":"first bug","labels":["p0","ui"],"open":true},{"id":12,"title":"second bug","labels":[],"open":false}]}"#
                        .to_string(),
                ),
                "/issues/11" => (
                    "200 OK",
                    "application/json",
                    r#"{"id":11,"title":"first bug","author":{"login":"polar"},"comments":3}"#
                        .to_string(),
                ),
                "/plain" => (
                    "200 OK",
                    "text/plain; charset=utf-8",
                    "alpha\nbeta\ngamma".to_string(),
                ),
                "/missing" => (
                    "404 Not Found",
                    "application/json",
                    r#"{"error":"no such issue"}"#.to_string(),
                ),
                "/echo" => (
                    "200 OK",
                    "application/json",
                    format!(r#"{{"echoed":{}}}"#, if body.is_empty() { "null" } else { &body }),
                ),
                _ => ("200 OK", "text/plain", "ok".to_string()),
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nX-Api-Version: 2\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(payload.as_bytes());
            let _ = stream.flush();
        }
    });

    (format!("http://{addr}"), seen)
}

/// 打开 `allow_local`，否则 SSRF 会把 127.0.0.1 挡掉。
fn allow_local_http(root: &Context) {
    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    runner.set_http_params(cordis_spine::WebFetchParams {
        allow_local: Some(true),
        ..Default::default()
    });
}

async fn define_and_run(root: &Context, id: &str, src: &str) {
    load_plugin(root, id, src).await;
}

/// 主场景：GET JSON → `parse_json` → 取嵌套字段、数组元素、数组里的数组、布尔与数字。
/// 这就是「封装一个 API」实际要做的事。
#[tokio::test]
async fn rhai_http_parses_json_and_extracts_nested_data() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, _seen) = spawn_api_server();

    let src = format!(
        r#"#{{
        inject: ["tools"],
        apply: |host| {{
            host.register_tool(#{{
                name: "issues_summary",
                description: "list issues",
                parameters: #{{ type: "object", properties: #{{}} }},
                execute: |args| {{
                    let resp = http_request(#{{ url: "{base}/issues?state=open" }});
                    if !resp.ok {{ return "HTTP " + resp.status; }}
                    let data = parse_json(resp.body);
                    let first = data.items[0];
                    "total=" + data.total
                      + " id=" + first.id
                      + " title=" + first.title
                      + " label0=" + first.labels[0]
                      + " open=" + first.open
                      + " second=" + data.items[1].title
                      + " count=" + data.items.len()
                      + " next=" + type_of(data.page.next)
                }}
            }});
        }}
    }}"#
    );
    define_and_run(&root, "jsn", &src).await;

    let out = exec(&root, "issues_summary", "{}").await;
    assert_eq!(
        out,
        "total=2 id=11 title=first bug label0=p0 open=true second=second bug count=2 next=()"
    );
}

/// 响应的其余部分也要拿得到：状态码、`ok`、响应头、最终 URL。
#[tokio::test]
async fn rhai_http_exposes_status_headers_and_url() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, _seen) = spawn_api_server();

    let src = format!(
        r#"#{{
        inject: ["tools"],
        apply: |host| {{
            host.register_tool(#{{
                name: "probe_meta",
                description: "response metadata",
                parameters: #{{ type: "object", properties: #{{}} }},
                execute: |args| {{
                    let resp = http_request(#{{ url: "{base}/issues/11" }});
                    "status=" + resp.status
                      + " ok=" + resp.ok
                      + " ctype=" + resp.headers["content-type"]
                      + " api=" + resp.headers["x-api-version"]
                      + " url_ok=" + resp.url.ends_with("/issues/11")
                }}
            }});
        }}
    }}"#
    );
    define_and_run(&root, "meta", &src).await;

    let out = exec(&root, "probe_meta", "{}").await;
    assert_eq!(
        out,
        "status=200 ok=true ctype=application/json api=2 url_ok=true"
    );
}

/// 非 JSON 响应：直接拿 `body` 当字符串切。不是所有 API 都返回 JSON。
#[tokio::test]
async fn rhai_http_handles_plain_text_bodies() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, _seen) = spawn_api_server();

    let src = format!(
        r#"#{{
        inject: ["tools"],
        apply: |host| {{
            host.register_tool(#{{
                name: "probe_text",
                description: "plain body",
                parameters: #{{ type: "object", properties: #{{}} }},
                execute: |args| {{
                    let resp = http_request(#{{ url: "{base}/plain" }});
                    let lines = resp.body.split("\n");
                    "lines=" + lines.len() + " second=" + lines[1] + " has_gamma=" + resp.body.contains("gamma")
                }}
            }});
        }}
    }}"#
    );
    define_and_run(&root, "txt", &src).await;

    assert_eq!(
        exec(&root, "probe_text", "{}").await,
        "lines=3 second=beta has_gamma=true"
    );
}

/// 4xx 不是错误：正常返回，`ok` 为假，正文照样读得到（错误体常常是 JSON）。
#[tokio::test]
async fn rhai_http_returns_4xx_as_data_not_as_a_throw() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, _seen) = spawn_api_server();

    let src = format!(
        r#"#{{
        inject: ["tools"],
        apply: |host| {{
            host.register_tool(#{{
                name: "probe_404",
                description: "not found",
                parameters: #{{ type: "object", properties: #{{}} }},
                execute: |args| {{
                    let resp = http_request(#{{ url: "{base}/missing" }});
                    let err = parse_json(resp.body);
                    "status=" + resp.status + " ok=" + resp.ok + " msg=" + err.error
                }}
            }});
        }}
    }}"#
    );
    define_and_run(&root, "nfnd", &src).await;

    assert_eq!(
        exec(&root, "probe_404", "{}").await,
        "status=404 ok=false msg=no such issue"
    );
}

/// POST：方法、自定义 header、`to_json` 出去的请求体，服务端都要真的收到；
/// 工具入参也要能流进请求体。
#[tokio::test]
async fn rhai_http_posts_headers_and_body_from_tool_args() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, seen) = spawn_api_server();

    let src = format!(
        r#"#{{
        inject: ["tools"],
        apply: |host| {{
            host.register_tool(#{{
                name: "create_issue",
                description: "post one",
                parameters: #{{ type: "object", properties: #{{ title: #{{ type: "string" }} }}, required: ["title"] }},
                execute: |args| {{
                    let resp = http_request(#{{
                        method: "POST",
                        url: "{base}/echo",
                        headers: #{{ "Authorization": "Bearer s3cret", "Content-Type": "application/json" }},
                        body: to_json(#{{ title: args.title, open: true }})
                    }});
                    let back = parse_json(resp.body);
                    "echoed_title=" + back.echoed.title + " open=" + back.echoed.open
                }}
            }});
        }}
    }}"#
    );
    define_and_run(&root, "post", &src).await;

    let out = exec(&root, "create_issue", r#"{"title":"从工具入参来的"}"#).await;
    assert_eq!(out, "echoed_title=从工具入参来的 open=true");

    let requests = seen.lock().unwrap().clone();
    let last = requests.last().expect("服务端应当收到请求");
    assert!(last.starts_with("POST /echo "), "{last}");
    assert!(last.contains("authorization: Bearer s3cret"), "{last}");
    assert!(last.contains(r#""title":"从工具入参来的""#), "{last}");
}

/// 一次 execute 里连打两跳：先列表、再按 id 取详情。真实的 API 封装就是这个形状。
#[tokio::test]
async fn rhai_http_chains_two_requests_in_one_tool_call() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, seen) = spawn_api_server();

    let src = format!(
        r#"#{{
        inject: ["tools"],
        apply: |host| {{
            host.register_tool(#{{
                name: "first_issue_detail",
                description: "list then fetch",
                parameters: #{{ type: "object", properties: #{{}} }},
                execute: |args| {{
                    let list = parse_json(http_request(#{{ url: "{base}/issues" }}).body);
                    let id = list.items[0].id;
                    let detail = parse_json(http_request(#{{ url: "{base}/issues/" + id }}).body);
                    detail.title + " by " + detail.author.login + " (" + detail.comments + " comments)"
                }}
            }});
        }}
    }}"#
    );
    define_and_run(&root, "chain", &src).await;

    assert_eq!(
        exec(&root, "first_issue_detail", "{}").await,
        "first bug by polar (3 comments)"
    );
    assert_eq!(seen.lock().unwrap().len(), 2, "应当打了两跳");
}

/// 闸仍在：换回默认策略，环回地址被 SSRF 挡住，且错误是抛出来的（不是静默空串）。
#[tokio::test]
async fn rhai_http_still_blocks_loopback_under_default_policy() {
    let root = boot().await;
    let (base, _seen) = spawn_api_server();

    let src = format!(
        r#"#{{
        inject: ["tools"],
        apply: |host| {{
            host.register_tool(#{{
                name: "probe_blocked",
                description: "ssrf",
                parameters: #{{ type: "object", properties: #{{}} }},
                execute: |args| {{ http_request(#{{ url: "{base}/issues" }}).body }}
            }});
        }}
    }}"#
    );
    define_and_run(&root, "blk", &src).await;

    let out = exec(&root, "probe_blocked", "{}").await;
    assert!(out.contains("SSRF"), "默认策略应当挡住环回：{out}");
}

/// `http_request` 只挂在**运行期**引擎上：编译检查会 eval 源码顶层，挂上去等于
/// 检查本身就能发请求——那是权限门够不着的时机。
#[tokio::test]
async fn http_request_is_not_available_at_define_time() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, seen) = spawn_api_server();

    let src = format!(
        r#"#{{
        inject: [],
        sneaky: http_request(#{{ url: "{base}/issues" }}),
        apply: |host| {{ }}
    }}"#
    );
    write_plugin(&root, "sneak", &[("source.rhai", &src)]);
    let defined = reload(&root, "sneak").await;
    assert!(
        defined.contains("这一版没跑起来"),
        "define 期不该能发请求：{defined}"
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "服务端不该收到任何请求：{:?}",
        seen.lock().unwrap()
    );
}

/// Codecs + HMAC (#99): running plugin `execute` can call helpers and get exact values.
#[tokio::test]
async fn rhai_codec_helpers_work_inside_execute() {
    let root = boot().await;

    let src = r#"#{
        inject: ["tools"],
        apply: |host| {
            host.register_tool(#{
                name: "codec_probe",
                description: "exercise script codecs",
                parameters: #{ type: "object", properties: #{} },
                execute: |args| {
                    let b64 = to_base64("user:pass");
                    let b64url = to_base64url("hi!");
                    let ue = url_encode("a b");
                    let hx = to_hex("Ab");
                    let s_empty = sha256("");
                    let s_hi = sha256("hi");
                    let mac = hmac_sha256("key", "The quick brown fox jumps over the lazy dog");
                    // Round-trip sanity (Blob → string via as_string when UTF-8).
                    let round = from_base64(b64).as_string();
                    "b64=" + b64
                      + " b64url=" + b64url
                      + " url=" + ue
                      + " hex=" + hx
                      + " sha_empty=" + s_empty
                      + " sha_hi=" + s_hi
                      + " hmac=" + mac
                      + " round=" + round
                }
            });
        }
    }"#;

    define_and_run(&root, "cdc", src).await;
    let out = exec(&root, "codec_probe", "{}").await;
    assert!(
        out.contains("b64=dXNlcjpwYXNz"),
        "to_base64(user:pass): {out}"
    );
    assert!(out.contains("url=a%20b"), "url_encode: {out}");
    assert!(out.contains("hex=4162"), "to_hex: {out}");
    assert!(
        out.contains("sha_empty=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        "sha256 empty: {out}"
    );
    assert!(
        out.contains("sha_hi=8f434346648f6b96df89dda901c5176b10a6d83961dd3c1ac88b59b2dc327aa4"),
        "sha256 hi: {out}"
    );
    assert!(
        out.contains("hmac=f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"),
        "hmac_sha256: {out}"
    );
    assert!(out.contains("round=user:pass"), "base64 roundtrip: {out}");
    assert!(out.contains("b64url=aGkh"), "to_base64url(hi!): {out}");
}

/// Codecs are runtime-only: top-level call during define must fail (like http).
#[tokio::test]
async fn codec_is_not_available_at_define_time() {
    let root = boot().await;
    let src = r#"#{
        inject: [],
        sneaky: to_base64("x"),
        apply: |host| { }
    }"#;
    write_plugin(&root, "cdcf", &[("source.rhai", src)]);
    let defined = reload(&root, "cdcf").await;
    assert!(
        defined.contains("这一版没跑起来"),
        "define 期不该有 to_base64：{defined}"
    );
}

/// Wall clock (#100): execute can call utc_now (map) + thin helpers.
#[tokio::test]
async fn rhai_utc_now_works_inside_execute() {
    let root = boot().await;

    let src = r#"#{
        inject: ["tools"],
        apply: |host| {
            host.register_tool(#{
                name: "time_probe",
                description: "exercise utc_now map",
                parameters: #{ type: "object", properties: #{} },
                execute: |args| {
                    let t = utc_now();
                    let sugar = utc_date();
                    let alias = now_date();
                    "secs=" + t.secs
                      + " ms=" + t.ms
                      + " date=" + t.date
                      + " rfc=" + t.rfc3339
                      + " y=" + t.year
                      + " m=" + t.month
                      + " d=" + t.day
                      + " sugar=" + sugar
                      + " alias=" + alias
                      + " unix=" + unix_time()
                }
            });
        }
    }"#;

    define_and_run(&root, "utm", src).await;
    let out = exec(&root, "time_probe", "{}").await;

    fn field<'a>(out: &'a str, key: &str) -> &'a str {
        out.split(&format!("{key}="))
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_else(|| panic!("parse {key} from {out}"))
    }

    let secs: i64 = field(&out, "secs")
        .parse()
        .unwrap_or_else(|_| panic!("secs not i64: {out}"));
    let date = field(&out, "date");
    let rfc = field(&out, "rfc");
    let y: i64 = field(&out, "y").parse().unwrap();
    let m: i64 = field(&out, "m").parse().unwrap();
    let d: i64 = field(&out, "d").parse().unwrap();
    let sugar = field(&out, "sugar");
    let alias = field(&out, "alias");

    assert!(secs > 1_700_000_000, "too early: {secs} in {out}");
    assert!(secs < 2_100_000_000, "too late: {secs} in {out}");

    let date_ok = date.len() == 10
        && date.as_bytes().get(4) == Some(&b'-')
        && date.as_bytes().get(7) == Some(&b'-')
        && date.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => true,
            _ => b.is_ascii_digit(),
        });
    assert!(date_ok, "date must match YYYY-MM-DD: {date} in {out}");

    let expected = format!("{y:04}-{m:02}-{d:02}");
    assert_eq!(date, expected, "date vs y/m/d inconsistent in {out}");
    assert!(
        rfc.starts_with(date) && rfc.ends_with('Z'),
        "rfc3339 must start with date and end Z: {rfc}"
    );
    assert_eq!(sugar, date, "utc_date sugar mismatch: {out}");
    assert_eq!(alias, date, "now_date alias mismatch: {out}");
}

/// utc_now is runtime-only: top-level call during define must fail (like codec).
#[tokio::test]
async fn utc_now_is_not_available_at_define_time() {
    let root = boot().await;
    let src = r#"#{
        inject: [],
        sneaky: utc_now(),
        apply: |host| { }
    }"#;
    write_plugin(&root, "utmf", &[("source.rhai", src)]);
    let defined = reload(&root, "utmf").await;
    assert!(
        defined.contains("这一版没跑起来"),
        "define 期不该有 utc_now：{defined}"
    );
}

/// `host.secret`：从 `$DOCK_HOME/secrets.json` 读密钥，经权限门（测试里 Allow）后进 execute。
#[tokio::test]
async fn rhai_host_secret_reads_secrets_json() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("secrets.json"),
        r#"{"openai":"sk-from-file","aliyun-ak":"ak-from-file"}"#,
    )
    .unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path());

    let root = boot().await;
    let src = r#"#{
        inject: ["tools"],
        apply: |host| {
            host.register_tool(#{
                name: "read_secret",
                description: "read openai secret",
                parameters: #{ type: "object", properties: #{} },
                execute: |args| {
                    let key = host.secret("openai");
                    "Authorization: Bearer " + key
                }
            });
        }
    }"#;
    define_and_run(&root, "sec", src).await;
    assert_eq!(
        exec(&root, "read_secret", "{}").await,
        "Authorization: Bearer sk-from-file"
    );
}

/// 文件没有时回落到 `DOCK_SECRET_<NAME>`（NAME 大写、非字母数字 → `_`）。
#[tokio::test]
async fn rhai_host_secret_falls_back_to_dock_secret_env() {
    let home = tempfile::tempdir().unwrap();
    // 故意不写 secrets.json
    let _env = cordis_base::test_env::scoped()
        .set("DOCK_HOME", home.path())
        .set("DOCK_SECRET_ALIYUN_AK", "ak-from-env");

    let root = boot().await;
    let src = r#"#{
        inject: ["tools"],
        apply: |host| {
            host.register_tool(#{
                name: "read_ak",
                description: "env fallback",
                parameters: #{ type: "object", properties: #{} },
                execute: |args| { host.secret("aliyun-ak") }
            });
        }
    }"#;
    define_and_run(&root, "akenv", src).await;
    assert_eq!(exec(&root, "read_ak", "{}").await, "ak-from-env");
}

/// 缺失密钥必须 throw，不能静默返回空串。
#[tokio::test]
async fn rhai_host_secret_missing_throws_not_empty() {
    let home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path());

    let root = boot().await;
    let src = r#"#{
        inject: ["tools"],
        apply: |host| {
            host.register_tool(#{
                name: "missing_secret",
                description: "should throw",
                parameters: #{ type: "object", properties: #{} },
                execute: |args| { host.secret("openai") }
            });
        }
    }"#;
    define_and_run(&root, "miss", src).await;
    let out = exec(&root, "missing_secret", "{}").await;
    assert!(out.contains("Error"), "应当是错误：{out}");
    assert!(out.contains("不存在") || out.contains("openai"), "{out}");
    assert_ne!(out.trim(), "");
    assert!(!out.contains("Authorization: Bearer  "), "{out}");
}

/// `cordis_inspect` builtins 列出 `host.secret`，但不泄密。
#[tokio::test]
async fn cordis_inspect_lists_host_secret_builtin() {
    let root = boot().await;
    let out = exec(&root, "cordis_inspect", r#"{"what":"builtins"}"#).await;
    assert!(out.contains("host.secret"), "{out}");
    assert!(
        out.contains("DOCK_SECRET_") || out.contains("secrets.json"),
        "应当提到查找路径：{out}"
    );
    assert!(!out.contains("sk-"), "{out}");
}

/// Removes its file even when an assertion unwinds — a failing test must not
/// leave scratch bytes in the crate directory.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(name: &str, bytes: &[u8]) -> Self {
        let path = std::env::current_dir().unwrap().join(name);
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }

    fn name(&self) -> String {
        self.0.file_name().unwrap().to_string_lossy().into_owned()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

const UPLOAD_BY_PATH_SRC: &str = r#"#{
    inject: ["tools"],
    apply: |host| {
        host.register_tool(#{
            name: "upload_by_path",
            description: "upload a file by path reference",
            parameters: #{
                type: "object",
                properties: #{ path: #{ type: "string" } },
                required: ["path"]
            },
            execute: |args| {
                let bytes = host.read_bytes(args.path);
                let resp = http_request(#{
                    method: "POST",
                    url: "__BASE__/echo",
                    multipart: [
                        #{ name: "purpose", value: "assistants" },
                        #{ name: "file", blob: bytes, filename: "data.bin", content_type: "application/octet-stream" }
                    ]
                });
                if !resp.ok { return "HTTP " + resp.status; }
                // Return only status + size — never the file body.
                "uploaded size=" + bytes.len() + " status=" + resp.status
            }
        });
    }
}"#;

/// #102 hard acceptance: tool args carry only a **path**; host.read_bytes loads
/// bytes; multipart upload succeeds; args/return never contain the file body.
#[tokio::test]
async fn path_reference_upload_args_are_path_only() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, seen) = spawn_api_server();

    let payload = b"REF_UPLOAD_PAYLOAD_bytes_\xff\x00_END";
    let file = TempFile::new(
        &format!(".tmp-dock-upload-{}e2e.bin", std::process::id()),
        payload,
    );
    define_and_run(
        &root,
        "upref",
        &UPLOAD_BY_PATH_SRC.replace("__BASE__", &base),
    )
    .await;

    let args = format!(r#"{{"path":"{}"}}"#, file.name());
    assert!(
        !args.as_bytes().windows(payload.len()).any(|w| w == payload),
        "tool args must not embed file bytes: {args}"
    );
    assert!(!args.contains("REF_UPLOAD_PAYLOAD"), "{args}");

    let out = exec(&root, "upload_by_path", &args).await;
    assert_eq!(
        out,
        format!("uploaded size={} status=200", payload.len()),
        "upload should succeed and report size only"
    );
    assert!(
        !out.as_bytes().windows(payload.len()).any(|w| w == payload),
        "tool return must not contain file body: {out}"
    );

    let requests = seen.lock().unwrap().clone();
    let last = requests.last().expect("server should see the upload");
    assert!(last.contains("multipart/form-data"), "{last}");
    assert!(
        last.contains(r#"name="purpose""#) && last.contains(r#"name="file""#),
        "both parts must reach the server: {last}"
    );
    assert!(
        last.contains(r#"filename="data.bin""#),
        "file part must carry its filename: {last}"
    );
    // Part order is wire order: text field first, file last (OSS PostObject rule).
    assert!(
        last.find(r#"name="purpose""#) < last.find(r#"name="file""#),
        "array order must be wire order: {last}"
    );
    assert!(
        last.contains("REF_UPLOAD_PAYLOAD"),
        "server must receive the file bytes: {last}"
    );
}

/// Convenience requirement: a file outside the workspace uploads by absolute
/// path, exactly like `read_file` can read it. The permission gate is the
/// control, not a directory allowlist.
#[tokio::test]
async fn host_read_bytes_accepts_absolute_path_outside_cwd() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, _seen) = spawn_api_server();

    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("outside-upload.bin");
    let payload = b"OUTSIDE_WORKSPACE_PAYLOAD";
    std::fs::write(&target, payload).unwrap();

    define_and_run(
        &root,
        "upabs",
        &UPLOAD_BY_PATH_SRC.replace("__BASE__", &base),
    )
    .await;

    let out = exec(
        &root,
        "upload_by_path",
        &serde_json::json!({ "path": target.to_str().unwrap() }).to_string(),
    )
    .await;
    assert_eq!(out, format!("uploaded size={} status=200", payload.len()));
}

/// What still fails: nothing to read. Missing files, directories and blank
/// paths throw instead of silently yielding empty bytes.
#[tokio::test]
async fn host_read_bytes_rejects_unreadable_paths() {
    let root = boot().await;
    let src = r#"#{
        inject: ["tools"],
        apply: |host| {
            host.register_tool(#{
                name: "read_probe",
                description: "should fail",
                parameters: #{
                    type: "object",
                    properties: #{ path: #{ type: "string" } },
                    required: ["path"]
                },
                execute: |args| {
                    let b = host.read_bytes(args.path);
                    "read-ok size=" + b.len()
                }
            });
        }
    }"#;
    define_and_run(&root, "rdprb", src).await;

    for (bad, needle) in [
        ("no/such/file-xyz.bin", "cannot resolve"),
        (".", "not a regular file"),
        ("   ", "must not be empty"),
    ] {
        let out = exec(
            &root,
            "read_probe",
            &serde_json::json!({ "path": bad }).to_string(),
        )
        .await;
        assert!(
            out.contains(needle),
            "path {bad:?} should fail with {needle:?}, got: {out}"
        );
        assert!(!out.contains("read-ok"), "{out}");
    }
}

/// The raised upload ceiling is reachable only through a file reference.
/// Same size, two provenances, two outcomes.
#[tokio::test]
async fn over_1mib_uploads_only_by_file_reference() {
    let root = boot().await;
    allow_local_http(&root);
    let (base, _seen) = spawn_api_server();

    // Distinct from the script-built blob below on purpose: provenance is a
    // digest, so byte-identical content is by definition indistinguishable
    // (and harmless — a script that can emit the exact bytes already has them).
    let big: Vec<u8> = (0..1024 * 1024 + 4096).map(|i| (i % 251) as u8).collect();
    let file = TempFile::new(&format!(".tmp-dock-big-{}.bin", std::process::id()), &big);

    let src = format!(
        r#"#{{
        inject: ["tools"],
        apply: |host| {{
            host.register_tool(#{{
                name: "put_big",
                description: "PUT a large body, by path or script-built",
                parameters: #{{
                    type: "object",
                    properties: #{{ path: #{{ type: "string" }}, inline: #{{ type: "boolean" }} }},
                    required: []
                }},
                execute: |args| {{
                    let body = if args.inline == true {{
                        // Script-built payload of the same size. Strings are
                        // capped at 1MiB by the engine, so a Blob is the only
                        // way a script can even hold this much — the gate that
                        // must stop it is provenance, not the engine.
                        blob({size}, 90)
                    }} else {{
                        host.read_bytes(args.path)
                    }};
                    let resp = http_request(#{{ method: "PUT", url: "{base}/put", body: body }});
                    "status=" + resp.status
                }}
            }});
        }}
    }}"#,
        size = big.len(),
    );
    define_and_run(&root, "bigput", &src).await;

    let out = exec(
        &root,
        "put_big",
        &serde_json::json!({ "path": file.name() }).to_string(),
    )
    .await;
    assert_eq!(out, "status=200", "file-referenced upload should succeed");

    let out = exec(
        &root,
        "put_big",
        &serde_json::json!({ "inline": true }).to_string(),
    )
    .await;
    assert!(
        out.contains("host.read_bytes"),
        "a script-built body of the same size must be refused and point at the file route: {out}"
    );
    assert!(!out.contains("status=200"), "{out}");
}

/// Regex helpers and read_bytes are listed for the model via `cordis_inspect`.
#[tokio::test]
async fn rhai_regex_and_read_bytes_listed_in_builtins() {
    let root = boot().await;
    let out = exec(&root, "cordis_inspect", r#"{"what":"builtins"}"#).await;
    for needle in [
        "regex_is_match",
        "regex_find",
        "regex_captures",
        "regex_replace",
        "host.read_bytes",
        "body_blob",
        "multipart",
    ] {
        assert!(out.contains(needle), "builtins must list {needle}: {out}");
    }
}

/// Regex works inside execute (runtime engine), including non-ASCII replacement.
#[tokio::test]
async fn rhai_regex_works_inside_execute() {
    let root = boot().await;
    let src = r#"#{
        inject: ["tools"],
        apply: |host| {
            host.register_tool(#{
                name: "regex_probe",
                description: "regex",
                parameters: #{ type: "object", properties: #{} },
                execute: |args| {
                    let ok = regex_is_match("\\d+", "ab12");
                    let found = regex_find("\\d+", "ab12cd");
                    let caps = regex_captures("(\\w+)=(\\w+)", "a=1");
                    let rep = regex_replace("(\\w+)=(\\w+)", "a=1 b=2", "$2:$1");
                    let cn = regex_replace("token", "my token", "密钥");
                    "ok=" + ok + " found=" + found + " g1=" + caps[1] + " rep=" + rep + " cn=" + cn
                }
            });
        }
    }"#;
    define_and_run(&root, "rex", src).await;
    let out = exec(&root, "regex_probe", "{}").await;
    assert_eq!(out, "ok=true found=12 g1=a rep=1:a 2:b cn=my 密钥");
}

/// 插件根经符号链接装上（注册表记的是别名路径），界面拿规范化后的真实路径来停 /
/// 删：认的是同一个目录，不能当成「别处的同名插件」——停不掉、删了目录插件还在跑。
#[cfg(unix)] // 用到符号链接
#[tokio::test]
async fn disk_plugin_is_matched_by_real_directory_not_spelling() {
    let root = boot().await;
    let real = tempfile::tempdir().unwrap();
    let alias_holder = tempfile::tempdir().unwrap();
    let alias = alias_holder.path().join("alias");
    std::os::unix::fs::symlink(real.path(), &alias).unwrap();
    let plugin_dir = real.path().join("echo");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        "name = \"Echo\"\npurpose = \"alias\"\nfactory = \"echo\"\nenabled = true\n",
    )
    .unwrap();
    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    let roots = [(PersistScope::Project, alias.clone())];
    runner.boot_disk_from(&roots).await;
    let row = |runner: &DynamicRunner| {
        runner
            .disk_plugins_in(&roots)
            .into_iter()
            .find(|p| p.id == "echo")
            .unwrap()
    };
    assert!(row(&runner).running, "经别名装上了");

    let canonical = plugin_dir.canonicalize().unwrap();
    runner.set_disk_enabled_at(&canonical, false).await.unwrap();
    let after = row(&runner);
    assert!(!after.running, "按真实路径停用要停得掉：{after:?}");
    assert!(
        after.shadowed_by.is_none(),
        "同一个目录不是同名的别处插件：{after:?}"
    );

    runner.set_disk_enabled_at(&canonical, true).await.unwrap();
    assert!(row(&runner).running);
    runner.delete_disk_at(&canonical).await.unwrap();
    assert!(!plugin_dir.exists());
    assert!(
        runner.snapshot(MAIN).iter().all(|r| r.plugin_id != "echo")
            && runner.disk_plugins_in(&roots).is_empty(),
        "删掉之后注册表里也不该留着它"
    );
    assert!(
        root.get::<DynEcho>(DYN_ECHO).is_none(),
        "删掉的插件不能还在跑"
    );
}

/// `register_tool` 带 `view`、而这个工具已有别人的卡片视图：apply 失败，作用域回滚，
/// 先挂上的工具不能留下半截。
#[tokio::test]
async fn register_tool_with_a_taken_view_leaves_no_tool_behind() {
    let root = boot().await;
    let views = root
        .require::<cordis_spine::ToolViews>(cordis_spine::TOOL_VIEWS)
        .unwrap();
    let _taken = views
        .register(
            "dup_tool",
            std::sync::Arc::new(|_: &cordis_spine::ToolViewInput<'_>| None),
        )
        .unwrap();
    write_plugin(
        &root,
        "dup",
        &[(
            "source.rhai",
            r#"#{
                inject: ["tools"],
                apply: |host| {
                    host.register_tool(#{
                        name: "dup_tool",
                        parameters: #{ type: "object", properties: #{} },
                        execute: |args| { "ok" },
                        view: |tc| #{ type: "text", text: "x" }
                    });
                    host.provide("dupAlive", #{ ok: true });
                }
            }"#,
        )],
    );
    let out = reload(&root, "dup").await;
    assert!(out.contains("已有卡片视图"), "{out}");
    let names: Vec<_> = tools_of(&root)
        .specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(!names.iter().any(|n| n == "dup_tool"), "{names:?}");
}

/// 回归：同一颗插件的 `session/event` 回调按发出的顺序、一条跑完再跑下一条。
/// 以前每条事件各开一个 spawn_blocking，回调并发乱序：脚本里攒的列表顺序会乱，
/// 「先改计数再刷状态项」的两步也会被别的回调插进来，状态项停在旧值。
#[tokio::test]
async fn session_event_handlers_run_in_order() {
    let root = boot().await;
    write_plugin(
        &root,
        "order",
        &[(
            "source.rhai",
            r#"#{
                inject: ["tools"],
                apply: |host| {
                    let seen = [];
                    host.on("session/event", |line| {
                        seen.push(line.split("\t")[1]);
                        host.set_status(#{ id: "order", text: seen.reduce(|sum, v| if sum == () { v } else { sum + "," + v }) });
                    });
                }
            }"#,
        )],
    );
    let ran = reload(&root, "order").await;
    assert!(ran.contains("运行中"), "{ran}");

    let n = 40;
    for i in 0..n {
        root.emit(
            cordis_spine::SESSION_EVENT,
            LogEvent::ToolExecute {
                id: format!("c{i}"),
                name: format!("t{i}"),
                arguments: "{}".into(),
                content: String::new(),
                images: vec![],
                is_error: false,
            },
        );
    }
    let want: Vec<String> = (0..n).map(|i| format!("t{i}")).collect();
    let want = want.join(",");
    let status = root
        .require::<cordis_spine::StatusItems>(cordis_spine::STATUS_ITEMS)
        .unwrap();
    let mut got = String::new();
    for _ in 0..200 {
        got = status
            .list()
            .into_iter()
            .find(|i| i.id == "order")
            .map(|i| i.text)
            .unwrap_or_default();
        if got.split(',').count() == n {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(got, want, "回调要按发出的顺序跑");
}

/// 仓库里的示例插件 `examples/plugins/activity` 照真实路径（拷进插件目录 → reload）能装上：
/// 面板、状态项、设置卡都登记；工具事件计数；面板按钮暂停 / 清零。示例跟着 Host API 一起变，坏了这里先红。
#[tokio::test]
async fn example_activity_plugin_runs() {
    let home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path());
    let root = boot().await;
    write_plugin(
        &root,
        "activity",
        &[
            (
                "plugin.toml",
                include_str!("../../examples/plugins/activity/plugin.toml"),
            ),
            (
                "source.rhai",
                include_str!("../../examples/plugins/activity/source.rhai"),
            ),
        ],
    );
    let ran = reload(&root, "activity").await;
    assert!(ran.contains("运行中"), "{ran}");

    let status = root
        .require::<cordis_spine::StatusItems>(cordis_spine::STATUS_ITEMS)
        .unwrap();
    let item = |s: &cordis_spine::StatusItems| {
        s.list()
            .into_iter()
            .find(|i| i.id == "activity")
            .expect("状态项 activity")
    };
    assert_eq!(item(&status).text, "空闲 · 工具 0 次");
    assert_eq!(item(&status).surface.as_deref(), Some("activity"));
    let settings = root
        .require::<cordis_spine::PluginSettings>(cordis_spine::PLUGIN_SETTINGS)
        .unwrap();
    assert!(
        settings.list().iter().any(|(id, _)| id == "activity"),
        "{:?}",
        settings.list()
    );

    // 工具事件：session/event 观察者在 emit 之后跑，轮询等它计数。
    let slots = root.require::<cordis_spine::TuiSlots>(TUI_SLOTS).unwrap();
    for name in ["list_dir", "glob", "list_dir"] {
        root.emit(
            cordis_spine::SESSION_EVENT,
            LogEvent::ToolExecute {
                id: format!("c-{name}"),
                name: name.into(),
                arguments: "{}".into(),
                content: "ok".into(),
                images: vec![],
                is_error: false,
            },
        );
    }
    let mut body = String::new();
    for _ in 0..100 {
        body = slots.view("activity").expect("面板有视图").to_plain();
        // 等第三条事件的回调整个跑完（先改计数、再刷状态项）。
        if body.contains("list_dir | 2") && item(&status).text == "空闲 · 工具 3 次" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(body.contains("list_dir | 2"), "{body}");
    assert!(body.contains("glob | 1"), "{body}");
    assert_eq!(item(&status).text, "空闲 · 工具 3 次");

    // 打开旧会话时回放的「本轮结束」（没有 agent/turn-end 钩子跟着）不记。
    root.emit(
        cordis_spine::SESSION_EVENT,
        LogEvent::TurnEnd(cordis_spine::TurnEndStatus::Completed),
    );
    // 流式回复：分片是到目前为止的全文，同一条回复只占一行（真模型会连发好几片）。
    for text in ["This", "This folder", "This folder has a.txt"] {
        root.emit(
            cordis_spine::SESSION_EVENT,
            LogEvent::LlmStream(cordis_spine::LlmOutput {
                text: text.into(),
                ..Default::default()
            }),
        );
    }
    for _ in 0..100 {
        body = slots.view("activity").unwrap().to_plain();
        if body.contains("This folder has a.txt") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(body.contains("• This folder has a.txt [回复]"), "{body}");
    assert!(!body.contains("本轮结束"), "{body}");
    assert_eq!(body.matches("[回复]").count(), 1, "{body}");

    // 面板按钮：暂停 → 状态项跟着变；清零 → 计数归零。
    let _ = slots.on_key("activity", "toggle");
    assert_eq!(item(&status).text, "记录已暂停");
    let _ = slots.on_key("activity", "toggle");
    let _ = slots.on_key("activity", "reset");
    assert_eq!(item(&status).text, "空闲 · 工具 0 次");
    assert!(slots
        .view("activity")
        .unwrap()
        .to_plain()
        .contains("还没有工具调用"));
}

/// web 面板：插件的 `register_slot(#{ web: "panel.html" })` 指向插件目录里的文件，
/// `TuiSlots::web` 现读给 GUI；`..`、绝对路径、没有目录的程序侧包都不许（HTML 不能
/// 跑出插件目录）。
#[tokio::test]
async fn rhai_slot_web_panel_reads_from_the_plugin_directory() {
    let root = boot().await;
    let source = |web: &str| {
        format!(
            r#"#{{
                inject: ["tui.slots"],
                apply: |host| {{
                    host.register_slot(#{{ id: "webdemo", title: "Web", render: || {{ "终端看这行" }}, web: "{web}" }});
                }}
            }}"#
        )
    };
    let plug = write_plugin(
        &root,
        "webdemo",
        &[
            ("panel.html", "<p>hello web</p>"),
            ("source.rhai", &source("panel.html")),
        ],
    );
    let ran = reload(&root, "webdemo").await;
    assert!(ran.contains("登记：面板 webdemo"), "{ran}");
    let slots = root.require::<cordis_spine::TuiSlots>(TUI_SLOTS).unwrap();
    assert!(slots.has_web("webdemo"));
    assert_eq!(slots.web("webdemo").unwrap(), "<p>hello web</p>");
    // 现读：改了文件，下次拿到的就是新的。
    std::fs::write(plug.join("panel.html"), "<p>v2</p>").unwrap();
    assert_eq!(slots.web("webdemo").unwrap(), "<p>v2</p>");
    assert_eq!(slots.render("webdemo").as_deref(), Some("终端看这行"));
    exec_json(
        &root,
        "cordis_plugin",
        json!({"action": "disable", "id": "webdemo"}),
    )
    .await;
    assert!(!slots.has_web("webdemo"));

    // 跑出插件目录的路径：apply 失败，面板不登记。
    for bad in ["../outside.html", "/etc/hosts"] {
        std::fs::write(plug.join("source.rhai"), source(bad)).unwrap();
        let out = reload(&root, "webdemo").await;
        assert!(out.contains("插件目录"), "{bad}: {out}");
        assert!(!slots.has_web("webdemo"), "{bad}");
    }

    // 程序侧 define 的包没有目录：不许带 web。
    let (id, pkg) = define(
        &root,
        new_plugin("webi"),
        "rhai",
        Some(&source("panel.html")),
    )
    .unwrap();
    let out = runner(&root)
        .run(MAIN, &id, &pkg, RunMode::Run)
        .await
        .unwrap_err();
    assert!(out.contains("磁盘插件"), "{out}");
    assert!(!slots.has_web("webdemo"));
}

/// 仓库里的示例 web 面板插件 `examples/plugins/trajectory` 照真实路径装上：
/// 面板有 web 界面，HTML 用的是桥（`dock.call("threads.history")`），终端有一行提示。
#[tokio::test]
async fn example_trajectory_plugin_serves_its_web_panel() {
    let root = boot().await;
    let example =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/trajectory");
    let plug = plugins_root(&root).join("trajectory");
    std::fs::create_dir_all(&plug).unwrap();
    for f in ["plugin.toml", "source.rhai", "panel.html"] {
        std::fs::copy(example.join(f), plug.join(f)).unwrap();
    }
    let ran = reload(&root, "trajectory").await;
    assert!(ran.contains("运行中"), "{ran}");
    let slots = root.require::<cordis_spine::TuiSlots>(TUI_SLOTS).unwrap();
    assert!(slots.has_web("trajectory"));
    let html = slots.web("trajectory").unwrap();
    assert!(html.contains("<title>会话轨迹</title>"), "{}", &html[..200]);
    assert!(html.contains("dock.call(\"threads.history\""));
    assert!(slots.render("trajectory").unwrap().contains("GUI"));
}

/// `cordis-plugin-development` 技能里的例子是模型照抄的起点：每一份都要能拷进插件
/// 目录、reload 起来，注册的工具按例子里写的那样工作。
#[tokio::test]
async fn bundled_skill_examples_define_run_and_work() {
    const DIR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/tools/skills/builtin/cordis-plugin-development/examples/"
    );
    let read = |name: &str| std::fs::read_to_string(format!("{DIR}{name}")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped()
        .set("DOCK_HOME", home.path())
        .remove("DOCK_SECRET_GITHUB");
    let root = boot().await;
    // plugin.toml 例子和 ping 一起：模板里每个键都要认得（认不得会在回执里提醒）。
    write_plugin(&root, "ping", &[("plugin.toml", &read("plugin.toml"))]);
    let out = load_plugin(&root, "ping", &read("ping.rhai")).await;
    assert!(!out.contains("认不得的键"), "{out}");
    assert_eq!(exec(&root, "demo_ping", "{}").await, "pong");

    load_plugin(&root, "memo", &read("memo.rhai")).await;
    assert_eq!(exec(&root, "memo_get", "{}").await, "（空）");
    let set = exec(&root, "memo_set", r#"{"text":"买牛奶"}"#).await;
    assert!(set.contains("已保存 3 字"), "{set}");
    assert_eq!(exec(&root, "memo_get", "{}").await, "买牛奶");

    // 缺密钥要回一句告诉用户去哪填，而不是抛错让模型反复重试。
    load_plugin(&root, "ghapi", &read("api-tool.rhai")).await;
    let out = exec(&root, "gh_issue", r#"{"repo":"o/r","n":1}"#).await;
    assert!(out.starts_with("未配置 GitHub Token"), "{out}");
}

// ── 磁盘插件的五个流程钩子 ───────────────────────────────────────────────

/// 装载一个磁盘 Rhai 插件（`plugin.toml` + `source.rhai`）。返回 TempDir，
/// 调用方要留着它：磁盘插件每次运行都重读源文件。
async fn boot_disk_rhai(root: &Context, id: &str, source: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join(id);
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        format!("name = \"{id}\"\npurpose = \"hooks\"\nfactory = \"rhai\"\nenabled = true\n"),
    )
    .unwrap();
    std::fs::write(plugin_dir.join("source.rhai"), source).unwrap();
    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    runner
        .boot_disk_from(&[(PersistScope::Project, dir.path().to_path_buf())])
        .await;
    dir
}

const RHAI_DISK_DENY: &str = r#"#{
    inject: [],
    apply: |host| {
        host.on("tools/pre-execute", |c| {
            if c.name == "bash" { #{ deny: "脚本不让跑 shell" } } else { () }
        });
    }
}"#;

const RHAI_DISK_REWRITE: &str = r#"#{
    inject: [],
    apply: |host| {
        host.on("tools/pre-execute", |c| {
            if c.name == "bash" {
                #{ rewrite: #{ name: "cordis_inspect", arguments: #{ what: "services" } } }
            } else { () }
        });
    }
}"#;

const RHAI_DISK_RESULT: &str = r#"#{
    inject: [],
    apply: |host| {
        host.on("tools/execute", |r| {
            if r.name == "cordis_inspect" { "REPLACED" } else { () }
        });
    }
}"#;

/// 无条件替换结果内容：用来验证钩子到底有没有被调用（不挑工具名）。
const RHAI_DISK_RESULT_ANY: &str = r#"#{
    inject: [],
    apply: |host| {
        host.on("tools/execute", |r| { "REPLACED" });
    }
}"#;

const RHAI_DISK_PRE_STEP: &str = r#"#{
    inject: [],
    apply: |host| {
        host.on("agent/pre-step", |s| { #{ deny: "脚本拒了这一轮" } });
    }
}"#;

const RHAI_DISK_PROMPT: &str = r#"#{
    inject: [],
    apply: |host| {
        host.on("system-prompt/assemble", |p| { "HOOK-SECTION" });
    }
}"#;

const RHAI_DISK_LLM: &str = r#"#{
    inject: [],
    apply: |host| {
        host.on("llm/stream", |o| { "REWRITTEN" });
    }
}"#;

/// `tools/pre-execute` 的脚本钩子能**否决**一次调用：拒绝在权限门之前，
/// 拒绝的理由原样回到模型。
#[tokio::test]
async fn disk_rhai_plugin_can_refuse_a_tool_call() {
    let root = boot().await;
    let _dir = boot_disk_rhai(&root, "hookdeny", RHAI_DISK_DENY).await;
    let out = exec(&root, "bash", r#"{"command":"echo hi"}"#).await;
    assert!(out.contains("脚本不让跑 shell"), "{out}");
}

/// 同一个钩子还能**改道**：模型要 bash，实际跑的是 `cordis_inspect`。改道之后
/// 仍按新名字过允许名单与权限门。
#[tokio::test]
async fn disk_rhai_plugin_can_redirect_a_tool_call() {
    let root = boot().await;
    let _dir = boot_disk_rhai(&root, "hookrw", RHAI_DISK_REWRITE).await;
    let out = exec(&root, "bash", r#"{"command":"echo ZQXJ_MARKER"}"#).await;
    assert!(!out.contains("ZQXJ_MARKER"), "改道之后不该真跑 bash：{out}");
    assert!(out.contains("services"), "改道后应真的换了工具：{out}");
}

/// `tools/execute` 的钩子能改写模型看到的结果内容。
#[tokio::test]
async fn disk_rhai_plugin_can_rewrite_a_tool_result() {
    let root = boot().await;
    let _dir = boot_disk_rhai(&root, "hookres", RHAI_DISK_RESULT).await;
    let out = exec(&root, "cordis_inspect", r#"{"what":"services"}"#).await;
    assert_eq!(out, "REPLACED");
}

/// `agent/pre-step` 的钩子能拒掉一整轮，拒绝理由进会话（模型看得见）。
#[tokio::test]
async fn disk_rhai_plugin_can_refuse_a_turn() {
    let root = boot_with_loop().await;
    let _dir = boot_disk_rhai(&root, "hookpre", RHAI_DISK_PRE_STEP).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await;
    assert!(out.is_err(), "这一轮应当被脚本拒掉：{out:?}");
    assert!(
        reminders(&root)
            .iter()
            .any(|t| t.contains("这一轮没有开始：脚本拒了这一轮")),
        "{:?}",
        reminders(&root)
    );
}

/// 回归：子代理这一轮被拒时，理由写在子会话里，不进父会话。
#[tokio::test]
async fn a_child_turn_refusal_stays_out_of_the_parent_session() {
    let root = boot_with_loop().await;
    let _dir = boot_disk_rhai(&root, "hookpre2", RHAI_DISK_PRE_STEP).await;
    let child = root.isolate("sessions");
    child
        .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-1"))
        .unwrap();
    let err = LoopHandle::new(child.clone(), std::sync::Arc::new(cordis_spine::GrokStep))
        .run("子任务")
        .await
        .unwrap_err();
    assert!(
        matches!(err, cordis_spine::Error::PreStepRejected),
        "{err:?}"
    );
    let parent = reminders(&root);
    assert!(
        parent.iter().all(|t| !t.contains("这一轮没有开始")),
        "父会话不该出现拒绝理由：{parent:?}"
    );
    let child_notes: Vec<String> = child
        .require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .filter_map(|e| match e {
            LogEvent::SystemReminder(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(
        child_notes
            .iter()
            .any(|t| t.contains("这一轮没有开始：脚本拒了这一轮")),
        "{child_notes:?}"
    );
}

/// `system-prompt/assemble` 的钩子能往系统提示里加一段（固定槽位，字节稳定）。
#[tokio::test]
async fn disk_rhai_plugin_can_add_a_system_prompt_section() {
    let root = boot_with_loop().await;
    let _dir = boot_disk_rhai(&root, "hookprompt", RHAI_DISK_PROMPT).await;
    let _ = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    let prompt = root
        .require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .find_map(|e| match e {
            LogEvent::Prompt(text) => Some(text.clone()),
            _ => None,
        })
        .expect("这一轮记下了系统提示");
    assert!(prompt.contains("HOOK-SECTION"), "{prompt}");
}

/// `llm/stream` 的钩子能改写采样的输出文本（只能改输出，改不了请求）。
#[tokio::test]
async fn disk_rhai_plugin_can_rewrite_the_llm_output() {
    let root = boot_with_loop().await;
    let _dir = boot_disk_rhai(&root, "hookllm", RHAI_DISK_LLM).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert!(
        matches!(out, TurnOutcome::Text(ref text) if text == "REWRITTEN"),
        "{out:?}"
    );
    let streamed: Vec<String> = root
        .require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .filter_map(|e| match e {
            LogEvent::LlmStream(output) => Some(output.text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        streamed.iter().any(|t| t == "REWRITTEN"),
        "改写后的文本要落进会话，不能只改 TurnOutcome：{streamed:?}"
    );
    assert!(
        streamed.iter().all(|t| t == "REWRITTEN" || t.is_empty()),
        "会话里不该留着改写前的原文：{streamed:?}"
    );
}

/// 回归：`tools/execute` 上**别的监听者先注册**时，脚本钩子的改写不能被吞掉。
/// 真实 dock 里 `turn-changes` 就挂在同一个事件上（它先注册，脚本后注册）。
#[tokio::test]
async fn disk_rhai_plugin_result_hook_survives_another_listener() {
    let root = boot().await;
    root.plugin(cordis_spine::turn_changes(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let _dir = boot_disk_rhai(&root, "hookres2", RHAI_DISK_RESULT).await;
    let out = exec(&root, "cordis_inspect", r#"{"what":"services"}"#).await;
    assert_eq!(out, "REPLACED");
}

/// 复现真实 dock 的环境：完整应用树（`install_app`）里，磁盘插件的
/// `tools/execute` 钩子还能不能改写模型看到的结果。真实 dock 上它不生效。
#[tokio::test]
async fn disk_rhai_result_hook_works_in_the_full_app_tree() {
    // 隔离 `$DOCK_HOME`：不把本机的用户插件带进这棵树。走 scoped().home()，
    // 结束时把变量还原，避免下一个用例还指着已经删掉的临时目录。
    let _env = cordis_base::test_env::scoped().home();
    let root = Context::new();
    cordis_spine::install_app(&root).await.unwrap();
    root.get::<cordis_spine::AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Allow);
    let _dir = boot_disk_rhai(&root, "hookres3", RHAI_DISK_RESULT_ANY).await;
    let out = exec(&root, "bash", r#"{"command":"echo hi"}"#).await;
    assert_eq!(out, "REPLACED");
}

/// 权限分层：程序侧 define 的会话包**不能**挂这五个钩子；能挂的只有从插件目录装载
/// 的（启动自动装、设置启用、批准过的 `cordis_plugin reload`）。
#[tokio::test]
async fn a_session_package_cannot_register_the_flow_hooks() {
    let root = boot().await;
    let (id, pkg) = define(&root, new_plugin("hooks"), "rhai", Some(RHAI_DISK_DENY)).unwrap();
    let err = runner(&root)
        .run(MAIN, &id, &pkg, RunMode::Run)
        .await
        .unwrap_err();
    assert!(err.contains("plugins directory"), "{err}");
}

/// 同一份脚本放进插件目录、经 `cordis_plugin reload` 装上（测试里自动批准）就挂得上，
/// 回执和 inspect 都列着这个钩子。
#[tokio::test]
async fn a_reloaded_plugin_can_register_the_flow_hooks() {
    let root = boot().await;
    let ran = load_plugin(&root, "hookdeny", RHAI_DISK_DENY).await;
    assert!(ran.contains("钩子 tools/pre-execute"), "{ran}");
    assert!(ran.contains("触发一次它挂的钩子"), "{ran}");
    let out = exec(&root, "bash", r#"{"command":"echo hi"}"#).await;
    assert!(out.contains("脚本不让跑 shell"), "{out}");
}

// ── 插件宿主的新能力：MCP 做不到的那一面 ─────────────────────────────────

/// 挂上子代理服务（插件的 `host.followup` 用它的信箱）。
async fn mount_subagents(root: &Context) {
    let presets = tempfile::tempdir().unwrap();
    let _ = root.provide(
        AGENT_PRESETS,
        AgentPresets::load(presets.path().to_path_buf()),
    );
    let _ = root.provide("dynamicTestPresets", presets);
    // 默认预设（code）不放行 cordis_plugin；写插件的是创造预设。
    root.require::<AgentPresets>(AGENT_PRESETS)
        .unwrap()
        .apply(CORDIS_PRESET_ID)
        .unwrap();
    root.plugin(
        cordis_spine::tool_task(),
        cordis_spine::TaskConfig::default(),
    )
    .unwrap()
    .wait()
    .await
    .unwrap();
}

/// `tools/execute` 钩子拿得到这次调用的参数：插件判断「刚跑完的是哪条命令」不用自己
/// 在 pre-execute 里记一份再按名字对。
#[tokio::test]
async fn tool_result_hooks_see_the_call_arguments() {
    let root = boot().await;
    load_plugin(
        &root,
        "argseen",
        r#"#{ inject: ["tools"], apply: |host| {
            host.register_tool(#{ name: "echo_args", parameters: #{ type: "object", properties: #{ word: #{ type: "string" } } },
                execute: |a| { "ran" } });
            host.on("tools/execute", |r| {
                if r.name == "echo_args" { "args=" + r.args.word + " raw=" + r.arguments } else { () }
            });
        } }"#,
    )
    .await;
    let out = exec(&root, "echo_args", r#"{"word":"hi"}"#).await;
    assert_eq!(out, r#"args=hi raw={"word":"hi"}"#);
}

/// `tools/pre-execute` 返回 `#{ ask }`：自动批准下也要人点头，确认框里写着插件给的理由。
#[tokio::test(flavor = "multi_thread")]
async fn a_pre_execute_ask_prompts_even_under_auto_approve() {
    let root = boot_gated().await;
    load_plugin(
        &root,
        "pushguard",
        r#"#{ inject: [], apply: |host| {
            host.on("tools/pre-execute", |c| {
                if c.name == "bash" && c.args.command.contains("push") { #{ ask: "会推到远端" } } else { () }
            });
        } }"#,
    )
    .await;
    let perms = root.require::<Permissions>(PERMISSIONS).unwrap();
    let tools = tools_of(&root);
    let call = tokio::spawn(async move {
        tools
            .execute(ToolCall {
                id: "t".into(),
                name: "bash".into(),
                arguments: r#"{"command":"git push origin nothing-here"}"#.into(),
            })
            .await
            .content
    });
    let prompt = wait_for_prompt(&perms).await;
    assert!(
        prompt
            .summary
            .contains("插件要求确认：会推到远端（插件 pushguard）"),
        "{}",
        prompt.summary
    );
    perms.resolve(PermissionOptionKind::RejectOnce);
    let out = call.await.unwrap();
    assert!(
        out.contains("权限被拒绝") && out.contains("会推到远端"),
        "{out}"
    );
    // 不相干的命令照常自动批准。
    let ok = exec(&root, "bash", r#"{"command":"echo fine"}"#).await;
    assert!(ok.contains("fine"), "{ok}");
    assert!(perms.front().is_none());
}

/// `session/entry`：结构化的会话事件。工具那条带参数和结果，助手那条来自采样收尾。
#[tokio::test]
async fn session_entry_reports_user_tool_and_finished_samples() {
    let root = boot_with_loop().await;
    load_plugin(
        &root,
        "ledger",
        r#"#{ inject: ["tools"], apply: |host| {
            let seen = [];
            host.on("session/entry", |e| {
                let tag = e.kind;
                if e.kind == "tool" { tag += ":" + e.name + ":" + e.args.word; }
                if e.kind == "assistant" { tag += ":" + e.text.len() + ":" + type_of(e.usage); }
                seen.push(tag);
            });
            host.register_tool(#{ name: "ledger_seen", parameters: #{ type: "object", properties: #{} },
                execute: |a| { seen.reduce(|s, v| if s == () { v } else { s + "," + v }) ?? "" } });
            host.register_tool(#{ name: "ledger_probe", parameters: #{ type: "object", properties: #{ word: #{ type: "string" } } },
                execute: |a| { "probed" } });
        } }"#,
    )
    .await;
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    sessions.append(LogEvent::ToolExecute {
        id: "c1".into(),
        name: "ledger_probe".into(),
        arguments: r#"{"word":"zz"}"#.into(),
        content: "probed".into(),
        images: vec![],
        is_error: false,
    });
    let mut seen = String::new();
    for _ in 0..100 {
        seen = exec(&root, "ledger_seen", "{}").await;
        if seen.contains("tool:ledger_probe:zz") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(seen.starts_with("user"), "{seen}");
    assert!(seen.contains("assistant:"), "采样收尾那一条：{seen}");
    assert!(seen.contains("tool:ledger_probe:zz"), "{seen}");
}

/// `host.followup`：插件送来的话进这一页的信箱并叫醒它；5 秒内再叫只排队不叫醒。
#[tokio::test]
async fn followup_wakes_the_page_and_rate_limits() {
    let root = boot().await;
    mount_subagents(&root).await;
    load_plugin(
        &root,
        "notify",
        r#"#{ inject: ["tools"], apply: |host| {
            host.register_tool(#{ name: "notify_ping", parameters: #{ type: "object", properties: #{} },
                execute: |a| { host.followup("CI 红了，看一下") } });
        } }"#,
    )
    .await;
    assert_eq!(exec(&root, "notify_ping", "{}").await, "woke");
    let sub = root
        .require::<cordis_spine::Subagents>(cordis_spine::SUBAGENTS)
        .unwrap();
    assert!(sub.has_parent_notices(MAIN));
    let notes = sub.drain_parent_notices(MAIN);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("插件 notify 送来的消息"), "{}", notes[0]);
    assert!(notes[0].contains("CI 红了"), "{}", notes[0]);
    assert_eq!(
        exec(&root, "notify_ping", "{}").await,
        "queued",
        "5 秒内只排队"
    );
}

/// 定时器在插件运行期间触发；间隔太短的直接拒绝。
#[tokio::test(flavor = "multi_thread")]
async fn timers_fire_and_reject_busy_loops() {
    let _env = cordis_base::test_env::scoped().home();
    let root = boot().await;
    load_plugin(
        &root,
        "ticker",
        r#"#{ inject: ["tools"], apply: |host| {
            host.after(1, || { host.save("fired", "yes"); });
            host.register_tool(#{ name: "ticker_fired", parameters: #{ type: "object", properties: #{} },
                execute: |a| { host.load("fired") ?? "no" } });
        } }"#,
    )
    .await;
    let mut fired = String::new();
    for _ in 0..60 {
        fired = exec(&root, "ticker_fired", "{}").await;
        if fired == "yes" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(fired, "yes");
    write_plugin(
        &root,
        "spinner",
        &[(
            "source.rhai",
            r#"#{ inject: [], apply: |host| { host.every(1, || { }); } }"#,
        )],
    );
    let out = reload(&root, "spinner").await;
    assert!(out.contains("at least 5 seconds"), "{out}");
}

/// `host.save` / `host.load` 跨 reload（和重启）留着，落在 `$DOCK_HOME/plugin-state/`。
#[tokio::test]
async fn plugin_state_survives_a_reload() {
    let _env = cordis_base::test_env::scoped().home();
    let root = boot().await;
    let src = r#"#{ inject: ["tools"], apply: |host| {
        host.register_tool(#{ name: "count_up", parameters: #{ type: "object", properties: #{} },
            execute: |a| { let n = (host.load("n") ?? 0) + 1; host.save("n", n); n } });
    } }"#;
    load_plugin(&root, "counter", src).await;
    assert_eq!(exec(&root, "count_up", "{}").await, "1");
    assert_eq!(exec(&root, "count_up", "{}").await, "2");
    reload(&root, "counter").await;
    assert_eq!(exec(&root, "count_up", "{}").await, "3", "换版不丢");
    let home = std::path::PathBuf::from(std::env::var("DOCK_HOME").unwrap());
    let files: Vec<_> = std::fs::read_dir(home.join("plugin-state"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].starts_with("counter-"), "{files:?}");
}

/// `host.llm`：用这一页的模型问一句，不进会话日志。
#[tokio::test]
async fn host_llm_asks_the_page_model_without_logging() {
    let root = boot_with_loop().await;
    load_plugin(
        &root,
        "asker",
        r#"#{ inject: ["tools"], apply: |host| {
            host.register_tool(#{ name: "ask_model", parameters: #{ type: "object", properties: #{} },
                execute: |a| { host.llm(#{ system: "be brief", prompt: "说一个词" }) } });
        } }"#,
    )
    .await;
    let before = root.require::<Sessions>(SESSIONS).unwrap().events().len();
    let out = exec(&root, "ask_model", "{}").await;
    assert!(!out.trim().is_empty() && !out.starts_with("Error"), "{out}");
    assert_eq!(
        root.require::<Sessions>(SESSIONS).unwrap().events().len(),
        before,
        "插件问模型不进会话日志"
    );
}

/// `host.card`：挂在这一轮下面，按钮点了在这一页上跑插件的回调；插件停了卡片留着、
/// 按钮失效。终端那边记一条 Notice。
#[tokio::test]
async fn cards_hang_under_the_turn_and_buttons_call_back() {
    let _env = cordis_base::test_env::scoped().home();
    let root = boot().await;
    root.plugin(cordis_spine::turn_cards(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    sessions.append(LogEvent::User("改一下解析器".into()));
    load_plugin(
        &root,
        "reviewer",
        r#"#{ inject: ["tools"], apply: |host| {
            host.register_tool(#{ name: "post_card", parameters: #{ type: "object", properties: #{} },
                execute: |a| {
                    host.card(#{ id: "tests", title: "改了 Rust 文件", text: "还没跑测试", tone: "warning",
                        view: #{ type: "kv", items: [#{ label: "文件", value: 2 }] },
                        actions: [#{ id: "run", label: "跑测试" }],
                        on_action: |action| { host.save("clicked", action); "已安排：" + action } })
                } });
            host.register_tool(#{ name: "clicked", parameters: #{ type: "object", properties: #{} },
                execute: |a| { host.load("clicked") ?? "none" } });
        } }"#,
    )
    .await;
    assert_eq!(exec(&root, "post_card", "{}").await, "tests");
    let cards = root
        .require::<cordis_spine::TurnCards>(cordis_spine::TURN_CARDS)
        .unwrap();
    let list = cards.list(MAIN);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].ordinal, 0);
    assert_eq!(list[0].spec.tone, "warning");
    assert!(list[0].spec.view.is_some());
    assert!(list[0].live);
    let toast = cards.action(MAIN, &list[0].key, "run").await.unwrap();
    assert_eq!(toast.as_deref(), Some("已安排：run"));
    assert_eq!(exec(&root, "clicked", "{}").await, "run");
    assert!(sessions.events().iter().any(
        |e| matches!(e, LogEvent::Notice { title, .. } if title == "reviewer · 改了 Rust 文件")
    ));

    exec_json(
        &root,
        "cordis_plugin",
        json!({"action": "disable", "id": "reviewer"}),
    )
    .await;
    let list = cards.list(MAIN);
    assert_eq!(list.len(), 1, "卡片留着");
    assert!(!list[0].live, "按钮失效");
    assert!(cards.action(MAIN, &list[0].key, "run").await.is_err());
}

// ── hooks 桥接（Claude Code / Codex 格式的 command hook）─────────────────

async fn mount_hooks(root: &Context) {
    root.plugin(cordis_spine::hooks_bridge(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
}

fn write_user_hooks(json: &str) {
    let home = std::path::PathBuf::from(std::env::var("DOCK_HOME").unwrap());
    std::fs::write(home.join("hooks.json"), json).unwrap();
}

/// 用户目录下的 `hooks.json` 直接生效：`PreToolUse` 退出码 2 拦下调用，stderr 是理由；
/// matcher 按 Claude 的名字（`Bash`）写也认。
#[tokio::test(flavor = "multi_thread")]
async fn a_user_pre_tool_use_hook_blocks_with_its_stderr() {
    let _env = cordis_base::test_env::scoped().home();
    let root = boot_gated().await;
    mount_hooks(&root).await;
    write_user_hooks(
        r#"{ "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [
            { "type": "command", "command": "input=$(cat); case \"$input\" in *rm*) echo '不许删东西' >&2; exit 2;; esac" } ] } ] } }"#,
    );
    let out = exec(&root, "bash", r#"{"command":"rm -rf /tmp/never-there"}"#).await;
    assert!(out.contains("不许删东西（PreToolUse hook）"), "{out}");
    let ok = exec(&root, "bash", r#"{"command":"echo ok"}"#).await;
    assert!(ok.contains("ok") && !ok.contains("hook"), "{ok}");
}

/// 项目里的 hooks 文件第一次要跑时在这一页问；点了头按内容指纹记下，之后不再问。
#[tokio::test(flavor = "multi_thread")]
async fn a_project_hooks_file_asks_once_before_it_runs() {
    let _env = cordis_base::test_env::scoped().home();
    let root = boot_gated().await;
    mount_hooks(&root).await;
    let dot_dock = plugins_root(&root).parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dot_dock).unwrap();
    std::fs::write(
        dot_dock.join("hooks.json"),
        r#"{ "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [
            { "type": "command", "command": "echo 'repo says no' >&2; exit 2" } ] } ] } }"#,
    )
    .unwrap();
    let perms = root.require::<Permissions>(PERMISSIONS).unwrap();
    let run = || {
        let tools = tools_of(&root);
        tokio::spawn(async move {
            tools
                .execute(ToolCall {
                    id: "t".into(),
                    name: "bash".into(),
                    arguments: r#"{"command":"echo hi"}"#.into(),
                })
                .await
                .content
        })
    };
    let first = run();
    let prompt = wait_for_prompt(&perms).await;
    assert!(prompt.tool.starts_with("hooks "), "{}", prompt.tool);
    assert!(
        prompt.summary.contains("repo says no"),
        "{}",
        prompt.summary
    );
    perms.resolve(PermissionOptionKind::AllowOnce);
    assert!(first
        .await
        .unwrap()
        .contains("repo says no（PreToolUse hook）"));
    let second = run().await.unwrap();
    assert!(second.contains("repo says no"), "{second}");
    assert!(perms.front().is_none(), "点过头就不再问");
}

/// `UserPromptSubmit` 的 stdout 当上下文送进这一轮；`Stop` 退出码 2 让回合接着干一次，
/// 下一次 hook 看到 `stop_hook_active: true` 就放行。
#[tokio::test]
async fn prompt_and_stop_hooks_shape_the_turn() {
    let _env = cordis_base::test_env::scoped().home();
    let root = boot_with_loop().await;
    mount_hooks(&root).await;
    write_user_hooks(
        r#"{ "hooks": {
            "UserPromptSubmit": [ { "hooks": [ { "type": "command", "command": "echo '今天是周五'" } ] } ],
            "Stop": [ { "hooks": [ { "type": "command",
                "command": "input=$(cat); case \"$input\" in *'\"stop_hook_active\":true'*) exit 0;; esac; echo '先跑测试' >&2; exit 2" } ] } ]
        } }"#,
    );
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    let seen = reminders(&root);
    assert!(
        seen.iter()
            .any(|r| r.contains("hooks 附加的上下文：\n今天是周五")),
        "{seen:?}"
    );
    let stops: Vec<_> = seen
        .iter()
        .filter(|r| r.contains("Stop hook 要求继续：先跑测试"))
        .collect();
    assert_eq!(stops.len(), 1, "只续一次：{seen:?}");
    assert_eq!(samples(&root), 2);
}

// ── 技能里的行为插件例子 ───────────────────────────────────────────────

fn skill_example(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/src/tools/skills/builtin/cordis-plugin-development/examples/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// `guard.rhai`：不可回滚的命令直接拒、冻结目录不许改、`git push` 要用户点头。
#[tokio::test(flavor = "multi_thread")]
async fn example_guard_enforces_project_rules() {
    let root = boot().await;
    load_plugin(&root, "guard", &skill_example("guard.rhai")).await;
    let out = exec(&root, "bash", r#"{"command":"git reset --hard HEAD~1"}"#).await;
    assert!(out.contains("项目规约不许 `git reset --hard`"), "{out}");
    let out = exec(
        &root,
        "search_replace",
        r#"{"file_path":"vendor/xai/lib.rs","old_string":"a","new_string":"b"}"#,
    )
    .await;
    assert!(out.contains("vendor/ 是冻结副本"), "{out}");
    let perms = root.require::<Permissions>(PERMISSIONS).unwrap();
    let tools = tools_of(&root);
    let push = tokio::spawn(async move {
        tools
            .execute(ToolCall {
                id: "t".into(),
                name: "bash".into(),
                arguments: r#"{"command":"git push origin main"}"#.into(),
            })
            .await
            .content
    });
    let prompt = wait_for_prompt(&perms).await;
    assert!(
        prompt.summary.contains("会改到仓库之外"),
        "{}",
        prompt.summary
    );
    perms.resolve(PermissionOptionKind::RejectOnce);
    assert!(push.await.unwrap().contains("权限被拒绝"));
    // 普通命令不受影响（echo 工具表原样回参数）。
    assert!(exec(&root, "bash", r#"{"command":"ls"}"#)
        .await
        .contains("ls"));
}

/// 先改一个 `.rs`，再回正文的模型。
struct EditRust;

impl cordis_spine::Sampler for EditRust {
    fn sample<'a>(
        &'a self,
        request: cordis_spine::PromptRequest,
        _on_delta: Box<dyn FnMut(cordis_spine::StreamDelta) + Send + 'a>,
    ) -> cordis_spine::BoxFuture<'a, cordis_spine::LlmOutput> {
        let edited = request
            .history
            .iter()
            .any(|e| matches!(e, LogEvent::ToolExecute { .. }));
        Box::pin(async move {
            if !edited {
                return cordis_spine::LlmOutput {
                    tool_calls: vec![ToolCall {
                        id: "e1".into(),
                        name: "search_replace".into(),
                        arguments: json!({ "file_path": "src/lib.rs", "old_string": "a", "new_string": "b" })
                            .to_string(),
                    }],
                    ..Default::default()
                };
            }
            cordis_spine::LlmOutput {
                text: "改好了".into(),
                ..Default::default()
            }
        })
    }
}

/// `done-check.rhai`：改了 `.rs` 没跑测试，回合想收尾时被拦回去一次，并挂一张「跑测试」卡片。
#[tokio::test]
async fn example_done_check_holds_the_turn_until_tests_run() {
    let _env = cordis_base::test_env::scoped().home();
    let root = boot().await;
    root.provide(
        cordis_spine::LLM,
        cordis_spine::Llm::from_sampler(root.clone(), std::sync::Arc::new(EditRust)),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root.plugin(cordis_spine::turn_cards(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    load_plugin(&root, "done-check", &skill_example("done-check.rhai")).await;
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("把 a 改成 b")
        .await
        .unwrap();
    let seen = reminders(&root);
    let held: Vec<_> = seen
        .iter()
        .filter(|r| r.contains("src/lib.rs，还没跑过 cargo test"))
        .collect();
    assert_eq!(held.len(), 1, "只拦一次：{seen:?}");
    let cards = root
        .require::<cordis_spine::TurnCards>(cordis_spine::TURN_CARDS)
        .unwrap()
        .list(MAIN);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].spec.title, "改了 Rust 文件，还没跑测试");
    assert_eq!(cards[0].spec.text, "src/lib.rs");
}

/// `trim-output.rhai`：去掉 ANSI 颜色码，超长输出只留头尾。
#[tokio::test]
async fn example_trim_output_strips_colors_and_long_middles() {
    let root = boot_gated().await;
    load_plugin(&root, "trim-output", &skill_example("trim-output.rhai")).await;
    let red = exec(
        &root,
        "bash",
        r#"{"command":"printf '\\033[31mred\\033[0m done'"}"#,
    )
    .await;
    assert!(red.contains("red done"), "{red:?}");
    assert!(!red.contains('\u{1b}'), "{red:?}");
    let long = exec(
        &root,
        "bash",
        r#"{"command":"yes abcdefghij | head -c 20000"}"#,
    )
    .await;
    assert!(
        long.contains("[trim-output：中间"),
        "{}",
        &long[..200.min(long.len())]
    );
    assert!(long.len() < 8000, "{}", long.len());
}
