# dock

Standalone Grok-shaped TUI. Own git repo, **no `grok-build/` path deps.**

Reference trees stay in AILab (`cordis/`, `deepseek-harness/`, `grok-build/`) and are not part of this repo.

```
cordis-rust      plugin kernel
cordis-markdown  baked Grok markdown renderer
cordis-spine     sessions + stub llm/tools + agent loop
cordis-tui       fullscreen Grok pager
cordis-app       session actor + binary
```

```bash
cargo run -p cordis-app
```

Stub LLM echoes. This tree is the product; do not path-dep crates outside this repo.
