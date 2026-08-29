---
name: cordis-plugin-development
description: >-
  Create, modify, debug, or extend dynamic Cordis Plugins in Dock: inspect the
  live fiber/service directory, define a Package from a preset factory or Rhai
  source, run it (permission overlay), stop, undefine, repair, or roll back.
  Use when the user wants a session-local Host extension rather than a static
  plugin in the repo.
---

# Develop Dynamic Cordis Plugins (Dock)

First inspect what is actually live, then define a Package, then run it. Do not infer a complete API from a service name or an example.

This Skill is Dock's internalization of the DSH `cordis-plugin-development` workflow. Lifecycle is the same (`define` ≠ `run`; `run` / `update`; `stop` keeps Packages; `undefine` deletes).

**How to customize:** lasting capability is a static plugin in `cordis-spine`. Session experiments are dynamic Packages. Teaching factories (`echo` / `note` / `hold` / `slash`) are fixed Rust bodies — you pick the id, you do not edit their code. **Any custom Host behavior is `factory: "rhai"` plus a `source` string.** There is no browser Client / JSX. TUI slots are plain-text callbacks on `tui.slots`. Definitions live only in this process.

Hot-plugged bodies always go through cordis-rust `ctx.plugin` / `fiber.dispose` under the `cordis-dynamic` group fiber. Do not ask to change the kernel.

## Standard workflow

1. `cordis_inspect` (omit `what`, or `services` / `fibers` / `tools` / `factories` / `builtins` / `slots` / `temporary`).
2. New Plugin: pick the smallest factory. Custom logic → `rhai`. Existing Plugin: `cordis_inspect_self(pluginId, packageId)` first (Rhai source is only in that call).
3. `cordis_define`. For `rhai`, pass `source`. Define compiles; it does not call `apply`, request approval, or change `currentPackageId`. Syntax failure must not mint an id.
4. `cordis_run` with the returned ids. Approval is the same permission overlay as `bash`.
5. User reject → do not retry that activation unless they ask. Technical failure → inspect, define a **new** Package on the **same** Plugin, retry with the correct mode.
6. Verify with `cordis_call` (name = dynamic tool, arguments JSON). Do not wait for a TUI slash or a later model turn just to exercise a Host-registered tool.
7. `cordis_stop` keeps definitions. `cordis_undefine` deletes the Plugin.

Do not treat a successful `define` as running.

## Tool usage guidance

| Tool | Use it when | Do not |
| --- | --- | --- |
| `cordis_inspect` | Live fibers, named services (`tools` / `slash` / `tui.slots` include method signatures), tools (names only), factories, `what: "builtins"` (Rhai `host`), `what: "slots"`, temporary Plugins | Invent `cordis_inspect_list` / `query`; treat the report as a business API |
| `cordis_inspect_self` | List Plugins, version pointers, or one Package's factory id, Rhai source, diagnostics | Fetch everything just to build a list; use it to start a Plugin |
| `cordis_define` | First Package, or append an immutable Package | Expect define to run `apply` or request approval |
| `cordis_run` | Activate an exact Package; `run` = first start / restart current / rollback; `update` = switch versions | Use `run` to switch versions implicitly |
| `cordis_call` | Host-execute a live tool (incl. dynamic `register_tool`) in this turn | Expect slash `/test` or a later model step to verify; call `cordis_call` recursively |
| `cordis_stop` | Pause effects; keep Packages | Mean permanent deletion |
| `cordis_undefine` | Permanently remove a Plugin | Call it while rollback or inspect is still needed |

## Choose a platform (Dock)

| Requirement | Where it lives |
| --- | --- |
| Files, commands, networking | Static tools (`bash`, `read_file`, `web_fetch`, …). Do not wrap these in a Plugin unless the user wants a session experiment |
| Custom named bag + model tool + slash + TUI text pane | **`factory: "rhai"`** (this Skill) |
| Prove provide / register / pending / extra `/command` | Preset `echo` / `note` / `hold` / `slash` — not for product logic |
| Page theme, Client RPC, JSX, Wasm bytes | Not in Dock. Do not send JS / TS / JSX / Wasm in `cordis_define` |
| Durable after restart | Static plugin in `cordis-spine` |

Prefer the capability closest to the data owner. Do not create a dynamic Plugin when an existing tool already does the job.

## Custom Host: `factory: "rhai"`

`cordis_define` fields: `plugin` (`kind: "new"` + `idPrefix` of 3–6 lowercase English letters, or `kind: "existing"` + `pluginId`), `name`, `purpose`, `factory: "rhai"`, `source` (Rhai string). Then `cordis_run`.

`source` must evaluate to a map. `apply` is a function; define does not call it. `inject` is an array of named services that must already be live, or the fiber stays pending (same as `hold`). Omit `inject` → `["tools"]`. Typical extras: `"slash"`, `"tui.slots"`. Scripts cannot `on_waterfall`, cannot `ctx.plugin`, cannot load disk modules, cannot nest `eval`.

```rhai
#{
    inject: ["tools", "slash", "tui.slots"],
    apply: |host| {
        let store = #{ text: "hello" };
        host.provide("dynMemo", store);
        host.register_tool(#{
            name: "memo_get",
            description: "Read the memo text",
            parameters: #{ type: "object", properties: #{} },
            execute: |args| { store.text }
        });
        host.register_tool(#{
            name: "memo_set",
            description: "Set the memo text",
            parameters: #{
                type: "object",
                properties: #{
                    text: #{ type: "string" }
                },
                required: ["text"]
            },
            execute: |args| {
                store.text = args.text;
                store.text
            }
        });
        host.register_slash(#{
            command: "memo",
            kind: "slot",
            text: "memo",
            description: "Open the memo pane"
        });
        host.register_slot(#{
            id: "memo",
            title: "便签",
            hud: true,
            render: || { store.text },
            on_key: |key| {
                if key == "esc" { "close" } else { () }
            }
        });
    }
}
```

Call `cordis_define` with that string in `source`. Example payload:

```json
{
  "plugin": { "kind": "new", "idPrefix": "memo" },
  "name": "Memo",
  "purpose": "Session-local memo bag and tool",
  "factory": "rhai",
  "source": "<the Rhai map above>"
}
```

After define returns `memo-N` / `pkg-N`, call `cordis_run` with `mode: "run"`. While the Package is running, `memo_get` / `memo_set` are visible to the model even if the Agent preset YAML omitted them. Verify immediately with `cordis_call` `name: "memo_get"` (arguments `{}`). `cordis_stop` unregisters tools, slash, slots, and the bag.

Keep mutable state in variables closed over by `execute` / `render` / `on_key` inside `apply` (as `store` above). `host.provide` mounts a JSON snapshot under a service name for other Packages to `inject` / `host.get`. `host.get` returns a **copy** (or `true` if that name is live but not a bag, or `()` if missing). It does not write back.

### `host` methods

Confirm signatures with `cordis_inspect` `what: "builtins"`.

| Call | Needs `inject` | Contract |
| --- | --- | --- |
| `host.provide(name, map)` | — | Mount `RhaiBag`. Duplicate live names fail. Stop unregisters |
| `host.get(name)` | — | Bag → map copy; live `tools` / `slash` / `tui.slots` / bag → `true`; else `()` |
| `host.register_tool(#{ name, description, parameters, execute })` | `"tools"` | `parameters` is a **JSON-schema map** (not a JSON string). Root `type` is `"object"`; `properties` is a map of field schemas; `required` names must exist in `properties`. Omit `parameters` → empty object schema. `execute` is `\|args\|` → string (`args` is the tool JSON as a map). Dynamic: bypasses Agent preset allowlist until stop |
| `host.register_slash(#{ command, kind, text, title?, send?, description? })` | `"slash"` | Additive only; reserved names (`/help`, `/quit`, `/agents`, …) fail **before** insert. `kind`: `prompt` (template, `{args}`), `overlay` (read-only pane), `slot` (`text` = slot id), **`tool`** (`text` = live tool name; typed args → JSON `{}` / raw object / `{"args":…}`; TUI runs `Tools::execute`, result in Notice). `send` defaults true for `prompt`. A throw later in `apply` rolls back earlier `provide` / `register_*` |
| `host.register_slot(#{ id, title?, hud?, render, on_key? })` | `"tui.slots"` | `id`: 1–32 chars, start with `a-z`, then `a-z0-9_-`. `render` → plain text (Notice layout, not widgets). `hud: true` paints the first line on the shortcut bar. `on_key(key)` keys: `esc` / `enter` / `up` / `down` / `char:x`. Return `"close"` to dismiss. No `on_key` → Esc still closes |
| `host.open_slot(id)` | `"tui.slots"` | Ask the TUI to open that overlay |
| `host.call_tool(name, args)` | `"tools"` | Runs a live model tool; bash-class tools still hit the permission overlay. `args` is a map or a JSON string |
| `host.log(message)` / `print` | — | Tagged `[cordis:{pluginId}]` |

The evaluator is not a security boundary (same trust as bash). Sync steps are bounded by operation limits.

## Preset factories (teaching / regression)

Select `factory` from `cordis_inspect` `what: "factories"`. Do not invent an id.

- `echo` — `provide("dynEcho")` and register `dyn_echo` (dynamic tool).
- `note` — `provide("dynNote")` in-memory string. No extra model tool.
- `hold` — `inject: ["dynHoldGate"]` with no provider; fiber stays pending.
- `slash` — one extra `/command` on `"slash"`. Needs `command`, `kind`, `text` on `cordis_define` (not inside Rhai). `kind: "slot"` opens a registered slot id. `kind: "tool"` runs a live tool (`text` = name) without going through the model.

## Access services

Rhai: `host.get("tools")` etc. Optional names: handle `()`. Declare `inject` only when the Package must wait for that name. Inspect reports unsatisfied inject as `waiting for`.

Static plugins in `cordis-spine`: `ctx.get` / `ctx.require`; do not capture `Arc<T>` from `ctx.get` into a long-lived closure.

## Manage side effects

Every `provide` / `register_*` is owned by the **whole host-half fiber**, not by a single call. `cordis_stop` / `undefine` dispose that fiber. `mode: "update"` **replaces** the fiber: it disposes the previous Run (every bag, tool, slash, and slot from that apply) and starts the new Package on a clean fiber. It does not overlay.

Failed `apply` still attaches whatever `register_*` / `provide` already did, then the kernel unloads the fiber **before** the error returns, so those names do not leak. `cordis_stop` after a failed run is a no-op because `currentPackageId` / `run` were never set — cleanup already happened. Do not create process-wide side effects outside `apply`. There is no `host.unregister_tool`; fiber dispose is the unregister.

Reserved slash names (`/agents`, `/help`, …) fail at the start of `register_slash`, before the extra is inserted. Put `register_slash` first if you want the script to throw before other work; rollback still runs if a later call throws.

## Versions, approval, and repair

- Plugin = stable id (`idPrefix` 3–6 letters; Host mints `prefix-N` with a monotonic counter). `undefine` does not recycle `agent-7`. Retry a failed Plugin with `kind: "existing"` and the same `pluginId`; a new `idPrefix` mints another Plugin.
- Package = immutable version (`packageId`). Change anything → new Package, never overwrite.
- `currentPackageId` = last successful version (not “is running”).
- `nextPackageId` = in-flight or last-failed target.

| Current state | Target | mode |
| --- | --- | --- |
| No current | Any Package under the Plugin | `run` |
| Has current | The same Package | `run` |
| Has current | A different Package | `update` |
| Update failed | `nextPackageId` | `update` to retry |
| Update failed | `currentPackageId` | `run` to roll back |

Approval: permission overlay on `cordis_run`. A grant remains after a technical failure.

After a technical failure:

1. `cordis_inspect_self(pluginId, packageId)` for source / `hostError`.
2. Duplicate `provide` name → `cordis_stop` the owner first.
3. Define a **new** Package on the same Plugin.
4. Run with the new `packageId` and the correct mode.

## Modify @pluginId

When the user names `@pluginId`, do not create another Plugin.

1. `cordis_inspect_self(pluginId, packageId)` (pre-step reminder is identity only — not source).
2. `cordis_define` with `plugin.kind: "existing"` and the original `pluginId`. For Rhai, pass a new `source`.
3. `cordis_run` `run` or `update` per the table.

If the id is gone (removed or lost on restart), say so. Do not mint a same-named replacement.

## Common failure checks

| Failure | Check first |
| --- | --- |
| `rhai syntax` / define Error | `source` must be a map with `apply` as a function; no id is minted |
| `has been registered` / already registered | Another **running** Package still provides that name or tool; `cordis_stop` it. A throwing apply must not leave orphans |
| `unknown factory` | `cordis_inspect` `what: "factories"` |
| `invalid-mode` / use mode `"update"` | current vs target Package |
| Fiber `[pending]` / `waiting for` | Unsatisfied `inject` (legal) |
| Permission rejected | Do not request the same `cordis_run` unless the user asks |
| Lost after restart | Expected. Process memory only |
| Want JS / Client / Wasm | Use `factory: "rhai"` or a static plugin |
