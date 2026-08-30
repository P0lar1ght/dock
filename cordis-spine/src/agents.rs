use std::sync::{Arc, Mutex};

use cordis::{Context, Inject, Plugin, plugin};

use crate::names::AGENTS;

/// Grok-shaped agent handle: id + what the loop needs to name it.
/// DSH `ctx.agents` is the registry; the concrete driver stays in the loop plugin.
#[derive(Clone, Debug)]
pub struct Agent {
    pub id: String,
}

#[derive(Clone)]
pub struct Agents {
    ctx: Context,
    inner: Arc<Mutex<Vec<Agent>>>,
}

impl Agents {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn register(&self, agent: Agent) {
        let id = agent.id.clone();
        self.inner.lock().unwrap().push(agent);
        self.ctx.emit("agent/created", id);
    }

    pub fn ensure(&self, id: &str) -> Agent {
        if let Some(existing) = self.get(id) {
            return existing;
        }
        let agent = Agent { id: id.into() };
        self.register(agent.clone());
        agent
    }

    pub fn get(&self, id: &str) -> Option<Agent> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }

    pub fn list(&self) -> Vec<Agent> {
        self.inner.lock().unwrap().clone()
    }
}

pub fn agents() -> Plugin {
    plugin("agents", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(AGENTS, Agents::new(ctx.clone()))?))
    })
}
