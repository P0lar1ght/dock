//! AI 辅助写预设：经 `"llm"` 采样一次，结果校验后回草稿；不写盘、不开会话。

use std::sync::{Arc, Mutex};

use cordis::Context;
use cordis_spine::{
    draft_preset, rewrite_persona, suggest_tools, BoxFuture, Llm, LlmOutput, PromptRequest,
    Rewrite, Sampler, StreamDelta, ToolChoice, LLM,
};

/// 回固定文本，记下收到的请求。
struct Canned {
    reply: LlmOutput,
    seen: Arc<Mutex<Vec<PromptRequest>>>,
}

impl Sampler for Canned {
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

fn root(reply: LlmOutput) -> (Context, Arc<Mutex<Vec<PromptRequest>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let root = Context::new();
    let sampler = Arc::new(Canned {
        reply,
        seen: seen.clone(),
    });
    root.provide(LLM, Llm::from_sampler(root.clone(), sampler))
        .unwrap();
    (root, seen)
}

fn text(t: &str) -> LlmOutput {
    LlmOutput {
        text: t.into(),
        ..LlmOutput::default()
    }
}

fn catalog() -> Vec<ToolChoice> {
    ["read_file", "grep", "write_file"]
        .iter()
        .map(|n| ToolChoice {
            name: (*n).into(),
            summary: format!("{n} 简介"),
        })
        .collect()
}

#[tokio::test]
async fn draft_sends_the_description_and_catalog_and_validates_the_reply() {
    let (ctx, seen) = root(text(
        r#"{"name": "审查员", "description": "只读审代码", "icon": "shield",
            "persona": "你只读代码。", "tools": [{"name": "read_file", "reason": "读"}, {"name": "rm"}]}"#,
    ));
    let d = draft_preset(&ctx, "帮我审代码，只读", &catalog(), &["shield".into()])
        .await
        .unwrap();
    assert_eq!(d.name, "审查员");
    assert_eq!(d.icon.as_deref(), Some("shield"));
    assert_eq!(d.tools.as_deref(), Some(&["read_file".to_string()][..]));

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "只采样一次");
    assert!(seen[0].tools.is_empty(), "不给模型工具");
    let prompt = format!("{:?}", seen[0].history);
    assert!(
        prompt.contains("帮我审代码") && prompt.contains("- grep: grep 简介"),
        "{prompt}"
    );
}

#[tokio::test]
async fn failures_come_back_as_chinese_errors() {
    let (ctx, _) = root(LlmOutput {
        error: Some("HTTP 401".into()),
        ..LlmOutput::default()
    });
    let err = draft_preset(&ctx, "随便", &catalog(), &[])
        .await
        .unwrap_err();
    assert!(err.contains("模型调用失败") && err.contains("401"), "{err}");

    let (ctx, _) = root(text("我不会写 JSON"));
    let err = draft_preset(&ctx, "随便", &catalog(), &[])
        .await
        .unwrap_err();
    assert!(err.contains("JSON"), "{err}");

    let err = draft_preset(&ctx, "  ", &catalog(), &[]).await.unwrap_err();
    assert!(err.contains("描述"), "{err}");

    let bare = Context::new();
    let err = draft_preset(&bare, "随便", &catalog(), &[])
        .await
        .unwrap_err();
    assert!(err.contains("模型服务没有挂载"), "{err}");
}

#[tokio::test]
async fn rewrite_and_suggest_tools() {
    let (ctx, seen) = root(text("```markdown\n# 审查员\n只读。\n```"));
    let out = rewrite_persona(&ctx, "你审代码", Rewrite::Expand, "只读审查")
        .await
        .unwrap();
    assert_eq!(out, "# 审查员\n只读。");
    assert!(seen.lock().unwrap()[0].system.contains("扩写"));
    assert!(rewrite_persona(&ctx, " ", Rewrite::Polish, "")
        .await
        .is_err());

    let (ctx, _) = root(text(
        r#"{"tools": [{"name": "grep", "reason": "搜"}, {"name": "nope"}]}"#,
    ));
    let picks = suggest_tools(&ctx, "搜代码", "", &catalog()).await.unwrap();
    assert_eq!(picks.len(), 1);
    assert_eq!(picks[0].name, "grep");

    let (ctx, _) = root(text(r#"{"tools": [{"name": "nope"}]}"#));
    assert!(suggest_tools(&ctx, "搜代码", "", &catalog()).await.is_err());
}
