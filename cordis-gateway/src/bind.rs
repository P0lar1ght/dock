//! Bind only loopback: `127.0.0.0/8` and `::1`. Never `0.0.0.0` / `::`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};

pub const DEFAULT_BIND: &str = "127.0.0.1:18991";

pub fn parse_bind(raw: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = raw
        .parse()
        .map_err(|e| format!("invalid bind address {raw}: {e}"))?;
    if addr.ip().is_loopback() {
        Ok(addr)
    } else {
        Err("gateway bind must be loopback (127.0.0.1 or ::1)".into())
    }
}

pub fn listen(raw: &str) -> Result<(TcpListener, SocketAddr), String> {
    let addr = parse_bind(raw)?;
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind {addr}: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set nonblocking: {e}"))?;
    let local = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?;
    if !local.ip().is_loopback() {
        return Err("gateway refused a non-loopback bind".into());
    }
    Ok((listener, local))
}

/// The other loopback family on the same port (`127.0.0.1` ↔ `::1`).
/// Fail-open: missing IPv6 (or a busy port) just skips the companion.
pub fn companion_listener(local: SocketAddr) -> Option<TcpListener> {
    let other = match local.ip() {
        IpAddr::V4(v4) if v4.is_loopback() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), local.port())
        }
        IpAddr::V6(v6) if v6.is_loopback() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), local.port())
        }
        _ => return None,
    };
    let listener = TcpListener::bind(other).ok()?;
    listener.set_nonblocking(true).ok()?;
    Some(listener)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback() {
        assert!(parse_bind("127.0.0.1:18991").is_ok());
        assert!(parse_bind("127.0.0.1:0").is_ok());
        assert!(parse_bind("[::1]:18991").is_ok());
        assert!(parse_bind("[::1]:0").is_ok());
        assert!(parse_bind("0.0.0.0:18991").is_err());
        assert!(parse_bind("[::]:18991").is_err());
        assert!(parse_bind("192.168.1.1:18991").is_err());
        let err = crate::protocol::json_error(
            "invalid_bind",
            "gateway bind must be loopback (127.0.0.1 or ::1)",
        );
        assert_eq!(err["error"]["code"], "invalid_bind");
    }
}
