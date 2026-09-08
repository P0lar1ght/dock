//! Chromiumoxide CDP session: launch, navigate, tabs, dispose.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::accessibility::{EnableParams, GetFullAxTreeParams};
use chromiumoxide::cdp::browser_protocol::dom::{
    FocusParams, GetBoxModelParams, ScrollIntoViewIfNeededParams,
};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, InsertTextParams,
};
use chromiumoxide::cdp::browser_protocol::target::{ActivateTargetParams, CloseTargetParams};
use chromiumoxide::page::{Page, ScreenshotParams};
use futures_util::StreamExt;
use tokio::task::JoinHandle;

use crate::config::dock_home;

use super::snapshot::{self, LeanSnapshot, RefEntry};
use super::wait::{retry_async, with_timeout};

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const NAV_TIMEOUT: Duration = Duration::from_secs(45);
const ACTION_TIMEOUT: Duration = Duration::from_secs(15);

#[allow(dead_code)] // retained for status / debugging profile path
pub struct ConnectedSession {
    browser: Browser,
    pages: Vec<Page>,
    active: usize,
    handler: JoinHandle<()>,
    pub(crate) user_data_dir: PathBuf,
    pub refs: HashMap<String, RefEntry>,
}

impl ConnectedSession {
    pub async fn launch(url: Option<&str>) -> Result<Self, String> {
        let user_data_dir = browser_user_data_dir();
        std::fs::create_dir_all(&user_data_dir)
            .map_err(|e| format!("create user-data-dir {}: {e}", user_data_dir.display()))?;

        let exe = discover_chrome()?;
        let mut builder = BrowserConfig::builder()
            .chrome_executable(&exe)
            .user_data_dir(&user_data_dir)
            .no_sandbox()
            .launch_timeout(LAUNCH_TIMEOUT)
            .arg("--disable-dev-shm-usage")
            .arg("--force-renderer-accessibility");

        // Headless by default (CI / servers). Set DOCK_BROWSER_HEADED=1 for UI.
        if std::env::var_os("DOCK_BROWSER_HEADED").is_some() {
            builder = builder.with_head();
        }

        let config = builder
            .build()
            .map_err(|e| format!("BrowserConfig: {e}"))?;

        let (browser, mut handler) = with_timeout(
            LAUNCH_TIMEOUT,
            async {
                Browser::launch(config)
                    .await
                    .map_err(|e| format!("launch: {e}"))
            },
            || "timed out launching Chromium".into(),
        )
        .await
        .map_err(|e| format!("launch Chromium ({exe}): {e}"))?;

        let handler = tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
        });

        let start = url.unwrap_or("about:blank");
        let page = with_timeout(
            NAV_TIMEOUT,
            async {
                browser
                    .new_page(start)
                    .await
                    .map_err(|e| format!("new_page: {e}"))
            },
            || "timed out opening page".into(),
        )
        .await
        .map_err(|e| format!("new_page({start}): {e}"))?;

        Ok(Self {
            browser,
            pages: vec![page],
            active: 0,
            handler,
            user_data_dir,
            refs: HashMap::new(),
        })
    }

    pub fn active_page(&self) -> Result<&Page, String> {
        self.pages
            .get(self.active)
            .ok_or_else(|| "no active tab".into())
    }

    pub async fn navigate(&mut self, url: &str) -> Result<String, String> {
        let page = self
            .pages
            .get(self.active)
            .ok_or_else(|| "no active tab".to_string())?
            .clone();
        with_timeout(
            NAV_TIMEOUT,
            async {
                page.goto(url)
                    .await
                    .map_err(|e| format!("goto: {e}"))?;
                Ok::<_, String>(())
            },
            || "navigate timeout".into(),
        )
        .await
        .map_err(|e| format!("navigate({url}): {e}"))?;
        self.refs.clear();
        let cur = page.url().await.ok().flatten().unwrap_or_else(|| url.into());
        Ok(cur)
    }

    pub async fn snapshot(&mut self, interactive: bool) -> Result<LeanSnapshot, String> {
        let page = self.active_page()?.clone();
        let snap = with_timeout(
            ACTION_TIMEOUT,
            async {
                page.execute(EnableParams::default())
                    .await
                    .map_err(|e| format!("ax enable: {e}"))?;
                let resp = page
                    .execute(GetFullAxTreeParams::builder().build())
                    .await
                    .map_err(|e| format!("getFullAXTree: {e}"))?;
                Ok::<_, String>(snapshot::build_lean_snapshot(
                    &resp.result.nodes,
                    interactive,
                ))
            },
            || "snapshot timeout".into(),
        )
        .await
        .map_err(|e| format!("browser_snapshot: {e}"))?;
        self.refs = snap.refs.clone();
        Ok(snap)
    }

    pub async fn click_ref(&mut self, ref_id: &str) -> Result<String, String> {
        let key = snapshot::parse_ref(ref_id).ok_or_else(|| "missing ref".to_string())?;
        let entry = self
            .refs
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "unknown ref `{key}` — call browser_snapshot first (have {} refs)",
                    self.refs.len()
                )
            })?;
        let backend = snapshot::backend_id(&entry)
            .ok_or_else(|| format!("ref `{key}` has no backend DOM node"))?;
        let page = self.active_page()?.clone();

        retry_async(3, Duration::from_millis(150), || {
            let page = page.clone();
            let backend = backend.clone();
            async move {
                page.execute(
                    ScrollIntoViewIfNeededParams::builder()
                        .backend_node_id(backend.clone())
                        .build(),
                )
                .await
                .map_err(|e| e.to_string())?;
                let model = page
                    .execute(
                        GetBoxModelParams::builder()
                            .backend_node_id(backend)
                            .build(),
                    )
                    .await
                    .map_err(|e| e.to_string())?
                    .result
                    .model;
                let point = chromiumoxide::layout::ElementQuad::from_quad(&model.content)
                    .quad_center();
                page.click(point)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(())
            }
        })
        .await?;

        self.refs.clear();
        Ok(format!("clicked @{key} ({})", entry.role))
    }

    pub async fn type_ref(
        &mut self,
        ref_id: Option<&str>,
        text: &str,
        submit: bool,
    ) -> Result<String, String> {
        let page = self.active_page()?.clone();
        if let Some(raw) = ref_id {
            let key = snapshot::parse_ref(raw).ok_or_else(|| "missing ref".to_string())?;
            let entry = self
                .refs
                .get(&key)
                .cloned()
                .ok_or_else(|| format!("unknown ref `{key}` — call browser_snapshot first"))?;
            let backend = snapshot::backend_id(&entry)
                .ok_or_else(|| format!("ref `{key}` has no backend DOM node"))?;
            page.execute(
                ScrollIntoViewIfNeededParams::builder()
                    .backend_node_id(backend.clone())
                    .build(),
            )
            .await
            .map_err(|e| e.to_string())?;
            page.execute(
                FocusParams::builder()
                    .backend_node_id(backend)
                    .build(),
            )
            .await
            .map_err(|e| format!("focus: {e}"))?;
        }

        page.execute(InsertTextParams::new(text))
            .await
            .map_err(|e| format!("type: {e}"))?;

        if submit {
            // Enter
            let down = DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyDown)
                .key("Enter")
                .code("Enter")
                .windows_virtual_key_code(13)
                .native_virtual_key_code(13)
                .build()
                .map_err(|e| e.to_string())?;
            page.execute(down).await.map_err(|e| e.to_string())?;
            let up = DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyUp)
                .key("Enter")
                .code("Enter")
                .windows_virtual_key_code(13)
                .native_virtual_key_code(13)
                .build()
                .map_err(|e| e.to_string())?;
            page.execute(up).await.map_err(|e| e.to_string())?;
            self.refs.clear();
        }

        Ok(format!(
            "typed {} chars{}",
            text.chars().count(),
            if submit { " + Enter" } else { "" }
        ))
    }

    pub async fn screenshot(&self, full_page: bool) -> Result<PathBuf, String> {
        let page = self.active_page()?.clone();
        let dir = browser_screenshot_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("screenshot dir {}: {e}", dir.display()))?;
        let path = dir.join(format!(
            "shot-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let params = ScreenshotParams::builder().full_page(full_page).build();
        page.save_screenshot(params, &path)
            .await
            .map_err(|e| format!("screenshot: {e}"))?;
        Ok(path)
    }

    /// Sync-friendly tab rows for the `/browser` cockpit cache.
    pub async fn tab_infos(&self) -> Vec<super::BrowserTabInfo> {
        let mut out = Vec::with_capacity(self.pages.len());
        for (i, p) in self.pages.iter().enumerate() {
            let u = p.url().await.ok().flatten().unwrap_or_default();
            out.push(super::BrowserTabInfo {
                index: i,
                url: u,
                active: i == self.active,
            });
        }
        out
    }

    pub async fn tabs(
        &mut self,
        action: &str,
        index: Option<usize>,
        url: Option<&str>,
    ) -> Result<String, String> {
        match action {
            "list" | "" => {
                let mut lines = Vec::new();
                for (i, p) in self.pages.iter().enumerate() {
                    let u = p.url().await.ok().flatten().unwrap_or_default();
                    let mark = if i == self.active { "*" } else { " " };
                    lines.push(format!("{mark} [{i}] {u}"));
                }
                Ok(if lines.is_empty() {
                    "(no tabs)".into()
                } else {
                    lines.join("\n")
                })
            }
            "new" => {
                let u = url.unwrap_or("about:blank");
                let page = self
                    .browser
                    .new_page(u)
                    .await
                    .map_err(|e| format!("new tab: {e}"))?;
                self.pages.push(page);
                self.active = self.pages.len() - 1;
                self.refs.clear();
                Ok(format!("opened tab {} -> {u}", self.active))
            }
            "switch" => {
                let i = index.ok_or_else(|| "switch requires index".to_string())?;
                if i >= self.pages.len() {
                    return Err(format!("tab index {i} out of range (0..{})", self.pages.len()));
                }
                let page = &self.pages[i];
                let tid = page.target_id().clone();
                self.browser
                    .execute(ActivateTargetParams::new(tid))
                    .await
                    .map_err(|e| format!("activate tab: {e}"))?;
                let _ = page.bring_to_front().await;
                self.active = i;
                self.refs.clear();
                Ok(format!("switched to tab {i}"))
            }
            "close" => {
                let i = index.unwrap_or(self.active);
                if i >= self.pages.len() {
                    return Err(format!("tab index {i} out of range"));
                }
                if self.pages.len() == 1 {
                    return Err("cannot close the last tab; use browser_close".into());
                }
                let page = self.pages.remove(i);
                let tid = page.target_id().clone();
                let _ = self.browser.execute(CloseTargetParams::new(tid)).await;
                if self.active >= self.pages.len() {
                    self.active = self.pages.len().saturating_sub(1);
                } else if i < self.active {
                    self.active -= 1;
                }
                self.refs.clear();
                Ok(format!("closed tab {i}; active={}", self.active))
            }
            other => Err(format!("unknown tabs action `{other}` (list|new|switch|close)")),
        }
    }

    pub async fn shutdown(mut self) {
        self.refs.clear();
        let _ = self.browser.close().await;
        let _ = self.browser.wait().await;
        self.handler.abort();
    }
}

/// Independent profile under `$DOCK_HOME/browser/user-data` — never the user's default Chrome.
pub fn browser_user_data_dir() -> PathBuf {
    dock_home().join("browser").join("user-data")
}

pub fn browser_screenshot_dir() -> PathBuf {
    dock_home().join("browser").join("screenshots")
}

/// Discover Chromium/Chrome binary. Prefers `CHROME_PATH`, then chromiumoxide `CHROME`, then PATH.
pub fn discover_chrome() -> Result<String, String> {
    for key in ["CHROME_PATH", "CHROME"] {
        if let Ok(p) = std::env::var(key) {
            let path = PathBuf::from(&p);
            if path.is_file() {
                return Ok(p);
            }
            return Err(format!(
                "{key}={p} is set but not an executable file. Install Chromium or fix the path."
            ));
        }
    }

    // chromiumoxide defaults (DetectionOptions) + a few explicit common paths.
    match chromiumoxide::detection::default_executable(Default::default()) {
        Ok(p) => Ok(p.display().to_string()),
        Err(e) => {
            let common = [
                "/usr/bin/google-chrome-stable",
                "/usr/bin/google-chrome",
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
                "/snap/bin/chromium",
            ];
            for c in common {
                if Path::new(c).is_file() {
                    return Ok(c.into());
                }
            }
            Err(format!(
                "Chromium/Chrome not found ({e}). Set CHROME_PATH or install google-chrome / chromium."
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_data_under_dock_home() {
        let prev = std::env::var_os("DOCK_HOME");
        std::env::set_var("DOCK_HOME", "/tmp/dock-test-home-browser");
        let p = browser_user_data_dir();
        assert!(p.ends_with("browser/user-data"), "{p:?}");
        assert!(!p.to_string_lossy().contains(".config/google-chrome"));
        match prev {
            Some(v) => std::env::set_var("DOCK_HOME", v),
            None => std::env::remove_var("DOCK_HOME"),
        }
    }

    #[test]
    fn discover_mentions_chrome_path_on_bad_env() {
        let prev = std::env::var_os("CHROME_PATH");
        std::env::set_var("CHROME_PATH", "/no/such/chrome-binary-xyz");
        let err = discover_chrome().unwrap_err();
        assert!(err.contains("CHROME_PATH"), "{err}");
        match prev {
            Some(v) => std::env::set_var("CHROME_PATH", v),
            None => std::env::remove_var("CHROME_PATH"),
        }
    }
}
