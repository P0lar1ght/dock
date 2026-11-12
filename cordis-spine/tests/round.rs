use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cordis::{plugin, Context, FiberState, Inject};
use cordis_spine::{
    agent_loop, install_fakes, install_without_llm, tool_goal, tool_todo, turn, Agent, Agents,
    BoxFuture, Driver, Error, Goal, Llm, LlmOutput, LoopHandle, PreStep, PromptRequest, Sampler,
    Sessions, StepStart, StreamDelta, ToolCall, TurnControl, TurnEnd, TurnOutcome, AGENTS,
    AGENT_LOOP, GOAL, LLM, ORDER_TURN_END_TODO, PRE_STEP, SESSIONS, STEP_START, SYSTEM_PROMPT,
    TOOLS, TURN, TURN_END,
};

/// 整个测试二进制共用一个隔离的 `DOCK_HOME`：第一次调用时建临时目录并设好，
/// 进程结束前既不还原也不删（`keep()`）。
///
/// 这样既能挡掉真实 `~/.dock` 里的 `roster.yml`（否则默认 code preset 里的
/// `enter_plan_mode` / `task` 会被过滤掉），也不需要任何跨 `await` 持有的锁 ——
/// `std::sync::Mutex` 的 guard 一旦跨 await，clippy 报 `await_holding_lock`，
/// 运行期还可能把执行器卡住。
fn isolated_home() -> &'static std::path::Path {
    static HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep();
        std::env::set_var("DOCK_HOME", &path);
        // 内置 cua-driver 行是「发现得到就注入」：开发机上装了 driver 的话，
        // 挂 `mcp-client` 会真的把守护进程拉起来。测试不碰本机 driver。
        std::env::set_var("DOCK_CUA_DRIVER", "off");
        path
    })
}

async fn boot() -> Context {
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root
}

#[tokio::test]
async fn loop_waits_for_all_five() {
    isolated_home();
    let root = Context::new();
    let fiber = root.plugin(agent_loop(), ()).unwrap();
    assert_eq!(fiber.state(), FiberState::Pending);
    install_fakes(&root).await.unwrap();
    fiber.wait().await.unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
}

#[tokio::test]
async fn one_round_writes_log_in_order() {
    isolated_home();
    let root = boot().await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("hello")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("echoed: hello".into()));

    let kinds = root.require::<Sessions>(SESSIONS).unwrap().kinds();
    assert_eq!(
        kinds,
        [
            "user",
            "pre-step",
            "prompt",
            "llm/stream",
            "tools/execute",
            "llm/stream",
            "turn-end"
        ]
    );

    let events = root.require::<Sessions>(SESSIONS).unwrap().events();
    match &events[4] {
        cordis_spine::LogEvent::ToolExecute { name, content, .. } => {
            assert_eq!(name, "echo");
            assert_eq!(content, "hello");
        }
        other => panic!("expected echo tool log, got {other:?}"),
    }

    let agents = root.require::<Agents>(AGENTS).unwrap().list();
    assert!(agents.iter().any(|a: &Agent| a.id == "main"));
}

#[tokio::test]
async fn pre_step_can_reject() {
    isolated_home();
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    root.on_waterfall(PRE_STEP, |mut step: PreStep, _| {
        step.deny("nope");
        step
    })
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();

    let err = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("nope")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PreStepRejected));
}

#[tokio::test]
async fn cancelled_turn_stops_before_sample() {
    isolated_home();
    let root = boot().await;
    root.plugin(turn(), ()).unwrap().wait().await.unwrap();
    root.require::<TurnControl>(TURN).unwrap().cancel();
    let err = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("hello")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Cancelled));
}

struct PromptOnly;

impl Driver for PromptOnly {
    fn handle_prompt<'a>(
        &'a self,
        ctx: &'a Context,
        prompt: String,
    ) -> BoxFuture<'a, cordis_spine::Result<TurnOutcome>> {
        Box::pin(async move {
            let sessions = ctx.require::<Sessions>(SESSIONS)?;
            let llm = ctx.require::<cordis_spine::Llm>(LLM)?;
            let system = ctx.require::<cordis_spine::SystemPrompt>(SYSTEM_PROMPT)?;
            let _tools = ctx.require::<cordis_spine::Tools>(TOOLS)?;
            let _agents = ctx.require::<Agents>(AGENTS)?;
            sessions.append(cordis_spine::LogEvent::User(prompt.into()));
            let assembled = system.assemble();
            sessions.append(cordis_spine::LogEvent::Prompt(assembled.clone()));
            let out = llm
                .stream(cordis_spine::PromptRequest {
                    system: assembled,
                    history: sessions.events(),
                    tools: Vec::new(),
                })
                .await;
            Ok(TurnOutcome::Text(out.summary()))
        })
    }
}

fn prompt_only_loop() -> cordis::Plugin {
    plugin(
        "prompt-only-loop",
        Inject::from([SESSIONS, LLM, TOOLS, SYSTEM_PROMPT, AGENTS]),
        |ctx, _: &()| {
            let handle = LoopHandle::new(ctx.clone(), Arc::new(PromptOnly));
            Ok(Some(ctx.provide(AGENT_LOOP, handle)?))
        },
    )
}

#[tokio::test]
async fn swap_loop_plugin_keeps_the_five_services() {
    isolated_home();
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    let grok = root.plugin(agent_loop(), ()).unwrap();
    grok.wait().await.unwrap();
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("hello")
        .await
        .unwrap();
    grok.dispose().await.unwrap();
    grok.wait().await.unwrap();
    assert!(root.get::<LoopHandle>(AGENT_LOOP).is_none());

    root.plugin(prompt_only_loop(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("again")
        .await
        .unwrap();

    let kinds = root.require::<Sessions>(SESSIONS).unwrap().kinds();
    assert_eq!(kinds.iter().filter(|k| **k == "llm/stream").count(), 3);
    assert_eq!(kinds.iter().filter(|k| **k == "tools/execute").count(), 1);
}

#[tokio::test]
async fn second_turn_does_not_reuse_previous_tool_result() {
    isolated_home();
    let root = boot().await;
    let agent = root.require::<LoopHandle>(AGENT_LOOP).unwrap();
    assert_eq!(
        agent.run("first").await.unwrap(),
        TurnOutcome::Text("echoed: first".into())
    );
    assert_eq!(
        agent.run("second").await.unwrap(),
        TurnOutcome::Text("echoed: second".into())
    );
}

fn extra_ping() -> cordis::Plugin {
    plugin("extra-ping", Inject::from(["tools"]), |ctx, _: &()| {
        let tools = ctx
            .require::<cordis_spine::Tools>(cordis_spine::TOOLS)
            .unwrap();
        let body: cordis_spine::ToolBody = std::sync::Arc::new(|call| {
            Box::pin(async move {
                cordis_spine::ToolResult {
                    call_id: call.id,
                    name: call.name,
                    content: "pong".into(),
                    ..Default::default()
                }
            })
        });
        cordis_spine::own_registered(
            ctx,
            vec![tools.register(
                cordis_spine::ToolSpec {
                    name: "docs__ping".into(),
                    description: "ping".into(),
                    parameters_json: r#"{"type":"object"}"#.into(),
                },
                body,
            )?],
        )?;
        Ok(None)
    })
}

#[tokio::test]
async fn tools_register_is_live_and_disposed_with_fiber() {
    isolated_home();
    use cordis_spine::{install_without_llm, ToolCall, Tools, TOOLS};

    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    let tools = root.require::<Tools>(TOOLS).unwrap();
    let before = tools
        .execute(ToolCall {
            id: "1".into(),
            name: "docs__ping".into(),
            arguments: "echo-me".into(),
        })
        .await;
    assert_eq!(before.content, "echo-me");
    assert!(!tools.specs().iter().any(|s| s.name == "docs__ping"));

    let fiber = root.plugin(extra_ping(), ()).unwrap();
    fiber.wait().await.unwrap();
    assert!(tools.specs().iter().any(|s| s.name == "docs__ping"));
    let after = tools
        .execute(ToolCall {
            id: "2".into(),
            name: "docs__ping".into(),
            arguments: "echo-me".into(),
        })
        .await;
    assert_eq!(after.content, "pong");

    fiber.dispose().await.unwrap();
    assert!(!tools.specs().iter().any(|s| s.name == "docs__ping"));
}

#[tokio::test]
async fn install_fakes_echo_has_no_capability_tools() {
    isolated_home();
    let root = boot().await;
    let tools = root.require::<cordis_spine::Tools>(TOOLS).unwrap();
    let names: Vec<String> = tools.specs().into_iter().map(|s| s.name).collect();
    for banned in [
        "web_fetch",
        "todo_write",
        "canvas_create",
        "canvas_edit",
        "canvas_data",
        "canvas_read",
        "ask_user_question",
        "enter_plan_mode",
        "job",
        "scheduler_create",
        "cordis_define",
        "cordis_run",
        "search_tool",
        "use_tool",
        "browser_open",
        "browser_snapshot",
    ] {
        assert!(
            !names.iter().any(|n| n == banned),
            "{banned} must not be on echo tools: {names:?}"
        );
    }
}

/// inspect 的 services 目录是手抄的（`Context` 没有类型擦除的「这个名字活着吗」），
/// 漏一个就是对模型少报一件活着的能力，而且没有任何症状：它会绕路，或者回「做不到」。
/// 这里把「`install_app` 真的挂了」和「报告里有」钉在一起。
#[tokio::test]
async fn cordis_inspect_services_covers_everything_install_app_mounts() {
    isolated_home();
    let root = Context::new();
    cordis_spine::install_app(&root).await.unwrap();
    // 默认 code preset 不含 cordis_*，走工具执行会被允许名单挡掉。
    root.require::<cordis_spine::AgentPresets>(cordis_spine::AGENT_PRESETS)
        .unwrap()
        .apply(cordis_spine::CORDIS_PRESET_ID)
        .unwrap();
    let services = root
        .require::<cordis_spine::Tools>(TOOLS)
        .unwrap()
        .execute(cordis_spine::ToolCall {
            id: "svc".into(),
            name: "cordis_inspect".into(),
            arguments: r#"{"what":"services"}"#.into(),
        })
        .await
        .content;
    for need in [
        "sessions",
        "llm",
        "tools",
        "systemPrompt",
        "context",
        "agents",
        "settings",
        "turn",
        "permissions",
        "cron",
        "roster",
        "jobs",
        "slash",
        "skills",
        "tui.slots",
        "agentPresets",
        "todos",
        "planMode",
        "ask",
        "mcp",
        "subagents",
        "memory",
        "browser",
        "lsp",
        "goal",
        "workflows",
        "computer",
        "compact",
        "dynamicCordisRunner",
        "rhaiBags",
    ] {
        assert!(
            services.contains(need),
            "cordis_inspect services must report the live service {need}: {services}"
        );
    }
}

/// 按需工具只靠 `search_tool` 描述里的名字被模型知道。名单按这一页实际藏起来、
/// 又叫得动的本地工具生成：漏一颗，模型不知道它存在；多列一颗常驻工具，模型会
/// 绕一圈去搜一颗本来就在表上的工具。默认预设（`code`）的 `on_demand_tools` 决定。
#[tokio::test]
async fn search_tool_names_every_builtin_on_demand_tool() {
    isolated_home();
    let root = Context::new();
    cordis_spine::install_app(&root).await.unwrap();
    let tools = root.require::<cordis_spine::Tools>(TOOLS).unwrap();
    let presets = root
        .require::<cordis_spine::AgentPresets>(cordis_spine::AGENT_PRESETS)
        .unwrap();
    let sampler = tools.specs_for_model();
    let desc = &sampler
        .iter()
        .find(|s| s.name == "search_tool")
        .expect("search_tool on the sampler")
        .description;
    let line = desc
        .split_once("Built-in on-demand tools:")
        .expect("search_tool lists the built-in on-demand tools")
        .1;
    let listed: std::collections::BTreeSet<&str> = line
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    for name in ["canvas_create", "workflow", "kill_task", "list_agents"] {
        assert!(listed.contains(name), "{name} missing: {line}");
    }
    for spec in tools.specs() {
        let expected = !tools.is_mcp(&spec.name)
            && !tools.is_dynamic(&spec.name)
            && tools.hidden_on(&root, &spec.name)
            && presets.allows(&spec.name);
        assert_eq!(
            listed.contains(spec.name.as_str()),
            expected,
            "{}: listed vs on-demand disagree: {line}",
            spec.name
        );
    }
}

/// 守望只调度：它的预设不写 `on_demand_tools`，list_agents / interrupt_agent
/// 常驻；默认预设（`code`）里这两颗按需加载。
#[tokio::test]
async fn warden_keeps_its_subagent_tools_resident() {
    isolated_home();
    let root = Context::new();
    cordis_spine::install_app(&root).await.unwrap();
    let tools = root.require::<cordis_spine::Tools>(TOOLS).unwrap();
    let model = |tools: &cordis_spine::Tools| -> Vec<String> {
        tools
            .specs_for_model()
            .into_iter()
            .map(|s| s.name)
            .collect()
    };
    let before = model(&tools);
    assert!(!before.iter().any(|n| n == "list_agents"), "{before:?}");
    root.require::<cordis_spine::AgentPresets>(cordis_spine::AGENT_PRESETS)
        .unwrap()
        .apply("warden")
        .unwrap();
    let after = model(&tools);
    for name in ["list_agents", "interrupt_agent"] {
        assert!(
            after.iter().any(|n| n == name),
            "warden must keep {name}: {after:?}"
        );
    }
}

#[tokio::test]
async fn install_app_registers_capability_tools_and_mcp_fail_open() {
    // 隔离 home：真实 `~/.dock` 里若有 `roster.yml`，默认 code preset 上的
    // enter_plan_mode / task 会被过滤掉。
    isolated_home();
    let root = Context::new();
    cordis_spine::install_app(&root).await.unwrap();
    let tools = root.require::<cordis_spine::Tools>(TOOLS).unwrap();
    let names: Vec<String> = tools.specs().into_iter().map(|s| s.name).collect();
    for need in [
        "web_fetch",
        "web_search",
        "todo_write",
        "ask_user_question",
        "handoff_to_user",
        "enter_plan_mode",
        "exit_plan_mode",
        "job",
        "kill_task",
        "scheduler_create",
        "scheduler_list",
        "scheduler_delete",
        "bash",
        "list_dir",
        "task",
        "send_message",
        "list_agents",
        "interrupt_agent",
        "lsp",
        "skill",
        "update_goal",
        "monitor",
        "workflow",
        "search_tool",
        "use_tool",
        "cordis_inspect",
        "cordis_define",
        "cordis_run",
        "cordis_call",
        "cordis_stop",
        "cordis_promote",
        "propose_preset",
    ] {
        assert!(
            names.iter().any(|n| n == need),
            "missing {need} in {names:?}"
        );
    }
    // 合并掉的两颗（inspect_self → inspect 的 pluginId，undefine → stop drop:true）
    // 不该再出现：八颗挤不进 search_tool 默认的五个命中，define / run 就是这么掉出
    // 窗口的。要加回来得是有意的决定，不是手滑。
    for retired in ["cordis_inspect_self", "cordis_undefine"] {
        assert!(
            !names.iter().any(|n| n == retired),
            "{retired} was folded into another cordis tool: {names:?}"
        );
    }
    let mcp = root
        .get::<cordis_spine::Mcp>(cordis_spine::MCP)
        .expect("mcp-client must provide even with no servers");
    let _ = mcp.list();
    let model: Vec<String> = tools
        .specs_for_model()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        model.iter().any(|n| n == "search_tool") && model.iter().any(|n| n == "use_tool"),
        "sampler must see search_tool/use_tool: {model:?}"
    );
    assert!(
        !model.iter().any(|n| n.starts_with("mcp_")),
        "sampler must hide MCP extras: {model:?}"
    );
    // 常驻的是高频工具，和按需拿不到的场景：只读子代理没有 `use_tool`，所以
    // web_* 必须常驻；子代理回报父级靠 send_message；`skill` 由技能目录
    // reminder 点名、几乎每次都用。低频工具只在 `search_tool` 的描述里留名字，
    // 用时按精确名搜一次拿 schema（见 `search_tool_names_every_builtin_on_demand_tool`）。
    // `memory_search` / `memory_get` are sampler-resident when memory is enabled
    // and absent from the table when disabled (default in this install test).
    for listed in [
        "skill",
        "web_fetch",
        "web_search",
        "job",
        "enter_plan_mode",
        "exit_plan_mode",
        "send_message",
    ] {
        assert!(
            model.iter().any(|n| n == listed),
            "sampler must expose the resident tool {listed}: {model:?}"
        );
    }
    for hidden in [
        "propose_preset",
        "scheduler_create",
        "monitor",
        "update_goal",
        "lsp",
        "cordis_inspect",
        "canvas_create",
        "canvas_edit",
        "canvas_data",
        "canvas_read",
        "workflow",
        "kill_task",
        "list_agents",
        "interrupt_agent",
    ] {
        assert!(
            !model.iter().any(|n| n == hidden),
            "sampler must hide deferred {hidden}: {model:?}"
        );
    }
    let canvas_search = tools
        .execute(cordis_spine::ToolCall {
            id: "st-canvas".into(),
            name: "search_tool".into(),
            arguments: r#"{"query":"canvas","limit":5}"#.into(),
        })
        .await;
    // 已有的 `search_tool` 结果也要能搜到预设名单里的按需工具。
    assert!(
        canvas_search.content.contains("canvas_create"),
        "search_tool should surface deferred locals: {}",
        canvas_search.content
    );
    // 默认预设不准调度工具：藏着，但搜不出来——搜到了 `use_tool` 也会拒绝。
    let sched_search = tools
        .execute(cordis_spine::ToolCall {
            id: "st-sched".into(),
            name: "search_tool".into(),
            arguments: r#"{"query":"scheduler","limit":5}"#.into(),
        })
        .await;
    assert!(
        !sched_search.content.contains("scheduler_create"),
        "search_tool must not surface tools outside the preset: {}",
        sched_search.content
    );
    // 浏览器不再是进程内工具：它是内置 MCP `browser`（`dock mcp browser`），由 `dock`
    // 二进制登记后才注入。测试进程不登记，所以工具表里一颗 browser_* 都不该有；
    // MCP 那条路径的端到端测试在 cordis-app/tests/browser_mcp.rs。
    assert!(
        !tools.specs().iter().any(|s| s.name.starts_with("browser_")),
        "in-process browser_* tools must be gone"
    );
    let search = tools
        .execute(cordis_spine::ToolCall {
            id: "st".into(),
            name: "search_tool".into(),
            arguments: r#"{"query":"linear"}"#.into(),
        })
        .await;
    assert!(
        search.content.contains("total_hidden_tools"),
        "search_tool: {}",
        search.content
    );
    assert!(root
        .get::<cordis_spine::PlanMode>(cordis_spine::PLAN_MODE)
        .is_some());
    assert!(root
        .get::<cordis_spine::Todos>(cordis_spine::TODOS)
        .is_some());
    assert!(root.get::<cordis_spine::Jobs>(cordis_spine::JOBS).is_some());
    assert!(root
        .get::<cordis_spine::Slash>(cordis_spine::SLASH)
        .is_some());
    assert!(root
        .get::<cordis_spine::AgentPresets>(cordis_spine::AGENT_PRESETS)
        .is_some());
    assert!(root
        .get::<cordis_spine::ContextBook>(cordis_spine::CONTEXT)
        .is_some());
    let assembled = root
        .require::<cordis_spine::SystemPrompt>(SYSTEM_PROMPT)
        .unwrap()
        .assemble();
    assert!(
        !assembled.contains("# Dynamic Cordis Plugins"),
        "{assembled}"
    );
    assert!(
        !assembled.contains("search_tool 查 cordis"),
        "cordis 写法不进系统提示: {assembled}"
    );
    assert!(assembled.contains("<agent-preset>"), "{assembled}");
    assert!(assembled.contains("当前是Dock编码模式"), "{assembled}");
    assert!(!assembled.contains("# Agent 预设"), "{assembled}");
    assert!(!assembled.contains("update_goal(objective"), "{assembled}");
    assert!(root.get::<cordis_spine::Ask>(cordis_spine::ASK).is_some());
    assert!(root
        .get::<cordis_spine::Compact>(cordis_spine::COMPACT)
        .is_some());
    assert!(root
        .get::<cordis_spine::DynamicRunner>(cordis_spine::DYNAMIC_CORDIS_RUNNER)
        .is_some());
    assert!(root
        .get::<cordis_spine::LspHub>(cordis_spine::LSP)
        .is_some());
    assert!(root
        .get::<cordis_spine::Browser>(cordis_spine::BROWSER)
        .is_some_and(|b| b.state() == cordis_spine::BrowserState::Missing));
    assert!(root
        .require::<cordis_spine::Slash>(cordis_spine::SLASH)
        .unwrap()
        .list()
        .iter()
        .any(|e| e.command == "browser"));
    assert!(root
        .get::<cordis_spine::Skills>(cordis_spine::SKILLS)
        .is_some());

    let lsp_parse = tools
        .execute(cordis_spine::ToolCall {
            id: "lsp-bad".into(),
            name: "lsp".into(),
            arguments: "{}".into(),
        })
        .await;
    assert!(
        lsp_parse.content.contains("Error"),
        "lsp missing operation: {}",
        lsp_parse.content
    );

    let todo = tools
        .execute(cordis_spine::ToolCall {
            id: "t1".into(),
            name: "todo_write".into(),
            arguments: r#"{"merge":false,"todos":[{"id":"1","content":"copied grok merge","status":"pending"}]}"#.into(),
        })
        .await;
    assert!(
        todo.content.contains("copied grok merge"),
        "{}",
        todo.content
    );

    let plan = tools
        .execute(cordis_spine::ToolCall {
            id: "p1".into(),
            name: "enter_plan_mode".into(),
            arguments: "{}".into(),
        })
        .await;
    assert!(plan.content.contains("exit_plan_mode"), "{}", plan.content);
    assert!(
        plan.content.contains("ask_user_question"),
        "{}",
        plan.content
    );
    assert!(root
        .get::<cordis_spine::PlanMode>(cordis_spine::PLAN_MODE)
        .is_some_and(|p| p.active()));

    // 计划模式里 bash 的只读命令直接跑（只读场景把关，见 `tools::read_only`）……
    let read = tools
        .execute(cordis_spine::ToolCall {
            id: "b1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"echo plan-read-ok"}"#.into(),
        })
        .await;
    assert!(read.content.contains("plan-read-ok"), "{}", read.content);
    // ……写文件照样被计划门挡下（计划文件以外）。
    let blocked = tools
        .execute(cordis_spine::ToolCall {
            id: "b2".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"not-the-plan.md","content":"x"}"#.into(),
        })
        .await;
    assert!(
        blocked.content.contains("计划模式已阻止"),
        "{}",
        blocked.content
    );

    assert!(root.get::<cordis_spine::Goal>(cordis_spine::GOAL).is_some());

    let disabled = tools
        .execute(cordis_spine::ToolCall {
            id: "g0".into(),
            name: "update_goal".into(),
            arguments: r#"{"message":"before /goal"}"#.into(),
        })
        .await;
    assert!(
        disabled.content.contains("goal_update_harness_disabled")
            && disabled.content.contains("no /goal"),
        "{}",
        disabled.content
    );

    let started = tools
        .execute(cordis_spine::ToolCall {
            id: "gs".into(),
            name: "update_goal".into(),
            arguments: r#"{"objective":"ship lsp"}"#.into(),
        })
        .await;
    assert!(
        started.content.contains("Goal set") && started.content.contains("ship lsp"),
        "{}",
        started.content
    );
    assert!(root
        .get::<cordis_spine::Goal>(cordis_spine::GOAL)
        .is_some_and(|g| g.active() && g.title() == "ship lsp"));

    let progress = tools
        .execute(cordis_spine::ToolCall {
            id: "g1".into(),
            name: "update_goal".into(),
            arguments: r#"{"message":"working on it"}"#.into(),
        })
        .await;
    assert!(
        progress.content.contains("working on it"),
        "{}",
        progress.content
    );

    let done = tools
        .execute(cordis_spine::ToolCall {
            id: "g2".into(),
            name: "update_goal".into(),
            arguments: r#"{"completed":true,"message":"shipped"}"#.into(),
        })
        .await;
    assert!(
        done.content.contains("Goal marked complete"),
        "{}",
        done.content
    );

    assert!(root
        .get::<cordis_spine::Workflows>(cordis_spine::WORKFLOWS)
        .is_some());
    let smoke = tools
        .execute(cordis_spine::ToolCall {
            id: "w1".into(),
            name: "workflow".into(),
            arguments: r#"{"source":{"type":"script","script":"let meta = #{ name: \"valid-name\", description: \"d\" };\ncomplete(\"ok\");"},"validate_only":true}"#.into(),
        })
        .await;
    assert!(
        smoke.content.contains("valid-name") && smoke.content.contains("冒烟"),
        "{}",
        smoke.content
    );

    assert!(root
        .get::<cordis_spine::Subagents>(cordis_spine::SUBAGENTS)
        .is_some());
    let unknown = tools
        .execute(cordis_spine::ToolCall {
            id: "sa0".into(),
            name: "task".into(),
            arguments: r#"{"prompt":"x","description":"bad type","subagent_type":"not-a-type"}"#
                .into(),
        })
        .await;
    assert!(
        unknown.content.contains("Unknown subagent type")
            && unknown.content.contains("general-purpose"),
        "{}",
        unknown.content
    );
    let started = tools
        .execute(cordis_spine::ToolCall {
            id: "sa1".into(),
            name: "task".into(),
            arguments: r#"{"prompt":"noop","description":"coord smoke","run_in_background":true}"#
                .into(),
        })
        .await;
    assert!(
        started.content.contains("Subagent started in background")
            && started.content.contains("agent_id:")
            && !started.content.contains("job_ids"),
        "{}",
        started.content
    );
}

struct CountThenComplete {
    n: Arc<AtomicUsize>,
    ctx: Context,
}

impl Sampler for CountThenComplete {
    fn sample<'a>(
        &'a self,
        _request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let n = self.n.clone();
        let ctx = self.ctx.clone();
        Box::pin(async move {
            let i = n.fetch_add(1, Ordering::SeqCst);
            if i >= 1 {
                if let Some(goal) = ctx.get::<Goal>(GOAL) {
                    goal.clear();
                }
            }
            LlmOutput {
                text: format!("round-{i}"),
                ..LlmOutput::default()
            }
        })
    }
}

#[tokio::test]
async fn active_goal_keeps_sampling_after_first_text() {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(tool_goal(), ()).unwrap().wait().await.unwrap();
    let samples = Arc::new(AtomicUsize::new(0));
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(CountThenComplete {
                n: samples.clone(),
                ctx: root.clone(),
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root.require::<Goal>(GOAL).unwrap().start("理解并分析 TUI");
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("理解并分析 TUI")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("round-1".into()));
    assert_eq!(
        samples.load(Ordering::SeqCst),
        2,
        "goal must not stop on the first text-only sample"
    );
    let kinds = root.require::<Sessions>(SESSIONS).unwrap().kinds();
    assert!(
        kinds.contains(&"system-reminder"),
        "Grok goal continuation is a hidden reminder, got {kinds:?}"
    );
    let users = kinds.iter().filter(|k| **k == "user").count();
    assert_eq!(
        users, 1,
        "continuation must not appear as a user bubble: {kinds:?}"
    );
}

#[tokio::test]
async fn text_without_goal_samples_once() {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    let samples = Arc::new(AtomicUsize::new(0));
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(CountThenComplete {
                n: samples.clone(),
                ctx: root.clone(),
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("hello")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("round-0".into()));
    assert_eq!(samples.load(Ordering::SeqCst), 1);
}

struct ToolsThenText {
    n: Arc<AtomicUsize>,
    tool_rounds: usize,
}

impl Sampler for ToolsThenText {
    fn sample<'a>(
        &'a self,
        _request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let n = self.n.clone();
        let tool_rounds = self.tool_rounds;
        Box::pin(async move {
            let i = n.fetch_add(1, Ordering::SeqCst);
            if i < tool_rounds {
                LlmOutput {
                    tool_calls: vec![ToolCall {
                        id: format!("c{i}"),
                        name: "echo".into(),
                        arguments: "x".into(),
                    }],
                    ..LlmOutput::default()
                }
            } else {
                LlmOutput {
                    text: "done".into(),
                    ..LlmOutput::default()
                }
            }
        })
    }
}

#[tokio::test]
async fn coding_turn_survives_more_than_eight_tool_rounds() {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    let samples = Arc::new(AtomicUsize::new(0));
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(ToolsThenText {
                n: samples.clone(),
                tool_rounds: 12,
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("work")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("done".into()));
    assert_eq!(samples.load(Ordering::SeqCst), 13);
}

/// Scripted `todo_write` turn: open the list, answer with text, then (after the
/// gate pushes back) close the list and answer again.
struct TodoScript {
    n: Arc<AtomicUsize>,
    /// Sample indexes at which a `todo_write` call is emitted, with its args.
    writes: Vec<(usize, &'static str)>,
}

impl Sampler for TodoScript {
    fn sample<'a>(
        &'a self,
        _request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let n = self.n.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
            let i = n.fetch_add(1, Ordering::SeqCst);
            match writes.iter().find(|(at, _)| *at == i) {
                Some((_, args)) => LlmOutput {
                    tool_calls: vec![ToolCall {
                        id: format!("t{i}"),
                        name: "todo_write".into(),
                        arguments: (*args).to_string(),
                    }],
                    ..LlmOutput::default()
                },
                None => LlmOutput {
                    text: format!("round-{i}"),
                    ..LlmOutput::default()
                },
            }
        })
    }
}

async fn boot_todo_loop(samples: Arc<AtomicUsize>, writes: Vec<(usize, &'static str)>) -> Context {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(tool_todo(), ()).unwrap().wait().await.unwrap();
    root.provide(
        LLM,
        Llm::from_sampler(root.clone(), Arc::new(TodoScript { n: samples, writes })),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root
}

const OPEN_LIST: &str = r#"{"merge":false,"todos":[
    {"id":"a","content":"读代码","status":"in_progress"},
    {"id":"b","content":"改实现","status":"pending"}]}"#;
const CLOSE_LIST: &str = r#"{"todos":[
    {"id":"a","status":"completed"},
    {"id":"b","status":"completed"}]}"#;

fn sentinel_reminders(root: &Context) -> Vec<String> {
    root.require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .filter_map(|e| match e {
            cordis_spine::LogEvent::SystemReminder(t)
                if t.contains(cordis_spine::TODO_BOOKKEEPING_SENTINEL) =>
            {
                Some(t.clone())
            }
            _ => None,
        })
        .collect()
}

/// Text with owed items gets one bookkeeping step — not another round of
/// work — and the turn ends right after it.
#[tokio::test]
async fn open_todos_get_one_bookkeeping_step_then_the_turn_ends() {
    let samples = Arc::new(AtomicUsize::new(0));
    // s0 opens the list, s1 answers with text, s2 is the bookkeeping step and
    // closes the list. The turn ends there; s1's text is the reply.
    let root = boot_todo_loop(samples.clone(), vec![(0, OPEN_LIST), (2, CLOSE_LIST)]).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("round-1".into()));
    assert_eq!(samples.load(Ordering::SeqCst), 3);
    let reminders = sentinel_reminders(&root);
    assert_eq!(
        reminders.len(),
        1,
        "one bookkeeping reminder: {reminders:?}"
    );
    assert!(
        reminders[0].contains("in_progress: 读代码"),
        "{}",
        reminders[0]
    );
    assert!(reminders[0].contains("pending: 改实现"), "{}", reminders[0]);
    let todos = root
        .get::<cordis_spine::Todos>(cordis_spine::TODOS)
        .unwrap();
    assert_eq!(todos.stats().completed, 2, "the bookkeeping write landed");
    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    let users = sessions.kinds().iter().filter(|k| **k == "user").count();
    assert_eq!(
        users, 1,
        "bookkeeping is a hidden reminder, not a user bubble"
    );
}

#[tokio::test]
async fn a_closed_list_lets_the_turn_end() {
    let samples = Arc::new(AtomicUsize::new(0));
    let root = boot_todo_loop(samples.clone(), vec![(0, CLOSE_LIST)]).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("round-1".into()));
    assert_eq!(samples.load(Ordering::SeqCst), 2);
    assert!(sentinel_reminders(&root).is_empty());
}

/// The model ignores the bookkeeping step (answers with text again): the turn
/// still ends after that one step — no second round, no work.
#[tokio::test]
async fn bookkeeping_fires_at_most_once_and_then_ends_the_turn() {
    let samples = Arc::new(AtomicUsize::new(0));
    let root = boot_todo_loop(samples.clone(), vec![(0, OPEN_LIST)]).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("round-1".into()));
    assert_eq!(
        samples.load(Ordering::SeqCst),
        3,
        "one tool round + text + exactly one bookkeeping step"
    );
    assert_eq!(sentinel_reminders(&root).len(), 1);
    let kinds = root.require::<Sessions>(SESSIONS).unwrap().kinds();
    assert_eq!(
        kinds.iter().filter(|k| **k == "llm/stream").count(),
        3,
        "{kinds:?}"
    );
    assert_eq!(
        kinds[kinds.len() - 3..],
        ["system-reminder", "llm/stream", "turn-end"],
        "the bookkeeping reminder, its one sample, then the turn ends: {kinds:?}"
    );
}

/// The status-only write that forgot `merge: true` must keep its content
/// (Grok's `effective_merge` auto-upgrade), or the live card and the gate
/// would both start showing raw ids.
#[tokio::test]
async fn status_only_write_without_merge_keeps_content() {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(tool_todo(), ()).unwrap().wait().await.unwrap();
    let tools = root.require::<cordis_spine::Tools>(TOOLS).unwrap();
    tools
        .execute(ToolCall {
            id: "1".into(),
            name: "todo_write".into(),
            arguments: OPEN_LIST.into(),
        })
        .await;
    let out = tools
        .execute(ToolCall {
            id: "2".into(),
            name: "todo_write".into(),
            arguments: r#"{"merge":false,"todos":[{"id":"a","status":"completed"}]}"#.into(),
        })
        .await;
    assert!(out.content.contains("读代码"), "{}", out.content);
    let todos = root
        .get::<cordis_spine::Todos>(cordis_spine::TODOS)
        .unwrap();
    assert_eq!(todos.snapshot().len(), 2, "auto-merge must not drop item b");
    assert_eq!(todos.stats().completed, 1);
}

/// `"todos"` is shared with subagent isolates (only sessions / turn /
/// agentPresets are isolated), so a child must not be gated — or nudged — by
/// the parent's list.
#[tokio::test]
async fn a_subagent_turn_is_not_gated_by_the_parent_list() {
    let samples = Arc::new(AtomicUsize::new(0));
    let root = boot_todo_loop(samples.clone(), vec![(0, OPEN_LIST)]).await;
    // Fill the shared list from the parent side.
    root.require::<cordis_spine::Tools>(TOOLS)
        .unwrap()
        .execute(ToolCall {
            id: "seed".into(),
            name: "todo_write".into(),
            arguments: OPEN_LIST.into(),
        })
        .await;
    samples.store(1, Ordering::SeqCst); // past the scripted write

    let child = root.isolate("sessions");
    child
        .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-1"))
        .unwrap();
    let out = LoopHandle::new(child.clone(), Arc::new(cordis_spine::GrokStep))
        .run("子任务")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("round-1".into()));
    assert_eq!(
        samples.load(Ordering::SeqCst),
        2,
        "child must end on its first text sample"
    );
    let kinds = child.require::<Sessions>(SESSIONS).unwrap().kinds();
    assert!(
        !kinds.contains(&"system-reminder"),
        "no parent-todo nudge in a child session: {kinds:?}"
    );
}

/// Long tool run without touching the list: the watchdog reminds the model to
/// check items off mid-turn, and does it once, not once per step.
#[tokio::test]
async fn a_stale_list_gets_a_mid_turn_nudge() {
    struct WriteThenGrind {
        n: Arc<AtomicUsize>,
        tool_rounds: usize,
    }
    impl Sampler for WriteThenGrind {
        fn sample<'a>(
            &'a self,
            _request: PromptRequest,
            _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            let n = self.n.clone();
            let rounds = self.tool_rounds;
            Box::pin(async move {
                let i = n.fetch_add(1, Ordering::SeqCst);
                let call = |name: &str, args: &str| LlmOutput {
                    tool_calls: vec![ToolCall {
                        id: format!("c{i}"),
                        name: name.into(),
                        arguments: args.into(),
                    }],
                    ..LlmOutput::default()
                };
                match i {
                    0 => call("todo_write", OPEN_LIST),
                    i if i <= rounds => call("echo", "x"),
                    _ => LlmOutput {
                        text: "done".into(),
                        ..LlmOutput::default()
                    },
                }
            })
        }
    }

    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(tool_todo(), ()).unwrap().wait().await.unwrap();
    let samples = Arc::new(AtomicUsize::new(0));
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(WriteThenGrind {
                n: samples.clone(),
                tool_rounds: 8,
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    let _ = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    let nudges = root
        .require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .filter(
            |e| matches!(e, cordis_spine::LogEvent::SystemReminder(t) if t.contains("没有更新")),
        )
        .count();
    assert_eq!(nudges, 1, "one stale-list nudge across 8 quiet tool rounds");
}

/// Grinds tool calls for `rounds` samples, then answers with text.
struct Grind {
    n: Arc<AtomicUsize>,
    rounds: usize,
}

impl Sampler for Grind {
    fn sample<'a>(
        &'a self,
        _request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let n = self.n.clone();
        let rounds = self.rounds;
        Box::pin(async move {
            let i = n.fetch_add(1, Ordering::SeqCst);
            if i < rounds {
                return LlmOutput {
                    tool_calls: vec![ToolCall {
                        id: format!("c{i}"),
                        name: "echo".into(),
                        arguments: "x".into(),
                    }],
                    ..LlmOutput::default()
                };
            }
            LlmOutput {
                text: format!("round-{i}"),
                ..LlmOutput::default()
            }
        })
    }
}

/// The mid-turn nudge is main-session discipline as much as the gate is:
/// `"todos"` is shared with children, so a grinding subagent must not be nagged
/// about a list it was never given — and its steps must not move the parent's
/// counters, since both sessions now hit one plugin-side watchdog.
#[tokio::test]
async fn a_subagent_grind_gets_no_stale_nudge() {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(tool_todo(), ()).unwrap().wait().await.unwrap();
    let samples = Arc::new(AtomicUsize::new(0));
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(Grind {
                n: samples.clone(),
                rounds: 9,
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    // Fill the shared list from the parent side, then hand the turn to a child.
    root.require::<cordis_spine::Tools>(TOOLS)
        .unwrap()
        .execute(ToolCall {
            id: "seed".into(),
            name: "todo_write".into(),
            arguments: OPEN_LIST.into(),
        })
        .await;

    let child = root.isolate("sessions");
    child
        .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-1"))
        .unwrap();
    let _ = LoopHandle::new(child.clone(), Arc::new(cordis_spine::GrokStep))
        .run("子任务")
        .await
        .unwrap();
    assert!(
        samples.load(Ordering::SeqCst) > 6,
        "the child must grind past the nudge threshold for this to prove anything"
    );
    let kinds = child.require::<Sessions>(SESSIONS).unwrap().kinds();
    assert!(
        !kinds.contains(&"system-reminder"),
        "no parent-list nudge in a child session: {kinds:?}"
    );
}

/// `agent/pre-step` fires on a subagent turn too, and its handlers can only
/// reach the session they registered on — the main one. Without the identity
/// guard a child turn eats the goal's one-shot instruction and drops it into
/// the parent's transcript, where the user's own next turn never sees it.
#[tokio::test]
async fn a_subagent_turn_does_not_eat_the_parent_goal_instruction() {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(tool_goal(), ()).unwrap().wait().await.unwrap();
    let samples = Arc::new(AtomicUsize::new(0));
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(TodoScript {
                n: samples,
                writes: vec![],
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root.require::<Goal>(GOAL).unwrap().start("交付这次改动");

    let child = root.isolate("sessions");
    child
        .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-1"))
        .unwrap();
    let _ = LoopHandle::new(child.clone(), Arc::new(cordis_spine::GrokStep))
        .run("子任务")
        .await
        .unwrap();

    let parent = root.require::<Sessions>(SESSIONS).unwrap().kinds();
    assert!(
        !parent.contains(&"system-reminder"),
        "a child turn must not append to the parent transcript: {parent:?}"
    );
    assert!(
        root.require::<Goal>(GOAL)
            .unwrap()
            .take_instruction()
            .is_some(),
        "the one-shot goal instruction must still be waiting for the user's own turn"
    );
}

/// A handler cannot see the votes cast outside it, so its own self-limit has to
/// be charged when it wins, not when it votes. A keep-working vote taking the
/// round must not quietly spend the todo bookkeeping quota.
#[tokio::test]
async fn a_lower_slot_winner_does_not_spend_the_todo_quota() {
    let samples = Arc::new(AtomicUsize::new(0));
    // s0 opens the list; every later sample is text the bookkeeping would object to.
    let root = boot_todo_loop(samples.clone(), vec![(0, OPEN_LIST)]).await;
    // Takes the first two endings, then falls silent.
    let _hog = root
        .on_waterfall(TURN_END, |end: TurnEnd, args| {
            let mut next = args.next::<TurnEnd>().unwrap_or(end);
            if next.rounds < 2 {
                next.keep_working(
                    ORDER_TURN_END_TODO - 1,
                    "<system-reminder>先做别的</system-reminder>",
                );
            }
            next
        })
        .unwrap();

    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    // Two rounds taken by the hog, then the bookkeeping step, then the turn ends.
    assert_eq!(out, TurnOutcome::Text("round-3".into()));
    assert_eq!(samples.load(Ordering::SeqCst), 5);
    assert_eq!(
        sentinel_reminders(&root).len(),
        1,
        "the bookkeeping step still happens after losing two rounds"
    );
}

/// `agent/step-start` is an extension point, not a loop private: a handler
/// mounted from outside sees every step numbered from 0 — across turn-end
/// continuations too — and the loop appends what it queues, in slot order.
/// The bookkeeping step is not a work step and runs no step-start handlers.
#[tokio::test]
async fn a_plugin_can_inject_a_reminder_before_every_step() {
    let samples = Arc::new(AtomicUsize::new(0));
    // s0 opens the list, s1..s3 answer with text; a keep-working vote continues
    // the turn twice, so steps 2 and 3 are on the far side of an
    // `agent/turn-end`. s4 is the bookkeeping step.
    let root = boot_todo_loop(samples.clone(), vec![(0, OPEN_LIST)]).await;
    let _continue = root
        .on_waterfall(TURN_END, |end: TurnEnd, args| {
            let mut next = args.next::<TurnEnd>().unwrap_or(end);
            if next.rounds < 2 {
                next.keep_working(90, "<system-reminder>再来</system-reminder>");
            }
            next
        })
        .unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    // Registered low slot first, so the raw vote order is the reverse of the
    // expected append order — the slot sort is what puts it right.
    let steps = seen.clone();
    let _early = root
        .on_waterfall(STEP_START, move |start: StepStart, args| {
            let mut next = args.next::<StepStart>().unwrap_or(start);
            steps.lock().unwrap().push(next.step);
            next.remind(-5, "<system-reminder>早</system-reminder>");
            next
        })
        .unwrap();
    let _late = root
        .on_waterfall(STEP_START, move |start: StepStart, args| {
            let mut next = args.next::<StepStart>().unwrap_or(start);
            next.remind(90, "<system-reminder>晚</system-reminder>");
            next
        })
        .unwrap();

    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("round-3".into()));
    assert_eq!(samples.load(Ordering::SeqCst), 5);
    assert_eq!(
        *seen.lock().unwrap(),
        vec![0, 1, 2, 3],
        "one call per work sample, counting on across turn-end continuations"
    );
    let injected: Vec<String> = root
        .require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .filter_map(|e| match e {
            cordis_spine::LogEvent::SystemReminder(t) if t.contains('早') || t.contains('晚') => {
                Some(t.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(injected.len(), 8, "both handlers land on all four steps");
    for (i, body) in injected.iter().enumerate() {
        let want = if i.is_multiple_of(2) { '早' } else { '晚' };
        assert!(body.contains(want), "step {}: {body}", i / 2);
    }
}

/// An active goal and an unfinished list both have a say at turn end. The
/// goal wants more work (the user asked for it); the list only wants
/// bookkeeping, which would end the turn. Keep-working beats bookkeeping
/// regardless of order slot or mount order, so the goal is not cut short.
async fn boot_goal_and_todo(samples: Arc<AtomicUsize>, todo_first: bool) -> Context {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    if todo_first {
        root.plugin(tool_todo(), ()).unwrap().wait().await.unwrap();
        root.plugin(tool_goal(), ()).unwrap().wait().await.unwrap();
    } else {
        root.plugin(tool_goal(), ()).unwrap().wait().await.unwrap();
        root.plugin(tool_todo(), ()).unwrap().wait().await.unwrap();
    }
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(TodoScript {
                n: samples,
                writes: vec![(0, OPEN_LIST)],
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root.require::<Goal>(GOAL).unwrap().start("交付这次改动");
    root
}

#[tokio::test]
async fn todo_bookkeeping_does_not_cut_an_active_goal_short() {
    for todo_first in [true, false] {
        let samples = Arc::new(AtomicUsize::new(0));
        let root = boot_goal_and_todo(samples, todo_first).await;
        let _ = root
            .require::<LoopHandle>(AGENT_LOOP)
            .unwrap()
            .run("干活")
            .await
            .unwrap();
        // Skip the goal's own pre-step instruction; look at turn-end continuations.
        let continuations: Vec<String> = root
            .require::<Sessions>(SESSIONS)
            .unwrap()
            .events()
            .iter()
            .filter_map(|e| match e {
                cordis_spine::LogEvent::SystemReminder(t)
                    if t.contains(cordis_spine::TODO_BOOKKEEPING_SENTINEL)
                        || t.contains("<goal-state>") =>
                {
                    Some(t.clone())
                }
                _ => None,
            })
            .collect();
        let first = continuations
            .first()
            .expect("an open list plus an active goal must continue the turn");
        assert!(
            first.contains("<goal-state>"),
            "the goal keeps working before any bookkeeping, todo_first={todo_first}: {first}"
        );
        assert!(
            !first.contains(cordis_spine::TODO_BOOKKEEPING_SENTINEL),
            "only the winning reminder is appended: {first}"
        );
        let bookkeeping = continuations
            .iter()
            .filter(|t| t.contains(cordis_spine::TODO_BOOKKEEPING_SENTINEL))
            .count();
        assert!(
            bookkeeping <= 1,
            "todo_first={todo_first}: {continuations:?}"
        );
        if bookkeeping == 1 {
            assert!(
                continuations
                    .last()
                    .unwrap()
                    .contains(cordis_spine::TODO_BOOKKEEPING_SENTINEL),
                "bookkeeping, if any, is the last thing the turn does: {continuations:?}"
            );
        }
    }
}

/// Plays `script(i)` for sample `i` and records which tools each sample was
/// offered.
struct Recorded {
    n: Arc<AtomicUsize>,
    offered: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    script: Arc<dyn Fn(usize) -> LlmOutput + Send + Sync>,
}

impl Sampler for Recorded {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let mut names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
        names.sort();
        self.offered.lock().unwrap().push(names);
        let i = self.n.fetch_add(1, Ordering::SeqCst);
        let out = (self.script)(i);
        Box::pin(async move { out })
    }
}

fn say(text: &str) -> LlmOutput {
    LlmOutput {
        text: text.into(),
        ..LlmOutput::default()
    }
}

fn call_tool(id: &str, name: &str, args: &str) -> LlmOutput {
    LlmOutput {
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args.into(),
        }],
        ..LlmOutput::default()
    }
}

struct Booted {
    root: Context,
    samples: Arc<AtomicUsize>,
    offered: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    _tools: Vec<cordis::Disposable>,
}

impl Booted {
    async fn run(&self, prompt: &str) -> TurnOutcome {
        self.root
            .require::<LoopHandle>(AGENT_LOOP)
            .unwrap()
            .run(prompt)
            .await
            .unwrap()
    }

    fn samples(&self) -> usize {
        self.samples.load(Ordering::SeqCst)
    }

    fn reminders(&self) -> Vec<String> {
        self.root
            .require::<Sessions>(SESSIONS)
            .unwrap()
            .events()
            .iter()
            .filter_map(|e| match e {
                cordis_spine::LogEvent::SystemReminder(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    fn bookkeeping(&self) -> usize {
        sentinel_reminders(&self.root).len()
    }

    /// `(name, content, is_error)` of every tool row in the log.
    fn tool_rows(&self) -> Vec<(String, String, bool)> {
        self.root
            .require::<Sessions>(SESSIONS)
            .unwrap()
            .events()
            .iter()
            .filter_map(|e| match e {
                cordis_spine::LogEvent::ToolExecute {
                    name,
                    content,
                    is_error,
                    ..
                } => Some((name.clone(), content.clone(), *is_error)),
                _ => None,
            })
            .collect()
    }

    fn todos(&self) -> Arc<cordis_spine::Todos> {
        self.root
            .get::<cordis_spine::Todos>(cordis_spine::TODOS)
            .unwrap()
    }
}

/// A fake tool that answers with `reply`.
fn fake_tool(root: &Context, name: &'static str, reply: &'static str) -> cordis::Disposable {
    let body: cordis_spine::ToolBody = Arc::new(move |call: ToolCall| {
        Box::pin(async move {
            cordis_spine::ToolResult {
                call_id: call.id,
                name: call.name,
                content: reply.into(),
                ..Default::default()
            }
        })
    });
    root.require::<cordis_spine::Tools>(TOOLS)
        .unwrap()
        .register(
            cordis_spine::ToolSpec {
                name: name.into(),
                description: format!("fake {name}"),
                parameters_json: r#"{"type":"object","properties":{}}"#.into(),
            },
            body,
        )
        .unwrap()
}

/// Todo plugin + loop + a recording sampler, plus a fake `ask_user_question`
/// and a fake work tool (`edit_fake`) so a normal step offers more than
/// `todo_write`.
async fn boot_recorded(script: impl Fn(usize) -> LlmOutput + Send + Sync + 'static) -> Booted {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(tool_todo(), ()).unwrap().wait().await.unwrap();
    let samples = Arc::new(AtomicUsize::new(0));
    let offered = Arc::new(std::sync::Mutex::new(Vec::new()));
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(Recorded {
                n: samples.clone(),
                offered: offered.clone(),
                script: Arc::new(script),
            }),
        ),
    )
    .unwrap();
    let tools = vec![
        fake_tool(&root, "ask_user_question", "用户答：先这样"),
        fake_tool(&root, "edit_fake", "改好了"),
    ];
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    Booted {
        root,
        samples,
        offered,
        _tools: tools,
    }
}

const AWAITING_LIST: &str = r#"{"merge":false,"todos":[
    {"id":"a","content":"读代码","status":"awaiting_user"},
    {"id":"b","content":"改实现","status":"awaiting_user"}]}"#;

#[tokio::test]
async fn bookkeeping_step_only_exposes_todo_write() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", OPEN_LIST),
        1 => say("方案列好了"),
        // The bookkeeping step tries to start work anyway: refused, not run.
        2 => call_tool("c2", "edit_fake", "{}"),
        _ => say("不该到这里"),
    })
    .await;
    assert_eq!(b.run("干活").await, TurnOutcome::Text("方案列好了".into()));
    assert_eq!(b.samples(), 3);
    let offered = b.offered.lock().unwrap().clone();
    assert!(
        offered[1].contains(&"edit_fake".to_string())
            && offered[1].contains(&"todo_write".to_string()),
        "a work step offers the whole table: {:?}",
        offered[1]
    );
    assert_eq!(
        offered[2],
        ["todo_write"],
        "the bookkeeping step offers only todo_write"
    );
    let rows = b.tool_rows();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1].0, "edit_fake");
    assert_eq!(
        rows[1].1, "没有执行：这一步只做 todo 记账，只能调用 todo_write。",
        "{rows:?}"
    );
    assert!(rows[1].2, "the refused call is an error result");
}

#[tokio::test]
async fn asking_the_user_this_turn_skips_the_todo_bookkeeping() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", OPEN_LIST),
        1 => call_tool("c1", "ask_user_question", "{}"),
        _ => say(&format!("round-{i}")),
    })
    .await;
    assert_eq!(b.run("干活").await, TurnOutcome::Text("round-2".into()));
    assert_eq!(b.samples(), 3, "no bookkeeping step after asking the user");
    assert_eq!(b.bookkeeping(), 0, "{:?}", b.reminders());
    assert_eq!(
        b.todos().stats().open(),
        2,
        "the list is left as the model wrote it"
    );
}

#[tokio::test]
async fn the_ask_flag_rearms_on_the_next_prompt() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", OPEN_LIST),
        1 => call_tool("c1", "ask_user_question", "{}"),
        _ => say(&format!("round-{i}")),
    })
    .await;
    assert_eq!(b.run("干活").await, TurnOutcome::Text("round-2".into()));
    assert_eq!(b.bookkeeping(), 0);
    // Next prompt: no ask this time, items still owed → one bookkeeping step.
    assert_eq!(b.run("继续").await, TurnOutcome::Text("round-3".into()));
    assert_eq!(b.samples(), 5, "s3 answers, s4 is the bookkeeping step");
    assert_eq!(b.bookkeeping(), 1);
    assert_eq!(b.offered.lock().unwrap()[4], ["todo_write"]);
}

#[tokio::test]
async fn plan_mode_skips_the_todo_bookkeeping() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", OPEN_LIST),
        _ => say(&format!("round-{i}")),
    })
    .await;
    let plan = cordis_spine::PlanMode::new(b.root.clone());
    plan.enter_active();
    b.root.provide(cordis_spine::PLAN_MODE, plan).unwrap();
    assert_eq!(b.run("出个计划").await, TurnOutcome::Text("round-1".into()));
    assert_eq!(b.samples(), 2, "plan mode: no bookkeeping step");
    assert_eq!(b.bookkeeping(), 0, "{:?}", b.reminders());
}

#[tokio::test]
async fn all_awaiting_user_skips_bookkeeping() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", AWAITING_LIST),
        _ => say(&format!("round-{i}")),
    })
    .await;
    assert_eq!(b.run("干活").await, TurnOutcome::Text("round-1".into()));
    assert_eq!(b.samples(), 2);
    assert_eq!(b.bookkeeping(), 0, "{:?}", b.reminders());
    assert!(
        !b.reminders().iter().any(|r| r.contains("当前清单")),
        "awaiting_user items are not 'current' or 'next': {:?}",
        b.reminders()
    );
}

/// `"todos"` is shared with subagents. A child calling `ask_user_question`
/// inside the parent's turn is the child handing *its* turn back — the parent
/// still owes its own bookkeeping.
#[tokio::test]
async fn a_child_ask_does_not_unlock_the_parent_gate() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", OPEN_LIST),
        1 => call_tool("c1", "run_child", "{}"),
        // Child turn: asks the user, then answers.
        2 => call_tool("c2", "ask_user_question", "{}"),
        3 => say("child-done"),
        _ => say(&format!("round-{i}")),
    })
    .await;
    let child = b.root.isolate("sessions");
    child
        .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-1"))
        .unwrap();
    let child_for_tool = child.clone();
    let body: cordis_spine::ToolBody = Arc::new(move |call: ToolCall| {
        let child = child_for_tool.clone();
        Box::pin(async move {
            let out = LoopHandle::new(child, Arc::new(cordis_spine::GrokStep))
                .run("子任务")
                .await;
            cordis_spine::ToolResult {
                call_id: call.id,
                name: call.name,
                content: format!("{out:?}"),
                ..Default::default()
            }
        })
    });
    let _runner = b
        .root
        .require::<cordis_spine::Tools>(TOOLS)
        .unwrap()
        .register(
            cordis_spine::ToolSpec {
                name: "run_child".into(),
                description: "run a child turn".into(),
                parameters_json: r#"{"type":"object","properties":{}}"#.into(),
            },
            body,
        )
        .unwrap();

    assert_eq!(b.run("干活").await, TurnOutcome::Text("round-4".into()));
    let child_kinds = child.require::<Sessions>(SESSIONS).unwrap().kinds();
    assert!(
        child_kinds.contains(&"tools/execute"),
        "the child really ran ask_user_question: {child_kinds:?}"
    );
    assert_eq!(
        b.samples(),
        6,
        "s4 is the parent's text, s5 its bookkeeping step"
    );
    assert_eq!(b.bookkeeping(), 1, "{:?}", b.reminders());
    assert_eq!(b.offered.lock().unwrap()[5], ["todo_write"]);
}

#[tokio::test]
async fn the_next_turn_reminds_about_items_the_last_turn_left_open() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", OPEN_LIST),
        _ => say(&format!("round-{i}")),
    })
    .await;
    // s1 text, s2 bookkeeping (ignored): both items still owed.
    b.run("干活").await;
    assert!(
        !b.reminders().iter().any(|r| r.contains("上轮清单")),
        "the first turn has no previous list"
    );
    b.run("换个方向：先写文档").await;
    let sessions = b.root.require::<Sessions>(SESSIONS).unwrap();
    let events = sessions.events();
    let second_user = events
        .iter()
        .rposition(|e| matches!(e, cordis_spine::LogEvent::User(_)))
        .unwrap();
    let carried: Vec<&String> = events[second_user..]
        .iter()
        .filter_map(|e| match e {
            cordis_spine::LogEvent::SystemReminder(t) if t.contains("上轮清单") => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(carried.len(), 1, "{carried:?}");
    let body = carried[0];
    assert!(body.contains("上轮清单还剩 2 项未完成"), "{body}");
    assert!(
        body.contains("in_progress: 读代码\npending: 改实现\n"),
        "{body}"
    );
    assert!(body.contains("用户的新消息优先"), "{body}");
    assert!(body.contains("先用 todo_write 更新或清空清单"), "{body}");
    // It lands before the second turn's first sample, not after it.
    let first_sample = events[second_user..]
        .iter()
        .position(|e| matches!(e, cordis_spine::LogEvent::LlmStream(_)))
        .unwrap();
    let at = events[second_user..]
        .iter()
        .position(
            |e| matches!(e, cordis_spine::LogEvent::SystemReminder(t) if t.contains("上轮清单")),
        )
        .unwrap();
    assert!(at < first_sample, "{:?}", sessions.kinds());
}

#[tokio::test]
async fn the_next_turn_stays_quiet_when_nothing_is_owed() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", OPEN_LIST),
        1 => call_tool("c1", "todo_write", CLOSE_LIST),
        _ => say(&format!("round-{i}")),
    })
    .await;
    b.run("干活").await;
    assert_eq!(b.todos().stats().completed, 2);
    b.run("再看看").await;
    assert!(
        !b.reminders()
            .iter()
            .any(|r| r.contains("上轮清单") || r.contains("awaiting_user")),
        "{:?}",
        b.reminders()
    );
}

/// `awaiting_user` items are not "left open", but the user's next message may
/// be the answer: the next turn says so instead of counting them.
#[tokio::test]
async fn the_next_turn_points_at_awaiting_user_items_without_counting_them() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", AWAITING_LIST),
        _ => say(&format!("round-{i}")),
    })
    .await;
    b.run("干活").await;
    b.run("可以，按方案做").await;
    let reminders = b.reminders();
    let carried: Vec<&String> = reminders
        .iter()
        .filter(|r| r.contains("在等用户（awaiting_user）"))
        .collect();
    assert_eq!(carried.len(), 1, "{reminders:?}");
    assert!(!carried[0].contains("上轮清单还剩"), "{}", carried[0]);
    assert!(
        carried[0].contains("「读代码」、「改实现」"),
        "{}",
        carried[0]
    );
    assert!(
        carried[0].contains("改回 pending / in_progress"),
        "{}",
        carried[0]
    );
}

/// The list just changed: the next step says where the model stands, as a
/// suggestion.
#[tokio::test]
async fn a_list_change_is_followed_by_a_focus_reminder() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", OPEN_LIST),
        1 => call_tool("c1", "edit_fake", "{}"),
        2 => call_tool(
            "c2",
            "todo_write",
            r#"{"todos":[{"id":"a","status":"completed"},{"id":"b","status":"in_progress"}]}"#,
        ),
        3 => call_tool(
            "c3",
            "todo_write",
            r#"{"todos":[{"id":"b","status":"completed"}]}"#,
        ),
        _ => say(&format!("round-{i}")),
    })
    .await;
    b.run("干活").await;
    let focus: Vec<String> = b
        .reminders()
        .into_iter()
        .filter(|r| r.contains("当前清单"))
        .collect();
    assert_eq!(focus.len(), 2, "{focus:?}");
    assert!(
        focus[0].contains("正在做「读代码」；下一项「改实现」。"),
        "{}",
        focus[0]
    );
    assert!(
        focus[1].contains("正在做「改实现」；这是最后一项。"),
        "{}",
        focus[1]
    );
    assert!(focus[0].contains("建议"), "{}", focus[0]);
    assert_eq!(b.bookkeeping(), 0, "everything was closed before the text");
}

/// An item is `in_progress`, a test command just ran and the list did not move
/// in that step: suggest ticking it off.
#[tokio::test]
async fn a_test_run_without_a_list_update_suggests_marking_it_done() {
    let b = boot_recorded(|i| match i {
        0 => call_tool("c0", "todo_write", OPEN_LIST),
        1 => call_tool(
            "c1",
            "bash",
            r#"{"command":"cd dock && cargo test -p cordis-spine"}"#,
        ),
        2 => call_tool("c2", "todo_write", CLOSE_LIST),
        _ => say(&format!("round-{i}")),
    })
    .await;
    b.run("干活").await;
    let nudges: Vec<String> = b
        .reminders()
        .into_iter()
        .filter(|r| r.contains("刚跑完测试 / 构建"))
        .collect();
    assert_eq!(nudges.len(), 1, "{:?}", b.reminders());
    assert!(
        nudges[0]
            .contains("（`cd dock && cargo test -p cordis-spine`），「读代码」还是 in_progress"),
        "{}",
        nudges[0]
    );
    assert!(nudges[0].contains("标 completed"), "{}", nudges[0]);
}

/// Reproduction of session 01a1d068: the model lists a plan and stops, the
/// user says 「先讨论方案」. The old turn-end gate ("Todos NOT complete —
/// continue working…") pushed the model into the work. Now the turn end only
/// gets a bookkeeping step that cannot run anything but `todo_write`.
#[tokio::test]
async fn after_lets_discuss_the_plan_first_the_model_does_not_start_working() {
    let b = boot_recorded(|i| match i {
        // Turn 1: plan as a list, then text asking whether to start.
        0 => call_tool("c0", "todo_write", OPEN_LIST),
        1 => say("方案：先读代码，再改实现。要开始吗？"),
        // Bookkeeping: the model ignores it.
        2 => say("好的"),
        // Turn 2, user: 「先讨论方案」. The model discusses.
        3 => say("好，先讨论：读代码这步要看哪些模块？"),
        // Bookkeeping again; the model tries to start editing anyway.
        4 => call_tool("c4", "edit_fake", "{}"),
        _ => say("不该到这里"),
    })
    .await;
    b.run("帮我把 todo 提醒重做一下").await;
    let out = b.run("先讨论方案").await;
    assert_eq!(
        out,
        TurnOutcome::Text("好，先讨论：读代码这步要看哪些模块？".into())
    );
    assert_eq!(
        b.samples(),
        5,
        "each turn: one text + one bookkeeping step, no work"
    );
    assert!(
        !b.reminders()
            .iter()
            .any(|r| r.contains("continue working") || r.contains("别停在这里向用户汇报")),
        "{:?}",
        b.reminders()
    );
    let offered = b.offered.lock().unwrap().clone();
    assert_eq!(offered[2], ["todo_write"]);
    assert_eq!(offered[4], ["todo_write"]);
    let worked: Vec<_> = b
        .tool_rows()
        .into_iter()
        .filter(|(name, _, is_error)| name != "todo_write" && !is_error)
        .collect();
    assert!(worked.is_empty(), "no work tool ran: {worked:?}");
}

/// `"goal"` is not isolated per subagent either (`ChildRunner` isolates
/// `sessions` / `turn` / `agentPresets`), so an active parent goal must not
/// push a child turn to the hard stop.
#[tokio::test]
async fn a_subagent_turn_is_not_gated_by_the_parent_goal() {
    let samples = Arc::new(AtomicUsize::new(0));
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(tool_goal(), ()).unwrap().wait().await.unwrap();
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(TodoScript {
                n: samples.clone(),
                writes: Vec::new(),
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root.require::<Goal>(GOAL).unwrap().start("交付这次改动");

    let child = root.isolate("sessions");
    child
        .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-1"))
        .unwrap();
    let out = LoopHandle::new(child.clone(), Arc::new(cordis_spine::GrokStep))
        .run("子任务")
        .await
        .unwrap();
    assert_eq!(
        out,
        TurnOutcome::Text("round-0".into()),
        "a child turn must not be continued by the parent's goal"
    );
    assert_eq!(
        samples.load(Ordering::SeqCst),
        1,
        "child must end on its first text sample"
    );
}

/// A catalog reminder names its loader (`skill` / `workflow`). Sending it to
/// a preset that cannot call the loader burns tokens and invites a
/// guaranteed-failed call: the sampler filters it out, `search_tool` no longer
/// indexes it (those tools stopped being deferred), and `use_tool` hits the
/// same allowlist.
///
/// Parent side of the invariant that `a_listing_role_holds_the_tools_its_
/// listing_names` guards for child roles.
#[tokio::test]
async fn main_session_catalogs_follow_the_preset_allowlist() {
    isolated_home();
    let root = Context::new();
    cordis_spine::install_app(&root).await.unwrap();
    let presets = root
        .require::<cordis_spine::AgentPresets>(cordis_spine::AGENT_PRESETS)
        .unwrap();
    let book = root
        .require::<cordis_spine::ContextBook>(cordis_spine::CONTEXT)
        .unwrap();
    let identity = root
        .require::<Sessions>(SESSIONS)
        .unwrap()
        .identity()
        .to_string();

    for (mode, want, want_roster) in [
        ("code", true, true),
        ("cordis", true, true),
        ("minimal", false, false),
        ("warden", false, true),
    ] {
        presets.apply(mode).unwrap();
        let assembly = book.assemble();
        let ids: Vec<String> = assembly.inspect().into_iter().map(|p| p.id).collect();
        assert!(
            !ids.iter().any(|id| id == "cordis"),
            "mode {mode}: cordis 写法在技能里，不进系统提示: {ids:?}"
        );
        assert!(
            !assembly.render().contains("动态 Host 扩展"),
            "mode {mode} still injects the cordis system pointer"
        );
        let start = StepStart::new(0, identity.clone());
        let seed = start.clone();
        let text = root
            .waterfall(STEP_START, start, move || seed)
            .into_reminders()
            .join("\n");
        for (listing, tool) in [("可用技能", "skill"), ("可用工作流", "workflow")] {
            let present = text.contains(listing);
            assert_eq!(
                present,
                want && presets.allows(tool),
                "mode {mode}: {listing} present={present} but allows({tool})={}: {text}",
                presets.allows(tool)
            );
            assert!(
                !assembly.render().contains(listing),
                "mode {mode}: {listing} must not ride the system prompt"
            );
        }
        assert_eq!(
            text.contains("# 子代理名册"),
            want_roster,
            "mode {mode}: roster reminder must follow whether task is callable: {text}"
        );
        let task = root
            .require::<cordis_spine::Tools>(cordis_spine::TOOLS)
            .unwrap()
            .specs_for_model_on(&root)
            .into_iter()
            .find(|s| s.name == "task");
        if let Some(task) = task {
            assert!(
                !task.description.contains("subagent_type is a role id"),
                "mode {mode}: roster must not ride the task description"
            );
        }
    }
}

/// 目标模式按脚本走：`work(i)` 为真的那次采样调一个干活的工具（`list_dir`），否则只回文字。
/// `done_at` 那次采样报完成，之后只回文字。
struct GoalScript {
    n: Arc<AtomicUsize>,
    work: fn(usize) -> bool,
    done_at: Option<usize>,
}

impl Sampler for GoalScript {
    fn sample<'a>(
        &'a self,
        _request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let i = self.n.fetch_add(1, Ordering::SeqCst);
        let (name, arguments) = if Some(i) == self.done_at {
            ("update_goal", r#"{"completed":true,"message":"做完了"}"#)
        } else if (self.work)(i) && self.done_at.is_none_or(|d| i < d) {
            ("list_dir", "{}")
        } else {
            return Box::pin(async move {
                LlmOutput {
                    text: format!("round-{i}"),
                    ..LlmOutput::default()
                }
            });
        };
        Box::pin(async move {
            LlmOutput {
                tool_calls: vec![ToolCall {
                    id: format!("g{i}"),
                    name: name.into(),
                    arguments: arguments.into(),
                }],
                ..LlmOutput::default()
            }
        })
    }
}

async fn boot_goal_script(
    samples: Arc<AtomicUsize>,
    work: fn(usize) -> bool,
    done_at: Option<usize>,
) -> Context {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(tool_goal(), ()).unwrap().wait().await.unwrap();
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(GoalScript {
                n: samples,
                work,
                done_at,
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root.require::<Goal>(GOAL).unwrap().start("把这件事做完");
    root
}

fn reminders_containing(root: &Context, needle: &str) -> Vec<String> {
    root.require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .filter_map(|e| match e {
            cordis_spine::LogEvent::SystemReminder(t) if t.contains(needle) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// 只回文字、从不动手的目标是空转：催 `STALL_LIMIT - 1` 次还不动就暂停，并写明原因。
/// 以前没有这道检查，这样的目标会一直续到 64 轮上限，整轮结束后再排下一轮，永不停。
#[tokio::test]
async fn an_idle_goal_pauses_after_a_few_nudges() {
    let samples = Arc::new(AtomicUsize::new(0));
    let root = boot_goal_script(samples.clone(), |_| false, None).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("开始")
        .await
        .unwrap();
    let limit = cordis_spine::STALL_LIMIT as usize;
    assert_eq!(out, TurnOutcome::Text(format!("round-{}", limit - 1)));
    assert_eq!(
        samples.load(Ordering::SeqCst),
        limit,
        "催了 {} 次",
        limit - 1
    );
    let goal = root.require::<Goal>(GOAL).unwrap();
    assert!(goal.paused() && !goal.active(), "空转该暂停，不是清掉");
    assert!(goal.status().contains("空转"), "{}", goal.status());
    // 恢复后从头数：又能催满一轮。
    assert!(goal.resume());
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("继续")
        .await
        .unwrap();
    assert_eq!(samples.load(Ordering::SeqCst), 2 * limit);
}

/// 真在干活的目标不因空转检查停下，多少轮都行，直到它自己报完成。
#[tokio::test]
async fn a_working_goal_is_not_stopped_for_stalling() {
    let samples = Arc::new(AtomicUsize::new(0));
    // 一次动手、一次回文字，交替 20 轮后报完成。
    let root = boot_goal_script(samples.clone(), |i| i.is_multiple_of(2), Some(40)).await;
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("开始")
        .await
        .unwrap();
    let goal = root.require::<Goal>(GOAL).unwrap();
    assert!(goal.completed(), "status={}", goal.status());
    assert!(!goal.paused());
    // 第 41 次（报完成那次）之后又采了一次收尾的文字，那次目标已经不在跑，不记。
    assert_eq!(samples.load(Ordering::SeqCst), 42);
    assert_eq!(goal.requests(), 41);
}

/// 只报进度（`update_goal(message)`）不算动手：光写进展的目标照样算空转。
#[tokio::test]
async fn progress_notes_alone_count_as_idle() {
    let samples = Arc::new(AtomicUsize::new(0));
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(tool_goal(), ()).unwrap().wait().await.unwrap();
    struct NotesOnly(Arc<AtomicUsize>);
    impl Sampler for NotesOnly {
        fn sample<'a>(
            &'a self,
            _request: PromptRequest,
            _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            let i = self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if i.is_multiple_of(2) {
                    LlmOutput {
                        tool_calls: vec![ToolCall {
                            id: format!("n{i}"),
                            name: "update_goal".into(),
                            arguments: r#"{"message":"还在推进"}"#.into(),
                        }],
                        ..LlmOutput::default()
                    }
                } else {
                    LlmOutput {
                        text: format!("round-{i}"),
                        ..LlmOutput::default()
                    }
                }
            })
        }
    }
    root.provide(
        LLM,
        Llm::from_sampler(root.clone(), Arc::new(NotesOnly(samples.clone()))),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root.require::<Goal>(GOAL).unwrap().start("把这件事做完");
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("开始")
        .await
        .unwrap();
    let goal = root.require::<Goal>(GOAL).unwrap();
    assert!(goal.paused(), "status={}", goal.status());
    assert_eq!(
        samples.load(Ordering::SeqCst),
        2 * cordis_spine::STALL_LIMIT as usize
    );
}

/// 每用掉 `CHECKIN_EVERY` 次请求插一条自查提醒，让模型对照完成标准自己收尾。
#[tokio::test]
async fn a_long_goal_gets_a_checkin_every_batch_of_requests() {
    let samples = Arc::new(AtomicUsize::new(0));
    let every = cordis_spine::CHECKIN_EVERY as usize;
    let root = boot_goal_script(samples.clone(), |_| true, Some(2 * every + 5)).await;
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("开始")
        .await
        .unwrap();
    let checkins = reminders_containing(&root, "<goal-checkin>");
    assert_eq!(checkins.len(), 2, "{checkins:?}");
    assert!(
        checkins[0].contains(&format!("{every} 次请求")),
        "{}",
        checkins[0]
    );
    assert!(checkins[0].contains("完成标准"), "{}", checkins[0]);
    assert!(root.require::<Goal>(GOAL).unwrap().completed());
}

/// 用户设的预算：到了就暂停（在干活也停），预算清掉，恢复后不再限。
#[tokio::test]
async fn a_request_budget_pauses_a_busy_goal() {
    let samples = Arc::new(AtomicUsize::new(0));
    let root = boot_goal_script(samples.clone(), |i| i.is_multiple_of(2), None).await;
    let goal = root.require::<Goal>(GOAL).unwrap();
    assert!(goal.set_budget(Some(cordis_spine::GoalBudget::Requests(6))));
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("开始")
        .await
        .unwrap();
    assert_eq!(samples.load(Ordering::SeqCst), 6);
    assert!(goal.paused(), "status={}", goal.status());
    assert!(goal.status().contains("预算"), "{}", goal.status());
    assert_eq!(goal.budget(), None, "到了就清掉，恢复后不再限");
}
