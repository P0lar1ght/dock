//! 分页：一个终端里并排跑多个会话。
//!
//! **一页 = 一棵 isolate 子树**。第 1 页就是根上下文（现有那一套原样不动），
//! 第 2 页起用 `root.isolate("sessions").isolate("turn")…` 派生出来：只有
//! [`PER_TAB_SERVICES`] 里的名字各自一份，`tools` / `llm` / `permissions` /
//! `mcp` / `theme` 照旧落回根 —— 一张 `"tools"` 表的不变式没被动过。
//!
//! 事件不分 realm（`ctx.emit` 不看 isolate），所以后台页的 `session/event`
//! 照样能把前台叫醒重绘，标签栏上的 `●` 就是靠这个活的。
//!
//! 装一页要挂哪些插件由**组合根**（`cordis-app`）给：TUI 不认识 spine / app
//! 的插件树，只负责开、关、切。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Fiber, Inject, Plugin};
use cordis_spine::{LogEvent, Sessions, AGENT_LOOP, AGENT_PRESETS, SESSIONS, TURN};

use crate::names::{
    SESSION, SESSION_PORT, TUI_PROMPT, TUI_SCROLLBACK, TUI_STATUS, TUI_TABS, TUI_WELCOME,
};
use crate::prompt::PromptWidget;
use crate::session::SessionRef;

/// 每页各有一份的服务。没列进来的一律落回根（全局单例：工具表、LLM、权限、
/// MCP、浏览器、cua、后台任务……），两页会真的抢同一个。
pub const PER_TAB_SERVICES: &[&str] = &[
    SESSIONS,
    TURN,
    AGENT_LOOP,
    SESSION,
    SESSION_PORT,
    TUI_SCROLLBACK,
    TUI_PROMPT,
    TUI_STATUS,
    TUI_WELCOME,
];

/// 旁问页额外要 isolate 的名字：它得有一份**自己的**只读预设，不能用全局那份。
const ASIDE_ISOLATES: &[&str] = &[AGENT_PRESETS];

/// `Alt+1..9` 能直达的上限，也是总页数上限（旁问页不占名额）。
pub const MAX_TABS: usize = 9;

/// 一页是常驻分页，还是 `/btw` 的旁问页。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    /// `Ctrl+N` / `Ctrl+F` 开出来的常驻页：全权工具，进标签栏。
    Normal,
    /// `/btw` 的旁问页：只读工具，不进标签栏，答完 Esc 就销毁。
    Aside,
}

/// 造一页：组合根给的插件工厂。参数是这一页的稳定编号（会话身份 `main#N`）
/// 与种类（旁问页要挂只读预设）。
pub type TabMount = Arc<dyn Fn(usize, TabKind) -> Plugin + Send + Sync>;

/// 标签栏要画的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabInfo {
    /// 稳定编号（第一页恒为 1）。关掉别的页也不会变。
    pub id: usize,
    pub title: String,
    /// 这一页正在生成。
    pub working: bool,
    pub active: bool,
    /// 从哪一页分叉出来的（`/tab fork` / `/btw`）。空白新页是 `None`。
    pub origin: Option<usize>,
    /// 常驻页还是只读旁问页。
    pub kind: TabKind,
}

struct Tab {
    id: usize,
    ctx: Context,
    /// 第 1 页就是根，没有可以 dispose 的 fiber。
    fiber: Option<Fiber>,
    origin: Option<usize>,
    kind: TabKind,
}

/// Named `"tui.tabs"`。调用点 live-lookup，别把 `Arc` 关进长生命周期闭包。
#[derive(Clone)]
pub struct Tabs {
    inner: Arc<Inner>,
}

struct Inner {
    root: Context,
    mount: TabMount,
    tabs: Mutex<Vec<Tab>>,
    active: AtomicUsize,
    next_id: AtomicUsize,
}

impl Tabs {
    fn new(root: Context, mount: TabMount) -> Self {
        let first = Tab {
            id: 1,
            ctx: root.clone(),
            fiber: None,
            origin: None,
            kind: TabKind::Normal,
        };
        Self {
            inner: Arc::new(Inner {
                root,
                mount,
                tabs: Mutex::new(vec![first]),
                active: AtomicUsize::new(0),
                next_id: AtomicUsize::new(2),
            }),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.tabs.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 只有一页时标签栏不画 —— 没开分页的人不该看见这条。
    pub fn bar_visible(&self) -> bool {
        self.len() > 1
    }

    pub fn active_index(&self) -> usize {
        self.inner.active.load(Ordering::Relaxed)
    }

    /// 当前页的上下文。事件循环每一帧都用它取 scrollback / prompt / 会话。
    pub fn active_ctx(&self) -> Context {
        let tabs = self.inner.tabs.lock().unwrap();
        let index = self.active_index().min(tabs.len().saturating_sub(1));
        tabs.get(index)
            .map(|t| t.ctx.clone())
            .unwrap_or_else(|| self.inner.root.clone())
    }

    pub fn list(&self) -> Vec<TabInfo> {
        let tabs = self.inner.tabs.lock().unwrap();
        let active = self.active_index();
        tabs.iter()
            .enumerate()
            .map(|(i, tab)| TabInfo {
                id: tab.id,
                title: tab_title(&tab.ctx),
                working: tab_working(&tab.ctx),
                active: i == active,
                origin: tab.origin,
                kind: tab.kind,
            })
            .collect()
    }

    /// 当前页的稳定编号（标签上显示的那个号）。
    pub fn active_id(&self) -> usize {
        let tabs = self.inner.tabs.lock().unwrap();
        tabs.get(self.active_index()).map(|t| t.id).unwrap_or(1)
    }

    fn index_of_id(&self, id: usize) -> Option<usize> {
        self.inner
            .tabs
            .lock()
            .unwrap()
            .iter()
            .position(|t| t.id == id)
    }

    /// 按标签上的号切页。号是稳定的，所以 `Alt+3` 永远是那一页。
    pub fn activate_id(&self, id: usize) -> bool {
        self.index_of_id(id).is_some_and(|i| self.activate(i))
    }

    /// 按标签上的号关页；`None` 关当前页。
    pub async fn close_id(&self, id: Option<usize>) -> Result<usize, String> {
        let index = match id {
            Some(id) => self
                .index_of_id(id)
                .ok_or_else(|| format!("没有第 {id} 页"))?,
            None => self.active_index(),
        };
        self.close(index).await
    }

    /// 切到第 `index` 页（0 基）。越界返回 false，按键不当回事。
    pub fn activate(&self, index: usize) -> bool {
        if index >= self.len() {
            return false;
        }
        self.inner.active.store(index, Ordering::Relaxed);
        true
    }

    /// 开一张空白新页并切过去。返回新页的 0 基位置。
    pub async fn open(&self) -> Result<usize, String> {
        self.open_with(None).await
    }

    /// 从当前页分叉：新页带着当前页的上下文快照，并记住来源。
    ///
    /// 快照是**值传递**（`model_history`，压缩后的那份）：分叉之后两页各写各的，
    /// 谁都污染不了谁。工具是全权的 —— 分页解决的是「一个终端里多个会话」，
    /// 只读那档留给后面的 `/btw`。
    pub async fn fork(&self) -> Result<usize, String> {
        let origin = self.active_id();
        let snapshot = {
            let ctx = self.active_ctx();
            let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
                return Err("当前页没有会话，分叉不了".into());
            };
            sessions.model_history()
        };
        if snapshot.is_empty() {
            return Err("当前页还没有对话，先说点什么再分叉".into());
        }
        self.open_with(Some((origin, snapshot))).await
    }

    /// 起一棵页子树并把快照种进去。旁问页与常驻页共用这条路。
    async fn mount_page(
        &self,
        kind: TabKind,
        seed: Option<Vec<LogEvent>>,
    ) -> Result<(usize, Context, Fiber), String> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        // 只把该隔离的名字拉进新 realm，其余照旧解析到根。
        let mut child = self.inner.root.clone();
        let extra = if kind == TabKind::Aside {
            ASIDE_ISOLATES
        } else {
            &[][..]
        };
        for name in PER_TAB_SERVICES.iter().chain(extra) {
            child = child.isolate(name);
        }
        let fiber = child
            .plugin((self.inner.mount)(id, kind), ())
            .map_err(|e| format!("开新分页失败：{e}"))?;
        fiber
            .wait()
            .await
            .map_err(|e| format!("新分页没起来：{e}"))?;
        if let Some(events) = seed {
            // 会话挂好之后再种：种进去的是快照，跟来源页从此各走各的。
            let Some(sessions) = child.get::<Sessions>(SESSIONS) else {
                let _ = fiber.dispose().await;
                return Err("新分页没有会话，中止".into());
            };
            sessions.seed(events);
        }
        Ok((id, child, fiber))
    }

    async fn open_with(&self, seed: Option<(usize, Vec<LogEvent>)>) -> Result<usize, String> {
        self.open_page(TabKind::Normal, seed).await
    }

    /// 建页 + 入册 + 切过去。返回新页的 0 基位置。
    async fn open_page(
        &self,
        kind: TabKind,
        seed: Option<(usize, Vec<LogEvent>)>,
    ) -> Result<usize, String> {
        if self.len() >= MAX_TABS {
            return Err(format!("最多 {MAX_TABS} 页"));
        }
        let origin = seed.as_ref().map(|(origin, _)| *origin);
        let (id, child, fiber) = self
            .mount_page(kind, seed.map(|(_, events)| events))
            .await?;
        let index = {
            let mut tabs = self.inner.tabs.lock().unwrap();
            tabs.push(Tab {
                id,
                ctx: child,
                fiber: Some(fiber),
                origin,
                kind,
            });
            tabs.len() - 1
        };
        self.activate(index);
        Ok(index)
    }

    /// `/btw`：从当前页分叉一个**只读**分页，把问题发进去并切过去。
    ///
    /// 「不打断」指的是**主线那一轮照跑**（各页各自的会话与循环），不是不换视线：
    /// 答案要走这一页自己的滚动区，markdown、工具卡、流式才都在。看完 `Alt+1`
    /// 回主线，或者 `/tab close` 关掉。
    pub async fn ask_aside(&self, question: String) -> Result<usize, String> {
        let question = question.trim().to_string();
        if question.is_empty() {
            return Err("要问点什么：/btw <问题>".into());
        }
        let origin = self.active_id();
        let snapshot = self
            .active_ctx()
            .get::<Sessions>(SESSIONS)
            .map(|s| s.model_history())
            .unwrap_or_default();
        let index = self
            .open_page(TabKind::Aside, Some((origin, snapshot)))
            .await?;
        let ctx = self.active_ctx();
        let Some(port) = ctx.get::<SessionRef>(SESSION_PORT) else {
            let _ = self.close(index).await;
            return Err("旁问页没有会话入口".into());
        };
        // 旁问不排队：它就是为了「别打断」而存在的。
        port.submit(question, true);
        let tabs = self.inner.tabs.lock().unwrap();
        Ok(tabs[index].id)
    }

    /// 把当前的只读旁问页转正：用它已经聊出来的历史开一张**全权**常驻页，
    /// 再把旁问那一页关掉。只读是「插一嘴」的约束，认真聊下去不该再受它管。
    pub async fn promote_active(&self) -> Result<usize, String> {
        let index = self.active_index();
        let (kind, origin, history) = {
            let tabs = self.inner.tabs.lock().unwrap();
            let tab = tabs.get(index).ok_or_else(|| "没有当前页".to_string())?;
            let history = tab
                .ctx
                .get::<Sessions>(SESSIONS)
                .map(|s| s.model_history())
                .unwrap_or_default();
            (tab.kind, tab.origin, history)
        };
        if kind != TabKind::Aside {
            return Err("这一页本来就是全权的，不用转正".into());
        }
        let new_index = self
            .open_page(TabKind::Normal, Some((origin.unwrap_or(1), history)))
            .await?;
        let new_id = {
            let tabs = self.inner.tabs.lock().unwrap();
            tabs[new_index].id
        };
        // 新页在旁问页后面，关掉旁问会把它左移；`close` 自己会修焦点。
        self.close(index).await?;
        Ok(new_id)
    }

    /// 把当前页最近一条模型回复带回它的来源页：填进那一页的输入框并切过去。
    ///
    /// **不**直接往来源页的历史里写 —— 那等于替用户说话。填进输入框，发不发、
    /// 怎么改都还是用户说了算。
    pub fn carry_back(&self) -> Result<usize, String> {
        let (origin_id, source_id) = {
            let tabs = self.inner.tabs.lock().unwrap();
            let tab = tabs
                .get(self.active_index())
                .ok_or_else(|| "没有当前页".to_string())?;
            let origin = tab
                .origin
                .ok_or_else(|| "这一页不是分叉出来的，没有来源页".to_string())?;
            (origin, tab.id)
        };
        let Some(origin_index) = self.index_of_id(origin_id) else {
            return Err(format!("来源页（第 {origin_id} 页）已经关掉了"));
        };
        let text = {
            let ctx = self.active_ctx();
            let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
                return Err("当前页没有会话".into());
            };
            last_reply(&sessions).ok_or_else(|| "这一页还没有模型回复可带".to_string())?
        };
        let origin_ctx = {
            let tabs = self.inner.tabs.lock().unwrap();
            tabs[origin_index].ctx.clone()
        };
        let Some(prompt) = origin_ctx.get::<PromptWidget>(TUI_PROMPT) else {
            return Err("来源页没有输入框".into());
        };
        prompt.insert_str(&carried_block(source_id, &text));
        self.activate(origin_index);
        Ok(origin_id)
    }

    /// 关掉第 `index` 页。第一页关不掉（它是根，关了就没有 dock 了）。
    pub async fn close(&self, index: usize) -> Result<usize, String> {
        if index == 0 {
            return Err("第一页关不掉（它是主会话）".into());
        }
        let tab = {
            let mut tabs = self.inner.tabs.lock().unwrap();
            if index >= tabs.len() {
                return Err("没有这一页".into());
            }
            tabs.remove(index)
        };
        let id = tab.id;
        if let Some(fiber) = tab.fiber {
            // 整页的插件都挂在这颗 fiber 下：dispose 一次，会话 / 循环 / 视图一起走。
            fiber
                .dispose()
                .await
                .map_err(|e| format!("关闭第 {id} 页失败：{e}"))?;
        }
        let len = self.len();
        let active = self.active_index();
        if active >= len {
            self.inner
                .active
                .store(len.saturating_sub(1), Ordering::Relaxed);
        } else if active > index {
            self.inner.active.store(active - 1, Ordering::Relaxed);
        }
        Ok(id)
    }
}

/// 这一页最近一条**有正文**的模型回复。只有工具调用的那几步跳过：带回去的
/// 该是结论，不是「我调用了 read_file」。
///
/// 走 `with_log` 借用而不是 `events()`：这条在每帧的 `aside()` 里被调用，
/// `events()` 会把整份日志（含工具输出的大字符串）深拷贝一遍。
fn last_reply(sessions: &Sessions) -> Option<String> {
    sessions.with_log(|events, _| {
        events.iter().rev().find_map(|event| match event {
            LogEvent::LlmStream(out) if !out.text.trim().is_empty() => Some(out.text.clone()),
            _ => None,
        })
    })
}

/// 带回来源页的块：注明出处并整段引用，用户接着往下写自己的问题。
fn carried_block(from: usize, text: &str) -> String {
    let quoted: String = text.lines().map(|line| format!("> {line}\n")).collect();
    format!("（来自第 {from} 页）\n{quoted}\n")
}

fn tab_working(ctx: &Context) -> bool {
    ctx.get::<SessionRef>(SESSION_PORT)
        .is_some_and(|s| s.working())
}

/// 标签标题：会话自己的标题 > 第一条用户消息 > 「新会话」。
///
/// 同 [`last_reply`]，走 `with_log` 借用：`list()` 每帧、每个分页各调一次。
fn tab_title(ctx: &Context) -> String {
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return "新会话".into();
    };
    if let Some(title) = sessions.live_title() {
        return clip(&title);
    }
    sessions
        .with_log(|events, _| {
            events.iter().find_map(|event| match event {
                LogEvent::User(text) => Some(clip(text)),
                _ => None,
            })
        })
        .unwrap_or_else(|| "新会话".into())
}

/// 标签栏一行放不下几个字，够认出是哪一页就行。
fn clip(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.trim();
    if line.is_empty() {
        return "新会话".into();
    }
    let mut out: String = line.chars().take(12).collect();
    if line.chars().count() > 12 {
        out.push('…');
    }
    out
}

/// Mount named `"tui.tabs"`。config 是组合根给的建页工厂。
pub fn tabs() -> Plugin {
    plugin("tui.tabs", Inject::new(), |ctx, mount: &TabMount| {
        Ok(Some(ctx.provide(
            TUI_TABS,
            Tabs::new(ctx.clone(), mount.clone()),
        )?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不挂任何插件的建页工厂：只验分页本身的开 / 关 / 切。
    fn noop_mount() -> TabMount {
        Arc::new(|_id, _kind| plugin("tab-noop", Inject::new(), |_ctx, _: &()| Ok(None)))
    }

    async fn boot() -> (Context, Tabs) {
        let root = Context::new();
        root.plugin(tabs(), noop_mount())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
        (root, (*tabs).clone())
    }

    #[tokio::test]
    async fn starts_with_one_tab_and_no_bar() {
        let (_root, tabs) = boot().await;
        assert_eq!(tabs.len(), 1);
        assert!(!tabs.bar_visible(), "只有一页时不该画标签栏");
        assert_eq!(tabs.active_index(), 0);
    }

    #[tokio::test]
    async fn open_switches_to_the_new_tab() {
        let (_root, tabs) = boot().await;
        let index = tabs.open().await.unwrap();
        assert_eq!(index, 1);
        assert_eq!(tabs.active_index(), 1, "开完就该切过去");
        assert!(tabs.bar_visible());
        let list = tabs.list();
        assert_eq!(list.len(), 2);
        assert!(list[1].active);
        assert!(!list[0].active);
    }

    /// 页号要稳定：关掉中间一页，后面的页不能改名换姓。
    #[tokio::test]
    async fn ids_are_stable_across_close() {
        let (_root, tabs) = boot().await;
        tabs.open().await.unwrap();
        tabs.open().await.unwrap();
        let ids: Vec<usize> = tabs.list().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);

        tabs.close(1).await.unwrap();
        let ids: Vec<usize> = tabs.list().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![1, 3], "第 3 页不该被改号");
    }

    #[tokio::test]
    async fn closing_before_the_active_tab_keeps_focus() {
        let (_root, tabs) = boot().await;
        tabs.open().await.unwrap(); // index 1
        tabs.open().await.unwrap(); // index 2，当前在这
        assert_eq!(tabs.active_index(), 2);
        tabs.close(1).await.unwrap();
        assert_eq!(tabs.active_index(), 1, "焦点该跟着那一页左移，而不是跳页");
        assert_eq!(tabs.list()[tabs.active_index()].id, 3);
    }

    #[tokio::test]
    async fn closing_the_active_last_tab_falls_back() {
        let (_root, tabs) = boot().await;
        tabs.open().await.unwrap();
        tabs.close(1).await.unwrap();
        assert_eq!(tabs.active_index(), 0);
        assert!(!tabs.bar_visible());
    }

    #[tokio::test]
    async fn first_tab_cannot_be_closed() {
        let (_root, tabs) = boot().await;
        assert!(tabs.close(0).await.is_err());
        assert_eq!(tabs.len(), 1);
    }

    #[tokio::test]
    async fn activate_stays_in_range() {
        let (_root, tabs) = boot().await;
        tabs.open().await.unwrap();
        assert!(!tabs.activate(5), "越界切页当没按");
        assert_eq!(tabs.active_index(), 1);
        assert!(tabs.activate(0));
        assert_eq!(tabs.active_index(), 0);
        // 标签上的号切页：号是稳定的，没有这个号就当没按。
        assert!(!tabs.activate_id(99));
        assert!(tabs.activate_id(2));
        assert_eq!(tabs.active_index(), 1);
    }

    /// 关页必须把**整棵**子树带走：会话 actor 和 agent 循环都挂在页 fiber 下面，
    /// 不级联就等于每关一页漏一个还在跑的后台任务。
    #[tokio::test]
    async fn closing_a_tab_disposes_its_subtree() {
        use std::sync::atomic::AtomicBool;

        let disposed = Arc::new(AtomicBool::new(false));
        let flag = disposed.clone();
        let mount: TabMount = Arc::new(move |_id, _kind| {
            let flag = flag.clone();
            cordis::plugin_async("tab-probe", Inject::new(), move |ctx, _: &()| {
                let flag = flag.clone();
                async move {
                    // 子插件：分页的会话 / 循环 / 视图都是这样挂进去的。
                    ctx.plugin(leaf(flag), ())?.wait().await?;
                    Ok(None)
                }
            })
        });
        fn leaf(flag: Arc<AtomicBool>) -> Plugin {
            plugin("tab-probe-leaf", Inject::new(), move |_ctx, _: &()| {
                let flag = flag.clone();
                Ok(Some(cordis::Disposable::from_fn(move || {
                    flag.store(true, Ordering::Relaxed)
                })))
            })
        }

        let root = Context::new();
        root.plugin(tabs(), mount).unwrap().wait().await.unwrap();
        let tabs = root.get::<Tabs>(TUI_TABS).unwrap();
        tabs.open().await.unwrap();
        assert!(!disposed.load(Ordering::Relaxed));

        tabs.close(1).await.unwrap();
        assert!(
            disposed.load(Ordering::Relaxed),
            "关页没有级联 dispose 子插件"
        );
    }

    /// 带回来源页的块要注明出处、整段引用，且**不**替用户按下发送。
    #[test]
    fn carried_block_quotes_and_credits() {
        let block = carried_block(3, "第一行\n第二行");
        assert!(block.starts_with("（来自第 3 页）"), "{block}");
        assert!(block.contains("> 第一行\n"), "{block}");
        assert!(block.contains("> 第二行\n"), "{block}");
    }

    /// 只有工具调用、没有正文的那几步不该被带回去 —— 要的是结论。
    #[test]
    fn last_reply_skips_toolonly_steps() {
        let ctx = Context::new();
        let sessions = Sessions::isolated_as(ctx, "probe");
        sessions.append(LogEvent::User("问".into()));
        sessions.append(LogEvent::LlmStream(cordis_spine::LlmOutput {
            text: String::new(),
            tool_calls: vec![cordis_spine::ToolCall {
                id: "1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            }],
            ..Default::default()
        }));
        sessions.append(LogEvent::LlmStream(cordis_spine::LlmOutput {
            text: "结论".into(),
            ..Default::default()
        }));
        assert_eq!(last_reply(&sessions).as_deref(), Some("结论"));
    }

    #[tokio::test]
    async fn tabs_are_capped() {
        let (_root, tabs) = boot().await;
        for _ in 1..MAX_TABS {
            tabs.open().await.unwrap();
        }
        assert_eq!(tabs.len(), MAX_TABS);
        assert!(tabs.open().await.is_err(), "第 10 页该被挡住");
    }
}
