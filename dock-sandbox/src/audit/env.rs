//! 给命令的环境变量：让它的出网走出口代理、信 Dock 根证书。
//!
//! 能覆盖到的：认 `HTTP(S)_PROXY` 的（curl、git、python、npm、go、cargo、pip…），Node 的
//! 原生 `fetch` 要 `NODE_USE_ENV_PROXY=1`（Node 24+）。不认这些变量、自己直连的程序代理看不到
//! ——那是已知缺口，不是这里能补的。

use std::path::Path;

/// 代理地址的环境变量名（大小写两套都设：不同程序认的不一样）。
const PROXY_VARS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

/// 「整个换掉信任的根证书」的变量：指向系统根证书 + Dock 根证书的合集。
const BUNDLE_VARS: &[&str] = &[
    "SSL_CERT_FILE",
    "CURL_CA_BUNDLE",
    "REQUESTS_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "CARGO_HTTP_CAINFO",
    "PIP_CERT",
    "npm_config_cafile",
];

/// 命令要加的环境变量。`ca_cert` / `bundle` 为 `None` 时（没开解密）只设代理。
/// `inherited_no_proxy` 是 Dock 进程原来的 `NO_PROXY`：照样不走代理。
pub fn command_env(
    port: u16,
    certs: Option<(&Path, &Path)>,
    inherited_no_proxy: &str,
) -> Vec<(String, String)> {
    let proxy = format!("http://127.0.0.1:{port}");
    let mut no_proxy = vec!["localhost", "127.0.0.1", "::1"];
    no_proxy.extend(
        inherited_no_proxy
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty()),
    );
    let no_proxy = no_proxy.join(",");
    let mut env: Vec<(String, String)> = PROXY_VARS
        .iter()
        .map(|k| (k.to_string(), proxy.clone()))
        .collect();
    env.push(("NO_PROXY".into(), no_proxy.clone()));
    env.push(("no_proxy".into(), no_proxy));
    env.push(("NODE_USE_ENV_PROXY".into(), "1".into()));
    env.push(("DOCK_AUDIT".into(), "1".into()));
    if let Some((ca_cert, bundle)) = certs {
        let bundle = bundle.display().to_string();
        env.extend(BUNDLE_VARS.iter().map(|k| (k.to_string(), bundle.clone())));
        env.push(("NODE_EXTRA_CA_CERTS".into(), ca_cert.display().to_string()));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_points_clients_at_the_proxy_and_the_bundle() {
        let env = command_env(
            4321,
            Some((Path::new("/ca.pem"), Path::new("/b.pem"))),
            "corp.local, ",
        );
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("HTTPS_PROXY"), Some("http://127.0.0.1:4321"));
        assert_eq!(get("all_proxy"), Some("http://127.0.0.1:4321"));
        assert_eq!(get("NO_PROXY"), Some("localhost,127.0.0.1,::1,corp.local"));
        assert_eq!(get("SSL_CERT_FILE"), Some("/b.pem"));
        assert_eq!(get("NODE_EXTRA_CA_CERTS"), Some("/ca.pem"));
        assert_eq!(get("NODE_USE_ENV_PROXY"), Some("1"));
        let plain = command_env(1, None, "");
        assert!(plain.iter().all(|(k, _)| k != "SSL_CERT_FILE"));
    }
}
