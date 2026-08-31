use cordis::{plugin, Disposable, Inject, Plugin};
use cordis_spine::{ASK, MCP, PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS, TURN};
use cordis_tui::{SESSION_PORT, GATEWAY};

use crate::bind;
use crate::handle::GatewayHandle;
use crate::http;

pub const DEFAULT_BIND: &str = bind::DEFAULT_BIND;

pub fn gateway() -> Plugin {
    let bind = std::env::var("DOCK_GATEWAY_BIND").unwrap_or_else(|_| DEFAULT_BIND.into());
    gateway_bind(bind)
}

pub fn gateway_bind(bind_addr: impl Into<String>) -> Plugin {
    let bind_addr = bind_addr.into();
    plugin(
        "gateway",
        Inject::from([
            SESSIONS,
            SESSION_PORT,
            PERMISSIONS,
            ASK,
            PLAN_MODE,
            MCP,
            TURN,
            SETTINGS,
        ]),
        move |ctx, _: &()| {
            let (listener, local_addr) = bind::listen(&bind_addr)
                .map_err(|e| cordis::Error::message(e))?;
            let handle = GatewayHandle::new(ctx.clone(), local_addr);
            handle.reset_transcript();
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let app = http::router(handle.clone());
            tokio::spawn(async move {
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(l) => l,
                    Err(_) => return,
                };
                let _ = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = shutdown_rx.await;
                    })
                    .await;
            });
            ctx.effect("gateway-http", move |scope| {
                scope.own(Disposable::from_fn(move || {
                    let _ = shutdown_tx.send(());
                }));
                Ok(())
            })?;
            Ok(Some(ctx.provide(GATEWAY, handle.as_ref_service())?))
        },
    )
}
