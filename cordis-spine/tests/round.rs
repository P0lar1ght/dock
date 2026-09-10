use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cordis::{plugin, Context, FiberState, Inject};
use cordis_spine::{
    agent_loop, install_fakes, install_without_llm, tool_goal, turn, Agent, Agents, BoxFuture,
    Driver, Error, Goal, Llm, LlmOutput, LoopHandle, PreStep, PromptRequest, Sampler, Sessions,
    StreamDelta, ToolCall, TurnControl, TurnOutcome, AGENTS, AGENT_LOOP, GOAL, LLM, PRE_STEP,
    SESSIONS, SYSTEM_PROMPT, TOOLS, TURN,
};

/// 进程级 env（`DOCK_HOME`）的共享锁：本文件里有一个测试会临时改它，
/// 其它测试必须读到稳定的值，所以大家都先拿这把锁。
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 独占 env 并把 `DOCK_HOME` 指向临时目录。
struct HomeGuard {
    prev: Option<std::ffi::OsString>,
    _dir: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("DOCK_HOME", v),
            None => std::env::remove_var("DOCK_HOME"),
        }
        // 字段按声明顺序释放：临时目录先删，锁最后放。
    }
}

fn lock_home() -> HomeGuard {
    let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("DOCK_HOME");
    std::env::set_var("DOCK_HOME", dir.path());
    HomeGuard {
        prev,
        _dir: dir,
        _lock: lock,
    }
}

async fn boot() -> Context {
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root
}

#[tokio::test]
async fn loop_waits_for_all_five() {
    let _env = env_lock();
    let root = Context::new();
    let fiber = root.plugin(agent_loop(), ()).unwrap();
    assert_eq!(fiber.state(), FiberState::Pending);
    install_fakes(&root).await.unwrap();
    fiber.wait().await.unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
}

#[tokio::test]
async fn one_round_writes_log_in_order() {
    let _env = env_lock();
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
    let _env = env_lock();
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    root.on_waterfall(PRE_STEP, |_: PreStep, _| PreStep {
        user: String::new(),
        enter: false,
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
    let _env = env_lock();
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
    let _env = env_lock();
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
    let _env = env_lock();
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
    let _env = env_lock();
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
    let _env = env_lock();
    let root = boot().await;
    let tools = root.require::<cordis_spine::Tools>(TOOLS).unwrap();
    let names: Vec<String> = tools.specs().into_iter().map(|s| s.name).collect();
    for banned in [
        "web_fetch",
        "todo_write",
        "ask_user_question",
        "enter_plan_mode",
        "get_task_output",
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

#[tokio::test]
async fn install_app_registers_capability_tools_and_mcp_fail_open() {
    // Isolated home so a live `roster.yml` in the real `DOCK_HOME` cannot
    // filter out enter_plan_mode / task on the default code preset.
    let _home = lock_home();
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
        "get_task_output",
        "wait_tasks",
        "kill_task",
        "scheduler_create",
        "scheduler_list",
        "scheduler_delete",
        "bash",
        "list_dir",
        "task",
        "subagent",
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
    for hidden in [
        "scheduler_create",
        "memory_search",
        "monitor",
        "update_goal",
        "lsp",
        "skill",
        "workflow",
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
            && started.content.contains("get_task_output"),
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
    let _env = env_lock();
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
        kinds.iter().any(|k| *k == "system-reminder"),
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
    let _env = env_lock();
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
    let _env = env_lock();
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
