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
            "llm/stream"
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
    root.on_waterfall(PRE_STEP, |step: PreStep, _| PreStep {
        enter: false,
        ..step
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
            sessions.append(cordis_spine::LogEvent::User(prompt));
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
        "report",
        "lsp",
        "skill",
        "update_goal",
        "memory_search",
        "memory_get",
        "monitor",
        "workflow",
        "search_tool",
        "use_tool",
        "cordis_inspect",
        "cordis_inspect_self",
        "cordis_define",
        "cordis_run",
        "cordis_call",
        "cordis_stop",
        "cordis_undefine",
        "cordis_promote",
        "browser_open",
        "browser_navigate",
        "browser_navigate_back",
        "browser_snapshot",
        "browser_click",
        "browser_hover",
        "browser_type",
        "browser_press_key",
        "browser_select_option",
        "browser_fill_form",
        "browser_wait_for",
        "browser_drag",
        "browser_handle_dialog",
        "browser_file_upload",
        "browser_resize",
        "browser_screenshot",
        "browser_tabs",
        "browser_evaluate",
        "browser_console_messages",
        "browser_network_requests",
        "browser_close",
    ] {
        assert!(
            names.iter().any(|n| n == need),
            "missing {need} in {names:?}"
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
    // `skill` / `workflow` are named by the system-prompt listings, so they
    // have to be callable without a `search_tool` round-trip first. Everything
    // else that is only reachable through discovery stays off the sampler.
    for listed in ["skill", "workflow"] {
        assert!(
            model.iter().any(|n| n == listed),
            "sampler must expose the listed tool {listed}: {model:?}"
        );
    }
    for hidden in [
        "scheduler_create",
        "memory_search",
        "monitor",
        "update_goal",
        "lsp",
        "cordis_inspect",
        "browser_open",
        "browser_navigate",
        "browser_navigate_back",
        "browser_snapshot",
        "browser_click",
        "browser_hover",
        "browser_type",
        "browser_press_key",
        "browser_select_option",
        "browser_fill_form",
        "browser_wait_for",
        "browser_drag",
        "browser_handle_dialog",
        "browser_file_upload",
        "browser_resize",
        "browser_screenshot",
        "browser_tabs",
        "browser_evaluate",
        "browser_console_messages",
        "browser_network_requests",
        "browser_close",
    ] {
        assert!(
            !model.iter().any(|n| n == hidden),
            "sampler must hide deferred {hidden}: {model:?}"
        );
    }
    let sched_search = tools
        .execute(cordis_spine::ToolCall {
            id: "st-sched".into(),
            name: "search_tool".into(),
            arguments: r#"{"query":"scheduler","limit":5}"#.into(),
        })
        .await;
    assert!(
        sched_search.content.contains("scheduler_create"),
        "search_tool should surface deferred locals: {}",
        sched_search.content
    );
    let browser_search = tools
        .execute(cordis_spine::ToolCall {
            id: "st-browser".into(),
            name: "search_tool".into(),
            arguments: r#"{"query":"browser","limit":30}"#.into(),
        })
        .await;
    for need in [
        "browser_open",
        "browser_navigate",
        "browser_navigate_back",
        "browser_snapshot",
        "browser_click",
        "browser_hover",
        "browser_type",
        "browser_press_key",
        "browser_select_option",
        "browser_fill_form",
        "browser_wait_for",
        "browser_drag",
        "browser_handle_dialog",
        "browser_file_upload",
        "browser_resize",
        "browser_screenshot",
        "browser_tabs",
        "browser_evaluate",
        "browser_console_messages",
        "browser_network_requests",
        "browser_close",
    ] {
        assert!(
            browser_search.content.contains(need),
            "search_tool should surface deferred {need}: {}",
            browser_search.content
        );
    }
    // Session stays Closed until browser_open; snapshot without open needs no Chromium.
    let browser_closed = tools
        .execute(cordis_spine::ToolCall {
            id: "br-snap".into(),
            name: "browser_snapshot".into(),
            arguments: r#"{"interactive":true}"#.into(),
        })
        .await;
    assert!(
        browser_closed.content.contains("not connected"),
        "browser closed snapshot: {}",
        browser_closed.content
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
    assert!(assembled.contains("search_tool 查 cordis"), "{assembled}");
    assert!(!assembled.contains("update_goal(objective"), "{assembled}");
    assert!(root.get::<cordis_spine::Ask>(cordis_spine::ASK).is_some());
    assert!(root
        .get::<cordis_spine::Compact>(cordis_spine::COMPACT)
        .is_some());
    assert!(root
        .get::<cordis_spine::DynamicRunner>(cordis_spine::DYNAMIC_CORDIS_RUNNER)
        .is_some());
    assert!(root
        .get::<cordis_spine::LspBackendAdapter>(cordis_spine::LSP)
        .is_some());
    assert!(root
        .get::<cordis_spine::Browser>(cordis_spine::BROWSER)
        .is_some_and(|b| b.session() == cordis_spine::BrowserSession::Closed));
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

    let blocked = tools
        .execute(cordis_spine::ToolCall {
            id: "b1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"echo should-block"}"#.into(),
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
            && started.content.contains("subagent_id:")
            && started.content.contains("use job with job_ids="),
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

#[tokio::test]
async fn open_todos_gate_the_turn_until_they_are_closed() {
    let samples = Arc::new(AtomicUsize::new(0));
    // s0 opens the list, s1 answers with text (gate fires), s2 closes the
    // list, s3 answers again and is allowed through.
    let root = boot_todo_loop(samples.clone(), vec![(0, OPEN_LIST), (2, CLOSE_LIST)]).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("round-3".into()));
    assert_eq!(
        samples.load(Ordering::SeqCst),
        4,
        "text with open todos must not end the turn"
    );
    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    let reminders: Vec<String> = sessions
        .events()
        .iter()
        .filter_map(|e| match e {
            cordis_spine::LogEvent::SystemReminder(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(reminders.len(), 1, "one nudge, not one per remaining item");
    assert!(reminders[0].contains(cordis_spine::TODO_GATE_SENTINEL));
    assert!(reminders[0].contains("读代码"), "{}", reminders[0]);
    let users = sessions.kinds().iter().filter(|k| **k == "user").count();
    assert_eq!(users, 1, "the gate is a hidden reminder, not a user bubble");
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
}

#[tokio::test]
async fn the_gate_gives_up_after_two_nudges() {
    let samples = Arc::new(AtomicUsize::new(0));
    // The model never closes the list; the turn must still end.
    let root = boot_todo_loop(samples.clone(), vec![(0, OPEN_LIST)]).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("干活")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("round-3".into()));
    assert_eq!(
        samples.load(Ordering::SeqCst),
        4,
        "one tool round + text + two gated rounds"
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
/// be charged when it wins, not when it votes. A lower slot taking the round
/// must not quietly spend the todo gate's two nudges.
#[tokio::test]
async fn a_lower_slot_winner_does_not_spend_the_todo_quota() {
    let samples = Arc::new(AtomicUsize::new(0));
    // s0 opens the list; every later sample is text the gate would object to.
    let root = boot_todo_loop(samples.clone(), vec![(0, OPEN_LIST)]).await;
    // Outranks the gate for the first two endings, then falls silent.
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
    // Two rounds taken by the hog, then the gate's own two, then the turn ends.
    assert_eq!(out, TurnOutcome::Text("round-5".into()));
    assert_eq!(samples.load(Ordering::SeqCst), 6);
    let gate_nudges = root
        .require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .filter(|e| {
            matches!(e, cordis_spine::LogEvent::SystemReminder(t)
                if t.contains(cordis_spine::TODO_GATE_SENTINEL))
        })
        .count();
    assert_eq!(
        gate_nudges, 2,
        "the gate still gets its full quota after losing two rounds"
    );
}

/// `agent/step-start` is an extension point, not a loop private: a handler
/// mounted from outside sees every step numbered from 0 — across turn-end
/// continuations too — and the loop appends what it queues, in slot order.
#[tokio::test]
async fn a_plugin_can_inject_a_reminder_before_every_step() {
    let samples = Arc::new(AtomicUsize::new(0));
    // s0 opens the list, s1..s3 answer with text; the gate continues the turn
    // twice, so steps 2 and 3 are on the far side of an `agent/turn-end`.
    let root = boot_todo_loop(samples.clone(), vec![(0, OPEN_LIST)]).await;
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
    assert_eq!(
        *seen.lock().unwrap(),
        vec![0, 1, 2, 3],
        "one call per sample, counting on across turn-end continuations"
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
        let want = if i % 2 == 0 { '早' } else { '晚' };
        assert!(body.contains(want), "step {}: {body}", i / 2);
    }
}

/// Both `agent/turn-end` handlers want another round. The order slot decides,
/// not the mount order: "advance the next todo" is more actionable than
/// "keep pushing the goal", so todo (10) beats goal (20) either way.
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
async fn the_lowest_order_slot_wins_regardless_of_mount_order() {
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
                    if t.contains(cordis_spine::TODO_GATE_SENTINEL)
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
            first.contains(cordis_spine::TODO_GATE_SENTINEL),
            "todo (order 10) must outrank goal (order 20), todo_first={todo_first}: {first}"
        );
        assert!(
            !first.contains("<goal-state>"),
            "only the winning reminder is appended: {first}"
        );
    }
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

/// A listing header names its loader (`skill` / `workflow`). Advertising it to
/// a preset that cannot call it burns tokens and invites a guaranteed-failed
/// call: the sampler filters it out, `search_tool` no longer indexes it (those
/// tools stopped being deferred), and `use_tool` hits the same allowlist.
///
/// Parent side of the invariant that `a_listing_role_holds_the_tools_its_
/// listing_names` guards for child roles.
#[tokio::test]
async fn main_session_listings_follow_the_preset_allowlist() {
    isolated_home();
    let root = Context::new();
    cordis_spine::install_app(&root).await.unwrap();
    let presets = root
        .require::<cordis_spine::AgentPresets>(cordis_spine::AGENT_PRESETS)
        .unwrap();
    let book = root
        .require::<cordis_spine::ContextBook>(cordis_spine::CONTEXT)
        .unwrap();

    for (mode, want) in [
        ("code", true),
        ("cordis", true),
        ("minimal", false),
        ("warden", false),
    ] {
        presets.apply(mode).unwrap();
        let ids: Vec<String> = book
            .assemble()
            .inspect()
            .into_iter()
            .map(|p| p.id)
            .collect();
        for (section, tool) in [("skills", "skill"), ("workflows", "workflow")] {
            let present = ids.iter().any(|id| id == section);
            assert_eq!(
                present,
                want && presets.allows(tool),
                "mode {mode}: {section} section present={present} but allows({tool})={}: {ids:?}",
                presets.allows(tool)
            );
        }
    }
}
