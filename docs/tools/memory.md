# Memory（Phase 1）

跨会话本地记忆。默认关闭。

## 启用

```toml
# ~/.dock/config.toml
[memory]
enabled = true
```

或 `DOCK_MEMORY=1`（`DOCK_MEMORY=0` 强制关）。

## 落盘

```
$DOCK_HOME/memory/
  search.sqlite
  global/{topics,observations}/
  workspace-<slug>/{topics,observations}/
```

`workspace-<slug>`：优先 git `origin` 的 `org/repo` + blake3 短哈希，否则 cwd 路径哈希。

旧版扁平 `~/.dock/memory` / `.dock/memory` **只读兼容**，不再双写。

## 斜杠

| 命令 | 作用 |
|---|---|
| `/flush` | LLM 摘要当前会话 → workspace `observations/` |
| `/dream` | 手动 consolidate observations → `topics/` |
| `/memory` | 只读浏览 list + preview |
| `/remember <note>` | 写一条 global observation |

压缩前若达到 flush 门槛会自动 `/flush`（失败不影响 compact）。compact 段仍写 `sessions/.../compaction/`。

## 工具

- `memory_search` — FTS5（无 embedding）
- `memory_get` — 按路径读文件

经 `search_tool` / `use_tool` 按需暴露（`register_deferred`）。
