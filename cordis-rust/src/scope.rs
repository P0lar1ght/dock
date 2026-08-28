use std::collections::HashMap;
use std::sync::Arc;

use crate::ids::IsolateKey;

/// Prototype-style isolate map: local overrides, then parent, then root defaults.
#[derive(Clone, Default)]
pub(crate) struct IsolateScope {
    parent: Option<Arc<IsolateScope>>,
    local: HashMap<String, IsolateKey>,
}

impl IsolateScope {
    pub(crate) fn get(&self, name: &str) -> Option<IsolateKey> {
        self.local
            .get(name)
            .copied()
            .or_else(|| self.parent.as_ref().and_then(|p| p.get(name)))
    }

    pub(crate) fn child(self: &Arc<Self>, name: &str, key: IsolateKey) -> Arc<Self> {
        let mut local = HashMap::new();
        local.insert(name.to_string(), key);
        Arc::new(Self {
            parent: Some(self.clone()),
            local,
        })
    }
}

/// Prototype-style intercept map (service-name → opaque config).
#[derive(Clone, Default)]
pub(crate) struct InterceptScope {
    parent: Option<Arc<InterceptScope>>,
    local: HashMap<String, Arc<dyn std::any::Any + Send + Sync>>,
}

impl InterceptScope {
    pub(crate) fn get(&self, name: &str) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        self.local
            .get(name)
            .cloned()
            .or_else(|| self.parent.as_ref().and_then(|p| p.get(name)))
    }

    pub(crate) fn child(
        self: &Arc<Self>,
        name: &str,
        config: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Arc<Self> {
        let mut local = HashMap::new();
        local.insert(name.to_string(), config);
        Arc::new(Self {
            parent: Some(self.clone()),
            local,
        })
    }

    pub(crate) fn chain(
        self: &Arc<Self>,
        inject: &HashMap<String, Option<Arc<dyn std::any::Any + Send + Sync>>>,
    ) -> Arc<Self> {
        let mut cur = self.clone();
        for (name, cfg) in inject {
            if let Some(cfg) = cfg {
                cur = cur.child(name, cfg.clone());
            }
        }
        cur
    }
}
