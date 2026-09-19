---
name: cordis-plugin-development
description: >-
  Create, modify, debug, or extend Cordis Plugins in Dock: inspect the live
  fiber/service directory, define a Rhai or preset Package, run it (permission
  overlay), observe session/event, promote to a disk plugin, stop, undefine,
  repair, or roll back. Use when the user wants a Host extension — session-local
  or lasting under .dock/plugins — rather than wrapping an existing tool.
---

# Develop Dynamic Cordis Plugins (Dock)

First inspect what is actually live, then define a Package, then run it. Do not infer a complete API from a service name or an example.

This Skill is Dock's internalization of the DSH `cordis-plugin-development` workflow. Lifecycle is the same (`define` ≠ `run`; `run` / `update`; `stop` keeps Packages; `undefine` drops memory). Cordis is **not** the default for every request.

**How to customize:** session experiments are dynamic Packages (process memory). Lasting Host extensions that the user cannot ship as a `cordis-spine` crate go on disk under `.dock/plugins/<id>/` (project) or `~/.dock/plugins/<id>/` (user). Teaching factories (`echo` / `note` / `hold` / `slash`) are fixed Rust bodies — you pick the id, you do not edit their code. **Any custom Host behavior is `factory: "rhai"`.** Prefer writing `plugin.toml` + `source.rhai` with `write_file` / `search_replace`, then `cordis_define` with `source_path` — do **not** paste large Rhai into the tool call. Inline `source` is only for tiny samples; `cordis_promote` still turns a small inline experiment into a disk plugin. There is no browser Client / JSX. TUI slots are plain-text callbacks on `tui.slots`.

Hot-plugged bodies always go through cordis-rust `ctx.plugin` / `fiber.dispose` under the `cordis-dynamic` group fiber. Do not ask to change the kernel. Do not wrap `bash` / `read_file` / MCP in a Plugin when those tools already do the job.

Hot-plugged bodies always go through cordis-rust `ctx.plugin` / `fiber.dispose` under the `cordis-dynamic` group fiber. Do not ask to change the kernel.

## Standard workflow

1. `cordis_inspect` (omit `what`, or `services` / `fibers` / `tools` / `factories` / `builtins` / `events` / `slots` / `temporary` / `permanent`).
2. New Plugin: pick the smallest factory. Custom logic → `rhai`. Existing Plugin: `cordis_inspect_self(pluginId, packageId)` first (`source_path` or inline source).
3. **Primary (file-first) for `rhai`:**
   1. `write_file` / `search_replace` → `.dock/plugins/<id>/plugin.toml` + `source.rhai` (or under `~/.dock/plugins/<id>/`).
   2. `cordis_define` with `source_path` (e.g. `.dock/plugins/demo/source.rhai`) — **not** a giant inline `source`.
   3. `cordis_run` with the returned ids.
   4. On failure: **edit the file**, `cordis_define` a **new** Package on the same Plugin (same `source_path`), then `update` / `run`. Do **not** re-paste the whole Rhai into the tool call.
4. **Tiny samples only:** `cordis_define` with inline `source` (≤128KiB). Prefer promoting later with `cordis_promote` if it should survive restart.
5. Define compiles; it does not call `apply`, request approval, or change `currentPackageId`. Syntax failure must not mint an id. `source` XOR `source_path`.
6. `cordis_run` approval is the same permission overlay as `bash`.
7. User reject → do not retry that activation unless they ask. Technical failure → inspect, edit file (or tiny inline), define a **new** Package on the **same** Plugin, retry with the correct mode.
8. Verify with `cordis_call` (name = dynamic tool, arguments JSON). Do not wait for a TUI slash or a later model turn just to exercise a Host-registered tool.
9. Session inline experiment → lasting disk: `cordis_promote` (permission overlay). Default id strips `-N`. Files already under `.dock/plugins/<id>/` autoload on next start without promote.
10. `cordis_stop` keeps in-memory definitions. `cordis_undefine` drops the in-memory Plugin; disk files stay.

Do not treat a successful `define` as running.

## Tool usage guidance

| Tool | Use it when | Do not |
| --- | --- | --- |
| `cordis_inspect` | Live fibers, named services (`tools` / `slash` / `tui.slots` include method signatures), tools (names only), factories, `what: "builtins"` (Rhai `host`), `what: "events"`, `what: "slots"`, `temporary` / `permanent` | Invent `cordis_inspect_list` / `query`; treat the report as a business API |
| `cordis_inspect_self` | List Plugins, version pointers, or one Package's factory id, `source_path` / inline source, diagnostics | Dump huge Rhai when only the path is needed; use it to start a Plugin |
| `cordis_define` | First Package, or append an immutable Package; prefer `source_path` for rhai | Paste large Rhai as `source`; pass both `source` and `source_path`; expect define to run `apply` |
| `cordis_run` | Activate an exact Package; `run` = first start / restart current / rollback; `update` = switch versions | Use `run` to switch versions implicitly |
| `cordis_call` | Host-execute a live tool (incl. dynamic `register_tool`) in this turn | Expect slash `/test` or a later model step to verify; call `cordis_call` recursively |
| `cordis_promote` | Write current/latest Package to `.dock/plugins/<id>/` and autostart it | Treat define as durable; delete disk files with undefine |
| `cordis_stop` | Pause effects; keep Packages | Mean deletion of disk files |
| `cordis_undefine` | Drop the in-memory Plugin | Delete `plugin.toml` (it does not) |

## Choose a platform (Dock)

| Requirement | Where it lives |
| --- | --- |
| Files, commands, networking | Static tools (`bash`, `read_file`, `web_fetch`, …). Do not wrap these in a Plugin unless the user wants a session experiment |
| Custom named bag + model tool + slash + TUI text pane | **`factory: "rhai"`** (this Skill) |
| Prove provide / register / pending / extra `/command` | Preset `echo` / `note` / `hold` / `slash` — not for product logic |
| Page theme, Client RPC, JSX, Wasm bytes | Not in Dock. Do not send JS / TS / JSX / Wasm in `cordis_define` |
| Durable after restart | `cordis_promote` → `.dock/plugins/<id>/`, or a static plugin in `cordis-spine` |

Prefer the capability closest to the data owner. Do not create a dynamic Plugin when an existing tool already does the job.

## Custom Host: `factory: "rhai"`

`cordis_define` fields: `plugin` (`kind: "new"` + `idPrefix` of 3–6 lowercase English letters, or `kind: "existing"` + `pluginId`), `name`, `purpose`, `factory: "rhai"`, and **either** `source_path` **or** `source` (not both). Then `cordis_run`.

| Input | When | Limit | Storage |
| --- | --- | --- | --- |
| `source_path` | **Default** for real plugins | ≤1 MiB | Path under `.dock/plugins/<id>/` or `~/.dock/plugins/<id>/`; Host re-reads on run/update |
| `source` | Tiny samples / one-liners only | ≤128 KiB | Inline on the Package |

`source_path` must not contain `..` and must resolve under those plugin roots. Inspect shows the path and does not dump the file.

The Rhai body (file or inline) must evaluate to a map. `apply` is a function; define does not call it. `inject` is an array of named services that must already be live, or the fiber stays pending (same as `hold`). Omit `inject` → `["tools"]`. Typical extras: `"slash"`, `"tui.slots"`. Scripts cannot `ctx.plugin`, cannot load disk modules, cannot nest `eval`. `host.on` reaches three events only: `"session/event"` (observe), `"agent/step-start"` and `"agent/turn-end"` (intercept) — the other waterfalls are not scriptable.

### File-first example (copy-paste)

`.dock/plugins/demo/plugin.toml`:

```toml
name = "Demo"
purpose = "File-backed rhai sample"
factory = "rhai"
enabled = true
```

`.dock/plugins/demo/source.rhai`:

```rhai
#{
    inject: ["tools"],
    apply: |host| {
        host.register_tool(#{
            name: "demo_ping",
            description: "Return pong",
            parameters: #{ type: "object", properties: #{} },
            execute: |args| { "pong" }
        });
    }
}
```

`cordis_define` JSON (no giant `source` string):

```json
{
  "plugin": { "kind": "new", "idPrefix": "demo" },
  "name": "Demo",
  "purpose": "File-backed rhai sample",
  "factory": "rhai",
  "source_path": ".dock/plugins/demo/source.rhai"
}
```

Then `cordis_run` with the returned `pluginId` / `packageId`. On failure, edit `source.rhai`, define again with `kind: "existing"` and the same `source_path`, then `mode: "update"` (or `run` per the version table).

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

The memo map above is the body of `source.rhai` (preferred) or a tiny inline `source`. After define returns `memo-N` / `pkg-N`, call `cordis_run` with `mode: "run"`. While the Package is running, `memo_get` / `memo_set` are visible to the model even if the Agent preset YAML omitted them. Verify immediately with `cordis_call` `name: "memo_get"` (arguments `{}`). `cordis_stop` unregisters tools, slash, slots, and the bag.

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
| `host.on("session/event", \|line\| { ... })` | — | Observe the session log after append. `line` is `user\t…` / `assistant\t…` / `tool\tname` / `reminder\t…`. Runs asynchronously; do not treat it as a waterfall. Stop unregisters. Do not re-enter `Sessions` from the handler |
| `host.on("agent/step-start", \|step\| { ... })` | — | Runs **before every sample** of a turn. `step` is `#{ step, identity, main }` — `step` counts from 0 and keeps counting across turn-end continuations, so per-turn state rearms at 0. Return a `<system-reminder>` string to inject it before this sample, or `()` for no opinion |
| `host.on("agent/turn-end", \|end\| { ... })` | — | Runs when the turn wants to end. `end` is `#{ text, rounds, ended_with_text, queued_followups, identity, main }`. Return a `<system-reminder>` string to keep the turn going, or `()` to let it end. The host **skips the hook entirely** when the user has queued the next message, so `queued_followups` is always `false` here — the user steers, no script gets the wheel. Subagent turns still call the hook: the reminder lands in that child's own transcript, so read `main` if you only mean the user-facing session |
| `host.log(message)` / `print` | — | Tagged `[cordis:{pluginId}]` |

The evaluator is not a security boundary (same trust as bash). Sync steps are bounded by operation limits.

### Script-level HTTP + JSON — wrap an API as a tool

You do **not** need an existing tool to talk to an API. The script has HTTP and JSON of its own, so one plugin is enough to turn any HTTP endpoint into a Dock tool.

| Call | Needs `inject` | Contract |
| --- | --- | --- |
| `http_request(#{ url, method?, headers?, body?, timeout_secs? })` | — | Returns `#{ status, ok, url, headers, body }`. `body` is always a **string** — run `parse_json(body)` for JSON APIs. `method` defaults to `"GET"` (`GET` `POST` `PUT` `PATCH` `DELETE` `HEAD` `OPTIONS`). `headers` is a map (`#{ "Authorization": "Bearer …" }`), not an array. `timeout_secs` defaults to 30, clamped to 120 |
| `parse_json(text)` / `value.to_json()` | — | Rhai builtins. `parse_json` gives a map/array you can index; `to_json()` serialises a map back to a string for `body` |

Shape of the whole thing — this is the entire plugin:

```rhai
#{
  inject: ["tools"],
  apply: |host| {
    host.register_tool(#{
      name: "gh_issue",
      description: "Read one GitHub issue",
      parameters: #{ type: "object", properties: #{ repo: #{ type: "string" }, n: #{ type: "integer" } }, required: ["repo", "n"] },
      execute: |args| {
        let resp = http_request(#{
          url: "https://api.github.com/repos/" + args.repo + "/issues/" + args.n,
          headers: #{ "Accept": "application/vnd.github+json", "User-Agent": "dock" }
        });
        if !resp.ok { return "HTTP " + resp.status + ": " + resp.body; }
        let issue = parse_json(resp.body);
        issue.title + "\n\n" + issue.body
      }
    });
  }
}
```

`execute` closures capture `host` and see `http_request` / `parse_json`, so all of this works from inside a registered tool. A top-level `fn` does **not** capture the enclosing scope — pass `host` in explicitly if you factor one out.

Boundaries, so you can tell a bug from a rule:

- **4xx/5xx are not errors.** They come back with `ok: false` and the status; check `resp.ok`. Only network failures, bad specs, SSRF blocks and permission denials throw.
- **3xx redirects are returned as-is** (not followed). Same as `web_fetch`'s client policy: SSRF and host permission only cover the URL you pass in, so auto-following a `302` to loopback/metadata would bypass them. If you need the next hop, call `http_request` again with the `Location` (and expect another permission prompt if the host differs).
- **Private and loopback addresses are blocked** by the same SSRF policy as `web_fetch` (DNS is resolved first, so a name pointing at `127.0.0.1` is blocked too). You cannot reach `localhost` services this way unless `[toolset.web_fetch] allow_local` is on.
- **The first request to a host asks the user for permission**, like `bash`. "Always allow" is remembered **per host**, so a plugin that talks to one API asks once.
- **Credentials go in `headers`, never in the URL** — `https://user:pw@host/` is rejected. Header values may not contain newlines.
- Request body ≤ 1 MiB, response body ≤ 4 MiB, ≤ 32 headers.
- `http_request` exists only while the plugin **runs**. It is not available during `cordis_define` (the define-time preflight evaluates your top-level map — a request there would dodge the permission gate), so never call it at the top level of the source; call it inside `apply` or inside an `execute` / handler closure.

### Script-level codecs + HMAC

Pure helpers for Basic auth, signed query strings, and content digests. **No I/O, no permission prompts.** Same runtime-only rule as `http_request` — not available during `cordis_define`.

| Call | Contract |
| --- | --- |
| `to_base64(input)` / `from_base64(text)` | Standard Base64. Input is `String` or `Blob`; decode returns `Blob` |
| `to_base64url(input)` / `from_base64url(text)` | URL-safe Base64 **without** padding on encode; decode accepts padded or unpadded |
| `url_encode(text)` / `url_decode(text)` | Percent-encode/decode UTF-8 for query values (` ` → `%20`) |
| `to_hex(input)` / `from_hex(text)` | Lowercase hex; `from_hex` rejects odd length / bad digits → `Blob` |
| `sha256(input)` / `sha256_blob(input)` | SHA-256 as lowercase hex or 32-byte `Blob` |
| `hmac_sha256(key, message)` / `hmac_sha256_blob(key, message)` | HMAC-SHA256 as hex or `Blob`; key/message are `String` or `Blob` |
| `unix_time()` / `unix_time_ms()` | Absolute UTC epoch seconds / milliseconds (`SystemTime`). Rhai's built-in `timestamp()` is `Instant` (relative/monotonic) — use these for API signing (SigV4, Aliyun, etc.) |

Inputs larger than 1 MiB throw a runtime error.

Basic auth example (pair with `http_request`):

```rhai
execute: |args| {
  let token = to_base64(args.user + ":" + args.pass);
  let resp = http_request(#{
    url: "https://api.example.com/v1/me",
    headers: #{ "Authorization": "Basic " + token }
  });
  if !resp.ok { return "HTTP " + resp.status + ": " + resp.body; }
  resp.body
}
```


The two intercept hooks run **inline on the turn**, so the script blocks the sample until it returns (bounded by the same operation limit); keep them short and avoid `host.call_tool` there. A throw is treated as no opinion — it is logged and the turn carries on, as is any return value that is not a string (returning `42` or a map logs and injects nothing, rather than stringifying it into the model's history). The host always passes control to the next handler for you, so a script cannot swallow the chain, and cannot append to `Sessions` itself: it returns text, the loop appends it. Built-in slots win: `tool-todo` (10) and `tool-goal` (20) outrank a script (50) when both want the same round.

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
2. `cordis_define` with `plugin.kind: "existing"` and the original `pluginId`. For Rhai, edit `source.rhai` (or pass a new tiny inline `source`) and the same `source_path` when file-backed.
3. `cordis_run` `run` or `update` per the table.

If the id is gone (removed or lost on restart), say so. Do not mint a same-named replacement.

## Common failure checks

| Failure | Check first |
| --- | --- |
| `rhai syntax` / define Error | File or inline body must be a map with `apply` as a function; no id is minted |
| `source_path` / `..` / outside roots | Path must sit under `.dock/plugins/<id>/` or `~/.dock/plugins/<id>/` |
| both `source` and `source_path` | Pass exactly one |
| `has been registered` / already registered | Another **running** Package still provides that name or tool; `cordis_stop` it. A throwing apply must not leave orphans |
| `unknown factory` | `cordis_inspect` `what: "factories"` |
| `invalid-mode` / use mode `"update"` | current vs target Package |
| Fiber `[pending]` / `waiting for` | Unsatisfied `inject` (legal) |
| Permission rejected | Do not request the same `cordis_run` unless the user asks |
| Lost after restart | Session Plugins are process memory. Promote to disk, or expect this |
| Want JS / Client / Wasm | Use `factory: "rhai"` or a static plugin |

## Disk plugins (survive restart)

Layout: `{project .dock or ~/.dock}/plugins/<id>/plugin.toml` plus `source.rhai` for `factory: "rhai"`. The **directory name** is `pluginId` (`[a-z][a-z0-9-]{1,31}`). Project overlays user on the same id. `enabled = false` skips autostart.

You can **author here first** and `cordis_define` with `source_path`, or start from a tiny inline `source` and `cordis_promote` later. Autoload on `install_app` does not show the permission overlay (workspace-trusted). `/cordis` lists both layers. `cordis_undefine` does not delete the directory.

```toml
name = "便签"
purpose = "Session memo bag"
factory = "rhai"
enabled = true
```

`cordis_promote` writes this from a live Package (`scope`: `project` default, or `user`) and still materializes `source.rhai` content even when the Package was file-backed.
