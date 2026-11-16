# dock-memory

Dock 的跨会话记忆引擎。crate 是纯引擎（不依赖内核 `cordis`，只依赖 `cordis-base`），对 spine 的工具面在 `cordis-spine/src/tools/memory/` 里挂。

## 布局

`$DOCK_HOME/memory/`（默认 `~/.dock/memory/`）：

```text
memory/
  search.sqlite          # 共享检索库（FTS5 + 可选 sqlite-vec KNN），跨 scope
  global/                # 全局 scope
    topics/              # 稳定的主题笔记
    observations/_inbox/ # 新观察落这里
    archive/             # dream / forget 的去向
    MEMORY.md            # 生成的索引清单（只读，勿手改）
    memory_state.sqlite  # 仅存 revisions / tombstones
  workspace-<slug>/      # 工作区 scope，结构同上
```

两个库分工明确，**不要混为一谈**：

- `search.sqlite` —— 真正的检索索引（`chunks` 表 + 向量列）
- `<scope>/memory_state.sqlite` —— 只记录删除墓碑与版本，`memory_tombstones` / `meta`（`schema_version`）

## 检索

`MemoryIndex`（`index.rs`）：

- `init_sqlite_vec()` 进程级注册一次（安全可重复调），之后 `open_or_create`
- sqlite-vec 可用时走 **FTS + 向量 KNN 混合**；不可用自动降级为 **FTS-only**（`tracing::warn` 提示，不报错）
- sqlite-vec 被 pin 在 `=0.1.7-alpha.2`，升级必须重新验证（源码里有 SAFETY 注释说明 ABI 假设）

检索管线（`search.rs` + 相关模块）：

| 模块 | 职责 |
|---|---|
| `search.rs` | `search_memory` / `search_memory_with_config`，结果格式化 |
| `query_expansion.rs` | 查询扩展 |
| `mmr.rs` | 最大边际相关性去重（`MmrConfig`，默认开） |
| `keywords.rs` | 关键词抽取 |
| `chunker.rs` / `tokenize.rs` | 分块与分词 |
| `embedding.rs` | 向量嵌入（`EmbeddingProvider` / `ApiEmbeddingProvider`，`embed_missing_chunks`） |
| `access.rs` | `forget` / 归档 / 路径分类（`PathClass`、`MemoryAccessPolicy`） |
| `manifest.rs` | `MEMORY.md` 生成与刷新（`refresh_all` / `regenerate_scope`，有预算限制） |
| `flush.rs` | flush 机制（`should_flush` / `process_flush_response`） |
| `dream.rs` | 记忆整理（`auto_dream_eligibility` / `process_dream_response`） |
| `watcher.rs` | `MemoryFileWatcher`：外部 `.md` 编辑后同步索引 |
| `storage.rs` | `persist_observation` / `save_remember_note` / `write_flush_observation` |

## 图谱与管理

跨 scope 的那一层：图谱页要看所有项目，不只是当前 cwd 的工作区。

| 模块 | 职责 |
|---|---|
| `links.rs` | `[[wikilink]]` 解析与改写（跳过代码块、行内代码） |
| `vault.rs` | `MemoryVault`：scope 清单、笔记 id、`LinkResolver`（链接对到哪篇） |
| `similarity.rs` | 相似建议（TF-IDF 余弦，中文按二字切） |
| `graph.rs` | `build_graph` / `note_links` / `search_notes` |
| `notes.rs` | 读、存、新建、改名、挪位置、遗忘、建链 / 断链 |
| `watcher.rs` | `MemoryChangeWatcher`：笔记一变就通知（忽略 `MEMORY.md`） |

笔记 id 是 `<scope 目录>/<相对路径>`，如 `global/topics/prefs.md`。
id 只在 `MemoryVault::entry` 里换成磁盘路径，换之前逐段校验、不跟符号链接。

写操作只认直接放在 `topics/`、`observations/_inbox/` 下的 `.md`。
改完同步检索库（库没建过就不建）并重新生成这个 scope 的 `MEMORY.md`。

`build_graph` 只读：不建目录、不碰检索库。

## 索引维度

`MemoryIndex::open_or_create` 沿用库里已有的嵌入维度。
只有明确给了别的维度（`open_or_create_with_dimensions`）才会重建向量表。

## 开关

- 配置 `[memory] enabled`，**默认关**
- 环境变量 `DOCK_MEMORY=1/0` 覆盖配置；`DOCK_MEMORY=0` 是**进程级强制关**，会话内无法再开启（工具返回中文错误 `"本进程已强制关闭记忆（DOCK_MEMORY=0）。"`）
- 开启时 `MemoryFileWatcher` 监视 `$DOCK_HOME/memory/`，外部编辑 `.md` 后自动重索引
- 工具名固定 `memory_search` / `memory_get`

## 相关文档

- 工具面（工具表、开关、参数、链接语法、图谱）：`docs/tools/memory.md`
- 布局与配置键：根 `AGENTS.md`、`config.toml.example`
- 消费侧（spine 挂载）：`cordis-spine/src/tools/memory/mod.rs`
