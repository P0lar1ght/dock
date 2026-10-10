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
//! 装一页要挂哪些插件由**组合根**（`cordis-app`）给：分页只负责开、关、切。
//! 它是宿主之间的契约——TUI 按它取当前页，网关按它开线程，定时任务驱动按它
//! 把会话开成一页——所以住在 spine，不在任何一个宿主的 crate 里。宿主自己的
//! 每页视图（TUI 的滚动区、输入框……）经 [`TabsConfig::per_tab`] 加进隔离名单。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::host::session_port::SessionRef;
use crate::names::{
    AGENT_LOOP, AGENT_PRESETS, ASK, GOAL, MCP, PERMISSIONS, PLAN_MODE, SESSION, SESSIONS,
    SESSION_PORT, SETTINGS, TABS, TABS_PAGE_OPENED, TODOS, TURN,
};
use crate::{AgentPresets, Ask, LogEvent, Mcp, Permissions, PlanMode, Sessions};
use cordis::{plugin, Context, Fiber, Inject, Plugin};

/// 每页各有一份的 harness 服务。没列进来的一律落回根（全局单例：工具表、LLM、
/// MCP 连接、浏览器、cua、后台任务……）。宿主的每页视图不在这里，由组合根经
/// [`TabsConfig::per_tab`] 补上。
///
/// `settings` 是这一页选的模型、协议、权限模式。`permissions` / `ask` 是这一页
/// 自己的队列：后台页的批准框不会弹到正在看的那一页上。`agentPresets` 也各一份：
/// 一页 `/preset` 或 `/cd` 不改别的页（旁问页的是只读 overlay）。MCP 连接仍是
/// 全局的，elicitation 在请求上盖来源页。
pub const PER_TAB_SERVICES: &[&str] = &[
    SESSIONS,
    TURN,
    AGENT_LOOP,
    SESSION,
    SESSION_PORT,
    GOAL,
    TODOS,
    PLAN_MODE,
    SETTINGS,
    PERMISSIONS,
    ASK,
    AGENT_PRESETS,
];

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

/// `tabs()` 插件的配置：建页工厂 + 宿主要按页隔离的额外名字。
#[derive(Clone)]
pub struct TabsConfig {
    pub mount: TabMount,
    /// 除 [`PER_TAB_SERVICES`] 外每页各一份的名字（TUI 的四个视图）。无头宿主留空。
    pub per_tab: &'static [&'static str],
}

impl TabsConfig {
    /// 不带宿主视图的分页（`dock serve`、测试装配）。
    pub fn headless(mount: TabMount) -> Self {
        Self {
            mount,
            per_tab: &[],
        }
    }
}

/// [`Tabs::carry_back`] 的结果：要带回哪一页、带什么。由宿主决定往哪儿放
/// （TUI 填进那一页的输入框）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarryBack {
    /// 来源页的下标（当前位置，不是稳定编号）。
    pub origin_index: usize,
    /// 来源页的稳定编号。
    pub origin_id: usize,
    /// 已注明出处、整段引用好的块。
    pub text: String,
}

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
    /// 这一页有权限 / 提问 / 计划批准 / MCP elicitation 在等你。
    pub pending: bool,
}

struct Tab {
    id: usize,
    ctx: Context,
    /// 第 1 页就是根，没有可以 dispose 的 fiber。
    fiber: Option<Fiber>,
    origin: Option<usize>,
    /// 旁问页分叉时来源页写的那份会话（`live_session_id`）。来源页之后换了会话
    /// （`Ctrl+W` / `/resume`），这张旁问页带的就是旧会话的快照，不再算它的侧边
    /// 聊天。空串 = 来源页当时还没落过盘，不比。常驻页恒为 `None`。
    origin_session: Option<String>,
    kind: TabKind,
}

/// Named `"tabs"`。调用点 live-lookup，别把 `Arc` 关进长生命周期闭包。
#[derive(Clone)]
pub struct Tabs {
    inner: Arc<Inner>,
}

struct Inner {
    root: Context,
    mount: TabMount,
    per_tab: &'static [&'static str],
    tabs: Mutex<Vec<Tab>>,
    active: AtomicUsize,
    next_id: AtomicUsize,
    /// 正在建的页（按稳定编号）从哪一页继承模型、权限模式、cwd、预设。按页记，
    /// 并发建页互不串；建完或建页的 future 被取消都由 [`SourceGuard`] 撤掉。
    mount_sources: Mutex<HashMap<usize, Context>>,
    /// 串行 [`Tabs::open_aside_for`]：「这一页有没有侧边聊天」和「挂上新的一张」
    /// 中间隔着建页的 await，不串行的话两个请求会各挂一张。
    aside_opening: tokio::sync::Mutex<()>,
}

/// 建页期间登记的来源页，drop 时撤掉。
struct SourceGuard<'a> {
    inner: &'a Inner,
    id: usize,
}

impl Drop for SourceGuard<'_> {
    fn drop(&mut self) {
        self.inner
            .mount_sources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

impl Tabs {
    fn new(root: Context, config: TabsConfig) -> Self {
        let first = Tab {
            id: 1,
            ctx: root.clone(),
            fiber: None,
            origin: None,
            origin_session: None,
            kind: TabKind::Normal,
        };
        Self {
            inner: Arc::new(Inner {
                root,
                mount: config.mount,
                per_tab: config.per_tab,
                tabs: Mutex::new(vec![first]),
                active: AtomicUsize::new(0),
                next_id: AtomicUsize::new(2),
                mount_sources: Mutex::new(HashMap::new()),
                aside_opening: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.tabs.lock().unwrap().len()
    }

    /// 某一种页开着几张。页数上限只数常驻页：旁问页不占名额（另有同样的上限）。
    fn count(&self, kind: TabKind) -> usize {
        self.inner
            .tabs
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.kind == kind)
            .count()
    }

    /// 正在建的第 `id` 页（建页工厂拿到的那个编号）该从哪一页继承设置：平时是
    /// 开它时的当前页，[`Self::open_aside_for`] 开的是被分叉的那一页。建页工厂
    /// （组合根）在挂插件时调；不在建页期间问的落回当前页。
    pub fn mount_source(&self, id: usize) -> Context {
        let source = self.inner.mount_sources.lock().unwrap().get(&id).cloned();
        source.unwrap_or_else(|| self.active_ctx())
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

    /// 每一页的 ctx。dashboard 要挨页问 `"sessions"`（每页一份），
    /// [`active_ctx`](Self::active_ctx) 只给当前那一页。
    pub fn contexts(&self) -> Vec<Context> {
        self.inner
            .tabs
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.ctx.clone())
            .collect()
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
                pending: tab_pending(&tab.ctx),
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

    /// 把一份磁盘会话开成自己的常驻页，并切过去。
    ///
    /// 当前页不动。已经有一页的 `live_session_id` 就是它时，只切到那一页，
    /// 不再复制一份。新页先按自己的 cwd `attach_disk`，再 `adopt_archived`，
    /// 之后的对话写回原来的会话目录。
    pub async fn open_archived(&self, session_id: &str) -> Result<usize, String> {
        if let Some(index) = self.index_of_session(session_id) {
            self.activate(index);
            let tabs = self.inner.tabs.lock().unwrap();
            return Ok(tabs[index].id);
        }
        if self.count(TabKind::Normal) >= MAX_TABS {
            return Err(format!("最多 {MAX_TABS} 页"));
        }
        // 先确认磁盘上有这份会话，再挂页。挂完才发现没有，会白白占掉一个页号。
        // 新页继承当前页的 cwd（`tab.sessions`），所以按当前页的 cwd 查。
        let cwd = crate::session_cwd(&self.active_ctx());
        if !Sessions::can_adopt_archived(session_id, &cwd) {
            return Err("这个会话开不了页（只认当前工作目录下的历史）".into());
        }
        let (id, child, fiber) = self
            .mount_page(TabKind::Normal, None, self.active_ctx())
            .await?;
        let adopted = child
            .get::<Sessions>(SESSIONS)
            .is_some_and(|sessions| sessions.adopt_archived(session_id));
        if !adopted {
            let _ = fiber.dispose().await;
            return Err("这个会话开不了页（只认当前工作目录下的历史）".into());
        }
        let index = {
            let mut tabs = self.inner.tabs.lock().unwrap();
            tabs.push(Tab {
                id,
                ctx: child,
                fiber: Some(fiber),
                origin: None,
                origin_session: None,
                kind: TabKind::Normal,
            });
            tabs.len() - 1
        };
        self.activate(index);
        Ok(id)
    }

    /// 哪一页正在写这份磁盘会话。空 id 不算（空白页的 `live_id` 都是空的）。
    fn index_of_session(&self, session_id: &str) -> Option<usize> {
        if session_id.is_empty() {
            return None;
        }
        self.inner.tabs.lock().unwrap().iter().position(|tab| {
            tab.ctx
                .get::<Sessions>(SESSIONS)
                .is_some_and(|sessions| sessions.live_session_id() == session_id)
        })
    }

    /// 已有页正 live 在这份磁盘会话上时切到那一页，返回页号。
    ///
    /// `/resume` / `--resume` 直接 `restore(id)` 会让两页同时往同一个
    /// `chat_history.jsonl` 追加，损坏历史；和 [`Self::open_archived`] 一样，
    /// 已开着的会话只切不复制。当前页自己的 live id 不会出现在归档列表里，
    /// 所以命中的一定是别的页。
    pub fn switch_to_live_session(&self, session_id: &str) -> Option<usize> {
        let index = self.index_of_session(session_id)?;
        let id = {
            let tabs = self.inner.tabs.lock().unwrap();
            tabs.get(index).map(|tab| tab.id)
        };
        self.activate(index);
        id
    }

    /// 起一棵页子树并把快照种进去。旁问页与常驻页共用这条路。新页的设置、cwd、
    /// 预设从 `source` 继承（建页工厂经 [`Self::mount_source`] 取）。
    async fn mount_page(
        &self,
        kind: TabKind,
        seed: Option<Vec<LogEvent>>,
        source: Context,
    ) -> Result<(usize, Context, Fiber), String> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        self.inner.mount_sources.lock().unwrap().insert(id, source);
        let _source = SourceGuard {
            inner: &self.inner,
            id,
        };
        // 只把该隔离的名字拉进新 realm，其余照旧解析到根。
        let mut child = self.inner.root.clone();
        for name in PER_TAB_SERVICES.iter().chain(self.inner.per_tab) {
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
        // 旁问页不落盘，没有会话 id；网关按 id 寻址线程，给它一个。
        if kind == TabKind::Aside {
            if let Some(sessions) = child.get::<Sessions>(SESSIONS) {
                sessions.assign_ephemeral_id();
            }
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
        if self.count(kind) >= MAX_TABS {
            return Err(format!("最多 {MAX_TABS} 页"));
        }
        let origin = seed.as_ref().map(|(origin, _)| *origin);
        let source = self.active_ctx();
        let origin_session = (kind == TabKind::Aside).then(|| {
            source
                .get::<Sessions>(SESSIONS)
                .map(|s| s.live_session_id())
                .unwrap_or_default()
        });
        let (id, child, fiber) = self
            .mount_page(kind, seed.map(|(_, events)| events), source)
            .await?;
        let index = {
            let mut tabs = self.inner.tabs.lock().unwrap();
            tabs.push(Tab {
                id,
                ctx: child,
                fiber: Some(fiber),
                origin,
                origin_session,
                kind,
            });
            tabs.len() - 1
        };
        self.activate(index);
        Ok(index)
    }

    /// 在 `cwd` 开一张常驻页，**不切过去**；给了 `archived` 就把那份磁盘会话
    /// 开进来（它得在 `cwd` 下）。网关 / 桌面 GUI 用：远端开一页不该把终端里正在
    /// 看的那页换走。那份会话已经有一页开着时直接回那一页。
    pub async fn open_at(&self, cwd: &Path, archived: Option<&str>) -> Result<Context, String> {
        if let Some(index) = archived.and_then(|id| self.index_of_session(id)) {
            return Ok(self.inner.tabs.lock().unwrap()[index].ctx.clone());
        }
        if self.count(TabKind::Normal) >= MAX_TABS {
            return Err(format!("最多 {MAX_TABS} 页"));
        }
        let (id, child, fiber) = self
            .mount_page(TabKind::Normal, None, self.active_ctx())
            .await?;
        let opened = (|| {
            // 建页时继承的是当前页的 cwd；改钉到 `cwd`，再按新目录重新挂盘。
            crate::change_dir(&child, cwd)?;
            let sessions = child
                .get::<Sessions>(SESSIONS)
                .ok_or_else(|| "新页没有会话".to_string())?;
            sessions.attach_disk();
            if let Some(archived) = archived {
                if !sessions.adopt_archived(archived) {
                    return Err(format!("{} 下没有会话 {archived}", cwd.display()));
                }
            }
            Ok(())
        })();
        if let Err(e) = opened {
            let _ = fiber.dispose().await;
            return Err(e);
        }
        self.inner.tabs.lock().unwrap().push(Tab {
            id,
            ctx: child.clone(),
            fiber: Some(fiber),
            origin: None,
            origin_session: None,
            kind: TabKind::Normal,
        });
        Ok(child)
    }

    /// 把落盘会话 `session_id` 开成一页（在它自己的 `cwd` 下），预设切回它记的那个；
    /// 已经开着就回那一页。**不切**当前页——这是后台要往某个会话里送东西用的（定时
    /// 任务到点）。新开的页发 [`TABS_PAGE_OPENED`]，网关据此把它纳入投影。
    pub async fn open_session(&self, session_id: &str, cwd: &Path) -> Result<Context, String> {
        if let Some(index) = self.index_of_session(session_id) {
            return Ok(self.inner.tabs.lock().unwrap()[index].ctx.clone());
        }
        let ctx = self.open_at(cwd, Some(session_id)).await?;
        // 预设跟着会话走（同网关 `thread/open`）；坏了 / 没了就沿用继承来的，只记一笔。
        let stamped = ctx.get::<Sessions>(SESSIONS).and_then(|s| s.preset_id());
        if let (Some(id), Some(presets)) = (stamped, ctx.get::<AgentPresets>(AGENT_PRESETS)) {
            if let Err(e) = presets.pin(&id) {
                eprintln!("dock: 定时任务开页，预设 `{id}` 没切过去：{e}");
            }
        }
        let identity = ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.identity().to_string())
            .unwrap_or_default();
        self.inner.root.emit(TABS_PAGE_OPENED, identity);
        Ok(ctx)
    }

    /// 给正在写 `parent_session_id` 的那一页开一张只读旁问页（GUI 的侧边聊天），
    /// **不切过去**；那一页已经有旁问页就回那一张（每页最多一张）。第二个返回值：
    /// 是不是已有的。
    ///
    /// 和 [`Self::ask_aside`] 一样带着来源页的上下文快照、只读预设、不落盘；不同
    /// 的是来源页不必是当前页（设置、cwd 从来源页继承），也不替用户发问题。
    ///
    /// 同一页的并发调用串行：后到的拿到先到的那张。TUI `/btw` 不走这条，同一页
    /// 可以另有 `/btw` 开的旁问页；那样回的是最早开的那张。
    pub async fn open_aside_for(&self, parent_session_id: &str) -> Result<(Context, bool), String> {
        let _opening = self.inner.aside_opening.lock().await;
        let (parent_id, parent_ctx) = {
            let index = self
                .index_of_session(parent_session_id)
                .ok_or_else(|| format!("会话 {parent_session_id} 没有开着"))?;
            let tabs = self.inner.tabs.lock().unwrap();
            let tab = &tabs[index];
            if tab.kind == TabKind::Aside {
                return Err("侧边聊天里不能再开侧边聊天".into());
            }
            (tab.id, tab.ctx.clone())
        };
        if let Some(existing) = self.aside_of_id(parent_id) {
            return Ok((existing, true));
        }
        let snapshot = parent_ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.model_history())
            .unwrap_or_default();
        if self.count(TabKind::Aside) >= MAX_TABS {
            return Err(format!("最多 {MAX_TABS} 个侧边聊天"));
        }
        let (id, child, fiber) = self
            .mount_page(TabKind::Aside, Some(snapshot), parent_ctx)
            .await?;
        self.inner.tabs.lock().unwrap().push(Tab {
            id,
            ctx: child.clone(),
            fiber: Some(fiber),
            origin: Some(parent_id),
            origin_session: Some(parent_session_id.to_string()),
            kind: TabKind::Aside,
        });
        Ok((child, false))
    }

    /// 稳定编号 `parent_id` 那一页的旁问页（来源页换过会话后留下的旧旁问页不算）。
    fn aside_of_id(&self, parent_id: usize) -> Option<Context> {
        let tabs = self.inner.tabs.lock().unwrap();
        tabs.iter()
            .find(|t| {
                t.kind == TabKind::Aside && t.origin == Some(parent_id) && aside_current(&tabs, t)
            })
            .map(|t| t.ctx.clone())
    }

    /// 正在写 `parent_session_id` 的那一页挂着的旁问页。
    pub fn aside_of(&self, parent_session_id: &str) -> Option<Context> {
        let index = self.index_of_session(parent_session_id)?;
        let parent_id = self.inner.tabs.lock().unwrap()[index].id;
        self.aside_of_id(parent_id)
    }

    /// `session_id` 是一张旁问页时，它的来源页（还开着、没换会话的话）。不是旁问页
    /// 是 `None`。
    pub fn aside_parent(&self, session_id: &str) -> Option<Context> {
        let index = self.index_of_session(session_id)?;
        let tabs = self.inner.tabs.lock().unwrap();
        let tab = &tabs[index];
        if tab.kind != TabKind::Aside || !aside_current(&tabs, tab) {
            return None;
        }
        let origin = tab.origin?;
        tabs.iter().find(|t| t.id == origin).map(|t| t.ctx.clone())
    }

    /// 当前页是旁问页时：`(旁问页, 来源页, 来源页的号)`，`/tab merge` 的两头。当前页
    /// 不是旁问页回 `Ok(None)`。来源页关了、或者换过会话（笔记会写进另一份会话）
    /// 报错。在一把锁里取，中间关页不会错位。
    pub fn active_aside_origin(&self) -> Result<Option<(Context, Context, usize)>, String> {
        let tabs = self.inner.tabs.lock().unwrap();
        let index = self.active_index().min(tabs.len().saturating_sub(1));
        let tab = tabs.get(index).ok_or_else(|| "没有当前页".to_string())?;
        if tab.kind != TabKind::Aside {
            return Ok(None);
        }
        let origin_id = tab.origin.ok_or_else(|| "这一页没有来源页".to_string())?;
        let origin = tabs
            .iter()
            .find(|t| t.id == origin_id)
            .ok_or_else(|| format!("来源页（第 {origin_id} 页）已经关掉了"))?;
        if !aside_current(&tabs, tab) {
            return Err(format!(
                "第 {origin_id} 页已经换了会话，这段旁问是从之前那份会话分叉的，不写进去"
            ));
        }
        Ok(Some((tab.ctx.clone(), origin.ctx.clone(), origin_id)))
    }

    /// `session_id` 那一页是不是旁问页。
    pub fn is_aside(&self, session_id: &str) -> bool {
        self.index_of_session(session_id)
            .is_some_and(|i| self.inner.tabs.lock().unwrap()[i].kind == TabKind::Aside)
    }

    /// 关掉正在写 `session_id` 的那一页。第一页（主会话）关不掉。
    pub async fn close_session(&self, session_id: &str) -> Result<(), String> {
        let index = self
            .index_of_session(session_id)
            .ok_or_else(|| format!("会话 {session_id} 没有开着"))?;
        self.close(index).await.map(|_| ())
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

    /// 取出当前页最近一条模型回复，准备带回它的来源页。**不**切页、**不**往
    /// 来源页的历史里写 —— 那等于替用户说话。放到哪儿由宿主决定（TUI 填进来源页
    /// 的输入框再切过去），发不发、怎么改都还是用户说了算。
    pub fn carry_back(&self) -> Result<CarryBack, String> {
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
        Ok(CarryBack {
            origin_index,
            origin_id,
            text: carried_block(source_id, &text),
        })
    }

    /// 关掉第 `index` 页。第一页关不掉（它是根，关了就没有 dock 了）。
    ///
    /// 从这一页分叉的旁问页（`/btw`、GUI 的侧边聊天）跟着关：来源页没了，它们
    /// 既带不回、也写不回。TUI `/tab close` 和网关 `thread/close` 都走这里。
    pub async fn close(&self, index: usize) -> Result<usize, String> {
        if index == 0 {
            return Err("第一页关不掉（它是主会话）".into());
        }
        let id = {
            let tabs = self.inner.tabs.lock().unwrap();
            tabs.get(index)
                .map(|t| t.id)
                .ok_or_else(|| "没有这一页".to_string())?
        };
        loop {
            let aside = self
                .inner
                .tabs
                .lock()
                .unwrap()
                .iter()
                .position(|t| t.kind == TabKind::Aside && t.origin == Some(id));
            let Some(aside) = aside else { break };
            self.close_one(aside).await?;
        }
        let index = self
            .index_of_id(id)
            .ok_or_else(|| "没有这一页".to_string())?;
        self.close_one(index).await
    }

    async fn close_one(&self, index: usize) -> Result<usize, String> {
        let tab = {
            let mut tabs = self.inner.tabs.lock().unwrap();
            if index >= tabs.len() {
                return Err("没有这一页".into());
            }
            tabs.remove(index)
        };
        let id = tab.id;
        // 该页的 MCP elicitation 队列是全局的，fiber dispose 带不走它：
        // 不 cancel 的话这条 job 哪页都不显示，工具调用永远不返回。
        if let (Some(sessions), Some(mcp)) =
            (tab.ctx.get::<Sessions>(SESSIONS), tab.ctx.get::<Mcp>(MCP))
        {
            let page = sessions.ui_page();
            mcp.elicitation().cancel_for(page.as_deref());
        }
        if let Some(sessions) = tab.ctx.get::<Sessions>(SESSIONS) {
            crate::discard_ephemeral_plan(&sessions);
        }
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

/// 旁问页还跟着它分叉时的那份会话：来源页还开着，并且没换会话。
fn aside_current(tabs: &[Tab], aside: &Tab) -> bool {
    let Some(origin) = aside.origin.and_then(|id| tabs.iter().find(|t| t.id == id)) else {
        return false;
    };
    match aside.origin_session.as_deref() {
        None | Some("") => true,
        Some(forked_from) => origin
            .ctx
            .get::<Sessions>(SESSIONS)
            .is_some_and(|s| s.live_session_id() == forked_from),
    }
}

fn tab_working(ctx: &Context) -> bool {
    ctx.get::<SessionRef>(SESSION_PORT)
        .is_some_and(|s| s.working())
}

/// 这一页自己的队列里有没有在等人回答的东西。切走之后标签上标出来。
fn tab_pending(ctx: &Context) -> bool {
    let page = ctx.get::<Sessions>(SESSIONS).and_then(|s| s.ui_page());
    ctx.get::<Permissions>(PERMISSIONS)
        .is_some_and(|p| p.front().is_some())
        || ctx.get::<Ask>(ASK).is_some_and(|a| a.front().is_some())
        || ctx
            .get::<PlanMode>(PLAN_MODE)
            .is_some_and(|p| p.front().is_some())
        || ctx
            .get::<Mcp>(MCP)
            .is_some_and(|m| m.elicitation().front_for(page.as_deref()).is_some())
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
                LogEvent::User(text) => {
                    crate::session::persist::title_from_text(text).map(|t| clip(&t))
                }
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

/// Mount named `"tabs"`。config 是组合根给的建页工厂与宿主的每页名字。
///
/// config 的类型必须是 [`TabsConfig`]（不带视图用 [`TabsConfig::headless`]）。插件
/// 配置在运行时才做类型检查：传裸的 [`TabMount`] 能编译，挂载时才报
/// 「plugin config has the wrong type」。
pub fn tabs() -> Plugin {
    plugin("tabs", Inject::new(), |ctx, config: &TabsConfig| {
        Ok(Some(
            ctx.provide(TABS, Tabs::new(ctx.clone(), config.clone()))?,
        ))
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
        root.plugin(tabs(), TabsConfig::headless(noop_mount()))
            .unwrap()
            .wait()
            .await
            .unwrap();
        let tabs = root.get::<Tabs>(TABS).unwrap();
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
        root.plugin(tabs(), TabsConfig::headless(mount))
            .unwrap()
            .wait()
            .await
            .unwrap();
        let tabs = root.get::<Tabs>(TABS).unwrap();
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
        sessions.append(LogEvent::LlmStream(crate::LlmOutput {
            text: String::new(),
            tool_calls: vec![crate::ToolCall {
                id: "1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            }],
            ..Default::default()
        }));
        sessions.append(LogEvent::LlmStream(crate::LlmOutput {
            text: "结论".into(),
            ..Default::default()
        }));
        assert_eq!(last_reply(&sessions).as_deref(), Some("结论"));
    }

    /// 每页带一份会话的建页工厂，并记下建每一页时 [`Tabs::mount_source`] 指向谁
    /// （组合根的 `tab.settings` / `tab.sessions` 就是从它继承）。
    fn session_mount(seen: Arc<Mutex<Vec<String>>>) -> TabMount {
        Arc::new(move |id, kind| {
            let seen = seen.clone();
            plugin("tab-sessions", Inject::new(), move |ctx, _: &()| {
                let from = ctx
                    .get::<Tabs>(TABS)
                    .and_then(|t| t.mount_source(id).get::<Sessions>(SESSIONS))
                    .map(|s| s.identity().to_string())
                    .unwrap_or_default();
                seen.lock().unwrap().push(from);
                let sessions = Sessions::tab(ctx.clone(), id);
                if kind == TabKind::Normal {
                    // 常驻页在真装配里挂盘才有 id；测试里直接给一个。
                    sessions.assign_ephemeral_id();
                }
                Ok(Some(ctx.provide(SESSIONS, sessions)?))
            })
        })
    }

    fn session_id_of(ctx: &Context) -> String {
        ctx.get::<Sessions>(SESSIONS).unwrap().live_session_id()
    }

    /// 网关给某个会话开侧边聊天：设置从**那个会话**继承（它不一定是当前页），
    /// 当前页不动；再开一次回同一张；旁问页不占常驻页的名额。
    #[tokio::test]
    async fn an_aside_forks_from_its_parent_not_the_active_tab() {
        let root = Context::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        root.plugin(tabs(), TabsConfig::headless(session_mount(seen.clone())))
            .unwrap()
            .wait()
            .await
            .unwrap();
        let tabs = (*root.get::<Tabs>(TABS).unwrap()).clone();
        tabs.open().await.unwrap();
        let parent = tabs.active_ctx();
        let parent_id = session_id_of(&parent);
        tabs.open().await.unwrap();
        let active = tabs.active_index();

        let (aside, existing) = tabs.open_aside_for(&parent_id).await.unwrap();
        assert!(!existing);
        assert_eq!(tabs.active_index(), active, "开侧边聊天不切页");
        let parent_identity = parent
            .get::<Sessions>(SESSIONS)
            .unwrap()
            .identity()
            .to_string();
        assert_eq!(
            seen.lock().unwrap().last(),
            Some(&parent_identity),
            "要从来源页继承"
        );
        let aside_id = session_id_of(&aside);
        assert!(aside_id.starts_with("aside-"), "{aside_id}");
        assert!(tabs.is_aside(&aside_id));
        assert!(!tabs.is_aside(&parent_id));
        assert_eq!(
            tabs.aside_parent(&aside_id).map(|c| session_id_of(&c)),
            Some(parent_id.clone())
        );

        let (again, existing) = tabs.open_aside_for(&parent_id).await.unwrap();
        assert!(existing, "每页最多一张旁问页");
        assert_eq!(session_id_of(&again), aside_id);
        assert!(
            tabs.open_aside_for(&aside_id).await.is_err(),
            "旁问里不再开旁问"
        );

        while tabs.count(TabKind::Normal) < MAX_TABS {
            tabs.open().await.unwrap();
        }
        assert_eq!(tabs.len(), MAX_TABS + 1, "旁问页不占常驻页的名额");
    }

    /// 同 [`session_mount`]，但旁问页要等 `gate` 放行才挂好（`waiting` 记有几张在等）：
    /// 好在它建到一半时插进别的操作。
    fn gated_mount(
        seen: Arc<Mutex<Vec<String>>>,
        gate: Arc<tokio::sync::Semaphore>,
        waiting: Arc<AtomicUsize>,
    ) -> TabMount {
        let inner = session_mount(seen);
        Arc::new(move |id, kind| {
            let page = inner(id, kind);
            let gate = gate.clone();
            let waiting = waiting.clone();
            cordis::plugin_async("gated", Inject::new(), move |ctx, _: &()| {
                let (page, gate, waiting) = (page.clone(), gate.clone(), waiting.clone());
                async move {
                    if kind == TabKind::Aside {
                        waiting.fetch_add(1, Ordering::SeqCst);
                        gate.acquire().await.unwrap().forget();
                    }
                    ctx.plugin(page, ())?.wait().await?;
                    Ok(None)
                }
            })
        })
    }

    struct Gated {
        tabs: Tabs,
        seen: Arc<Mutex<Vec<String>>>,
        gate: Arc<tokio::sync::Semaphore>,
        waiting: Arc<AtomicUsize>,
    }

    async fn gated_boot() -> Gated {
        let root = Context::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let waiting = Arc::new(AtomicUsize::new(0));
        root.plugin(
            tabs(),
            TabsConfig::headless(gated_mount(seen.clone(), gate.clone(), waiting.clone())),
        )
        .unwrap()
        .wait()
        .await
        .unwrap();
        let tabs = (*root.get::<Tabs>(TABS).unwrap()).clone();
        Gated {
            tabs,
            seen,
            gate,
            waiting,
        }
    }

    fn identity_of(ctx: &Context) -> String {
        ctx.get::<Sessions>(SESSIONS)
            .unwrap()
            .identity()
            .to_string()
    }

    /// 侧边聊天建到一半时另开一页（TUI `Ctrl+N`），或者建侧边聊天的请求被取消：
    /// 那一页都该从当前页继承，不能串到被分叉的那一页上。以前来源是一个全局槽，
    /// 建页期间谁来都读到它；future 取消时也不复位。
    #[tokio::test]
    async fn pages_opened_while_an_aside_mounts_inherit_the_active_page() {
        let Gated {
            tabs,
            seen,
            gate,
            waiting,
        } = gated_boot().await;
        tabs.open().await.unwrap();
        let parent = session_id_of(&tabs.active_ctx());
        tabs.open().await.unwrap();
        let active_index = tabs.active_index();
        let active = identity_of(&tabs.active_ctx());

        let opening = tokio::spawn({
            let tabs = tabs.clone();
            let parent = parent.clone();
            async move { tabs.open_aside_for(&parent).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while waiting.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("旁问页开始建");
        tabs.open().await.unwrap();
        assert_eq!(
            seen.lock().unwrap().last(),
            Some(&active),
            "建到一半另开的页"
        );

        opening.abort();
        let _ = opening.await;
        tabs.activate(active_index);
        tabs.open().await.unwrap();
        assert_eq!(
            seen.lock().unwrap().last(),
            Some(&active),
            "建侧边聊天被取消之后开的页"
        );
        gate.add_permits(1);
    }

    /// 同一页的侧边聊天并发开两次：只挂一张，后到的拿到同一张。
    #[tokio::test]
    async fn concurrent_asides_for_one_page_share_one() {
        let Gated { tabs, gate, .. } = gated_boot().await;
        tabs.open().await.unwrap();
        let parent = session_id_of(&tabs.active_ctx());
        let open = |tabs: Tabs, parent: String| {
            tokio::spawn(async move { tabs.open_aside_for(&parent).await })
        };
        let a = open(tabs.clone(), parent.clone());
        let b = open(tabs.clone(), parent.clone());
        gate.add_permits(2);
        let (a, a_existing) = a.await.unwrap().unwrap();
        let (b, b_existing) = b.await.unwrap().unwrap();
        assert_eq!(session_id_of(&a), session_id_of(&b));
        assert!(a_existing != b_existing, "一个新开、一个回已有的");
        assert_eq!(tabs.count(TabKind::Aside), 1);
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
