//! Bind only IPv4 loopback.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};

pub const DEFAULT_BIND: &str = "127.0.0.1:18991";

pub fn parse_bind(raw: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = raw
        .parse()
        .map_err(|e| format!("invalid bind address {raw}: {e}"))?;
    match addr.ip() {
        IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST => Ok(addr),
        _ => Err("gateway bind must be 127.0.0.1".into()),
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
    if local.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err("gateway refused a non-loopback bind".into());
    }
    Ok((listener, local))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ipv4_loopback() {
        assert!(parse_bind("127.0.0.1:18991").is_ok());
        assert!(parse_bind("127.0.0.1:0").is_ok());
        assert!(parse_bind("0.0.0.0:18991").is_err());
        assert!(parse_bind("[::1]:18991").is_err());
        assert!(parse_bind("192.168.1.1:18991").is_err());
        let err = crate::protocol::json_error("invalid_bind", "gateway bind must be 127.0.0.1");
        assert_eq!(err["error"]["code"], "invalid_bind");
    }
}
