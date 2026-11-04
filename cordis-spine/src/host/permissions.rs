//! Live-looked-up permission gate. Tools call [`Permissions::request`]; the TUI
//! resolves the front of the queue. Do not capture this Arc in a long-lived closure.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use cordis::{plugin, Context, Inject, Plugin};
use tokio::sync::oneshot;

use crate::host::settings::{AppSettings, PermissionMode};
use crate::names::{PERMISSIONS, PERMISSION_EVENT, SETTINGS, SUBAGENT_CHANGED};
use crate::tools::task::SubagentChanged;
use cordis_base::acp::PermissionOptionKind;

#[derive(Clone, Debug)]
pub struct PermissionPrompt {
    pub tool: String,
    pub summary: String,
    /// 发起这次请求的子代理（agent id）；主会话自己发的是 `None`。子代理和派它的那一页
    /// 共用同一个队列、同一份「始终允许」；档位可以单独设（[`Permissions::set_agent_mode`]）。
    pub agent_id: Option<String>,
}

struct Pending {
    /// 入队序号，见 [`Permissions::front_seq`]。
    seq: u64,
    prompt: PermissionPrompt,
    /// 只读场景的请求（[`Permissions::request_strict`]）：「始终允许」不替它点头。
    strict: bool,
    tx: oneshot::Sender<PermissionOptionKind>,
}

pub struct Permissions {
    ctx: Context,
    queue: Mutex<VecDeque<Pending>>,
    always: Mutex<HashSet<String>>,
    never: Mutex<HashSet<String>>,
    /// 单独设过档位的子代理（agent id → 档位）。没设的跟这一页的 [`AppSettings`] 走。
    /// 只在内存里：同一页里同 id 接着聊还认，Dock 重启后回到跟随。
    agent_modes: Mutex<HashMap<String, PermissionMode>>,
    last_resolve: Mutex<Option<PermissionOptionKind>>,
    next_seq: std::sync::atomic::AtomicU64,
}

impl Permissions {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            queue: Mutex::new(VecDeque::new()),
            always: Mutex::new(HashSet::new()),
            never: Mutex::new(HashSet::new()),
            agent_modes: Mutex::new(HashMap::new()),
            last_resolve: Mutex::new(None),
            next_seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// 队首那条请求的入队序号（这个队列里单调递增）。事件载荷是 `()`，同一条请求
    /// 还挂着时也可能再收到事件；投影方拿它判断「队首换没换」，不重复报同一条。
    pub fn front_seq(&self) -> Option<u64> {
        self.queue.lock().unwrap().front().map(|p| p.seq)
    }

    pub fn front(&self) -> Option<PermissionPrompt> {
        self.queue.lock().unwrap().front().map(|p| p.prompt.clone())
    }

    /// Last decision that actually popped the queue. Gateway projects this
    /// when `front()` is empty after a TUI (or web) resolve.
    pub fn last_resolve(&self) -> Option<PermissionOptionKind> {
        *self.last_resolve.lock().unwrap()
    }

    /// Resolve the front of the queue. Returns `false` when empty so a second
    /// resolver (TUI vs web) can fail closed instead of auto-allowing.
    pub fn resolve(&self, kind: PermissionOptionKind) -> bool {
        let mut queue = self.queue.lock().unwrap();
        let Some(pending) = queue.pop_front() else {
            return false;
        };
        let tool = pending.prompt.tool.clone();
        // 「始终 / 永不」对队列里同一颗工具已经排着的请求同样生效（一批只读命令并发跑时
        // 会一下排好几条），不然用户刚选了「始终允许」，后面几条还要一条条点。只读场景
        // 的请求不吃「始终允许」，照旧留在队列里等人点头；「永不」它也认。
        let mut also = Vec::new();
        match kind {
            PermissionOptionKind::AllowAlways => {
                self.always.lock().unwrap().insert(tool.clone());
                queue.retain_mut(|p| {
                    let same = p.prompt.tool == tool && !p.strict;
                    if same {
                        also.push(std::mem::replace(&mut p.tx, oneshot::channel().0));
                    }
                    !same
                });
            }
            PermissionOptionKind::RejectAlways => {
                self.never.lock().unwrap().insert(tool.clone());
                queue.retain_mut(|p| {
                    let same = p.prompt.tool == tool;
                    if same {
                        also.push(std::mem::replace(&mut p.tx, oneshot::channel().0));
                    }
                    !same
                });
            }
            _ => {}
        }
        drop(queue);
        *self.last_resolve.lock().unwrap() = Some(kind);
        let _ = pending.tx.send(kind);
        for tx in also {
            let _ = tx.send(kind);
        }
        self.ctx.emit(PERMISSION_EVENT, ());
        true
    }

    /// 子代理 `agent_id` 单独设的档位；`None` = 跟这一页走。
    pub fn agent_mode(&self, agent_id: &str) -> Option<PermissionMode> {
        self.agent_modes.lock().unwrap().get(agent_id).copied()
    }

    /// 给一个子代理单独设档位（`None` = 回到跟这一页走）。设完它要是落到自动批准，
    /// 它已经排着的请求一并处理掉：普通的放行，只读场景的拒绝（见 [`Self::ask`]）。
    /// 发 [`SUBAGENT_CHANGED`]，各端的子代理投影跟着刷新。
    pub fn set_agent_mode(&self, agent_id: &str, mode: Option<PermissionMode>) {
        {
            let mut modes = self.agent_modes.lock().unwrap();
            match mode {
                Some(mode) => modes.insert(agent_id.to_string(), mode),
                None => modes.remove(agent_id),
            };
        }
        if self.effective_mode(Some(agent_id)) == PermissionMode::Allow {
            let mut settled = Vec::new();
            self.queue.lock().unwrap().retain_mut(|p| {
                let mine = p.prompt.agent_id.as_deref() == Some(agent_id);
                if mine {
                    let tx = std::mem::replace(&mut p.tx, oneshot::channel().0);
                    settled.push((tx, !p.strict));
                }
                !mine
            });
            if !settled.is_empty() {
                for (tx, allow) in settled {
                    let _ = tx.send(if allow {
                        PermissionOptionKind::AllowOnce
                    } else {
                        PermissionOptionKind::RejectOnce
                    });
                }
                self.ctx.emit(PERMISSION_EVENT, ());
            }
        }
        self.ctx.emit(
            SUBAGENT_CHANGED,
            SubagentChanged {
                id: agent_id.into(),
            },
        );
    }

    /// 这次请求实际按哪一档：子代理单独设过的优先，否则这一页的。
    fn effective_mode(&self, agent_id: Option<&str>) -> PermissionMode {
        agent_id
            .and_then(|id| self.agent_mode(id))
            .or_else(|| {
                self.ctx
                    .get::<AppSettings>(SETTINGS)
                    .map(|s| s.permission_mode())
            })
            .unwrap_or(PermissionMode::Ask)
    }

    pub async fn request(&self, tool: &str, summary: &str) -> bool {
        self.ask(tool, summary, None, false).await
    }

    /// 同 [`Self::request`]，并记下是哪个子代理发的（`agent_id`，展示用）。
    pub async fn request_from(&self, tool: &str, summary: &str, agent_id: Option<String>) -> bool {
        self.ask(tool, summary, agent_id, false).await
    }

    /// 只读场景（计划模式、只读子代理、旁问页）里一条可能改东西的 bash：一定要人点头。
    /// 自动批准和「以后都允许」都不算数——它们是在普通模式下给的；「以后都拒绝」照旧生效。
    /// 例外：子代理落在自动批准上（单独设的或跟这一页的）时直接拒绝、不弹——用户选了
    /// 不想被它打扰，只读的保证也还在。
    pub async fn request_strict(
        &self,
        tool: &str,
        summary: &str,
        agent_id: Option<String>,
    ) -> bool {
        self.ask(tool, summary, agent_id, true).await
    }

    async fn ask(&self, tool: &str, summary: &str, agent_id: Option<String>, strict: bool) -> bool {
        let auto = self.effective_mode(agent_id.as_deref()) == PermissionMode::Allow;
        if strict {
            if auto && agent_id.is_some() {
                return false;
            }
        } else if auto || self.always.lock().unwrap().contains(tool) {
            return true;
        }
        if self.never.lock().unwrap().contains(tool) {
            return false;
        }
        let (tx, rx) = oneshot::channel();
        let seq = self
            .next_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        self.queue.lock().unwrap().push_back(Pending {
            seq,
            strict,
            prompt: PermissionPrompt {
                tool: tool.to_string(),
                summary: summary.to_string(),
                agent_id,
            },
            tx,
        });
        self.ctx.emit(PERMISSION_EVENT, ());
        // 等的一方可能被丢掉（这一轮被停掉，工具的 future 跟着没了）：请求要跟着撤出队列，
        // 不然它一直挂在队首，后面的真请求被它挡着，各端显示的还是它的摘要。
        let _withdraw = Withdraw { perms: self, seq };
        rx.await.ok().is_some_and(PermissionOptionKind::is_allow)
    }

    /// 撤掉一条还在队列里的请求（等它的一方已经不在了）。已经被处理掉的什么也不做。
    fn withdraw(&self, seq: u64) {
        let mut queue = self.queue.lock().unwrap();
        let before = queue.len();
        queue.retain(|p| p.seq != seq);
        if queue.len() == before {
            return;
        }
        drop(queue);
        // 投影方在队首换了时按 `last_resolve` 报上一条的结局：撤掉的就是没批准。
        *self.last_resolve.lock().unwrap() = Some(PermissionOptionKind::RejectOnce);
        self.ctx.emit(PERMISSION_EVENT, ());
    }
}

/// [`Permissions::ask`] 的守卫：future 被丢掉时把自己那条请求撤出队列。
struct Withdraw<'a> {
    perms: &'a Permissions,
    seq: u64,
}

impl Drop for Withdraw<'_> {
    fn drop(&mut self) {
        self.perms.withdraw(self.seq);
    }
}

pub fn permissions() -> Plugin {
    plugin("permissions", Inject::new(), |ctx, _: &()| {
        Ok(Some(
            ctx.provide(PERMISSIONS, Permissions::new(ctx.clone()))?,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis::Context;

    /// 回归：同一颗工具排着几条请求（一批只读 bash 并发跑时就是这样），用户对第一条选
    /// 「始终允许」后，后面几条以前还要一条条再点。现在跟着一起放行；只读场景（严格）的不算。
    #[tokio::test]
    async fn allow_always_releases_the_rest_of_the_queue_for_that_tool() {
        let ctx = Context::new();
        let perms = std::sync::Arc::new(Permissions::new(ctx));
        let ask = |tool: &'static str, strict: bool| {
            let perms = perms.clone();
            tokio::spawn(async move { perms.ask(tool, "s", None, strict).await })
        };
        let first = ask("bash", false);
        let second = ask("bash", false);
        let strict = ask("bash", true);
        let other = ask("write_file", false);
        while perms.queue.lock().unwrap().len() < 4 {
            tokio::task::yield_now().await;
        }
        assert!(perms.resolve(PermissionOptionKind::AllowAlways));
        let within = |h: tokio::task::JoinHandle<bool>| async move {
            tokio::time::timeout(std::time::Duration::from_secs(2), h)
                .await
                .ok()
                .and_then(Result::ok)
        };
        assert_eq!(within(first).await, Some(true));
        assert_eq!(
            within(second).await,
            Some(true),
            "同一颗工具排着的那条要跟着放行"
        );
        assert_eq!(
            perms.front().map(|p| p.tool),
            Some("bash".to_string()),
            "只读场景的那条照旧要人点头"
        );
        assert!(perms.resolve(PermissionOptionKind::RejectOnce));
        assert!(!strict.await.unwrap());
        assert_eq!(perms.front().map(|p| p.tool), Some("write_file".into()));
        assert!(perms.resolve(PermissionOptionKind::AllowOnce));
        assert!(other.await.unwrap());
    }

    /// 回归：这一轮在等权限时被停掉（工具的 future 被丢掉），那条请求以前一直挂在队首：
    /// 下一条真请求排在它后面，各端显示的还是旧那条的摘要（批准的不是眼前这条命令），
    /// 点「允许」也只是放掉那条死请求。现在等的一方没了，请求跟着撤出队列。
    #[tokio::test]
    async fn abandoned_request_leaves_the_queue() {
        let ctx = Context::new();
        let perms = std::sync::Arc::new(Permissions::new(ctx));
        let stale = {
            let perms = perms.clone();
            tokio::spawn(async move { perms.ask("bash", "$ echo hello", None, false).await })
        };
        while perms.front().is_none() {
            tokio::task::yield_now().await;
        }
        stale.abort();
        let _ = stale.await;
        assert!(perms.front().is_none(), "停掉的那一轮的请求要撤掉");
        assert_eq!(
            perms.last_resolve(),
            Some(PermissionOptionKind::RejectOnce),
            "撤掉的请求按「没批准」报，不沿用上一次的决定"
        );

        let real = {
            let perms = perms.clone();
            tokio::spawn(async move { perms.ask("bash", "$ rm -rf build", None, false).await })
        };
        while perms.front().is_none() {
            tokio::task::yield_now().await;
        }
        assert_eq!(perms.front().unwrap().summary, "$ rm -rf build");
        assert!(perms.resolve(PermissionOptionKind::AllowOnce));
        assert!(real.await.unwrap(), "点的「允许」给的是眼前这条");
    }

    async fn within(h: tokio::task::JoinHandle<bool>) -> Option<bool> {
        tokio::time::timeout(std::time::Duration::from_secs(2), h)
            .await
            .ok()
            .and_then(Result::ok)
    }

    /// 一页：给定档位的设置 + 它的权限队列。守卫拿着，设置才一直在。
    fn page(mode: PermissionMode) -> (cordis::Disposable, std::sync::Arc<Permissions>) {
        let ctx = Context::new();
        let settings = AppSettings::new("x");
        settings.set_permission_mode(mode);
        let guard = ctx.provide(SETTINGS, settings).unwrap();
        (guard, std::sync::Arc::new(Permissions::new(ctx)))
    }

    fn ask_as(
        perms: &std::sync::Arc<Permissions>,
        agent: Option<&str>,
        strict: bool,
    ) -> tokio::task::JoinHandle<bool> {
        let perms = perms.clone();
        let agent = agent.map(str::to_string);
        tokio::spawn(async move { perms.ask("bash", "$ touch x", agent, strict).await })
    }

    /// 子代理单独设的档位盖过这一页的：页是自动批准、它设成请求批准就照样问；
    /// 没设的子代理跟页走。
    #[tokio::test]
    async fn agent_mode_overrides_the_page() {
        let (_page, perms) = page(PermissionMode::Allow);
        perms.set_agent_mode("kid-ask", Some(PermissionMode::Ask));
        assert_eq!(
            within(ask_as(&perms, Some("kid-auto"), false)).await,
            Some(true)
        );
        let asked = ask_as(&perms, Some("kid-ask"), false);
        while perms.front().is_none() {
            tokio::task::yield_now().await;
        }
        assert_eq!(perms.front().unwrap().agent_id.as_deref(), Some("kid-ask"));
        assert!(perms.resolve(PermissionOptionKind::RejectOnce));
        assert_eq!(within(asked).await, Some(false));

        // 反过来：页是请求批准，单独设成自动批准的子代理不问。
        let (_page, perms) = page(PermissionMode::Ask);
        perms.set_agent_mode("kid", Some(PermissionMode::Allow));
        assert_eq!(within(ask_as(&perms, Some("kid"), false)).await, Some(true));
        perms.set_agent_mode("kid", None);
        assert_eq!(perms.agent_mode("kid"), None);
        let _asked = ask_as(&perms, Some("kid"), false);
        while perms.front().is_none() {
            tokio::task::yield_now().await;
        }
    }

    /// 回归：主会话开着自动批准，只读子代理（探索）跑会改东西的 bash 还是弹窗——用户
    /// 选了不想被打扰却照样被打扰。现在落在自动批准上的子代理直接拒（只读的保证还在）；
    /// 主会话自己的只读场景（计划模式）照旧问。
    #[tokio::test]
    async fn strict_request_from_an_auto_subagent_is_refused_without_asking() {
        let (_page, perms) = page(PermissionMode::Allow);
        assert_eq!(
            within(ask_as(&perms, Some("explore"), true)).await,
            Some(false)
        );
        assert!(perms.front().is_none(), "不该进队列");

        perms.set_agent_mode("careful", Some(PermissionMode::Ask));
        let careful = ask_as(&perms, Some("careful"), true);
        let main = ask_as(&perms, None, true);
        while perms.queue.lock().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
        assert!(perms.resolve(PermissionOptionKind::AllowOnce));
        assert!(perms.resolve(PermissionOptionKind::AllowOnce));
        assert_eq!(within(careful).await, Some(true));
        assert_eq!(within(main).await, Some(true));
    }

    /// 把一个子代理切到自动批准时，它已经排着的请求一并处理掉（普通的放行、只读的拒绝），
    /// 别人的留着。
    #[tokio::test]
    async fn switching_an_agent_to_auto_settles_its_queued_requests() {
        let (_page, perms) = page(PermissionMode::Ask);
        let plain = ask_as(&perms, Some("kid"), false);
        let strict = ask_as(&perms, Some("kid"), true);
        let other = ask_as(&perms, Some("other"), false);
        while perms.queue.lock().unwrap().len() < 3 {
            tokio::task::yield_now().await;
        }
        perms.set_agent_mode("kid", Some(PermissionMode::Allow));
        assert_eq!(within(plain).await, Some(true));
        assert_eq!(within(strict).await, Some(false));
        assert_eq!(perms.front().unwrap().agent_id.as_deref(), Some("other"));
        assert!(perms.resolve(PermissionOptionKind::AllowOnce));
        assert_eq!(within(other).await, Some(true));
    }

    #[test]
    fn resolve_returns_false_when_empty() {
        let ctx = Context::new();
        let perms = Permissions::new(ctx);
        assert!(!perms.resolve(PermissionOptionKind::AllowOnce));
        assert!(perms.last_resolve().is_none());
    }
}
