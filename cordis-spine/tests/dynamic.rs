use cordis::Context;
use cordis_spine::{
    agent_loop, agent_presets, dynamic_runner, llm, permissions, settings, slash, tool_cordis,
    tool_todo, tools, tui_slots, AgentPresets, DynEcho, DynamicRunner, LlmConfig, LlmMode,
    LogEvent, LoopHandle, PermissionMode, PersistScope, PluginOrigin, PreStep, RhaiBag, RunMode,
    Sessions, Slash, ToolCall, Tools, TurnOutcome, AGENT_LOOP, AGENT_PRESETS, CORDIS_PRESET_ID,
    DYNAMIC_CORDIS_RUNNER, DYN_ECHO, DYN_ECHO_TOOL, PRE_STEP, SESSIONS, SETTINGS, SLASH, TOOLS,
    TUI_SLOTS,
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
    assert!(
        services.contains("register / register_dynamic"),
        "{services}"
    );
    assert!(services.contains("cannot replace /agents"), "{services}");
    assert!(services.contains("request_open"), "{services}");
    let tools_only = exec(&root, "cordis_inspect", r#"{"what":"tools"}"#).await;
    assert!(
        !tools_only.contains("register / register_dynamic"),
        "{tools_only}"
    );

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
async fn update_mode_and_undefine() {
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
    let gone = exec(&root, "cordis_undefine", r#"{"pluginId":"note-1"}"#).await;
    assert!(gone.contains("Removed"), "{gone}");
    let listed = exec(&root, "cordis_inspect_self", "{}").await;
    assert!(
        listed.contains("No dynamic Plugins") || !listed.contains("note-1"),
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
            execute: |args| { "memo-ok" }
        });
        host.register_slot(#{
            id: "memo",
            title: "便签",
            hud: true,
            render: || { "hello slot" }
        });
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
    let listed = exec(&root, "cordis_inspect_self", "{}").await;
    assert!(
        listed.contains("No dynamic Plugins") || !listed.contains("bad-1"),
        "{listed}"
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
        "cordis_inspect_self",
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
        "cordis_inspect_self",
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

    exec(&root, "cordis_stop", r#"{"pluginId":"memo-1"}"#).await;
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
    let root = boot().await;
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

    let model: Vec<_> = tools_of(&root)
        .specs_for_model()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        model.iter().any(|n| n == DYN_ECHO_TOOL),
        "dynamic tool must be visible to the sampler: {model:?}"
    );
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
            command: "agents",
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

    exec(&root, "cordis_undefine", r#"{"pluginId":"orb-1"}"#).await;
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
    let listed = exec(&root, "cordis_inspect_self", "{}").await;
    assert!(
        listed.contains("No dynamic Plugins") || !listed.contains("noap-1"),
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

    let gone = exec(&root, "cordis_undefine", r#"{"pluginId":"echo"}"#).await;
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
