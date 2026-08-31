//! Named `"gateway"` implementation. Pairing + transcript live here; HTTP
//! handlers live-look spine services off `Context`.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use cordis::Context;
use cordis_spine::{
    Ask, LogEvent, Mcp, Permissions, PlanMode, ASK, ASK_EVENT, MCP, MCP_ELICIT_EVENT, PERMISSIONS,
    PERMISSION_EVENT, PLAN_EVENT, PLAN_MODE, SESSIONS, SESSION_EVENT,
};
use cordis_tui::{GatewayPort, GatewayRef, PairingBinding, PairingError, PairingPrompt};

use crate::pairing::{IssuedTicket, PairingStatus, PairingStore};
use crate::transcript::{ProjectedEvent, Transcript};

pub struct GatewayInner {
    pub ctx: Context,
    pub pairing: Mutex<PairingStore>,
    pub transcript: Mutex<Transcript>,
    pub local_addr: SocketAddr,
}

#[derive(Clone)]
pub struct GatewayHandle {
    inner: Arc<GatewayInner>,
}

impl GatewayHandle {
    pub fn new(ctx: Context, local_addr: SocketAddr) -> Self {
        let inner = Arc::new(GatewayInner {
            pairing: Mutex::new(PairingStore::new(ctx.clone())),
            transcript: Mutex::new(Transcript::new()),
            local_addr,
            ctx: ctx.clone(),
        });
        listen_events(&inner);
        Self { inner }
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
        self.inner.pairing.lock().unwrap().request(application, origin)
    }

    pub fn poll_pairing(
        &self,
        id: &str,
        origin: &str,
    ) -> Result<(PairingStatus, Option<IssuedTicket>, u64), PairingError> {
        self.inner.pairing.lock().unwrap().poll(id, origin)
    }

    pub fn exchange_pairing(
        &self,
        id: &str,
        origin: &str,
    ) -> Result<IssuedTicket, PairingError> {
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

    pub fn authenticate(
        &self,
        ticket: &str,
        origin: &str,
    ) -> Result<IssuedTicket, PairingError> {
        self.inner
            .pairing
            .lock()
            .unwrap()
            .authenticate(ticket, origin)
    }

    pub fn history_since(&self, since_seq: u64) -> Vec<ProjectedEvent> {
        self.inner.transcript.lock().unwrap().history_since(since_seq)
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
        self.inner.local_addr
    }
}

fn listen_events(inner: &Arc<GatewayInner>) {
    let for_session = inner.clone();
    let _ = inner.ctx.on(SESSION_EVENT, move |event: &LogEvent| {
        for_session
            .transcript
            .lock()
            .unwrap()
            .ingest_log(event.clone());
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
        if let Some(front) = mcp.elicitation().front() {
            t.elicit_requested(&front);
        } else {
            t.elicit_resolved();
        }
    });
}
