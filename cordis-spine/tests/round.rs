use std::sync::Arc;

use cordis::{plugin, Context, FiberState, Inject};
use cordis_spine::{
    agent_loop, install_fakes, turn, Agent, Agents, BoxFuture, Driver, Error, LoopHandle, PreStep,
    Sessions, TurnControl, TurnOutcome, AGENT_LOOP, AGENTS, LLM, PRE_STEP, SESSIONS, SYSTEM_PROMPT,
    TOOLS, TURN,
};

async fn boot() -> Context {
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    root.plugin(agent_loop(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root
}

#[tokio::test]
async fn loop_waits_for_all_five() {
    let root = Context::new();
    let fiber = root.plugin(agent_loop(), ()).unwrap();
    assert_eq!(fiber.state(), FiberState::Pending);
    install_fakes(&root).await.unwrap();
    fiber.wait().await.unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
}

#[tokio::test]
async fn one_round_writes_log_in_order() {
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
    let root = Context::new();
    install_fakes(&root).await.unwrap();
    root.on_waterfall(PRE_STEP, |_: PreStep, _| PreStep {
        user: String::new(),
        enter: false,
    })
    .unwrap();
    root.plugin(agent_loop(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();

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
            let out = llm.stream(cordis_spine::PromptRequest {
                system: assembled,
                history: sessions.events(),
                tools: Vec::new(),
            }).await;
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
    assert_eq!(
        kinds.iter().filter(|k| **k == "llm/stream").count(),
        3
    );
    assert_eq!(
        kinds.iter().filter(|k| **k == "tools/execute").count(),
        1
    );
}

#[tokio::test]
async fn second_turn_does_not_reuse_previous_tool_result() {
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
    plugin(
        "extra-ping",
        Inject::from(["tools"]),
        |ctx, _: &()| {
            let tools = ctx.require::<cordis_spine::Tools>(cordis_spine::TOOLS).unwrap();
            let body: cordis_spine::ToolBody = std::sync::Arc::new(|call| {
                Box::pin(async move {
                    cordis_spine::ToolResult {
                        call_id: call.id,
                        name: call.name,
                        content: "pong".into(),
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
        },
    )
}

#[tokio::test]
async fn tools_register_is_live_and_disposed_with_fiber() {
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
    ] {
        assert!(
            !names.iter().any(|n| n == banned),
            "{banned} must not be on echo tools: {names:?}"
        );
    }
}

#[tokio::test]
async fn install_app_registers_capability_tools_and_mcp_fail_open() {
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
        "lsp",
        "update_goal",
        "memory_search",
        "memory_get",
        "monitor",
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
    assert!(root.get::<cordis_spine::PlanMode>(cordis_spine::PLAN_MODE).is_some());
    assert!(root.get::<cordis_spine::Todos>(cordis_spine::TODOS).is_some());
    assert!(root.get::<cordis_spine::Jobs>(cordis_spine::JOBS).is_some());
    assert!(root.get::<cordis_spine::Ask>(cordis_spine::ASK).is_some());

    let todo = tools
        .execute(cordis_spine::ToolCall {
            id: "t1".into(),
            name: "todo_write".into(),
            arguments: r#"{"merge":false,"todos":[{"id":"1","content":"copied grok merge","status":"pending"}]}"#.into(),
        })
        .await;
    assert!(todo.content.contains("copied grok merge"), "{}", todo.content);

    let plan = tools
        .execute(cordis_spine::ToolCall {
            id: "p1".into(),
            name: "enter_plan_mode".into(),
            arguments: "{}".into(),
        })
        .await;
    assert!(plan.content.contains("exit_plan_mode"), "{}", plan.content);
    assert!(plan.content.contains("ask_user_question"), "{}", plan.content);
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
        blocked.content.contains("blocked in plan mode"),
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

    root.get::<cordis_spine::Goal>(cordis_spine::GOAL)
        .unwrap()
        .start("ship lsp");
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
}
