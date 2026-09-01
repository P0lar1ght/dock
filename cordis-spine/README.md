# cordis-spine

Five named Cordis services (DSH plugin seams) plus one Grok-shaped turn driver.

| ctx key | Fake | Role |
|---|---|---|
| `sessions` | in-memory log | Grok conversation persist / DSH session log |
| `llm` | echo / text stub, or `cordis-grok-llm` | Grok sampler (`chat` / `resp` / `anthropic`) |
| `tools` | echo, or Grok workspace tools (`--features grok`) | Grok ToolBridge execute |
| `systemPrompt` | fixed string | Assemble facade: live-look `"context"` then `system-prompt/assemble` |
| `context` | empty `ContextBook` | Prompt fragments (`set_base` / `section`) + occupancy `window()` |
| `agents` | in-memory registry | Grok `Agent` handle + DSH registry |
| `agentLoop` | **the** loop plugin | Grok `handle_prompt` one inner-loop step |

The loop injects the five services and live-looks them up. Replace the driver by swapping **only** the `agent-loop` plugin.

One round: `agent/pre-step` → assemble prompt → sample (`llm/stream`) → `tools/execute` → sample again until text.

```rust
let root = cordis::Context::new();
cordis_spine::install_spine(&root).await?;
let reply = root.require::<LoopHandle>(AGENT_LOOP)?.run("hello").await?;
println!("{reply}");
```

`install_spine` keeps the echo fakes for unit tests. `install_workspace(cwd)` swaps in Grok `list_dir` / `read_file` (feature `grok`, on by default):

```bash
cargo run --example run -- "列出仓库文件"
```
