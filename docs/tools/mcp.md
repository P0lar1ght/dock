# MCP

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `mcp-client`

- **ctx**：`"mcp"` + `"tools"`
- **模型工具**：`search_tool` `use_tool`

stdio + Streamable HTTP。先走 MCP `2026-07-28`（无 initialize / 无 session，`_meta` + `MCP-Protocol-Version`）；服务器仍是 initialize 时代则回退 `2025-11-25`。fail-open。MCP 工具 `inputSchema` 原样 `register_mcp`，公名 `mcp_{server}__{tool}`，与 `register_deferred` 的本地工具一起 **不进** sampler `specs_for_model`。模型只看见静态描述的 `search_tool` / `use_tool`（`code` / `cordis` / `warden` 允许名单含这两项，`minimal` 不含）。`search_tool` 用 **BM25**（`bm25` crate，`Language::English`，标识符按 `__` / `_` / `-` / camelCase 拆词进文档与查询）匹配组/本名/描述/入参名，只返回命中项（每项完整 `input_schema`），limit 默认 5、最大 255；`total_hidden_tools` 是目录总数。零命中回退子串扫描，仍为零时 note 指出下一步（目录是英文，换 server / 能力名再搜）。**不重复发 schema**：命中的工具如果 schema 已在本会话模型历史里（`Sessions::model_history()` 扫既往 `search_tool` 结果，压缩后自动失效），该行回 `schema_in_context: true` 而不带 `input_schema`；**精确工具名搜索永远回完整 schema**，是重复搜索的逃生舱。输出另有 20KB 预算（按命中行计，JSON 外壳不计）：schema 先花掉其中 3/5，之后的命中降级成 `schema_omitted: "budget"`（只剩名字和短描述，仍可直接 `use_tool`），预算耗尽才整条不发、并在 note 里说清丢了几条。保住 `limit` 255 的 parity，又不至于一次把整个目录灌进历史。`use_tool` 调度 MCP 或按需本地工具，输出 20KB 帽：超帽时全文落 `$DOCK_HOME/tool-output/<call_id>.txt`，内联只留开头约 2KB 预览，提示给路径和接着读的行号（写盘失败退回内联 20KB）。第一类工具误走 `use_tool` 会纠正。`/mcps` Space 立刻 `dispose` 注销或追加注册，不改系统提示；目录变化在 `agent/pre-step` 与开关时写服务器级 `<system-reminder>`（已连接/已更新/已断开 + 数量，不含 schema）：**按会话记**（页号 + 落盘 id），每一页第一轮都会收到完整清单、写进正在跑的那一页（以前整个进程只发一次、只写第 1 页，GUI 另开的对话里模型不知道有浏览器）；内置的 `browser` / `cua-driver` 另带一句「什么时候用它」（`discover.rs` 的 `builtin_description`）。`tools/list` 跟 `nextCursor`（最多 64 页）；`notifications/tools/list_changed` 50ms 合并后重列。advertise `elicitation.form` + `elicitation.url`；stdio 读循环 / HTTP POST SSE / GET SSE 收 `elicitation/create`，**服务器请求交给单独任务处理**（回复 stdio 直接写、HTTP 走 `reply_tx`）—— 内联 await 会让读侧一直等到用户填完整张表单，期间连本次 `tools/call` 的结果都读不到。两页可以同时打同一台服务器。HTTP 上 `elicitation/create` 出现在那次 `tools/call` 的 POST 响应里，提问归发出这次调用的页，各页弹各页的框。stdio 是一条共享流：只有一路在飞时归那一页，多路同时在飞时按发送顺序各领一道还没人领的题。单次 JSON-RPC 有兜底超时：默认 600s，`DOCK_MCP_CALL_TIMEOUT_SECS=<秒数>` 覆盖（`0` = 不限），超时/放弃时按规范发 `notifications/cancelled`。工具执行本身由 agent 循环与 `TurnControl` 赛跑，所以 `[stop]` 对 MCP 调用同样生效，不必等服务器回话。HTTP GET 长连接；initialize 时代 `Mcp-Session-Id` 的 POST 404 会重新握手再试一次。HTTP 服务器 `i` 浏览器 PKCE OAuth（DCR 或 `oauth.clientId`），token 在 `~/.dock/mcp_credentials.json`，启动不自动开浏览器

启动不等握手：
- `mcp-client` 挂载时只登记服务器，握手在后台**并发**跑，`dock serve` / TUI 马上就绪。
- 连上的工具随到随登记；没连完时 `/mcps` 显示「连接中…」，`search_tool` 报 `status: "partial"`。
- 目录变化照常在下一轮开头用 `<system-reminder>` 通告。
- 代价：启动后马上发的第一轮可能还看不到慢服务器的工具，下一轮补上（不是 bug）。
- 重连 / 重载期间同样显示「连接中…」、`search_tool` 报 `partial`。
- 握手途中被停用、重连或换了配置：晚到的结果按 `attempt` 认出过期，成败都不写回。

### MCP Apps（界面）

Dock 按 [MCP Apps](https://modelcontextprotocol.io/extensions/apps/overview)（`2026-01-26`）当客户端：

- 握手时声明 `capabilities.extensions["io.modelcontextprotocol/ui"] = { mimeTypes: ["text/html;profile=mcp-app"] }`。
- 工具 `_meta.ui.resourceUri`（旧写法 `_meta["ui/resourceUri"]` 也认）指向 `ui://` 界面资源；不是 `ui://` 的不认。
- `_meta.ui.visibility`：
  - 没写 = 模型和界面都能用；
  - 不含 `model` 的不进工具表、不在 `/mcps` 里，只有同一服务器的界面能调；
    要停掉它们只能停整台服务器。
- 每次 `tools/call` 的 `_meta` 带 `dock/sessionId`（调用方会话）。
- 本机 stdio 服务器另带 `dock/cwd`（调用方的工作目录，按项目存数据的服务用）；
  那是本机绝对路径，远程 HTTP 服务器收不到。
- 模型调挂了界面的工具：文本照常进对话；原始 `CallToolResult`（含 `structuredContent`）另存
  `<会话目录>/mcp-apps/<调用 id>.json`（`{ server, tool, resourceUri, arguments, result }`），给界面原样回放。
  - 单条上限 4 MB，超了只留文本块；不落盘的页不存。
  - 文件名是调用 id 的安全字符，换过字符的再接原 id 的哈希；随会话目录一起删。
- `Mcp` 给客户端用的入口：
  - `app_tools()`：带界面或只给界面用的工具；
  - `read_resource(server, uri)`：`resources/read`；
  - `call_from_app(page, server, tool, args)`：界面发起的调用，只认 `visibility` 含 `app`、没在 `/mcps` 停用、不过权限门的工具，不进对话流。
- 界面自己登记的工具（`appCapabilities.tools`）：桌面端经网关 `mcp/app/view/*` 报上来，spine 插件
  `mcp-app-views` 按 `app_<server>__<name>` 登记成动态工具（经 `use_tool` 调），调用时推回桌面端交给
  那个界面跑；只有打开界面的那个会话调得到，界面关了 / 连接断了就收回。细节见
  `cordis-gateway/README.md`「界面自己的工具」。
- TUI 不渲染界面：同一个工具在终端里只是普通 MCP 工具。
