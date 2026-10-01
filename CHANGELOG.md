# Changelog

本文件记录用户可见的变化。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号用[语义化版本](https://semver.org/lang/zh-CN/)。

0.x 期间不承诺 `config.toml` 的键与 `dock.1` 协议的向后兼容：破坏性变更会写进对应版本，
并在升级说明里给出改法。

## [Unreleased]

### 变更

- **浏览器工具改成内置 MCP**：`dock mcp browser`（新 crate `cordis-browser`）。
  - Dock 自动注入内置行 `[mcp_servers.browser]`，零配置；`DOCK_BROWSER_MCP=off` 关掉。
  - 工具公名从 `browser_*` 变成 `mcp_browser__browser_*`，照旧经 `search_tool` / `use_tool`。
  - 进程内的 `browser_*` 已删除；预设允许名单里不再列它们。
  - 权限门随之改名：`mcp_browser__browser_evaluate` 与 bash 同级。
- **按会话分标签页**：一个 Chromium、共用登录态，每个会话只看得到、只关得掉自己的标签页。
- **多个 Dock 共用一个 Chromium**：第二个进程读 `DevToolsActivePort` 连上已开的那个，不再抢 profile。
- `/browser` 驾驶舱只显示内置 MCP 的状态与有头 / 无头偏好，不再列标签页和截图。

### 新增

- **网关浏览器画面** `browser/view/*`（能力 `browserView`）：看会话正在用的标签页、接手操作。
  - CDP screencast 推 JPEG 帧；鼠标 / 滚轮 / 按键 / 文字转回页面；地址栏、前进后退刷新。
  - agent 换标签页画面跟着换；帧写出去才让 Chrome 发下一帧。
  - 本地、远程、网页端同一条路，不需要客户端碰 CDP 端口。

### 新增（远程）

- **`dock serve --remote`**：给别的机器上的 GUI / 浏览器 UI 连的常驻网关。
  - 仍只绑回环（默认 `127.0.0.1:18990`，占用直接报错）；TLS 交给反向代理或 Tailscale。
  - 只认设备令牌，不挂配对与 ticket 的 HTTP 路由；跑到 SIGTERM 为止。
- **设备令牌** `dock device add|list|revoke`：盘上只存 sha256；撤销立刻生效并断开连接。
  - 同一条连接鉴权失败 5 次断开。
- SDK：`DockClient({ deviceToken, gatewayUrl: 'https://…' })`；令牌模式对非回环地址强制 https。
- 部署说明：`docs/REMOTE.md`（Caddy / nginx / systemd / Tailscale）。

### 修复

- MCP 工具的 `isError: true` 现在会把结果标成失败；以前只有正文以 `Error:` 开头才算。

## [0.1.2] - 2026-09-29

### 新增

- **长期记忆换成 `dock-memory`**：以前是「递归 grep `~/.dock/memory` 下的 md/txt」的纯关键词搜索，
  现在是有索引的检索。落盘布局改成 `$DOCK_HOME/memory/{global,workspace-<slug>}/{topics,
  observations/_inbox,archive}/`，每个 scope 一份生成的 `MEMORY.md` 索引（标题 + 相对路径摘要）；
  检索是 FTS5 + MMR，配了 `[memory.embedding]`（model / base / api_key / dimensions，或
  `DOCK_MEMORY_EMBEDDING_API_KEY`）再叠加 sqlite-vec 向量混合，不配就只走关键词；文件被外部
  改动后写入 watcher 的 dirty 集，下次搜索前增量同步。**默认关闭**，`[memory] enabled = true`
  或 `DOCK_MEMORY=1` 打开（`DOCK_MEMORY=0` 是进程级强制关，会话里也开不回来）。
- **长期记忆怎么进上下文**：不再塞进系统提示，改挂在 `agent/step-start`（order 7）的
  `<system-reminder>` 上，并且与 `AGENTS.md` 一样排在**第一条用户消息之前**，让跨会话的这段
  前缀能命中缓存；`/context` 单列一行「记忆」，点开看实际注入的原文。
- **记忆的四条斜杠**：`/flush`（把本会话要点写进 workspace 的 `observations/`，压缩前达门槛也会
  自动 flush）、`/dream`（把 observations 整理成 topics）、`/memory`（双栏浏览：左列表右预览，
  `/` 按文件名或正文过滤，`t` 是会话开关）、`/remember <note>`（让模型改写成结构化 markdown
  后落一条 global observation，改写失败就存原文）。
- **`/memory` 里能删条目**：`x` 连按两次删掉选中的 topic / inbox note —— 归档进 `archive/`、
  在 `memory_state.sqlite` 留墓碑、从索引里删行；`MEMORY.md`、`archive/` 与两个 db 只读，
  可写范围只有 `topics/*.md` 与 `observations/_inbox/*.md`。
- **`memory_search` / `memory_get` 变成常驻工具**：记忆启用时直接 `register` 进工具表，不再需要
  `search_tool` 发现（启用开关与 `t` 都会即时反映到表里）；`memory_search` 的结果在 TUI 里画成
  Grok 风格的折叠卡，`memory_get` 走 Read 卡。
- **多会话分页（tabs）完全隔离**：goal、todos、plan-mode、模型与协议、权限模式与权限队列、
  提问队列都按页各一份 —— 后台页弹的批准框 / 提问不再串进正在看的那一页；工具表、LLM、MCP
  连接、浏览器、cua、后台任务仍全局一份。标签栏用 `◆` 标出有待回答的页。
- **分页各自落盘**：常驻分页按**自己的 cwd** 落盘，关页或退出后都能在 `/resume` 与会话面板里
  找到，从历史会话开的页接着写回原会话目录；`/btw` 旁问页仍不落盘。dashboard 的历史会话按
  `Enter` 开成**自己的一页**（原来只是恢复到当前页），已经开着的再按只切过去，别的 cwd 的行尾
  标「其它目录」并提示先 `/cd`。
- **`dock serve` 无头入口**：`dock serve --origin <origin> [--application <id>] [--bind <addr>]`
  不挂 TUI，网关挂载即在回环上监听（默认端口交给 OS 分配）；stdout 一行一个 JSON，先报
  `ready`（含 ws / ticket / 过期时间），之后 stdin 每写一行 `{"cmd":"ticket"}` 回一张新 ticket，
  stdin 关掉即退出。ticket 只走这条管道交给父进程，不开放 HTTP 领取。桌面 GUI 就靠它拉起单个
  Dock 进程、跨目录列会话。
- **`dock.1` 改按会话开页**：`thread/start` 另开一页、`thread/open` 打开关着的会话、
  `thread/close` 关页、`thread/list {scope:"all"}` 列开着的页加跨目录名册（第 1 页的别名仍是
  `live`），每页一份投影；`turn/start|enqueue|steer`、`environment/*`、`slash/execute` 对关着的
  会话**按需开页**，而 `turn/cancel`、`queue/*` 这类不产生内容的调用不开页。
- **网关的查询与状态**：`thread/search` 按消息内容搜会话（中文按子串匹配，空格分词要全命中）；
  `thread/history` 对关着的会话也回放成 `events`（按真实时间、每轮以 `turn/completed` 收尾，
  重开会话不再挂着一轮）；`thread/rename` / `thread/delete` 对别的 cwd 的关着会话也能用。
- **一轮的结果落盘**：每轮结束往 `chat_history.jsonl` 写一行 `turn-end`（completed / cancelled /
  failed，失败时带 `error` 原文），关着的会话回放时能看出这一轮是停止还是出错；旧会话没有这行
  就整行跳过，读法照旧。
- **一轮的结果推给浏览器**：`turn/completed` 带 `status`（completed / cancelled / failed）与失败
  的 `error` 原文；工具完成状态 `item/tool_completed.status` 由落盘的 `is_error` 与权限拒绝判定，
  报 completed / failed / cancelled / denied；模型的思考过程新增 `item/reasoning_delta` 增量推送；
  提问支持多选（`interaction/requested` 每题带 `multiSelect`）。
- **预设（`presets`）从只读变可写**：新增 `preset/list`（带 builtin / origin / icon，另给
  `defaultId`）、`preset/create`（可 `basedOn` 复制一份人设 / 工具 / 子代理）、`preset/delete`
  （删掉覆盖层即恢复内置，没改过的内置拒删）、`preset/get` / `preset/update`（整份读写，损坏
  预设可整份重写）、`tool/catalog`（resident / deferred / dynamic），以及经 "llm" 采样一次的
  `preset/draft` / `preset/rewrite` / `preset/suggestTools`（AI 起草预设、改写提示词、按目录推荐
  工具；跑在隔离会话里，不写盘）。`agent.yml` 多一个可选 `icon`。
- **设置页要的数据**：`mcp/list`（每台的连接状态、工具数，不含启动命令）、`mcp/reconnect`、
  `model/list`（只给鉴权类型，不给密钥）。
- **动态插件的 Rhai 能力扩了一圈**：脚本可自己发 HTTP（`http_request`，禁跟随重定向）、正则
  一族、Blob body / multipart 上传与 `host.read_bytes` 按路径读文件、codecs 与 `hmac_sha256`
  （签名 query / Basic auth）、`unix_time` / `utc_now` / `utc_date`、`host.secret`（读
  `secrets.json` 或 `DOCK_SECRET_*`，首次读走 Ask 且摘要不带内容）；`cordis_define` 支持
  `source_path` 指向磁盘上的脚本文件，不必再把大段 Rhai 塞进参数。
- **`read_file` 支持文档与图片**：按字节判断类型 —— 图片压进多模态结果、PDF 解析（新 `format`
  默认 `image`，按页渲染成图，10 页以上必须给 `pages`，单次最多 20 页；`format=text` 仍抽文本）、
  PPTX 按 `--- Slide N ---` 输出含备注，docx 这类不认识的二进制直接拒；新增 `pages` / `format`
  两个参数。
- **会话记住模型与推理强度**：`meta.json` 新增 `model` / `effort`，切完立刻写回，`/resume`、
  `--resume`、网关开页时切回来（模型已经不在 `config.toml` 里就留着当前的，不报错）。
- **`/undo`（别名 `/rewind`）与空闲 Esc**：撤销上一轮**没有模型输出**的用户消息 —— 从会话日志
  里摘掉并还原回输入框（含图片，图片 chip 重新绑定回输入框）。只有请求失败、PreStep、提醒不算
  输出，所以 provider 报错那轮可以直接撤了重发，不必 `/new`；底栏可撤时显示 `Esc:undo`。
- **只读场景也给 `bash`**：计划模式开着、子会话能力档是 `read-only`、或角色预设标了
  `read_only: true`（内置 `explore` / `plan`、`/btw` 页）时，判定为只读的命令（白名单程序、无写盘
  重定向、无命令替换与后台、无 `git commit` / `sed -i` 这类改动参数）直接跑，其余一律问用户；
  自动批准与「以后都允许」在这里不算数。
- **压缩分段与会话全文搜索**：compact 成功后往会话目录写 `compaction/segment_NNN.md` 与
  `INDEX.md`；`/resume` 与会话面板的搜索建 FTS 索引（随会话目录重建），按用户提示词内容也能
  命中，不再只搜标题。
- **`dock-core`**：新的 TS 包，把 `dock.1` 的线程事件做成类型化契约外加线程状态 reducer，
  无运行时依赖、Node 22 直跑 `.ts`；`npm test` 与 `npm run typecheck` 进了 CI。
- **提问 overlay 支持多选**：多选画 `□` / `☑`，「其他」是真输入框（有光标与聚焦框，输入法
  候选条不再飘位），`Space` / `→` 切题、`Esc` 逐级退。
- **权限框的摘要**：长参数按 grok 风格做引号感知换行，动作与角色 / 标签高亮，长 token 隐去。

### 变更

- **子代理只走信箱**：`report` 工具删除，并入方向中立的 `send_message({agent_id, message})`；
  「做完要回报」变成写进子代理初始任务的一句话。子代理不再是作业（`job` / `kill_task` 只管
  后台 bash 与 monitor，列不出子代理），id 一族一名字（子代理 `agent_id`、作业 `job_id`）。
  排队回执的字幕从「本轮结束后执行」改成「运行中，下一步读到」；文档同时写明**分页父信箱尚未
  端到端** —— 从 `main#N` 派出的子代理回话进的是全局父信箱，只有 `main` 去取。
- **`/cd` 不再改进程 cwd**：进程从启动起 cwd 就不动，目录挂在会话上 —— 工具、系统提示的辅助
  函数、子进程、TUI 视图都读会话自己的 cwd；`/cd` 只钉当前这一页，新分页继承开它那页的目录，
  子代理继承父会话。项目级资源（`.dock/config.toml`、skills、动态插件）只认启动目录，`/cd` 到
  带这些的目录会提示「只在启动目录生效」。
- **计划文件按会话 / 页落盘**：主会话在 `sessions/<cwd-key>/<id>/plan.md`，不落盘的分页在
  `tabs/<pid>/<main#N>/plan.md`；`/view-plan` 看本页的计划，计划写门只豁免本页那一个文件。
- **`cordis_*` 从八颗收成六颗**：`cordis_inspect_self` 并入 `cordis_inspect`（给 pluginId /
  packageId），`cordis_undefine` 并入 `cordis_stop(drop: true)`；两者都不删磁盘文件。目的是让
  这几颗挤进 `search_tool` 的默认命中数。
- **工具卡失败改读 `is_error`**：工具结果新增 `is_error` 并落盘，TUI 不再按输出文本猜成败
  （删掉 execute / mcp / search_tool / memory_search 四份各自的规则）。
- **技能与规约路径整读**：`read_file` 对 `SKILL.md`、`AGENTS.md` 这类路径在 token 上限内整份读完，
  不再被 1000 行截断。
- **`inject` 明确是启动闸不是授权**：`waiting` 以内核 fiber 状态为准不是权限；`cordis_inspect`
  的 services 分「reachable」「also mounted」。
- **dashboard 滚轮按落点分工**：落在对话区滚对话、落在列表区移选中行、落在别处不动；
  `Ctrl+J` / `Ctrl+K` 按焦点分工；peek 从只看尾巴改成看整段。
- **工具链下限提到 rustc 1.94**（`[workspace.package].rust-version`，CI 与 release 钉 1.94.0）。
- **主任务 dock 的 `[✗]`** 走与 `/tasks` 相同的 kill_task / Jobs 路径，不再是一套私有取消。
- **输入框焦点改成事件驱动**：只在 working 边沿变化，回合中点输入框能聚焦、回合结束空框自动还焦；
  聚焦边框对比拉高，点滚动区失焦、点回框里夺焦。

### 修复

- **中文记忆搜索恒为空**：FTS5 的 unicode61 不切 CJK，写入与查询两侧把连续中文展开成重叠二元组；
  含 FTS5 语法字符的词按 phrase 转义。
- **记忆假命中**：多词查询从 OR 改成 AND，FTS-only 的分数下限抬高，生成的 `MEMORY.md` 不再
  参与排序归一化。
- **记忆的体量门**：`memory_get` 只收 memory 根下的 `.md`、超过 256 KiB 拒绝；`/memory` 的删除
  对超过 256 KiB 的文件直接拒绝（不整份读进来算 hash）；记忆相关的错误与通知全部改成中文。
- **流式回复在浏览器里重复**：用量更新不再多发一次空的 `LlmStream`，一轮只发一次
  `turn/completed`。
- **`mcp/reload` 以前什么都没重载**（只回服务器个数），现在真的重载并回报
  added / removed / reconnected / disabled / failed。
- **`http_request` 禁跟随重定向**：SSRF 与 host 门只护得住第一个 URL，3xx 现在原样返回。
- **`regex_replace` 的 UTF-8**：`$n` 替换模板按字节拼接会把中文写坏，改成按字符。
- **关着的会话改名 / 删除**：在别的 cwd 的会话以前一律回落 not_found，现在按名册找到并用会话
  自带的 cwd 改 `meta.json` 或删目录。
- **没有对话内容的会话不再落盘**：只有「MCP 已连接」这类后台提醒、提示词组装或 turn-end 记账的
  会话不写盘（写过的删掉），会话列表不再冒空会话（归档同一条规则）。
- **恢复时应用会话记的预设**：TUI `/resume`、进程 `--resume`、网关恢复、slash resume 四条路
  统一走 `apply_restored_preset`。
- **恢复撞车**：别的页正 live 在同一份会话上时切过去，而不是再恢复一份（两页同时追加同一个
  `chat_history.jsonl` 会弄坏历史）。
- **MCP elicitation 串页**：串行化按 `tools/call` 的 id 归属，而不是按连接把整台服务器串起来 ——
  两页可以同时调用、各弹各的框；关页时这一页未答的 elicitation 会被 cancel 掉，不再卡住工具调用。
- **分页杂项**：跨进程的计划目录、关页残留、写门路径（`..` 与 macOS `/var` ↔ `/private/var`
  符号链接前缀不再误挡）等一批 review 问题。
- **流传输失败**改放进 `LlmOutput.error`，不再当正文写进 wire 历史（此前它会挡住 Esc 取消-收回）。
- **模型选不到的报错**：模型不在目录里又没有地址时直说，并指向 `config.toml` 的
  `[model."<id>"]`；在目录里但当前协议没配地址就说缺哪个 `api_base_url`，不再报
  `relative URL without a base`。
- **Mermaid 流程图**的内联排版不再被换行拆断，操作行紧跟图表并能鼠标悬停高亮。
- **`ask` 提问**的单选不再画多个实心点，「其他」有真实焦点，`Space` / `→` 切题。
- **CI**：显式安装 rustfmt / clippy 组件。

### 破坏性变更（升级注意）

- 记忆落盘布局从扁平的 `~/.dock/memory/*.md` / `.dock/memory/` 换成
  `$DOCK_HOME/memory/{global,workspace-<slug>}/`。旧路径仍然**只读**兼容（`memory_get` 按路径
  还能读），但不再被索引，也不会再双写；旧路径下的 `.txt` 不再可读。想继续用就把文件搬进新布局
  的 `topics/` 或 `observations/_inbox/`。
- 子代理协议：`report` 没了，用 `send_message` 回话；`subagent_id` / `task_id` 分别改成
  `agent_id` / `job_id`（旧字段仍收）。旧的 `agents/*.yml` 里写着 `report` 的仍能读。
- 动态插件：`cordis_inspect_self` → `cordis_inspect`，`cordis_undefine` → `cordis_stop(drop:true)`。
- `dock.1`：`turn/completed` 改成**一轮一次**（以前每个「有文本、没工具」的流式片段后都发，
  客户端按它切分会看到轮数变少）；`item/tool_completed.status` 是四值枚举，按输出文本猜成败的
  客户端必须改；`scope:"all"` 的列表会带回跨目录会话（不带 scope 仍是旧语义）。
- `/cd` 不再改进程 cwd，项目级 `.dock/` 配置与 skills 只在启动目录生效；计划文件位置按会话 / 页
  划分（旧的 `.dock/plan.md` 相对路径仍被写门认作兼容）。
- 从源码构建需要 rustc 1.94+。

## [0.1.1] - 2026-09-19

### 新增

- **`/computer` 自己会装 cua-driver**：驾驶舱变成状态机（未挂载 / 未安装 / 缺授权 / 已禁用 /
  未连上 / 已连接），`i` 走 trycua 官方脚本装或重装，`p`（macOS）让 driver 自己拉起
  Accessibility / 屏幕录制授权对话框。两个动作都是**两步**：先列出要执行的每一步，`Enter` 才真的跑，
  `Esc` 只取消确认；进度一行行显示在驾驶舱里，装完自动重新探测 + 重载 MCP，不用重启 dock。
- **cua-driver 零配置接入**：Dock 启动时自己发现本机 driver（`DOCK_CUA_DRIVER` → `PATH` →
  `~/.local/bin` → macOS `/Applications/CuaDriver.app`），找得到就注入内置
  `[mcp_servers.cua-driver]`，不必再手写 `config.toml`；`DOCK_CUA_DRIVER=off` 彻底关掉。
  Dock 的 release **不带** driver 二进制。
- 桌面类关键词（click / screenshot / 桌面 …）搜不到工具且 driver 没连上时，`search_tool` 的 note
  会直接指向 `/computer` 的安装键。
- **覆盖层右上角的 `[✗]` 支持鼠标悬停高亮**：指针落在按钮上提亮加粗，移开复位；悬停区与点击区
  共用各覆盖层已上报的 `close_button` 矩形，新覆盖层照常返回该矩形就自动有悬停反馈。

### 变更

- `/computer` 的 `Ctrl+R` 重新探测本机 driver 并重载 MCP 配置；打开驾驶舱也会静默重探一次
  （在 dock 外面装好、授权好的 driver 这样接上）。
- `/mcps` 里禁用内置的 cua-driver 行时，Dock 把完整一行（`command` / `args` / `enabled = false`）
  落进用户 `config.toml`，此后该行归配置文件管。
- **`/usage` 的未命中按来源拆开**：会话总计折了子代理与压缩，而「上一轮」和每轮命中率
  走势只有主循环，两个口径并排摆着容易被读成「主循环每轮都在漏 token」。现在多一行
  `未命中来源: 主循环 N · 子代理 M · 压缩 K`，只有真有子代理 / 压缩时才出现。
- **压缩（auto-compact）开始记账**：它是一次整段历史、几乎零命中的满价请求，且之后那一轮
  必然全量重算。它隔离掉了 `"sessions"`，以前用量整笔掉进黑洞——`/usage` 里看不到开销，
  命中率掉格也无从归因。现在它进总计与 `模型调用`（标成「压缩 N」），但仍不是用户的一轮：
  不进 `numTurns`、不占走势格子。
- **不报 cache write 的 wire 上注明口径**：`chat/completions` 与 Responses 不单独上报
  `cache_creation`，这一轮新写进缓存的 token 全落在「未命中」里。overlay 现在在这一段标
  「含首次写入（这条 wire 不单列）」，不然会被当成和 Claude Code `/cost` 的 `input` 同一个口径。
- **`DOCK_CACHE_DEBUG`**：默认关的诊断日志。把每次请求与同一会话上一次逐条比对，第一处不同
  报下标与字节偏移，并把上游 read / write / miss 贴在同一条记录下，写进
  `$DOCK_HOME/scratch/cache-debug.log`（设成路径则写到该路径）。命中率掉下去时用来分辨
  「前缀真被改写了」还是「上游计数问题」。**日志含对话片段**，只在本机调试用。

### 修复

- **Stop 后不再把假文本留在历史里**：请求已发出但响应头还没回时按 Stop（插队发送也走这条），
  采样器以前返回 `"cancelled"` 占位文本，`finish_llm` 会把它填进本轮那条助手记录——历史里
  从此留着一句模型从没说过的话，`seal_incomplete_tool_calls` 不再弹出空槽位，cancel-rewind
  （把提示词还回输入框）也一起失效。现在取消返回空输出。

## [0.1.0] - 2026-09-14

首个公开版本。macOS（Apple Silicon / Intel）与 Linux（x86_64 / aarch64）有预编译二进制，
`install.sh` 一键安装；也可以从源码 `cargo run -p cordis-app`。

### 新增

- **一切皆插件**：内核 crate `cordis`（`Context`、`inject`、named service、waterfall、fiber
  生命周期），产品面由 `install_app` 挂成插件树；`agent/pre-step`、`agent/step-start`、
  `agent/turn-end`、`llm/stream`、`tools/execute`、`system-prompt/assemble` 六个扩展点。
- **Grok 外形 TUI**：主题、scrollback 卡片、prompt、overlay、底栏、快捷键；斜杠命令目录见
  `CLI.md`，模型工具见 `TOOLS.md`。
- **模型目录由 `config.toml` 声明**：一个端点可同时声明 `responses` / `chat_completions` /
  `messages` 三条协议并在运行时切；能力（`context_window`、`reasoning`、`supports_images`、
  单价）写在模型级；不写就按 128k 估、不发该参数。
- **工具与 MCP**：`search_tool` 走 BM25 索引、schema 去重与输出预算；MCP 是 fail-open ——
  没有 `[mcp_servers.*]` 时仍挂载，连不上不影响启动。
- **动态 Cordis 插件**：会话内定义 Package（预设或 Rhai）→ 运行 → 提升为磁盘插件，
  支持 `/` 命令、`tui.slots`、`agent/*` 扩展点；工作流见 `skills/cordis-plugin-development/`。
- **会话与恢复**：`~/.dock/sessions/<cwd>/` 落盘，`dock --resume [id]` 恢复。
- **浏览器 companion**：进程内 loopback 网关（默认不监听，`/pair` 开启），`dock.1` 协议 +
  `embed-sdk` 宿主页 SDK。
- **技能与工作流**：`skills/`、`.dock/skills/`、`~/.dock/skills/`、`.agents/skills/` 五层发现；
  内置技能编译期嵌入，启动物化到 `~/.dock/bundled/skills/`。

### 变更

- 对外二进制名定为 **`dock`**（此前是 `cordis-tui`）；新增 `dock --version`。
- 七个第一方 crate 的版本号统一由 `[workspace.package].version` 提供
  （各自改为 `version.workspace = true`），发版只需改一处。
- CI 只在 `main` push 与 PR 上跑门禁；发版另由 `.github/workflows/release.yml` 在 `v*` tag 上
  触发，先复述门禁并校验 tag 与 workspace 版本一致，再构建四平台产物。

### 已知限制

- 只发 macOS 与 Linux；Windows 未做适配验证。
- Linux 预编译产物动态链系统库，有两条运行下限：**glibc ≥ 2.39**（构建机是
  `ubuntu-24.04` / `ubuntu-24.04-arm`，两个架构同档）与 **OpenSSL 3**
  （`libssl.so.3` / `libcrypto.so.3`，来自 `cordis-spine` 的 reqwest `default-tls`）。
  实际可用范围约等于 Ubuntu 24.04+ / Debian 13+ / Fedora 40+；更老的发行版
  （Ubuntu 22.04、Debian 12、RHEL 9、Amazon Linux 2）请从源码构建。`install.sh` 装完会跑一次
  `dock --version`，跑不起来会直接报错退出，不会假装装好了。
- macOS 产物未做代码签名 / 公证，Gatekeeper 首次运行可能拦截（`install.sh` 会给出
  `xattr -d com.apple.quarantine` 提示）。
- 需要浏览器 companion 时，宿主页 SDK 要自己 build `embed-sdk/`（`npm ci && npm run build`），
  它不随二进制发布。
