//! 内置 code-review 工作流：用按提示词扮演 scope / reviewer / verifier 的假模型，跑真的
//! `code_review.rhai`，看范围判断、去重、核实、排序和报告。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cordis::Context;
use cordis_spine::{
    agent_loop, install_without_llm, tool_task, tool_workflow, turn, AgentPresets, BoxFuture, Llm,
    LlmOutput, LogEvent, PromptRequest, Sampler, StreamDelta, TaskConfig, ToolCall, Tools,
    WorkflowRunSnap, Workflows, AGENT_LOOP, AGENT_PRESETS, LLM, TOOLS, WORKFLOWS,
    WORKFLOW_TOOL_NAME,
};

struct Harness {
    root: Context,
    _home: tempfile::TempDir,
    _env: cordis_base::test_env::EnvScope,
}

/// scope 怎么答。
#[derive(Clone, Copy, Default, PartialEq)]
enum Scope {
    #[default]
    Diff,
    Empty,
    /// 给一条带管道的命令（不是只读的单条 git diff）。
    Unsafe,
    /// 给一条会跑外部程序的 `git diff --ext-diff`。
    ExtDiff,
    /// 审 GitHub PR #42，PR 的提交在本地。
    Pr,
    /// 审 GitHub PR #42，PR 的提交不在本地。
    PrRemote,
}

#[derive(Clone, Default)]
struct Team {
    scope: Scope,
    /// 这个角度的审查员交不出结构化结果。
    broken_angle: Option<&'static str>,
    /// 去重的子代理交不出结构化结果。
    broken_dedupe: bool,
}

struct Scripted {
    team: Team,
    /// 审查员收到的提示里有没有「这是 PR，本地检出不一定是 PR 分支」。
    pr_noted: Arc<AtomicU32>,
    /// 审查员 / 核实员收到「用 git show <sha>:<路径> 读 PR 版本」的次数。
    git_show_hinted: Arc<AtomicU32>,
    /// 收到「PR 的提交不在本地，只看 diff」的次数。
    diff_only_hinted: Arc<AtomicU32>,
    /// 提示里没有「只用只读命令」规矩的子代理数。
    missing_rule: Arc<AtomicU32>,
    /// 起过几个子代理（按第一条任务算）。
    agents: Arc<AtomicU32>,
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let rest = text.split_once(open)?.1;
    Some(rest.split_once(close)?.0)
}

fn fenced(value: serde_json::Value) -> String {
    format!("```json\n{value}\n```")
}

fn finding(file: &str, line: u32, severity: &str, title: &str) -> serde_json::Value {
    serde_json::json!({
        "file": file, "line": line, "severity": severity, "title": title,
        "detail": format!("{title} 的细节"), "suggestion": format!("修 {title}"),
    })
}

impl Scripted {
    fn reply(&self, prompt: &str) -> String {
        if prompt.starts_with("Your final message did not satisfy the output contract") {
            return "还是交不出。".into();
        }
        if prompt.contains("git show abcdef1234567:<path>") {
            self.git_show_hinted.fetch_add(1, Ordering::SeqCst);
        }
        if prompt.contains("The PR's commits are not available locally") {
            self.diff_only_hinted.fetch_add(1, Ordering::SeqCst);
        }
        let is_agent = prompt.contains("Find the code change to review")
            || prompt.contains("Review ONE code change")
            || prompt.contains("<findings-json>");
        if is_agent && !prompt.contains("Use read-only commands only") {
            self.missing_rule.fetch_add(1, Ordering::SeqCst);
        }
        if prompt.contains("Find the code change to review") {
            self.agents.fetch_add(1, Ordering::SeqCst);
            return match self.team.scope {
                Scope::Empty => {
                    fenced(serde_json::json!({ "empty": true, "diff_command": "", "files": [] }))
                }
                Scope::ExtDiff => fenced(serde_json::json!({
                    "empty": false, "diff_command": "git diff --ext-diff HEAD", "files": [{ "path": "a.rs" }],
                })),
                Scope::Unsafe => fenced(serde_json::json!({
                    "empty": false, "diff_command": "git diff HEAD | tee /tmp/x", "files": [{ "path": "a.rs" }],
                })),
                Scope::Pr | Scope::PrRemote => fenced(serde_json::json!({
                    "empty": false, "diff_command": "gh pr diff 42", "pr": 42,
                    "head_sha": "abcdef1234567", "head_local": self.team.scope == Scope::Pr,
                    "description": "PR #42 的改动", "files": [{ "path": "a.rs" }],
                })),
                Scope::Diff => fenced(serde_json::json!({
                    "empty": false,
                    "diff_command": "git diff HEAD",
                    "description": "未提交的改动",
                    "files": [{ "path": "a.rs", "change": "改了解析" }, { "path": "b.rs", "change": "加缓存" }],
                })),
            };
        }
        if prompt.contains("Review ONE code change from a single angle") {
            self.agents.fetch_add(1, Ordering::SeqCst);
            if prompt.contains("the local checkout may not be the PR branch") {
                self.pr_noted.fetch_add(1, Ordering::SeqCst);
            }
            let angle = between(prompt, "Angle: ", ":").unwrap_or_default();
            if self.team.broken_angle == Some(angle) {
                return "看不出来。".into();
            }
            let findings = match angle {
                // 同一处 a.rs:10，两个角度都报：合并成一条，取更严重的那份。
                "Correctness" => vec![finding("a.rs", 10, "high", "解析越界")],
                "Security" => vec![finding("a.rs", 10, "critical", "越界读到别人的数据")],
                // 核实会否掉的误报。
                // 外加一条和 a.rs:10 是同一个问题、只是行号差一行的（要靠去重合并）。
                "Performance and resources" => vec![
                    finding("b.rs", 5, "medium", "误报：缓存没上限"),
                    finding("a.rs", 11, "medium", "同一处越界：循环多走一步"),
                ],
                "Tests" => vec![finding("a.rs", 20, "medium", "新分支没有测试")],
                // 低优先级的放在最前面报，报告里要排到后面。
                "Maintainability" => vec![finding("b.rs", 30, "low", "命名误导")],
                _ => vec![],
            };
            return fenced(serde_json::json!({ "findings": findings }));
        }
        if let Some(packet) = between(prompt, "<candidates-json>\n", "\n</candidates-json>") {
            self.agents.fetch_add(1, Ordering::SeqCst);
            if self.team.broken_dedupe {
                return "分不出来。".into();
            }
            // 标题里带「越界」的是同一个问题；再塞一个不存在的 id 和一个乱写的 id，脚本要忽略。
            let items: Vec<serde_json::Value> = serde_json::from_str(packet).unwrap();
            let mut group: Vec<String> = items
                .iter()
                .filter(|f| f["title"].as_str().unwrap().contains("越界"))
                .map(|f| f["id"].as_str().unwrap().to_string())
                .collect();
            group.push("c999".into());
            group.push("cx".into());
            return fenced(serde_json::json!({ "groups": [group] }));
        }
        if let Some(packet) = between(prompt, "<findings-json>\n", "\n</findings-json>") {
            self.agents.fetch_add(1, Ordering::SeqCst);
            let items: Vec<serde_json::Value> = serde_json::from_str(packet).unwrap();
            let verdicts: Vec<_> = items
                .iter()
                .map(|f| {
                    let real = !f["title"].as_str().unwrap().starts_with("误报");
                    serde_json::json!({
                        "id": f["id"], "real": real,
                        "reason": if real { "代码里确实这样" } else { "代码已经限了大小" },
                    })
                })
                .collect();
            return fenced(serde_json::json!({ "verdicts": verdicts }));
        }
        "（不认识的任务）".into()
    }
}

impl Sampler for Scripted {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        let prompt = request
            .history
            .iter()
            .rev()
            .find_map(|e| match e {
                LogEvent::User(text) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let text = self.reply(&prompt);
        Box::pin(async move {
            on_delta(StreamDelta::Text(text.clone()));
            LlmOutput {
                text,
                ..LlmOutput::default()
            }
        })
    }
}

type Hints = (Arc<AtomicU32>, Arc<AtomicU32>, Arc<AtomicU32>);

fn hints() -> Hints {
    (
        Arc::new(AtomicU32::new(0)),
        Arc::new(AtomicU32::new(0)),
        Arc::new(AtomicU32::new(0)),
    )
}

async fn boot(team: Team, agents: Arc<AtomicU32>) -> Harness {
    boot_with(team, agents, Arc::new(AtomicU32::new(0)), hints()).await
}

async fn boot_with(
    team: Team,
    agents: Arc<AtomicU32>,
    pr_noted: Arc<AtomicU32>,
    hints: Hints,
) -> Harness {
    let home = tempfile::tempdir().unwrap();
    let env = cordis_base::test_env::scoped().set("DOCK_HOME", home.path().join("dock-home"));
    std::fs::create_dir_all(home.path().join("dock-home")).unwrap();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(turn(), ()).unwrap().wait().await.unwrap();
    root.provide(AGENT_PRESETS, AgentPresets::load(home.path().to_path_buf()))
        .unwrap();
    root.plugin(tool_task(), TaskConfig::default())
        .unwrap()
        .wait()
        .await
        .unwrap();
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(Scripted {
                team,
                agents,
                pr_noted,
                git_show_hinted: hints.0,
                diff_only_hinted: hints.1,
                missing_rule: hints.2,
            }),
        ),
    )
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

struct Review {
    run: WorkflowRunSnap,
    report: String,
    agents: u32,
    pr_noted: u32,
    git_show_hinted: u32,
    diff_only_hinted: u32,
    missing_rule: u32,
}

impl Review {
    fn field(&self, key: &str) -> String {
        let prefix = format!("{key}: ");
        self.run
            .result_summary
            .as_deref()
            .unwrap_or_default()
            .lines()
            .find_map(|l| l.strip_prefix(prefix.as_str()))
            .unwrap_or_default()
            .to_string()
    }
}

async fn review(team: Team, args: serde_json::Value) -> Review {
    let agents = Arc::new(AtomicU32::new(0));
    let pr_noted = Arc::new(AtomicU32::new(0));
    let hs = hints();
    let h = boot_with(team, agents.clone(), pr_noted.clone(), hs.clone()).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let out = tools
        .execute(ToolCall {
            id: "wf".into(),
            name: WORKFLOW_TOOL_NAME.into(),
            arguments: serde_json::json!({
                "source": { "type": "name", "name": "code-review" },
                "args": args,
            })
            .to_string(),
        })
        .await;
    let v: serde_json::Value =
        serde_json::from_str(&out.content).unwrap_or_else(|_| panic!("{}", out.content));
    let run_id = v["run_id"].as_str().unwrap().to_string();
    let wf = h.root.require::<Workflows>(WORKFLOWS).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let run = loop {
        if let Some(r) = wf.list().into_iter().find(|r| r.run_id == run_id) {
            if r.status != "active" {
                break r;
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "code-review 没收尾");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(run.status, "complete", "{run:?}");
    let summary = run.result_summary.clone().unwrap_or_default();
    let path = summary
        .lines()
        .find_map(|l| l.strip_prefix("path: "))
        .unwrap_or_else(|| panic!("结果里没有报告路径：{summary}"))
        .to_string();
    let report = std::fs::read_to_string(&path).unwrap();
    Review {
        run,
        report,
        agents: agents.load(Ordering::SeqCst),
        pr_noted: pr_noted.load(Ordering::SeqCst),
        git_show_hinted: hs.0.load(Ordering::SeqCst),
        diff_only_hinted: hs.1.load(Ordering::SeqCst),
        missing_rule: hs.2.load(Ordering::SeqCst),
    }
}

/// 四个角度并行审，同一处的两条合并（取更严重的、两个角度都记上），误报被核实否掉，
/// 按严重程度排序。
#[tokio::test]
async fn findings_are_merged_verified_and_ranked() {
    let r = review(Team::default(), serde_json::json!({})).await;
    let report = &r.report;
    assert_eq!(r.field("status"), "issues", "{report}");
    assert_eq!(r.field("findings"), "3", "{report}");
    assert!(
        report.contains("`git diff HEAD`（未提交的改动），2 个文件"),
        "{report}"
    );
    assert!(
        report.contains("### 1. [严重] 越界读到别人的数据"),
        "同一处取更严重的那份，排第一：{report}"
    );
    assert!(
        report.contains("`a.rs:10` · 正确性、安全、性能"),
        "同一处（含行号差一行的）合成一条，角度都记上：{report}"
    );
    assert!(
        !report.contains("循环多走一步"),
        "重复的那条不能单列：{report}"
    );
    assert!(
        report.contains("### 2. [中] 新分支没有测试"),
        "测试角度的发现：{report}"
    );
    assert!(report.contains("`a.rs:20` · 测试"), "{report}");
    assert!(report.contains("### 3. [低] 命名误导"), "{report}");
    // 给界面的结构化结果：每条批注带位置、严重程度和角度，不摊进给模型看的摘要。
    let data = r.run.result_data.as_deref().expect("result_data");
    assert_eq!(data["kind"], "code-review");
    assert_eq!(data["diff_command"], "git diff HEAD");
    let items = data["findings"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["file"], "a.rs");
    assert_eq!(items[0]["line"], 10);
    assert_eq!(items[0]["severity"], "critical");
    assert_eq!(
        items[0]["angles"],
        serde_json::json!(["正确性", "安全", "性能"])
    );
    let summary = r.run.result_summary.as_deref().unwrap_or_default();
    assert!(!summary.contains("data:"), "结构化结果不进摘要：{summary}");
    assert!(!report.contains("缓存没上限"), "误报不能进报告：{report}");
    assert!(report.contains("1 条发现核实后不成立"), "{report}");
    // scope + 5 个审查员 + 去重 + 1 个核实员。
    assert_eq!(r.agents, 8);
    assert_eq!(r.missing_rule, 0, "每个子代理都要带上只读规矩");
}

/// 没有改动：不起审查员，直接说没东西可审。
#[tokio::test]
async fn nothing_to_review_stops_after_scope() {
    let team = Team {
        scope: Scope::Empty,
        ..Team::default()
    };
    let r = review(team, serde_json::json!({})).await;
    assert_eq!(r.field("status"), "clean", "{}", r.report);
    assert!(r.report.contains("没有要审查的改动"), "{}", r.report);
    assert_eq!(r.agents, 1);
}

/// 审查员要自己跑那条命令：不是单条只读的 git diff / git show 就不往下走。
#[tokio::test]
async fn an_unsafe_diff_command_is_refused() {
    let team = Team {
        scope: Scope::Unsafe,
        ..Team::default()
    };
    let r = review(team, serde_json::json!({})).await;
    assert_eq!(r.field("status"), "failed", "{}", r.report);
    assert!(r.report.contains("不是一条只读的"), "{}", r.report);
    assert_eq!(r.agents, 1, "不能带着这条命令去起审查员");
}

/// 一个角度没审成：报告照出，记 partial，并点名没审到的角度。
#[tokio::test]
async fn a_failed_reviewer_is_named_and_marks_the_review_partial() {
    let team = Team {
        broken_angle: Some("Security"),
        ..Team::default()
    };
    let r = review(team, serde_json::json!({})).await;
    assert_eq!(r.field("status"), "partial", "{}", r.report);
    assert!(r.report.contains("这次没审到：安全"), "{}", r.report);
    assert!(
        r.report.contains("[高] 解析越界"),
        "别的角度照常进报告：{}",
        r.report
    );
}

/// 只审指定的角度；不认识的当自定义要求。
#[tokio::test]
async fn angles_can_be_chosen() {
    let r = review(
        Team::default(),
        serde_json::json!({ "angles": ["security", "检查 SQL 拼接"], "focus": "鉴权" }),
    )
    .await;
    assert!(
        r.report
            .contains("**角度**：安全、检查 SQL 拼接；关注：鉴权"),
        "{}",
        r.report
    );
    // scope + 2 个审查员 + 1 个核实员。
    assert_eq!(r.agents, 4);
}

/// 冒烟（canned host 的假结果没有 diff_command）也要能走完，不能在脚本里报错。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validate_only_smoke_passes() {
    let agents = Arc::new(AtomicU32::new(0));
    let h = boot(Team::default(), agents).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let out = tools
        .execute(ToolCall {
            id: "wf".into(),
            name: WORKFLOW_TOOL_NAME.into(),
            arguments: serde_json::json!({
                "source": { "type": "name", "name": "code-review" },
                "validate_only": true,
            })
            .to_string(),
        })
        .await;
    assert!(out.content.contains("冒烟检查通过"), "{}", out.content);
}

/// 去重的子代理失败：退回精确的 file:line 合并，照样出报告（行号差一行的那条单列）。
#[tokio::test]
async fn a_failed_dedupe_falls_back_to_exact_merges() {
    let team = Team {
        broken_dedupe: true,
        ..Team::default()
    };
    let r = review(team, serde_json::json!({})).await;
    assert_eq!(r.field("status"), "issues", "{}", r.report);
    assert_eq!(r.field("findings"), "4", "{}", r.report);
    assert!(
        r.report.contains("`a.rs:10` · 正确性、安全"),
        "{}",
        r.report
    );
    assert!(r.report.contains("循环多走一步"), "{}", r.report);
}

/// 审 GitHub PR：范围是 `gh pr diff <n>`，报告写明 PR 号，结构化结果带上 pr；审查员被告知
/// 本地检出不一定是 PR 的分支。
#[tokio::test]
async fn a_pull_request_is_reviewed_with_gh() {
    let team = Team {
        scope: Scope::Pr,
        ..Team::default()
    };
    let r = review(team, serde_json::json!({ "pr": 42 })).await;
    assert!(
        r.report.contains("**范围**：PR #42（`gh pr diff 42`）"),
        "{}",
        r.report
    );
    let data = r.run.result_data.as_deref().expect("result_data");
    assert_eq!(data["pr"], 42);
    assert_eq!(data["diff_command"], "gh pr diff 42");
    assert_eq!(r.pr_noted, 5, "5 个审查员都要被提醒以 diff 为准");
    // 回归：审 PR 时子代理为了看 PR 版本的文件把仓库拷到磁盘上，一次弹 75 次批准。提交在本地就
    // 告诉它们用只读的 `git show <sha>:<路径>`（审查员 5 个 + 核实员 1 个）。
    assert_eq!(
        r.git_show_hinted, 6,
        "审查员和核实员都要知道怎么只读地看 PR 版本"
    );
    assert_eq!(r.diff_only_hinted, 0);
    assert_eq!(r.missing_rule, 0);
}

/// PR 的提交不在本地：只告诉它们看 diff，不给 `git show`（也不能让它们去下载文件）。
#[tokio::test]
async fn a_remote_pull_request_is_judged_from_the_diff() {
    let team = Team {
        scope: Scope::PrRemote,
        ..Team::default()
    };
    let r = review(team, serde_json::json!({ "pr": 42 })).await;
    assert_eq!(r.git_show_hinted, 0);
    assert_eq!(r.diff_only_hinted, 6);
}

/// `--ext-diff` / `--textconv` 会跑 git 配置里的外部程序，`--no-index` 能比任意路径：审查员
/// 会不经批准跑这条命令，所以这类参数也拒。
#[tokio::test]
async fn a_diff_command_that_runs_external_programs_is_refused() {
    let team = Team {
        scope: Scope::ExtDiff,
        ..Team::default()
    };
    let r = review(team, serde_json::json!({})).await;
    assert_eq!(r.field("status"), "failed", "{}", r.report);
    assert_eq!(r.agents, 1);
}
