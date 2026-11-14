//! 浏览器授权回调页：ChatGPT 订阅登录与 MCP 服务器 OAuth 共用。
//!
//! 页面分两半发：收到回调就先发**前半页**（「正在连接…」，见 `callback_page.html`），
//! 连接不关；换码有了结果再补一行 `settle({...})` 把页面切到真实结果，然后关连接。
//! 这样浏览器那一页说的是真话（连上了哪个账号、套餐额度开没开、还要不要再授权一轮），
//! 又不用让页面另开一个接口轮询。
//!
//! 调用方没 `finish` 就把 [`CallbackPage`] 丢了（登录被取消、出错、进程退出），
//! 连接随之关闭；页面发现文档加载完了还没等到结果，就显示「这次登录中断了」。

use serde::Serialize;
use tokio::io::AsyncWriteExt;

use super::oauth::html_escape;

/// 前半页模板。`{{SERVICE}}` 由 [`head`] 转义后填入。
const TEMPLATE: &str = include_str!("callback_page.html");

/// 页面最后停在哪种状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PageState {
    /// 连上了。
    Ok,
    /// 这一轮完成了，但还要再授权一轮（浏览器会打开下一页）。
    Next,
    /// 没连上。
    Fail,
}

/// 小标签的语气（颜色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Tone {
    Ok,
    Warn,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Chip {
    text: String,
    tone: Tone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Link {
    text: String,
    href: String,
}

/// 浏览器那一页最后显示什么。只有纯文本：页面用 `textContent` 填，不解析 HTML。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PageOutcome {
    state: PageState,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    chips: Vec<Chip>,
    /// 说明。反引号里的部分显示成命令胶囊（`/auth login` 这种）。
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    /// 「接下来可以」：左对齐的一列，同样认反引号。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    steps: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    link: Option<Link>,
    foot: String,
}

impl PageOutcome {
    fn new(state: PageState, title: String, foot: &str) -> Self {
        Self {
            state,
            title,
            account: None,
            chips: Vec::new(),
            detail: None,
            steps: Vec::new(),
            link: None,
            foot: foot.to_string(),
        }
    }

    /// 「已连接 {service}」。
    pub(crate) fn connected(service: &str) -> Self {
        Self::new(
            PageState::Ok,
            format!("已连接 {service}"),
            "回到 Dock 继续 · 这一页可以关掉了",
        )
    }

    /// 这一轮完成、浏览器马上打开下一页授权。
    pub(crate) fn next_step(title: &str) -> Self {
        Self::new(
            PageState::Next,
            title.to_string(),
            "下一页打开后，这一页就可以关掉了",
        )
    }

    /// 「没有连上 {service}」，`reason` 是给人看的原因与下一步。
    pub(crate) fn failed(service: &str, reason: impl Into<String>) -> Self {
        Self::new(
            PageState::Fail,
            format!("没有连上 {service}"),
            "回到 Dock 看详情 · 需要的话重新登录",
        )
        .detail(reason)
    }

    pub(crate) fn account(mut self, account: Option<String>) -> Self {
        self.account = account.filter(|a| !a.is_empty());
        self
    }

    pub(crate) fn chip(mut self, text: &str, tone: Tone) -> Self {
        self.chips.push(Chip {
            text: text.to_string(),
            tone,
        });
        self
    }

    pub(crate) fn detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        self.detail = (!detail.is_empty()).then_some(detail);
        self
    }

    pub(crate) fn step(mut self, step: &str) -> Self {
        self.steps.push(step.to_string());
        self
    }

    /// 页面上的按钮链接。只收 https（页面那头也会再挡一次）。
    pub(crate) fn link(mut self, text: &str, href: &str) -> Self {
        if href.starts_with("https://") {
            self.link = Some(Link {
                text: text.to_string(),
                href: href.to_string(),
            });
        }
        self
    }
}

/// HTTP 头。没有 `Content-Length`：正文一直到连接关闭，前半页先到先画。
///
/// `Referrer-Policy: no-referrer`：地址里带着授权码，点页面上的链接不能把它带出去。
fn http_head(status: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:\r\n\
         Connection: close\r\n\r\n"
    )
}

/// 前半页：停在「正在连接 {service}…」。
pub(crate) fn head(service: &str) -> String {
    TEMPLATE.replace("{{SERVICE}}", &html_escape(service))
}

/// 后半页：把页面切到 `outcome`，收尾。
///
/// JSON 嵌在 `<script>` 里：`<` 一律转成 `\u003c`，内容里再怎么写 `</script>`
/// 也跳不出去；U+2028 / U+2029 同样转义。
pub(crate) fn tail(outcome: &PageOutcome) -> String {
    let json = serde_json::to_string(outcome)
        .unwrap_or_else(|_| "{\"state\":\"fail\",\"title\":\"\",\"foot\":\"\"}".into())
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    format!("<script>settle({json});</script>\n</body>\n</html>\n")
}

/// 已经发出前半页、还开着的浏览器连接。
#[derive(Debug)]
pub(crate) struct CallbackPage {
    stream: tokio::net::TcpStream,
}

impl CallbackPage {
    /// 发前半页（200），连接留着等结果。
    ///
    /// 写失败（浏览器已经把这一页关了）不影响登录：回调参数已经拿到了，照常换码。
    pub(crate) async fn open(mut stream: tokio::net::TcpStream, service: &str) -> Self {
        let first = format!("{}{}", http_head("200 OK"), head(service));
        let _ = stream.write_all(first.as_bytes()).await;
        let _ = stream.flush().await;
        Self { stream }
    }

    /// 回调本身就不对（被拒、state 对不上、缺 code）：一次发完整页（400）。
    pub(crate) async fn reject(
        mut stream: tokio::net::TcpStream,
        service: &str,
        outcome: &PageOutcome,
    ) {
        let page = format!(
            "{}{}{}",
            http_head("400 Bad Request"),
            head(service),
            tail(outcome)
        );
        let _ = stream.write_all(page.as_bytes()).await;
        let _ = stream.shutdown().await;
    }

    /// 把页面切到最后结果并关连接。浏览器已经关了这一页也无所谓。
    pub(crate) async fn finish(mut self, outcome: &PageOutcome) {
        let _ = self.stream.write_all(tail(outcome).as_bytes()).await;
        let _ = self.stream.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_name_is_escaped() {
        let page = head("<img src=x onerror=alert(1)>");
        assert!(!page.contains("<img src=x"), "服务名要转义");
        assert!(page.contains("正在连接 &lt;img"), "标题里是转义后的服务名");
        assert!(!page.contains("{{SERVICE}}"));
    }

    /// 上游给的错误描述、账号名都可能带 `</script>`：拼进脚本也跳不出去。
    #[test]
    fn the_outcome_cannot_break_out_of_the_script() {
        let outcome = PageOutcome::failed("ChatGPT", "</script><script>alert(1)</script>\u{2028}");
        let tail = tail(&outcome);
        assert_eq!(tail.matches("</script>").count(), 1, "{tail}");
        assert!(tail.contains("\\u003c/script>"), "{tail}");
        assert!(tail.contains("\\u2028"), "{tail}");
    }

    #[test]
    fn the_outcome_carries_what_the_page_shows() {
        let outcome = PageOutcome::connected("ChatGPT")
            .account(Some("a@b.c".into()))
            .chip("套餐额度已开", Tone::Ok)
            .detail("说明")
            .step("运行 `/auth login`")
            .link("Manage usage", "https://chatgpt.com/settings/usage");
        let v: serde_json::Value = serde_json::from_str(
            tail(&outcome)
                .split_once("settle(")
                .unwrap()
                .1
                .split_once(");</script>")
                .unwrap()
                .0,
        )
        .unwrap();
        assert_eq!(v["state"], "ok");
        assert_eq!(v["title"], "已连接 ChatGPT");
        assert_eq!(v["account"], "a@b.c");
        assert_eq!(v["chips"][0]["tone"], "ok");
        assert_eq!(v["steps"][0], "运行 `/auth login`");
        assert_eq!(v["link"]["href"], "https://chatgpt.com/settings/usage");

        // 不是 https 的链接不收。
        let plain = PageOutcome::connected("x").link("x", "javascript:alert(1)");
        assert!(plain.link.is_none());
        // 空账号、空说明都不显示。
        let empty = PageOutcome::connected("x")
            .account(Some(String::new()))
            .detail("");
        assert!(empty.account.is_none() && empty.detail.is_none());
    }
}
