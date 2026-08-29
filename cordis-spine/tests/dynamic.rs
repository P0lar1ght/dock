use cordis::Context;
use cordis_spine::{
    agent_presets, dynamic_runner, permissions, settings, slash, tool_cordis, tools, tui_slots,
    AgentPresets, DynEcho, DynamicRunner, LogEvent, PermissionMode, PreStep, RhaiBag, RunMode,
    Sessions, Slash, ToolCall, Tools, AGENT_PRESETS, CORDIS_PRESET_ID, DYNAMIC_CORDIS_RUNNER,
    DYN_ECHO, DYN_ECHO_TOOL, PRE_STEP, SESSIONS, SETTINGS, SLASH, TOOLS, TUI_SLOTS,
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
    root.waterfall(
        PRE_STEP,
        PreStep {
            user: user.into(),
            enter: true,
        },
        || PreStep {
            user: user.into(),
            enter: true,
        },
    );
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
