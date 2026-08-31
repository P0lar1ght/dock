//! Bootstrap HTTP: pairing request / poll / exchange / tickets. Loopback only.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::handle::GatewayHandle;
use crate::protocol::{self, http_error};
use crate::ws;

#[derive(Clone)]
pub struct AppState {
    pub gateway: GatewayHandle,
}

pub fn router(gateway: GatewayHandle) -> Router {
    Router::new()
        .route("/v1/pairing/requests", post(create_pairing))
        .route("/v1/pairing/requests/{id}", get(poll_pairing))
        .route("/v1/pairing/exchanges", post(exchange_pairing))
        .route("/v1/connection/tickets", post(connection_ticket))
        .route(protocol::WS_PATH, get(ws::upgrade))
        .fallback(not_found)
        .layer(axum::middleware::from_fn(cors))
        .with_state(AppState { gateway })
}

async fn cors(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    // Reflect Origin on purpose: any local page (vite, file-like http, another
    // app) may call loopback bootstrap. Authorization is Origin pairing +
    // loopback bind, not a CORS allowlist. A malicious page can hit the API
    // but cannot get a ticket without TUI approval for that Origin.
    let origin = req.headers().get(header::ORIGIN).cloned();
    let is_options = req.method() == Method::OPTIONS;
    let mut res = if is_options {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(req).await
    };
    if let Some(origin) = origin {
        res.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        res.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            header::HeaderValue::from_static("content-type"),
        );
        res.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            header::HeaderValue::from_static("GET,POST,OPTIONS"),
        );
        res.headers_mut()
            .insert(header::VARY, header::HeaderValue::from_static("Origin"));
    }
    res
}

#[derive(Deserialize)]
struct ApplicationBody {
    application: String,
}

#[derive(Deserialize)]
struct ExchangeBody {
    #[serde(rename = "pairingRequestId")]
    pairing_request_id: String,
}

async fn create_pairing(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ApplicationBody>,
) -> Response {
    let origin = origin_header(&headers);
    match state.gateway.request_pairing(&body.application, &origin) {
        Ok((id, expires_at)) => (
            StatusCode::OK,
            Json(json!({ "pairingRequestId": id, "expiresAt": expires_at })),
        )
            .into_response(),
        Err(e) => pairing_http_error(e),
    }
}

async fn poll_pairing(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let origin = origin_header(&headers);
    match state.gateway.poll_pairing(&id, &origin) {
        Ok((status, expires_at)) => {
            // Ticket plaintext only leaves via POST /v1/pairing/exchanges.
            let body = json!({
                "status": status.as_str(),
                "expiresAt": expires_at
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => pairing_http_error(e),
    }
}

async fn exchange_pairing(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ExchangeBody>,
) -> Response {
    let origin = origin_header(&headers);
    match state
        .gateway
        .exchange_pairing(&body.pairing_request_id, &origin)
    {
        Ok(ticket) => ticket_response(ticket.token, ticket.expires_unix_ms),
        Err(e) => pairing_http_error(e),
    }
}

async fn connection_ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ApplicationBody>,
) -> Response {
    let origin = origin_header(&headers);
    match state.gateway.issue_ticket(&body.application, &origin) {
        Ok(ticket) => ticket_response(ticket.token, ticket.expires_unix_ms),
        Err(e) => pairing_http_error(e),
    }
}

fn ticket_response(ticket: String, expires_at: u64) -> Response {
    (
        StatusCode::OK,
        Json(json!({ "ticket": ticket, "expiresAt": expires_at })),
    )
        .into_response()
}

fn origin_header(headers: &HeaderMap) -> String {
    headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

fn pairing_http_error(e: cordis_tui::PairingError) -> Response {
    let status = match e.code {
        "origin_required" | "invalid_application" | "invalid_params" => StatusCode::BAD_REQUEST,
        "pairing_required" | "origin_mismatch" | "denied" => StatusCode::FORBIDDEN,
        "not_found" => StatusCode::NOT_FOUND,
        "expired" => StatusCode::GONE,
        "pending" | "not_pending" | "not_ready" => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(http_error(e.code, e.message))).into_response()
}

async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(http_error("not_found", "no such endpoint")),
    )
        .into_response()
}
