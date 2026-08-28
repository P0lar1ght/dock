# dock

Grok-shaped TUI，独立 git 仓库，嵌在 AILab 里。**不 path-dep `grok-build/`。**

外面的参考树（`cordis/`、`deepseek-harness/`、`grok-build/`）不属于本仓库。插件规则见 [AGENTS.md](AGENTS.md)；工具清单（已有 / 待做 / 不做）见 [TOOLS.md](TOOLS.md)。

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

Model picker (`/model`, F2 Settings) reads `~/.dock/config.toml` then `.dock/config.toml`. See `config.toml.example`. `DOCK_MODEL` overrides `[models].default`.

This workspace's `.dock/config.toml` defaults to OpenRouter **MiniMax M3 free** (`minimax/minimax-m3:free`). Fill `api_key` there or export `OPENROUTER_API_KEY`. Empero (`glm-5.3-flash` / `qwen3.8-flash`) stays in the picker; it is currently in maintenance.

Stub LLM echoes. 产品只在这棵树里改。
