# vendor/xai

从 grok-build 拷来的 crate。包名仍是 `xai-*`。

| 子目录 | 干什么 | crate 名 | 在用 |
|---|---|---|---|
| [`workflow/`](workflow/) | Rhai 工作流引擎 | `xai-workflow` | 是，`cordis-spine` path-dep |
| [`grok-tools/`](grok-tools/) | Grok 工具库整棵拷贝 | `xai-grok-tools` | **否**。缺 `xai-grok-tools-api` 等，不能当 workspace 成员编；spine/tui 里相关逻辑是抄出来的，这里只对照 |

> `xai-fuzzy-file-search` 已迁至 [`cordis-tui/fuzzy-file-search/`](../../cordis-tui/fuzzy-file-search/)（仍为独立 crate，包名不变）。
