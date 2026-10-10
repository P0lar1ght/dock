//! 插话（steer）：一轮进行中送来的用户消息在下一个步骤边界并进这一轮，
//! 不打断正在进行的采样和工具。侧边聊天写进主线的笔记走同一个步骤边界。

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
    /// 第 0 次采样时送来的不是插话，是侧边聊天写进来的笔记。
    side_note: bool,
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
                if self.side_note {
                    self.ctx
                        .require::<Sessions>(SESSIONS)
                        .unwrap()
                        .push_side_note("旁问结论：retry 要退避".into());
                } else {
                    cordis_spine::steer(&self.ctx, "改用方案 B".into(), Vec::new()).unwrap();
                }
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
    boot_with(text_first, false).await
}

async fn boot_with(
    text_first: bool,
    side_note: bool,
) -> (Context, Arc<AtomicUsize>, Arc<Mutex<Vec<Vec<LogEvent>>>>) {
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
                side_note,
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

/// 侧边聊天的笔记在一轮进行中写进来：和插话一样在下一个步骤边界落（紧跟在工具
/// 结果后面），模型下一次采样看得到；它不是插话，没有插话提示、也不是用户消息。
#[tokio::test]
async fn a_side_note_joins_at_the_next_step_boundary() {
    let (root, n, seen) = boot_with(false, true).await;
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
    let note = second
        .iter()
        .position(|e| {
            matches!(e, LogEvent::SystemReminder(t)
                if cordis_spine::side_note_text(t).as_deref() == Some("旁问结论：retry 要退避"))
        })
        .expect("第二次采样要看到笔记");
    assert!(
        note > tool,
        "笔记不能夹在 tool_calls 和结果之间：{second:?}"
    );
    assert!(
        !second
            .iter()
            .any(|e| matches!(e, LogEvent::SystemReminder(t) if t == STEER_REMINDER)),
        "笔记不是插话：{second:?}"
    );
}

/// 笔记赶上模型要收尾的那一步：不为它多采一步（插话会）。这一轮照常收尾，
/// 笔记留在收件箱里，由会话 actor 收尾后落进历史。
#[tokio::test]
async fn a_side_note_does_not_keep_the_turn_going() {
    let (root, n, _seen) = boot_with(true, true).await;
    let out = root
        .require::<LoopHandle>(AGENT_LOOP)
        .unwrap()
        .run("用方案 A 做")
        .await
        .unwrap();
    assert_eq!(out, TurnOutcome::Text("方案 A 做完了".into()));
    assert_eq!(n.load(Ordering::SeqCst), 1, "笔记不该让模型多采一步");
    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    assert_eq!(sessions.take_side_notes(), ["旁问结论：retry 要退避"]);
}

/// 只回一句固定文本、记下请求的模型。
struct Recording {
    reply: LlmOutput,
    seen: Arc<Mutex<Vec<PromptRequest>>>,
}

impl Sampler for Recording {
    fn sample<'a>(
        &'a self,
        request: PromptRequest,
        _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> BoxFuture<'a, LlmOutput> {
        self.seen.lock().unwrap().push(request);
        let reply = self.reply.clone();
        Box::pin(async move { reply })
    }
}

async fn draft_with(
    reply: LlmOutput,
) -> (Result<String, String>, Vec<PromptRequest>, usize, usize) {
    isolated_home();
    let root = Context::new();
    install_without_llm(&root).await.unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    root.provide(
        LLM,
        Llm::from_sampler(
            root.clone(),
            Arc::new(Recording {
                reply,
                seen: seen.clone(),
            }),
        ),
    )
    .unwrap();
    let sessions = root.require::<Sessions>(SESSIONS).unwrap();
    sessions.append(LogEvent::User("retry 为什么 sleep？".into()));
    sessions.append(LogEvent::LlmStream(LlmOutput {
        text: "为了等锁释放".into(),
        ..LlmOutput::default()
    }));
    let before = sessions.events().len();
    let out = cordis_spine::draft_side_note(&root).await;
    let after = sessions.events().len();
    let requests = seen.lock().unwrap().clone();
    (out, requests, before, after)
}

/// 起草写进主线的笔记：模型看到的是旁问的历史 + 一条起草要求；这次采样不进
/// 旁问页的日志。
#[tokio::test]
async fn drafting_a_side_note_reads_the_aside_and_leaves_its_log_alone() {
    let (out, requests, before, after) = draft_with(LlmOutput {
        text: "  - sleep 300ms 是在等锁释放  ".into(),
        ..LlmOutput::default()
    })
    .await;
    assert_eq!(out.unwrap(), "- sleep 300ms 是在等锁释放");
    assert_eq!(before, after, "起草不该往旁问页写东西");
    let history = &requests[0].history;
    assert!(
        history
            .iter()
            .any(|e| matches!(e, LogEvent::User(t) if t == "retry 为什么 sleep？")),
        "{history:?}"
    );
    assert!(
        matches!(history.last(), Some(LogEvent::User(t)) if t.contains("写进主线")),
        "{history:?}"
    );
}

/// 模型出错 / 没写出东西：照实报错，不交一段空笔记。
#[tokio::test]
async fn a_failed_draft_says_why() {
    let (out, ..) = draft_with(LlmOutput {
        error: Some("429".into()),
        ..LlmOutput::default()
    })
    .await;
    assert!(out.unwrap_err().contains("429"));
    let (out, ..) = draft_with(LlmOutput::default()).await;
    assert!(out.is_err());
}
