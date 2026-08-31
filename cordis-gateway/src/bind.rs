//! Bind only loopback: `127.0.0.0/8` and `::1`. Never `0.0.0.0` / `::`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};

use cordis_tui::CompanionStatus;

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

pub struct CompanionListener {
    pub status: CompanionStatus,
    pub listener: Option<TcpListener>,
}

/// The other loopback family on the same port (`127.0.0.1` ↔ `::1`).
/// Failure is reported on `status` — never silent.
pub fn companion_listener(local: SocketAddr) -> CompanionListener {
    let other = match local.ip() {
        IpAddr::V4(v4) if v4.is_loopback() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), local.port())
        }
        IpAddr::V6(v6) if v6.is_loopback() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), local.port())
        }
        _ => {
            return CompanionListener {
                status: CompanionStatus::Failed {
                    addr: local,
                    error: "primary bind is not loopback".into(),
                },
                listener: None,
            };
        }
    };
    match TcpListener::bind(other).and_then(|listener| {
        listener.set_nonblocking(true)?;
        Ok(listener)
    }) {
        Ok(listener) => {
            let addr = listener.local_addr().unwrap_or(other);
            CompanionListener {
                status: CompanionStatus::Listening(addr),
                listener: Some(listener),
            }
        }
        Err(e) => CompanionListener {
            status: CompanionStatus::Failed {
                addr: other,
                error: e.to_string(),
            },
            listener: None,
        },
    }
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

    #[test]
    fn companion_binds_ipv6_localhost() {
        let (_primary, addr) = listen("127.0.0.1:0").unwrap();
        let companion = companion_listener(addr);
        match companion.status {
            CompanionStatus::Listening(v6) => {
                assert!(v6.is_ipv6(), "{v6}");
                assert_eq!(v6.port(), addr.port());
                assert!(companion.listener.is_some());
            }
            CompanionStatus::Failed { addr, error } => {
                panic!("companion {addr} failed (IPv6/localhost must bind in tests): {error}");
            }
        }
    }

    #[test]
    fn companion_reports_busy_port() {
        let held = TcpListener::bind("[::1]:0").expect("test host must allow ::1");
        let port = held.local_addr().unwrap().port();
        let (_v4, addr) = listen(&format!("127.0.0.1:{port}")).unwrap();
        let companion = companion_listener(addr);
        match companion.status {
            CompanionStatus::Failed {
                addr: failed,
                error,
            } => {
                assert_eq!(failed.port(), port);
                assert!(!error.is_empty(), "{error}");
                assert!(companion.listener.is_none());
            }
            CompanionStatus::Listening(v6) => {
                panic!("companion silently bound {v6} while [::1]:{port} is held");
            }
        }
    }
}
