//! Fetch pipeline copied from Grok `web_fetch/client.rs`:
//! `validate_url`, `upgrade_to_https`, same-host redirect `fetch_url`, htmd.

use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, USER_AGENT};
use url::Url;

use super::config::{WebFetchParams, MAX_REDIRECTS, MAX_URL_LENGTH, USER_AGENT_STRING};
use super::domain::DomainMatcher;
use super::error::WebFetchError;
use super::http::HttpClient;
use super::ssrf::{self, check_ssrf};

enum FetchResult {
    Content {
        body: Vec<u8>,
        content_type: String,
        final_url: String,
        status_code: u16,
    },
    CrossHostRedirect {
        original_host: String,
        redirect_url: String,
    },
}

/// Validates URL scheme, length, credentials, and hostname labels.
fn validate_url(raw: &str) -> Result<Url, WebFetchError> {
    if raw.len() > MAX_URL_LENGTH {
        return Err(WebFetchError::UrlTooLong {
            max: MAX_URL_LENGTH,
        });
    }

    let parsed = Url::parse(raw)?;

    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(WebFetchError::UnsupportedScheme {
                scheme: scheme.to_string(),
            });
        }
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(WebFetchError::CredentialsInUrl);
    }

    if let Some(host) = parsed.host_str() {
        if host.split('.').count() < 2 && !ssrf::is_explicit_local_host(host) {
            return Err(WebFetchError::SingleLabelHost {
                host: host.to_string(),
            });
        }
    }

    Ok(parsed)
}

/// Upgrade `http://` to `https://`, except for explicit loopback hosts.
fn upgrade_to_https(url: &mut Url) {
    if url.scheme() != "http" {
        return;
    }
    if let Some(host) = url.host_str() {
        if ssrf::is_explicit_local_host(host) {
            return;
        }
    }
    let _ = url.set_scheme("https");
}

/// Fetch a URL with manual same-host redirect handling.
async fn fetch_hops(
    client: &reqwest::Client,
    url: &Url,
    max_content_length: usize,
    allow_local: bool,
    via_proxy: bool,
) -> Result<FetchResult, WebFetchError> {
    let mut current_url = url.clone();
    let mut hops = 0;

    loop {
        check_ssrf(&current_url, allow_local, via_proxy).await?;

        let resp = client
            .get(current_url.as_str())
            .header(USER_AGENT, USER_AGENT_STRING)
            .header(
                ACCEPT,
                "text/markdown,text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header(ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .send()
            .await?;

        let status = resp.status();

        if status.is_redirection() {
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err(WebFetchError::TooManyRedirects { max: MAX_REDIRECTS });
            }

            if let Some(location) = resp.headers().get("location") {
                let location_str = location.to_str().unwrap_or("");
                let mut next_url = current_url
                    .join(location_str)
                    .map_err(|e| WebFetchError::InvalidRedirect(format!("{e}")))?;
                if is_same_host(&current_url, &next_url) {
                    upgrade_to_https(&mut next_url);
                    current_url = next_url;
                    continue;
                }
                return Ok(FetchResult::CrossHostRedirect {
                    original_host: current_url.host_str().unwrap_or("unknown").to_string(),
                    redirect_url: next_url.to_string(),
                });
            }
        }

        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/html")
            .to_string();
        let final_url = resp.url().to_string();
        let status_code = status.as_u16();
        let body = resp.bytes().await?;

        if body.len() > max_content_length {
            return Err(WebFetchError::ResponseTooLarge {
                max: max_content_length,
            });
        }

        return Ok(FetchResult::Content {
            body: body.to_vec(),
            content_type,
            final_url,
            status_code,
        });
    }
}

fn is_same_host(a: &Url, b: &Url) -> bool {
    a.host_str() == b.host_str()
}

fn is_html(content_type: &str) -> bool {
    content_type.contains("text/html") || content_type.contains("application/xhtml")
}

fn html_to_markdown(html: &str) -> String {
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "noscript", "svg", "iframe", "object", "embed",
        ])
        .build();
    converter.convert(html).unwrap_or_else(|_| html.to_string())
}

fn to_prompt(result: FetchResult, max_markdown: usize) -> Result<String, WebFetchError> {
    match result {
        FetchResult::CrossHostRedirect {
            original_host,
            redirect_url,
        } => Ok(format!(
            "Error: cross-host redirect from {original_host} to {redirect_url}. Make a new web_fetch call with the redirect URL if needed."
        )),
        FetchResult::Content {
            body,
            content_type,
            final_url,
            status_code,
        } => {
            let raw = String::from_utf8_lossy(&body);
            let mut text = if is_html(&content_type) {
                html_to_markdown(&raw)
            } else {
                raw.into_owned()
            };
            if text.len() > max_markdown {
                text.truncate(max_markdown);
                text.push_str("\n\n[truncated]");
            }
            Ok(format!("HTTP {status_code} {final_url}\n\n{text}"))
        }
    }
}

pub async fn fetch_url(raw: &str, params: &WebFetchParams) -> Result<String, WebFetchError> {
    let mut url = validate_url(raw)?;
    upgrade_to_https(&mut url);

    if let Some(list) = params.allowed_domains() {
        if let Some(blocked) = DomainMatcher::new(&list).check(&url) {
            let super::domain::DomainNotAllowed::DomainNotAllowed(host) = blocked;
            return Ok(format!(
                "Error: domain {host} is not in the allowed domains list"
            ));
        }
    }

    let http = HttpClient::new(params)?;
    let client = http.get_or_rebuild()?;
    let result = match fetch_hops(
        &client,
        &url,
        params.max_content_length(),
        params.allow_local(),
        params.via_proxy(),
    )
    .await
    {
        Ok(result) => result,
        Err(e @ WebFetchError::HttpRequest(_)) => {
            http.invalidate();
            return Err(e);
        }
        Err(e) => return Err(e),
    };
    to_prompt(result, params.max_markdown_length())
}

/// Grok `web_search` talks to the xAI Responses API (account). Dock has no
/// Grok account client; search reuses the copied fetch/SSRF path against a
/// public HTML index.
pub async fn search_web(query: &str, params: &WebFetchParams) -> Result<String, WebFetchError> {
    let q = query.trim();
    if q.is_empty() {
        return Err(WebFetchError::InvalidRedirect("empty query".into()));
    }
    let url = Url::parse_with_params("https://html.duckduckgo.com/html/", &[("q", q)])
        .map_err(WebFetchError::InvalidUrl)?;
    // `allowed_domains` 是给**模型点名的 URL**用的闸；搜索端点是这里写死的，
    // 不该受它管——`html.duckduckgo.com` 不可能出现在任何人的白名单里，照判
    // 就是整颗 `web_search` 报废。SSRF 与其余参数照常生效。
    let mut params = params.clone();
    params.allowed_domains = None;
    let body = fetch_url(url.as_str(), &params).await?;
    Ok(extract_results(&body, q))
}

fn extract_results(body: &str, query: &str) -> String {
    let mut hits = Vec::new();
    let mut rest = body;
    while let Some(idx) = rest.find("http") {
        let slice = &rest[idx..];
        let end = slice
            .find(|c: char| c.is_whitespace() || c == '"' || c == '<' || c == '\'')
            .unwrap_or(slice.len().min(200));
        let url = &slice[..end];
        if url.starts_with("http")
            && !url.contains("duckduckgo.com")
            && !hits.iter().any(|h: &String| h == url)
        {
            hits.push(url.to_string());
        }
        rest = &slice[end.min(slice.len()).max(1)..];
        if hits.len() >= 8 {
            break;
        }
    }
    if hits.is_empty() {
        return format!(
            "No results for {query:?}. Snippet:\n{}",
            body.chars().take(800).collect::<String>()
        );
    }
    let mut out = format!("Search results for {query:?}:\n");
    for (i, u) in hits.iter().enumerate() {
        out.push_str(&format!("{}. {u}\n", i + 1));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_url_accepts_valid() {
        assert!(validate_url("https://docs.python.org/3/").is_ok());
    }

    #[test]
    fn validate_url_rejects_single_label_hosts() {
        assert!(matches!(
            validate_url("https://intranet/"),
            Err(WebFetchError::SingleLabelHost { .. })
        ));
    }

    #[test]
    fn upgrade_to_https_skips_explicit_local_hosts() {
        let mut url = Url::parse("http://127.0.0.1:8080/").unwrap();
        upgrade_to_https(&mut url);
        assert_eq!(url.scheme(), "http");
        let mut remote = Url::parse("http://example.com/").unwrap();
        upgrade_to_https(&mut remote);
        assert_eq!(remote.scheme(), "https");
    }

    #[test]
    fn validate_url_rejects_credentials() {
        assert!(matches!(
            validate_url("https://user:pass@example.com/"),
            Err(WebFetchError::CredentialsInUrl)
        ));
    }

    #[test]
    fn html_to_markdown_strips_script() {
        let md = html_to_markdown("<html><script>alert(1)</script><p>hi</p></html>");
        assert!(md.contains("hi"));
        assert!(!md.contains("alert"));
    }
}
