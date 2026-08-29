//! Cached HTTP client with atomic invalidation for `web_fetch`.
//!
//! Copied from Grok `web_fetch/http.rs`. `xai_grok_extra_ca` is replaced with
//! a plain `reqwest::Client::builder()` — dock has no extra-CA crate.

use std::sync::Arc;

use arc_swap::ArcSwapOption;

use super::config::WebFetchParams;
use super::error::WebFetchError;

/// Cached, invalidatable HTTP client for web fetching.
#[derive(Clone, Debug)]
pub(crate) struct HttpClient {
    inner: Arc<ArcSwapOption<reqwest::Client>>,
    params: WebFetchParams,
}

impl HttpClient {
    pub(crate) fn new(params: &WebFetchParams) -> Result<Self, WebFetchError> {
        if let Some(ref endpoint) = params.proxy_endpoint {
            reqwest::Proxy::all(endpoint)
                .map_err(|e| WebFetchError::ProxyConfigError(e.to_string()))?;
        }
        Ok(Self {
            inner: Arc::new(ArcSwapOption::from(None)),
            params: params.clone(),
        })
    }

    pub(crate) fn get_or_rebuild(&self) -> Result<Arc<reqwest::Client>, WebFetchError> {
        if let Some(client) = self.inner.load_full() {
            return Ok(client);
        }
        let fresh = Arc::new(Self::build(&self.params)?);
        self.inner.store(Some(Arc::clone(&fresh)));
        Ok(fresh)
    }

    pub(crate) fn invalidate(&self) {
        self.inner.store(None);
    }

    fn build(params: &WebFetchParams) -> Result<reqwest::Client, WebFetchError> {
        let proxy = params
            .proxy_endpoint
            .as_ref()
            .map(reqwest::Proxy::all)
            .transpose()
            .map_err(|e| WebFetchError::ProxyConfigError(e.to_string()))?;
        let mut builder = reqwest::Client::builder()
            .timeout(params.timeout_secs())
            .connect_timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .tcp_nodelay(true)
            .gzip(true)
            .brotli(true)
            .deflate(true);
        if let Some(proxy) = proxy {
            builder = builder.proxy(proxy);
        }
        builder.build().map_err(WebFetchError::ClientBuildError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_or_rebuild_returns_client() {
        let client = HttpClient::new(&WebFetchParams::default()).unwrap();
        let http = client.get_or_rebuild().unwrap();
        assert!(Arc::strong_count(&http) >= 1);
    }

    #[test]
    fn invalidate_forces_rebuild() {
        let client = HttpClient::new(&WebFetchParams::default()).unwrap();
        let first = client.get_or_rebuild().unwrap();
        let first_ptr = Arc::as_ptr(&first);
        client.invalidate();
        let second = client.get_or_rebuild().unwrap();
        assert_ne!(first_ptr, Arc::as_ptr(&second));
    }

    #[test]
    fn build_with_proxy_endpoint() {
        let params = WebFetchParams {
            proxy_endpoint: Some("https://proxy.corp.example.com".into()),
            ..Default::default()
        };
        assert!(HttpClient::new(&params).is_ok());
    }

    #[test]
    fn build_with_invalid_proxy_endpoint() {
        let params = WebFetchParams {
            proxy_endpoint: Some("not a valid url".into()),
            ..Default::default()
        };
        let err = HttpClient::new(&params).unwrap_err().to_string();
        assert!(
            err.contains("proxy"),
            "Expected proxy-related error, got: {err}"
        );
    }
}
