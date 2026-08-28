use std::sync::Arc;

use cordis::{plugin, Context, FiberState, Inject};
use cordis_spine::{
    agent_loop, install_fakes, Agent, Agents, BoxFuture, Driver, Error, LoopHandle, PreStep,
    Sessions, TurnOutcome, AGENT_LOOP, AGENTS, LLM, PRE_STEP, SESSIONS, SYSTEM_PROMPT, TOOLS,
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
            sessions.append(cordis_spine::LogEvent::LlmStream(out.clone()));
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

struct ExtraPing;

impl cordis_spine::ExtraTools for ExtraPing {
    fn handles(&self, name: &str) -> bool {
        name == "docs__ping"
    }

    fn specs(&self) -> Vec<cordis_spine::ToolSpec> {
        vec![cordis_spine::ToolSpec {
            name: "docs__ping".into(),
            description: "ping".into(),
            parameters_json: r#"{"type":"object"}"#.into(),
        }]
    }

    fn execute(&self, call: cordis_spine::ToolCall) -> BoxFuture<'_, cordis_spine::ToolResult> {
        Box::pin(async move {
            cordis_spine::ToolResult {
                call_id: call.id,
                name: call.name,
                content: "pong".into(),
            }
        })
    }
}

fn extra_mcp() -> cordis::Plugin {
    plugin(
        "fake-mcp",
        Inject::new(),
        |ctx, _: &()| {
            Ok(Some(ctx.provide(
                cordis_spine::TOOLS_MCP,
                cordis_spine::ExtraToolsHandle::new(ExtraPing),
            )?))
        },
    )
}

#[tokio::test]
async fn tools_live_looks_up_extra_mcp() {
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

    root.plugin(extra_mcp(), ()).unwrap().wait().await.unwrap();
    assert!(tools.specs().iter().any(|s| s.name == "docs__ping"));
    let after = tools
        .execute(ToolCall {
            id: "2".into(),
            name: "docs__ping".into(),
            arguments: "echo-me".into(),
        })
        .await;
    assert_eq!(after.content, "pong");
}
