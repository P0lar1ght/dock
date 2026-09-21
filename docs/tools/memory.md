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

## 系统提示 `<memory>`

记忆开启时，`tool-memory` 经 `ContextBook::section` 注入 `<memory>…</memory>`（路径说明 + 各 scope 的 `MEMORY.md` 正文，超长截断；缺失则仅路径提示）。关闭时不注入。

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
