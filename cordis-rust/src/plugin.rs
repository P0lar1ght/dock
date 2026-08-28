use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use crate::dispose::{BoxFuture, Disposable};
use crate::error::Result;
use crate::ids::PluginId;

pub(crate) type Config = Arc<dyn std::any::Any + Send + Sync>;
pub(crate) type ApplyFn =
    Arc<dyn Fn(crate::Context, Config) -> BoxFuture<Result<Option<Disposable>>> + Send + Sync>;

/// Named dependencies a plugin requires before it can load.
#[derive(Clone, Debug, Default)]
pub struct Inject {
    pub(crate) deps: HashMap<String, Option<Config>>,
}

impl Inject {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn require(mut self, name: impl Into<String>) -> Self {
        self.deps.insert(name.into(), None);
        self
    }

    /// Require `name` and attach intercept config visible on the plugin context.
    pub fn require_with<T: Send + Sync + 'static>(
        mut self,
        name: impl Into<String>,
        config: T,
    ) -> Self {
        self.deps
            .insert(name.into(), Some(Arc::new(config) as Config));
        self
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.deps.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.deps.is_empty()
    }
}

impl From<&[&str]> for Inject {
    fn from(names: &[&str]) -> Self {
        let mut deps = HashMap::new();
        for name in names {
            deps.insert((*name).to_string(), None);
        }
        Self { deps }
    }
}

impl<const N: usize> From<[&str; N]> for Inject {
    fn from(names: [&str; N]) -> Self {
        Inject::from(&names[..])
    }
}

impl From<Vec<&str>> for Inject {
    fn from(names: Vec<&str>) -> Self {
        Inject::from(names.as_slice())
    }
}

impl From<Vec<String>> for Inject {
    fn from(names: Vec<String>) -> Self {
        let mut deps = HashMap::new();
        for name in names {
            deps.insert(name, None);
        }
        Self { deps }
    }
}

/// A plugin entry: name, inject list, and apply callback.
#[derive(Clone)]
pub struct Plugin {
    pub(crate) id: PluginId,
    pub(crate) name: String,
    pub(crate) inject: Inject,
    pub(crate) apply: ApplyFn,
}

impl Plugin {
    pub fn id(&self) -> PluginId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn inject(&self) -> &Inject {
        &self.inject
    }
}

/// Build a synchronous plugin. `apply` runs once every inject dependency is live.
pub fn plugin<C, F>(name: impl Into<String>, inject: impl Into<Inject>, apply: F) -> Plugin
where
    C: Send + Sync + 'static,
    F: Fn(&crate::Context, &C) -> Result<Option<Disposable>> + Send + Sync + 'static,
{
    plugin_async(name, inject, move |ctx, config: &C| {
        let out = apply(&ctx, config);
        async move { out }
    })
}

/// Build a plugin whose apply body is async.
pub fn plugin_async<C, F, Fut>(
    name: impl Into<String>,
    inject: impl Into<Inject>,
    apply: F,
) -> Plugin
where
    C: Send + Sync + 'static,
    F: Fn(crate::Context, &C) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Option<Disposable>>> + Send + 'static,
{
    let name = name.into();
    let apply = Arc::new(apply);
    Plugin {
        id: crate::ids::next_plugin(),
        name,
        inject: inject.into(),
        apply: Arc::new(move |ctx, cfg| {
            let apply = apply.clone();
            Box::pin(async move {
                let config = cfg
                    .downcast_ref::<C>()
                    .ok_or_else(|| crate::Error::message("plugin config has the wrong type"))?;
                apply(ctx, config).await
            })
        }),
    }
}

/// Provide `T` under `name` for the lifetime of this plugin.
pub fn service<T, F>(name: impl Into<String>, inject: impl Into<Inject>, ctor: F) -> Plugin
where
    T: Send + Sync + 'static,
    F: Fn(&crate::Context) -> Result<T> + Send + Sync + 'static,
{
    let name = name.into();
    let name_for_apply = name.clone();
    plugin(name, inject, move |ctx, _: &()| {
        let value = ctor(ctx)?;
        let d = ctx.provide(&name_for_apply, value)?;
        Ok(Some(d))
    })
}
