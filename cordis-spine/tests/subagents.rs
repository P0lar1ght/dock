//! Mode-directory roster + one continuable spawn tool: a child runs a turn,
//! parks idle, accepts `send_message`, and pushes a turn-end notice to the
//! parent.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use cordis_spine::{
    agent_loop, blocked_tool_message, install_without_llm, tool_task, turn, AgentPresets,
    BoxFuture, Llm, LlmOutput, LogEvent, PromptRequest, Sampler, Sessions, StreamDelta, Subagents,
    TaskConfig, ToolCall, Tools, AGENT_LOOP, AGENT_PRESETS, LLM, SESSIONS, SUBAGENTS, TOOLS,
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
    boot_with(sampler, TaskConfig::default()).await
}

async fn boot_with(sampler: Arc<dyn Sampler>, cfg: TaskConfig) -> Harness {
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(turn(), ()).unwrap().wait().await.unwrap();
    let home = tempfile::tempdir().unwrap();
    root.provide(AGENT_PRESETS, AgentPresets::load(home.path().to_path_buf()))
        .unwrap();
    root.plugin(tool_task(), cfg).unwrap().wait().await.unwrap();
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
        .find_map(|l| l.trim().strip_prefix("agent_id:"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| panic!("no agent_id in {text}"))
}

async fn spawn_bg(tools: &Tools, prompt: &str, subagent_type: &str) -> String {
    let started = tools
        .execute(ToolCall {
            id: "t0".into(),
            name: "task".into(),
            arguments: serde_json::json!({
                "prompt": prompt,
                "description": "spawn test",
                "subagent_type": subagent_type,
                "run_in_background": true,
            })
            .to_string(),
        })
        .await;
    assert!(
        started.content.contains("Subagent started in background")
            && started.content.contains("send_message")
            && started.content.contains("do not poll")
            && !started.content.contains("job_ids"),
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
async fn task_schema_drops_the_unimplemented_params() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let spec = tools
        .specs_for_model()
        .into_iter()
        .find(|s| s.name == "task")
        .expect("task spec");
    let v: serde_json::Value = serde_json::from_str(&spec.parameters_json).unwrap();
    let props = v["properties"].as_object().unwrap();
    for gone in ["cwd", "isolation", "model"] {
        assert!(!props.contains_key(gone), "{gone} should not be advertised");
    }
    for kept in [
        "prompt",
        "description",
        "subagent_type",
        "run_in_background",
        "resume_from",
        "reload_roster",
    ] {
        assert!(props.contains_key(kept), "{kept} missing from the schema");
    }
    assert_eq!(
        v["required"],
        serde_json::json!(["prompt", "description"]),
        "schema required-set must match what the tool enforces"
    );
    // The roster is closed to this mode's agents/ ids.
    assert!(
        v["properties"]["subagent_type"]["enum"].is_array(),
        "subagent_type must carry the live roster enum"
    );
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
        ack.contains("queued") && ack.contains("running") && ack.contains("next step"),
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

/// The point of the merge: a background child's turn end reaches the parent
/// without any polling.
#[tokio::test]
async fn background_turn_end_pushes_notice_with_output() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "general-purpose").await;
    wait_idle(&sub, &id).await;
    let notices = sub.drain_parent_notices();
    assert!(
        notices.iter().any(|t| t.contains(&id)
            && t.contains("FIRST_TURN")
            && t.contains("finished its turn and is idle")),
        "{notices:?}"
    );
}

/// A foreground spawn hands the result back inline, so the same completion
/// must not also queue a notice.
#[tokio::test]
async fn foreground_spawn_consumes_its_own_notice() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let out = tools
        .execute(ToolCall {
            id: "fg".into(),
            name: "task".into(),
            arguments: serde_json::json!({
                "prompt": "INLINE_RESULT",
                "description": "foreground",
                "subagent_type": "general-purpose",
                "run_in_background": false,
            })
            .to_string(),
        })
        .await;
    assert!(out.content.contains("INLINE_RESULT"), "{}", out.content);
    assert!(out.content.contains("<subagent_meta>"), "{}", out.content);
    let notices = sub.drain_parent_notices();
    assert!(
        !notices
            .iter()
            .any(|t| t.contains("finished its turn and is idle")),
        "the inline result already carried this completion: {notices:?}"
    );
}

/// Stop cancels the session's children; the next prompt re-opens spawns.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_all_stops_children() {
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
    sub.cancel_all();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !sub.snapshot(&id).is_some_and(|s| s.cancelled || s.done) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "cancel_all did not stop {id}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    gate.release.notify_waiters();
}

/// An idle child frees its concurrent slot: with a limit of 1, the second
/// spawn must start once the first parks.
#[tokio::test]
async fn idle_child_does_not_hold_the_concurrency_slot() {
    let h = boot_with(
        Arc::new(LastUser),
        TaskConfig {
            limits: cordis_spine::SubagentLimits {
                max_concurrent: 1,
                ..Default::default()
            },
            ..TaskConfig::default()
        },
    )
    .await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let first = spawn_bg(&tools, "FIRST", "general-purpose").await;
    wait_idle(&sub, &first).await;
    let second = spawn_bg(&tools, "SECOND", "general-purpose").await;
    wait_output_contains(&sub, &second, "SECOND").await;
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
        .find(|s| s.name == "task")
        .expect("task spec");
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
async fn reload_roster_picks_up_new_yml() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let before = tools
        .execute(ToolCall {
            id: "r0".into(),
            name: "task".into(),
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
            name: "task".into(),
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

/// A context that executes tools as child `id` would: its own isolated
/// session (so the caller identity is the child's id) and its role's preset.
fn child_ctx(h: &Harness, id: &str, role: &str) -> Context {
    let def = h
        .root
        .require::<AgentPresets>(AGENT_PRESETS)
        .unwrap()
        .subagent(role)
        .unwrap();
    let child = h.root.isolate("sessions").isolate("agentPresets");
    child
        .provide(
            SESSIONS,
            Sessions::isolated_as(child.clone(), id.to_string()),
        )
        .unwrap();
    child
        .provide(AGENT_PRESETS, AgentPresets::overlay(def.to_preset(role)))
        .unwrap();
    child
}

/// A context that executes tools as a second main page (`main#2`).
fn page_ctx(h: &Harness) -> Context {
    let tab = h.root.isolate("sessions");
    tab.provide(SESSIONS, Sessions::tab(tab.clone(), 2))
        .unwrap();
    tab
}

async fn call_on(tools: &Tools, ctx: &Context, name: &str, args: serde_json::Value) -> String {
    tools
        .execute_on(
            ctx,
            ToolCall {
                id: format!("c-{name}"),
                name: name.into(),
                arguments: args.to_string(),
            },
        )
        .await
        .content
}

/// `report` 并进了 `send_message`：子代理用同一颗工具、填启动它的会话 id 回话，
/// 消息排进父信箱，等父级采样时才进历史，不改父级的现场历史。
#[tokio::test]
async fn child_send_message_reaches_the_parent_mailbox() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    // `explore` 是只读角色：它也得能回话。
    let id = spawn_bg(&tools, "FIRST_TURN", "explore").await;
    wait_idle(&sub, &id).await;
    let child = child_ctx(&h, &id, "explore");
    let ack = call_on(
        &tools,
        &child,
        "send_message",
        serde_json::json!({"agent_id": "main", "message": "仓库只有 README"}),
    )
    .await;
    assert!(ack.contains("delivered to main"), "{ack}");
    let events = h.root.require::<Sessions>(SESSIONS).unwrap().events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, LogEvent::SystemReminder(_))),
        "the message must queue until the parent samples: {events:?}"
    );
    let notices = sub.drain_parent_notices();
    assert!(
        notices
            .iter()
            .any(|t| t.contains(&id) && t.contains("仓库只有 README")),
        "{notices:?}"
    );
    assert!(
        !tools.specs_for_model().iter().any(|s| s.name == "report"),
        "report 已并进 send_message，不该再注册"
    );
}

/// 相邻授权：父只能发给直接子，子只能发给启动它的会话；兄弟、自己、别的分页
/// 一律拒绝。`list_agents` / `interrupt_agent` 同样只看调用方自己的孩子。
#[tokio::test]
async fn send_message_follows_exactly_one_adjacent_edge() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let a = spawn_bg(&tools, "A_TURN", "general-purpose").await;
    let b = spawn_bg(&tools, "B_TURN", "general-purpose").await;
    wait_idle(&sub, &a).await;
    wait_idle(&sub, &b).await;
    sub.drain_parent_notices();

    let child_a = child_ctx(&h, &a, "general-purpose");
    let msg = |to: &str| serde_json::json!({"agent_id": to, "message": "hi"});
    let sibling = call_on(&tools, &child_a, "send_message", msg(&b)).await;
    assert!(sibling.starts_with("Error"), "兄弟之间不能发：{sibling}");
    let own = call_on(&tools, &child_a, "send_message", msg(&a)).await;
    assert!(own.starts_with("Error"), "不能发给自己：{own}");
    let other_page = call_on(&tools, &child_a, "send_message", msg("main#2")).await;
    assert!(
        other_page.starts_with("Error"),
        "只认启动它的会话：{other_page}"
    );
    assert!(
        sub.drain_parent_notices().is_empty(),
        "被拒的消息不该进父信箱"
    );

    let page = page_ctx(&h);
    let foreign = call_on(&tools, &page, "send_message", msg(&a)).await;
    assert!(
        foreign.starts_with("Error"),
        "别的分页不是它的父级：{foreign}"
    );
    let listed = call_on(&tools, &page, "list_agents", serde_json::json!({})).await;
    assert_eq!(listed, "(no subagents)");
    let interrupt = call_on(
        &tools,
        &page,
        "interrupt_agent",
        serde_json::json!({"agent_id": a}),
    )
    .await;
    assert!(interrupt.starts_with("Error"), "{interrupt}");

    let listed = tools
        .execute(ToolCall {
            id: "ls".into(),
            name: "list_agents".into(),
            arguments: "{}".into(),
        })
        .await
        .content;
    assert!(listed.contains(&a) && listed.contains(&b), "{listed}");
    let ok = tools
        .execute(ToolCall {
            id: "ok".into(),
            name: "send_message".into(),
            arguments: msg(&a).to_string(),
        })
        .await
        .content;
    assert!(ok.contains("delivered to idle agent"), "{ok}");
}

/// 回报指令写在子代理的初始任务里，并点名父级 id；人设里不再有上报段。
#[tokio::test]
async fn the_first_task_tells_the_child_whom_to_reply_to() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "explore").await;
    wait_idle(&sub, &id).await;
    let events = sub.events(&id);
    let first_user = events
        .iter()
        .find_map(|e| match e {
            LogEvent::User(t) => Some(t.clone()),
            _ => None,
        })
        .unwrap();
    assert!(first_user.contains("FIRST_TURN"), "{first_user}");
    assert!(
        first_user.contains(r#"agent_id 为 "main""#) && first_user.contains("send_message"),
        "{first_user}"
    );
    let prompts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            LogEvent::Prompt(p) => Some(p.clone()),
            _ => None,
        })
        .collect();
    assert!(
        prompts.iter().all(|p| !p.contains("子代理上报")),
        "上报段不该再进系统提示：{prompts:?}"
    );
}

/// 第一步发一次工具调用（被 gate 挡住），之后把收到的父级消息原样当正文输出。
struct ToolThenEchoMessages {
    gate: Arc<Gate>,
    calls: std::sync::atomic::AtomicUsize,
}

impl Sampler for ToolThenEchoMessages {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let gate = self.gate.clone();
        let nth = self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if nth == 0 {
                let notified = gate.release.notified();
                gate.started.store(true, Ordering::SeqCst);
                notified.await;
                return LlmOutput {
                    tool_calls: vec![ToolCall {
                        id: "step-1".into(),
                        name: "list_agents".into(),
                        arguments: "{}".into(),
                    }],
                    ..LlmOutput::default()
                };
            }
            let text: String = request
                .history
                .iter()
                .filter_map(|e| match e {
                    LogEvent::SystemReminder(t) if t.contains("sent a message") => Some(t.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            on_delta(StreamDelta::Text(text.clone()));
            LlmOutput {
                text,
                ..LlmOutput::default()
            }
        })
    }
}

/// 在跑的子代理在**下一步**读到父级的消息，而不是等整轮结束再开一轮。
#[tokio::test(flavor = "multi_thread")]
async fn a_running_child_reads_a_parent_message_at_its_next_step() {
    let gate = Arc::new(Gate {
        started: AtomicBool::new(false),
        release: Notify::new(),
    });
    let h = boot(Arc::new(ToolThenEchoMessages {
        gate: gate.clone(),
        calls: std::sync::atomic::AtomicUsize::new(0),
    }))
    .await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "general-purpose").await;
    wait_flag(&gate.started).await;
    wait_running(&sub, &id).await;
    let ack = tools
        .execute(ToolCall {
            id: "mid".into(),
            name: "send_message".into(),
            arguments: serde_json::json!({"agent_id": id, "message": "MID_TURN"}).to_string(),
        })
        .await
        .content;
    assert!(
        ack.contains("running") && ack.contains("next step"),
        "{ack}"
    );
    gate.release.notify_waiters();
    let out = wait_output_contains(&sub, &id, "MID_TURN").await;
    assert!(
        out.contains("Agent main sent a message:\nMID_TURN"),
        "{out}"
    );
    let notices = sub.drain_parent_notices();
    assert_eq!(
        notices
            .iter()
            .filter(|t| t.contains("finished its turn"))
            .count(),
        1,
        "消息在同一轮里读到，不该多开一轮：{notices:?}"
    );
}

/// 子代理不是作业：`job` 列不出它，`kill_task` 也处置不了它。
#[tokio::test]
async fn jobs_do_not_see_subagents() {
    let h = boot(Arc::new(LastUser)).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let sub = h.root.require::<Subagents>(SUBAGENTS).unwrap();
    h.root
        .plugin(cordis_spine::jobs(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    h.root
        .plugin(cordis_spine::tool_jobs(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let id = spawn_bg(&tools, "FIRST_TURN", "general-purpose").await;
    wait_idle(&sub, &id).await;
    let run = |name: &str, args: serde_json::Value| {
        let tools = tools.clone();
        let name = name.to_string();
        async move {
            tools
                .execute(ToolCall {
                    id: "j".into(),
                    name,
                    arguments: args.to_string(),
                })
                .await
                .content
        }
    };
    let listed = run("job", serde_json::json!({})).await;
    assert!(
        listed.contains("No background tasks") && !listed.contains(&id),
        "{listed}"
    );
    let polled = run("job", serde_json::json!({"job_ids": [id]})).await;
    assert!(polled.contains("not found"), "{polled}");
    let killed = run("kill_task", serde_json::json!({"job_id": id})).await;
    assert!(killed.contains("not found"), "{killed}");
    assert!(!sub.snapshot(&id).unwrap().done, "kill_task 不该处置子代理");
}
