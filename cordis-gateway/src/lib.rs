//! Loopback HTTP/WS Cordis plugin. JSON-RPC `dock.1` over Origin pairing.
//!
//! Named service `"gateway"` (`GatewayRef`). Plugin mounts idle; HTTP starts
//! on `start_listen` (`gateway_bind` for tests). Dispose stops the server.
//! Call sites live-look spine services — do not capture `Arc<Sessions>` in
//! the HTTP lifetime.

mod bind;
mod handle;
mod http;
mod image_store;
mod pairing;
mod plugin;
mod protocol;
mod rpc;
mod transcript;
mod ws;

pub mod handlers;

pub use handle::GatewayHandle;
pub use plugin::{gateway, gateway_bind, gateway_idle, DEFAULT_BIND};
pub use protocol::{CAPABILITIES, LIVE_THREAD_ID, PROTOCOL_VERSION};

pub const GATEWAY: &str = cordis_tui::GATEWAY;
pub const GATEWAY_PAIRING: &str = cordis_tui::GATEWAY_PAIRING;
