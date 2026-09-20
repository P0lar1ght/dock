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
  global/{topics,observations}/ + MEMORY.md
  workspace-<slug>/{topics,observations}/ + MEMORY.md
```

`workspace-<slug>`：优先 git `origin` 的 `org/repo` + blake3 短哈希，否则 cwd 路径哈希。

每个 scope 根目录的 `MEMORY.md` 是生成的精简索引（标题 + 相对路径摘要），在 `/remember`、`/flush`、`/dream` 与 `ensure_layout` 后刷新。工具与模型只读；`memory_get` / `/memory` 可展示。

旧版扁平 `~/.dock/memory` / `.dock/memory` **只读兼容**，不再双写。

## 系统提示 `<memory>`

记忆开启时，`tool-memory` 经 `ContextBook::section` 注入 `<memory>…</memory>`（路径说明 + 各 scope 的 `MEMORY.md` 正文，超长截断；缺失则仅路径提示）。关闭时不注入。

## 斜杠

| 命令 | 作用 |
|---|---|
| `/flush` | LLM 摘要当前会话 → workspace `observations/` |
| `/dream` | 手动 consolidate observations → `topics/` |
| `/memory` | 双栏浏览器：左文件列表（global/workspace），右 markdown 预览（`cordis_markdown`）。窄屏单栏；`↑↓`/`Enter` 选择；`/` 过滤文件名；`Esc` 关闭；可选 `t` 会话开关记忆 |
| `/remember` | 无参数：输入框留下用法；有参数：写一条 global observation |

压缩前若达到 flush 门槛会自动 `/flush`（失败不影响 compact）。compact 段仍写 `sessions/.../compaction/`。

## 工具

- `memory_search` — FTS5（无 embedding）。写入路径已 `reindex_file`；搜索只 open+query，DB 缺失或空时才全树 reindex 一次
- `memory_get` — 按路径读文件

经 `search_tool` / `use_tool` 按需暴露（`register_deferred`）。
