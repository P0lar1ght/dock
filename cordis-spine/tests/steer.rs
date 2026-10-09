//! 插话（steer）：一轮进行中送来的用户消息在下一个步骤边界并进这一轮，
//! 不打断正在进行的采样和工具。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cordis::Context;
use cordis_base::types::{INTERRUPTED_TOOL_RESULT, STEER_REMINDER};
use cordis_spine::Llm;
use cordis_spine::{
    agent_loop, install_without_llm, BoxFuture, LlmOutput, LogEvent, LoopHandle, PromptRequest,
    Sampler, Sessions, StreamDelta, ToolCall, TurnControl, TurnOutcome, AGENT_LOOP, LLM, SESSIONS,
    TURN,
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

/// 第 0 次采样：发一个工具调用，同时用户插话（模拟一步进行中送达）。
/// 之后：把看到的历史记下来，回一句话收尾。
struct ToolThenSteer {
    ctx: Context,
    n: Arc<AtomicUsize>,
    /// 第 0 次采样就回文本（插话赶上模型要收尾的那一步）。
    text_first: bool,
    seen: Arc<Mutex<Vec<Vec<LogEvent>>>>,
}

impl Sampler for ToolThenSteer {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        Box::pin(async move {
            let i = self.n.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push(request.history.clone());
            if i == 0 {
                cordis_spine::steer(&self.ctx, "改用方案 B".into(), Vec::new()).unwrap();
                if self.text_first {
                    return LlmOutput {
                        text: "方案 A 做完了".into(),
                        ..LlmOutput::default()
                    };
                }
                return LlmOutput {
                    tool_calls: vec![ToolCall {
                        id: "c0".into(),
                        name: "echo".into(),
                        arguments: "x".into(),
                    }],
                    ..LlmOutput::default()
                };
            }
            LlmOutput {
                text: format!("reply-{i}"),
                ..LlmOutput::default()
            }
        })
    }
}

async fn boot(text_first: bool) -> (Context, Arc<AtomicUsize>, Arc<Mutex<Vec<Vec<LogEvent>>>>) {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    root.plugin(cordis_spine::turn(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let n = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(ToolThenSteer {
                ctx: root.clone(),
                n: n.clone(),
                text_first,
                seen: seen.clone(),
            }),
        ),
    )
    .unwrap();
    root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
    (root, n, seen)
}

/// 插话在工具跑完之后、下一次采样之前送达：工具没被中断，插话紧跟在它的结果
/// 后面，前面带着插话提示；这一轮接着跑，不是被停掉再开一轮。
#[tokio::test]
async fn a_steer_joins_the_running_turn_after_the_tool_round() {
    let (root, n, seen) = boot(false).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("用方案 A 做")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("reply-1".into()));
    assert_eq!(n.load(Ordering::SeqCst), 2);

    let second = seen.lock().unwrap()[1].clone();
    let tool = second
        .iter()
        .position(|e| matches!(e, LogEvent::ToolExecute { id, .. } if id == "c0"))
        .expect("工具结果要在");
    match &second[tool] {
        LogEvent::ToolExecute { content, .. } => {
            assert_ne!(content, INTERRUPTED_TOOL_RESULT, "插话不该中断工具")
        }
        _ => unreachable!(),
    }
    let tail: Vec<_> = second[tool + 1..].to_vec();
    assert!(
        matches!(
            tail.as_slice(),
            [LogEvent::SystemReminder(note), LogEvent::User(text), ..]
                if note == STEER_REMINDER && text == "改用方案 B"
        ),
        "插话要紧跟在工具结果后面：{tail:?}"
    );
    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    assert!(!sessions.has_steers(), "送达后收件箱要空");
    assert!(
        !root.require::<TurnControl>(TURN).unwrap().yield_requested(),
        "送达后让路信号要撤掉"
    );
}

/// 插话赶上模型要收尾的那一步：不收尾，送达后再采一步回应它。旧实现里这条
/// 插话要么把这一轮停掉，要么留到下一轮。
#[tokio::test]
async fn a_steer_arriving_with_the_final_answer_gets_one_more_step() {
    let (root, n, seen) = boot(true).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("用方案 A 做")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("reply-1".into()));
    assert_eq!(n.load(Ordering::SeqCst), 2);
    let second = seen.lock().unwrap()[1].clone();
    assert!(
        matches!(second.last(), Some(LogEvent::User(text)) if text == "改用方案 B"),
        "第二次采样要看到插话：{second:?}"
    );
}

/// 撤回插话（还没有输出时按 Esc 收回输入框）要连带它前面那条提示一起摘掉。
#[tokio::test]
async fn rewinding_a_steer_drops_its_note() {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    sessions.append(LogEvent::User("先做 A".into()));
    sessions.append(LogEvent::LlmStream(LlmOutput {
        text: "在做 A".into(),
        ..LlmOutput::default()
    }));
    let id = sessions.push_steer("改做 B".into(), Vec::new());
    let steer = sessions.take_steer(&id).unwrap();
    sessions.append_steer(steer);

    let (text, _) = sessions
        .rewind_inflight_user()
        .expect("插话还没有输出，能撤");
    assert_eq!(text, "改做 B");
    assert!(
        matches!(sessions.events().last(), Some(LogEvent::LlmStream(_))),
        "插话提示要一起摘掉：{:?}",
        sessions.events()
    );
}
