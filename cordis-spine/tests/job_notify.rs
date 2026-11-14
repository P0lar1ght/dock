//! 后台任务跑完的通知（`job-notify`）：跑完进起它的那一页的信箱，闲着就叫醒那一页；
//! 模型已经拿到结果的不再通知。
//!
//! 以前模型只能拿 `job` 加长 `timeout_ms` 干等：本机 276 个会话里 436 次带等待的
//! `job` 调用，timeout 中位数 240s，124 次等到点还没跑完。

use std::sync::Arc;
use std::time::{Duration, Instant};

use cordis::Context;
use cordis_spine::{
    agent_loop, install_core, job_notify, jobs, tool_jobs, tool_task, turn, workspace_tools,
    AgentPresets, BoxFuture, JobEnd, JobFinished, Jobs, Llm, LlmOutput, LogEvent, LoopHandle,
    PromptRequest, Sampler, Sessions, StreamDelta, Subagents, TaskConfig, ToolCall, Tools,
    AGENT_LOOP, AGENT_PRESETS, JOBS, JOB_FINISHED, LLM, SESSIONS, SUBAGENTS, TOOLS,
};

/// 什么都不调、只回一句话的模型：这里只看信箱有没有进历史。
struct Quiet;

impl Sampler for Quiet {
    fn sample<'a>(
        &'a self,
        _request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        Box::pin(async move {
            on_delta(StreamDelta::Text("ok".into()));
            LlmOutput {
                text: "ok".into(),
                ..LlmOutput::default()
            }
        })
    }
}

struct Harness {
    root: Context,
    tools: Arc<Tools>,
    sub: Arc<Subagents>,
    jobs: Arc<Jobs>,
    _home: tempfile::TempDir,
}

async fn boot() -> Harness {
    let root = Context::new();
    install_core(&root).await.unwrap();
    // 带工作区工具（bash）的那张工具表，同 `install_app`。
    root.plugin(workspace_tools(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(turn(), ()).unwrap().wait().await.unwrap();
    let home = tempfile::tempdir().unwrap();
    root.provide(AGENT_PRESETS, AgentPresets::load(home.path().to_path_buf()))
        .unwrap();
    root.plugin(tool_task(), TaskConfig::default())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.plugin(jobs(), ()).unwrap().wait().await.unwrap();
    root.plugin(tool_jobs(), ()).unwrap().wait().await.unwrap();
    root.plugin(job_notify(), ()).unwrap().wait().await.unwrap();
    root.provide(LLM, Llm::from_sampler(root.clone(), Arc::new(Quiet)))
        .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    Harness {
        tools: root.require::<Tools>(TOOLS).unwrap(),
        sub: root.require::<Subagents>(SUBAGENTS).unwrap(),
        jobs: root.require::<Jobs>(JOBS).unwrap(),
        root,
        _home: home,
    }
}

/// 一页（`main#N` 或子代理），有自己的会话、这一轮和循环。
async fn page(root: &Context, sessions: impl FnOnce(Context) -> Sessions) -> Context {
    let page = root.isolate(SESSIONS).isolate("turn").isolate(AGENT_LOOP);
    page.provide(SESSIONS, sessions(page.clone())).unwrap();
    page.plugin(turn(), ()).unwrap().wait().await.unwrap();
    page.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    page
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

/// 起一条后台 bash，返回 job id。
async fn background(tools: &Tools, ctx: &Context, command: &str) -> String {
    let out = call_on(
        tools,
        ctx,
        "bash",
        serde_json::json!({ "command": command, "is_background": true }),
    )
    .await;
    assert!(out.contains("notified when it exits"), "{out}");
    job_id(&out)
}

fn job_id(out: &str) -> String {
    out.lines()
        .find_map(|l| l.strip_prefix("job_id: "))
        .unwrap_or_else(|| panic!("没有 job_id：{out}"))
        .trim()
        .to_string()
}

async fn until(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !cond() {
        assert!(Instant::now() < deadline, "等不到：{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn finished(jobs: &Jobs, id: &str) {
    until(&format!("{id} 跑完"), Duration::from_secs(10), || {
        jobs.snapshot(id).is_some_and(|s| s.done)
    })
    .await;
}

/// 等过一个攒批窗口（1s）再多给一点：该送的都送进信箱了。
async fn past_the_batch_window() {
    tokio::time::sleep(Duration::from_millis(1600)).await;
}

/// 后台任务跑完：通知进起它的那一页的信箱，并且叫醒那一页（闲着就开一轮）；正文带
/// 退出码和输出结尾，不用模型再去 `job` 等。
#[tokio::test]
async fn a_finished_background_job_wakes_the_page_that_started_it() {
    let h = boot().await;
    let id = background(&h.tools, &h.root, "echo first-line; echo last-line; exit 3").await;
    until("通知叫醒第 1 页", Duration::from_secs(10), || {
        h.sub.has_parent_notices("main")
    })
    .await;
    let notices = h.sub.drain_parent_notices("main");
    assert_eq!(notices.len(), 1, "{notices:?}");
    let text = &notices[0];
    assert!(
        text.contains(&format!("后台任务 {id} 结束了：exit 3（失败）")),
        "{text}"
    );
    assert!(text.contains("last-line"), "要带输出结尾：{text}");
    assert!(text.contains("不是用户发的消息"), "{text}");
    assert!(text.starts_with("<system-reminder>"), "{text}");
}

/// 模型已经用 `job` 等到了结果：不再为同一个结果叫醒这一页。以前没有通知，现在要防的
/// 是「等到了又被叫醒一次」。
#[tokio::test]
async fn a_result_already_read_with_job_is_not_announced_again() {
    let h = boot().await;
    let id = background(&h.tools, &h.root, "sleep 0.3; echo waited-for").await;
    let out = call_on(
        &h.tools,
        &h.root,
        "job",
        serde_json::json!({ "job_ids": [id], "timeout_ms": 10_000 }),
    )
    .await;
    assert!(out.contains("waited-for"), "{out}");
    past_the_batch_window().await;
    assert!(!h.sub.has_parent_notices("main"), "读到了还叫醒");
    assert!(h.sub.drain_parent_notices("main").is_empty());
}

/// 通知已经进了信箱、还没送到，模型先用 `job` 读了：撤掉那条。
#[tokio::test]
async fn reading_the_result_retracts_a_notice_not_yet_delivered() {
    let h = boot().await;
    let id = background(&h.tools, &h.root, "echo early").await;
    until("通知进信箱", Duration::from_secs(10), || {
        h.sub.has_parent_notices("main")
    })
    .await;
    let out = call_on(
        &h.tools,
        &h.root,
        "job",
        serde_json::json!({ "job_ids": [id] }),
    )
    .await;
    assert!(out.contains("early"), "{out}");
    assert!(
        !h.sub.has_parent_notices("main"),
        "读到了还留着要叫醒的通知"
    );
    assert!(h.sub.drain_parent_notices("main").is_empty());
}

/// 只看了一眼还在跑的：不算拿到结果，跑完照常通知。
#[tokio::test]
async fn peeking_at_a_running_job_still_gets_the_notice() {
    let h = boot().await;
    let id = background(&h.tools, &h.root, "sleep 0.5; echo later").await;
    let out = call_on(
        &h.tools,
        &h.root,
        "job",
        serde_json::json!({ "job_ids": [id] }),
    )
    .await;
    assert!(out.contains("[running"), "{out}");
    until("跑完的通知", Duration::from_secs(10), || {
        h.sub.has_parent_notices("main")
    })
    .await;
}

/// 模型自己 `kill_task` 停的：它知道，不发通知。
#[tokio::test]
async fn a_job_the_model_killed_is_not_announced() {
    let h = boot().await;
    let id = background(&h.tools, &h.root, "sleep 30").await;
    let out = call_on(
        &h.tools,
        &h.root,
        "kill_task",
        serde_json::json!({ "job_id": id }),
    )
    .await;
    assert!(out.contains("killed"), "{out}");
    finished(&h.jobs, &id).await;
    past_the_batch_window().await;
    assert!(h.sub.drain_parent_notices("main").is_empty());
}

/// 别处停掉的（任务面板）：模型不知道，送进信箱，但不为它叫醒这一页。
#[tokio::test]
async fn a_job_stopped_elsewhere_is_delivered_without_waking() {
    let h = boot().await;
    let id = background(&h.tools, &h.root, "sleep 30").await;
    assert!(h.jobs.kill(&id).await.contains("killed"));
    finished(&h.jobs, &id).await;
    past_the_batch_window().await;
    assert!(!h.sub.has_parent_notices("main"), "被停掉的不该叫醒");
    let notices = h.sub.drain_parent_notices("main");
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("被停掉了"), "{}", notices[0]);
}

/// 前台等到点转入后台的命令：模型拿到的只是半截输出，跑完要通知它剩下的。
#[tokio::test]
async fn a_command_moved_to_the_background_announces_its_end() {
    let h = boot().await;
    let out = call_on(
        &h.tools,
        &h.root,
        "bash",
        serde_json::json!({ "command": "echo before; sleep 1; echo after-detach", "timeout_ms": 300 }),
    )
    .await;
    assert!(out.contains("已转入后台"), "{out}");
    assert!(out.contains("跑完会通知你"), "{out}");
    let id = job_id(&out);
    until("跑完的通知", Duration::from_secs(10), || {
        h.sub.has_parent_notices("main")
    })
    .await;
    let notices = h.sub.drain_parent_notices("main");
    assert!(notices[0].contains(&id), "{notices:?}");
    assert!(notices[0].contains("after-detach"), "{notices:?}");
}

/// 前台命令照常跑完：结果已经随调用交回，不再通知。
#[tokio::test]
async fn a_foreground_command_is_not_announced() {
    let h = boot().await;
    let out = call_on(
        &h.tools,
        &h.root,
        "bash",
        serde_json::json!({ "command": "echo fg" }),
    )
    .await;
    assert!(out.contains("fg"), "{out}");
    past_the_batch_window().await;
    assert!(h.sub.drain_parent_notices("main").is_empty());
}

/// 前后脚跑完的几条攒成一次：叫醒时信箱里两条都在，那一页只开一轮。没有攒批时
/// 第一条一到就叫醒，第二条要么夹在那一轮中间、要么再叫醒一轮。
#[tokio::test]
async fn jobs_finishing_close_together_arrive_together() {
    let h = boot().await;
    let a = background(&h.tools, &h.root, "sleep 0.1; echo a").await;
    let b = background(&h.tools, &h.root, "sleep 0.5; echo b").await;
    until("叫醒", Duration::from_secs(10), || {
        h.sub.has_parent_notices("main")
    })
    .await;
    let notices = h.sub.drain_parent_notices("main");
    assert_eq!(notices.len(), 2, "两条要一起到：{notices:?}");
    assert!(notices.iter().any(|n| n.contains(&a)), "{notices:?}");
    assert!(notices.iter().any(|n| n.contains(&b)), "{notices:?}");
}

/// 分页（`main#2`）起的：通知进那一页，第 1 页看不到；那一页的信箱轮把它写进历史。
#[tokio::test]
async fn a_tabs_job_reports_to_that_tab() {
    let h = boot().await;
    let tab = page(&h.root, |ctx| Sessions::tab(ctx, 2)).await;
    let id = background(&h.tools, &tab, "echo from-tab").await;
    until("通知进第 2 页", Duration::from_secs(10), || {
        h.sub.has_parent_notices("main#2")
    })
    .await;
    assert!(!h.sub.has_parent_notices("main"), "不该落到第 1 页");

    let handle = tab.require::<LoopHandle>(AGENT_LOOP).unwrap();
    handle.continue_mailbox().await.unwrap();
    let reminded = |ctx: &Context| {
        ctx.require::<Sessions>(SESSIONS)
            .unwrap()
            .events()
            .iter()
            .any(|e| matches!(e, LogEvent::SystemReminder(t) if t.contains(&id)))
    };
    assert!(reminded(&tab), "要进第 2 页的历史");
    assert!(!reminded(&h.root), "第 1 页的历史里不该有");
}

/// 子代理起的：通知按它的身份进信箱、不叫醒（子代理只由父级叫醒），它下一次采样带上。
#[tokio::test]
async fn a_subagents_job_waits_for_its_next_step() {
    let h = boot().await;
    let child = page(&h.root, |ctx| Sessions::isolated_as(ctx, "child-jobtest")).await;
    let id = background(&h.tools, &child, "echo from-child").await;
    finished(&h.jobs, &id).await;
    past_the_batch_window().await;
    assert!(
        !h.sub.has_parent_notices("child-jobtest"),
        "子代理不该被叫醒"
    );
    assert!(!h.sub.has_parent_notices("main"), "也不该落到第 1 页");

    let handle = child.require::<LoopHandle>(AGENT_LOOP).unwrap();
    handle.run("go on").await.unwrap();
    let got = child
        .require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .any(|e| matches!(e, LogEvent::SystemReminder(t) if t.contains(&id)));
    assert!(got, "子代理下一次采样要带上它起的任务的通知");
}

/// 不是在哪一页的一轮里起的任务（没有归属）：没有哪个模型在等它，不通知。
#[tokio::test]
async fn a_job_without_a_page_is_not_announced() {
    let h = boot().await;
    let id = h.jobs.start("echo orphan");
    finished(&h.jobs, &id).await;
    past_the_batch_window().await;
    assert!(h.sub.drain_parent_notices("main").is_empty());
}

/// 跑完事件本身（别的插件也能听）：带归属和怎么结束的。
#[tokio::test]
async fn the_finished_event_carries_the_page_and_the_end() {
    let h = boot().await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _listen = h
        .root
        .on(JOB_FINISHED, move |done: &JobFinished| {
            let _ = tx.send(done.clone());
        })
        .unwrap();
    let id = background(&h.tools, &h.root, "exit 0").await;
    let done = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.id, id);
    assert_eq!(done.owner.as_deref(), Some("main"));
    assert_eq!(done.end, JobEnd::Exited(0));
}
