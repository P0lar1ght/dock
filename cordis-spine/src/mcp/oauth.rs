//! MCP HTTP OAuth: RFC 8414 / 9728 discovery, RFC 7591 DCR, PKCE browser consent.
//!
//! Not grok.com account login. Tokens live in [`super::credentials`].

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use reqwest::header::{HeaderMap, WWW_AUTHENTICATE};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use url::Url;

use crate::config::McpServer;

use super::credentials::{self, McpCredentialStore, StoredCredentials, TokenResponse};

const CLIENT_NAME: &str = "Dock";
const BROWSER_AUTH_TIMEOUT: Duration = Duration::from_secs(600);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct AuthMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub scopes_supported: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ProtectedResource {
    #[serde(default)]
    authorization_servers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AsMetadata {
    #[serde(default)]
    authorization_endpoint: Option<String>,
    #[serde(default)]
    token_endpoint: Option<String>,
    #[serde(default)]
    registration_endpoint: Option<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallbackPayload {
    pub code: String,
    pub state: String,
    pub issuer: Option<String>,
}

pub fn is_auth_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("401") || e.contains("unauthorized") || e.contains("oauth")
}

pub fn parse_resource_metadata_url(www_authenticate: &str) -> Option<String> {
    let lower = www_authenticate.to_ascii_lowercase();
    let key = "resource_metadata=";
    let idx = lower.find(key)?;
    let rest = www_authenticate[idx + key.len()..].trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        let end = rest
            .find(|c: char| c == ',' || c.is_whitespace())
            .unwrap_or(rest.len());
        let value = rest[..end].trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    }
}

pub fn parse_oauth_callback_params(
    params: &HashMap<String, String>,
) -> Result<OAuthCallbackPayload, String> {
    if let Some(error) = params.get("error") {
        let desc = params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| "Unknown error".into());
        return Err(format!("OAuth error: {error} - {desc}"));
    }
    let code = params
        .get("code")
        .filter(|s| !s.is_empty())
        .cloned()
        .ok_or_else(|| "Missing authorization code".to_string())?;
    let state = params
        .get("state")
        .filter(|s| !s.is_empty())
        .cloned()
        .ok_or_else(|| "Missing state parameter".to_string())?;
    Ok(OAuthCallbackPayload {
        code,
        state,
        issuer: params.get("iss").cloned(),
    })
}

fn in_flight(
) -> &'static tokio::sync::Mutex<HashMap<String, watch::Receiver<Option<Result<(), String>>>>> {
    static CELL: OnceLock<
        tokio::sync::Mutex<HashMap<String, watch::Receiver<Option<Result<(), String>>>>>,
    > = OnceLock::new();
    CELL.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

/// Browser PKCE login, persist tokens, leave reconnect to the caller.
/// In-process only: a second `/mcps` `i` on the same server waits for the first tab.
pub async fn browser_login(server: &McpServer) -> Result<(), String> {
    let name = server.name.clone();
    let mut map = in_flight().lock().await;
    if map.get(&name).is_some_and(|rx| rx.has_changed().is_err()) {
        map.remove(&name);
    }
    if let Some(rx) = map.get(&name) {
        let mut rx = rx.clone();
        drop(map);
        loop {
            if let Some(result) = rx.borrow_and_update().clone() {
                return result;
            }
            if rx.changed().await.is_err() {
                return Err("Auth leader dropped".into());
            }
        }
    }
    let (tx, rx) = watch::channel(None);
    map.insert(name.clone(), rx);
    drop(map);
    let result = browser_login_inner(server).await;
    let _ = tx.send(Some(result.clone()));
    in_flight().lock().await.remove(&name);
    result
}

async fn browser_login_inner(server: &McpServer) -> Result<(), String> {
    let url = match &server.transport {
        crate::config::McpTransport::Http { url, .. } => url.clone(),
        _ => return Err("OAuth 只适用于 HTTP MCP 服务器".into()),
    };
    let resource = url.clone();
    let meta = discover_metadata(&url).await?;
    let requested_port = server.oauth.callback_port.unwrap_or(0);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", requested_port))
        .await
        .map_err(|e| format!("无法绑定回调端口 {requested_port}: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("回调端口: {e}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let scopes = if server.oauth.scopes.is_empty() {
        meta.scopes_supported.clone()
    } else {
        server.oauth.scopes.clone()
    };

    let (client_id, client_secret) = if let Some(id) = server.oauth.client_id.clone() {
        (id, server.oauth.client_secret.clone())
    } else {
        let id = dynamic_register(&meta, &redirect_uri, &scopes).await?;
        (id, None)
    };

    let verifier = pkce_verifier();
    let challenge = pkce_challenge(&verifier);
    let state = random_urlsafe(24);
    let mut auth = Url::parse(&meta.authorization_endpoint)
        .map_err(|e| format!("authorization_endpoint: {e}"))?;
    {
        let mut q = auth.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", &client_id);
        q.append_pair("redirect_uri", &redirect_uri);
        q.append_pair("state", &state);
        q.append_pair("code_challenge", &challenge);
        q.append_pair("code_challenge_method", "S256");
        q.append_pair("resource", &resource);
        if !scopes.is_empty() {
            q.append_pair("scope", &scopes.join(" "));
        }
    }
    let auth_url = auth.to_string();
    tracing::info!(
        server = server.name.as_str(),
        url = auth_url.as_str(),
        "MCP OAuth: opening browser"
    );
    if let Err(e) = webbrowser::open(&auth_url) {
        tracing::warn!(%e, url = %auth_url, "未能自动打开浏览器；请手动打开该地址");
    }

    let callback = tokio::time::timeout(BROWSER_AUTH_TIMEOUT, accept_callback(listener))
        .await
        .map_err(|_| {
            format!(
                "OAuth 等待超时（{}s）。请打开: {auth_url}",
                BROWSER_AUTH_TIMEOUT.as_secs()
            )
        })??;
    if callback.state != state {
        return Err("OAuth state mismatch".into());
    }
    let token = exchange_code(
        &meta.token_endpoint,
        &client_id,
        client_secret.as_deref(),
        &redirect_uri,
        &callback.code,
        &verifier,
        &resource,
    )
    .await?;
    persist(
        server,
        &resource,
        client_id,
        client_secret,
        &meta.token_endpoint,
        token,
        scopes,
    )
}

pub async fn refresh_stored(server: &McpServer) -> Result<String, String> {
    let url = match &server.transport {
        crate::config::McpTransport::Http { url, .. } => url.clone(),
        _ => return Err("not HTTP".into()),
    };
    let parsed = Url::parse(&url).map_err(|e| e.to_string())?;
    let store = McpCredentialStore::load_default().map_err(|e| e.to_string())?;
    let Some(creds) = store.get(&server.name, &parsed).cloned() else {
        return Err("no stored credentials".into());
    };
    let Some(refresh) = creds
        .token_response
        .as_ref()
        .and_then(|t| t.refresh_token.clone())
    else {
        return Err("no refresh token".into());
    };
    let token_endpoint = match creds.token_endpoint.clone() {
        Some(ep) => ep,
        None => discover_metadata(&url).await?.token_endpoint,
    };
    let token = refresh_token(
        &token_endpoint,
        &creds.client_id,
        creds.client_secret.as_deref(),
        &refresh,
        &url,
    )
    .await?;
    let access = token.access_token.clone();
    persist(
        server,
        &url,
        creds.client_id,
        creds.client_secret,
        &token_endpoint,
        token,
        creds.granted_scopes,
    )?;
    Ok(access)
}

fn persist(
    server: &McpServer,
    resource: &str,
    client_id: String,
    client_secret: Option<String>,
    token_endpoint: &str,
    token: TokenResponse,
    scopes: Vec<String>,
) -> Result<(), String> {
    let parsed = Url::parse(resource).map_err(|e| e.to_string())?;
    let mut store = McpCredentialStore::load_default().map_err(|e| e.to_string())?;
    store
        .insert_and_save(
            &server.name,
            &parsed,
            StoredCredentials {
                client_id,
                client_secret,
                token_endpoint: Some(token_endpoint.to_string()),
                granted_scopes: scopes,
                token_received_at: Some(credentials::now_unix()),
                token_response: Some(token),
            },
        )
        .map_err(|e| e.to_string())
}

pub async fn discover_metadata(mcp_url: &str) -> Result<AuthMetadata, String> {
    let client = probe_client()?;
    let prm_urls = {
        let mut urls = Vec::new();
        if let Ok(resp) = tokio::time::timeout(
            DISCOVERY_TIMEOUT,
            client
                .post(mcp_url)
                .header("Accept", "application/json, text/event-stream")
                .body("{}")
                .send(),
        )
        .await
        {
            if let Ok(resp) = resp {
                if let Some(h) = header_join(resp.headers(), &WWW_AUTHENTICATE) {
                    if let Some(u) = parse_resource_metadata_url(&h) {
                        urls.push(u);
                    }
                }
            }
        }
        urls.extend(well_known_protected_resource(mcp_url));
        urls
    };
    let mut issuers = Vec::new();
    for prm in prm_urls {
        if let Ok(body) = fetch_json(&prm).await {
            if let Ok(prm) = serde_json::from_value::<ProtectedResource>(body) {
                issuers.extend(prm.authorization_servers);
            }
        }
    }
    if let Ok(origin) = Url::parse(mcp_url) {
        if let Some(host) = origin.host_str() {
            let origin_s = format!(
                "{}://{}{}",
                origin.scheme(),
                host,
                origin.port().map(|p| format!(":{p}")).unwrap_or_default()
            );
            if !issuers.iter().any(|i| i == &origin_s) {
                issuers.push(origin_s);
            }
        }
    }
    for issuer in issuers {
        let well_known = as_well_known(&issuer);
        if let Ok(body) = fetch_json(&well_known).await {
            if let Ok(meta) = serde_json::from_value::<AsMetadata>(body) {
                if let (Some(auth), Some(token)) =
                    (meta.authorization_endpoint, meta.token_endpoint)
                {
                    return Ok(AuthMetadata {
                        authorization_endpoint: auth,
                        token_endpoint: token,
                        registration_endpoint: meta.registration_endpoint,
                        scopes_supported: meta.scopes_supported,
                    });
                }
            }
        }
    }
    Err("未发现 OAuth 授权服务器（RFC 8414 / 9728）".into())
}

fn well_known_protected_resource(mcp_url: &str) -> Vec<String> {
    let Ok(u) = Url::parse(mcp_url) else {
        return Vec::new();
    };
    let origin = format!(
        "{}://{}{}",
        u.scheme(),
        u.host_str().unwrap_or(""),
        u.port().map(|p| format!(":{p}")).unwrap_or_default()
    );
    let path = u.path().trim_end_matches('/');
    let mut out = vec![format!("{origin}/.well-known/oauth-protected-resource")];
    if !path.is_empty() && path != "/" {
        out.insert(
            0,
            format!("{origin}/.well-known/oauth-protected-resource{path}"),
        );
    }
    out
}

fn as_well_known(issuer: &str) -> String {
    let Ok(u) = Url::parse(issuer) else {
        return format!(
            "{}/.well-known/oauth-authorization-server",
            issuer.trim_end_matches('/')
        );
    };
    let origin = format!(
        "{}://{}{}",
        u.scheme(),
        u.host_str().unwrap_or(""),
        u.port().map(|p| format!(":{p}")).unwrap_or_default()
    );
    let path = u.path().trim_end_matches('/');
    if path.is_empty() || path == "/" {
        format!("{origin}/.well-known/oauth-authorization-server")
    } else {
        format!("{origin}/.well-known/oauth-authorization-server{path}")
    }
}

fn probe_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())
}

async fn fetch_json(url: &str) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

fn header_join(headers: &HeaderMap, name: &reqwest::header::HeaderName) -> Option<String> {
    let vals: Vec<&str> = headers
        .get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    if vals.is_empty() {
        None
    } else {
        Some(vals.join(", "))
    }
}

async fn dynamic_register(
    meta: &AuthMetadata,
    redirect_uri: &str,
    scopes: &[String],
) -> Result<String, String> {
    let Some(reg) = &meta.registration_endpoint else {
        return Err("未配置 oauth.clientId，且授权服务器没有动态注册端点".into());
    };
    let client = probe_client()?;
    let body = json!({
        "client_name": CLIENT_NAME,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "scope": scopes.join(" "),
    });
    let resp = client
        .post(reg)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("动态注册: {e}"))?;
    if !resp.status().is_success() {
        let t = resp.text().await.unwrap_or_default();
        return Err(format!("动态注册失败: {t}"));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    v.get("client_id")
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "动态注册未返回 client_id".into())
}

async fn exchange_code(
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
    resource: &str,
) -> Result<TokenResponse, String> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
        ("resource", resource),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    post_token(token_endpoint, &form).await
}

async fn refresh_token(
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    refresh: &str,
    resource: &str,
) -> Result<TokenResponse, String> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", client_id),
        ("resource", resource),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    post_token(token_endpoint, &form).await
}

async fn post_token(endpoint: &str, form: &[(&str, &str)]) -> Result<TokenResponse, String> {
    let client = probe_client()?;
    let resp = client
        .post(endpoint)
        .form(form)
        .send()
        .await
        .map_err(|e| format!("token: {e}"))?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(json!({}));
    if !status.is_success() {
        return Err(format!(
            "token HTTP {status}: {}",
            v.get("error_description")
                .or_else(|| v.get("error"))
                .and_then(|e| e.as_str())
                .unwrap_or("exchange failed")
        ));
    }
    let access = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "token response missing access_token".to_string())?;
    Ok(TokenResponse {
        access_token: access.to_string(),
        token_type: v
            .get("token_type")
            .and_then(|t| t.as_str())
            .unwrap_or("bearer")
            .to_string(),
        expires_in: v.get("expires_in").and_then(|n| n.as_u64()),
        refresh_token: v
            .get("refresh_token")
            .and_then(|t| t.as_str())
            .map(str::to_string),
        scope: v.get("scope").and_then(|t| t.as_str()).map(str::to_string),
    })
}

fn pkce_verifier() -> String {
    random_urlsafe(32)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn random_urlsafe(nbytes: usize) -> String {
    let mut buf = vec![0u8; nbytes];
    getrandom::getrandom(&mut buf).expect("getrandom");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn accept_callback(
    listener: tokio::net::TcpListener,
) -> Result<OAuthCallbackPayload, String> {
    let (mut stream, _) = listener
        .accept()
        .await
        .map_err(|e| format!("callback accept: {e}"))?;
    let mut buf = vec![0u8; 8192];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| format!("callback read: {e}"))?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req.lines().next().unwrap_or("");
    let target = path.split_whitespace().nth(1).unwrap_or("/");
    let qs = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let params = parse_query(qs);
    let parsed = parse_oauth_callback_params(&params);
    let (status, body) = match &parsed {
        Ok(_) => (
            "200 OK",
            "<!DOCTYPE html><html><head><meta charset=utf-8><title>授权完成</title></head>\
             <body style=\"font-family:sans-serif;text-align:center;padding:50px\">\
             <h1>授权完成</h1><p>可以关闭此窗口并回到终端。</p>\
             <script>window.close();</script></body></html>"
                .to_string(),
        ),
        Err(e) => {
            let msg = html_escape(e);
            (
                "400 Bad Request",
                format!(
                    "<!DOCTYPE html><html><head><meta charset=utf-8><title>授权失败</title></head>\
                     <body style=\"font-family:sans-serif;text-align:center;padding:50px\">\
                     <h1>授权失败</h1><p>{msg}</p></body></html>"
                ),
            )
        }
    };
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    parsed
}

fn parse_query(qs: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(qs.as_bytes())
        .into_owned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_metadata_quoted_and_bare() {
        assert_eq!(
            parse_resource_metadata_url(
                "Bearer realm=\"mcp\", resource_metadata=\"https://ex/.well-known/oauth-protected-resource\""
            )
            .as_deref(),
            Some("https://ex/.well-known/oauth-protected-resource")
        );
        assert_eq!(
            parse_resource_metadata_url("Bearer realm=mcp, resource_metadata=https://as/prm")
                .as_deref(),
            Some("https://as/prm")
        );
        assert!(parse_resource_metadata_url("Basic realm=x").is_none());
    }

    #[test]
    fn callback_requires_code_and_state() {
        let mut p = HashMap::new();
        p.insert("code".into(), "c".into());
        p.insert("state".into(), "s".into());
        p.insert("iss".into(), "https://as".into());
        let got = parse_oauth_callback_params(&p).unwrap();
        assert_eq!(got.code, "c");
        assert_eq!(got.issuer.as_deref(), Some("https://as"));
        let mut err = HashMap::new();
        err.insert("error".into(), "access_denied".into());
        err.insert("error_description".into(), "nope".into());
        assert!(parse_oauth_callback_params(&err)
            .unwrap_err()
            .contains("access_denied"));
    }

    #[test]
    fn pkce_challenge_is_s256() {
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(v),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn is_auth_error_detects_401() {
        assert!(is_auth_error("MCP HTTP 401 unauthorized"));
        assert!(is_auth_error("未发现 OAuth 授权服务器"));
        assert!(!is_auth_error("HTTP 500: boom"));
        assert!(!is_auth_error("stdio MCP 服务器不需要浏览器登录"));
    }

    #[test]
    fn well_known_inserts_path() {
        assert_eq!(
            well_known_protected_resource("https://mcp.linear.app/mcp"),
            vec![
                "https://mcp.linear.app/.well-known/oauth-protected-resource/mcp".to_string(),
                "https://mcp.linear.app/.well-known/oauth-protected-resource".to_string(),
            ]
        );
        assert_eq!(
            as_well_known("https://auth.example.com/tenant"),
            "https://auth.example.com/.well-known/oauth-authorization-server/tenant"
        );
        assert_eq!(
            as_well_known("https://auth.example.com"),
            "https://auth.example.com/.well-known/oauth-authorization-server"
        );
    }

    #[test]
    fn query_decodes_percent() {
        let p = parse_query("code=a%2Fb&state=s+t");
        assert_eq!(p.get("code").map(String::as_str), Some("a/b"));
        assert_eq!(p.get("state").map(String::as_str), Some("s t"));
    }
}
