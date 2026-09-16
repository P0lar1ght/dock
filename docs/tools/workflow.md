# 工作流

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-workflow`

- **ctx**：`"workflows"` + `"tools"`
- **模型工具**：`workflow`

Grok Rhai 引擎（`vendor/xai/workflow`）+ 同款 oneshot ack。Host `SpawnAgent` live-lookup `"subagents"`，并把 `output_schema` 拼进子代理 prompt（JSON 输出会解析给脚本）；`model` / `effort` / `capability_mode` / `max_output_tokens` 被忽略（只记 debug 日志）。内置 `deep-research` 脚本在 `cordis-spine/src/workflow/workflows/deep_research.rhai`；磁盘扫描 bundled → 内置 → `{cwd}/.dock/workflows/<name>.rhai` → `~/.dock/workflows/`（同名不覆盖已有）。向 `"context"` 登记 listing 段（窗口 token ×4 ×**3%**，与技能共用 `src/listing.rs`）。同样两道门：**只有主会话拿这一段**（同 `listings: true` 开关），且 `workflow` 工具必须对本会话可见才发。每个目录项登记 slash extra（`kind: tool`，`text=workflow`；不可盖 `RESERVED_SLASH` / `/skills`）。`/name` 与 `/workflow <name>` 直接 `Tools::execute`，不经模型。`tools/execute` 路径靠近 workflows 目录时中途发现。`code` / `cordis` 允许名单含 `workflow`。**`register`（进 sampler）**——理由同 `skill`
