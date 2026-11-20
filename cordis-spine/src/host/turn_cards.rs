//! 插件挂在一轮回答下面的卡片（Rhai `host.card`）：一段标题 + 正文 + 可选视图树 +
//! 几个按钮。MCP 做不到这个——MCP App 只能画在它自己的工具结果里；插件能在**任何**
//! 一轮结束时（`agent/turn-end`）、甚至定时器里，往那一轮下面挂一张卡：「本轮改了
//! 3 个 Rust 文件 · [跑测试]」「缓存命中 12% · [看原因]」。
//!
//! - 轮次按这一页日志里第几条用户消息算（`ordinal`，同 [`crate::TurnChanges`]，网关
//!   用同一个序号换成投影的 `turnId`）。
//! - 只在内存里：页关了、Dock 重启就没了（终端那边发卡时另记一条 Notice，跟着会话落盘）。
//! - 插件停了 / 换版了，卡片留着，按钮失效（`live: false`）。
//! - 按钮点了在**卡片所在的那一页**上跑插件的回调：回调里 `host.followup` / `call_tool`
//!   都落在那一页。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};
use cordis_base::types::{LogEvent, NoticeKind};
use cordis_base::view::ViewNode;

use crate::names::{SESSIONS, TURN_CARDS, TURN_CARDS_CHANGED};
use crate::session::log::Sessions;

/// 一页最多留这么多张卡，多了丢最早的。
const MAX_PER_PAGE: usize = 50;

/// [`TURN_CARDS_CHANGED`] 的载荷：`page` 那一页第 `ordinal` 轮的卡片变了。
#[derive(Clone, Debug)]
pub struct TurnCardsChanged {
    pub page: Arc<str>,
    pub ordinal: usize,
}

/// 卡片的内容（插件给的那部分）。
#[derive(Clone, Debug, Default)]
pub struct CardSpec {
    pub title: String,
    pub text: String,
    /// `default` / `muted` / `accent` / `success` / `warning` / `danger`。
    pub tone: String,
    pub view: Option<ViewNode>,
    /// `(id, label)`。
    pub actions: Vec<(String, String)>,
}

/// 按钮回调：拿到按钮 id，回一句给用户看的提示（`None` = 不提示）。
pub type CardHandler = Arc<dyn Fn(&str) -> Result<Option<String>, String> + Send + Sync>;

/// 给界面的一张卡。
#[derive(Clone, Debug)]
pub struct CardView {
    /// 线上的 id（全局唯一）。
    pub key: String,
    pub plugin: String,
    /// 插件自己起的 id（`host.update_card` 认它）。
    pub id: String,
    pub ordinal: usize,
    pub spec: CardSpec,
    /// 插件还在、按钮还能点。
    pub live: bool,
}

struct Card {
    key: String,
    page: Arc<str>,
    ordinal: usize,
    plugin: String,
    id: String,
    spec: CardSpec,
    handler: Option<CardHandler>,
}

pub struct TurnCards {
    ctx: Context,
    cards: Mutex<Vec<Card>>,
    seq: AtomicU64,
}

impl TurnCards {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            cards: Mutex::new(Vec::new()),
            seq: AtomicU64::new(1),
        }
    }

    /// 在 `page` 当前这一轮下面挂一张卡（`page` 必须是主会话页）。同一页同一轮里
    /// 同一个插件的同一个 `id` 再发一次是**原地更新**；别的轮次就是新卡。返回 `id`。
    pub fn post(
        &self,
        page: &Context,
        plugin: &str,
        id: Option<&str>,
        spec: CardSpec,
        handler: Option<CardHandler>,
    ) -> Result<String, String> {
        let sessions = page
            .get::<Sessions>(SESSIONS)
            .ok_or("这一页没有会话，卡片没处挂")?;
        if !sessions.is_main() {
            return Err(
                "卡片只能挂在主会话页上（子代理的回合里请传 identity 或在主会话的钩子里发）".into(),
            );
        }
        let ordinal = user_count(&sessions)
            .checked_sub(1)
            .ok_or("这一页还没有用户消息，卡片没有可挂的轮次")?;
        let identity: Arc<str> = sessions.identity().into();
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let id = id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("card-{seq}"));
        let fresh = {
            let mut cards = self.cards.lock().unwrap();
            match cards.iter_mut().find(|c| {
                c.page == identity && c.ordinal == ordinal && c.plugin == plugin && c.id == id
            }) {
                Some(card) => {
                    card.spec = spec.clone();
                    card.handler = handler;
                    false
                }
                None => {
                    cards.push(Card {
                        key: format!("c{seq}"),
                        page: identity.clone(),
                        ordinal,
                        plugin: plugin.to_string(),
                        id: id.clone(),
                        spec: spec.clone(),
                        handler,
                    });
                    let mine: Vec<usize> = cards
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.page == identity)
                        .map(|(i, _)| i)
                        .collect();
                    if mine.len() > MAX_PER_PAGE {
                        cards.remove(mine[0]);
                    }
                    true
                }
            }
        };
        // 终端没有卡片位：第一次发的时候记一条 Notice（只给人看、跟着会话落盘）。
        if fresh {
            let mut body = spec.text.clone();
            if let Some(view) = &spec.view {
                let plain = view.to_plain();
                if !plain.trim().is_empty() {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(&plain);
                }
            }
            sessions.append(LogEvent::Notice {
                kind: NoticeKind::Other,
                title: format!("{plugin} · {}", spec.title),
                body,
            });
        }
        self.notify(&identity, ordinal);
        Ok(id)
    }

    /// 改插件 `plugin` 最近一张叫 `id` 的卡。
    pub fn update(
        &self,
        plugin: &str,
        id: &str,
        change: impl FnOnce(&mut CardSpec),
    ) -> Result<(), String> {
        let (page, ordinal) = {
            let mut cards = self.cards.lock().unwrap();
            let card = cards
                .iter_mut()
                .rev()
                .find(|c| c.plugin == plugin && c.id == id)
                .ok_or_else(|| format!("没有叫 {id} 的卡片"))?;
            change(&mut card.spec);
            (card.page.clone(), card.ordinal)
        };
        self.notify(&page, ordinal);
        Ok(())
    }

    /// 摘掉插件 `plugin` 最近一张叫 `id` 的卡。
    pub fn remove(&self, plugin: &str, id: &str) -> Result<(), String> {
        let (page, ordinal) = {
            let mut cards = self.cards.lock().unwrap();
            let at = cards
                .iter()
                .rposition(|c| c.plugin == plugin && c.id == id)
                .ok_or_else(|| format!("没有叫 {id} 的卡片"))?;
            let card = cards.remove(at);
            (card.page, card.ordinal)
        };
        self.notify(&page, ordinal);
        Ok(())
    }

    /// 插件停了：它的卡留着，按钮失效。
    pub fn detach_plugin(&self, plugin: &str) {
        let touched: Vec<(Arc<str>, usize)> = {
            let mut cards = self.cards.lock().unwrap();
            cards
                .iter_mut()
                .filter(|c| c.plugin == plugin && c.handler.is_some())
                .map(|c| {
                    c.handler = None;
                    (c.page.clone(), c.ordinal)
                })
                .collect()
        };
        for (page, ordinal) in touched {
            self.notify(&page, ordinal);
        }
    }

    /// 页关了：丢掉它的卡。
    pub fn forget_page(&self, page: &str) {
        self.cards.lock().unwrap().retain(|c| &*c.page != page);
    }

    /// 一页的卡，按轮次、再按发出的先后。
    pub fn list(&self, page: &str) -> Vec<CardView> {
        let cards = self.cards.lock().unwrap();
        let mut out: Vec<CardView> = cards
            .iter()
            .filter(|c| &*c.page == page)
            .map(view_of)
            .collect();
        out.sort_by_key(|c| c.ordinal);
        out
    }

    /// 一页第 `ordinal` 轮的卡。
    pub fn at(&self, page: &str, ordinal: usize) -> Vec<CardView> {
        self.list(page)
            .into_iter()
            .filter(|c| c.ordinal == ordinal)
            .collect()
    }

    /// 点了 `page` 上卡片 `key` 的按钮 `action`。回调在阻塞线程上跑（它是脚本）。
    pub async fn action(
        &self,
        page: &str,
        key: &str,
        action: &str,
    ) -> Result<Option<String>, String> {
        let handler = {
            let cards = self.cards.lock().unwrap();
            let card = cards
                .iter()
                .find(|c| &*c.page == page && c.key == key)
                .ok_or("卡片已经不在了")?;
            if !card.spec.actions.iter().any(|(id, _)| id == action) {
                return Err(format!("这张卡没有按钮 {action}"));
            }
            card.handler.clone().ok_or("插件已经停了，按钮不再可用")?
        };
        let action = action.to_string();
        tokio::task::spawn_blocking(move || handler(&action))
            .await
            .map_err(|e| e.to_string())?
    }

    fn notify(&self, page: &Arc<str>, ordinal: usize) {
        self.ctx.emit(
            TURN_CARDS_CHANGED,
            TurnCardsChanged {
                page: page.clone(),
                ordinal,
            },
        );
    }
}

fn view_of(c: &Card) -> CardView {
    CardView {
        key: c.key.clone(),
        plugin: c.plugin.clone(),
        id: c.id.clone(),
        ordinal: c.ordinal,
        spec: c.spec.clone(),
        live: c.handler.is_some(),
    }
}

fn user_count(sessions: &Sessions) -> usize {
    sessions.with_log(|events, _| {
        events
            .iter()
            .filter(|e| matches!(e, LogEvent::User(_)))
            .count()
    })
}

/// 挂 `"turnCards"`。
pub fn turn_cards() -> Plugin {
    plugin("turn-cards", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(TURN_CARDS, TurnCards::new(ctx.clone()))?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn page() -> Context {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx.clone());
        let _ = ctx.provide(SESSIONS, sessions);
        ctx
    }

    fn spec(title: &str) -> CardSpec {
        CardSpec {
            title: title.into(),
            text: "正文".into(),
            actions: vec![("go".into(), "跑".into())],
            ..CardSpec::default()
        }
    }

    #[tokio::test]
    async fn a_card_lands_on_the_current_turn_and_updates_in_place() {
        let ctx = page().await;
        let cards = TurnCards::new(ctx.clone());
        let sessions = ctx.require::<Sessions>(SESSIONS).unwrap();
        assert!(
            cards.post(&ctx, "p", None, spec("x"), None).is_err(),
            "还没有轮次"
        );
        sessions.append(LogEvent::User("一".into()));
        sessions.append(LogEvent::User("二".into()));
        let id = cards
            .post(&ctx, "p", Some("review"), spec("v1"), None)
            .unwrap();
        assert_eq!(id, "review");
        cards
            .post(&ctx, "p", Some("review"), spec("v2"), None)
            .unwrap();
        let list = cards.list(sessions.identity());
        assert_eq!(list.len(), 1, "同一轮同一个 id 是原地更新");
        assert_eq!(list[0].ordinal, 1);
        assert_eq!(list[0].spec.title, "v2");
        assert!(!list[0].live);
        // 终端那边只在第一次发的时候记一条 Notice。
        let notices = sessions
            .events()
            .iter()
            .filter(|e| matches!(e, LogEvent::Notice { .. }))
            .count();
        assert_eq!(notices, 1);
    }

    #[tokio::test]
    async fn actions_run_the_handler_until_the_plugin_detaches() {
        let ctx = page().await;
        let cards = TurnCards::new(ctx.clone());
        let sessions = ctx.require::<Sessions>(SESSIONS).unwrap();
        sessions.append(LogEvent::User("一".into()));
        let handler: CardHandler = Arc::new(|a| Ok(Some(format!("点了 {a}"))));
        cards
            .post(&ctx, "p", Some("c"), spec("x"), Some(handler))
            .unwrap();
        let key = cards.list(sessions.identity())[0].key.clone();
        let page = sessions.identity().to_string();
        assert_eq!(
            cards.action(&page, &key, "go").await.unwrap().as_deref(),
            Some("点了 go")
        );
        assert!(cards.action(&page, &key, "nope").await.is_err());
        cards.detach_plugin("p");
        assert!(!cards.list(&page)[0].live);
        let err = cards.action(&page, &key, "go").await.unwrap_err();
        assert!(err.contains("停了"), "{err}");
        cards.remove("p", "c").unwrap();
        assert!(cards.list(&page).is_empty());
    }
}
