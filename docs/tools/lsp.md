# LSP

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-lsp`

- **ctx**：`"lsp"` + `"tools"`
- **模型工具**：`lsp`（按需）

Grok `LspManager`/`dispatch`。第一次调用时读 `~/.dock/lsp.json` 与 `<cwd>/.dock/lsp.json`（项目盖用户）；没有配置则按工作区标记探测 PATH 上的 `rust-analyzer` / `typescript-language-server` / `gopls` / `pyright-langserver`（标记可在子目录，跳过 `node_modules` / `target`）。`/lsp` 把缺的服务器写入项目 `.dock/lsp.json`（不覆盖已有条目）；`/lsp user` 写 `~/.dock/lsp.json`。`search_replace` / `write_file` 之后后台 `didChange`。相对路径按 cwd 展开。没服务器时 fail-open（工具仍注册，调用返回配置说明）。`register_deferred`

## 配置样例

`lsp.json` 例（`~/.dock/lsp.json` 或项目 `.dock/lsp.json`；也可 `{ "lspServers": { … } }`）。没写时按工作区标记探测 PATH：

```json
{
  "rust-analyzer": {
    "command": "rust-analyzer",
    "extensionToLanguage": { ".rs": "rust" }
  }
}
```
