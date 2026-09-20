//! Host-half fiber lifecycle over the `cordis-dynamic` group.
//! Copied from DSH `cordis-host-runner/lifecycle.ts`: settle a child under
//! `group.ctx.plugin`, dispose it before rethrowing any startup failure, and
//! treat unsatisfied `inject` as a legal pending fiber.

use cordis::{Fiber, FiberState, Plugin};

/// Await the group, start one child, and dispose it before rethrowing any
/// startup failure so a failed run never lingers. A valid unresolved inject
/// may remain pending.
pub async fn start_host_half(group: &Fiber, plugin: Plugin) -> Result<Fiber, String> {
    group.wait().await.map_err(display_err)?;
    let fiber = group.ctx().plugin(plugin, ()).map_err(display_err)?;
    match fiber.wait().await {
        Ok(()) => Ok(fiber),
        Err(error) => {
            let _ = fiber.dispose().await;
            let message = error.to_string();
            if message.contains("has been registered") || message.contains("already registered") {
                Err(format!(
                    "{message} — to REPLACE something an earlier dynamic package registered, first cordis_stop that package's pluginId \
(find it with cordis_inspect what:\"temporary\"), then run the new version."
                ))
            } else {
                Err(message)
            }
        }
    }
}

/// Inject names that do not resolve yet. A settled fiber that is not active
/// is waiting on exactly these.
pub fn missing_services(names: &[String], live: impl Fn(&str) -> bool) -> Vec<String> {
    names.iter().filter(|name| !live(name)).cloned().collect()
}

/// 状态只看内核的 fiber 状态。以前还拿「我们认不认得 inject 里的名字」当第二判据，
/// 于是 `inject: ["settings"]` 这种**已经 active** 的包被报成 waiting——内核按名字
/// 解析，认不认得是我们这边的事，不是插件的事。
pub fn host_status(fiber: Option<&Fiber>) -> &'static str {
    match fiber {
        None => "absent",
        Some(fiber) => match fiber.state() {
            FiberState::Active => "running",
            FiberState::Failed => "failed",
            FiberState::Disposed | FiberState::Unloading => "stopped",
            FiberState::Pending | FiberState::Loading => "waiting",
        },
    }
}

fn display_err(err: impl std::fmt::Display) -> String {
    err.to_string()
}
