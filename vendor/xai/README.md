# vendor/xai

从 grok-build 拷来的 crate。包名仍是 `xai-*`。

| 子目录 | 干什么 | crate 名 | 在用 |
|---|---|---|---|
| [`workflow/`](workflow/) | Rhai 工作流引擎 | `xai-workflow` | 是，`cordis-spine` path-dep |
| [`fuzzy-file-search/`](fuzzy-file-search/) | 模糊搜文件（`@` 补全） | `xai-fuzzy-file-search` | 是，`cordis-tui` path-dep |
| [`grok-tools/`](grok-tools/) | Grok 工具库整棵拷贝 | `xai-grok-tools` | **否**。缺 `xai-grok-tools-api` 等，不能当 workspace 成员编；spine/tui 里相关逻辑是抄出来的，这里只对照 |
