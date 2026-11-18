//! 内置 code-review 工作流：用按提示词扮演审查员 / 核实员的假模型，跑真的 `code_review.rhai`，
//! 看范围、合并、核实、排序、档位和报告。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cordis::Context;
use cordis_spine::{
    agent_loop, install_without_llm, tool_task, tool_workflow, turn, AgentPresets, BoxFuture, Llm,
    LlmOutput, LogEvent, PromptRequest, Sampler, StreamDelta, TaskConfig, ToolCall, Tools,
    WorkflowRunSnap, Workflows, AGENT_LOOP, AGENT_PRESETS, LLM, TOOLS, WORKFLOWS,
    WORKFLOW_TOOL_NAME,
};
use serde_json::{json, Value};

struct Harness {
    root: Context,
    _home: tempfile::TempDir,
    _env: cordis_base::test_env::EnvScope,
}

/// 审查员找到的范围。
#[derive(Clone, Copy, Default, PartialEq)]
enum Scope {
    #[default]
    Diff,
    Empty,
    /// 给一条带管道的命令（不是只读的单条 git diff）。
    Unsafe,
    /// 审 GitHub PR #42。
    Pr,
}

#[derive(Clone, Default)]
struct Team {
    scope: Scope,
    /// 审查员交不出结构化结果。
    broken_reviewer: bool,
    /// 核实员交不出结构化结果。
    broken_verifier: bool,
    /// 审查员说有一部分没看。
    skipped: bool,
    /// 审查员把 skipped 填成「无」（没跳过，但没留空）。
    skipped_none: bool,
}

#[derive(Default)]
struct Seen {
    /// 起过几个子代理（按第一条任务算，契约重试不算）。
    agents: AtomicU32,
    /// 提示里没有「只用只读命令」规矩的子代理数。
    missing_rule: AtomicU32,
    reviewer_prompt: Mutex<String>,
    verifier_prompt: Mutex<String>,
}

struct Scripted {
    team: Team,
    seen: Arc<Seen>,
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let rest = text.split_once(open)?.1;
    Some(rest.split_once(close)?.0)
}

fn fenced(value: Value) -> String {
    format!("```json\n{value}\n```")
}

fn finding(
    file: &str,
    line: u32,
    severity: &str,
    category: &str,
    confidence: f64,
    title: &str,
) -> Value {
    json!({
        "file": file, "line": line, "severity": severity, "category": category,
        "confidence": confidence, "title": title,
        "detail": format!("{title} 的细节"), "suggestion": format!("修 {title}"),
    })
}

impl Scripted {
    fn reply(&self, prompt: &str) -> String {
        if prompt.starts_with("Your final message did not satisfy the output contract") {
            return "还是交不出。".into();
        }
        let reviewer = prompt.contains("You are reviewing a code change made by another engineer");
        let verifier = prompt.contains("<findings-json>");
        if (reviewer || verifier) && !prompt.contains("Use read-only commands only") {
            self.seen.missing_rule.fetch_add(1, Ordering::SeqCst);
        }
        if reviewer {
            self.seen.agents.fetch_add(1, Ordering::SeqCst);
            *self.seen.reviewer_prompt.lock().unwrap() = prompt.to_string();
            if self.team.broken_reviewer {
                return "看不出来。".into();
            }
            let findings = vec![
                // 同一处 a.rs:10 报了两条：合并成一条，取更严重的那份，类别都记上。
                finding("a.rs", 10, "high", "correctness", 0.9, "解析越界"),
                finding(
                    "a.rs",
                    10,
                    "critical",
                    "security",
                    0.8,
                    "越界读到别人的数据",
                ),
                // 核实会否掉的误报；审查员自己也没把握（快速档按置信度去掉）。
                finding("b.rs", 5, "medium", "performance", 0.3, "误报：缓存没上限"),
                // 测试类最多算「中」。
                finding("a.rs", 20, "high", "tests", 0.7, "新分支没有测试"),
                // 低优先级的放在最前面也要排到后面。
                finding("b.rs", 30, "low", "maintainability", 0.6, "命名误导"),
            ];
            let mut out = match self.team.scope {
                Scope::Empty => {
                    return fenced(
                        json!({ "empty": true, "diff_command": "", "files": [], "findings": [] }),
                    )
                }
                Scope::Diff => json!({
                    "empty": false,
                    "diff_command": "git diff HEAD",
                    "description": "未提交的改动",
                    "files": [{ "path": "a.rs", "change": "改了解析" }, { "path": "b.rs", "change": "加缓存" }],
                }),
                Scope::Unsafe => json!({
                    "empty": false,
                    "diff_command": "git diff HEAD | tee /tmp/x",
                    "description": "未提交的改动",
                    "files": [{ "path": "a.rs" }],
                }),
                Scope::Pr => json!({
                    "empty": false,
                    "diff_command": "gh pr diff 42",
                    "pr": 42,
                    "head_sha": "abcdef1234567",
                    "description": "PR #42 的改动",
                    "files": [{ "path": "a.rs" }],
                }),
            };
            out["findings"] = json!(findings);
            out["correct"] = json!(false);
            out["overall"] = json!("越界读会泄露数据，要先修");
            if self.team.skipped {
                out["skipped"] = json!("vendor/ 下的生成文件没看");
            }
            if self.team.skipped_none {
                out["skipped"] = json!("无");
            }
            return fenced(out);
        }
        if let Some(packet) = between(prompt, "<findings-json>\n", "\n</findings-json>") {
            self.seen.agents.fetch_add(1, Ordering::SeqCst);
            *self.seen.verifier_prompt.lock().unwrap() = prompt.to_string();
            if self.team.broken_verifier {
                return "核不了。".into();
            }
            let items: Vec<Value> = serde_json::from_str(packet).unwrap();
            let verdicts: Vec<_> = items
                .iter()
                .map(|f| {
                    let real = !f["title"].as_str().unwrap().starts_with("误报");
                    json!({
                        "id": f["id"], "real": real,
                        "reason": if real { "代码里确实这样" } else { "代码已经限了大小" },
                    })
                })
                .collect();
            return fenced(json!({ "verdicts": verdicts }));
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

async fn boot(team: Team, seen: Arc<Seen>) -> Harness {
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
        Llm::from_sampler(root.clone(), Arc::new(Scripted { team, seen })),
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
    seen: Arc<Seen>,
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

    fn agents(&self) -> u32 {
        self.seen.agents.load(Ordering::SeqCst)
    }

    fn reviewer_prompt(&self) -> String {
        self.seen.reviewer_prompt.lock().unwrap().clone()
    }

    fn verifier_prompt(&self) -> String {
        self.seen.verifier_prompt.lock().unwrap().clone()
    }
}

async fn review(team: Team, args: Value) -> Review {
    let seen = Arc::new(Seen::default());
    let h = boot(team, seen.clone()).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let out = tools
        .execute(ToolCall {
            id: "wf".into(),
            name: WORKFLOW_TOOL_NAME.into(),
            arguments: json!({
                "source": { "type": "name", "name": "code-review" },
                "args": args,
            })
            .to_string(),
        })
        .await;
    let v: Value = serde_json::from_str(&out.content).unwrap_or_else(|_| panic!("{}", out.content));
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
    Review { run, report, seen }
}

/// 标准档：一个审查员（自己定范围、通读）+ 一个核实员。同一处的两条合并（取更严重的、类别都记上），
/// 测试类封顶「中」，误报被核实否掉，按严重程度排；带整体判断。
#[tokio::test]
async fn one_reviewer_and_one_verifier_merge_verify_and_rank() {
    let r = review(Team::default(), json!({})).await;
    let report = &r.report;
    assert_eq!(r.field("status"), "issues", "{report}");
    assert_eq!(r.field("findings"), "3", "{report}");
    assert!(
        report.contains("**范围**：`git diff HEAD`（未提交的改动），2 个文件"),
        "{report}"
    );
    assert!(report.contains("**档位**：标准"), "{report}");
    assert!(
        report.contains("**整体判断**：有要先处理的问题——越界读会泄露数据，要先修"),
        "{report}"
    );
    assert!(
        report.contains("### 1. [严重] 越界读到别人的数据"),
        "同一处取更严重的那份，排第一：{report}"
    );
    assert!(report.contains("`a.rs:10` · 安全、正确性"), "{report}");
    assert!(!report.contains("解析越界"), "合并掉的那条不单列：{report}");
    assert!(
        report.contains("### 2. [中] 新分支没有测试"),
        "测试类封顶「中」：{report}"
    );
    assert!(report.contains("### 3. [低] 命名误导"), "{report}");
    assert!(!report.contains("缓存没上限"), "误报不能进报告：{report}");
    assert!(report.contains("1 条发现核实后不成立"), "{report}");
    assert!(report.contains("> 核实：代码里确实这样"), "{report}");
    // 给界面的结构化结果：契约不变（位置、严重程度、类别、核实说明），不摊进给模型看的摘要。
    let data = r.run.result_data.as_deref().expect("result_data");
    assert_eq!(data["kind"], "code-review");
    assert_eq!(data["diff_command"], "git diff HEAD");
    assert_eq!(data["pr"], 0);
    let items = data["findings"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["file"], "a.rs");
    assert_eq!(items[0]["line"], 10);
    assert_eq!(items[0]["severity"], "critical");
    assert_eq!(items[0]["angles"], json!(["安全", "正确性"]));
    assert_eq!(items[0]["note"], "代码里确实这样");
    let summary = r.run.result_summary.as_deref().unwrap_or_default();
    assert!(!summary.contains("data:"), "结构化结果不进摘要：{summary}");
    // 审查员 + 核实员，没有单独的范围 / 去重子代理。
    assert_eq!(r.agents(), 2);
    assert_eq!(
        r.seen.missing_rule.load(Ordering::SeqCst),
        0,
        "每个子代理都要带上只读规矩"
    );
    // 审查员自己把改动打一次、读一遍；核实员按审查员给的命令看改动。
    assert!(r.reviewer_prompt().contains("Print the change ONCE"));
    assert!(r.verifier_prompt().contains("`git diff HEAD`"));
}

/// 没有改动：审查员说空，直接收尾，不起核实员。
#[tokio::test]
async fn nothing_to_review_stops_after_the_reviewer() {
    let team = Team {
        scope: Scope::Empty,
        ..Team::default()
    };
    let r = review(team, json!({})).await;
    assert_eq!(r.field("status"), "clean", "{}", r.report);
    assert!(r.report.contains("没有要审查的改动"), "{}", r.report);
    assert_eq!(r.agents(), 1);
}

/// 范围命令不是单条只读的 git diff：发现照出，但不把这条命令交给核实员、也不交给界面；
/// 报告里说清楚。
#[tokio::test]
async fn an_unsafe_diff_command_is_not_passed_on() {
    let team = Team {
        scope: Scope::Unsafe,
        ..Team::default()
    };
    let r = review(team, json!({})).await;
    assert_eq!(r.field("status"), "issues", "{}", r.report);
    assert!(r.report.contains("不是一条只读的"), "{}", r.report);
    let data = r.run.result_data.as_deref().expect("result_data");
    assert_eq!(data["diff_command"], "");
    assert!(
        !r.verifier_prompt().contains("tee"),
        "{}",
        r.verifier_prompt()
    );
    assert!(r.verifier_prompt().contains("未提交的改动"));
}

/// 审查员没交回结果：直接说没审成（契约重试那一次也失败）。
#[tokio::test]
async fn a_failed_reviewer_fails_the_review() {
    let team = Team {
        broken_reviewer: true,
        ..Team::default()
    };
    let r = review(team, json!({})).await;
    assert_eq!(r.field("status"), "failed", "{}", r.report);
    assert!(
        r.report.contains("审查员没有交回可用的结果"),
        "{}",
        r.report
    );
    assert_eq!(r.agents(), 1);
}

/// 核实员没交回结果：发现照列、标「未核实」，记 partial，不整份丢掉。
#[tokio::test]
async fn a_failed_verifier_keeps_the_findings_unverified() {
    let team = Team {
        broken_verifier: true,
        ..Team::default()
    };
    let r = review(team, json!({})).await;
    assert_eq!(r.field("status"), "partial", "{}", r.report);
    assert_eq!(
        r.field("findings"),
        "4",
        "误报没人否掉，也照列：{}",
        r.report
    );
    assert!(r.report.contains("下面的发现都没核实"), "{}", r.report);
    assert!(r.report.contains("> 核实：未核实"), "{}", r.report);
}

/// 快速档（参数或文字里的「快速」）：只有审查员，不核实；审查员自己没把握的去掉。
#[tokio::test]
async fn the_quick_level_skips_verification() {
    for args in [json!({ "level": "quick" }), json!({ "query": "快速" })] {
        let r = review(Team::default(), args.clone()).await;
        assert_eq!(r.agents(), 1, "{args}");
        assert_eq!(r.field("findings"), "3", "{args}：{}", r.report);
        assert!(r.report.contains("**档位**：快速"), "{}", r.report);
        assert!(
            r.report.contains("快速档：发现没有逐条核实"),
            "{}",
            r.report
        );
        assert!(
            r.report.contains("1 条审查员自己没把握的发现"),
            "{}",
            r.report
        );
        assert!(!r.report.contains("缓存没上限"), "{}", r.report);
        assert!(
            r.reviewer_prompt()
                .contains("<request-json>\n\"\"\n</request-json>"),
            "「快速」是档位，不是范围：{}",
            r.reviewer_prompt()
        );
    }
    // 文字里的范围照样交给审查员。
    let r = review(Team::default(), json!({ "query": "快速 #42" })).await;
    assert_eq!(r.agents(), 1);
    assert!(r
        .reviewer_prompt()
        .contains("<request-json>\n\"#42\"\n</request-json>"));
}

/// 审 GitHub PR：报告写明 PR 号、结构化结果带上 pr；核实员知道怎么只读地看 PR 版本的文件。
#[tokio::test]
async fn a_pull_request_is_reported_and_the_verifier_reads_the_pr_version() {
    let team = Team {
        scope: Scope::Pr,
        ..Team::default()
    };
    let r = review(team, json!({ "pr": 42 })).await;
    assert!(
        r.report.contains("**范围**：PR #42（`gh pr diff 42`）"),
        "{}",
        r.report
    );
    assert!(r.reviewer_prompt().contains("PR #42"));
    assert!(r
        .reviewer_prompt()
        .contains("gh pr view <number> --json headRefOid"));
    let data = r.run.result_data.as_deref().expect("result_data");
    assert_eq!(data["pr"], 42);
    assert_eq!(data["diff_command"], "gh pr diff 42");
    let v = r.verifier_prompt();
    assert!(v.contains("This is GitHub PR #42"), "{v}");
    assert!(v.contains("git show abcdef1234567:<path>"), "{v}");
}

/// 关注点和要特别看的方面交给审查员，报告里写上；不再按角度拆审查员。
#[tokio::test]
async fn focus_and_angles_go_to_the_one_reviewer() {
    let r = review(
        Team::default(),
        json!({ "angles": ["security", "检查 SQL 拼接"], "focus": "鉴权" }),
    )
    .await;
    assert!(
        r.report
            .contains("；关注：鉴权；也请特别看：安全、检查 SQL 拼接"),
        "{}",
        r.report
    );
    assert!(r
        .reviewer_prompt()
        .contains("也请特别看：安全、检查 SQL 拼接"));
    assert_eq!(r.agents(), 2);
}

/// 审查员说有没看的部分：照出报告，记 partial，覆盖说明里写明。
#[tokio::test]
async fn skipped_parts_mark_the_review_partial() {
    let team = Team {
        skipped: true,
        ..Team::default()
    };
    let r = review(team, json!({})).await;
    assert_eq!(r.field("status"), "partial", "{}", r.report);
    assert!(
        r.report
            .contains("审查员没看的部分：vendor/ 下的生成文件没看"),
        "{}",
        r.report
    );
}

/// skipped 填成「无」：不算有没看的部分，状态照常（不是 partial），也不写进覆盖说明。
#[tokio::test]
async fn a_skipped_field_saying_none_is_not_a_gap() {
    let team = Team {
        skipped_none: true,
        ..Team::default()
    };
    let r = review(team, json!({})).await;
    assert_eq!(r.field("status"), "issues", "{}", r.report);
    assert!(!r.report.contains("审查员没看的部分"), "{}", r.report);
}

/// 冒烟（canned host 的假结果没有 findings）也要能走完，不能在脚本里报错。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validate_only_smoke_passes() {
    let h = boot(Team::default(), Arc::new(Seen::default())).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let out = tools
        .execute(ToolCall {
            id: "wf".into(),
            name: WORKFLOW_TOOL_NAME.into(),
            arguments: json!({
                "source": { "type": "name", "name": "code-review" },
                "validate_only": true,
            })
            .to_string(),
        })
        .await;
    assert!(out.content.contains("冒烟检查通过"), "{}", out.content);
}
