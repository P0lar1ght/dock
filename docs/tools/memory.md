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
  - `path` 除了 `memory_search` 给的绝对路径，也认笔记引用，见下「笔记之间的链接」。

Memory **启用**时两颗工具 `register` 进 sampler 工具表（模型可直接调用，无需 `search_tool`/`use_tool`）。`DOCK_MEMORY=0` / 配置关闭 / 会话 `t` 关掉：从表移除（或调用返回 disabled）。会话 `t` 再打开则恢复常驻。

## 笔记之间的链接

笔记用 `[[文件名]]` 互相链接，语法和 Obsidian 一样。
记忆目录可以直接当 Obsidian 的 vault 打开。

- `[[prefs]]`：文件名不带 `.md`，不分大小写。
- `[[prefs|显示的字]]`：换显示文字。
- `[[prefs#小标题]]`：带小标题（解析时只看文件名）。
- `[[topics/prefs]]`、`[[global/topics/prefs]]`：带路径，点名某一篇。
- 围栏代码块和行内代码里的 `[[…]]` 不算链接。

同名时取谁：

- 先取链接所在 scope 里的，再取全局的，最后才是别的项目的。
- 同一档里主题（`topics/`）优先于观察。
- 文件名对不上时按标题再找一次（`[[审计日志改造]]`）。

谁会写链接：

- agent：记忆提醒里写了这条约定，写笔记时和索引里已有的笔记有关就链过去。
- `/flush`：只链对话里原样出现过文件名的笔记，不编文件名。
- `/dream`：保留输入里已有的链接；主题之间用标题互相链接。
- 用户：在 GUI 的「记忆」页里建链 / 断链，或者自己编辑文件。

`memory_get` 的 `path` 可以直接给 `[[链接]]`、笔记名，
或索引里那种相对路径（`topics/prefs.md`）；相对路径一律按笔记引用找，不看进程的当前目录。
在全局和当前工作区里找；回包的 `**File:**` 是解析出来的真实路径。

实现：`dock-memory/src/links.rs`（解析与改写）、`vault.rs`（对到具体文件）。

## 记忆图谱与管理（GUI）

桌面 GUI 侧栏的「记忆」页。走 dock.1 的 `memory/*`，只认受信连接。
方法与回包见 `cordis-gateway/README.md`「记忆图谱」。

图上有什么：

- 点 = 笔记。颜色分当前项目 / 全局 / 其他项目；主题实心，观察空心。
- 点的大小跟链接数走；没有链接的笔记散在外圈。
- 实线 = 链接，有方向（谁引用谁）；箭头默认只画悬停 / 选中那篇的。
- 虚线 = 相似建议：内容相近、还没链上的两篇。默认只画悬停 / 选中那篇的。
- 虚线圆 = 链接指向的笔记还不存在。
- 标签随缩放出现：链接多的大点先标；悬停 / 选中那篇和它的邻居总是标。

能做什么：

- 看：默认「当前项目 + 全局」，可切到全部项目；图谱 / 列表两种视图；按标题和正文搜。
- 改：预览与编辑、新建、重命名、观察升为主题、移到全局 / 当前项目、遗忘。
- 链接：抽屉里「链接到…」、点虚线把建议转成链接、点实线断开、按住 ⌥ 从一篇拖到另一篇。
- 局部图谱：抽屉顶上画选中这篇 1–2 跳以内的邻居和它的相似建议，点一篇跳过去。
- 图谱设置（画布右上角）：箭头、全图显示相似建议、还没写的笔记、标签多少、点大小、线粗。
- 力导向：向心力、斥力、连线拉力、连线长度。设置记在本机。
- 按条件着色：最多 3 组搜索条件（同搜索框），命中的上色、其余变灰。
- 只给 3 组：图上任意两点都可能挨着，颜色再多在色觉障碍下分不清。
- 重命名文件时，别的笔记里指向它的链接跟着改写。
- 挪位置时，它指向别人的、别人指向它的链接都保持指向原来那几篇。

和开关的关系：

- 记忆关着也能看、能改：文件就在盘上，开关管的是 agent 用不用它。
- 只有直接放在 `topics/`、`observations/_inbox/` 下的 `.md` 能改。
- 嵌套目录和旧版平铺的 `observations/*.md` 只读。
- 遗忘和 `/memory` 里按两次 `x` 是同一条路：归档 + 墓碑 + 清检索条目。

实时：agent 或外部编辑器改了笔记，Dock 推 `memory/changed`，图就地更新。
生成的 `MEMORY.md` 不触发推送（记忆开着时每一步都会重写它）。

### 相似建议

不依赖嵌入模型：词频—逆文档频率向量的余弦相似度。
中文按相邻两字切，英文和标识符按词。

- 每篇最多连 3 条，分数低于 0.1 的不连（拿真实记忆目录校准）。
- 已经有链接的两篇不再建议。
- 笔记超过 1500 篇不算（回包 `similarSkipped: true`）。

实现：`dock-memory/src/similarity.rs`、`graph.rs`。

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
