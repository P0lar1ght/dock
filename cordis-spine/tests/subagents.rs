//! Mode-directory roster + continuable mailbox (queued / urgent / interrupt / report).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use cordis_spine::{
    agent_loop, blocked_tool_message, install_without_llm, tool_subagent, tool_task, turn,
    AgentPresets, BoxFuture, Llm, LlmOutput, LogEvent, PromptRequest, Sampler, Sessions,
    StreamDelta, Subagents, ToolCall, Tools, AGENT_LOOP, AGENT_PRESETS, LLM, SESSIONS, SUBAGENTS,
    TOOLS,
};
use tokio::sync::Notify;

struct Harness {
    root: Context,
    _home: tempfile::TempDir,
}

struct LastUser;

impl Sampler for LastUser {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        Box::pin(async move {
            let text = last_user(&request.history);
            if !text.is_empty() {
                on_delta(StreamDelta::Text(text.clone()));
            }
            LlmOutput {
                text,
                ..LlmOutput::default()
            }
        })
    }
}

struct Gate {
    started: AtomicBool,
    release: Notify,
}

struct GatedLastUser {
    gate: Arc<Gate>,
    first: AtomicBool,
}

impl Sampler for GatedLastUser {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let gate = self.gate.clone();
        let wait = self.first.swap(false, Ordering::SeqCst);
        Box::pin(async move {
            if wait {
                let notified = gate.release.notified();
                gate.started.store(true, Ordering::SeqCst);
                notified.await;
            }
            let text = last_user(&request.history);
            if !text.is_empty() {
                on_delta(StreamDelta::Text(text.clone()));
            }
            LlmOutput {
                text,
                ..LlmOutput::default()
            }
        })
    }
}

fn last_user(history: &[LogEvent]) -> String {
    history
        .iter()
        .rev()
        .find_map(|e| match e {
            LogEvent::User(text) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

async fn boot(sampler: Arc<dyn Sampler>) -> Harness {
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(turn(), ()).unwrap().wait().await.unwrap();
    let home = tempfile::tempdir().unwrap();
    root.provide(AGENT_PRESETS, AgentPresets::load(home.path().to_path_buf()))
        .unwrap();
    root.plugin(tool_task(), ()).unwrap().wait().await.unwrap();
    root.plugin(tool_subagent(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.provide(LLM, Llm::from_sampler(root.clone(), sampler))
        .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    let _ = root
        .require::<cordis_spine::LoopHandle>(AGENT_LOOP)
        .unwrap();
    Harness { root, _home: home }
}

fn parse_id(text: &str) -> String {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("subagent_id:"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| panic!("no subagent_id in {text}"))
}

async fn spawn_bg(tools: &Tools, prompt: &str, subagent_type: &str) -> String {
    let started = tools
        .execute(ToolCall {
            id: "t0".into(),
            name: "subagent".into(),
            arguments: serde_json::json!({
                "prompt": prompt,
                "description": "mailbox test",
                "subagent_type": subagent_type,
                "run_in_background": true,
            })
            .to_string(),
        })
        .await;
    assert!(
        started.content.contains("Subagent started in background")
            && started.content.contains("send_message")
            && !started.content.contains("get_task_output"),
        "{}",
        started.content
    );
    parse_id(&started.content)
}

async fn wait_idle(sub: &Subagents, id: &str) -> cordis_spine::SubagentSnap {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(s) = sub.snapshot(id) {
            if !s.running() {
                return s;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("subagent {id} still running");
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

async fn wait_running(sub: &Subagents, id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if sub.snapshot(id).is_some_and(|s| s.running()) {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("subagent {id} never started");
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

async fn wait_output_contains(sub: &Subagents, id: &str, needle: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(s) = sub.snapshot(id) {
            if s.output.contains(needle) && !s.running() {
                return s.output;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let got = sub.snapshot(id).map(|s| s.output).unwrap_or_default();
            panic!("timeout waiting for {needle:?} in {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

#[tokio::test]
async fn unknown_type_lists_roster() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let out = tools
        .execute(ToolCall {
            id: "bad".into(),
            name: "task".into(),
            arguments: r#"{"prompt":"x","description":"bad type","subagent_type":"not-a-type"}"#
                .into(),
        })
        .await;
    assert!(
        out.content.contains("Unknown subagent type") && out.content.contains("explore"),
        "{}",
        out.content
    );
    let via_subagent = tools
        .execute(ToolCall {
            id: "bad2".into(),
            name: "subagent".into(),
            arguments: r#"{"prompt":"x","description":"bad type","subagent_type":"not-a-type"}"#
                .into(),
        })
        .await;
    assert!(
        via_subagent.content.contains("Unknown subagent type")
            && via_subagent.content.contains("explore"),
        "{}",
        via_subagent.content
    );
}

#[tokio::test]
async fn empty_roster_with_task_allowed_errors() {
    let h = boot(Arc::new(LastUser)).await;
    let home = &h._home;
    std::fs::write(
        home.path().join("solo.yml"),
        "name: 独\ntools:\n  - task\n  - send_message\n  - list_agents\n  - interrupt_agent\n",
    )
    .unwrap();
    let presets = h.root.require::<AgentPresets>(AGENT_PRESETS).unwrap();
    presets.resync();
    presets.apply("solo").unwrap();
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let out = tools
        .execute(ToolCall {
            id: "m0".into(),
            name: "task".into(),
            arguments: r#"{"prompt":"x","description":"none"}"#.into(),
        })
        .await;
    assert!(out.content.contains("no subagents"), "{}", out.content);
}

#[tokio::test]
async fn explore_overlay_blocks_write_file() {
    let h = boot(Arc::new(LastUser)).await;
    let def = h
        .root
        .require::<AgentPresets>(AGENT_PRESETS)
        .unwrap()
        .subagent("explore")
        .expect("shipped explore");
    let child = h.root.isolate("agentPresets");
    child
        .provide(
            AGENT_PRESETS,
            AgentPresets::overlay(def.to_preset("explore")),
        )
        .unwrap();
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let blocked = tools
        .execute_on(
            &child,
            ToolCall {
                id: "w".into(),
                name: "write_file".into(),
                arguments: r#"{"target_file":"x.rs","contents":"nope"}"#.into(),
            },
        )
        .await;
    assert_eq!(blocked.content, blocked_tool_message());
    let allowed = tools
        .execute_on(
            &child,
            ToolCall {
                id: "r".into(),
                name: "read_file".into(),
                arguments: r#"{"target_file":"README.md"}"#.into(),
            },
        )
        .await;
    assert_ne!(allowed.content, blocked_tool_message());
}

#[tokio::test]
async fn queued_followup_is_next_turn_without_envelope() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "general-purpose").await;
    wait_idle(&sub, &id).await;
    let ack = sub.send_message(&id, "QUEUED_FOLLOWUP", false).unwrap();
    assert!(
        ack.contains("queued") && ack.contains("idle") && ack.contains("starting now"),
        "{ack}"
    );
    let out = wait_output_contains(&sub, &id, "QUEUED_FOLLOWUP").await;
    assert!(!out.contains("主代理在你工作期间发来消息"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_waits_until_current_turn_ends() {
    let gate = Arc::new(Gate {
        started: AtomicBool::new(false),
        release: Notify::new(),
    });
    let h = boot(Arc::new(GatedLastUser {
        gate: gate.clone(),
        first: AtomicBool::new(true),
    }))
    .await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "general-purpose").await;
    wait_flag(&gate.started).await;
    wait_running(&sub, &id).await;
    let ack = sub.send_message(&id, "QUEUED_FOLLOWUP", false).unwrap();
    assert!(
        ack.contains("queued") && ack.contains("running") && ack.contains("after the current turn"),
        "{ack}"
    );
    assert!(
        sub.snapshot(&id).unwrap().running(),
        "queued must not cancel the in-flight turn"
    );
    gate.release.notify_waiters();
    let out = wait_output_contains(&sub, &id, "QUEUED_FOLLOWUP").await;
    assert!(!out.contains("主代理在你工作期间发来消息"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn urgent_while_running_injects_envelope_without_interrupt() {
    let gate = Arc::new(Gate {
        started: AtomicBool::new(false),
        release: Notify::new(),
    });
    let h = boot(Arc::new(GatedLastUser {
        gate: gate.clone(),
        first: AtomicBool::new(true),
    }))
    .await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "general-purpose").await;
    wait_flag(&gate.started).await;
    wait_running(&sub, &id).await;
    let ack = sub.send_message(&id, "STEER_NOW", true).unwrap();
    assert!(
        ack.contains("urgent") && ack.contains("running") && ack.contains("steer"),
        "{ack}"
    );
    gate.release.notify_waiters();
    let out = wait_output_contains(&sub, &id, "STEER_NOW").await;
    assert!(out.contains("主代理在你工作期间发来消息"), "{out}");
    assert!(out.contains("<user_query>"), "{out}");
}

#[tokio::test]
async fn urgent_while_idle_is_plain_next_turn() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "general-purpose").await;
    wait_idle(&sub, &id).await;
    let ack = sub.send_message(&id, "STEER_IDLE", true).unwrap();
    assert!(
        ack.contains("urgent") && ack.contains("idle") && ack.contains("starting now"),
        "{ack}"
    );
    assert!(
        !ack.contains("queued as the next turn"),
        "idle urgent must not reuse the queued ack: {ack}"
    );
    let out = wait_output_contains(&sub, &id, "STEER_IDLE").await;
    assert!(!out.contains("主代理在你工作期间发来消息"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupt_keeps_queued_inbox() {
    let gate = Arc::new(Gate {
        started: AtomicBool::new(false),
        release: Notify::new(),
    });
    let h = boot(Arc::new(GatedLastUser {
        gate: gate.clone(),
        first: AtomicBool::new(true),
    }))
    .await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "general-purpose").await;
    wait_flag(&gate.started).await;
    sub.send_message(&id, "AFTER_INTERRUPT", false).unwrap();
    let msg = sub.interrupt_agent(&id);
    assert!(msg.contains("interrupt requested"), "{msg}");
    gate.release.notify_waiters();
    let out = wait_output_contains(&sub, &id, "AFTER_INTERRUPT").await;
    assert!(!out.contains("主代理在你工作期间发来消息"), "{out}");
}

#[tokio::test]
async fn interrupt_idle_is_noop_not_a_wake() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "general-purpose").await;
    wait_idle(&sub, &id).await;
    let msg = sub.interrupt_agent(&id);
    assert!(msg.contains("already idle"), "{msg}");
    assert!(msg.contains("send_message"), "{msg}");
    assert!(sub.snapshot(&id).unwrap().idle);
}

#[tokio::test]
async fn idle_without_report_forwards_turn_text() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "general-purpose").await;
    wait_idle(&sub, &id).await;
    let notices = sub.drain_parent_notices();
    assert!(
        notices
            .iter()
            .any(|t| t.contains(&id) && t.contains("FIRST_TURN") && t.contains("未调用 report")),
        "{notices:?}"
    );
}

#[tokio::test]
async fn report_appends_parent_system_reminder() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "explore").await;
    wait_idle(&sub, &id).await;
    let def = h
        .root
        .require::<AgentPresets>(AGENT_PRESETS)
        .unwrap()
        .subagent("explore")
        .unwrap();
    let child = h.root.isolate("sessions").isolate("agentPresets");
    child
        .provide(SESSIONS, Sessions::isolated_as(child.clone(), id.clone()))
        .unwrap();
    child
        .provide(
            AGENT_PRESETS,
            AgentPresets::overlay(def.to_preset("explore")),
        )
        .unwrap();
    let result = tools
        .execute_on(
            &child,
            ToolCall {
                id: "rp".into(),
                name: "report".into(),
                arguments: r#"{"output":"仓库只有 README"}"#.into(),
            },
        )
        .await;
    assert!(
        result.content.contains("report accepted"),
        "{}",
        result.content
    );
    let events = h.root.require::<Sessions>(SESSIONS).unwrap().events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, LogEvent::SystemReminder(_))),
        "report must queue until the parent samples: {events:?}"
    );
    let notices = sub.drain_parent_notices();
    assert!(
        notices
            .iter()
            .any(|t| t.contains(&id) && t.contains("仓库只有 README")),
        "{notices:?}"
    );
}

async fn wait_flag(flag: &AtomicBool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("sampler never started");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn subagent_type_enum(tools: &Tools) -> Vec<String> {
    let spec = tools
        .specs_for_model()
        .into_iter()
        .find(|s| s.name == "subagent")
        .expect("subagent spec");
    let v: serde_json::Value = serde_json::from_str(&spec.parameters_json).unwrap();
    v["properties"]["subagent_type"]["enum"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn subagent_reload_roster_picks_up_new_yml() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let before = tools
        .execute(ToolCall {
            id: "r0".into(),
            name: "subagent".into(),
            arguments: r#"{"reload_roster":true}"#.into(),
        })
        .await;
    assert!(
        before.content.contains("explore") && !before.content.contains("- review"),
        "{}",
        before.content
    );
    assert!(
        !subagent_type_enum(&tools).iter().any(|id| id == "review"),
        "{:?}",
        subagent_type_enum(&tools)
    );

    std::fs::create_dir_all(h._home.path().join("code").join("agents")).unwrap();
    std::fs::write(
        h._home
            .path()
            .join("code")
            .join("agents")
            .join("review.yml"),
        "name: 评审\ndescription: 审 diff\ntools:\n  - read_file\n",
    )
    .unwrap();

    let after = tools
        .execute(ToolCall {
            id: "r1".into(),
            name: "subagent".into(),
            arguments: r#"{"reload_roster":true}"#.into(),
        })
        .await;
    assert!(
        after.content.contains("review") && after.content.contains("评审"),
        "{}",
        after.content
    );
    let ids = subagent_type_enum(&tools);
    assert!(ids.iter().any(|id| id == "review"), "{ids:?}");
    assert!(ids.iter().any(|id| id == "explore"), "{ids:?}");
}
