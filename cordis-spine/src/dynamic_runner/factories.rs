//! Preset Rust factories plus the Rhai source factory occupying the same
//! `define`/`run` slot.

use std::sync::Mutex;

use cordis::{plugin, Inject, Plugin};

use crate::names::{SLASH, TOOLS};
use crate::slash::{Slash, SlashEntry};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

pub const DYN_ECHO: &str = "dynEcho";
pub const DYN_NOTE: &str = "dynNote";
pub const DYN_HOLD_GATE: &str = "dynHoldGate";
pub const DYN_ECHO_TOOL: &str = "dyn_echo";
pub const RHAI_FACTORY: &str = "rhai";

/// Tiny live service mounted by the `echo` factory.
pub struct DynEcho;

impl DynEcho {
    pub fn ping(&self) -> &'static str {
        "pong"
    }
}

/// Tiny live service mounted by the `note` factory.
pub struct DynNote {
    text: Mutex<String>,
}

impl DynNote {
    pub fn get(&self) -> String {
        self.text.lock().unwrap().clone()
    }

    pub fn set(&self, text: impl Into<String>) {
        *self.text.lock().unwrap() = text.into();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FactoryInfo {
    pub id: &'static str,
    pub purpose: &'static str,
    pub provides: &'static [&'static str],
    pub inject: &'static [&'static str],
    pub tools: &'static [&'static str],
    build: fn(fiber_name: &str, contrib: Option<&SlashEntry>) -> Plugin,
}

pub fn list_factories() -> &'static [FactoryInfo] {
    &FACTORIES
}

pub fn lookup_factory(id: &str) -> Option<&'static FactoryInfo> {
    FACTORIES.iter().find(|f| f.id == id)
}

impl FactoryInfo {
    pub fn build(&self, fiber_name: &str, contrib: Option<&SlashEntry>) -> Plugin {
        (self.build)(fiber_name, contrib)
    }
}

const FACTORIES: &[FactoryInfo] = &[
    FactoryInfo {
        id: "echo",
        purpose: "Provide dynEcho and register dyn_echo so inspect/tools prove a live host half.",
        provides: &[DYN_ECHO],
        inject: &[TOOLS],
        tools: &[DYN_ECHO_TOOL],
        build: build_echo,
    },
    FactoryInfo {
        id: "note",
        purpose: "Provide dynNote (in-memory string). No extra model tool.",
        provides: &[DYN_NOTE],
        inject: &[],
        tools: &[],
        build: build_note,
    },
    FactoryInfo {
        id: "hold",
        purpose: "Inject a missing service so the host half stays pending (legal Cordis wait).",
        provides: &[],
        inject: &[DYN_HOLD_GATE],
        tools: &[],
        build: build_hold,
    },
    FactoryInfo {
        id: "slash",
        purpose: "Register one extra /command on the named slash service (prompt send/fill, read-only overlay, open a slot, or run a live tool by name). Cannot replace builtins.",
        provides: &[],
        inject: &[SLASH],
        tools: &[],
        build: build_slash,
    },
    FactoryInfo {
        id: RHAI_FACTORY,
        purpose: "Evaluate model-authored Rhai source: host.provide / register_tool / register_slash / register_slot. Define compiles; run calls apply.",
        provides: &[],
        inject: &[TOOLS],
        tools: &[],
        build: build_rhai_stub,
    },
];

fn build_rhai_stub(_fiber_name: &str, _: Option<&SlashEntry>) -> Plugin {
    plugin("rhai-stub", Inject::new(), |_, _: &()| {
        Err(cordis::Error::plugin(
            "rhai factory is built from source at run; this stub is not used",
        ))
    })
}

fn build_echo(fiber_name: &str, _: Option<&SlashEntry>) -> Plugin {
    plugin(fiber_name, Inject::from([TOOLS]), |ctx, _: &()| {
        ctx.provide(DYN_ECHO, DynEcho)?;
        let tools = ctx.require::<Tools>(TOOLS)?;
        let body: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { echo_tool(&ctx, call) })
            })
        };
        own_registered(
            ctx,
            vec![
                tools.register_dynamic(
                    ToolSpec {
                        name: DYN_ECHO_TOOL.into(),
                        description:
                            "Ping the session-local dynEcho service mounted by the echo factory."
                                .into(),
                        parameters_json: r#"{"type":"object","properties":{}}"#.into(),
                    },
                    body,
                )?,
            ],
        )?;
        Ok(None)
    })
}

fn build_note(fiber_name: &str, _: Option<&SlashEntry>) -> Plugin {
    plugin(fiber_name, Inject::new(), |ctx, _: &()| {
        ctx.provide(
            DYN_NOTE,
            DynNote {
                text: Mutex::new(String::new()),
            },
        )?;
        Ok(None)
    })
}

fn build_hold(fiber_name: &str, _: Option<&SlashEntry>) -> Plugin {
    plugin(fiber_name, Inject::from([DYN_HOLD_GATE]), |_ctx, _: &()| {
        Ok(None)
    })
}

fn build_slash(fiber_name: &str, contrib: Option<&SlashEntry>) -> Plugin {
    let contrib = contrib.cloned();
    plugin(fiber_name, Inject::from([SLASH]), move |ctx, _: &()| {
        let Some(entry) = contrib.clone() else {
            return Err(cordis::Error::plugin(
                "slash factory needs command, kind, and text from cordis_define",
            ));
        };
        let slash = ctx.require::<Slash>(SLASH)?;
        own_registered(ctx, vec![slash.register(entry)?])?;
        Ok(None)
    })
}

fn echo_tool(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let Some(echo) = ctx.get::<DynEcho>(DYN_ECHO) else {
        return tool_result(call, "Error: dynEcho is not mounted");
    };
    tool_result(call, echo.ping())
}
