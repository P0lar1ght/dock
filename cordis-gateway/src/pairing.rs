//! Application+Origin pairing. Approval tokens never leave the process.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use cordis::Context;
use cordis_tui::{PairingBinding, PairingError, PairingPrompt, GATEWAY_PAIRING};

pub const PAIRING_TTL: Duration = Duration::from_secs(5 * 60);
pub const TICKET_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

impl PairingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Expired => "expired",
        }
    }
}

#[derive(Clone, Debug)]
struct PendingRequest {
    id: String,
    application: String,
    origin: String,
    created_at: SystemTime,
    expires_at: Instant,
    expires_unix_ms: u64,
    status: PairingStatus,
    ticket: Option<IssuedTicket>,
}

#[derive(Clone, Debug)]
pub struct IssuedTicket {
    pub token: String,
    pub digest: String,
    pub origin: String,
    pub application: String,
    pub expires_at: Instant,
    pub expires_unix_ms: u64,
}

#[derive(Clone, Debug)]
struct Binding {
    application: String,
    origin: String,
    bound_at: SystemTime,
}

pub struct PairingStore {
    ctx: Context,
    pending: VecDeque<PendingRequest>,
    bindings: Vec<Binding>,
    tickets: HashMap<String, IssuedTicket>,
}

impl PairingStore {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            pending: VecDeque::new(),
            bindings: Vec::new(),
            tickets: HashMap::new(),
        }
    }

    pub fn request(
        &mut self,
        application: &str,
        origin: &str,
    ) -> Result<(String, u64), PairingError> {
        let application = normalize_application(application)?;
        let origin = require_origin(origin)?;
        self.gc();
        let id = uuid::Uuid::new_v4().to_string();
        let now = Instant::now();
        let created_at = SystemTime::now();
        let expires_unix_ms = unix_ms(created_at) + PAIRING_TTL.as_millis() as u64;
        self.pending.push_back(PendingRequest {
            id: id.clone(),
            application,
            origin,
            created_at,
            expires_at: now + PAIRING_TTL,
            expires_unix_ms,
            status: PairingStatus::Pending,
            ticket: None,
        });
        self.emit();
        Ok((id, expires_unix_ms))
    }

    pub fn poll(&mut self, id: &str, origin: &str) -> Result<(PairingStatus, u64), PairingError> {
        let origin = require_origin(origin)?;
        self.gc();
        let req = self
            .pending
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| PairingError::new("not_found", "pairing request not found"))?;
        if req.origin != origin {
            return Err(PairingError::new(
                "origin_mismatch",
                "Origin does not match this pairing request",
            ));
        }
        Ok((req.status, req.expires_unix_ms))
    }

    pub fn exchange(&mut self, id: &str, origin: &str) -> Result<IssuedTicket, PairingError> {
        let origin = require_origin(origin)?;
        self.gc();
        let req = self
            .pending
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| PairingError::new("not_found", "pairing request not found"))?;
        if req.origin != origin {
            return Err(PairingError::new(
                "origin_mismatch",
                "Origin does not match this pairing request",
            ));
        }
        match req.status {
            PairingStatus::Approved => req.ticket.clone().ok_or_else(|| {
                PairingError::new("not_ready", "pairing is approved but ticket is missing")
            }),
            PairingStatus::Pending => Err(PairingError::new(
                "pending",
                "pairing request is still waiting for TUI confirmation",
            )),
            PairingStatus::Denied => Err(PairingError::new("denied", "pairing request was denied")),
            PairingStatus::Expired => Err(PairingError::new("expired", "pairing request expired")),
        }
    }

    pub fn issue_for_binding(
        &mut self,
        application: &str,
        origin: &str,
    ) -> Result<IssuedTicket, PairingError> {
        let application = normalize_application(application)?;
        let origin = require_origin(origin)?;
        self.gc();
        let bound = self
            .bindings
            .iter()
            .any(|b| b.application == application && b.origin == origin);
        if !bound {
            return Err(PairingError::new(
                "pairing_required",
                "this origin is not bound; create a pairing request",
            ));
        }
        let ticket = mint_ticket(&application, &origin);
        self.tickets.insert(ticket.digest.clone(), ticket.clone());
        Ok(ticket)
    }

    pub fn authenticate(
        &mut self,
        token: &str,
        origin: &str,
    ) -> Result<IssuedTicket, PairingError> {
        let origin = require_origin(origin)?;
        self.gc();
        let digest = ticket_digest(token);
        let ticket = self
            .tickets
            .get(&digest)
            .cloned()
            .ok_or_else(|| PairingError::new("invalid_ticket", "ticket is not valid"))?;
        if ticket.origin != origin {
            return Err(PairingError::new(
                "origin_mismatch",
                "ticket Origin does not match this connection",
            ));
        }
        if ticket.expires_at <= Instant::now() {
            self.tickets.remove(&digest);
            return Err(PairingError::new("expired", "ticket expired"));
        }
        Ok(ticket)
    }

    pub fn front(&mut self) -> Option<PairingPrompt> {
        self.gc();
        self.pending
            .iter()
            .find(|p| p.status == PairingStatus::Pending)
            .map(to_prompt)
    }

    pub fn pending(&mut self) -> Vec<PairingPrompt> {
        self.gc();
        self.pending
            .iter()
            .filter(|p| p.status == PairingStatus::Pending)
            .map(to_prompt)
            .collect()
    }

    pub fn bindings(&self) -> Vec<PairingBinding> {
        self.bindings
            .iter()
            .map(|b| PairingBinding {
                application: b.application.clone(),
                origin: b.origin.clone(),
                bound_at: b.bound_at,
            })
            .collect()
    }

    pub fn confirm(&mut self, id: &str) -> Result<(), PairingError> {
        self.gc();
        let req = self
            .pending
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| PairingError::new("not_found", "pairing request not found"))?;
        if req.status != PairingStatus::Pending {
            return Err(PairingError::new(
                "not_pending",
                "pairing request is not waiting for confirmation",
            ));
        }
        let ticket = mint_ticket(&req.application, &req.origin);
        req.status = PairingStatus::Approved;
        req.ticket = Some(ticket.clone());
        let application = req.application.clone();
        let origin = req.origin.clone();
        self.tickets.insert(ticket.digest.clone(), ticket);
        if !self
            .bindings
            .iter()
            .any(|b| b.application == application && b.origin == origin)
        {
            self.bindings.push(Binding {
                application,
                origin,
                bound_at: SystemTime::now(),
            });
        }
        self.emit();
        Ok(())
    }

    pub fn deny(&mut self, id: &str) -> Result<(), PairingError> {
        self.gc();
        let req = self
            .pending
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| PairingError::new("not_found", "pairing request not found"))?;
        if req.status != PairingStatus::Pending {
            return Err(PairingError::new(
                "not_pending",
                "pairing request is not waiting for confirmation",
            ));
        }
        req.status = PairingStatus::Denied;
        self.emit();
        Ok(())
    }

    pub fn revoke(&mut self, origin: &str) -> Result<(), PairingError> {
        let origin = origin.trim();
        if origin.is_empty() {
            return Err(PairingError::new("invalid_params", "origin is required"));
        }
        let before = self.bindings.len();
        self.bindings.retain(|b| b.origin != origin);
        self.tickets.retain(|_, t| t.origin != origin);
        if self.bindings.len() == before {
            return Err(PairingError::new("not_found", "no binding for that origin"));
        }
        self.emit();
        Ok(())
    }

    fn gc(&mut self) {
        let now = Instant::now();
        for req in &mut self.pending {
            if req.status == PairingStatus::Pending && req.expires_at <= now {
                req.status = PairingStatus::Expired;
            }
        }
        self.tickets.retain(|_, t| t.expires_at > now);
    }

    fn emit(&self) {
        self.ctx.emit(GATEWAY_PAIRING, ());
    }
}

fn to_prompt(req: &PendingRequest) -> PairingPrompt {
    PairingPrompt {
        id: req.id.clone(),
        application: req.application.clone(),
        origin: req.origin.clone(),
        created_at: req.created_at,
        expires_at: UNIX_EPOCH + Duration::from_millis(req.expires_unix_ms),
    }
}

fn mint_ticket(application: &str, origin: &str) -> IssuedTicket {
    let mut bytes = [0u8; 32];
    let _ = getrandom::getrandom(&mut bytes);
    let token = hex_encode(&bytes);
    let digest = ticket_digest(&token);
    let now = SystemTime::now();
    IssuedTicket {
        token,
        digest,
        origin: origin.to_string(),
        application: application.to_string(),
        expires_at: Instant::now() + TICKET_TTL,
        expires_unix_ms: unix_ms(now) + TICKET_TTL.as_millis() as u64,
    }
}

fn ticket_digest(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex_encode(&h.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn normalize_application(value: &str) -> Result<String, PairingError> {
    let application = value.trim().to_ascii_lowercase();
    if application.is_empty()
        || application.len() > 64
        || !application
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        || !application.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
        })
    {
        return Err(PairingError::new(
            "invalid_application",
            "application must be a stable lowercase identifier",
        ));
    }
    Ok(application)
}

pub fn require_origin(origin: &str) -> Result<String, PairingError> {
    let origin = origin.trim();
    if origin.is_empty() {
        return Err(PairingError::new(
            "origin_required",
            "browser Origin header is required",
        ));
    }
    Ok(origin.to_string())
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}
