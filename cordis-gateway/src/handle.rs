//! Named `"gateway"` implementation. Pairing + transcript live here; HTTP
//! handlers live-look spine services off `Context`.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use cordis::Context;
use cordis_spine::{
    Ask, LogEvent, Mcp, Permissions, PlanMode, Sessions, ASK, ASK_EVENT, MCP, MCP_ELICIT_EVENT,
    PERMISSIONS, PERMISSION_EVENT, PLAN_EVENT, PLAN_MODE, SESSIONS, SESSION_EVENT,
};
use cordis_tui::{
    CompanionStatus, GatewayPort, GatewayRef, PairingBinding, PairingError, PairingPrompt,
};

use crate::image_store::ImageInputStore;
use crate::pairing::{IssuedTicket, PairingStatus, PairingStore};
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
    pub transcript: Mutex<Transcript>,
    pub images: Mutex<ImageInputStore>,
    preferred: SocketAddr,
    listen: Mutex<ListenSlot>,
}

#[derive(Clone)]
pub struct GatewayHandle {
    inner: Arc<GatewayInner>,
}

impl GatewayHandle {
    pub fn idle(ctx: Context, preferred: SocketAddr) -> Self {
        let inner = Arc::new(GatewayInner {
            pairing: Mutex::new(PairingStore::new(ctx.clone())),
            transcript: Mutex::new(Transcript::new()),
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
        Self { inner }
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

    pub fn ctx(&self) -> &Context {
        &self.inner.ctx
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

    pub fn history_since(&self, since_seq: u64) -> Vec<ProjectedEvent> {
        self.inner
            .transcript
            .lock()
            .unwrap()
            .history_since(since_seq)
    }

    pub fn latest_seq(&self) -> u64 {
        self.inner.transcript.lock().unwrap().latest_seq()
    }

    pub fn subscribe_transcript(&self) -> tokio::sync::broadcast::Receiver<ProjectedEvent> {
        self.inner.transcript.lock().unwrap().subscribe()
    }

    pub fn reset_transcript(&self) {
        if let Some(sessions) = self.inner.ctx.get::<cordis_spine::Sessions>(SESSIONS) {
            self.inner
                .transcript
                .lock()
                .unwrap()
                .reset_from_sessions(&sessions);
        } else {
            *self.inner.transcript.lock().unwrap() = Transcript::new();
        }
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

fn listen_events(inner: &Arc<GatewayInner>) {
    let for_session = inner.clone();
    let _ = inner.ctx.on(SESSION_EVENT, move |event: &LogEvent| {
        let attachments = if matches!(event, LogEvent::User(_)) {
            last_user_attachments(&for_session.ctx)
        } else {
            Vec::new()
        };
        for_session
            .transcript
            .lock()
            .unwrap()
            .ingest_log_with(event.clone(), &attachments);
    });
    let for_perm = inner.clone();
    let ctx_perm = inner.ctx.clone();
    let _ = inner.ctx.on(PERMISSION_EVENT, move |_: &()| {
        let Some(perms) = ctx_perm.get::<Permissions>(PERMISSIONS) else {
            return;
        };
        let mut t = for_perm.transcript.lock().unwrap();
        if let Some(front) = perms.front() {
            t.permission_requested(&front);
        } else if let Some(kind) = perms.last_resolve() {
            t.permission_resolved(kind);
        }
    });
    let for_ask = inner.clone();
    let ctx_ask = inner.ctx.clone();
    let _ = inner.ctx.on(ASK_EVENT, move |_: &()| {
        let Some(ask) = ctx_ask.get::<Ask>(ASK) else {
            return;
        };
        let mut t = for_ask.transcript.lock().unwrap();
        if ask.front().is_some() {
            t.interaction_requested(&ask);
        } else {
            t.interaction_resolved();
        }
    });
    let for_plan = inner.clone();
    let ctx_plan = inner.ctx.clone();
    let _ = inner.ctx.on(PLAN_EVENT, move |_: &()| {
        let Some(plan) = ctx_plan.get::<PlanMode>(PLAN_MODE) else {
            return;
        };
        let mut t = for_plan.transcript.lock().unwrap();
        if let Some(front) = plan.front() {
            t.plan_requested(&front);
        } else if let Some(decision) = plan.last_decision() {
            t.plan_resolved(decision);
        }
    });
    let for_elicit = inner.clone();
    let ctx_elicit = inner.ctx.clone();
    let _ = inner.ctx.on(MCP_ELICIT_EVENT, move |_: &()| {
        let Some(mcp) = ctx_elicit.get::<Mcp>(MCP) else {
            return;
        };
        let mut t = for_elicit.transcript.lock().unwrap();
        let page = ctx_elicit
            .get::<cordis_spine::Sessions>(cordis_spine::SESSIONS)
            .and_then(|s| s.ui_page());
        if let Some(front) = mcp.elicitation().front_for(page.as_deref()) {
            t.elicit_requested(&front);
        } else {
            t.elicit_resolved();
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
