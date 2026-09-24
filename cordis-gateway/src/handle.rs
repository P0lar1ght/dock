//! Named `"gateway"` implementation. Pairing + transcript live here; HTTP
//! handlers live-look spine services off `Context`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use cordis::Context;
use cordis_spine::{
    Ask, LogEvent, Mcp, PageLogEvent, PageTurnEnd, Permissions, PlanMode, Sessions, ASK, ASK_EVENT,
    MCP, MCP_ELICIT_EVENT, PERMISSIONS, PERMISSION_EVENT, PLAN_EVENT, PLAN_MODE, ROOT_IDENTITY,
    SESSIONS, SESSION_PAGE_EVENT, SESSION_TURN_END,
};
use cordis_tui::{
    CompanionStatus, GatewayPort, GatewayRef, PairingBinding, PairingError, PairingPrompt,
};

use crate::image_store::ImageInputStore;
use crate::pairing::{IssuedTicket, PairingStatus, PairingStore};
use crate::protocol::LIVE_THREAD_ID;
use crate::transcript::{ProjectedEvent, Transcript};

struct ListenSlot {
    listening: bool,
    local_addr: SocketAddr,
    companion: CompanionStatus,
    shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
}

pub struct GatewayInner {
    pub ctx: Context,
    pub pairing: Mutex<PairingStore>,
    /// 每页一份投影，按分页身份（`main` / `main#N`）存，懒建。
    transcripts: Mutex<HashMap<String, Transcript>>,
    /// 各页投影共用的推送通道；事件上带页身份，`ws.rs` 按订阅过滤。
    events_tx: tokio::sync::broadcast::Sender<ProjectedEvent>,
    pub images: Mutex<ImageInputStore>,
    preferred: SocketAddr,
    listen: Mutex<ListenSlot>,
}

#[derive(Clone)]
pub struct GatewayHandle {
    inner: Arc<GatewayInner>,
    /// 线程级处理（`slash/execute` 等）作用在哪一页；`None` 是第 1 页（根）。
    /// 见 [`Self::scoped`]。
    scope: Option<crate::threads::Page>,
}

impl GatewayHandle {
    pub fn idle(ctx: Context, preferred: SocketAddr) -> Self {
        let inner = Arc::new(GatewayInner {
            pairing: Mutex::new(PairingStore::new(ctx.clone())),
            transcripts: Mutex::new(HashMap::new()),
            events_tx: Transcript::channel(),
            images: Mutex::new(ImageInputStore::new()),
            preferred,
            listen: Mutex::new(ListenSlot {
                listening: false,
                local_addr: preferred,
                companion: CompanionStatus::Stopped,
                shutdown_tx: None,
            }),
            ctx: ctx.clone(),
        });
        listen_events(&inner);
        Self { inner, scope: None }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, ListenSlot> {
        self.inner.listen.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn companion_status(&self) -> CompanionStatus {
        self.slot().companion.clone()
    }

    pub fn listen_addr(&self) -> SocketAddr {
        self.slot().local_addr
    }

    pub fn is_listening(&self) -> bool {
        self.slot().listening
    }

    pub fn start_listen(&self) -> Result<SocketAddr, PairingError> {
        let mut g = self.slot();
        if g.listening {
            return Ok(g.local_addr);
        }
        let preferred = self.inner.preferred;
        let (listener, actual) = match crate::bind::listen(&preferred.to_string()) {
            Ok(ok) => ok,
            Err(msg) => {
                g.companion = CompanionStatus::Failed {
                    addr: preferred,
                    error: msg.clone(),
                };
                return Err(PairingError::new("bind_failed", msg));
            }
        };
        if preferred.port() != 0 && actual.port() != preferred.port() {
            eprintln!("dock gateway: {} 已被占用，改绑 {}", preferred, actual);
        }
        let companion = crate::bind::companion_listener(actual);
        if let CompanionStatus::Failed { addr, error } = &companion.status {
            eprintln!(
                "dock gateway: 未能监听 {addr}（{error}）。本机 IPv6 / localhost 可能连不上。"
            );
        }
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        g.listening = true;
        g.local_addr = actual;
        g.companion = companion.status.clone();
        g.shutdown_tx = Some(shutdown_tx);
        drop(g);

        let app = crate::http::router(self.clone());
        crate::http::spawn_listener(listener, app.clone(), shutdown_rx.clone());
        if let Some(v6) = companion.listener {
            crate::http::spawn_listener(v6, app, shutdown_rx);
        }
        Ok(actual)
    }

    pub fn stop_listen(&self) -> Result<(), PairingError> {
        let mut g = self.slot();
        let tx = g.shutdown_tx.take();
        g.listening = false;
        g.companion = CompanionStatus::Stopped;
        g.local_addr = self.inner.preferred;
        drop(g);
        if let Some(tx) = tx {
            let _ = tx.send(true);
        }
        Ok(())
    }

    /// 这个句柄作用的那一页的 ctx（没 [`scoped`](Self::scoped) 就是根 / 第 1 页）。
    /// 全局服务（MCP、工具表……）从哪一页查都落回根。
    pub fn ctx(&self) -> &Context {
        self.scope
            .as_ref()
            .map_or(&self.inner.ctx, |page| &page.ctx)
    }

    /// 同一个网关，但 `ctx()` / 投影都指向 `page`。线程级的处理（斜杠命令一大串
    /// 函数都读 `gateway.ctx()`）在入口解析出页之后换成它，不用逐层传页。
    pub fn scoped(&self, page: crate::threads::Page) -> Self {
        Self {
            inner: self.inner.clone(),
            scope: Some(page),
        }
    }

    /// 这个句柄作用的那一页的分页身份。
    pub fn page_identity(&self) -> &str {
        self.scope
            .as_ref()
            .map_or(ROOT_IDENTITY, |page| page.identity.as_str())
    }

    pub fn inner(&self) -> Arc<GatewayInner> {
        self.inner.clone()
    }

    pub fn as_ref_service(&self) -> GatewayRef {
        GatewayRef::new(Arc::new(self.clone()) as Arc<dyn GatewayPort>)
    }

    pub fn request_pairing(
        &self,
        application: &str,
        origin: &str,
    ) -> Result<(String, u64), PairingError> {
        self.inner
            .pairing
            .lock()
            .unwrap()
            .request(application, origin)
    }

    pub fn poll_pairing(
        &self,
        id: &str,
        origin: &str,
    ) -> Result<(PairingStatus, u64), PairingError> {
        self.inner.pairing.lock().unwrap().poll(id, origin)
    }

    pub fn exchange_pairing(&self, id: &str, origin: &str) -> Result<IssuedTicket, PairingError> {
        self.inner.pairing.lock().unwrap().exchange(id, origin)
    }

    pub fn issue_ticket(
        &self,
        application: &str,
        origin: &str,
    ) -> Result<IssuedTicket, PairingError> {
        self.inner
            .pairing
            .lock()
            .unwrap()
            .issue_for_binding(application, origin)
    }

    /// 见 [`crate::pairing::PairingStore::issue_trusted`]：只给 `dock serve` 的
    /// stdout 控制通道用。
    pub fn issue_trusted_ticket(
        &self,
        application: &str,
        origin: &str,
    ) -> Result<IssuedTicket, PairingError> {
        self.inner
            .pairing
            .lock()
            .unwrap()
            .issue_trusted(application, origin)
    }

    pub fn authenticate(&self, ticket: &str, origin: &str) -> Result<IssuedTicket, PairingError> {
        self.inner
            .pairing
            .lock()
            .unwrap()
            .authenticate(ticket, origin)
    }

    /// `page` 那一页的投影里 `since_seq` 之后的事件。
    pub fn history_since(&self, page: &str, since_seq: u64) -> Vec<ProjectedEvent> {
        self.with_transcript(page, |t| t.history_since(since_seq))
    }

    pub fn latest_seq(&self, page: &str) -> u64 {
        self.with_transcript(page, |t| t.latest_seq())
    }

    /// 所有页的投影事件（带页身份）。
    pub fn subscribe_transcript(&self) -> tokio::sync::broadcast::Receiver<ProjectedEvent> {
        self.inner.events_tx.subscribe()
    }

    /// 这个句柄那一页的投影按它当前的会话重建（`thread/start` / `restore` 之后）。
    pub fn reset_transcript(&self) {
        let page = match &self.scope {
            Some(page) => Some(page.clone()),
            None => crate::threads::page_by_identity(self, ROOT_IDENTITY),
        };
        if let Some(page) = page {
            self.reset_page(&page);
        }
    }

    /// 按这一页当前的会话重建它的投影，`threadId` 跟着换成它现在的会话 id。
    pub fn reset_page(&self, page: &crate::threads::Page) {
        let thread_id = page.thread_id();
        let sessions = page.ctx.get::<Sessions>(SESSIONS);
        self.with_transcript(&page.identity, |t| {
            t.set_thread_id(thread_id);
            if let Some(sessions) = sessions {
                t.reset_from_sessions(&sessions);
            }
        });
    }

    /// 关页后丢掉它的投影。
    pub fn drop_page(&self, identity: &str) {
        self.inner.transcripts.lock().unwrap().remove(identity);
    }

    fn with_transcript<R>(&self, page: &str, f: impl FnOnce(&mut Transcript) -> R) -> R {
        with_transcript(&self.inner, page, None, f)
    }

    pub fn put_image(
        &self,
        id: String,
        mime: String,
        width: u32,
        height: u32,
        digest: String,
        data: Vec<u8>,
    ) -> Result<crate::image_store::StoredImage, crate::protocol::RpcError> {
        self.inner
            .images
            .lock()
            .unwrap()
            .put(id, mime, width, height, digest, data)
    }

    pub fn take_image(
        &self,
        id: &str,
        digest: &str,
    ) -> Result<crate::image_store::StoredImage, crate::protocol::RpcError> {
        self.inner.images.lock().unwrap().take(id, digest)
    }
}

impl GatewayPort for GatewayHandle {
    fn pairing_front(&self) -> Option<PairingPrompt> {
        self.inner.pairing.lock().unwrap().front()
    }

    fn pairing_pending(&self) -> Vec<PairingPrompt> {
        self.inner.pairing.lock().unwrap().pending()
    }

    fn pairing_bindings(&self) -> Vec<PairingBinding> {
        self.inner.pairing.lock().unwrap().bindings()
    }

    fn pairing_confirm(&self, id: &str) -> Result<(), PairingError> {
        self.inner.pairing.lock().unwrap().confirm(id)
    }

    fn pairing_deny(&self, id: &str) -> Result<(), PairingError> {
        self.inner.pairing.lock().unwrap().deny(id)
    }

    fn pairing_revoke(&self, origin: &str) -> Result<(), PairingError> {
        self.inner.pairing.lock().unwrap().revoke(origin)
    }

    fn local_addr(&self) -> SocketAddr {
        GatewayHandle::listen_addr(self)
    }

    fn companion_status(&self) -> CompanionStatus {
        GatewayHandle::companion_status(self)
    }

    fn is_listening(&self) -> bool {
        GatewayHandle::is_listening(self)
    }

    fn start_listen(&self) -> Result<SocketAddr, PairingError> {
        GatewayHandle::start_listen(self)
    }

    fn stop_listen(&self) -> Result<(), PairingError> {
        GatewayHandle::stop_listen(self)
    }
}

/// 取（没有就建）`page` 的投影。新建时 `threadId` 用 `thread_id`；没给就按页
/// 身份猜：第 1 页 `live`，其它页先用身份占位，`reset_page` 会换成会话 id。
fn with_transcript<R>(
    inner: &GatewayInner,
    page: &str,
    thread_id: Option<String>,
    f: impl FnOnce(&mut Transcript) -> R,
) -> R {
    let mut all = inner.transcripts.lock().unwrap();
    let t = all.entry(page.to_string()).or_insert_with(|| {
        let id = thread_id.unwrap_or_else(|| {
            if page == ROOT_IDENTITY {
                LIVE_THREAD_ID.to_string()
            } else {
                page.to_string()
            }
        });
        Transcript::for_page(page, id, inner.events_tx.clone())
    });
    f(t)
}

/// 所有开着的页（第 1 页在前）。监听器里用：拿不到 `GatewayHandle`，只有 inner。
fn pages(inner: &Arc<GatewayInner>) -> Vec<crate::threads::Page> {
    crate::threads::open_pages(&GatewayHandle {
        inner: inner.clone(),
        scope: None,
    })
}

fn listen_events(inner: &Arc<GatewayInner>) {
    // 会话事件按页投影：`session/event` 不分 realm、不带页身份，订带身份的那条。
    let for_session = inner.clone();
    let _ = inner
        .ctx
        .on(SESSION_PAGE_EVENT, move |tagged: &PageLogEvent| {
            let page = pages(&for_session)
                .into_iter()
                .find(|p| p.identity == *tagged.page);
            let event = &*tagged.event;
            let attachments = match (&page, event) {
                (Some(page), LogEvent::User(_)) => last_user_attachments(&page.ctx),
                _ => Vec::new(),
            };
            let thread_id = page.as_ref().map(|p| p.thread_id());
            with_transcript(&for_session, &tagged.page, thread_id, |t| {
                t.ingest_log_with(event.clone(), &attachments)
            });
        });
    let for_turn_end = inner.clone();
    let _ = inner.ctx.on(SESSION_TURN_END, move |end: &PageTurnEnd| {
        let thread_id = pages(&for_turn_end)
            .into_iter()
            .find(|p| p.identity == *end.page)
            .map(|p| p.thread_id());
        with_transcript(&for_turn_end, &end.page, thread_id, |t| t.turn_ended());
    });
    // 队列事件载荷是 `()`，也不知道是哪一页的：挨页对账，序号没变的页不动。
    let for_perm = inner.clone();
    let _ = inner.ctx.on(PERMISSION_EVENT, move |_: &()| {
        for page in pages(&for_perm) {
            let Some(perms) = page.ctx.get::<Permissions>(PERMISSIONS) else {
                continue;
            };
            let front = perms.front_seq().zip(perms.front());
            with_transcript(&for_perm, &page.identity, Some(page.thread_id()), |t| {
                t.sync_permission(front, perms.last_resolve())
            });
        }
    });
    let for_ask = inner.clone();
    let _ = inner.ctx.on(ASK_EVENT, move |_: &()| {
        for page in pages(&for_ask) {
            let Some(ask) = page.ctx.get::<Ask>(ASK) else {
                continue;
            };
            with_transcript(&for_ask, &page.identity, Some(page.thread_id()), |t| {
                t.sync_interaction(ask.front_seq(), &ask)
            });
        }
    });
    let for_plan = inner.clone();
    let _ = inner.ctx.on(PLAN_EVENT, move |_: &()| {
        for page in pages(&for_plan) {
            let Some(plan) = page.ctx.get::<PlanMode>(PLAN_MODE) else {
                continue;
            };
            let front = plan.front_seq().zip(plan.front());
            with_transcript(&for_plan, &page.identity, Some(page.thread_id()), |t| {
                t.sync_plan(front, plan.last_decision())
            });
        }
    });
    let for_elicit = inner.clone();
    let _ = inner.ctx.on(MCP_ELICIT_EVENT, move |_: &()| {
        let Some(mcp) = for_elicit.ctx.get::<Mcp>(MCP) else {
            return;
        };
        let elicitation = mcp.elicitation();
        for page in pages(&for_elicit) {
            let who = Some(page.identity.as_str());
            let front = elicitation
                .front_seq_for(who)
                .zip(elicitation.front_for(who));
            with_transcript(&for_elicit, &page.identity, Some(page.thread_id()), |t| {
                t.sync_elicit(front)
            });
        }
    });
}

fn last_user_attachments(ctx: &Context) -> Vec<serde_json::Value> {
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return Vec::new();
    };
    let rows = sessions.user_images();
    crate::transcript::attachment_values(rows.last().map(Vec::as_slice).unwrap_or(&[]))
}
