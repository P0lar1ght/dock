use cordis::{plugin, Context, Disposable, Inject, Plugin};
use cordis_spine::{ASK, MCP, PERMISSIONS, PLAN_MODE, SESSIONS, SETTINGS, TURN};
use cordis_spine::{GATEWAY, SESSION_PORT};

use crate::bind;
use crate::handle::GatewayHandle;
use crate::methods::{GatewayMethods, GATEWAY_METHODS};
use crate::pairing::{normalize_application, require_origin};
use crate::serve::{ServeConfig, ServeControl, GATEWAY_SERVE};

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

/// 无头网关（`dock serve`）：挂载即监听，另外 provide `"gateway.serve"`
/// （[`ServeControl`]），由组合根拿它跑 stdin / stdout 控制通道。仍只绑回环。
/// `"gateway"`（`GatewayRef`）照常 provide，别的插件看到的是同一个网关。
pub fn gateway_serve(bind_addr: impl Into<String>) -> Plugin {
    let bind_addr = bind_addr.into();
    plugin("gateway", inject(), move |ctx, config: &ServeConfig| {
        // 配置错了在启动时就报，别等父进程拿着一张签不出来的 ticket 干等。
        normalize_application(&config.application)
            .map_err(|e| cordis::Error::message(e.to_string()))?;
        require_origin(&config.origin).map_err(|e| cordis::Error::message(e.to_string()))?;
        let (handle, provided) = mount_handle(ctx, &bind_addr, true)?;
        let control = ServeControl::new(handle.clone(), config.clone(), handle.listen_addr());
        let serve = ctx.provide(GATEWAY_SERVE, control)?;
        ctx.effect("gateway-serve", move |scope| {
            scope.own(serve);
            Ok(())
        })?;
        Ok(Some(provided))
    })
}

/// 远程网关（`dock serve --remote`）：挂载即监听，只认设备令牌，端口不顺延。仍只绑回环，
/// TLS 与对外暴露交给前面的反向代理（Caddy / nginx）或 Tailscale。
pub fn gateway_remote(bind_addr: impl Into<String>) -> Plugin {
    let bind_addr = bind_addr.into();
    plugin("gateway", inject(), move |ctx, _: &()| {
        let preferred = bind::parse_bind(&bind_addr).map_err(cordis::Error::message)?;
        let handle = GatewayHandle::idle_remote(ctx.clone(), preferred);
        let provided = finish_mount(ctx, handle, true)?.1;
        Ok(Some(provided))
    })
}

fn inject() -> Inject {
    Inject::from([
        SESSIONS,
        SESSION_PORT,
        PERMISSIONS,
        ASK,
        PLAN_MODE,
        MCP,
        TURN,
        SETTINGS,
    ])
}

fn mount(bind_addr: impl Into<String>, auto_listen: bool) -> Plugin {
    let bind_addr = bind_addr.into();
    plugin("gateway", inject(), move |ctx, _: &()| {
        let (_, provided) = mount_handle(ctx, &bind_addr, auto_listen)?;
        Ok(Some(provided))
    })
}

/// 建 `GatewayHandle`、按需监听、dispose 时停监听，并 provide `"gateway"`。
fn mount_handle(
    ctx: &Context,
    bind_addr: &str,
    auto_listen: bool,
) -> Result<(GatewayHandle, Disposable), cordis::Error> {
    let preferred = bind::parse_bind(bind_addr).map_err(cordis::Error::message)?;
    finish_mount(
        ctx,
        GatewayHandle::idle(ctx.clone(), preferred),
        auto_listen,
    )
}

fn finish_mount(
    ctx: &Context,
    handle: GatewayHandle,
    auto_listen: bool,
) -> Result<(GatewayHandle, Disposable), cordis::Error> {
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
    // 功能方法表：vcs、定时任务……各自的插件挂载时往里登记（见 [`features`]）。
    ctx.provide(GATEWAY_METHODS, GatewayMethods::default())?;
    let provided = ctx.provide(GATEWAY, handle.as_ref_service())?;
    Ok((handle, provided))
}

/// dock.1 的功能插件，按挂载顺序。组合根挂完网关后逐个挂；想去掉或换掉哪一块，
/// 就在组合根换，网关本身不用改。
pub fn features() -> Vec<Plugin> {
    vec![
        crate::handlers::settings::gateway_settings(),
        crate::handlers::preset::gateway_presets(),
        crate::handlers::fs::gateway_fs(),
        crate::handlers::canvas::gateway_canvas(),
        crate::handlers::vcs::gateway_vcs(),
        crate::handlers::schedule::gateway_schedule(),
    ]
}
