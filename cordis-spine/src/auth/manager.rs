//! 鉴权服务本体（named service `"auth"`）。
//!
//! 管三样东西：已注册的 provider、正在跑的登录（每家同时一个，可取消）、
//! 推理时学到的「这条通道不收的字段」。**账号状态不在这里**——那归 provider，
//! 每次都读盘：另一个 Dock 进程登录 / 登出 / 切账号，这边不重启就看得到。
//!
//! 有变化（登录开始 / 结束、取消、登出、切账号、导入）就发 [`AUTH_CHANGED`]，
//! 载荷是 provider id；网关据此推 `auth/changed`，界面重拉 `auth/status`。
//!
//! 还管**自动发现的模型**：登录成功、切账号、导入、登出之后（以及启动时缓存
//! 过期了）向 provider 要一次活动账号的模型，写进 `$DOCK_HOME/auth_models.json`，
//! 读目录时并进来（见 `cordis_base::config::auth_models`）。活动账号登出了、
//! 没开套餐额度，就把那一家的条目删掉——列表里不留用不了的模型。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use cordis::{plugin, Context, Disposable, Inject, Plugin};

use cordis_base::auth::{
    AccountKey, AccountSummary, AuthError, AuthProvider, Lease, LoginRequest, LoginUi,
    LogoutOutcome, RemoteModel,
};
use cordis_base::config::auth_models::{self, AuthModelCache, ProviderModels};

use crate::names::{AUTH, AUTH_CHANGED};

/// 一次登录走到哪了。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginState {
    /// 等用户在浏览器里点同意。`url` = 授权页（拿到之前是 `None`）。
    Pending { url: Option<String> },
    /// 没成（超时、被拒、换码失败……）：给人看的那句话。
    Failed { message: String },
    /// 成了。
    Succeeded { account: AccountKey },
}

struct LoginSlot {
    /// 这次登录的序号：旧任务的结果不许盖掉新任务的状态。
    id: u64,
    state: LoginState,
    abort: Option<tokio::task::AbortHandle>,
}

/// `(provider, 发给上游的模型名)` → 那条通道明确说过不收的字段。
type UnsupportedParams = BTreeMap<(String, String), BTreeSet<String>>;

/// 统一鉴权服务。
///
/// provider 插件（`chatgpt_auth` 之类）在启动时把自己注册进来；模型条目的
/// `auth = "..."` 按 id 找实现。**没装那个插件 = 那条模型不可用**，不是崩。
#[derive(Clone, Default)]
pub struct AuthManager {
    providers: Arc<RwLock<BTreeMap<String, Arc<dyn AuthProvider>>>>,
    logins: Arc<Mutex<BTreeMap<String, LoginSlot>>>,
    next_login: Arc<AtomicU64>,
    /// `(provider, 发给上游的模型名)` → 上游明确说过不收的字段（`error.param`）。
    /// 记下来，下一次请求一开始就不发，免得每轮都先撞一次 400。
    unsupported: Arc<Mutex<UnsupportedParams>>,
    /// 发 [`AUTH_CHANGED`] 用。测试里裸建的没有。
    ctx: Option<Context>,
    /// 自动发现的模型写到哪（`$DOCK_HOME/auth_models.json`）。挂载时定下，之后环境
    /// 变量变了也不跟。`None` = 不写也不拉——测试里裸建的服务不碰真实的 DOCK_HOME。
    models_cache: Option<PathBuf>,
    /// 改那个文件时持着：本进程两次刷新的读-改-写不交错。
    models_lock: Arc<Mutex<()>>,
}

impl AuthManager {
    /// 带事件出口的服务（插件挂载时用）。
    pub fn with_events(ctx: Context) -> Self {
        Self {
            ctx: Some(ctx),
            models_cache: Some(auth_models::path()),
            ..Self::default()
        }
    }

    /// 测试用：自动发现的模型写到 `path`。
    #[cfg(test)]
    pub(crate) fn with_models_cache(mut self, path: PathBuf) -> Self {
        self.models_cache = Some(path);
        self
    }

    /// 注册一个 provider。同 id 后注册的盖掉先注册的（插件重挂时用得上）。
    ///
    /// 回的 `Disposable` 卸下这一份（已经被别的同 id provider 盖掉了就不动）。
    pub fn register(&self, provider: Arc<dyn AuthProvider>) -> Disposable {
        let id = provider.id().to_string();
        self.write().insert(id.clone(), provider.clone());
        self.changed(&id);
        let this = self.clone();
        Disposable::from_fn(move || {
            let mut providers = this.write();
            if providers
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &provider))
            {
                providers.remove(&id);
            }
            drop(providers);
            this.changed(&id);
        })
    }

    /// 按 id 找 provider。找不到 = 没装那个插件。
    pub fn provider(&self, id: &str) -> Option<Arc<dyn AuthProvider>> {
        self.read().get(id).cloned()
    }

    /// 已注册的 provider id（`/auth list` 用）。
    pub fn provider_ids(&self) -> Vec<String> {
        self.read().keys().cloned().collect()
    }

    /// 某个 provider 当前的活动账号（provider 读盘）。
    pub fn active_account(&self, provider: &str) -> Option<AccountKey> {
        self.provider(provider)?.active_account()
    }

    /// 某个 provider 的账号表（provider 读盘）。
    pub fn accounts(&self, provider: &str) -> Vec<AccountSummary> {
        self.provider(provider)
            .map(|p| p.accounts())
            .unwrap_or_default()
    }

    /// 按账号 id 在所有 provider 里找。
    pub fn find_account(&self, account_id: &str) -> Option<(String, AccountSummary)> {
        let providers: Vec<_> = self.read().values().cloned().collect();
        providers.into_iter().find_map(|p| {
            p.accounts()
                .into_iter()
                .find(|a| a.account.client_id == account_id)
                .map(|a| (p.id().to_string(), a))
        })
    }

    /// 切活动账号（`/auth use`）。
    pub fn set_active(&self, provider: &str, account_id: &str) -> Result<AccountKey, AuthError> {
        let impl_ = self.require(provider)?;
        let key = impl_.set_active(account_id)?;
        self.changed(provider);
        // 官方：换了账号就重拉可选模型（各账号的套餐不一样）。
        self.refresh_models_soon(provider);
        Ok(key)
    }

    /// 导入凭据（`/auth import`，VPS 场景）。
    pub fn import(&self, provider: &str, raw: &str) -> Result<AccountKey, AuthError> {
        let impl_ = self.require(provider)?;
        let key = impl_.import(raw)?;
        self.changed(provider);
        self.refresh_models_soon(provider);
        Ok(key)
    }

    /// 登出。`account_id` 不给 = 活动账号。回实际登出的那个账号。
    pub async fn logout(
        &self,
        provider: &str,
        account_id: Option<&str>,
        revoke: bool,
    ) -> Result<(AccountKey, LogoutOutcome), AuthError> {
        let impl_ = self.require(provider)?;
        let account = match account_id {
            Some(id) => impl_
                .accounts()
                .into_iter()
                .find(|a| a.account.client_id == id)
                .map(|a| a.account)
                .ok_or_else(|| AuthError::Rejected {
                    code: "unknown_account".into(),
                    detail: format!("本机没有这个账号：{id}"),
                })?,
            None => impl_.active_account().ok_or_else(|| AuthError::Rejected {
                code: "unknown_account".into(),
                detail: "当前没有登录的账号".into(),
            })?,
        };
        let was_active = impl_.active_account().as_ref() == Some(&account);
        let outcome = impl_.logout(&account, revoke).await;
        self.changed(provider);
        // 登出的是活动账号：它的模型从列表里拿掉（重拉时看到已登出就删条目）。
        if was_active {
            self.refresh_models_soon(provider);
        }
        Ok((account, outcome?))
    }

    /// 拿一份至少还能用 `min_valid` 的 access token。
    ///
    /// provider 没装 / 还没登录，都是 `NeedsLogin`——**缺插件也算「要登录」**：
    /// 用户看到的是「先登录」，而不是一串插件名。
    pub async fn lease(&self, provider: &str, min_valid: Duration) -> Result<Lease, AuthError> {
        let Some(impl_) = self.provider(provider) else {
            return Err(AuthError::NeedsLogin {
                reason: format!("没有装 {provider} 的鉴权插件"),
            });
        };
        let Some(acct) = impl_.active_account() else {
            return Err(AuthError::NeedsLogin {
                reason: format!("{} 还没登录：/auth login", impl_.info().display_name),
            });
        };
        impl_.lease(&acct, min_valid).await
    }

    /// 把上游的 HTTP 错误归一成 [`AuthError`]。不是鉴权问题就 `None`。
    pub fn classify(&self, provider: &str, status: u16, body: &[u8]) -> Option<AuthError> {
        self.provider(provider)?.classify(status, body)
    }

    /// 开始登录，**立刻返回**：授权要人在浏览器里点，可能一两分钟。
    ///
    /// 同一家同时只跑一个登录：再发起一次 = 掐掉旧的（它的回环监听随之关掉），
    /// 换成新的。TUI 的 `/auth` 和桌面端的按钮走的是同一个口子，不会两头各开一个。
    /// 失败时经 `ui.status` 报一句；结果也记在 [`Self::login_state`] 里。
    pub fn start_login(
        &self,
        provider: &str,
        req: LoginRequest,
        ui: Arc<dyn LoginUi>,
    ) -> Result<(), AuthError> {
        let impl_ = self.require(provider)?;
        let id = self.next_login.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut logins = self.logins();
            if let Some(abort) = logins.remove(provider).and_then(|old| old.abort) {
                abort.abort();
            }
            logins.insert(
                provider.to_string(),
                LoginSlot {
                    id,
                    state: LoginState::Pending { url: None },
                    abort: None,
                },
            );
        }
        let session = SessionUi {
            inner: ui,
            manager: self.clone(),
            provider: provider.to_string(),
            id,
        };
        let task = tokio::spawn(async move {
            let result = impl_.login(&session, req).await;
            let succeeded = result.is_ok();
            let state = match result {
                Ok(account) => LoginState::Succeeded { account },
                Err(e) => {
                    let message = e.message();
                    session.inner.status(&format!("登录失败：{message}"));
                    LoginState::Failed { message }
                }
            };
            session.manager.settle(&session.provider, session.id, state);
            if succeeded {
                session.manager.refresh_models_soon(&session.provider);
            }
        });
        if let Some(slot) = self.logins().get_mut(provider) {
            if slot.id == id {
                slot.abort = Some(task.abort_handle());
            }
        }
        self.changed(provider);
        Ok(())
    }

    /// 中断正在跑的登录。回「真的有一个在跑」。
    ///
    /// 掐掉那个任务就够了：凭据在换码成功之后才写盘，不留半截状态。
    pub fn cancel_login(&self, provider: &str) -> bool {
        let slot = self.logins().remove(provider);
        let cancelled = match slot {
            Some(slot) => {
                let pending = matches!(slot.state, LoginState::Pending { .. });
                if let Some(abort) = slot.abort {
                    abort.abort();
                }
                pending
            }
            None => false,
        };
        self.changed(provider);
        cancelled
    }

    /// 最近一次登录的状态（没登录过 / 取消了就是 `None`）。
    pub fn login_state(&self, provider: &str) -> Option<LoginState> {
        self.logins().get(provider).map(|s| s.state.clone())
    }

    /// 重拉 `provider` 活动账号能用的模型，写进 `auth_models.json`，回这份列表。
    ///
    /// 没有活动账号 / 登出了 / 没开套餐额度 / 刷新发现授权作废了 → 删掉那一家的
    /// 条目并回对应的错：列表里不留用不了的模型。网络之类的临时失败不动旧列表。
    pub async fn refresh_models(&self, provider: &str) -> Result<Vec<RemoteModel>, AuthError> {
        let impl_ = self.require(provider)?;
        let name = impl_.info().display_name;
        let active = impl_.accounts().into_iter().find(|a| a.active);
        let result = match &active {
            None => Err(AuthError::NeedsLogin {
                reason: format!("{name} 还没登录：/auth login"),
            }),
            Some(a) if !a.signed_in => Err(AuthError::NeedsLogin {
                reason: format!("{name} 已经登出了：/auth login"),
            }),
            Some(a) if !a.plan_usage => Err(AuthError::NotEligible {
                code: "missing_plan_usage_scope".into(),
            }),
            Some(a) => impl_.models(&a.account).await,
        };
        match (&result, &active) {
            (Ok(list), Some(a)) => {
                let entry = ProviderModels {
                    display_name: name.to_string(),
                    account: a.account.client_id.clone(),
                    fetched_at: crate::auth::credentials::now_unix(),
                    models: list.clone(),
                };
                let provider = provider.to_string();
                self.edit_models(move |cache| {
                    cache.providers.insert(provider, entry);
                    true
                });
            }
            (Err(AuthError::NeedsLogin { .. } | AuthError::NotEligible { .. }), _) => {
                self.forget_models(provider);
            }
            _ => {}
        }
        self.changed(provider);
        result
    }

    /// 缓存里 `provider` 那一家的模型（还没拉过就是 `None`）。
    pub fn cached_models(&self, provider: &str) -> Option<ProviderModels> {
        let path = self.models_cache.as_ref()?;
        AuthModelCache::read(path).providers.remove(provider)
    }

    /// 启动时用：缓存缺了、是别的账号的、超过 `max_age` 了才在后台重拉；
    /// 活动账号用不了（没登录 / 登出了 / 没开套餐额度）就直接删掉那一家的条目。
    pub fn refresh_models_if_stale(&self, provider: &str, max_age: Duration) {
        let Some(path) = &self.models_cache else {
            return;
        };
        let Some(impl_) = self.provider(provider) else {
            return;
        };
        let active = impl_.accounts().into_iter().find(|a| a.active);
        let cached = AuthModelCache::read(path).providers.remove(provider);
        match active {
            Some(a) if a.signed_in && a.plan_usage => {
                let now = crate::auth::credentials::now_unix();
                let fresh = cached.is_some_and(|c| {
                    c.account == a.account.client_id
                        && now.saturating_sub(c.fetched_at) < max_age.as_secs()
                });
                if !fresh {
                    self.refresh_models_soon(provider);
                }
            }
            _ if cached.is_some() => self.forget_models(provider),
            _ => {}
        }
    }

    /// 在后台 [`Self::refresh_models`]。没有缓存路径（测试里裸建）或不在 tokio 里就不做。
    fn refresh_models_soon(&self, provider: &str) {
        if self.models_cache.is_none() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let manager = self.clone();
        let provider = provider.to_string();
        runtime.spawn(async move {
            if let Err(e) = manager.refresh_models(&provider).await {
                tracing::info!(provider = provider.as_str(), error = %e.message(), "没拉到订阅通道的模型");
            }
        });
    }

    fn forget_models(&self, provider: &str) {
        let provider = provider.to_string();
        self.edit_models(move |cache| cache.providers.remove(&provider).is_some());
    }

    /// 读-改-写 `auth_models.json`。`edit` 回 `false` = 没改，不写盘。
    fn edit_models(&self, edit: impl FnOnce(&mut AuthModelCache) -> bool) {
        let Some(path) = &self.models_cache else {
            return;
        };
        let _guard = self.models_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut cache = AuthModelCache::read(path);
        if !edit(&mut cache) {
            return;
        }
        if let Err(e) = crate::auth::oauth::write_private(path, cache.to_json().as_bytes()) {
            tracing::warn!(path = %path.display(), error = %e, "auth_models.json 写不了");
        }
    }

    /// 记下「这条通道不收 `param`」（上游 400 `…_unsupported_capability` 说的）。
    pub fn remember_unsupported(&self, provider: &str, model: &str, param: &str) {
        self.unsupported
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry((provider.to_string(), model.to_string()))
            .or_default()
            .insert(param.to_string());
    }

    /// 这条通道、这个模型已知不收的字段。
    pub fn unsupported_params(&self, provider: &str, model: &str) -> Vec<String> {
        self.unsupported
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(provider.to_string(), model.to_string()))
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 登录任务结束：只有还是**这一次**的登录才写状态（被新登录顶掉的不算）。
    fn settle(&self, provider: &str, id: u64, state: LoginState) {
        {
            let mut logins = self.logins();
            match logins.get_mut(provider) {
                Some(slot) if slot.id == id => {
                    slot.state = state;
                    slot.abort = None;
                }
                _ => return,
            }
        }
        self.changed(provider);
    }

    /// 授权页地址到手了：记进状态（界面能显示、能复制）。
    fn record_url(&self, provider: &str, id: u64, url: &str) {
        {
            let mut logins = self.logins();
            match logins.get_mut(provider) {
                Some(slot) if slot.id == id => {
                    slot.state = LoginState::Pending {
                        url: Some(url.to_string()),
                    };
                }
                _ => return,
            }
        }
        self.changed(provider);
    }

    fn require(&self, provider: &str) -> Result<Arc<dyn AuthProvider>, AuthError> {
        self.provider(provider).ok_or_else(|| AuthError::Rejected {
            code: "unknown_provider".into(),
            detail: format!("没有装 {provider} 的鉴权插件"),
        })
    }

    fn changed(&self, provider: &str) {
        if let Some(ctx) = &self.ctx {
            ctx.emit(AUTH_CHANGED, provider.to_string());
        }
    }

    fn logins(&self) -> MutexGuard<'_, BTreeMap<String, LoginSlot>> {
        self.logins.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn read(&self) -> RwLockReadGuard<'_, BTreeMap<String, Arc<dyn AuthProvider>>> {
        // 锁中毒（某个线程 panic 时）也照常读：这里的锁只护一张小表，
        // 中毒不等于数据坏了。
        self.providers.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, BTreeMap<String, Arc<dyn AuthProvider>>> {
        self.providers.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// 登录任务用的界面：先把授权页地址记进状态，再转给调用方的界面。
struct SessionUi {
    inner: Arc<dyn LoginUi>,
    manager: AuthManager,
    provider: String,
    id: u64,
}

impl LoginUi for SessionUi {
    fn open_url(&self, url: &str) {
        self.manager.record_url(&self.provider, self.id, url);
        self.inner.open_url(url);
    }

    fn status(&self, message: &str) {
        self.inner.status(message);
    }
}

/// 把 `AuthManager` 挂成 named service `"auth"`。
pub fn auth_manager() -> Plugin {
    plugin("auth-manager", Inject::new(), |ctx, _: &()| {
        Ok(Some(
            ctx.provide(AUTH, AuthManager::with_events(ctx.clone()))?,
        ))
    })
}

/// provider 插件用它把自己注册进来。回的 `Disposable` 交给插件持有。
pub fn register_provider(ctx: &Context, provider: Arc<dyn AuthProvider>) -> Option<Disposable> {
    ctx.get::<AuthManager>(AUTH)
        .map(|manager| manager.register(provider))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_base::auth::{
        AuthFuture, LoginTarget, ProviderInfo, RemoteModel, Retry, WirePolicy,
    };
    use std::time::SystemTime;

    /// 假 provider：账号表由测试摆；登录要么一直等着（看取消），要么马上给结果。
    struct Fake {
        id: &'static str,
        policy: WirePolicy,
        active: Mutex<Option<AccountKey>>,
        /// `None` = 登录一直等着（模拟用户还没点同意）。
        login_result: Option<Result<AccountKey, AuthError>>,
        /// 活动账号还登着 / 开着套餐额度（账号表里报的）。
        signed_in: Mutex<bool>,
        plan: Mutex<bool>,
        /// `models()` 回什么、被调了几次。
        models_result: Mutex<Result<Vec<RemoteModel>, AuthError>>,
        model_calls: std::sync::atomic::AtomicUsize,
    }

    impl Fake {
        fn new(id: &'static str) -> Self {
            Self {
                id,
                policy: WirePolicy::default(),
                active: Mutex::new(None),
                login_result: Some(Ok(account(id))),
                signed_in: Mutex::new(true),
                plan: Mutex::new(true),
                models_result: Mutex::new(Ok(vec![RemoteModel {
                    slug: "m1".into(),
                    ..Default::default()
                }])),
                model_calls: Default::default(),
            }
        }

        fn signed_in_now(id: &'static str) -> Arc<Self> {
            let fake = Arc::new(Self::new(id));
            *fake.active.lock().unwrap() = Some(account(id));
            fake
        }

        fn calls(&self) -> usize {
            self.model_calls.load(Ordering::SeqCst)
        }
    }

    fn account(provider: &str) -> AccountKey {
        AccountKey {
            provider: provider.into(),
            subject: "u1".into(),
            client_id: format!("{provider}-acct"),
        }
    }

    impl AuthProvider for Fake {
        fn id(&self) -> &'static str {
            self.id
        }

        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                display_name: "Fake",
                usage_url: None,
            }
        }

        fn wire_policy(&self) -> &WirePolicy {
            &self.policy
        }

        fn login<'a>(
            &'a self,
            ui: &'a dyn LoginUi,
            _req: LoginRequest,
        ) -> AuthFuture<'a, Result<AccountKey, AuthError>> {
            Box::pin(async move {
                ui.open_url(&format!("https://{}/authorize", self.id));
                match &self.login_result {
                    Some(result) => {
                        if let Ok(acct) = result {
                            *self.active.lock().unwrap() = Some(acct.clone());
                        }
                        result.clone()
                    }
                    None => std::future::pending().await,
                }
            })
        }

        fn accounts(&self) -> Vec<AccountSummary> {
            self.active
                .lock()
                .unwrap()
                .iter()
                .map(|a| AccountSummary {
                    account: a.clone(),
                    email: None,
                    signed_in: *self.signed_in.lock().unwrap(),
                    plan_usage: *self.plan.lock().unwrap(),
                    expires_at: 0,
                    active: true,
                })
                .collect()
        }

        fn active_account(&self) -> Option<AccountKey> {
            self.active.lock().unwrap().clone()
        }

        fn set_active(&self, account_id: &str) -> Result<AccountKey, AuthError> {
            let acct = account(self.id);
            if acct.client_id != account_id {
                return Err(AuthError::Rejected {
                    code: "unknown_account".into(),
                    detail: String::new(),
                });
            }
            *self.active.lock().unwrap() = Some(acct.clone());
            Ok(acct)
        }

        fn lease(
            &self,
            acct: &AccountKey,
            _min_valid: Duration,
        ) -> AuthFuture<'_, Result<Lease, AuthError>> {
            let account = acct.clone();
            Box::pin(async move {
                Ok(Lease {
                    bearer: "token".into(),
                    expires_at: SystemTime::now() + Duration::from_secs(3600),
                    generation: 1,
                    account,
                })
            })
        }

        fn on_unauthorized(&self, _lease: &Lease) -> AuthFuture<'_, Result<Retry, AuthError>> {
            Box::pin(async { Ok(Retry::GiveUp) })
        }

        fn logout(
            &self,
            _acct: &AccountKey,
            _revoke: bool,
        ) -> AuthFuture<'_, Result<LogoutOutcome, AuthError>> {
            *self.signed_in.lock().unwrap() = false;
            Box::pin(async { Ok(LogoutOutcome::Revoked) })
        }

        fn models(
            &self,
            _acct: &AccountKey,
        ) -> AuthFuture<'_, Result<Vec<RemoteModel>, AuthError>> {
            self.model_calls.fetch_add(1, Ordering::SeqCst);
            let result = self.models_result.lock().unwrap().clone();
            Box::pin(async move { result })
        }

        fn classify(&self, status: u16, _body: &[u8]) -> Option<AuthError> {
            (status == 401).then(|| AuthError::NeedsLogin {
                reason: "expired".into(),
            })
        }
    }

    /// 记下界面收到了什么。
    #[derive(Default)]
    struct Recorder {
        urls: Mutex<Vec<String>>,
        statuses: Mutex<Vec<String>>,
    }

    impl LoginUi for Recorder {
        fn open_url(&self, url: &str) {
            self.urls.lock().unwrap().push(url.into());
        }
        fn status(&self, message: &str) {
            self.statuses.lock().unwrap().push(message.into());
        }
    }

    #[tokio::test]
    async fn lease_needs_a_provider_and_an_account() {
        let manager = AuthManager::default();
        let err = manager
            .lease("fake", Duration::from_secs(60))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::NeedsLogin { .. }));

        let _hold = manager.register(Arc::new(Fake::new("fake")));
        let err = manager
            .lease("fake", Duration::from_secs(60))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::NeedsLogin { .. }), "{err:?}");

        manager.set_active("fake", "fake-acct").unwrap();
        let lease = manager
            .lease("fake", Duration::from_secs(60))
            .await
            .expect("lease");
        assert_eq!(lease.bearer, "token");
        assert_eq!(manager.provider_ids(), vec!["fake".to_string()]);
    }

    /// 上游的 401 归一成交鉴权错误；别的状态码不归一。
    #[test]
    fn classify_only_maps_auth_failures() {
        let manager = AuthManager::default();
        let _hold = manager.register(Arc::new(Fake::new("fake")));
        assert!(manager.classify("fake", 401, b"").is_some());
        assert!(manager.classify("fake", 400, b"").is_none());
    }

    /// 两家并存：账号各归各的，按账号 id 能找回是哪一家的。
    #[test]
    fn accounts_belong_to_their_own_provider() {
        let manager = AuthManager::default();
        let _a = manager.register(Arc::new(Fake::new("alpha")));
        let _b = manager.register(Arc::new(Fake::new("beta")));
        manager.set_active("beta", "beta-acct").unwrap();
        assert!(manager.accounts("alpha").is_empty());
        assert_eq!(manager.accounts("beta").len(), 1);
        let (provider, summary) = manager.find_account("beta-acct").unwrap();
        assert_eq!(provider, "beta");
        assert_eq!(summary.account.provider, "beta");
        assert!(manager.find_account("alpha-acct").is_none());
    }

    /// 卸下插件 = provider 不见了；被新的同 id provider 盖掉之后，旧的卸下不误删新的。
    #[test]
    fn disposing_a_provider_only_removes_that_one() {
        let manager = AuthManager::default();
        let old = manager.register(Arc::new(Fake::new("fake")));
        let _new = manager.register(Arc::new(Fake::new("fake")));
        old.dispose_sync();
        assert!(manager.provider("fake").is_some());
    }

    /// 再发起一次登录 = 掐掉旧的；旧的结果不会盖掉新的状态。取消后状态清空。
    #[tokio::test]
    async fn a_second_login_replaces_the_first_and_cancel_clears_it() {
        let manager = AuthManager::default();
        let mut fake = Fake::new("slow");
        fake.login_result = None; // 一直等着用户点同意
        let _hold = manager.register(Arc::new(fake));
        let ui = Arc::new(Recorder::default());

        manager
            .start_login("slow", LoginRequest::default(), ui.clone())
            .unwrap();
        // 让第一个先跑到「打开授权页」那一步（用户点了一次、页没点完又点一次）。
        tokio::time::sleep(Duration::from_millis(20)).await;
        manager
            .start_login("slow", LoginRequest::default(), ui.clone())
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(ui.urls.lock().unwrap().len(), 2, "两次都打开过授权页");
        assert_eq!(
            manager.login_state("slow"),
            Some(LoginState::Pending {
                url: Some("https://slow/authorize".into())
            })
        );

        assert!(manager.cancel_login("slow"), "有一个在跑，取消该成功");
        assert_eq!(manager.login_state("slow"), None);
        assert!(!manager.cancel_login("slow"), "没有在跑的就不算取消了");
    }

    /// 登录失败：状态里留着那句话，界面也收到一句「登录失败」——不再只打到 stderr。
    #[tokio::test]
    async fn a_failed_login_is_reported_and_kept() {
        let manager = AuthManager::default();
        let mut fake = Fake::new("broken");
        fake.login_result = Some(Err(AuthError::NeedsLogin {
            reason: "授权被拒绝（access_denied）".into(),
        }));
        let _hold = manager.register(Arc::new(fake));
        let ui = Arc::new(Recorder::default());
        manager
            .start_login(
                "broken",
                LoginRequest {
                    target: LoginTarget::New,
                    ..LoginRequest::default()
                },
                ui.clone(),
            )
            .unwrap();
        for _ in 0..50 {
            if matches!(
                manager.login_state("broken"),
                Some(LoginState::Failed { .. })
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        match manager.login_state("broken") {
            Some(LoginState::Failed { message }) => {
                assert!(message.contains("access_denied"), "{message}")
            }
            other => panic!("{other:?}"),
        }
        let statuses = ui.statuses.lock().unwrap().clone();
        assert!(
            statuses.iter().any(|s| s.contains("登录失败")),
            "{statuses:?}"
        );
    }

    #[test]
    fn unsupported_params_are_remembered_per_model() {
        let manager = AuthManager::default();
        manager.remember_unsupported("chatgpt", "gpt-a", "service_tier");
        manager.remember_unsupported("chatgpt", "gpt-a", "service_tier");
        assert_eq!(
            manager.unsupported_params("chatgpt", "gpt-a"),
            vec!["service_tier".to_string()]
        );
        assert!(manager.unsupported_params("chatgpt", "gpt-b").is_empty());
    }

    fn models_cache() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(auth_models::FILE);
        (dir, path)
    }

    /// 等后台任务把条件做成（最多 2 秒）。
    async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("等不到：{what}");
    }

    /// 拉到的模型写进 `auth_models.json`，读目录时就有 `<provider>/<slug>`；
    /// 活动账号没开套餐额度，那一家的条目删掉——列表里不留用不了的模型。
    #[tokio::test]
    async fn refreshed_models_land_in_the_catalog() {
        let (_dir, path) = models_cache();
        let manager = AuthManager::default().with_models_cache(path.clone());
        let fake = Fake::signed_in_now("fake");
        let _hold = manager.register(fake.clone());

        let list = manager.refresh_models("fake").await.expect("拉到了");
        assert_eq!(list.len(), 1);
        let cache = AuthModelCache::read(&path);
        let entry = &cache.providers["fake"];
        assert_eq!(entry.account, "fake-acct", "记下是哪个账号的列表");
        assert_eq!(entry.display_name, "Fake");
        let catalog =
            cordis_base::config::load_catalog_with(&[], &auth_models::catalog_entries(&cache));
        assert_eq!(catalog[0].id, "fake/m1");
        assert_eq!(catalog[0].auth_provider(), Some("fake"));

        *fake.plan.lock().unwrap() = false;
        let err = manager.refresh_models("fake").await.unwrap_err();
        assert!(matches!(err, AuthError::NotEligible { .. }), "{err:?}");
        assert!(AuthModelCache::read(&path).providers.is_empty());
    }

    /// 网络抖一下不动旧列表；授权作废了（刷新说要重新登录）才删。
    #[tokio::test]
    async fn a_flaky_fetch_keeps_the_list_but_a_dead_login_drops_it() {
        let (_dir, path) = models_cache();
        let manager = AuthManager::default().with_models_cache(path.clone());
        let fake = Fake::signed_in_now("fake");
        let _hold = manager.register(fake.clone());
        manager.refresh_models("fake").await.unwrap();

        *fake.models_result.lock().unwrap() = Err(AuthError::Transport("down".into()));
        assert!(manager.refresh_models("fake").await.is_err());
        assert!(AuthModelCache::read(&path).providers.contains_key("fake"));

        *fake.models_result.lock().unwrap() = Err(AuthError::NeedsLogin {
            reason: "dead".into(),
        });
        assert!(manager.refresh_models("fake").await.is_err());
        assert!(AuthModelCache::read(&path).providers.is_empty());
    }

    /// 启动时只在缓存缺了、是别的账号的、过期了才重拉；登出了直接删。
    #[tokio::test]
    async fn startup_refetches_only_when_the_list_is_missing_stale_or_foreign() {
        let (_dir, path) = models_cache();
        let manager = AuthManager::default().with_models_cache(path.clone());
        let fake = Fake::signed_in_now("fake");
        let _hold = manager.register(fake.clone());
        let day = Duration::from_secs(86_400);

        manager.refresh_models_if_stale("fake", day);
        eventually("缺了就拉", || fake.calls() == 1).await;

        manager.refresh_models_if_stale("fake", day);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(fake.calls(), 1, "新鲜的不重拉");

        let mut cache = AuthModelCache::read(&path);
        cache.providers.get_mut("fake").unwrap().account = "someone-else".into();
        std::fs::write(&path, cache.to_json()).unwrap();
        manager.refresh_models_if_stale("fake", day);
        eventually("换了账号就重拉", || fake.calls() == 2).await;

        manager.refresh_models_if_stale("fake", Duration::ZERO);
        eventually("过期了就重拉", || fake.calls() == 3).await;

        *fake.signed_in.lock().unwrap() = false;
        manager.refresh_models_if_stale("fake", day);
        assert!(
            AuthModelCache::read(&path).providers.is_empty(),
            "登出了当场删，不用等网络"
        );
        assert_eq!(fake.calls(), 3);
    }

    /// 登录成功、登出活动账号之后都会自动同步列表。
    #[tokio::test]
    async fn login_and_logout_keep_the_list_in_step() {
        let (_dir, path) = models_cache();
        let manager = AuthManager::default().with_models_cache(path.clone());
        let fake = Arc::new(Fake::new("fake"));
        let _hold = manager.register(fake.clone());

        manager
            .start_login(
                "fake",
                LoginRequest::default(),
                Arc::new(Recorder::default()),
            )
            .unwrap();
        eventually("登录成功后拉模型", || {
            AuthModelCache::read(&path).providers.contains_key("fake")
        })
        .await;

        manager.logout("fake", None, true).await.unwrap();
        eventually("登出后拿掉", || {
            AuthModelCache::read(&path).providers.is_empty()
        })
        .await;
    }

    /// 测试里裸建的服务不拉也不写（不碰真实的 DOCK_HOME）。
    #[tokio::test]
    async fn a_bare_manager_never_touches_the_cache() {
        let manager = AuthManager::default();
        let fake = Fake::signed_in_now("fake");
        let _hold = manager.register(fake.clone());
        manager.refresh_models_if_stale("fake", Duration::ZERO);
        manager.set_active("fake", "fake-acct").unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(fake.calls(), 0);
        assert!(manager.cached_models("fake").is_none());
    }
}
