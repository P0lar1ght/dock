//! workflow host：预算闸门与每 run 并发池。
//!
//! 引擎自己不记账（`vendor/xai/workflow` 的 `reserve_agent_calls` 只看 host 回
//! 执），所以这两条都只能从 host 侧证明。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use cordis_spine::{
    agent_loop, install_without_llm, tool_task, tool_workflow, turn, AgentPresets, BoxFuture, Llm,
    LlmOutput, LogEvent, PromptRequest, Sampler, Sessions, StreamDelta, ToolCall, Tools,
    WorkflowRunSnap, Workflows, AGENT_LOOP, AGENT_PRESETS, LLM, SESSIONS, TOOLS, WORKFLOWS,
};
use cordis_spine::{TaskConfig, WORKFLOW_TOOL_NAME};
use tokio::sync::Notify;

struct Harness {
    root: Context,
    _home: tempfile::TempDir,
    _env: cordis_base::test_env::EnvScope,
}

/// 回显最后一条 user 消息，并记录自己被调用过几次 / 同时几个在跑。
struct Counting {
    live: Arc<AtomicU32>,
    peak: Arc<AtomicU32>,
    /// 每个子代理都等这个门放行，好把并发钉在一个可观测的状态上。
    gate: Arc<Notify>,
    hold: bool,
}

impl Sampler for Counting {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let live = self.live.clone();
        let peak = self.peak.clone();
        let gate = self.gate.clone();
        let hold = self.hold;
        Box::pin(async move {
            let now = live.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            if hold {
                gate.notified().await;
            }
            live.fetch_sub(1, Ordering::SeqCst);
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

async fn boot(sampler: Arc<dyn Sampler>, cfg: TaskConfig) -> Harness {
    let home = tempfile::tempdir().unwrap();
    // scratch 落在 `DOCK_HOME` 下；工作流目录扫描也读它。
    let env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path().join("dock-home"));
    std::fs::create_dir_all(home.path().join("dock-home")).unwrap();

    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(turn(), ()).unwrap().wait().await.unwrap();
    root.provide(AGENT_PRESETS, AgentPresets::load(home.path().to_path_buf()))
        .unwrap();
    root.plugin(tool_task(), cfg).unwrap().wait().await.unwrap();
    root.provide(LLM, Llm::from_sampler(root.clone(), sampler))
        .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    let _ = root
        .require::<cordis_spine::LoopHandle>(AGENT_LOOP)
        .unwrap();
    root.plugin(tool_workflow(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    Harness {
        root,
        _home: home,
        _env: env,
    }
}

/// 起一条内联脚本，返回 run id。
async fn launch(h: &Harness, script: &str, agent_budget: u64) -> String {
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let out = tools
        .execute(ToolCall {
            id: "wf".into(),
            name: WORKFLOW_TOOL_NAME.into(),
            arguments: serde_json::json!({
                "source": { "type": "script", "script": script },
                "agent_budget": agent_budget,
            })
            .to_string(),
        })
        .await;
    let value: serde_json::Value = serde_json::from_str(&out.content)
        .unwrap_or_else(|_| panic!("launch was rejected: {}", out.content));
    value["run_id"].as_str().expect("run_id").to_string()
}

fn wf_run(h: &Harness, run_id: &str) -> Option<WorkflowRunSnap> {
    h.root
        .require::<Workflows>(WORKFLOWS)
        .unwrap()
        .list()
        .into_iter()
        .find(|r| r.run_id == run_id)
}

async fn wait_terminal(h: &Harness, run_id: &str) -> WorkflowRunSnap {
    let wf = h.root.require::<Workflows>(WORKFLOWS).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(run) = wf.list().into_iter().find(|r| r.run_id == run_id) {
            if run.status != "active" {
                return run;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let got = wf.list().into_iter().find(|r| r.run_id == run_id);
            panic!("workflow {run_id} never finished: {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn counting(hold: bool) -> (Arc<dyn Sampler>, Arc<AtomicU32>, Arc<Notify>) {
    let peak = Arc::new(AtomicU32::new(0));
    let gate = Arc::new(Notify::new());
    let sampler: Arc<dyn Sampler> = Arc::new(Counting {
        live: Arc::new(AtomicU32::new(0)),
        peak: peak.clone(),
        gate: gate.clone(),
        hold,
    });
    (sampler, peak, gate)
}

/// 记录子代理那一轮实际拿到的工具表。
struct Recording {
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Sampler for Recording {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        *self.seen.lock().unwrap() = request.tools.iter().map(|t| t.name.clone()).collect();
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

fn recording() -> (Arc<dyn Sampler>, Arc<std::sync::Mutex<Vec<String>>>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sampler: Arc<dyn Sampler> = Arc::new(Recording { seen: seen.clone() });
    (sampler, seen)
}

const TWO_AGENTS: &str = r#"
let meta = #{ name: "budget-probe", description: "two sequential agents" };
let a = agent("first");
let b = agent("second");
complete("done");
"#;

/// `agent_budget` 必须真正拦住第二次 `agent()`。改动前它是死参数，这条会以
/// `complete` 收尾。
#[tokio::test]
async fn agent_budget_blocks_the_second_agent() {
    let (sampler, _peak, _gate) = counting(false);
    let h = boot(sampler, TaskConfig::default()).await;
    let run_id = launch(&h, TWO_AGENTS, 1).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "failed", "预算耗尽的 run 不该跑完：{run:?}");
    let message = run.pause_message.unwrap_or_default();
    assert!(
        message.contains("budget"),
        "失败原因要说清是预算：{message}"
    );
    assert_eq!(run.agent_budget, 1);
    assert_eq!(run.agents_used, 1, "被拒的那次不记账");
}

/// 预算够用时两个 agent 都该跑完，记账也要对得上。
#[tokio::test]
async fn a_sufficient_budget_runs_every_agent() {
    let (sampler, _peak, _gate) = counting(false);
    let h = boot(sampler, TaskConfig::default()).await;
    let run_id = launch(&h, TWO_AGENTS, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "complete", "{run:?}");
    assert_eq!(run.agents_used, 2);
}

const PARALLEL_FOUR: &str = r#"
let meta = #{ name: "fanout-probe", description: "four parallel agents" };
let jobs = [
    #{ prompt: "one" },
    #{ prompt: "two" },
    #{ prompt: "three" },
    #{ prompt: "four" },
];
let results = parallel(jobs);
complete("done");
"#;

/// 跑一次 `parallel(4)`，返回（终态快照，观察到的最大同时在跑数）。
async fn fanout_peak(max_concurrent: usize) -> (WorkflowRunSnap, u32) {
    let (sampler, peak, gate) = counting(true);
    let cfg = TaskConfig {
        workflow_max_concurrent_agents: max_concurrent,
        ..TaskConfig::default()
    };
    let h = boot(sampler, cfg).await;
    let run_id = launch(&h, PARALLEL_FOUR, 8).await;

    // 稳定放行：每次只让一个子代理往下走，好让 peak 反映真实的同时在跑数。
    let pump = tokio::spawn(async move {
        loop {
            gate.notify_one();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let run = wait_terminal(&h, &run_id).await;
    pump.abort();
    (run, peak.load(Ordering::SeqCst))
}

/// 一次 run 同时在跑的子代理不得超过池子大小。改动前 `parallel()` 有几个 job
/// 就同时起几个（`admission.rs` 对 workflow owner 直接放行，而 dock 没有池）。
#[tokio::test]
async fn a_run_keeps_at_most_max_concurrent_agents_live() {
    let (run, peak) = fanout_peak(1).await;
    assert_eq!(run.status, "complete", "{run:?}");
    assert_eq!(peak, 1, "并发上限 1 时不能有两个子代理同时在跑");
    assert_eq!(run.agents_used, 4);
}

/// 反证：池子放开后同一个脚本确实会并发起来 —— 上一条的 `peak == 1` 是池子
/// 拦下来的，不是脚本本来就串行。
#[tokio::test]
async fn a_wide_pool_lets_the_same_fanout_run_concurrently() {
    let (run, peak) = fanout_peak(4).await;
    assert_eq!(run.status, "complete", "{run:?}");
    assert!(peak > 1, "池子放开后应当观察到并发，实际 peak={peak}");
}

/// 停掉一次在跑的 run：run 以 `cancelled` 收尾，它的子代理不许活下来。
///
/// 改动前 `RunSlot.cancel` 全仓没人调用，而且子代理 owner 的 run id 被写死成
/// `"host"`，`cancel_workflow_children` 按 run 一个也匹配不到。
#[tokio::test]
async fn stopping_a_run_cancels_it_and_its_children() {
    let (sampler, _peak, gate) = counting(true);
    let h = boot(sampler, TaskConfig::default()).await;
    let run_id = launch(&h, PARALLEL_FOUR, 8).await;
    let wf = h.root.require::<Workflows>(WORKFLOWS).unwrap();
    let sub = h
        .root
        .require::<cordis_spine::Subagents>(cordis_spine::SUBAGENTS)
        .unwrap();

    // 等孩子真的起来，再停 —— 否则测的是"还没开始就停了"。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while sub.list().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "子代理没起来");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        wf.stop(&run_id).as_deref(),
        Some(run_id.as_str()),
        "停一次在跑的 run 应当命中"
    );
    // 放行卡住的子代理，让取消路径能走完。
    let pump = tokio::spawn(async move {
        loop {
            gate.notify_one();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let run = wait_terminal(&h, &run_id).await;
    pump.abort();

    assert_eq!(run.status, "cancelled", "{run:?}");
    assert_eq!(run.agents_running, 0, "取消后不该还有孩子挂在账上");
    let alive: Vec<_> = sub.list().into_iter().filter(|s| !s.done).collect();
    assert!(alive.is_empty(), "取消后还有子代理在跑：{alive:?}");
}

/// 停一个不存在的 run 要如实说没命中，而不是假装停了。
#[tokio::test]
async fn stopping_an_unknown_run_reports_a_miss() {
    let (sampler, _peak, _gate) = counting(false);
    let h = boot(sampler, TaskConfig::default()).await;
    let wf = h.root.require::<Workflows>(WORKFLOWS).unwrap();
    assert_eq!(wf.stop("wf_nope"), None);
    assert_eq!(wf.stop(""), None);
}

/// run 也能按显示名点停，不必知道内部 run id。
#[tokio::test]
async fn a_run_can_be_stopped_by_display_name() {
    let (sampler, _peak, gate) = counting(true);
    let h = boot(sampler, TaskConfig::default()).await;
    let run_id = launch(&h, PARALLEL_FOUR, 8).await;
    let wf = h.root.require::<Workflows>(WORKFLOWS).unwrap();

    assert_eq!(wf.stop("fanout-probe").as_deref(), Some(run_id.as_str()));
    let pump = tokio::spawn(async move {
        loop {
            gate.notify_one();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let run = wait_terminal(&h, &run_id).await;
    pump.abort();
    assert_eq!(run.status, "cancelled", "{run:?}");
}

/// 脚本的 `log()` 要能在快照里看到。改动前这条请求被直接丢弃，而
/// `deep_research.rhai` 的 6 处 `log()` 是那条长跑工作流唯一的进度输出。
#[tokio::test]
async fn script_logs_reach_the_snapshot() {
    let (sampler, _peak, _gate) = counting(false);
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{ name: "log-probe", description: "logs only" };
log("第一步");
log("第二步");
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "complete", "{run:?}");
    assert_eq!(run.logs.as_slice(), ["第一步", "第二步"]);
    assert_eq!(run.latest_log(), Some("第二步"));
}

/// 脚本不能把任意长的字符串顶进 TUI。
#[tokio::test]
async fn oversized_phase_and_log_are_truncated() {
    let (sampler, _peak, _gate) = counting(false);
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{ name: "flood-probe", description: "oversized strings" };
let huge = "x";
for i in 0..14 { huge += huge; }
phase(huge);
log(huge);
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "complete", "{run:?}");
    let phase = run.current_phase.clone().expect("phase");
    assert!(
        phase.len() <= 256 + 4,
        "阶段标题没截断：{} 字节",
        phase.len()
    );
    let log = run.latest_log().expect("log");
    assert!(log.len() <= 4096 + 4, "日志没截断：{} 字节", log.len());
}

/// 环形缓冲要保住上限，留最近的。
#[tokio::test]
async fn the_log_ring_keeps_only_the_latest_lines() {
    let (sampler, _peak, _gate) = counting(false);
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{ name: "ring-probe", description: "many logs" };
for i in 0..200 { log("line-" + i.to_string()); }
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "complete", "{run:?}");
    assert_eq!(run.logs.len(), 50);
    assert_eq!(run.latest_log(), Some("line-199"));
    assert_eq!(run.logs.first().map(String::as_str), Some("line-150"));
}

/// 前 `bad_turns` 轮回散文，之后回一段合规 JSON。
struct Flaky {
    remaining_bad: AtomicU32,
    turns: Arc<AtomicU32>,
}

impl Sampler for Flaky {
    fn sample<'a>(
        &'a self,
        _request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        self.turns.fetch_add(1, Ordering::SeqCst);
        let bad = self
            .remaining_bad
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                n.checked_sub(1).or(Some(0))
            })
            .is_ok_and(|n| n > 0);
        let text = if bad {
            "我查完了，结论是可以。".to_string()
        } else {
            "```json\n{\"ok\": true}\n```".to_string()
        };
        Box::pin(async move {
            on_delta(StreamDelta::Text(text.clone()));
            LlmOutput {
                text,
                ..LlmOutput::default()
            }
        })
    }
}

fn flaky(bad_turns: u32) -> (Arc<dyn Sampler>, Arc<AtomicU32>) {
    let turns = Arc::new(AtomicU32::new(0));
    let sampler: Arc<dyn Sampler> = Arc::new(Flaky {
        remaining_bad: AtomicU32::new(bad_turns),
        turns: turns.clone(),
    });
    (sampler, turns)
}

const SCHEMA_AGENT: &str = r#"
let meta = #{ name: "contract-probe", description: "one agent under contract" };
let r = agent("请给结论", #{
    output_schema: #{
        "type": "object",
        "required": ["ok"],
        "properties": #{ "ok": #{ "type": "boolean" } },
    },
});
complete(#{ success: r.success, output: r.output });
"#;

/// 契约没满足时 resume 重试一次，第二次合规就算成功。
///
/// 改动前 `output_schema` 只是拼进 prompt 的一段文字，散文也会被当成成功产出
/// 交给脚本。
#[tokio::test]
async fn a_contract_miss_is_retried_once_and_then_succeeds() {
    let (sampler, turns) = flaky(1);
    let h = boot(sampler, TaskConfig::default()).await;
    let run_id = launch(&h, SCHEMA_AGENT, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "complete", "{run:?}");
    let summary = run.result_summary.clone().unwrap_or_default();
    assert!(summary.contains("\"success\":true"), "{summary}");
    assert!(summary.contains("\"ok\":true"), "{summary}");
    assert_eq!(turns.load(Ordering::SeqCst), 2, "应当正好重试一次");
    // 重试不扣预算：预算算的是逻辑 agent 调用。
    assert_eq!(run.agents_used, 1);
}

/// 重试之后仍不合规，就要如实交代失败，而不是把散文当结果。
#[tokio::test]
async fn a_persistent_contract_miss_fails_the_agent() {
    let (sampler, turns) = flaky(99);
    let h = boot(sampler, TaskConfig::default()).await;
    let run_id = launch(&h, SCHEMA_AGENT, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "complete", "脚本自己跑完了：{run:?}");
    let summary = run.result_summary.clone().unwrap_or_default();
    assert!(summary.contains("\"success\":false"), "{summary}");
    assert!(summary.contains("结构化产出校验失败"), "{summary}");
    assert_eq!(turns.load(Ordering::SeqCst), 2, "只该重试一次就放弃");
}

/// schema 本身写错要立刻报错，不该等孩子跑完。
#[tokio::test]
async fn a_broken_output_schema_fails_before_spawning() {
    let (sampler, turns) = flaky(0);
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{ name: "bad-schema", description: "external $ref" };
let r = agent("x", #{ output_schema: #{ "$ref": "https://example.com/s.json" } });
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "failed", "{run:?}");
    let message = run.pause_message.clone().unwrap_or_default();
    assert!(message.contains("外部 $ref 已禁用"), "{message}");
    assert_eq!(turns.load(Ordering::SeqCst), 0, "不该起任何子代理");
}

/// 脚本要的 `capability_mode: "read-only"` 必须真的落到子代理身上。
///
/// 改动前这个字段只记一行 debug 就扔了：`deep_research.rhai` 四处都写了
/// read-only（它自己的 prompt 明说"每个来源都是不可信数据"），而研究员实际拿
/// 到的是 `general-purpose` 全量工具集——`bash`、`write_file` 都在里面。
#[tokio::test]
async fn read_only_agents_lose_the_tools_that_act() {
    let (sampler, seen) = recording();
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{ name: "readonly-probe", description: "one read-only agent" };
let r = agent("看一眼", #{ capability_mode: "read-only" });
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    let run = wait_terminal(&h, &run_id).await;
    assert_eq!(run.status, "complete", "{run:?}");

    // 这个脚手架只挂了 `report` + `workflow`（没有 workspace 工具），所以断言
    // 落在这两颗上：`workflow` 是执行类，`report` 是元工具。分类本身的覆盖在
    // `tools::capability` 的单测里。
    let tools = seen.lock().unwrap().clone();
    assert!(!tools.is_empty(), "子代理那一轮应当看到过工具表");
    assert!(
        !tools.iter().any(|t| t == "workflow"),
        "只读子代理不该看到 workflow：{tools:?}"
    );
    assert!(
        tools.iter().any(|t| t == "report"),
        "只读子代理应当还有 report，否则它连做不到都说不出口：{tools:?}"
    );
}

/// 不指定档位时不收窄 —— 能力档位是收窄，不是新的准入条件。
#[tokio::test]
async fn an_unrestricted_agent_keeps_its_full_toolset() {
    let (sampler, seen) = recording();
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{ name: "unrestricted-probe", description: "no capability mode" };
let r = agent("看一眼");
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    wait_terminal(&h, &run_id).await;

    let tools = seen.lock().unwrap().clone();
    assert!(
        tools.iter().any(|t| t == "workflow"),
        "没指定档位就不该被收窄：{tools:?}"
    );
}

/// 档位名写错要当场报错，不能悄悄按不设限跑。
#[tokio::test]
async fn an_unknown_capability_mode_fails_the_agent() {
    let (sampler, _seen) = recording();
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{ name: "bad-capability", description: "typo" };
let r = agent("x", #{ capability_mode: "readonlyish" });
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "failed", "{run:?}");
    let message = run.pause_message.clone().unwrap_or_default();
    assert!(message.contains("capability_mode 只能是"), "{message}");
}

/// 先 `report` 一次再收尾的子代理——`general-purpose` 的人设就是这么要求的
/// （「结束前用 report 向启动你的主代理上报自足结论」）。
struct Reporting {
    /// 还没报过的那一步发工具调用，之后正常收尾；否则会卡在工具循环里。
    pending: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl Sampler for Reporting {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let text = last_user(&request.history);
        let first = self.pending.lock().unwrap().insert(text.clone());
        Box::pin(async move {
            on_delta(StreamDelta::Text(text.clone()));
            let tool_calls = if first {
                vec![ToolCall {
                    id: "r1".into(),
                    name: "report".into(),
                    arguments: serde_json::json!({ "output": "阶段性结论" }).to_string(),
                }]
            } else {
                Vec::new()
            };
            LlmOutput {
                text,
                tool_calls,
                ..LlmOutput::default()
            }
        })
    }
}

fn reporting() -> Arc<dyn Sampler> {
    Arc::new(Reporting {
        pending: std::sync::Mutex::new(std::collections::HashSet::new()),
    })
}

/// run 没跑完之前，主线程一次也不该被叫醒。
///
/// 改动前有**两条**通道漏过去：子代理的回合结束通知（`surface_completion: false`
/// 声明了不该发，但那个字段全仓没人读），以及 `report`（人设明文要求每个子代理
/// 都调）。一条 deep-research 能把主线程叫醒十几次，每次都在半份结果上开一轮。
#[tokio::test]
async fn a_running_workflow_never_wakes_the_main_thread() {
    let h = boot(reporting(), TaskConfig::default()).await;
    let sub = h
        .root
        .require::<cordis_spine::Subagents>(cordis_spine::SUBAGENTS)
        .unwrap();

    let run_id = launch(&h, TWO_AGENTS, 4).await;
    // 轮询到 run 收尾为止，全程盯着父信箱。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let run = wf_run(&h, &run_id);
        let done = run.as_ref().is_some_and(|r| r.status != "active");
        if !done {
            assert!(
                !sub.has_parent_notices(),
                "run 还在跑（子代理 {} 个）就叫醒了主线程",
                run.map(|r| r.agents.len()).unwrap_or(0)
            );
        } else {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "run 没有收尾");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 收尾之后正好一条：结果 + 过程。
    assert!(sub.has_parent_notices(), "收尾了却没通知主线程");
    let notices = sub.drain_parent_notices();
    assert_eq!(notices.len(), 1, "整条 run 只该有一条通知：{notices:?}");
    let text = &notices[0];
    assert!(text.contains("budget-probe"), "{text}");
    assert!(text.contains("已完成"), "{text}");
    assert!(
        text.contains("阶段性结论"),
        "过程上报要随结果一起交付：{text}"
    );
}

/// 上报要立刻出现在滚动区，但不能进模型历史。
///
/// 用户看不见中间过程就会把收尾唤醒误判成「planner 上报叫醒了主线程」。
/// `LogEvent::Notice` 是给用户看的卡；`model_history` 滤掉它，才不会退回中途唤醒。
#[tokio::test]
async fn agent_reports_become_pager_cards_not_model_turns() {
    let h = boot(reporting(), TaskConfig::default()).await;
    let run_id = launch(&h, TWO_AGENTS, 4).await;
    let run = wait_terminal(&h, &run_id).await;
    assert_eq!(run.status, "complete", "{run:?}");

    let sessions = h.root.require::<Sessions>(SESSIONS).unwrap();
    let pager = sessions.events();
    let reports: Vec<_> = pager
        .iter()
        .filter_map(|e| match e {
            LogEvent::Notice { title, body, .. } if title.contains("上报") => {
                Some((title.as_str(), body.as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(reports.len(), 2, "两次 agent() 各一张上报卡：{pager:?}");
    assert!(
        reports.iter().all(|(_, body)| *body == "阶段性结论"),
        "{reports:?}"
    );
    assert!(
        pager.iter().any(|e| matches!(
            e,
            LogEvent::Notice { title, .. } if title.contains("完成")
        )),
        "收尾也该有一张卡：{pager:?}"
    );
    assert!(
        !sessions
            .model_history()
            .iter()
            .any(|e| matches!(e, LogEvent::Notice { .. })),
        "Notice 进模型历史就会把主线程叫醒"
    );
}

/// 上报同时进 run 的进度流，overlay 才有东西显示。
#[tokio::test]
async fn agent_reports_land_on_the_run_progress() {
    let h = boot(reporting(), TaskConfig::default()).await;
    let run_id = launch(&h, TWO_AGENTS, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "complete", "{run:?}");
    assert_eq!(run.agents.len(), 2, "两次 agent() 就该有两行");
    for row in run.agents.iter() {
        assert_eq!(row.state, "done", "{row:?}");
        assert_eq!(row.latest_report.as_deref(), Some("阶段性结论"), "{row:?}");
    }
    assert!(
        run.logs.iter().any(|l| l.contains("阶段性结论")),
        "{:?}",
        run.logs
    );
}

/// 脚本声明的阶段要进快照，详情页左栏按它渲染。
#[tokio::test]
async fn declared_phases_reach_the_snapshot() {
    let (sampler, _peak, _gate) = counting(false);
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{
    name: "phase-probe",
    description: "declares phases",
    phases: [
        #{ title: "Plan", detail: "想清楚" },
        #{ title: "Do" },
    ],
};
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "complete", "{run:?}");
    assert_eq!(
        run.phases.as_slice(),
        [
            ("Plan".to_string(), "想清楚".to_string()),
            ("Do".to_string(), String::new()),
        ]
    );
}

/// `agent()` 没写 `phase:` 时回退到脚本当前 `phase()` 的那一段。
///
/// `deep_research.rhai` 的 planner 就是这个形状：调了 `phase("Plan")`，但那次
/// `agent()` 只给了 label / capability_mode / output_schema。改动前它会掉进
/// 「其它」，详情页的 Plan 一栏显示「这一阶段还没有子代理」。
#[tokio::test]
async fn an_agent_without_a_phase_inherits_the_current_one() {
    let (sampler, _peak, _gate) = counting(false);
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{
    name: "phase-fallback",
    description: "planner without an explicit phase",
    phases: [#{ title: "Plan" }, #{ title: "Research" }],
};
phase("Plan");
let planner = agent("规划", #{ label: "research-planner" });
phase("Research");
let worker = agent("调研", #{ label: "researcher-0", phase: "Research" });
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "complete", "{run:?}");
    let by_label = |label: &str| {
        run.agents
            .iter()
            .find(|a| a.label == label)
            .unwrap_or_else(|| panic!("没有 {label}：{:?}", run.agents))
            .phase
            .clone()
    };
    assert_eq!(
        by_label("research-planner").as_deref(),
        Some("Plan"),
        "没写 phase 的要跟当时的 phase() 走"
    );
    assert_eq!(
        by_label("researcher-0").as_deref(),
        Some("Research"),
        "写了的以写的为准"
    );
}

/// 还在跑时就要把阶段钉在行上。详情页按行上的 `phase` 分栏，等收尾再写
/// 用户早就看见「Plan 这一阶段还没有子代理」。
#[tokio::test]
async fn a_running_planner_is_stamped_with_the_current_phase() {
    let (sampler, _peak, gate) = counting(true);
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{
    name: "live-phase",
    description: "stamp while running",
    phases: [#{ title: "Plan" }, #{ title: "Research" }],
};
phase("Plan");
let planner = agent("规划", #{ label: "research-planner" });
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(run) = wf_run(&h, &run_id) {
            if let Some(row) = run.agents.iter().find(|a| a.label == "research-planner") {
                assert_eq!(
                    row.phase.as_deref(),
                    Some("Plan"),
                    "跑着时就要归到 Plan：{run:?}"
                );
                break;
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "planner 没起来");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pump = tokio::spawn(async move {
        loop {
            gate.notify_one();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let run = wait_terminal(&h, &run_id).await;
    pump.abort();
    assert_eq!(run.status, "complete", "{run:?}");
}

/// 一个阶段都没声明过的脚本，子代理的 phase 仍是 None。
#[tokio::test]
async fn an_agent_stays_unphased_when_the_script_never_declared_one() {
    let (sampler, _peak, _gate) = counting(false);
    let h = boot(sampler, TaskConfig::default()).await;
    let script = r#"
let meta = #{ name: "no-phase", description: "never calls phase()" };
let a = agent("干活");
complete("done");
"#;
    let run_id = launch(&h, script, 4).await;
    let run = wait_terminal(&h, &run_id).await;

    assert_eq!(run.status, "complete", "{run:?}");
    assert_eq!(run.agents.len(), 1);
    assert_eq!(run.agents[0].phase, None);
}

/// 已经收尾的 run 不能再"停"一次。
#[tokio::test]
async fn a_finished_run_cannot_be_stopped_again() {
    let (sampler, _peak, _gate) = counting(false);
    let h = boot(sampler, TaskConfig::default()).await;
    let run_id = launch(&h, TWO_AGENTS, 4).await;
    let run = wait_terminal(&h, &run_id).await;
    assert_eq!(run.status, "complete", "{run:?}");

    let wf = h.root.require::<Workflows>(WORKFLOWS).unwrap();
    assert_eq!(wf.stop(&run_id), None, "结束了的 run 不该还能停");
}
