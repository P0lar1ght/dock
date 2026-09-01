use cordis::{plugin, Disposable, Inject, Plugin};
use cordis_spine::{ASK, MCP, PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS, TURN};
use cordis_tui::{GATEWAY, SESSION_PORT};

use crate::bind;
use crate::handle::GatewayHandle;

pub const DEFAULT_BIND: &str = bind::DEFAULT_BIND;

/// Mount `"gateway"` without binding a port. TUI `/pair` calls `start_listen`.
pub fn gateway() -> Plugin {
    let bind = std::env::var("DOCK_GATEWAY_BIND").unwrap_or_else(|_| DEFAULT_BIND.into());
    gateway_idle(bind)
}

/// Mount without listening. `DOCK_GATEWAY_BIND` is only the preferred address.
pub fn gateway_idle(bind_addr: impl Into<String>) -> Plugin {
    mount(bind_addr, false)
}

/// Bind immediately (integration tests).
pub fn gateway_bind(bind_addr: impl Into<String>) -> Plugin {
    mount(bind_addr, true)
}

fn mount(bind_addr: impl Into<String>, auto_listen: bool) -> Plugin {
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
            let preferred = bind::parse_bind(&bind_addr).map_err(cordis::Error::message)?;
            let handle = GatewayHandle::idle(ctx.clone(), preferred);
            handle.reset_transcript();
            if auto_listen {
                handle
                    .start_listen()
                    .map_err(|e| cordis::Error::message(e.to_string()))?;
            }
            let h = handle.clone();
            ctx.effect("gateway-http", move |scope| {
                scope.own(Disposable::from_fn(move || {
                    let _ = h.stop_listen();
                }));
                Ok(())
            })?;
            Ok(Some(ctx.provide(GATEWAY, handle.as_ref_service())?))
        },
    )
}
