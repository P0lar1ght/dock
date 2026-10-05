//! `"gateway.methods"`：dock.1 的功能方法表。
//!
//! 网关本身只写连接、线程、轮次这些协议骨架（`rpc::dispatch` 里那张 `match`）；
//! 一块功能（PR 列表、定时任务……）是一颗插件，挂载时把自己的方法登记进来，随插件
//! fiber 一起注销。给 GUI 加一页 = 再挂一颗插件，不用改网关。
//!
//! 每个方法带一份 [`MethodPolicy`]：跑多久（要不要放到连接锁外）、谁能调（是否只认
//! 桌面 GUI 的受信 ticket）、关着的线程要不要先开页。核心方法（`rpc::CORE_METHODS`）
//! 不能登记：表里的策略会套到核心方法上，detached 还会把核心实现顶掉。

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use cordis::{Context, Disposable};
use cordis_spine::BoxFuture;
use serde_json::Value;

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;

/// Named service key.
pub const GATEWAY_METHODS: &str = "gateway.methods";

/// 一个方法怎么跑、谁能调。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MethodPolicy {
    /// 一次要几秒（调模型、走网络、读大目录）：鉴权后放到连接锁外另起任务跑，
    /// 不卡这条连接的推送和其它请求。拿不到连接状态（订阅表）。
    pub detached: bool,
    /// 能改本机配置（等于能在本机跑程序）：只认 `dock serve` 交给父进程的受信 ticket，
    /// 配对来的网页和远程设备一律 `forbidden`。
    pub trusted_only: bool,
    /// 线程级方法：`threadId` 指向关着的会话时先开页再做。
    pub opens_thread: bool,
}

impl MethodPolicy {
    pub fn detached() -> Self {
        Self {
            detached: true,
            ..Self::default()
        }
    }
}

/// 方法体：拿网关句柄与参数，回 `result` 或错误。
pub type MethodHandler =
    Arc<dyn Fn(GatewayHandle, Value) -> BoxFuture<'static, Result<Value, RpcError>> + Send + Sync>;

/// 把一个 async 函数包成 [`MethodHandler`]。
pub fn method<F, Fut>(f: F) -> MethodHandler
where
    F: Fn(GatewayHandle, Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, RpcError>> + Send + 'static,
{
    Arc::new(move |gateway, params| Box::pin(f(gateway, params)))
}

#[derive(Clone)]
struct Entry {
    policy: MethodPolicy,
    handler: MethodHandler,
}

/// Named `"gateway.methods"`。调用点 live-lookup。
#[derive(Clone, Default)]
pub struct GatewayMethods {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}

impl GatewayMethods {
    /// 登记一个方法。同名已登记就失败。
    pub fn register(
        &self,
        name: &str,
        policy: MethodPolicy,
        handler: MethodHandler,
    ) -> cordis::Result<Disposable> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(cordis::Error::plugin("方法名不能为空"));
        }
        if crate::rpc::is_core_method(&name) {
            return Err(cordis::Error::plugin(format!(
                "方法 {name} 是网关核心方法，插件不能登记"
            )));
        }
        {
            let mut entries = self.entries.lock().unwrap();
            if entries.contains_key(&name) {
                return Err(cordis::Error::plugin(format!("方法 {name} 已登记")));
            }
            entries.insert(name.clone(), Entry { policy, handler });
        }
        let entries = self.entries.clone();
        Ok(Disposable::from_fn(move || {
            entries.lock().unwrap().remove(&name);
        }))
    }

    pub fn policy(&self, name: &str) -> Option<MethodPolicy> {
        self.entries.lock().unwrap().get(name).map(|e| e.policy)
    }

    pub fn handler(&self, name: &str) -> Option<MethodHandler> {
        self.entries
            .lock()
            .unwrap()
            .get(name)
            .map(|e| e.handler.clone())
    }
}

/// 功能插件登记自己的一组方法：随插件 fiber 一起注销。
pub fn register_methods(
    ctx: &Context,
    methods: Vec<(&str, MethodPolicy, MethodHandler)>,
) -> cordis::Result<()> {
    let table = ctx.require::<GatewayMethods>(GATEWAY_METHODS)?;
    let mut owned = Vec::with_capacity(methods.len());
    for (name, policy, handler) in methods {
        owned.push(table.register(name, policy, handler)?);
    }
    ctx.effect("gateway.methods.register", |scope| {
        for d in owned {
            scope.own(d);
        }
        Ok(())
    })?;
    Ok(())
}

/// 这一网关的方法表里 `name` 的策略（没登记 = 默认：连接锁内跑、谁都能调、不开页）。
pub(crate) fn policy_of(gateway: &GatewayHandle, name: &str) -> MethodPolicy {
    gateway
        .ctx()
        .get::<GatewayMethods>(GATEWAY_METHODS)
        .and_then(|table| table.policy(name))
        .unwrap_or_default()
}

/// 跑表里的方法；没登记回 `method_not_found`。
pub(crate) async fn call(
    gateway: GatewayHandle,
    name: &str,
    params: Value,
) -> Result<Value, RpcError> {
    let handler = gateway
        .ctx()
        .get::<GatewayMethods>(GATEWAY_METHODS)
        .and_then(|table| table.handler(name));
    match handler {
        Some(handler) => handler(gateway, params).await,
        None => Err(RpcError::method_not_found(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_names_are_refused_and_disposal_frees_them() {
        let table = GatewayMethods::default();
        let noop = method(|_, _| async { Ok(Value::Null) });
        let first = table
            .register("x/list", MethodPolicy::detached(), noop.clone())
            .unwrap();
        assert!(table
            .register("x/list", MethodPolicy::default(), noop.clone())
            .is_err());
        assert!(table.policy("x/list").unwrap().detached);
        first.dispose_sync();
        assert!(table.policy("x/list").is_none());
        assert!(table
            .register("x/list", MethodPolicy::default(), noop)
            .is_ok());
    }

    /// 核心方法名登记不进来：否则 `detached()` 一登记，`turn/start` 就被送去跑插件的
    /// handler，核心实现和连接上的订阅状态都没了。
    #[test]
    fn core_method_names_are_refused() {
        let table = GatewayMethods::default();
        let noop = method(|_, _| async { Ok(Value::Null) });
        for name in [
            "turn/start",
            "thread/subscribe",
            "initialize",
            "browser/view/open",
        ] {
            assert!(
                table
                    .register(name, MethodPolicy::detached(), noop.clone())
                    .is_err(),
                "{name} 不该登记成功"
            );
            assert!(table.policy(name).is_none());
        }
    }
}
