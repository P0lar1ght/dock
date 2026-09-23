# Memory

跨会话本地记忆。默认关闭。

## 启用

```toml
# ~/.dock/config.toml
[memory]
enabled = true
```

或 `DOCK_MEMORY=1`（`DOCK_MEMORY=0` 强制关，进程级；会话内 `t` 无法再打开）。

## 落盘

```
$DOCK_HOME/memory/
  search.sqlite
  global/{topics,observations/_inbox,archive}/ + MEMORY.md + memory_state.sqlite
  workspace-<slug>/{topics,observations/_inbox,archive}/ + MEMORY.md + memory_state.sqlite
```

`workspace-<slug>`：优先 git `origin` 的 `org/repo` + blake3 短哈希，否则 cwd 路径哈希。

每个 scope 根目录的 `MEMORY.md` 是生成的精简索引（标题 + 相对路径摘要），在 `/remember`、`/flush`、`/dream` 与 `ensure_layout` 后刷新。工具与模型只读；`memory_get` / `/memory` 可展示。

旧版扁平 `~/.dock/memory` / `.dock/memory` **只读兼容**，不再双写。

## 注入：消息流里的 reminder，不进系统提示

记忆开启时，`tool-memory` 挂在 `agent/step-start`（order 7，排在工程规约之后、待办之前），把路径说明 + 各 scope 的 `MEMORY.md` 正文（超长截断；缺失则仅路径提示）包进 `<system-reminder>`，开头固定是 `<system-reminder>\n# 长期记忆`。关闭时不注入。放置与 `AGENTS.md` 相同（`StepStart::remind_preamble`）：会话还没向模型发过请求时排在第一条用户消息**之前**，新会话的请求头逐字节相同、跨会话命中前缀缓存；之后的注入追加在尾部。

规则与 `AGENTS.md` 相同：**只追加、不原地改写**。渲染结果与本会话历史里最近一份一致就不注入；记忆变了就在尾部追加新版本，旧副本留在原处；压缩吃掉旧副本后历史里找不到，下一步重新注入。它原来是系统提示里唯一随工作区（workspace scope）和每次 `MEMORY.md` 重新生成而变的一段，搬走后系统提示前缀不再因此作废。`MEMORY.md` 由会话内容沉淀而来，注入前与 `AGENTS.md` 走同一套中和：内容里的 `<system-reminder>` 变体会被转义，不能提前闭合或伪造提醒框架。

`/context` 里「记忆」单列一行（`MEMORY.md · 已计入消息`；开着但本会话还没注入时写「待轮次注入消息」），点开可看全局 / 工作区的 `MEMORY.md` 路径与实际注入的原文。

## 斜杠

| 命令 | 作用 |
|---|---|
| `/flush` | LLM 摘要当前会话 → workspace `observations/_inbox/` |
| `/dream` | 手动 consolidate observations → `topics/` |
| `/memory` | 双栏浏览器：左文件列表（global/workspace），右 markdown 预览（`cordis_markdown`）。窄屏单栏；`↑↓`/`Enter` 选择；`/` 过滤文件名；`Esc` 关闭；可选 `t` 会话开关记忆 |
| `/remember` | 无参数：输入框留下用法；有参数：写一条 global observation |

压缩前若达到 flush 门槛会自动 `/flush`（失败不影响 compact）。compact 段仍写 `sessions/.../compaction/`。

## 工具

- `memory_search` — FTS5；若 `[memory.embedding]` 已配置则查询侧 embed + hybrid；搜索前若 file watcher dirty 则 `sync_dirty_paths`。写入后 soft-fail `embed_missing_chunks`
- `memory_get` — 仅读取 memory 根下 `.md`（拒绝 sqlite/二进制）；体量上限 256KiB

Memory **启用**时两颗工具 `register` 进 sampler 工具表（模型可直接调用，无需 `search_tool`/`use_tool`）。`DOCK_MEMORY=0` / 配置关闭 / 会话 `t` 关掉：从表移除（或调用返回 disabled）。会话 `t` 再打开则恢复常驻。

## Safety gates

- `memory_get` / `read_memory_file`：仅 `.md`；超过 `MAX_MEMORY_READ_BYTES`（256KiB）拒绝。
- `/memory` forget：超过 `MAX_FORGET_FILE_BYTES`（256KiB）拒绝删除（不整文件读入再 hash）。


## /memory list labels

Inbox notes show as `global|workspace/observations/_inbox/<file>.md` (not bare `observations/<file>.md`).

## Hybrid search & embeddings

FTS5 is always on. Optional `[memory.embedding]` (model + base + API key) enables
sqlite-vec hybrid search; missing config stays FTS-only.

## Forget

In `/memory`, press `x` twice to delete the selected topic/inbox note (archives
under `archive/`, tombstones in `memory_state.sqlite`, drops index rows).
`MEMORY.md` is not deletable.

## Deferred (not in this tip)

auto-capture / auto-dream timer / drag_select.
