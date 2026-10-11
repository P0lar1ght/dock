//! 一次回复里的多个工具调用：连在一起的只读调用一起跑，别的一颗一颗跑；结果按模型
//! 给的顺序记。参考 Codex 的 `tools/parallel.rs`（能并行的拿读锁，别的拿写锁）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cordis::Context;
use cordis_spine::Llm;
use cordis_spine::{
    agent_loop, install_without_llm, BoxFuture, LlmOutput, LogEvent, LoopHandle, PromptRequest,
    Sampler, Sessions, StreamDelta, ToolBody, ToolCall, ToolResult, ToolSpec, Tools, AGENT_LOOP,
    LLM, SESSIONS, TOOLS,
};

fn isolated_home() {
    static HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let path = tempfile::tempdir().unwrap().keep();
        std::env::set_var("DOCK_HOME", &path);
        std::env::set_var("DOCK_CUA_DRIVER", "off");
        path
    });
}

/// 第 0 次采样一口气发 `calls`，之后回一句话收尾。
struct Batch {
    n: AtomicUsize,
    calls: Vec<(&'static str, &'static str)>,
}

impl Sampler for Batch {
    fn sample<'a>(
        &'a self,
        _request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        Box::pin(async move {
            if self.n.fetch_add(1, Ordering::SeqCst) == 0 {
                return LlmOutput {
                    tool_calls: self
                        .calls
                        .iter()
                        .map(|(id, name)| ToolCall {
                            id: (*id).into(),
                            name: (*name).into(),
                            arguments: "{}".into(),
                        })
                        .collect(),
                    ..LlmOutput::default()
                };
            }
            LlmOutput {
                text: "done".into(),
                ..LlmOutput::default()
            }
        })
    }
}

/// 探针工具：睡一会儿，记下同时在跑的最多几个、每个调用的起止时间。
#[derive(Default)]
struct Probe {
    running: AtomicUsize,
    peak: AtomicUsize,
    spans: Mutex<Vec<(String, Instant, Instant)>>,
}

fn probe_body(probe: Arc<Probe>) -> ToolBody {
    Arc::new(move |call| {
        let probe = probe.clone();
        Box::pin(async move {
            let now = probe.running.fetch_add(1, Ordering::SeqCst) + 1;
            probe.peak.fetch_max(now, Ordering::SeqCst);
            let start = Instant::now();
            tokio::time::sleep(Duration::from_millis(200)).await;
            probe.running.fetch_sub(1, Ordering::SeqCst);
            probe
                .spans
                .lock()
                .unwrap()
                .push((call.id.clone(), start, Instant::now()));
            ToolResult {
                call_id: call.id.clone(),
                name: call.name,
                content: format!("ran {}", call.id),
                ..Default::default()
            }
        })
    })
}

async fn run(calls: Vec<(&'static str, &'static str)>) -> (Arc<Probe>, Vec<String>) {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(cordis_spine::turn(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let probe = Arc::new(Probe::default());
    let tools = root.require::<Tools>(TOOLS).unwrap();
    // 测试环境里记忆默认关，这三颗没注册：借它们的名字（前两颗只读，后一颗改动）。
    let mut keep = Vec::new();
    for name in ["memory_search", "memory_get", "memory_write"] {
        keep.push(
            tools
                .register(
                    ToolSpec {
                        name: name.into(),
                        description: "probe".into(),
                        parameters_json: r#"{"type":"object"}"#.into(),
                    },
                    probe_body(probe.clone()),
                )
                .unwrap(),
        );
    }
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(Batch {
                n: AtomicUsize::new(0),
                calls,
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    root.require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("go")
        .await
        .unwrap();
    let order = root
        .require::<Sessions>(SESSIONS)
        .unwrap()
        .events()
        .iter()
        .filter_map(|e| match e {
            LogEvent::ToolExecute { id, content, .. } => {
                assert_eq!(content, &format!("ran {id}"), "结果要对得上调用");
                Some(id.clone())
            }
            _ => None,
        })
        .collect();
    drop(keep);
    (probe, order)
}

/// 两颗只读调用一起跑；改动的那颗等它们跑完、自己单独跑，后面的只读调用等它跑完；
/// 结果按模型给的顺序记。
#[tokio::test]
async fn read_only_calls_in_one_reply_run_together() {
    let (probe, order) = run(vec![
        ("c0", "memory_search"),
        ("c1", "memory_get"),
        ("c2", "memory_write"),
        ("c3", "memory_search"),
    ])
    .await;
    assert_eq!(order, ["c0", "c1", "c2", "c3"], "结果按模型给的顺序记");
    assert_eq!(probe.peak.load(Ordering::SeqCst), 2, "c0、c1 应该一起跑");
    let spans = probe.spans.lock().unwrap().clone();
    let span = |id: &str| {
        spans
            .iter()
            .find(|(i, ..)| i == id)
            .map(|(_, s, e)| (*s, *e))
            .unwrap()
    };
    let (_, c0_end) = span("c0");
    let (_, c1_end) = span("c1");
    let (c2_start, c2_end) = span("c2");
    let (c3_start, _) = span("c3");
    assert!(
        c2_start >= c0_end && c2_start >= c1_end,
        "改动的要等前面的读跑完"
    );
    assert!(c3_start >= c2_end, "改动之后的读要等它跑完");
}
