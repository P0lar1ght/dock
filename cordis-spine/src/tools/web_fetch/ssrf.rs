//! SSRF (Server-Side Request Forgery) protection for `web_fetch`.
//!
//! Policy:
//! - Non-public addresses (loopback, RFC 1918, link-local, CGNAT, TEST-NET,
//!   multicast, etc.) are blocked by default.
//! - Local access is opt-in via tool params (`WebFetchParams::allow_local`,
//!   set from `[toolset.web_fetch] allow_local`). Even when enabled, only
//!   **explicit** loopback hosts are allowed (`localhost`, `127.0.0.0/8`
//!   literals, `::1`). A public hostname that resolves to loopback/private
//!   stays blocked.
//! - A configured forward proxy (`WebFetchParams::proxy_endpoint`) switches the
//!   DNS pre-check off for plain hostnames — see [`skips_local_dns`]. The
//!   address the request actually reaches is resolved by the proxy, so the local
//!   lookup is not evidence. IP literals and explicit local hostnames are still
//!   judged locally.
//!
//! Reference: [IANA IPv4 Special-Purpose Address Registry](https://www.iana.org/assignments/iana-ipv4-special-registry/)

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::Url;

use super::error::WebFetchError;

/// Hostnames/IP literals that may reach loopback when local binding is
/// enabled. Public names that *resolve* to loopback are not included — that
/// closes DNS rebinding through a non-local hostname.
pub(crate) fn is_explicit_local_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(&host);
    // Drop IPv6 zone id if present (`fe80::1%lo0`).
    let host = host.split('%').next().unwrap_or(host);

    if host == "localhost" {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return ip.is_loopback();
    }
    false
}

/// Returns `true` if an IP is not globally routable and should be treated as
/// local/private for SSRF.
pub(crate) fn is_non_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_non_public_ipv4(v4),
        IpAddr::V6(v6) => is_non_public_ipv6(v6),
    }
}

fn is_non_public_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        // "This network" (RFC 1122) 0.0.0.0/8
        || ipv4_in_cidr(ip, [0, 0, 0, 0], 8)
        // CGNAT (RFC 6598) 100.64.0.0/10 — cloud metadata-ish
        || ipv4_in_cidr(ip, [100, 64, 0, 0], 10)
        // IETF Protocol Assignments (RFC 6890) 192.0.0.0/24
        || ipv4_in_cidr(ip, [192, 0, 0, 0], 24)
        // TEST-NET-1 (RFC 5737)
        || ipv4_in_cidr(ip, [192, 0, 2, 0], 24)
        // Benchmarking (RFC 2544)
        || ipv4_in_cidr(ip, [198, 18, 0, 0], 15)
        // TEST-NET-2 / TEST-NET-3
        || ipv4_in_cidr(ip, [198, 51, 100, 0], 24)
        || ipv4_in_cidr(ip, [203, 0, 113, 0], 24)
        // Reserved (RFC 6890) 240.0.0.0/4
        || ipv4_in_cidr(ip, [240, 0, 0, 0], 4)
}

fn ipv4_in_cidr(ip: Ipv4Addr, base: [u8; 4], prefix: u8) -> bool {
    let ip = u32::from(ip);
    let base = u32::from(Ipv4Addr::from(base));
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    (ip & mask) == (base & mask)
}

fn is_non_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_non_public_ipv4(v4);
    }
    // Anything not globally routable: loopback, ULA, link-local, unspecified, multicast.
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
}

/// Loopback including IPv4-mapped forms (`::ffff:127.0.0.1`).
///
/// `IpAddr::is_loopback` is false for mapped addresses even when the embedded
/// v4 is loopback, so local opt-in must use this helper.
fn is_loopback_addr(ip: IpAddr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
        IpAddr::V4(_) => false,
    }
}

/// Whether a resolved address is blocked for this request host.
///
/// Dual-gate: even with local binding allowed, only explicit loopback hosts
/// may use loopback IPs; private/link-local never open via this flag.
pub(crate) fn is_blocked_for_host(ip: IpAddr, host: &str, allow_local: bool) -> bool {
    if !is_non_public_ip(ip) {
        return false;
    }
    if allow_local && is_loopback_addr(ip) && is_explicit_local_host(host) {
        return false;
    }
    true
}

/// 配了转发代理时，这个 host 要不要跳过**本地** DNS 预检。
///
/// 为什么跳过：出口是代理，连接目的地址由代理侧解析，本地那次 `lookup_host`
/// 的结果跟这次请求会连到哪里**没有关系**。fake-ip / 分流 DNS 下它返回的是保留
/// 段假址（本机实测 `example.com` → `198.18.0.164`，落在下面的 RFC 2544 段里），
/// 拿它当证据就会把每一次取页面都判成 SSRF。
///
/// 两处仍然不跳过，它们不依赖本地解析：
/// - **IP 字面量**——URL 自己点名了地址，代理救不了它
/// - **显式本地主机名**（`localhost` / `127.0.0.0/8` / `::1`）——仍由 `allow_local` 决定
///
/// 代价说清楚：代理若是本机在跑（Clash 那种），它自己会去解析并连内网，也就是
/// 「代理成为信任边界」。这是配置代理这一动作隐含的信任转移，不是这里放出来的。
pub(crate) fn skips_local_dns(host: &str, via_proxy: bool) -> bool {
    via_proxy && host.parse::<IpAddr>().is_err() && !is_explicit_local_host(host)
}

/// Resolve hostname via DNS and verify none of the resolved addresses are
/// blocked under the SSRF policy.
///
/// `allow_local` comes from tool config (`WebFetchParams::allow_local`); it is
/// not read from the environment here so the agent cannot flip the policy.
/// `via_proxy` likewise comes from the resolved tool params
/// (`WebFetchParams::via_proxy`), never from the request.
pub(crate) async fn check_ssrf(
    url: &Url,
    allow_local: bool,
    via_proxy: bool,
) -> Result<(), WebFetchError> {
    let host = url
        .host_str()
        .ok_or_else(|| WebFetchError::SingleLabelHost {
            host: String::new(),
        })?;

    // If the host is already a literal IP, check it directly.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_for_host(ip, host, allow_local) {
            return Err(WebFetchError::SsrfBlocked {
                host: host.to_string(),
                ip,
            });
        }
        return Ok(());
    }

    if skips_local_dns(host, via_proxy) {
        return Ok(());
    }

    // DNS resolution.
    let port = url.port_or_known_default().unwrap_or(443);
    let addr_str = format!("{host}:{port}");
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&addr_str)
        .await
        .map_err(|e| WebFetchError::DnsResolution {
            host: host.to_string(),
            source: e,
        })?
        .collect();

    if addrs.is_empty() {
        return Err(WebFetchError::DnsEmpty(host.to_string()));
    }

    // Any non-public address blocks the request. When allow_local is on,
    // only *explicit* loopback hosts may use loopback IPs — a rebinding name
    // that resolves to 127.0.0.1 stays blocked.
    addrs
        .iter()
        .find(|addr| is_blocked_for_host(addr.ip(), host, allow_local))
        .map_or(Ok(()), |addr| {
            Err(WebFetchError::SsrfBlocked {
                host: host.to_string(),
                ip: addr.ip(),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── IPv4 blocking ───────────────────────────────────────────────────

    #[test]
    fn blocks_rfc1918_10x() {
        assert!(is_non_public_ip("10.0.0.1".parse().unwrap()));
        assert!(is_blocked_for_host(
            "10.0.0.1".parse().unwrap(),
            "10.0.0.1",
            true
        ));
    }

    #[test]
    fn blocks_rfc1918_172x() {
        assert!(is_non_public_ip("172.16.0.1".parse().unwrap()));
        assert!(is_non_public_ip("172.31.255.255".parse().unwrap()));
        assert!(!is_non_public_ip("172.15.0.1".parse().unwrap()));
        assert!(!is_non_public_ip("172.32.0.1".parse().unwrap()));
    }

    #[test]
    fn blocks_rfc1918_192168() {
        assert!(is_non_public_ip("192.168.0.1".parse().unwrap()));
        assert!(is_non_public_ip("192.168.255.255".parse().unwrap()));
    }

    #[test]
    fn blocks_link_local() {
        assert!(is_non_public_ip("169.254.0.1".parse().unwrap()));
        assert!(is_non_public_ip("169.254.169.254".parse().unwrap()));
    }

    #[test]
    fn blocks_cgnat_cloud_metadata() {
        assert!(is_non_public_ip("100.64.0.1".parse().unwrap()));
        assert!(is_non_public_ip("100.127.255.255".parse().unwrap()));
        assert!(!is_non_public_ip("100.63.0.1".parse().unwrap()));
        assert!(!is_non_public_ip("100.128.0.1".parse().unwrap()));
    }

    #[test]
    fn blocks_unspecified() {
        assert!(is_non_public_ip("0.0.0.0".parse().unwrap()));
        assert!(is_non_public_ip("::".parse().unwrap()));
    }

    #[test]
    fn blocks_testnet_reserved_and_this_network() {
        // TEST-NET / reserved / this-network special-purpose ranges
        assert!(is_non_public_ip("192.0.2.1".parse().unwrap()));
        assert!(is_non_public_ip("198.51.100.1".parse().unwrap()));
        assert!(is_non_public_ip("203.0.113.1".parse().unwrap()));
        assert!(is_non_public_ip("240.0.0.1".parse().unwrap()));
        assert!(is_non_public_ip("0.1.2.3".parse().unwrap()));
        assert!(is_non_public_ip("198.18.0.1".parse().unwrap()));
    }

    #[test]
    fn blocks_loopback_by_default() {
        assert!(is_blocked_for_host(
            "127.0.0.1".parse().unwrap(),
            "127.0.0.1",
            false
        ));
        assert!(is_blocked_for_host(
            "127.0.0.2".parse().unwrap(),
            "127.0.0.2",
            false
        ));
        assert!(is_blocked_for_host("::1".parse().unwrap(), "::1", false));
        assert!(is_blocked_for_host(
            "127.0.0.1".parse().unwrap(),
            "localhost",
            false
        ));
    }

    #[test]
    fn allows_explicit_loopback_when_local_binding_enabled() {
        assert!(!is_blocked_for_host(
            "127.0.0.1".parse().unwrap(),
            "127.0.0.1",
            true
        ));
        assert!(!is_blocked_for_host(
            "127.0.0.2".parse().unwrap(),
            "127.0.0.2",
            true
        ));
        assert!(!is_blocked_for_host("::1".parse().unwrap(), "::1", true));
        assert!(!is_blocked_for_host(
            "127.0.0.1".parse().unwrap(),
            "localhost",
            true
        ));
        assert!(!is_blocked_for_host(
            "127.0.0.1".parse().unwrap(),
            "localhost.",
            true
        ));
        // IPv4-mapped loopback (common dual-stack DNS result for localhost).
        assert!(!is_blocked_for_host(
            "::ffff:127.0.0.1".parse().unwrap(),
            "localhost",
            true
        ));
        assert!(!is_blocked_for_host(
            "::ffff:127.0.0.1".parse().unwrap(),
            "127.0.0.1",
            true
        ));
        // Metadata / private ranges stay blocked even with the opt-in.
        assert!(is_blocked_for_host(
            "169.254.169.254".parse().unwrap(),
            "169.254.169.254",
            true
        ));
        assert!(is_blocked_for_host(
            "10.0.0.1".parse().unwrap(),
            "10.0.0.1",
            true
        ));
        // Mapped private is still blocked under local opt-in.
        assert!(is_blocked_for_host(
            "::ffff:10.0.0.1".parse().unwrap(),
            "localhost",
            true
        ));
    }

    #[test]
    fn rebinding_hostname_to_loopback_stays_blocked() {
        // Hostnames that resolve to local IPs stay blocked even when local
        // binding is allowed — only explicit local hosts open loopback.
        assert!(is_blocked_for_host(
            "127.0.0.1".parse().unwrap(),
            "evil.example.com",
            true
        ));
        assert!(is_blocked_for_host(
            "127.0.0.1".parse().unwrap(),
            "localtest.me",
            true
        ));
        assert!(is_blocked_for_host(
            "::1".parse().unwrap(),
            "attacker.test",
            true
        ));
    }

    #[test]
    fn explicit_local_host_detection() {
        assert!(is_explicit_local_host("localhost"));
        assert!(is_explicit_local_host("LOCALHOST."));
        assert!(is_explicit_local_host("127.0.0.1"));
        assert!(is_explicit_local_host("127.1.2.3"));
        assert!(is_explicit_local_host("::1"));
        assert!(is_explicit_local_host("[::1]"));
        assert!(!is_explicit_local_host("example.com"));
        assert!(!is_explicit_local_host("10.0.0.1"));
        assert!(!is_explicit_local_host("notlocalhost"));
    }

    #[test]
    fn allows_public_ips() {
        assert!(!is_non_public_ip("1.1.1.1".parse().unwrap()));
        assert!(!is_non_public_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_non_public_ip("142.250.80.46".parse().unwrap()));
        assert!(!is_blocked_for_host(
            "1.1.1.1".parse().unwrap(),
            "1.1.1.1",
            false
        ));
    }

    // ── IPv6 ────────────────────────────────────────────────────────────

    #[test]
    fn blocks_ipv6_link_local() {
        assert!(is_non_public_ip("fe80::1".parse().unwrap()));
    }

    #[test]
    fn blocks_ipv6_unique_local() {
        assert!(is_non_public_ip("fc00::1".parse().unwrap()));
        assert!(is_non_public_ip("fd00::1".parse().unwrap()));
    }

    #[test]
    fn blocks_ipv4_mapped_ipv6_private() {
        assert!(is_non_public_ip(
            "::ffff:10.0.0.1".parse::<IpAddr>().unwrap()
        ));
        assert!(is_non_public_ip(
            "::ffff:192.168.1.1".parse::<IpAddr>().unwrap()
        ));
    }

    #[test]
    fn allows_ipv4_mapped_ipv6_public() {
        assert!(!is_non_public_ip(
            "::ffff:8.8.8.8".parse::<IpAddr>().unwrap()
        ));
    }

    // ── check_ssrf integration ──────────────────────────────────────────

    #[tokio::test]
    async fn ssrf_blocks_ip_literal_private() {
        let url = Url::parse("https://10.0.0.1/secret").unwrap();
        let result = check_ssrf(&url, false, false).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("private"));
    }

    #[tokio::test]
    async fn ssrf_blocks_loopback_literal_by_default() {
        let url = Url::parse("http://127.0.0.1:8080/").unwrap();
        let result = check_ssrf(&url, false, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ssrf_allows_loopback_literal_when_opted_in() {
        let url = Url::parse("http://127.0.0.1:8080/").unwrap();
        assert!(check_ssrf(&url, true, false).await.is_ok());
    }

    #[tokio::test]
    async fn ssrf_allows_ip_literal_public() {
        let url = Url::parse("https://1.1.1.1/").unwrap();
        let result = check_ssrf(&url, false, false).await;
        assert!(result.is_ok());
    }

    // ── via_proxy ───────────────────────────────────────────────────────

    #[test]
    fn skips_local_dns_only_for_plain_names_under_proxy() {
        // 名字 + 代理 = 本地解析不作数，跳过。
        assert!(skips_local_dns("example.com", true));
        assert!(skips_local_dns("html.duckduckgo.com", true));
        // 没配代理：一切照旧。
        assert!(!skips_local_dns("example.com", false));
        // IP 字面量：URL 自己点名了地址，代理救不了它。
        assert!(!skips_local_dns("10.0.0.1", true));
        assert!(!skips_local_dns("127.0.0.1", true));
        assert!(!skips_local_dns("169.254.169.254", true));
        assert!(!skips_local_dns("::1", true));
        // 显式本地主机名：仍由 allow_local 决定。
        assert!(!skips_local_dns("localhost", true));
        assert!(!skips_local_dns("localhost.", true));
    }

    /// 配了代理时，普通域名不再触发本地 DNS——用 `.invalid`（RFC 2606，永不解析）
    /// 来证。旧行为下这个名字必然走 DNS 并失败（NXDOMAIN，或 fake-ip 环境里解成
    /// 198.18 保留段被拦），两种都是 `Err`，所以这条用例是真能打到原缺陷的。
    #[tokio::test]
    async fn via_proxy_skips_dns_for_plain_hostname() {
        let url = Url::parse("https://not-a-real-host.invalid/page").unwrap();
        assert!(
            check_ssrf(&url, false, true).await.is_ok(),
            "配了代理时不该再拿本地 DNS 结果判私网"
        );
        // 同一个 URL 不配代理：还是一路走到 DNS，解析不出来就报错。
        assert!(check_ssrf(&url, false, false).await.is_err());
    }

    /// 开了代理也不能靠 IP 字面量摸内网。
    #[tokio::test]
    async fn via_proxy_still_blocks_ip_literals() {
        for raw in [
            "https://10.0.0.1/secret",
            "http://127.0.0.1:8080/",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            let url = Url::parse(raw).unwrap();
            assert!(
                check_ssrf(&url, false, true).await.is_err(),
                "{raw} 不该因为配了代理就放行"
            );
        }
        // 公网字面量照常。
        let public = Url::parse("https://1.1.1.1/").unwrap();
        assert!(check_ssrf(&public, false, true).await.is_ok());
    }
}
