//! MCP Apps 的 `ui/update-model-context`：界面把「用户在界面里看到 / 改成的样子」交给
//! agent。按页、按界面只留最新一份（规范：每次更新覆盖上一次），下一步采样前用一条
//! system-reminder 交给模型，交过就清；不单独触发一轮。
//!
//! 在跑的一轮在下一个步骤边界就能看到；空闲的页等用户发下一条消息。只告诉这一页
//! 自己（子代理、别的页身份不同，取不到）。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};
use cordis_base::types::{StepStart, ORDER_STEP_START_MCP_APPS};
use serde_json::Value;

use crate::names::{MCP_APP_CONTEXT, SESSIONS, STEP_START};
use crate::session::log::Sessions;

/// 一个界面的文字部分上限（字节）。
const MAX_TEXT_BYTES: usize = 8 * 1024;
/// `structuredContent` 序列化后的上限（字节）。
const MAX_STRUCTURED_BYTES: usize = 8 * 1024;

/// 一个界面交来的上下文。`label` 给模型认是哪个界面（服务器名）。
#[derive(Clone, Debug, PartialEq)]
pub struct AppContextUpdate {
    pub label: String,
    /// 规范的 `ContentBlock[]`；只取文字，别的类型记一句占位。
    pub content: Vec<Value>,
    pub structured: Option<Value>,
}

/// 各页还没交给 agent 的界面上下文（页身份 → 界面键 → 最新一份）。
#[derive(Clone, Default)]
pub struct McpAppContext {
    pending: Arc<Mutex<HashMap<String, BTreeMap<String, AppContextUpdate>>>>,
}

impl McpAppContext {
    /// 记下（覆盖）`identity` 这一页上 `key` 这个界面的最新上下文。两样都空 = 撤回。
    pub fn set(&self, identity: &str, key: &str, update: AppContextUpdate) {
        let mut map = self.pending.lock().unwrap();
        let page = map.entry(identity.to_string()).or_default();
        if update.content.is_empty() && update.structured.is_none() {
            page.remove(key);
        } else {
            page.insert(key.to_string(), update);
        }
        if page.is_empty() {
            map.remove(identity);
        }
    }

    /// 取走这一页攒的，排成一条提醒；没有就是 `None`。
    pub fn take_reminder(&self, identity: &str) -> Option<String> {
        let page = self.pending.lock().unwrap().remove(identity)?;
        let parts: Vec<String> = page.values().map(render).collect();
        Some(format!(
            "<system-reminder>\n用户打开的 MCP App 界面更新了它的状态（这是界面交给你的上下文，\
             不是用户的新消息，不用为此单独回复）：\n\n{}\n</system-reminder>",
            parts.join("\n\n")
        ))
    }
}

fn render(update: &AppContextUpdate) -> String {
    let mut text = String::new();
    for block in &update.content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
            Some(other) => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&format!("（界面附了一段 {other} 内容，这里不转交）"));
            }
            None => {}
        }
    }
    let mut out = format!("【{}】", update.label);
    if !text.is_empty() {
        out.push('\n');
        out.push_str(&clip(&text, MAX_TEXT_BYTES));
    }
    if let Some(structured) = &update.structured {
        out.push_str("\n结构化数据：");
        out.push_str(&clip(&structured.to_string(), MAX_STRUCTURED_BYTES));
    }
    out
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…（已截断，原文 {} 字节）", &text[..end], text.len())
}

/// 记一份界面上下文到 `page` 这一页（网关收到 `mcp/app/context` 时调）。没挂插件就什么也不做。
pub fn record_app_context(page: &Context, key: &str, update: AppContextUpdate) {
    let (Some(store), Some(sessions)) = (
        page.get::<McpAppContext>(MCP_APP_CONTEXT),
        page.get::<Sessions>(SESSIONS),
    ) else {
        return;
    };
    store.set(sessions.identity(), key, update);
}

pub fn mcp_app_context() -> Plugin {
    plugin("mcp-app-context", Inject::new(), |ctx, _: &()| {
        let store = McpAppContext::default();
        ctx.provide(MCP_APP_CONTEXT, store.clone())?;
        let handle = ctx.on_waterfall(STEP_START, move |start: StepStart, args| {
            let mut next = args.next::<StepStart>().unwrap_or(start);
            if let Some(body) = store.take_reminder(&next.identity) {
                next.remind(ORDER_STEP_START_MCP_APPS, body);
            }
            next
        })?;
        Ok(Some(handle))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn update(label: &str, text: &str) -> AppContextUpdate {
        AppContextUpdate {
            label: label.into(),
            content: vec![json!({ "type": "text", "text": text })],
            structured: None,
        }
    }

    #[test]
    fn latest_update_per_app_wins_and_is_taken_once() {
        let store = McpAppContext::default();
        store.set("main", "board ui://a", update("board", "3 个方框"));
        store.set("main", "board ui://a", update("board", "5 个方框"));
        store.set("main#2", "board ui://a", update("board", "别的页"));
        let body = store.take_reminder("main").unwrap();
        assert!(body.starts_with("<system-reminder>"), "{body}");
        assert!(
            body.contains("5 个方框") && !body.contains("3 个方框"),
            "{body}"
        );
        assert!(!body.contains("别的页"));
        assert!(store.take_reminder("main").is_none(), "取过就清");
        assert!(store.take_reminder("main#2").is_some());
    }

    #[test]
    fn empty_update_withdraws_and_structured_is_capped() {
        let store = McpAppContext::default();
        store.set("main", "k", update("board", "x"));
        store.set(
            "main",
            "k",
            AppContextUpdate {
                label: "board".into(),
                content: vec![],
                structured: None,
            },
        );
        assert!(store.take_reminder("main").is_none(), "两样都空 = 撤回");

        store.set(
            "main",
            "k",
            AppContextUpdate {
                label: "board".into(),
                content: vec![json!({ "type": "image", "data": "AAAA", "mimeType": "image/png" })],
                structured: Some(json!({ "big": "y".repeat(MAX_STRUCTURED_BYTES * 2) })),
            },
        );
        let body = store.take_reminder("main").unwrap();
        assert!(body.contains("image"), "{body}");
        assert!(body.contains("已截断"), "超长的结构化数据要截断");
        assert!(body.len() < MAX_STRUCTURED_BYTES + 1024);
    }

    #[tokio::test]
    async fn step_start_injects_for_the_recording_page_only() {
        let ctx = Context::new();
        ctx.plugin(mcp_app_context(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let store = ctx.get::<McpAppContext>(MCP_APP_CONTEXT).unwrap();
        store.set(
            "main",
            "board ui://a",
            update("board", "用户选中了「登录」"),
        );
        let run = |id: &str| {
            let start = StepStart::new(0, id);
            let seed = start.clone();
            ctx.waterfall(STEP_START, start, move || seed)
                .into_reminders()
        };
        assert!(run("child-1").is_empty(), "子代理身份不同，取不到");
        let got = run("main");
        assert_eq!(got.len(), 1);
        assert!(got[0].contains("用户选中了「登录」"));
        assert!(run("main").is_empty());
    }
}
