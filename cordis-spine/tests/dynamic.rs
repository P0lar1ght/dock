use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use cordis_spine::{
    agent_loop, agent_presets, dynamic_runner, llm, permissions, settings, slash, tool_cordis,
    tool_todo, tools, tui_slots, AgentPresets, DynEcho, DynamicRunner, LlmConfig, LlmMode,
    LogEvent, LoopHandle, PermissionMode, PersistScope, PluginOrigin, PreStep, RhaiBag, RunMode,
    Sessions, Slash, ToolBody, ToolCall, ToolResult, ToolSpec, Tools, TurnOutcome, AGENT_LOOP,
    AGENT_PRESETS, CORDIS_PRESET_ID, DYNAMIC_CORDIS_RUNNER, DYN_ECHO, DYN_ECHO_TOOL, PRE_STEP,
    SESSIONS, SETTINGS, SLASH, TOOLS, TUI_SLOTS,
};
use serde_json::json;

const MAIN: &str = "main";

async fn boot() -> Context {
    let root = Context::new();
    cordis_spine::install_core(&root).await.unwrap();
    root.plugin(tools(), ()).unwrap().wait().await.unwrap();
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
    root
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
    let out = exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo"}"#,
    )
    .await;
    assert!(out.contains("echo-1/pkg-1"), "{out}");
    assert!(out.contains("not running"), "{out}");
    assert!(root.get::<DynEcho>(DYN_ECHO).is_none());
    let names: Vec<_> = tools_of(&root)
        .specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(!names.iter().any(|n| n == DYN_ECHO_TOOL), "{names:?}");
}

#[tokio::test]
async fn run_stop_roundtrip_mounts_fiber_and_tool() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo"}"#,
    )
    .await;
    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"echo-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");
    assert!(root.get::<DynEcho>(DYN_ECHO).is_some());
    let ping = exec(&root, DYN_ECHO_TOOL, "{}").await;
    assert_eq!(ping, "pong");

    let fibers = exec(&root, "cordis_inspect", r#"{"what":"fibers"}"#).await;
    assert!(fibers.contains("cordis-dynamic"), "{fibers}");
    assert!(fibers.contains("echo-1"), "{fibers}");
    let services = exec(&root, "cordis_inspect", r#"{"what":"services"}"#).await;
    assert!(services.contains("dynEcho"), "{services}");
    assert!(services.contains("dynamicCordisRunner"), "{services}");
    assert!(services.contains("host.register_tool"), "{services}");
    assert!(services.contains("cannot replace /agents"), "{services}");
    assert!(services.contains("host.open_slot"), "{services}");
    let tools_only = exec(&root, "cordis_inspect", r#"{"what":"tools"}"#).await;
    assert!(!tools_only.contains("host.register_tool"), "{tools_only}");

    exec(&root, "cordis_stop", r#"{"pluginId":"echo-1"}"#).await;
    assert!(root.get::<DynEcho>(DYN_ECHO).is_none());
    let names: Vec<_> = tools_of(&root)
        .specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(!names.iter().any(|n| n == DYN_ECHO_TOOL), "{names:?}");
    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    let row = runner.inspect_plugin(MAIN, "echo-1").unwrap();
    assert_eq!(row.current_package_id.as_deref(), Some("pkg-1"));
    assert!(row.active_run.is_none());
}

#[tokio::test]
async fn second_echo_fails_and_does_not_leave_a_fiber() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"A","purpose":"a","factory":"echo"}"#,
    )
    .await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"B","purpose":"b","factory":"echo"}"#,
    )
    .await;
    exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"echo-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    let failed = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"echo-2","packageId":"pkg-2","mode":"run"}"#,
    )
    .await;
    assert!(
        failed.contains("has been registered") || failed.contains("already registered"),
        "{failed}"
    );
    assert!(failed.contains("cordis_stop"), "{failed}");
    assert!(root.get::<DynEcho>(DYN_ECHO).is_some());
    let fibers = exec(&root, "cordis_inspect", r#"{"what":"fibers"}"#).await;
    assert!(fibers.contains("echo-1"), "{fibers}");
    assert!(!fibers.contains("echo-2"), "{fibers}");
    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    assert!(runner
        .inspect_plugin(MAIN, "echo-2")
        .unwrap()
        .active_run
        .is_none());
}

/// 两颗工具并成一颗之后，两个问题都还能问，而且问错了要报错而不是静默回目录。
#[tokio::test]
async fn inspect_answers_both_questions_and_rejects_a_bare_package_id() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"both"},"name":"Both","purpose":"merge","factory":"note"}"#,
    )
    .await;

    let directory = exec(&root, "cordis_inspect", r#"{"what":"factories"}"#).await;
    assert!(directory.contains("factories:"), "{directory}");

    let plugin = exec(&root, "cordis_inspect", r#"{"pluginId":"both-1"}"#).await;
    assert!(plugin.contains("mode: plugin"), "{plugin}");
    assert!(plugin.contains("pkg-1"), "{plugin}");

    let package = exec(
        &root,
        "cordis_inspect",
        r#"{"pluginId":"both-1","packageId":"pkg-1"}"#,
    )
    .await;
    assert!(package.contains("mode: package"), "{package}");
    assert!(package.contains("factory: note"), "{package}");

    // pluginId 比 what 窄，窄的赢。
    let both = exec(
        &root,
        "cordis_inspect",
        r#"{"what":"factories","pluginId":"both-1"}"#,
    )
    .await;
    assert!(both.contains("mode: plugin"), "{both}");
    assert!(!both.contains("factories:"), "{both}");

    let orphan = exec(&root, "cordis_inspect", r#"{"packageId":"pkg-1"}"#).await;
    assert!(orphan.contains("requires pluginId"), "{orphan}");
}

/// `drop` 是开关不是第二颗工具：停掉保留版本，drop 才丢定义；两者都不碰磁盘。
#[tokio::test]
async fn stop_keeps_the_definition_unless_drop_is_set() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"keep"},"name":"Keep","purpose":"stop vs drop","factory":"echo"}"#,
    )
    .await;
    exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"keep-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;

    let stopped = exec(&root, "cordis_stop", r#"{"pluginId":"keep-1"}"#).await;
    assert!(
        stopped.contains("definition and versions remain"),
        "{stopped}"
    );
    let still = exec(&root, "cordis_inspect", r#"{"pluginId":"keep-1"}"#).await;
    assert!(still.contains("pkg-1"), "stop must keep Packages: {still}");
    // 停下来的包可以再 run 起来，这正是不 drop 的理由。
    let again = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"keep-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(again.contains("\"status\":\"running\""), "{again}");

    let dropped = exec(&root, "cordis_stop", r#"{"pluginId":"keep-1","drop":true}"#).await;
    assert!(dropped.contains("Removed"), "{dropped}");
    assert!(dropped.contains("wasRunning=true"), "{dropped}");
    let gone = exec(&root, "cordis_inspect", r#"{"pluginId":"keep-1"}"#).await;
    assert!(gone.contains("Error"), "{gone}");
}

#[tokio::test]
async fn hold_stays_pending() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"hold"},"name":"Hold","purpose":"wait","factory":"hold"}"#,
    )
    .await;
    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"hold-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("waiting") || ran.contains("pending"), "{ran}");
    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    let row = runner.inspect_plugin(MAIN, "hold-1").unwrap();
    let run = row.active_run.expect("hold should keep a fiber");
    assert_eq!(run.waiting_for, ["dynHoldGate"]);
    let fiber = runner
        .live_fibers(MAIN)
        .into_iter()
        .find(|f| f.plugin_id.as_deref() == Some("hold-1"))
        .expect("hold fiber");
    assert_eq!(fiber.state, "pending");
}

#[tokio::test]
async fn update_mode_and_drop() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"note"},"name":"N1","purpose":"v1","factory":"note"}"#,
    )
    .await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"existing","pluginId":"note-1"},"name":"N2","purpose":"v2","factory":"note"}"#,
    )
    .await;
    exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"note-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    let bad = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"note-1","packageId":"pkg-2","mode":"run"}"#,
    )
    .await;
    assert!(bad.contains("update"), "{bad}");
    let updated = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"note-1","packageId":"pkg-2","mode":"update"}"#,
    )
    .await;
    assert!(updated.contains("pkg-2"), "{updated}");
    let gone = exec(&root, "cordis_stop", r#"{"pluginId":"note-1","drop":true}"#).await;
    assert!(gone.contains("Removed"), "{gone}");
    let listed = exec(&root, "cordis_inspect", r#"{"what":"temporary"}"#).await;
    assert!(
        listed.contains("No session-local Plugins") || !listed.contains("note-1"),
        "{listed}"
    );
}

#[tokio::test]
async fn slash_factory_adds_and_stop_removes() {
    let root = boot().await;
    let defined = exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"slsh"},"name":"Standup","purpose":"站会","factory":"slash","command":"standup","kind":"prompt","text":"写站会：{args}","send":true}"#,
    )
    .await;
    assert!(defined.contains("slsh-1/pkg-1"), "{defined}");
    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"slsh-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");
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

    exec(&root, "cordis_stop", r#"{"pluginId":"slsh-1"}"#).await;
    assert!(slash.list().is_empty());
}

#[tokio::test]
async fn slash_tool_kind_binds_live_tool_name() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo"}"#,
    )
    .await;
    exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"echo-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    let defined = exec(
        &root,
        "cordis_define",
        &format!(
            r#"{{"plugin":{{"kind":"new","idPrefix":"trig"}},"name":"Test","purpose":"probe","factory":"slash","command":"probe","kind":"tool","text":"{DYN_ECHO_TOOL}"}}"#
        ),
    )
    .await;
    assert!(defined.contains("factory slash"), "{defined}");
    // "Defined trig-N/pkg-M (Test); ..." — counter is process-global across prefixes.
    let ids = defined
        .strip_prefix("Defined ")
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or_else(|| panic!("{defined}"));
    let (plugin_id, package_id) = ids.split_once('/').unwrap_or_else(|| panic!("{defined}"));
    let ran = exec(
        &root,
        "cordis_run",
        &format!(r#"{{"pluginId":"{plugin_id}","packageId":"{package_id}","mode":"run"}}"#),
    )
    .await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");
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
    let out = exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"slsh"},"name":"Nope","purpose":"no","factory":"slash","command":"help","kind":"overlay","text":"x"}"#,
    )
    .await;
    assert!(out.contains("cannot shadow"), "{out}");
    assert!(root.require::<Slash>(SLASH).unwrap().list().is_empty());
}

#[tokio::test]
async fn echo_rejects_slash_fields() {
    let root = boot().await;
    let out = exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo","command":"standup"}"#,
    )
    .await;
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

#[tokio::test]
async fn rhai_compile_reject_does_not_mint() {
    let root = boot().await;
    let out = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "bad"},
            "name": "Bad",
            "purpose": "syntax",
            "factory": "rhai",
            "source": "this is not { rhai"
        }),
    )
    .await;
    assert!(out.contains("Error"), "{out}");
    assert!(out.contains("rhai"), "{out}");
    assert!(out.contains("source must be a map"), "{out}");
    assert!(out.contains("does not call apply"), "{out}");
    let listed = exec(&root, "cordis_inspect", r#"{"what":"temporary"}"#).await;
    assert!(
        listed.contains("No session-local Plugins") || !listed.contains("bad-1"),
        "{listed}"
    );
}

/// `inject` 进的是**内核**的 `Inject`，内核按名字查 store（类型擦除），所以一个
/// 脚本够不着的 spine 服务照样能满足它——fiber active、apply 跑过、工具可调。
/// 以前 `waiting for` 拿 `service_present` 那张只认几个名字的表单独判，于是这种包
/// 被报成 waiting，模型会去修一个没坏的插件。
#[tokio::test]
async fn injecting_a_mounted_spine_service_runs_and_is_not_reported_as_waiting() {
    let root = boot().await;
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "gate"},
            "name": "Gate",
            "purpose": "inject a spine service the script cannot reach",
            "factory": "rhai",
            "source": r#"
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
        }),
    )
    .await;
    assert!(defined.contains("gate-1/pkg-1"), "{defined}");

    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"gate-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    let receipt: serde_json::Value = serde_json::from_str(&ran).expect("run receipt json");
    assert_eq!(receipt["status"], "running", "{ran}");
    assert_eq!(receipt["host"]["fiberState"], "active", "{ran}");
    assert_eq!(
        receipt["host"]["waitingFor"],
        json!([]),
        "a resolved inject is not a wait: {ran}"
    );

    let fibers = exec(&root, "cordis_inspect", r#"{"what":"fibers"}"#).await;
    assert!(fibers.contains("gate-1 [active]"), "{fibers}");
    assert!(
        !fibers.contains("waiting for"),
        "running plugin must not be listed as waiting: {fibers}"
    );

    // 真跑起来了，不只是状态好看。
    let called = exec(&root, "cordis_call", r#"{"name":"gate_ping"}"#).await;
    assert!(called.contains("pong"), "{called}");

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

/// 超限时该说的是「改走 source_path」，不是「把脚本写短点」。
#[tokio::test]
async fn oversized_inline_source_points_at_source_path() {
    let root = boot().await;
    let big = format!(
        "#{{ inject: [], apply: |host| {{ }} }}\n// {}",
        "x".repeat(130 * 1024)
    );
    let out = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "big"},
            "name": "Big",
            "purpose": "oversize",
            "factory": "rhai",
            "source": big,
        }),
    )
    .await;
    assert!(out.contains("128KiB"), "{out}");
    assert!(out.contains("source_path"), "{out}");
    assert!(out.contains("write_file"), "{out}");
    let listed = exec(&root, "cordis_inspect", r#"{"what":"temporary"}"#).await;
    assert!(
        !listed.contains("big-1"),
        "oversize must not mint: {listed}"
    );
}

/// `cordis_*` 是 deferred 工具，描述不常驻上下文。apply 炸了的那一刻是唯一需要
/// 修复闭环的时刻，所以闭环跟着失败一起回；而 mode/id 这类校验错误自己已经写清了
/// 下一步，再贴一段会把它埋掉。
#[tokio::test]
async fn a_failed_apply_carries_the_repair_loop_and_a_mode_error_does_not() {
    let root = boot().await;
    exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "boom"},
            "name": "Boom",
            "purpose": "throwing apply",
            "factory": "rhai",
            "source": r#"#{ inject: ["tools"], apply: |host| { throw "nope" } }"#,
        }),
    )
    .await;
    let failed = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"boom-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(failed.contains("Error"), "{failed}");
    assert!(failed.contains("kind \"existing\""), "{failed}");
    assert!(failed.contains("cordis_inspect"), "{failed}");

    let bad_mode = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"boom-1","packageId":"pkg-404","mode":"run"}"#,
    )
    .await;
    assert!(bad_mode.contains("Error"), "{bad_mode}");
    assert!(
        !bad_mode.contains("kind \"existing\""),
        "a validation error must not carry the repair essay: {bad_mode}"
    );
}

#[tokio::test]
async fn rhai_source_path_define_run_and_inspect_shows_path() {
    let root = boot().await;
    let cwd = std::env::current_dir().unwrap();
    let plug = cwd.join(".dock/plugins/demopath");
    std::fs::create_dir_all(&plug).unwrap();
    let source_file = plug.join("source.rhai");
    std::fs::write(
        plug.join("plugin.toml"),
        "name = \"DemoPath\"\npurpose = \"file backed\"\nfactory = \"rhai\"\nenabled = true\n",
    )
    .unwrap();
    std::fs::write(
        &source_file,
        r#"#{
            inject: ["tools"],
            apply: |host| {
                host.register_tool(#{
                    name: "demopath_ping",
                    description: "pong from file",
                    parameters: #{ type: "object", properties: #{} },
                    execute: |args| { "file-pong" }
                });
            }
        }"#,
    )
    .unwrap();

    let both = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "dpath"},
            "name": "DemoPath",
            "purpose": "file backed",
            "factory": "rhai",
            "source": "#{ inject: [], apply: |host| { } }",
            "source_path": ".dock/plugins/demopath/source.rhai"
        }),
    )
    .await;
    assert!(both.contains("Error"), "{both}");
    assert!(
        both.contains("not both") || both.contains("source or source_path"),
        "{both}"
    );

    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "dpath"},
            "name": "DemoPath",
            "purpose": "file backed",
            "factory": "rhai",
            "source_path": ".dock/plugins/demopath/source.rhai"
        }),
    )
    .await;
    assert!(defined.contains("dpath-1/pkg-1"), "{defined}");

    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"dpath-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");
    assert_eq!(exec(&root, "demopath_ping", "{}").await, "file-pong");

    let inspect = exec_json(
        &root,
        "cordis_inspect",
        json!({ "pluginId": "dpath-1", "packageId": "pkg-1" }),
    )
    .await;
    assert!(inspect.contains("source_path:"), "{inspect}");
    assert!(inspect.contains("demopath"), "{inspect}");
    assert!(!inspect.contains("file-pong"), "{inspect}");
    assert!(!inspect.contains("register_tool"), "{inspect}");

    // Escape / outside root rejected
    let escape = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "nope"},
            "name": "Nope",
            "purpose": "escape",
            "factory": "rhai",
            "source_path": ".dock/plugins/demopath/../../../Cargo.toml"
        }),
    )
    .await;
    assert!(escape.contains("Error"), "{escape}");
    assert!(
        escape.contains("..")
            || escape.contains("must resolve under")
            || escape.contains("source_path"),
        "{escape}"
    );

    let _ = std::fs::remove_dir_all(plug);
}

#[tokio::test]
async fn rhai_source_path_rejects_symlink_swap_after_define() {
    let root = boot().await;
    let cwd = std::env::current_dir().unwrap();
    let plug = cwd.join(".dock/plugins/swapprobe");
    std::fs::create_dir_all(&plug).unwrap();
    let source_file = plug.join("source.rhai");
    std::fs::write(
        plug.join("plugin.toml"),
        "name = \"SwapProbe\"\npurpose = \"symlink swap\"\nfactory = \"rhai\"\nenabled = true\n",
    )
    .unwrap();
    std::fs::write(
        &source_file,
        r#"#{
            inject: ["tools"],
            apply: |host| {
                host.register_tool(#{
                    name: "swapprobe_ping",
                    description: "should not run after swap",
                    parameters: #{ type: "object", properties: #{} },
                    execute: |args| { "file-pong" }
                });
            }
        }"#,
    )
    .unwrap();

    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "swap"},
            "name": "SwapProbe",
            "purpose": "symlink swap",
            "factory": "rhai",
            "source_path": ".dock/plugins/swapprobe/source.rhai"
        }),
    )
    .await;
    assert!(defined.contains("swap-1/pkg-1"), "{defined}");

    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "TOP SECRET").unwrap();
    std::fs::remove_file(&source_file).unwrap();
    std::os::unix::fs::symlink(outside.path(), &source_file).unwrap();

    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"swap-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("Error"), "{ran}");
    assert!(
        ran.contains("must resolve under")
            || ran.contains("source_path")
            || ran.contains("not a file"),
        "{ran}"
    );
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

    let _ = std::fs::remove_dir_all(plug);
}

#[tokio::test]
async fn rhai_run_registers_tool_provide_and_slot_then_stop_unregisters() {
    let root = boot().await;
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "memo"},
            "name": "Memo",
            "purpose": "bag",
            "factory": "rhai",
            "source": RHAI_MEMO,
        }),
    )
    .await;
    assert!(defined.contains("memo-1/pkg-1"), "{defined}");
    assert!(root.get::<RhaiBag>("dynMemo").is_none());

    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"memo-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");
    let bag = root.get::<RhaiBag>("dynMemo").expect("provide dynMemo");
    assert_eq!(bag.get()["text"], "hi");
    assert_eq!(exec(&root, "memo_get", "{}").await, "memo-ok");

    let inspect = exec_json(
        &root,
        "cordis_inspect",
        json!({
            "pluginId": "memo-1",
            "packageId": "pkg-1"
        }),
    )
    .await;
    assert!(inspect.contains("source:"), "{inspect}");
    assert!(inspect.contains("register_tool"), "{inspect}");

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
        vec![("memo-1".to_string(), "便签".to_string())]
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

    exec(&root, "cordis_stop", r#"{"pluginId":"memo-1"}"#).await;
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

    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo"}"#,
    )
    .await;
    exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"echo-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;

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
        search_res.contains("\"server\": \"dynamic\""),
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

    // 也可以通过 cordis_call 调度执行
    let call_res = exec(
        &root,
        "cordis_call",
        &json!({
            "name": DYN_ECHO_TOOL,
            "arguments": {}
        })
        .to_string(),
    )
    .await;
    assert!(call_res.contains("pong"), "{call_res}");

    assert_eq!(exec(&root, DYN_ECHO_TOOL, "{}").await, "pong");
}

#[tokio::test]
async fn cordis_call_invokes_dynamic_tool_from_host_meta_path() {
    let root = boot().await;

    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo"}"#,
    )
    .await;
    exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"echo-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;

    let via_meta = exec(
        &root,
        "cordis_call",
        &format!(r#"{{"name":"{DYN_ECHO_TOOL}","arguments":{{}}}}"#),
    )
    .await;
    assert!(
        via_meta.contains("## cordis_call → dyn_echo") && via_meta.contains("pong"),
        "{via_meta}"
    );

    let recursive = exec(
        &root,
        "cordis_call",
        r#"{"name":"cordis_call","arguments":{"name":"dyn_echo"}}"#,
    )
    .await;
    assert!(recursive.contains("cannot call itself"), "{recursive}");

    let missing = exec(
        &root,
        "cordis_call",
        r#"{"name":"dyn_repo_info","arguments":{}}"#,
    )
    .await;
    assert!(missing.contains("is not registered"), "{missing}");
}

#[tokio::test]
async fn at_plugin_id_pre_step_injects_identity_reminder() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo"}"#,
    )
    .await;
    let user = "@echo-1 please inspect";
    root.waterfall(PRE_STEP, PreStep::new(user, true, MAIN), move || {
        PreStep::new(user, true, MAIN)
    });
    let events = root.require::<Sessions>(SESSIONS).unwrap().events();
    let reminder = events.iter().find_map(|e| match e {
        LogEvent::SystemReminder(text) => Some(text.as_str()),
        _ => None,
    });
    let text = reminder.expect("system reminder");
    assert!(text.contains("echo-1"), "{text}");
    assert!(text.contains("pkg-1"), "{text}");
    assert!(!text.contains("factory"), "{text}");
    assert!(!text.to_lowercase().contains("source:"), "{text}");
}

#[tokio::test]
async fn other_session_cannot_see_owned_plugin() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo"}"#,
    )
    .await;
    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    assert!(runner.inspect_plugin(MAIN, "echo-1").is_ok());
    assert!(runner.inspect_plugin("child-other", "echo-1").is_err());
    assert!(runner.reference("child-other", "echo-1").is_err());
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
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "orb"},
            "name": "Orphan",
            "purpose": "rollback",
            "factory": "rhai",
            "source": RHAI_APPLY_THEN_RESERVED_SLASH,
        }),
    )
    .await;
    assert!(defined.contains("orb-1/pkg-1"), "{defined}");

    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"orb-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(
        ran.contains("cannot shadow") || ran.contains("Error") || ran.contains("error"),
        "{ran}"
    );
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

    let stopped = exec(&root, "cordis_stop", r#"{"pluginId":"orb-1"}"#).await;
    assert!(
        stopped.contains("wasRunning") || stopped.contains("Stopped") || stopped.contains("orb-1"),
        "{stopped}"
    );
    let names = extra_names(&root);
    assert!(!names.iter().any(|n| n == "orb_list"), "{names:?}");

    exec(&root, "cordis_stop", r#"{"pluginId":"orb-1","drop":true}"#).await;
    let again = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "orb"},
            "name": "Retry",
            "purpose": "new id",
            "factory": "echo",
        }),
    )
    .await;
    assert!(again.contains("orb-2"), "{again}");
    assert!(!again.contains("orb-1/"), "{again}");
}

#[tokio::test]
async fn update_replaces_entire_fiber_not_overlay() {
    let root = boot().await;
    exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "aagt"},
            "name": "A",
            "purpose": "tools",
            "factory": "rhai",
            "source": RHAI_TWO_TOOLS,
        }),
    )
    .await;
    exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"aagt-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(extra_names(&root).iter().any(|n| n == "aagt_list"));
    assert!(root.get::<RhaiBag>("dynAgentList").is_some());

    exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "existing", "pluginId": "aagt-1"},
            "name": "B",
            "purpose": "slash only",
            "factory": "rhai",
            "source": RHAI_SLASH_ONLY,
        }),
    )
    .await;
    let updated = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"aagt-1","packageId":"pkg-2","mode":"update"}"#,
    )
    .await;
    assert!(updated.contains("pkg-2"), "{updated}");
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
    let out = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "noap"},
            "name": "NoApply",
            "purpose": "missing apply",
            "factory": "rhai",
            "source": "#{ inject: [\"tools\"] }",
        }),
    )
    .await;
    assert!(out.contains("apply"), "{out}");
    let listed = exec(&root, "cordis_inspect", r#"{"what":"temporary"}"#).await;
    assert!(
        listed.contains("No session-local Plugins") || !listed.contains("noap-1"),
        "{listed}"
    );
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
    exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "strp"},
            "name": "Str",
            "purpose": "string params",
            "factory": "rhai",
            "source": RHAI_STRING_PARAMS,
        }),
    )
    .await;
    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"strp-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("must be a map"), "{ran}");
    assert!(ran.contains("not a JSON string"), "{ran}");
    let names = extra_names(&root);
    assert!(!names.iter().any(|n| n == "str_params"), "{names:?}");
}

#[tokio::test]
async fn rhai_required_unknown_property_fail_loud() {
    let root = boot().await;
    exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "schm"},
            "name": "Schema",
            "purpose": "bad required",
            "factory": "rhai",
            "source": RHAI_BAD_REQUIRED,
        }),
    )
    .await;
    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"schm-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("undeclared property"), "{ran}");
    assert!(ran.contains("nope"), "{ran}");
    let names = extra_names(&root);
    assert!(!names.iter().any(|n| n == "bad_req"), "{names:?}");
}

#[tokio::test]
async fn concurrent_identical_cordis_run_shares_in_flight() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo"}"#,
    )
    .await;
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
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo"}"#,
    )
    .await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"existing","pluginId":"echo-1"},"name":"Note","purpose":"other","factory":"note"}"#,
    )
    .await;
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
    exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "obsv"},
            "name": "Observer",
            "purpose": "session/event",
            "factory": "rhai",
            "source": RHAI_ON_EVENT,
        }),
    )
    .await;
    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"obsv-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");

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
    assert!(listing.contains("echo"), "{listing}");
    assert!(listing.contains("永久"), "{listing}");

    let permanent = exec(&root, "cordis_inspect", r#"{"what":"permanent"}"#).await;
    assert!(
        permanent.contains("永久") || permanent.contains("echo") || permanent.contains("disk"),
        "{permanent}"
    );

    let gone = exec(&root, "cordis_stop", r#"{"pluginId":"echo","drop":true}"#).await;
    assert!(gone.contains("Removed"), "{gone}");
    assert!(plugin_dir.join("plugin.toml").exists());
}

#[tokio::test]
async fn promote_writes_disk_and_stops_session_copy() {
    let root = boot().await;
    exec(
        &root,
        "cordis_define",
        r#"{"plugin":{"kind":"new","idPrefix":"echo"},"name":"Echo","purpose":"ping","factory":"echo"}"#,
    )
    .await;
    exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"echo-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(root.get::<DynEcho>(DYN_ECHO).is_some());

    let dir = tempfile::tempdir().unwrap();
    let runner = root
        .require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap();
    let receipt = runner
        .promote_into(
            MAIN,
            "echo-1",
            None,
            PersistScope::Project,
            dir.path().to_path_buf(),
        )
        .await
        .expect("promote");
    assert_eq!(receipt.plugin_id, "echo");
    assert!(receipt.stopped_source);
    assert!(dir.path().join("echo").join("plugin.toml").exists());
    assert!(root.get::<DynEcho>(DYN_ECHO).is_some());
    assert!(runner.inspect_plugin(MAIN, "echo").is_ok());
    let session = runner.inspect_plugin(MAIN, "echo-1").unwrap();
    assert!(session.active_run.is_none());
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

async fn run_rhai(root: &Context, prefix: &str, source: &str) {
    let defined = exec_json(
        root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": prefix},
            "name": "Hooks",
            "purpose": "waterfall",
            "factory": "rhai",
            "source": source,
        }),
    )
    .await;
    assert!(defined.contains("pkg-1"), "{defined}");
    let ran = exec_json(
        root,
        "cordis_run",
        json!({"pluginId": format!("{prefix}-1"), "packageId": "pkg-1", "mode": "run"}),
    )
    .await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");
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
/// the chain — and its slot (50) loses to the built-in gate (10).
#[tokio::test]
async fn a_rhai_hook_cannot_outrank_or_swallow_the_todo_gate() {
    let root = boot_with_loop().await;
    // Script first, so the built-in gate is *inside* it on the chain: if the
    // host ever stopped calling `args.next()`, the gate's vote would vanish.
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
    let first = seen.first().expect("the gate must continue the turn");
    assert!(
        first.contains(cordis_spine::TODO_GATE_SENTINEL),
        "the built-in gate outranks the script: {first}"
    );
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

async fn define_and_run(root: &Context, prefix: &str, src: &str) {
    let defined = exec_json(
        root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": prefix},
            "name": "Api", "purpose": "wrap an api",
            "factory": "rhai", "source": src
        }),
    )
    .await;
    assert!(defined.contains("pkg-1"), "{defined}");
    let ran = exec(
        root,
        "cordis_run",
        &format!(r#"{{"pluginId":"{prefix}-1","packageId":"pkg-1","mode":"run"}}"#),
    )
    .await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");
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

/// `http_request` 只挂在**运行期**引擎上：define 期的 preflight 会 eval 源码顶层，
/// 挂上去等于 `cordis_define` 本身就能发请求——那是权限门够不着的时机。
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
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "sneak"},
            "name": "Sneaky", "purpose": "define-time fetch",
            "factory": "rhai", "source": src
        }),
    )
    .await;
    assert!(
        defined.contains("Error"),
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
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "cdcf"},
            "name": "SneakyCodec", "purpose": "define-time codec",
            "factory": "rhai", "source": src
        }),
    )
    .await;
    assert!(
        defined.contains("Error"),
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
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "utmf"},
            "name": "SneakyTime", "purpose": "define-time utc_now",
            "factory": "rhai", "source": src
        }),
    )
    .await;
    assert!(
        defined.contains("Error"),
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
        runner.session_plugins().is_empty() && runner.disk_plugins_in(&roots).is_empty(),
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
    exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "dup"},
            "name": "Dup",
            "purpose": "view conflict",
            "factory": "rhai",
            "source": r#"#{
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
        }),
    )
    .await;
    let out = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"dup-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
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
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "order"},
            "name": "Order",
            "purpose": "session/event order",
            "factory": "rhai",
            "source": r#"#{
                inject: ["tools"],
                apply: |host| {
                    let seen = [];
                    host.on("session/event", |line| {
                        seen.push(line.split("\t")[1]);
                        host.set_status(#{ id: "order", text: seen.reduce(|sum, v| if sum == () { v } else { sum + "," + v }) });
                    });
                }
            }"#,
        }),
    )
    .await;
    assert!(!defined.contains("Error"), "{defined}");
    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"order-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(!ran.contains("Error"), "{ran}");

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

/// 仓库里的示例插件 `examples/plugins/activity` 照真实路径（define → run）能装上：
/// 面板、状态项、设置卡都登记；工具事件计数；面板按钮暂停 / 清零。示例跟着 Host API 一起变，坏了这里先红。
#[tokio::test]
async fn example_activity_plugin_runs() {
    let home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path());
    let root = boot().await;
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "activ"},
            "name": "Agent 活动",
            "purpose": "example",
            "factory": "rhai",
            "source": include_str!("../../examples/plugins/activity/source.rhai"),
        }),
    )
    .await;
    assert!(!defined.contains("Error"), "{defined}");
    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"activ-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(!ran.contains("Error"), "{ran}");

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
        settings.list().iter().any(|(id, _)| id == "activ-1"),
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

/// web 面板：磁盘插件的 `register_slot(#{ web: "panel.html" })` 指向插件目录里的文件，
/// `TuiSlots::web` 现读给 GUI；内联插件、`..`、绝对路径都不许（HTML 不能跑出插件目录）。
#[tokio::test]
async fn rhai_slot_web_panel_reads_from_the_plugin_directory() {
    // 插件放在用户级插件根（DOCK_HOME/plugins）下，用绝对 source_path，不切进程 cwd：
    // 同一个测试二进制里有按 cwd 相对路径建文件的用例（上传那几条），切 cwd 会和它们抢。
    let home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path());
    let root = boot().await;
    let plug = home.path().join("plugins/webdemo");
    std::fs::create_dir_all(&plug).unwrap();
    std::fs::write(plug.join("panel.html"), "<p>hello web</p>").unwrap();
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
    let run = |prefix: &'static str, n: u32| {
        let root = root.clone();
        async move {
            exec(
                &root,
                "cordis_run",
                &format!(r#"{{"pluginId":"{prefix}-{n}","packageId":"pkg-1","mode":"run"}}"#),
            )
            .await
        }
    };

    std::fs::write(plug.join("source.rhai"), source("panel.html")).unwrap();
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "webd"},
            "name": "WebDemo",
            "purpose": "web panel",
            "factory": "rhai",
            "source_path": plug.join("source.rhai").display().to_string()
        }),
    )
    .await;
    assert!(!defined.contains("Error"), "{defined}");
    let ran = run("webd", 1).await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");
    let slots = root.require::<cordis_spine::TuiSlots>(TUI_SLOTS).unwrap();
    assert!(slots.has_web("webdemo"));
    assert_eq!(slots.web("webdemo").unwrap(), "<p>hello web</p>");
    // 现读：改了文件，下次拿到的就是新的。
    std::fs::write(plug.join("panel.html"), "<p>v2</p>").unwrap();
    assert_eq!(slots.web("webdemo").unwrap(), "<p>v2</p>");
    assert_eq!(slots.render("webdemo").as_deref(), Some("终端看这行"));
    exec(&root, "cordis_stop", r#"{"pluginId":"webd-1"}"#).await;
    assert!(!slots.has_web("webdemo"));

    // 跑出插件目录的路径：apply 失败，面板不登记。
    for bad in ["../outside.html", "/etc/hosts"] {
        std::fs::write(plug.join("source.rhai"), source(bad)).unwrap();
        let defined = exec_json(
            &root,
            "cordis_define",
            json!({
                "plugin": {"kind": "existing", "pluginId": "webd-1"},
                "name": "WebDemo",
                "purpose": "web panel",
                "factory": "rhai",
                "source_path": plug.join("source.rhai").display().to_string()
            }),
        )
        .await;
        let pkg = defined
            .split("webd-1/")
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_alphanumeric() && c != '-').next())
            .unwrap_or("pkg-2")
            .to_string();
        let out = exec(
            &root,
            "cordis_run",
            &format!(r#"{{"pluginId":"webd-1","packageId":"{pkg}","mode":"update"}}"#),
        )
        .await;
        assert!(out.contains("插件目录"), "{bad}: {out}");
        assert!(!slots.has_web("webdemo"), "{bad}");
    }

    // 内联插件没有目录：不许带 web。
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "webi"},
            "name": "WebInline",
            "purpose": "inline web",
            "factory": "rhai",
            "source": source("panel.html"),
        }),
    )
    .await;
    assert!(!defined.contains("Error"), "{defined}");
    // 定义回执里带 `<插件 id>/<包 id>`。
    let (id, pkg) = defined
        .split(|c: char| c.is_whitespace() || c == '`' || c == '"' || c == '(' || c == ')')
        .find_map(|w| {
            let (id, pkg) = w.split_once("/pkg-")?;
            let n: String = pkg.chars().take_while(char::is_ascii_digit).collect();
            id.starts_with("webi-")
                .then(|| (id.to_string(), format!("pkg-{n}")))
        })
        .expect("回执里有插件 id 和包 id");
    let out = exec(
        &root,
        "cordis_run",
        &format!(r#"{{"pluginId":"{id}","packageId":"{pkg}","mode":"run"}}"#),
    )
    .await;
    assert!(out.contains("磁盘插件"), "{out}");
    assert!(!slots.has_web("webdemo"));
}

/// 仓库里的示例 web 面板插件 `examples/plugins/trajectory` 照磁盘插件的真实路径装上：
/// 面板有 web 界面，HTML 用的是桥（`dock.call("threads.history")`），终端有一行提示。
#[tokio::test]
async fn example_trajectory_plugin_serves_its_web_panel() {
    // 插件放在用户级插件根（DOCK_HOME/plugins）下，用绝对 source_path，不切进程 cwd：
    // 同一个测试二进制里有按 cwd 相对路径建文件的用例（上传那几条），切 cwd 会和它们抢。
    let home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path());
    let root = boot().await;
    let example =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/trajectory");
    let plug = home.path().join("plugins/trajectory");
    std::fs::create_dir_all(&plug).unwrap();
    for f in ["plugin.toml", "source.rhai", "panel.html"] {
        std::fs::copy(example.join(f), plug.join(f)).unwrap();
    }
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "traj"},
            "name": "会话轨迹",
            "purpose": "example",
            "factory": "rhai",
            "source_path": plug.join("source.rhai").display().to_string()
        }),
    )
    .await;
    assert!(!defined.contains("Error"), "{defined}");
    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"traj-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");
    let slots = root.require::<cordis_spine::TuiSlots>(TUI_SLOTS).unwrap();
    assert!(slots.has_web("trajectory"));
    let html = slots.web("trajectory").unwrap();
    assert!(html.contains("<title>会话轨迹</title>"), "{}", &html[..200]);
    assert!(html.contains("dock.call(\"threads.history\""));
    assert!(slots.render("trajectory").unwrap().contains("GUI"));
}

/// `cordis-plugin-development` 技能里的例子是模型照抄的起点：每一份都要真能
/// define + run，注册的工具按例子里写的那样工作。
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
    // 发号器全进程单调，`define_and_run` 认 `-1`，所以每份例子单独起一棵树。
    let root = boot().await;
    define_and_run(&root, "ping", &read("ping.rhai")).await;
    assert_eq!(exec(&root, "demo_ping", "{}").await, "pong");

    let root = boot().await;
    define_and_run(&root, "memo", &read("memo.rhai")).await;
    assert_eq!(exec(&root, "memo_get", "{}").await, "（空）");
    let set = exec(&root, "memo_set", r#"{"text":"买牛奶"}"#).await;
    assert!(set.contains("已保存 3 字"), "{set}");
    assert_eq!(exec(&root, "memo_get", "{}").await, "买牛奶");

    // 缺密钥要回一句告诉用户去哪填，而不是抛错让模型反复重试。
    let root = boot().await;
    define_and_run(&root, "ghapi", &read("api-tool.rhai")).await;
    let out = exec(&root, "gh_issue", r#"{"repo":"o/r","n":1}"#).await;
    assert!(out.starts_with("未配置 GitHub Token"), "{out}");
}

// ---- examples/plugins/tool-batch ----------------------------------------------------------
// 真跑：工作区工具（read_file / list_dir / grep / bash）+ 磁盘插件自动加载（boot_disk_from），
// 再经 Tools::execute 调 tool_batch。host.call_tool 用 block_in_place，必须多线程运行时。

async fn boot_tool_batch() -> (Context, tempfile::TempDir, tempfile::TempDir) {
    // 照 README 装：整个目录拷进插件根，启动时自动加载（plugin.toml enabled = true）。
    let example =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/tool-batch");
    let files: Vec<(&str, String)> = ["plugin.toml", "source.rhai"]
        .into_iter()
        .map(|f| (f, std::fs::read_to_string(example.join(f)).unwrap()))
        .collect();
    boot_disk_plugin("tool-batch", &files).await
}

/// 工作区工具 + 一个磁盘插件（`files` 写进插件根里的 `id/`），Allow 模式。
async fn boot_disk_plugin(
    id: &str,
    files: &[(&str, String)],
) -> (Context, tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let root = Context::new();
    cordis_spine::install_core(&root).await.unwrap();
    root.plugin(settings(), ()).unwrap().wait().await.unwrap();
    root.get::<cordis_spine::AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Allow);
    root.plugin(permissions(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(cordis_spine::workspace_tools(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(slash(), ()).unwrap().wait().await.unwrap();
    root.plugin(tui_slots(), ()).unwrap().wait().await.unwrap();
    for p in [cordis_spine::status_items(), cordis_spine::tool_views()] {
        root.plugin(p, ()).unwrap().wait().await.unwrap();
    }
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
    let plugins = tempfile::tempdir().unwrap();
    let plug = plugins.path().join(id);
    std::fs::create_dir_all(&plug).unwrap();
    for (f, body) in files {
        std::fs::write(plug.join(f), body).unwrap();
    }
    root.require::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER)
        .unwrap()
        .boot_disk_from(&[(PersistScope::Project, plugins.path().to_path_buf())])
        .await;
    (root, home, plugins)
}

/// 跑一次 `tool_batch`，回内容文本。插件把停下 / 被拒标成 `is_error`（内容只有 JSON）；
/// 这里给 `is_error` 的结果补一行 `Error: tool_batch stopped at step N` / `rejected`，
/// 断言时一眼看出是哪种失败。
async fn batch(root: &Context, v: serde_json::Value) -> String {
    let r = tools_of(root)
        .execute(ToolCall {
            id: "t".into(),
            name: "tool_batch".into(),
            arguments: v.to_string(),
        })
        .await;
    if !r.is_error {
        return r.content;
    }
    let v: serde_json::Value = serde_json::from_str(&r.content)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.content));
    let how = match v["failed_step"].as_i64() {
        Some(n) if v["stopped"] == true => format!("stopped at step {n}"),
        _ => "rejected".to_string(),
    };
    format!("Error: tool_batch {how}: {}\n{}", v["error"], r.content)
}

/// 去掉中途停下时的 `Error: …` 首行，剩下的 JSON。
fn batch_json(out: &str) -> serde_json::Value {
    let body = match out.strip_prefix("Error: ") {
        Some(rest) => rest.split_once('\n').map(|(_, j)| j).unwrap_or(rest),
        None => out,
    };
    serde_json::from_str(body).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn example_tool_batch_chains_with_save_as_and_paths() {
    let (root, home, _p) = boot_tool_batch().await;
    let f = home.path().join("data.json");
    std::fs::write(
        &f,
        r#"{"name":"dock","items":[{"id":7,"tag":"a"},{"id":9,"tag":"b"}],"cmd":"echo whole-ref"}"#,
    )
    .unwrap();
    let out = batch(
        &root,
        json!({
            "steps": [
                {"tool": "bash", "args": {"command": format!("cat {}", f.display())}, "save_as": "cfg"},
                // 拼接引用：路径 + 数组下标（含负数）。
                {"tool": "bash", "args": {"command": "echo ${cfg.name}-${cfg.items.1.id}-${cfg.items.-1.tag}"},
                 "save_as": "label", "expect": "dock-9-b"},
                // 整串引用：原值代入（这里是字符串命令本身）。
                {"tool": "bash", "args": {"command": "${cfg.cmd}"}, "save_as": "whole"},
                // 非字符串值拼进文本时是 JSON。
                {"tool": "bash", "args": {"command": "echo '${cfg.items.0}'"}, "save_as": "item0"},
                {"tool": "read_file", "args": {"target_file": f}, "expect": "\"dock\""},
                {"tool": "list_dir", "args": {"target_directory": home.path()}, "expect": "data.json"},
                {"tool": "grep", "args": {"pattern": "whole-ref", "path": home.path()}, "save_as": "hit"}
            ],
            "return": ["label", "whole", "item0", "cfg.items.-1", "hit", "step4"]
        }),
    )
    .await;
    assert!(!out.starts_with("Error"), "{out}");
    let v = batch_json(&out);
    assert_eq!(v["ok"], true, "{out}");
    assert_eq!(v["completed"], 7, "{out}");
    assert!(v["failed_step"].is_null(), "{out}");
    let r = &v["results"];
    assert!(r["label"].as_str().unwrap().contains("dock-9-b"), "{out}");
    assert!(r["whole"].as_str().unwrap().contains("whole-ref"), "{out}");
    assert!(
        r["item0"]["id"] == 7 || r["item0"].as_str().is_some_and(|s| s.contains("\"id\":7")),
        "{out}"
    );
    // 路径返回原值：对象原样回成 JSON 对象。
    assert_eq!(r["cfg.items.-1"]["tag"], "b", "{out}");
    assert!(r["hit"].as_str().unwrap().contains("data.json"), "{out}");
    assert!(r["step4"].as_str().unwrap().contains("dock"), "{out}");
    let trace = v["trace"].as_array().unwrap();
    assert_eq!(trace.len(), 7, "{out}");
    assert!(
        trace.iter().all(|t| t.as_str().unwrap().contains(" ok ")),
        "{out}"
    );
    println!("chain: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn example_tool_batch_expect_failure_stops() {
    let (root, home, _p) = boot_tool_batch().await;
    let marker = home.path().join("should-not-exist");
    let out = batch(
        &root,
        json!({
            "steps": [
                {"tool": "bash", "args": {"command": "echo hello"}, "expect": "goodbye"},
                {"tool": "bash", "args": {"command": format!("touch {}", marker.display())}}
            ]
        }),
    )
    .await;
    assert!(
        out.starts_with("Error: tool_batch stopped at step 0"),
        "{out}"
    );
    let v = batch_json(&out);
    assert_eq!(v["ok"], false);
    assert_eq!(v["failed_step"], 0);
    assert_eq!(v["completed"], 0);
    assert_eq!(v["stopped"], true);
    assert!(
        v["error"].as_str().unwrap().contains("expect not found"),
        "{out}"
    );
    assert_eq!(v["trace"].as_array().unwrap().len(), 1, "{out}");
    assert!(!marker.exists(), "第二步不该跑");
    // 默认返回最后跑过的那一步（失败那步）的结果。
    assert!(
        v["results"]["step0"].as_str().unwrap().contains("hello"),
        "{out}"
    );

    // 工具自己报错（bash 非零退出）也算失败、默认停。
    let out = batch(
        &root,
        json!({ "steps": [
            {"tool": "bash", "args": {"command": "exit 3"}},
            {"tool": "bash", "args": {"command": format!("touch {}", marker.display())}}
        ]}),
    )
    .await;
    assert!(
        out.starts_with("Error: tool_batch stopped at step 0"),
        "{out}"
    );
    assert!(
        batch_json(&out)["error"].as_str().unwrap().contains("exit"),
        "{out}"
    );
    assert!(!marker.exists());
    println!("stop: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn example_tool_batch_on_error_continue() {
    let (root, _home, _p) = boot_tool_batch().await;
    let out = batch(
        &root,
        json!({
            "steps": [
                {"tool": "bash", "args": {"command": "exit 3"}, "on_error": "continue", "save_as": "bad"},
                {"tool": "bash", "args": {"command": "echo hi"}, "expect": "nope", "on_error": "continue"},
                {"tool": "bash", "args": {"command": "echo after"}, "save_as": "after"}
            ],
            "return": ["bad", "after"]
        }),
    )
    .await;
    // 没有停下：不带 Error 前缀，但 ok=false、failed_step 指向第一处失败。
    assert!(!out.starts_with("Error"), "{out}");
    let v = batch_json(&out);
    assert_eq!(v["ok"], false, "{out}");
    assert_eq!(v["stopped"], false, "{out}");
    assert_eq!(v["failed_step"], 0, "{out}");
    assert_eq!(v["completed"], 1, "{out}");
    let trace: Vec<_> = v["trace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().to_string())
        .collect();
    assert_eq!(trace.len(), 3, "{out}");
    assert!(
        trace[0].contains(" fail ") && trace[1].contains(" fail ") && trace[2].contains(" ok "),
        "{trace:?}"
    );
    assert!(
        v["results"]["bad"].as_str().unwrap().contains("exit"),
        "{out}"
    );
    assert!(
        v["results"]["after"].as_str().unwrap().contains("after"),
        "{out}"
    );
    println!("continue: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn example_tool_batch_missing_ref_names_the_step() {
    let (root, home, _p) = boot_tool_batch().await;
    let marker = home.path().join("should-not-exist");
    let out = batch(
        &root,
        json!({
            "steps": [
                {"tool": "bash", "args": {"command": "echo '{\"a\":1}'"}, "save_as": "x"},
                // on_error=continue 也不救引用错：批次本身写错了。
                {"tool": "bash", "args": {"command": "echo ${nope.field}"}, "on_error": "continue"},
                {"tool": "bash", "args": {"command": format!("touch {}", marker.display())}}
            ]
        }),
    )
    .await;
    assert!(
        out.starts_with("Error: tool_batch stopped at step 1"),
        "{out}"
    );
    let v = batch_json(&out);
    let err = v["error"].as_str().unwrap();
    assert!(err.contains("step 1") && err.contains("\"nope\""), "{out}");
    assert!(!marker.exists());

    // 字段不存在、下标越界同样点名。
    let out = batch(
        &root,
        json!({ "steps": [
            {"tool": "bash", "args": {"command": "echo '{\"a\":[1]}'"}, "save_as": "x"},
            {"tool": "bash", "args": {"command": "echo ${x.a.5}"}}
        ]}),
    )
    .await;
    assert!(
        batch_json(&out)["error"]
            .as_str()
            .unwrap()
            .contains("out of range"),
        "{out}"
    );
    let out = batch(
        &root,
        json!({ "steps": [
            {"tool": "bash", "args": {"command": "echo '{\"a\":[1]}'"}, "save_as": "x"},
            {"tool": "bash", "args": {"command": "echo ${x.b}"}}
        ]}),
    )
    .await;
    assert!(
        batch_json(&out)["error"]
            .as_str()
            .unwrap()
            .contains("field \"b\" not found"),
        "{out}"
    );
    println!("missing ref: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn example_tool_batch_truncates_results() {
    let (root, _home, _p) = boot_tool_batch().await;
    let out = batch(
        &root,
        json!({
            "steps": [
                {"tool": "bash", "args": {"command": "head -c 100 /dev/zero | tr '\\0' x"}, "save_as": "big"},
                {"tool": "bash", "args": {"command": "echo '{\"list\":[\"aaaaaaaaaaaa\",\"bbbbbbbbbbbb\"]}'"}, "save_as": "obj"}
            ],
            "return": ["big", "obj"],
            "max_chars": 10
        }),
    )
    .await;
    let v = batch_json(&out);
    assert_eq!(v["ok"], true, "{out}");
    assert_eq!(
        v["results"]["big"], "xxxxxxxxxx…[truncated 90 chars]",
        "{out}"
    );
    // 对象超长时转成 JSON 文本再截。
    let obj = v["results"]["obj"].as_str().unwrap();
    assert!(
        obj.starts_with("{\"list\":[\"") && obj.contains("…[truncated"),
        "{out}"
    );
    // 默认 4000：短结果原样。
    let out = batch(
        &root,
        json!({"steps": [{"tool": "bash", "args": {"command": "echo short"}}]}),
    )
    .await;
    assert!(
        batch_json(&out)["results"]["step0"]
            .as_str()
            .unwrap()
            .contains("short"),
        "{out}"
    );
    println!("trunc: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn example_tool_batch_rejects_bad_batches_before_running() {
    let (root, home, _p) = boot_tool_batch().await;
    let marker = home.path().join("should-not-exist");
    let touch = json!({"tool": "bash", "args": {"command": format!("touch {}", marker.display())}});
    let steps: Vec<_> = (0..51).map(|_| touch.clone()).collect();
    let out = batch(&root, json!({ "steps": steps })).await;
    assert!(out.starts_with("Error: tool_batch rejected"), "{out}");
    assert!(out.contains("too many calls: 51 > 50"), "{out}");
    assert!(!marker.exists(), "一步都不该跑");

    // 50 步正好放行。
    let steps: Vec<_> = (0..50)
        .map(|i| json!({"tool": "bash", "args": {"command": format!("echo {i}")}}))
        .collect();
    let out = batch(&root, json!({ "steps": steps })).await;
    let v = batch_json(&out);
    assert_eq!(v["completed"], 50, "{out}");

    for (bad, want) in [
        (json!({"steps": []}), "steps is empty"),
        (json!({}), "steps must be an array"),
        (json!({"steps": [{"args": {}}]}), "step 0: missing tool"),
        (
            json!({"steps": [touch.clone(), {"tool": "tool_batch", "args": {}}]}),
            "cannot call itself",
        ),
        (
            json!({"steps": [{"tool": "bash", "args": {"command": "true"}, "on_error": "retry"}]}),
            "on_error must be",
        ),
        (
            json!({"steps": [{"tool": "bash", "args": {"command": "true"}, "save_as": "a.b"}]}),
            "save_as must be",
        ),
        // 跑之前就按 schema 校验：不存在的工具、缺必填参数。
        (
            json!({"steps": [touch.clone(), {"tool": "no_such_tool"}]}),
            "step 1: unknown tool \\\"no_such_tool\\\"",
        ),
        (
            json!({"steps": [{"tool": "use_tool", "args": {"tool_name": "tool_batch"}}]}),
            "cannot call itself",
        ),
        (
            json!({"steps": [touch.clone(), {"tool": "bash", "args": {}}]}),
            "step 1: bash needs argument \\\"command\\\"",
        ),
        (
            json!({"steps": [{"parallel": [touch.clone(), {"parallel": []}]}]}),
            "parallel groups cannot nest",
        ),
        (
            json!({"steps": [{"parallel": []}]}),
            "parallel must be a non-empty array",
        ),
        (
            json!({"steps": [{"tool": "bash", "args": {"command": "true"}, "timeout_ms": 0}]}),
            "timeout_ms must be",
        ),
        (
            json!({"steps": [touch.clone()], "max_chars": 0}),
            "max_chars must be",
        ),
    ] {
        let out = batch(&root, bad).await;
        assert!(out.starts_with("Error: tool_batch rejected"), "{out}");
        assert!(out.contains(want), "{want}: {out}");
    }
    assert!(!marker.exists(), "校验失败的批次一步都不该跑");
}

/// 批里的每一次调用照样单独过权限门：逐条入审批队列；拒绝（权限被拒绝）按失败处理、默认停。
/// 这里从根 ctx 调，请求方就是主会话（agent_id None）；子代理身份见下一条。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn example_tool_batch_each_call_goes_through_the_permission_gate() {
    let (root, home, _p) = boot_tool_batch().await;
    root.get::<cordis_spine::AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Ask);
    let perms = root
        .require::<cordis_spine::Permissions>(cordis_spine::PERMISSIONS)
        .unwrap();
    let marker = home.path().join("third");
    let r2 = root.clone();
    let m2 = marker.clone();
    let run = tokio::spawn(async move {
        batch(
            &r2,
            json!({ "steps": [
                {"tool": "bash", "args": {"command": "echo one"}},
                {"tool": "bash", "args": {"command": "echo two"}},
                {"tool": "bash", "args": {"command": format!("touch {}", m2.display())}}
            ]}),
        )
        .await
    });
    let mut prompts = Vec::new();
    for kind in [
        cordis_base::acp::PermissionOptionKind::AllowOnce,
        cordis_base::acp::PermissionOptionKind::RejectOnce,
    ] {
        let front = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(f) = perms.front() {
                    break f;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("批里的 bash 该排进审批队列");
        prompts.push(front.clone());
        assert!(perms.resolve(kind));
    }
    let out = tokio::time::timeout(std::time::Duration::from_secs(10), run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts
            .iter()
            .all(|p| p.tool == "bash" && p.agent_id.is_none()),
        "{prompts:?}"
    );
    assert!(prompts[0].summary.contains("echo one"), "{prompts:?}");
    assert!(
        out.starts_with("Error: tool_batch stopped at step 1"),
        "{out}"
    );
    assert!(out.contains("权限被拒绝"), "{out}");
    assert!(perms.front().is_none());
    assert!(!marker.exists(), "拒绝后第三步不该跑");
}

/// 批里的工作区工具按**调用方那一页**的工作目录解析相对路径、起 bash：GUI 一个进程挂多页，
/// 进程 cwd 不是项目目录。以前插件经 host 调进来时丢了 `EXEC_CTX`，`read_file("x")` 读的是进程 cwd。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plugin_call_tool_uses_the_callers_cwd() {
    let (root, _home, _p) = boot_tool_batch().await;
    let tools = tools_of(&root);
    let proj = tempfile::tempdir().unwrap();
    let proj_path = proj.path().canonicalize().unwrap();
    std::fs::write(proj_path.join("only-here.txt"), "marker-7f3a").unwrap();
    assert_ne!(std::env::current_dir().unwrap(), proj_path);

    let page = root.isolate("sessions");
    let sessions = Sessions::isolated_as(page.clone(), "page-cwd");
    sessions.pin_workspace_cwd(&proj_path);
    let _s = page.provide(SESSIONS, sessions).unwrap();
    let out = tools
        .execute_on(
            &page,
            ToolCall {
                id: "tb".into(),
                name: "tool_batch".into(),
                arguments: json!({
                    "steps": [{"parallel": [
                        {"tool": "read_file", "args": {"target_file": "only-here.txt"}, "save_as": "f"},
                        {"tool": "bash", "args": {"command": "pwd -P"}, "save_as": "d"}
                    ]}],
                    "return": ["f", "d"]
                })
                .to_string(),
            },
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("marker-7f3a"), "{}", out.content);
    assert!(
        out.content.contains(&*proj_path.to_string_lossy()),
        "{}",
        out.content
    );
}

/// 批里的 `write_file` / `search_replace` 要记进这一轮的改动（能撤销、GUI 改动面板看得见）。
/// 记录挂在 `tools/pre-execute` / `tools/execute` 上，靠 `exec_ctx()` 找到这一页；插件经 host
/// 调进来时跑在阻塞线程上，以前那两处 waterfall 拿不到 ctx，直接跳过。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plugin_call_tool_edits_land_in_turn_changes() {
    let (root, _home, _p) = boot_tool_batch().await;
    root.plugin(cordis_spine::turn_changes(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let tools = tools_of(&root);
    let proj = tempfile::tempdir().unwrap();
    let proj_path = proj.path().canonicalize().unwrap();
    std::fs::write(proj_path.join("a.txt"), "one\ntwo\n").unwrap();
    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    assert!(sessions.is_main());
    sessions.pin_workspace_cwd(&proj_path);
    sessions.append(LogEvent::User("改两个文件".into()));

    let out = tools
        .execute_on(
            &root,
            ToolCall {
                id: "tb".into(),
                name: "tool_batch".into(),
                arguments: json!({
                    "steps": [
                        {"tool": "write_file", "args": {"target_file": "notes/new.md", "contents": "x\ny\n"}},
                        {"tool": "search_replace", "args": {"file_path": "a.txt", "old_string": "two", "new_string": "TWO"}}
                    ]
                })
                .to_string(),
            },
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(
        std::fs::read_to_string(proj_path.join("notes/new.md")).unwrap(),
        "x\ny\n"
    );

    let changes = root
        .require::<cordis_spine::TurnChanges>(cordis_spine::TURN_CHANGES)
        .unwrap();
    let turns = changes.list(&root);
    assert_eq!(turns.len(), 1, "{turns:#?}");
    let files: Vec<(&str, &str)> = turns[0]
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.status))
        .collect();
    assert_eq!(
        files,
        vec![("notes/new.md", "added"), ("a.txt", "modified")]
    );
}

/// 插件工具里的 host.call_tool 以**调用方**身份执行：子代理调 tool_batch 时，批里的审批
/// 记在子代理名下；调用方那一页开着计划模式时，批里的 bash 被计划门挡住；调用方这一轮
/// 被停止时，正在跑的 bash 被打断。以前一律走插件的根 ctx，这三样都不认。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plugin_call_tool_runs_as_the_caller() {
    let (root, home, _p) = boot_tool_batch().await;
    let tools = tools_of(&root);
    let call = |args: serde_json::Value| ToolCall {
        id: "tb".into(),
        name: "tool_batch".into(),
        arguments: args.to_string(),
    };
    let marker = home.path().join("plan-blocked");
    let touch = json!({"steps": [{"tool": "bash", "args": {"command": format!("touch {}", marker.display())}}]});

    // 计划模式挂在调用方那一页上（根上没有）：批里的写操作被挡。
    let page = root.isolate("sessions").isolate(cordis_spine::PLAN_MODE);
    let _s = page
        .provide(SESSIONS, Sessions::isolated_as(page.clone(), "child-plan"))
        .unwrap();
    let plan = cordis_spine::PlanMode::new(page.clone());
    plan.set(true);
    let _p2 = page.provide(cordis_spine::PLAN_MODE, plan).unwrap();
    let out = tools.execute_on(&page, call(touch.clone())).await.content;
    assert!(
        out.contains("计划模式已阻止") || out.contains("只读模式"),
        "{out}"
    );
    assert!(!marker.exists());

    // 子代理身份：审批请求记在 child-7 名下。
    root.get::<cordis_spine::AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Ask);
    let perms = root
        .require::<cordis_spine::Permissions>(cordis_spine::PERMISSIONS)
        .unwrap();
    let child = root.isolate("sessions");
    let _c = child
        .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-7"))
        .unwrap();
    let (t2, c2, a2) = (tools.clone(), child.clone(), call(touch.clone()));
    let run = tokio::spawn(async move { t2.execute_on(&c2, a2).await.content });
    let front = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(f) = perms.front() {
                break f;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(front.agent_id.as_deref(), Some("child-7"), "{front:?}");
    assert!(perms.resolve(cordis_base::acp::PermissionOptionKind::RejectOnce));
    let out = run.await.unwrap();
    assert!(out.contains("权限被拒绝"), "{out}");
    root.get::<cordis_spine::AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Allow);

    // 停止：调用方这一轮的 TurnControl 取消后，批里正在跑的 bash 被打断。
    let turn_page = root.isolate("sessions").isolate(cordis_spine::TURN);
    let _s3 = turn_page
        .provide(
            SESSIONS,
            Sessions::isolated_as(turn_page.clone(), "child-stop"),
        )
        .unwrap();
    let _t = turn_page
        .provide(cordis_spine::TURN, cordis_spine::TurnControl::new())
        .unwrap();
    let (t3, c3) = (tools.clone(), turn_page.clone());
    let started = std::time::Instant::now();
    let run = tokio::spawn(async move {
        t3.execute_on(
            &c3,
            ToolCall {
                id: "tb".into(),
                name: "tool_batch".into(),
                arguments: json!({"steps": [
                    {"tool": "bash", "args": {"command": "sleep 20"}},
                    {"tool": "bash", "args": {"command": "echo never"}}
                ]})
                .to_string(),
            },
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    turn_page
        .get::<cordis_spine::TurnControl>(cordis_spine::TURN)
        .unwrap()
        .cancel();
    let out = tokio::time::timeout(std::time::Duration::from_secs(10), run)
        .await
        .expect("停止后批次该很快返回")
        .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("cancelled"), "{}", out.content);
    assert!(!out.content.contains("never"), "{}", out.content);
}

/// 只登记一颗 `probe` 工具的磁盘插件，`execute_body` 是它的 execute 体（`args` 是参数 map），
/// 用来直接试 host 能力。`toml_extra` 追加到 plugin.toml（如 `allow_eval = true`）。
async fn boot_probe(
    execute_body: &str,
    toml_extra: &str,
) -> (Context, tempfile::TempDir, tempfile::TempDir) {
    let toml = format!(
        "name = \"probe\"\npurpose = \"host api probe\"\nfactory = \"rhai\"\nenabled = true\n{toml_extra}\n"
    );
    let source = format!(
        r#"#{{
    inject: ["tools"],
    apply: |host| {{
        host.register_tool(#{{
            name: "probe",
            description: "probe",
            parameters: #{{ type: "object", properties: #{{}} }},
            execute: |args| {{ {execute_body} }}
        }});
    }}
}}"#
    );
    boot_disk_plugin("probe", &[("plugin.toml", toml), ("source.rhai", source)]).await
}

/// 登记一颗按名字算只读的慢工具（`memory_search` / `memory_get` 是 `Class::Read`），
/// 用来测 `call_tools_parallel` 的并发分批。`counters` = (正在跑, 峰值)，几颗工具共用一对。
/// 返回的 Disposable 必须留着，否则会卸掉。
fn register_slow_read(
    root: &Context,
    name: &str,
    delay: Duration,
    counters: &(Arc<AtomicUsize>, Arc<AtomicUsize>),
    content: &str,
) -> cordis::Disposable {
    let (running, peak) = counters.clone();
    let content = content.to_string();
    let body: ToolBody = Arc::new(move |call| {
        let peak = peak.clone();
        let running = running.clone();
        let delay = delay;
        let content = content.clone();
        Box::pin(async move {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            running.fetch_sub(1, Ordering::SeqCst);
            ToolResult {
                call_id: call.id,
                name: call.name,
                content,
                ..Default::default()
            }
        })
    });
    tools_of(root)
        .register(
            ToolSpec {
                name: name.into(),
                description: "slow read probe".into(),
                parameters_json: r#"{"type":"object","properties":{}}"#.into(),
            },
            body,
        )
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_call_tool_rejects_unknown_tools() {
    let (root, _h, _p) = boot_probe(
        r#"host.call_tool("no_such_tool", #{ x: 1 }) + "|" + host.call_tool("bash", #{ command: "echo hi" })"#,
        "",
    )
    .await;
    let out = exec(&root, "probe", "{}").await;
    let (unknown, known) = out.split_once('|').unwrap();
    assert_eq!(unknown, "Error: unknown tool `no_such_tool`", "{out}");
    assert!(known.contains("hi"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_call_tool_ex_reports_structured_results() {
    let (root, _h, _p) = boot_probe(
        r#"let a = host.call_tool_ex("bash", #{ command: "echo ok" });
           let b = host.call_tool_ex("bash", #{ command: "exit 3" });
           let c = host.call_tool_ex("nope", #{});
           #{ is_error: false, content: [a, b, c] }"#,
        "",
    )
    .await;
    let r = tools_of(&root)
        .execute(ToolCall {
            id: "t".into(),
            name: "probe".into(),
            arguments: "{}".into(),
        })
        .await;
    assert!(!r.is_error, "{}", r.content);
    let v: serde_json::Value = serde_json::from_str(&r.content).unwrap();
    assert_eq!(v[0]["ok"], true, "{v}");
    assert_eq!(v[0]["is_error"], false);
    assert_eq!(v[0]["tool"], "bash");
    assert_eq!(v[0]["images"], 0);
    assert!(v[0]["text"].as_str().unwrap().contains("ok"));
    assert_eq!(v[1]["is_error"], true, "{v}");
    assert_eq!(v[2]["is_error"], true);
    assert_eq!(v[2]["text"], "Error: unknown tool `nope`");
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_tool_can_return_is_error() {
    let (root, _h, _p) = boot_probe(
        r#"if args.fail == true { #{ is_error: true, content: "boom" } } else { #{ x: 1 } }"#,
        "",
    )
    .await;
    let run = |args: &str| {
        let tools = tools_of(&root);
        let args = args.to_string();
        async move {
            tools
                .execute(ToolCall {
                    id: "t".into(),
                    name: "probe".into(),
                    arguments: args,
                })
                .await
        }
    };
    let failed = run(r#"{"fail":true}"#).await;
    assert!(failed.is_error);
    assert_eq!(failed.content, "boom");
    // 没有 is_error 键的 map 照旧当 JSON 文本、算成功（向后兼容）。
    let plain = run("{}").await;
    assert!(!plain.is_error);
    assert_eq!(plain.content, r#"{"x":1}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_lists_tools_and_reads_schemas() {
    let (root, _h, _p) = boot_probe(
        r#"let s = host.tool_schema("bash");
           let names = host.list_tools().map(|t| t.name);
           #{ is_error: false, content: #{
               has_bash: names.contains("bash"),
               schema_name: s.name,
               requires_command: s.parameters.required.contains("command"),
               missing: host.tool_schema("nope") == ()
           } }"#,
        "",
    )
    .await;
    let out = exec(&root, "probe", "{}").await;
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v,
        json!({"has_bash": true, "schema_name": "bash", "requires_command": true, "missing": true}),
        "{out}"
    );

    // 计划模式：计划门管的写工具标 plan_blocked（不剔掉——只读 bash、写计划文件仍放行）。
    let (root2, _h2, _p3) = boot_probe(
        r#"let write = host.tool_schema("write_file");
           let read = host.tool_schema("read_file");
           #{ is_error: false, content: #{
               write_blocked: write.plan_blocked,
               write_listed_blocked: host.list_tools().filter(|t| t.name == "write_file")[0].plan_blocked,
               read_blocked: read.plan_blocked,
               read_name: read.name
           } }"#,
        "",
    )
    .await;
    // 用带计划模式的调用方 ctx 跑这颗 probe。
    let tools = tools_of(&root2);
    // 计划模式要挂在 execute 的调用方上；把同一套 plan 挂到 root2 对应的页再调。
    let page2 = root2.isolate("sessions").isolate(cordis_spine::PLAN_MODE);
    let _s2 = page2
        .provide(
            SESSIONS,
            Sessions::isolated_as(page2.clone(), "schema-plan-2"),
        )
        .unwrap();
    let plan2 = cordis_spine::PlanMode::new(page2.clone());
    plan2.set(true);
    let _pp = page2.provide(cordis_spine::PLAN_MODE, plan2).unwrap();
    let out = tools
        .execute_on(
            &page2,
            ToolCall {
                id: "t".into(),
                name: "probe".into(),
                arguments: "{}".into(),
            },
        )
        .await
        .content;
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["write_blocked"], true, "{out}");
    assert_eq!(v["write_listed_blocked"], true, "{out}");
    assert_eq!(v["read_blocked"], false, "{out}");
    assert_eq!(v["read_name"], "read_file", "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_call_tools_parallel_runs_concurrently_in_order() {
    let (root, _h, _p) = boot_probe(
        r#"let rs = host.call_tools_parallel([
               #{ tool: "memory_search" },
               #{ tool: "memory_get" },
               #{ tool: "nope" },
               #{ tool: "memory_search", timeout_ms: 300 }
           ]);
           #{ is_error: false, content: rs }"#,
        "",
    )
    .await;
    let peak = Arc::new(AtomicUsize::new(0));
    let counters = (Arc::new(AtomicUsize::new(0)), peak.clone());
    let _k1 = register_slow_read(
        &root,
        "memory_search",
        Duration::from_millis(1000),
        &counters,
        "slow-ok",
    );
    let _k2 = register_slow_read(
        &root,
        "memory_get",
        Duration::from_millis(1000),
        &counters,
        "slow-ok",
    );
    let started = std::time::Instant::now();
    let out = exec(&root, "probe", "{}").await;
    let elapsed = started.elapsed();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v[0]["text"].as_str().unwrap().contains("slow-ok"), "{out}");
    assert!(v[1]["text"].as_str().unwrap().contains("slow-ok"), "{out}");
    assert_eq!(v[2]["text"], "Error: unknown tool `nope`");
    assert_eq!(v[3]["is_error"], true);
    assert!(
        v[3]["text"].as_str().unwrap().contains("timed out"),
        "{out}"
    );
    // 两颗只读慢工具并行：远小于串行的 2s；第 4 颗 300ms 超时。
    assert!(elapsed < Duration::from_millis(1900), "{elapsed:?}");
    assert!(
        peak.load(Ordering::SeqCst) >= 2,
        "相邻只读调用应该叠在一起跑，peak={}",
        peak.load(Ordering::SeqCst)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_call_tools_parallel_serializes_unsafe_calls() {
    let (root, _h, _p) = boot_probe(
        r#"let rs = host.call_tools_parallel([
               #{ tool: "bash", args: #{ command: "sleep 1; echo one" } },
               #{ tool: "bash", args: #{ command: "sleep 1; echo two" } }
           ]);
           #{ is_error: false, content: rs }"#,
        "",
    )
    .await;
    let started = std::time::Instant::now();
    let out = exec(&root, "probe", "{}").await;
    let elapsed = started.elapsed();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v[0]["text"].as_str().unwrap().contains("one"), "{out}");
    assert!(v[1]["text"].as_str().unwrap().contains("two"), "{out}");
    // sleep 不是只读命令：必须按顺序跑，不能把 write/click 这类和别的叠一起。
    assert!(
        elapsed >= Duration::from_millis(1800),
        "unsafe calls must run one at a time, got {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_call_tools_parallel_stops_mid_batch() {
    let (root, _h, _p) = boot_probe(
        r#"let rs = host.call_tools_parallel([
               #{ tool: "memory_search" },
               #{ tool: "memory_get" }
           ]);
           #{ is_error: false, content: rs }"#,
        "",
    )
    .await;
    let peak = Arc::new(AtomicUsize::new(0));
    let counters = (Arc::new(AtomicUsize::new(0)), peak.clone());
    let _k1 = register_slow_read(
        &root,
        "memory_search",
        Duration::from_secs(5),
        &counters,
        "late",
    );
    let _k2 = register_slow_read(
        &root,
        "memory_get",
        Duration::from_secs(5),
        &counters,
        "late",
    );
    let page = root.isolate("sessions").isolate(cordis_spine::TURN);
    let _s = page
        .provide(SESSIONS, Sessions::isolated_as(page.clone(), "stop-par"))
        .unwrap();
    let _t = page
        .provide(cordis_spine::TURN, cordis_spine::TurnControl::new())
        .unwrap();
    let tools = tools_of(&root);
    let started = std::time::Instant::now();
    let run = tokio::spawn({
        let tools = tools.clone();
        let page = page.clone();
        async move {
            tools
                .execute_on(
                    &page,
                    ToolCall {
                        id: "t".into(),
                        name: "probe".into(),
                        arguments: "{}".into(),
                    },
                )
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    page.get::<cordis_spine::TurnControl>(cordis_spine::TURN)
        .unwrap()
        .cancel();
    let out = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("Stop should finish the parallel batch quickly")
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    // probe 把 call_tools_parallel 的结果数组原样回出来：每一颗都得是被停掉的错误。
    let results: Vec<serde_json::Value> = serde_json::from_str(&out.content).expect(&out.content);
    assert!(!results.is_empty(), "{}", out.content);
    for r in &results {
        assert_eq!(r["is_error"], true, "{}", out.content);
        assert!(
            r["text"].as_str().unwrap_or_default().contains("cancelled"),
            "{}",
            out.content
        );
    }
    assert!(
        !out.content.contains("late"),
        "stopped calls should not return their success body: {}",
        out.content
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_timeout_or_stop_withdraws_a_pending_approval() {
    let (root, home, _p) = boot_probe(
        r#"host.call_tool_ex("write_file", #{ target_file: args.path, content: "x" }, #{ timeout_ms: 400 })"#,
        "",
    )
    .await;
    root.get::<cordis_spine::AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Ask);
    let perms = root
        .require::<cordis_spine::Permissions>(cordis_spine::PERMISSIONS)
        .unwrap();
    let path = home.path().join("timeout-withdraw.txt");
    let tools = tools_of(&root);
    let run = tokio::spawn({
        let tools = tools.clone();
        let arguments = json!({"path": path}).to_string();
        async move {
            tools
                .execute(ToolCall {
                    id: "t".into(),
                    name: "probe".into(),
                    arguments,
                })
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if perms.front().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("approval should be queued");
    let out = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("timeout should return")
        .unwrap();
    assert!(
        out.content.contains("timed out") || out.is_error,
        "{}",
        out.content
    );
    assert!(
        perms.front().is_none(),
        "timed-out approval must leave the queue, still have {:?}",
        perms.front()
    );
    assert!(!path.exists(), "should not have written after timeout");

    // Stop 丢掉正在等审批的调用，请求同样要撤出队列。
    let (root, home, _p) = boot_probe(
        r#"host.call_tool_ex("write_file", #{ target_file: args.path, content: "x" })"#,
        "",
    )
    .await;
    root.get::<cordis_spine::AppSettings>(SETTINGS)
        .unwrap()
        .set_permission_mode(PermissionMode::Ask);
    let perms = root
        .require::<cordis_spine::Permissions>(cordis_spine::PERMISSIONS)
        .unwrap();
    let path = home.path().join("stop-withdraw.txt");
    let page = root.isolate("sessions").isolate(cordis_spine::TURN);
    let _s = page
        .provide(SESSIONS, Sessions::isolated_as(page.clone(), "stop-w"))
        .unwrap();
    let _t = page
        .provide(cordis_spine::TURN, cordis_spine::TurnControl::new())
        .unwrap();
    let tools = tools_of(&root);
    let run = tokio::spawn({
        let tools = tools.clone();
        let page = page.clone();
        let arguments = json!({"path": path}).to_string();
        async move {
            tools
                .execute_on(
                    &page,
                    ToolCall {
                        id: "t".into(),
                        name: "probe".into(),
                        arguments,
                    },
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if perms.front().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("approval should be queued before Stop");
    page.get::<cordis_spine::TurnControl>(cordis_spine::TURN)
        .unwrap()
        .cancel();
    let out = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("Stop should drop the waiting call")
        .unwrap();
    assert!(
        out.content.contains("cancelled") && out.is_error,
        "{}",
        out.content
    );
    // 等 Drop / Withdraw 跑完
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if perms.front().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("stopped approval must leave the queue");
    assert!(!path.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_call_tools_parallel_rejects_bad_input() {
    let (root, _h, _p) = boot_probe(
        r#"let many = [];
           for i in 0..17 { many.push(#{ tool: "bash", args: #{ command: "true" } }); }
           let a = ""; let b = "";
           try { host.call_tools_parallel(many); } catch (e) { a = `${e}`; }
           try { host.call_tools_parallel([#{ args: #{} }]); } catch (e) { b = `${e}`; }
           let c = "";
           try { host.call_tool_ex("bash", #{ command: "true" }, #{ timeout_ms: 600001 }); } catch (e) { c = `${e}`; }
           a + "|" + b + "|" + c"#,
        "",
    )
    .await;
    let out = exec(&root, "probe", "{}").await;
    assert!(out.contains("at most 16"), "{out}");
    assert!(out.contains("needs tool"), "{out}");
    assert!(out.contains("at most 600000"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_json_round_trips_any_value() {
    let (root, _h, _p) = boot_probe(
        r#"let arr = parse_json("[1, \"a\", {\"b\": [true, null]}]");
           let num = parse_json("42");
           let s = parse_json("\"x\\ny\"");
           let out = [
               arr[2].b[0],
               num + 1,
               s,
               to_json([1, "a\tb", #{ k: "q\"uote" }]),
               to_json("line\nbreak"),
               to_json(3),
               #{ m: [1, 2] }.to_json(),
               [#{ z: () }].to_json()
           ];
           out.map(|v| `${v}`).reduce(|acc, v| acc + "\u0001" + v, "")"#,
        "",
    )
    .await;
    let out = exec(&root, "probe", "{}").await;
    let parts: Vec<&str> = out.split('\u{1}').skip(1).collect();
    assert_eq!(
        parts,
        vec![
            "true",
            "43",
            "x\ny",
            r#"[1,"a\tb",{"k":"q\"uote"}]"#,
            r#""line\nbreak""#,
            "3",
            r#"{"m":[1,2]}"#,
            r#"[{"z":null}]"#,
        ],
        "{out}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_eval_expr_is_opt_in_and_expression_only() {
    let body = r#"let r = [];
        r.push(`${host.eval_expr("prev.count > 2 && name == \"x\"", #{ prev: #{ count: 3 }, name: "x" })}`);
        r.push(`${host.eval_expr("[1, 2, 3].len() * 2")}`);
        for e in ["let a = 1; a", "host.log(\"x\")", "loop { }", "fn f() { 1 }"] {
            try { r.push(`${host.eval_expr(e)}`); } catch (err) { r.push("ERR " + `${err}`); }
        }
        r.reduce(|acc, v| acc + "\u0001" + v, "")"#;
    let (root, _h, _p) = boot_probe(body, "allow_eval = true").await;
    let out = exec(&root, "probe", "{}").await;
    let parts: Vec<&str> = out.split('\u{1}').skip(1).collect();
    assert_eq!(parts[0], "true", "{out}");
    assert_eq!(parts[1], "6", "{out}");
    // 语句、host、死循环、定义函数都过不去，而且要报出对应的原因（不只是 ERR 前缀）。
    assert_eq!(parts.len(), 6, "{out}");
    for (part, needle) in [
        (parts[2], "let"),
        (parts[3], "host"),
        (parts[4], "loop"),
        (parts[5], "fn"),
    ] {
        assert!(part.starts_with("ERR "), "{out}");
        assert!(
            part.to_lowercase().contains(needle),
            "expected {needle:?} in {part:?} ({out})"
        );
    }

    let (root, _h, _p) = boot_probe(body, "").await;
    let out = exec(&root, "probe", "{}").await;
    assert!(
        out.contains("eval_expr is off") && out.contains("cordis_define"),
        "{out}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_eval_expr_rejects_function_values_at_parse() {
    // 审查复现：七层嵌套、一个变量都不读的闭包，267 字节跑 24.6s，操作数上限和停止都拦不住。
    // 表达式引擎不许匿名函数、也关了 Fn / call / curry：解析时就拒，不会开始跑。
    // 取字段、过滤用原生 pluck / filter_eq。`sleep` 与「字符串函数名回调」（大数组上）也要很快报错。
    // 操作数上限本身见 rhai_host 的单测。
    let nested = "[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15].map(|x| [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15].map(|x1| [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15].map(|x2| [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15].map(|x3| 1).len()).len()).len())";
    let body = format!(
        r#"let r = [];
        for e in [{nested:?}, "[1].map(Fn(\"len\"))", "Fn(\"len\").curry(1)", "sleep(1000000000)", "blob(16000000).to_array().map(\"is_even\").len()"] {{
            let started = timestamp();
            let s = "";
            try {{ s = `${{host.eval_expr(e)}}`; }} catch (err) {{ s = "ERR " + `${{err}}`; }}
            r.push(s + " @" + `${{started.elapsed}}`);
        }}
        let items = #{{ items: [#{{ id: 1, state: "open" }}, #{{ id: 2, state: "done" }}, #{{ id: 3, state: "open" }}] }};
        r.push(`${{host.eval_expr("result.items.pluck(\"id\")", #{{ result: items }})}}`);
        r.push(`${{host.eval_expr("filter_eq(result.items, \"state\", \"open\").pluck(\"id\")", #{{ result: items }})}}`);
        r.reduce(|acc, v| acc + "\u0001" + v, "")"#
    );
    let (root, _h, _p) = boot_probe(&body, "allow_eval = true").await;
    let out = exec(&root, "probe", "{}").await;
    let parts: Vec<&str> = out.split('\u{1}').skip(1).collect();
    assert_eq!(parts.len(), 7, "{out}");
    for part in &parts[..5] {
        assert!(part.starts_with("ERR "), "{out}");
        assert!(part.contains("eval_expr:"), "{out}");
        let secs: f64 = part.rsplit('@').next().unwrap().trim().parse().unwrap();
        assert!(secs < 1.0, "rejected before running, took {secs}s: {out}");
    }
    assert!(parts[0].contains("Unexpected '|'"), "{out}");
    assert!(parts[1].contains("'Fn' is disabled"), "{out}");
    assert!(parts[3].contains("`sleep` is not available"), "{out}");
    assert!(parts[4].to_lowercase().contains("too large"), "{out}");
    assert_eq!(parts[5], "[1, 2, 3]", "{out}");
    assert_eq!(parts[6], "[1, 3]", "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_eval_expr_stays_off_for_cordis_define_packages() {
    // 即使用 source_path 指到一份写了 allow_eval = true 的 plugin.toml，会话包也开不了。
    let home = tempfile::tempdir().unwrap();
    let _env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path());
    let root = boot().await;
    let plug = home.path().join("plugins/evalsneak");
    std::fs::create_dir_all(&plug).unwrap();
    std::fs::write(
        plug.join("plugin.toml"),
        "name = \"Sneak\"\npurpose = \"should not get eval\"\nfactory = \"rhai\"\nenabled = true\nallow_eval = true\n",
    )
    .unwrap();
    std::fs::write(
        plug.join("source.rhai"),
        r#"#{
            inject: ["tools"],
            apply: |host| {
                host.register_tool(#{
                    name: "eval_sneak",
                    description: "try eval",
                    parameters: #{ type: "object", properties: #{} },
                    execute: |args| {
                        let r = "";
                        try { r = `${host.eval_expr("1 + 1")}`; } catch (e) { r = `${e}`; }
                        r
                    }
                });
            }
        }"#,
    )
    .unwrap();
    let defined = exec_json(
        &root,
        "cordis_define",
        json!({
            "plugin": {"kind": "new", "idPrefix": "esnk"},
            "name": "Sneak",
            "purpose": "should not get eval",
            "factory": "rhai",
            "source_path": plug.join("source.rhai").display().to_string()
        }),
    )
    .await;
    assert!(!defined.contains("Error"), "{defined}");
    let ran = exec(
        &root,
        "cordis_run",
        r#"{"pluginId":"esnk-1","packageId":"pkg-1","mode":"run"}"#,
    )
    .await;
    assert!(ran.contains("\"status\":\"running\""), "{ran}");
    let out = exec(&root, "eval_sneak", "{}").await;
    assert!(
        out.contains("eval_expr is off") && out.contains("cordis_define"),
        "{out}"
    );
    assert!(!out.trim().ends_with('2'), "{out}");
}

/// 并行分组：组里的调用同时跑，结果存成数组 stepN，各颗 save_as 也能被后面的步骤引用。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn example_tool_batch_parallel_group_runs_concurrently() {
    let (root, _home, _p) = boot_tool_batch().await;
    let peak = Arc::new(AtomicUsize::new(0));
    let counters = (Arc::new(AtomicUsize::new(0)), peak.clone());
    let _k1 = register_slow_read(
        &root,
        "memory_search",
        Duration::from_millis(1000),
        &counters,
        r#"{"n":1}"#,
    );
    let _k2 = register_slow_read(
        &root,
        "memory_get",
        Duration::from_millis(1000),
        &counters,
        r#"{"n":2}"#,
    );
    let _k3 = register_slow_read(
        &root,
        "canvas_read",
        Duration::from_millis(1000),
        &counters,
        "three",
    );
    let started = std::time::Instant::now();
    let out = batch(
        &root,
        json!({
            "steps": [
                {"parallel": [
                    {"tool": "memory_search", "save_as": "a"},
                    {"tool": "memory_get", "save_as": "b"},
                    {"tool": "canvas_read"}
                ]},
                {"tool": "bash", "args": {"command": "echo sum-${a.n}-${b.n}"}, "save_as": "sum", "expect": "sum-1-2"}
            ],
            "return": ["sum", "step0.2", "a"]
        }),
    )
    .await;
    let elapsed = started.elapsed();
    assert!(!out.starts_with("Error"), "{out}");
    let v = batch_json(&out);
    assert_eq!(v["ok"], true, "{out}");
    assert_eq!(v["completed"], 4, "{out}");
    assert!(
        v["results"]["sum"].as_str().unwrap().contains("sum-1-2"),
        "{out}"
    );
    assert!(
        v["results"]["step0.2"].as_str().unwrap().contains("three"),
        "{out}"
    );
    assert_eq!(v["results"]["a"]["n"], 1, "{out}");
    let trace: Vec<String> = v["trace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().to_string())
        .collect();
    assert!(trace[0].starts_with("#0.0 memory_search ok"), "{trace:?}");
    assert_eq!(trace.len(), 4, "{trace:?}");
    // 三颗只读慢工具并行：远小于串行的 3s。
    assert!(elapsed < Duration::from_millis(2500), "{elapsed:?}");
    assert!(
        peak.load(Ordering::SeqCst) >= 3,
        "parallel group should run read-only tools together, peak={}",
        peak.load(Ordering::SeqCst)
    );

    // 组里一颗失败：整组跑完，默认停在这一步，后面的不跑。
    let out = batch(
        &root,
        json!({ "steps": [
            {"parallel": [
                {"tool": "bash", "args": {"command": "exit 3"}},
                {"tool": "bash", "args": {"command": "echo fine"}, "save_as": "fine"}
            ]},
            {"tool": "bash", "args": {"command": "echo never"}}
        ], "return": ["fine"]}),
    )
    .await;
    assert!(
        out.starts_with("Error: tool_batch stopped at step 0"),
        "{out}"
    );
    let v = batch_json(&out);
    assert!(
        v["error"].as_str().unwrap().starts_with("step 0.0 (bash)"),
        "{out}"
    );
    assert_eq!(v["completed"], 1, "{out}");
    assert!(
        v["results"]["fine"].as_str().unwrap().contains("fine"),
        "{out}"
    );
    assert_eq!(v["trace"].as_array().unwrap().len(), 2, "{out}");
}

/// when / transform（plugin.toml 开了 allow_eval）与单步 timeout_ms。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn example_tool_batch_when_transform_and_timeout() {
    let (root, home, _p) = boot_tool_batch().await;
    let marker = home.path().join("skipped");
    let out = batch(
        &root,
        json!({
            "steps": [
                {"tool": "bash", "args": {"command": "echo '{\"items\":[{\"id\":7},{\"id\":9}]}'"},
                 "save_as": "ids", "transform": "result.items.pluck(\"id\")"},
                {"tool": "bash", "args": {"command": format!("touch {}", marker.display())},
                 "when": "ids.len() > 5"},
                {"tool": "bash", "args": {"command": "echo last=${ids.-1}"}, "when": "ids[0] == 7",
                 "save_as": "last"}
            ],
            "return": ["ids", "last"]
        }),
    )
    .await;
    let v = batch_json(&out);
    assert_eq!(v["ok"], true, "{out}");
    assert_eq!(v["completed"], 2, "{out}");
    assert_eq!(v["skipped"], 1, "{out}");
    assert_eq!(v["results"]["ids"], json!([7, 9]), "{out}");
    assert!(
        v["results"]["last"].as_str().unwrap().contains("last=9"),
        "{out}"
    );
    assert!(!marker.exists(), "when 为 false 的步骤不该跑");

    // when 不是布尔、表达式写错：批次本身写错了，停。
    let out = batch(
        &root,
        json!({"steps": [{"tool": "bash", "args": {"command": "echo x"}, "when": "1 + 1"}]}),
    )
    .await;
    assert!(
        out.starts_with("Error: tool_batch stopped at step 0"),
        "{out}"
    );
    assert!(out.contains("when must evaluate to true/false"), "{out}");

    // 单步时限：到点算失败。
    let started = std::time::Instant::now();
    let out = batch(
        &root,
        json!({"steps": [{"tool": "bash", "args": {"command": "sleep 10"}, "timeout_ms": 300}]}),
    )
    .await;
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(
        out.starts_with("Error: tool_batch stopped at step 0"),
        "{out}"
    );
    assert!(out.contains("timed out"), "{out}");
}
