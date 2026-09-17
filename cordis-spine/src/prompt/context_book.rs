//! Named `"context"` inventory: prompt sections + occupancy snapshot.
//!
//! Capability plugins `inject: ["context"]` then [`ContextBook::section`] /
//! [`set_base`] / [`replace_base`]. Fiber dispose unregisters. `"systemPrompt"`
//! live-looks this table as the `system-prompt/assemble` default. `/context`
//! live-looks [`ContextBook::window`].

use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use indexmap::IndexMap;

use crate::names::CONTEXT;
use crate::prompt::assemble::PromptAssembly;
use crate::prompt::context_usage::{
    occupancy_detail, snapshot_context, ContextSnapshot, OccupancyDetail, OccupancyKind,
};

type BaseFn = Arc<dyn Fn(&Context) -> String + Send + Sync>;
type SectionFn = Arc<dyn Fn(&Context) -> Option<String> + Send + Sync>;

#[derive(Clone)]
enum Slot {
    Base(BaseFn),
    Replace(SectionFn),
    Section {
        order: i32,
        skip_on_replace: bool,
        body: SectionFn,
    },
}

struct Inner {
    slots: IndexMap<String, Slot>,
}

/// Prompt-fragment registry. Type is not `Context` (that is `cordis::Context`).
#[derive(Clone)]
pub struct ContextBook {
    ctx: Context,
    inner: Arc<Mutex<Inner>>,
}

impl ContextBook {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            inner: Arc::new(Mutex::new(Inner {
                slots: IndexMap::new(),
            })),
        }
    }

    fn insert(&self, id: &str, slot: Slot) -> cordis::Result<Disposable> {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.slots.contains_key(id) {
                return Err(cordis::Error::plugin(format!(
                    "duplicate context section {id}"
                )));
            }
            inner.slots.insert(id.to_string(), slot);
        }
        Ok(self.disposable_for(id))
    }

    fn disposable_for(&self, id: &str) -> Disposable {
        let inner = self.inner.clone();
        let id = id.to_string();
        Disposable::from_fn(move || {
            inner.lock().unwrap().slots.shift_remove(&id);
        })
    }

    pub fn set_base(
        &self,
        id: &str,
        body: impl Fn(&Context) -> String + Send + Sync + 'static,
    ) -> cordis::Result<Disposable> {
        self.insert(id, Slot::Base(Arc::new(body)))
    }

    pub fn replace_base(
        &self,
        id: &str,
        body: impl Fn(&Context) -> Option<String> + Send + Sync + 'static,
    ) -> cordis::Result<Disposable> {
        self.insert(id, Slot::Replace(Arc::new(body)))
    }

    /// Named section. Skipped once a [`replace_base`](Self::replace_base) slot
    /// has actually produced a replacement — a `replace_prompt` preset whose
    /// persona is blank produces none, and then this section still applies.
    pub fn section(
        &self,
        order: i32,
        id: &str,
        body: impl Fn(&Context) -> Option<String> + Send + Sync + 'static,
    ) -> cordis::Result<Disposable> {
        self.insert(
            id,
            Slot::Section {
                order,
                skip_on_replace: true,
                body: Arc::new(body),
            },
        )
    }

    /// Named section that survives a landed `replace_prompt` replacement.
    ///
    /// No production caller today: the subagent roster that needed it now
    /// lives on the `task` tool description. Kept as the escape hatch for a
    /// section that has to outlive `replace_prompt` (a hard safety rule, say).
    pub fn section_always(
        &self,
        order: i32,
        id: &str,
        body: impl Fn(&Context) -> Option<String> + Send + Sync + 'static,
    ) -> cordis::Result<Disposable> {
        self.insert(
            id,
            Slot::Section {
                order,
                skip_on_replace: false,
                body: Arc::new(body),
            },
        )
    }

    pub fn assemble(&self) -> PromptAssembly {
        self.assemble_on(&self.ctx)
    }

    pub fn assemble_on(&self, exec: &Context) -> PromptAssembly {
        let slots = self.inner.lock().unwrap().slots.clone();
        let mut a = PromptAssembly::new(String::new());
        for (_, slot) in &slots {
            if let Slot::Base(body) = slot {
                a.set_base(body(exec));
            }
        }
        for (id, slot) in &slots {
            if let Slot::Replace(body) = slot {
                if let Some(text) = body(exec) {
                    if !text.trim().is_empty() {
                        a.replace_base_named(id, text);
                        break;
                    }
                }
            }
        }
        // Judge on what actually landed, not on what the preset intended: a
        // `replace_prompt` preset with a blank persona yields no replacement,
        // and skipping the sections then would leave only the bare base.
        let replaced = a.replaced();
        for (id, slot) in &slots {
            if let Slot::Section {
                order,
                skip_on_replace,
                body,
            } = slot
            {
                if *skip_on_replace && replaced {
                    continue;
                }
                if let Some(text) = body(exec) {
                    if !text.trim().is_empty() {
                        a.section(*order, id, text);
                    }
                }
            }
        }
        a
    }

    pub fn window(&self) -> ContextSnapshot {
        snapshot_context(&self.ctx)
    }

    pub fn detail(&self, kind: OccupancyKind) -> OccupancyDetail {
        occupancy_detail(&self.ctx, kind)
    }
}

pub fn own_sections(ctx: &Context, disposers: Vec<Disposable>) -> cordis::Result<()> {
    ctx.effect("context.register", |scope| {
        for d in disposers {
            scope.own(d);
        }
        Ok(())
    })?;
    Ok(())
}

pub fn context() -> Plugin {
    plugin("context", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(CONTEXT, ContextBook::new(ctx.clone()))?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn duplicate_id_errors() {
        let ctx = Context::new();
        ctx.plugin(context(), ()).unwrap().wait().await.unwrap();
        let book = ctx.get::<ContextBook>(CONTEXT).unwrap();
        let _keep = book.set_base("base", |_| "a".into()).unwrap();
        assert!(book.set_base("base", |_| "b".into()).is_err());
    }

    /// `replace_prompt: true` with a blank persona produces no replacement, so
    /// the sections must survive. Judging on `AgentPresets::replaces_prompt()`
    /// dropped persona / skills / workflows / cordis and shipped a bare base.
    #[tokio::test]
    async fn blank_replacement_keeps_sections() {
        let ctx = Context::new();
        ctx.plugin(context(), ()).unwrap().wait().await.unwrap();
        let mut preset = crate::agent::presets::AgentPreset::new("blank");
        preset.replace_prompt = true;
        preset.persona = "   \n".into();
        ctx.provide(
            crate::names::AGENT_PRESETS,
            crate::agent::presets::AgentPresets::overlay(preset),
        )
        .unwrap();
        let book = ctx.get::<ContextBook>(CONTEXT).unwrap();
        let _base = book.set_base("base", |_| "BASE".into()).unwrap();
        let _replace = book.replace_base("persona-replace", |_| None).unwrap();
        let _skills = book
            .section(41, "skills", |_| Some("SKILLS".into()))
            .unwrap();
        let out = book.assemble().render();
        assert!(out.contains("BASE"), "{out}");
        assert!(out.contains("SKILLS"), "{out}");
    }

    /// The other half: a replacement that *does* land still hides the sections.
    #[tokio::test]
    async fn landed_replacement_skips_sections() {
        let ctx = Context::new();
        ctx.plugin(context(), ()).unwrap().wait().await.unwrap();
        let book = ctx.get::<ContextBook>(CONTEXT).unwrap();
        let _base = book.set_base("base", |_| "BASE".into()).unwrap();
        let _replace = book
            .replace_base("persona-replace", |_| Some("PERSONA".into()))
            .unwrap();
        let _skills = book
            .section(41, "skills", |_| Some("SKILLS".into()))
            .unwrap();
        let _roster = book
            .section_always(30, "roster", |_| Some("ROSTER".into()))
            .unwrap();
        let out = book.assemble().render();
        assert!(out.contains("PERSONA"), "{out}");
        assert!(!out.contains("BASE"), "{out}");
        assert!(!out.contains("SKILLS"), "{out}");
        assert!(out.contains("ROSTER"), "{out}");
    }

    #[tokio::test]
    async fn dispose_drops_section() {
        let ctx = Context::new();
        ctx.plugin(context(), ()).unwrap().wait().await.unwrap();
        let book = ctx.get::<ContextBook>(CONTEXT).unwrap();
        let d = book
            .section(10, "cordis", |_| Some("hello cordis".into()))
            .unwrap();
        assert!(
            book.assemble().render().contains("hello cordis"),
            "{}",
            book.assemble().render()
        );
        d.dispose_sync();
        assert!(
            !book.assemble().render().contains("hello cordis"),
            "{}",
            book.assemble().render()
        );
    }
}
