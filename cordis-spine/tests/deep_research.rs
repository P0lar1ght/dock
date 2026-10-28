//! 内置 deep-research 脚本的评分逻辑：用一个按提示词扮演 planner / researcher /
//! verifier / synthesizer 的假模型，跑真的 `deep_research.rhai`，看报告的 status 与覆盖说明。

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

/// 假研究员团队的行为开关。
#[derive(Clone, Default)]
struct Team {
    /// planner 拆出的问题数。
    questions: usize,
    /// 每个问题交几条结论。
    claims_per_question: usize,
    /// 研究员都写一条「不确定」（真模型几乎总会写）。
    uncertainty: bool,
    /// verifier 多交一条不在包里的 verdict。
    verifier_extra_id: bool,
    /// verifier 漏掉包里的第一条。
    verifier_skips_first: bool,
    /// 这个问题的结论 verifier 一律判不支持。
    reject_question: Option<usize>,
    /// 这个问题的研究员交不出结构化结果（重试也不行）。
    garbage_question: Option<usize>,
    /// verifier 交不出结构化结果（重试也不行）。
    verifier_garbage: bool,
}

struct Scripted(Team);

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let rest = text.split_once(open)?.1;
    Some(rest.split_once(close)?.0)
}

fn fenced(value: serde_json::Value) -> String {
    format!("```json\n{value}\n```")
}

impl Scripted {
    fn reply(&self, prompt: &str) -> String {
        let t = &self.0;
        if prompt.contains("independent questions") && prompt.contains("<query-json>") {
            let qs: Vec<String> = (0..t.questions).map(|i| format!("question-{i}")).collect();
            return fenced(serde_json::json!({ "questions": qs }));
        }
        if let Some(q) = between(prompt, "<question-json>\n", "\n</question-json>") {
            let q: String = serde_json::from_str(q).unwrap();
            let qi = q.trim_start_matches("question-");
            if t.garbage_question.is_some_and(|g| qi == g.to_string()) {
                return "查不到，放弃了。".into();
            }
            let claims: Vec<_> = (0..t.claims_per_question)
                .map(|k| {
                    serde_json::json!({
                        "claim": format!("Q{qi} fact {k}"),
                        "evidence": format!("evidence for Q{qi} fact {k}"),
                        "source_title": format!("Source {qi}-{k}"),
                        "source_locator": format!("https://example.com/{qi}/{k}"),
                        "source_type": "primary",
                        "confidence": "high",
                    })
                })
                .collect();
            let uncertainties: Vec<String> = if t.uncertainty {
                vec![format!("Q{qi}: 2025 年之后的变化没查到")]
            } else {
                vec![]
            };
            return fenced(serde_json::json!({ "claims": claims, "uncertainties": uncertainties }));
        }
        if let Some(packet) = between(
            prompt,
            "<candidate-claims-json>\n",
            "\n</candidate-claims-json>",
        ) {
            if t.verifier_garbage {
                return "来源打不开。".into();
            }
            let claims: Vec<serde_json::Value> = serde_json::from_str(packet).unwrap();
            let mut verdicts: Vec<_> = claims
                .iter()
                .enumerate()
                .filter(|(i, _)| !(t.verifier_skips_first && *i == 0))
                .map(|(_, c)| {
                    let id = c["id"].as_str().unwrap();
                    let text = c["claim"].as_str().unwrap();
                    let rejected = t
                        .reject_question
                        .is_some_and(|q| text.starts_with(&format!("Q{q} ")));
                    serde_json::json!({
                        "claim_id": id,
                        "supported": !rejected,
                        "reason": if rejected { "来源里没有这句话" } else { "来源原文支持" },
                        "evidence": format!("checked {id}"),
                        "source_title": "Independent",
                        "source_locator": format!("https://check.example.com/{id}"),
                    })
                })
                .collect();
            if t.verifier_extra_id {
                verdicts.push(serde_json::json!({
                    "claim_id": "claim-999", "supported": true, "reason": "多出来的",
                    "evidence": "x", "source_title": "x", "source_locator": "x",
                }));
            }
            return fenced(serde_json::json!({ "verdicts": verdicts }));
        }
        if let Some(packet) = between(
            prompt,
            "<verified-findings-json>\n",
            "\n</verified-findings-json>",
        ) {
            let findings: Vec<serde_json::Value> = serde_json::from_str(packet).unwrap();
            let body: Vec<String> = findings
                .iter()
                .map(|f| {
                    format!(
                        "{} [{}].",
                        f["claim"].as_str().unwrap(),
                        f["citation"].as_str().unwrap()
                    )
                })
                .collect();
            return format!(
                "<report-body>\nDirect answer.\n\n{}\n</report-body>",
                body.join("\n")
            );
        }
        // 契约重试：上一轮交不出结构化结果的研究员，这一轮还是交不出。
        if prompt.starts_with("Your final message did not satisfy the output contract") {
            return "还是查不到。".into();
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

async fn boot(team: Team) -> Harness {
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
        Llm::from_sampler(root.clone(), Arc::new(Scripted(team))),
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

/// 跑一次 deep-research，回（结局快照，报告全文）。
async fn research(team: Team, breadth: usize) -> (WorkflowRunSnap, String) {
    let h = boot(team).await;
    let tools = h.root.require::<Tools>(TOOLS).unwrap();
    let out = tools
        .execute(ToolCall {
            id: "wf".into(),
            name: WORKFLOW_TOOL_NAME.into(),
            arguments: serde_json::json!({
                "source": { "type": "name", "name": "deep-research" },
                "args": { "query": "测试问题", "breadth": breadth },
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
        assert!(
            tokio::time::Instant::now() < deadline,
            "deep-research 没收尾"
        );
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
    (run, report)
}

fn status_of(run: &WorkflowRunSnap) -> String {
    run.result_summary
        .as_deref()
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("status: "))
        .unwrap_or_default()
        .to_string()
}

fn base() -> Team {
    Team {
        questions: 2,
        claims_per_question: 2,
        ..Team::default()
    }
}

#[tokio::test]
async fn everything_checks_out_and_the_report_is_verified() {
    let (run, report) = research(base(), 2).await;
    assert_eq!(status_of(&run), "verified", "{report}");
    assert!(report.contains("[S4]"), "{report}");
}

/// 回归：研究员写了「不确定」不等于没覆盖。真模型几乎每次都会写几条不确定，以前
/// 任何一条都把整份报告打成 Partial——12 条结论全过了核验也是 Partial。
#[tokio::test]
async fn uncertainties_are_reported_but_do_not_make_the_report_partial() {
    let team = Team {
        uncertainty: true,
        ..base()
    };
    let (run, report) = research(team, 2).await;
    assert_eq!(status_of(&run), "verified", "{report}");
    assert!(
        report.contains("2025 年之后的变化没查到"),
        "不确定之处仍要写进覆盖说明：{report}"
    );
}

/// 回归：verifier 多交一条不在包里的 verdict，不该把它负责的所有结论一起扔掉。
/// 以前整片（一半的结论）作废，报告里只剩另一半。
#[tokio::test]
async fn a_stray_verdict_does_not_discard_the_whole_verifier_shard() {
    let team = Team {
        verifier_extra_id: true,
        ..base()
    };
    let (run, report) = research(team, 2).await;
    for n in 1..=4 {
        assert!(
            report.contains(&format!("[S{n}]")),
            "第 {n} 条结论丢了：{report}"
        );
    }
    assert_eq!(status_of(&run), "verified", "{report}");
}

/// verifier 漏掉一条：只有那一条算没核验，别的照收；那一条的问题还有别的结论，覆盖不缺。
#[tokio::test]
async fn a_missing_verdict_only_excludes_that_claim() {
    let team = Team {
        verifier_skips_first: true,
        ..base()
    };
    let (run, report) = research(team, 2).await;
    assert!(report.contains("[S3]"), "其余三条要留下：{report}");
    assert!(!report.contains("[S4]"), "漏掉的那条不能算核验过：{report}");
    assert!(report.contains("claim-0"), "要说清哪条没核验：{report}");
    assert_eq!(status_of(&run), "verified", "{report}");
}

/// 一个问题的结论全被核验否掉：那个问题没覆盖，报告是 Partial，并说清是哪个问题。
#[tokio::test]
async fn a_question_with_no_verified_claim_makes_the_report_partial() {
    let team = Team {
        reject_question: Some(1),
        ..base()
    };
    let (run, report) = research(team, 2).await;
    assert_eq!(status_of(&run), "partial", "{report}");
    assert!(
        report.contains("question-1"),
        "要点名没覆盖的问题：{report}"
    );
}

/// 回归：候选结论的上限要在问题之间分，不能先到先得。breadth 6、每题 6 条时总数 36
/// 超过上限 24，以前前四题占满、后两题一条都进不了核验。
#[tokio::test]
async fn the_candidate_cap_is_shared_across_questions() {
    let team = Team {
        questions: 6,
        claims_per_question: 6,
        ..Team::default()
    };
    let (_run, report) = research(team, 6).await;
    for q in 0..6 {
        assert!(
            report.contains(&format!("Q{q} fact 0")),
            "问题 {q} 一条结论都没进报告：{report}"
        );
    }
}

/// 一个问题的研究员交不出结构化结果：别的问题照常核验进报告，报告记 Partial 并点名。
#[tokio::test]
async fn a_failed_researcher_makes_the_report_partial_but_keeps_the_rest() {
    let team = Team {
        garbage_question: Some(1),
        ..base()
    };
    let (run, report) = research(team, 2).await;
    assert_eq!(status_of(&run), "partial", "{report}");
    assert!(
        report.contains("Q0 fact 1"),
        "另一个问题的结论要留下：{report}"
    );
    assert!(report.contains("Question 2 failed"), "{report}");
}

/// verifier 整个失败：它那片一条都没法核验，报告如实说，不当成核验通过。
#[tokio::test]
async fn a_failed_verifier_leaves_its_claims_unverified() {
    let team = Team {
        verifier_garbage: true,
        ..base()
    };
    let (run, report) = research(team, 2).await;
    assert_eq!(status_of(&run), "partial", "{report}");
    assert!(report.contains("could not be verified"), "{report}");
    assert!(!report.contains("[S1]"), "没核验的不能当结论：{report}");
}
