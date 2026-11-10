# 工具卡美化清单

对照 `TOOLS.md`（工具索引）和内置 MCP（`browser`、`cua-driver`）。
一行一颗（或一组同形的）工具。
- 美化：`components/tools/specs.tsx` 里有专用画法。
- 实测：真模型（Atria-Dawn-Preview）、隔离的真 Dock 调过，并截图看过。
  - 「真机」= 在完整 GUI（index.html）里实时跑。
  - 「回放」= 录下的真实事件在 `preview.html#tool-fixtures-<样本>` 回放。
  - 「旧会话」= 用户以前的真实会话：拷进隔离 `DOCK_HOME` 用无头 GUI 逐屏看，
    图片（隔离环境取不到）在用户自己的 GUI 里看。

画法代码：
- `components/tools/parts.tsx`：输出面板、高亮代码、文件行、徽标、键值表、截图缩略图。
- `components/tools/specs.tsx`：每颗工具的行头 + 正文。
- `components/ToolCard.tsx`：外壳（四态：运行中 / 失败 / 中断 / 拒绝）。

## 通用外壳

- [x] 行头：图标（文件类按类型着色）+ 动词 + 主参数 + 补充（文字或徽标）
- [x] 正文：圆角浅框，等宽字；头 12 行 + 「展开全部（还有 N 行）」；悬停复制
- [x] 超长单行（整段 JSON）先截 600 字，「展开全部（还有 N 字）」
- [x] 文件路径可点（复用对话流的 PathLinkContext，开文件面板）
- [x] 工具结果里的图：展开时按 `item/image` 取，缩略图点开看大图
- [x] 行头已说完的（已停止、已关闭、本来就空闲、打开 / 点击）不给展开
- [x] use_tool 调的按里面那颗工具画，正文拿里面的参数
- [x] 参数表里整段代码 / 长文先露 6 行，「展开（共 N 行）」；行头挑短的单行参数
- [x] 输出像 Markdown 文档（标题行 / 代码围栏）就排版，固定高度 + 渐隐 +「展开全文」
- [x] 读图片的 `read_file` 只出缩略图，不列「Image content included inline」
- [x] 大图挂到 body（以前在对话栏里渲染，被渐隐遮罩裁成一栏宽）

## 工作区

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `read_file` | 按扩展名 Shiki 高亮 + 真实行号 | [x] | [x] 真机 |
| `list_dir` | 文件类型图标网格，目录蓝色 | [x] | [x] 真机 |
| `glob` | 类型图标 + 路径，可点 | [x] | [x] 真机 |
| `grep` | 按文件分组，命中词黄底，行号 | [x] | [x] 真机 |
| `search_replace` | 行级 diff + 行内改动高亮；+/− 计数 | [x] | [x] 真机（含失败） |
| `write_file` | 按扩展名高亮 + 行数 | [x] | [x] 真机 |
| `bash` | 终端样式：`$ 命令` + 输出；退出码徽标；后台徽标；`0 failed` 不标红；有描述见下 | [x] | [x] 真机（含失败、后台） |

## 后台任务

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `job` | 状态徽标 + 命令 + 输出 | [x] | [x] 回放 |
| `kill_task` | 一行「已停止」 | [x] | [x] 回放 |
| `monitor` | 盯的命令 + 后台任务号；有描述见下 | [x] | [x] 回放 |

**有描述（`description`）的 bash / monitor：**
- 行头：描述（正文字，不写 Bash）+ 右侧灰色程序徽标（`find`、`cargo test`），悬停描述看命令原文。
- 程序名规则在 `dock/programLabel.ts`，同 TUI 的 `program_label`。
- 展开的终端里照旧是完整命令 + 输出。
- 子代理卡的「正在 …」和迷你步骤也说描述。
- 权限条：首行 `$ 完整命令`（最多三行），描述另起一行；拆分在 `dock/permission.ts`。
- 预览：`preview.html#bash-description`、`#permission`。

## 网页

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `web_search` | 结果列表：域名 + 路径（解码跳转、去跟踪参数） | [x] | [x] 回放 |
| `web_fetch` | HTTP 状态徽标 + 正文 Markdown 预览 | [x] | [x] 回放 |

## 计划与交互

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `todo_write` | 进度条 + 列表 | [x] | [x] 回放 |
| `ask_user_question` | 问题 / 回答卡 | [x] | [x] 回放 |
| `enter_plan_mode` / `exit_plan_mode` | 计划模式徽标；批准后计划正文排版 | [x] | [x] 回放 |
| `update_goal` | 新目标 / 进展 / 已完成 / 卡住了徽标 | [x] | [x] 回放 |

## 子代理

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `task` | SubagentCard（原有，没动） | — | [x] 回放 |
| `send_message` | 收件人 + 消息气泡 + 已送达 | [x] | [x] 回放 |
| `list_agents` | 状态 + 角色 + 说明 | [x] | [x] 回放 |
| `interrupt_agent` | 已打断 / 本来就空闲 | [x] | [x] 回放 |

## 知识与代码智能

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `skill` | 技能名 + 路径，正文 Markdown | [x] | [x] 回放 |
| `memory_search` | 命中卡：分数 + 文件 + 片段 | [x] | [~] 只测到「无结果」，命中格式按源码 |
| `memory_get` | Markdown 高亮 + 行号 | [x] | [x] 回放 |
| `lsp` | 符号列表：名字 + 种类 + 行，可点 | [x] | [x] 回放 |

## 画布

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `canvas_create` / `edit` / `data` | CanvasChip（原有，没动） | — | [x] 旧会话 |
| `canvas_read` | 标题 + 版本；版本说明（新的在上）+ HTML 高亮 + data | [x] | [x] 旧会话 |

## 定时任务

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `scheduler_create` | 周期徽标（每 10 分钟）+ 属性表 | [x] | [x] 回放 |
| `scheduler_list` | 任务行：内容 + 周期 + 下次 | [x] | [x] 回放 |
| `scheduler_delete` | 一行「已关闭」 | [x] | [x] 回放 |

## 扩展

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `workflow` | 参数表 + 结果 | [~] 通用 | [x] 只测到失败调用 |
| `cordis_inspect` | 说明文字（可展开） | [x] | [x] 回放 |
| `cordis_define` | 用途 + Rhai 源码高亮 | [x] | [x] 回放 |
| `cordis_run` | 状态徽标 + 运行属性 | [x] | [x] 回放 |
| `cordis_call` / `promote` / `stop` / `undefine` | 参数表 + 结果（通用） | [~] | [x] 旧会话 |

## MCP

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `search_tool` | 服务器徽标 + 名字（`tool_name`）+ 一句说明；前 8 条 | [x] | [x] 真机 |
| `use_tool` | 按里面那颗工具画 | [x] | [x] 真机 |
| 其它 `mcp_<server>__<tool>` | 服务器徽标 + 参数表 + JSON 高亮 / Markdown | [x] | [x] 旧会话（wonder、sungods） |

## 浏览器（`mcp_browser__browser_*`）

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `open` / `navigate` / `navigate_back` | 地址进行头，不展开 | [x] | [x] 真机 |
| `snapshot` | 元素行：角色 + 名字 + ref 徽标（ref 后面还跟 `value=` 也认） | [x] | [x] 真机 |
| `screenshot` | 截图缩略图（`item/image`） | [x] | [x] 真机（取到像素） |
| `evaluate` | 表达式 + 结果（JSON 高亮） | [x] | [x] 回放 |
| `js`（Playwright 脚本） | 行头写 title；正文脚本高亮 + log + 返回值（JSON 高亮），截图走缩略图；去终端颜色码 | [x] | [x] 真机（香港 iPhone 库存任务，41 次） |
| `click` / `hover` | ref 进行头，不展开 | [x] | [x] 回放（click） |
| `tabs` | 标签列表，当前页标出 | [x] | [x] 回放 |
| `console_messages` / `network_requests` | 列表；空的写「还没有记录」 | [x] | [x] 回放（console） |
| `type` / `press_key` / `fill_form` / `select_option` 等 | 输入内容 + 参数表 | [~] | [x] 旧会话（type、select、drag） |
| 旧会话里的 `browser_*`（浏览器 MCP 化之前的进程内工具） | 同上，按动作画 | [x] | [x] 旧会话 |

## 桌面（`mcp_cua-driver__*`）

新调用只做只读的：用户在用这台机器，点击 / 输入 / 截图会打扰，也会把桌面内容发给模型。
点击 / 输入 / 截图类用旧会话里的真实调用看。

| 工具 | 画法 | 美化 | 实测 |
|---|---|---|---|
| `list_apps` | 应用列表：名字 + bundle + pid | [x] | [x] 真机 |
| `list_windows` | 窗口列表（屏上的在前）；截断的 JSON 逐个抠 | [x] | [x] 真机 |
| `get_screen_size` / `get_cursor_position` | 一句话 + 键值表 | [x] | [x] 真机 |
| `click` / `type_text` / `press_key` / `hotkey` 等 | 动作 + 元素（`元素 #13 AXButton`）/ 坐标 / 文字 / 键；`effect` 徽标（已确认 / 未验证）；结果压平成键值 | [x] | [x] 旧会话 |
| `launch_app` / `bring_to_front` | 应用 / 窗口；窗口列表 | [x] | [x] 旧会话 |
| `get_window_state` / `zoom` 一类截图 | 行头写窗口标题 + 元素数；正文 AX 树；截图缩略图 | [x] | [x] 旧会话（缩略图在用户 GUI 里看的是 read_file 读的图） |

## 重跑实测

脚本在 `scripts/tool-cards-e2e/`：
- 隔离 Dock：临时 `DOCK_HOME`（只拷 `config.toml`、`secrets.json`）+ `dock serve --remote` + 设备令牌。
- `harness.mjs`：无头 Chrome 跑完整 GUI，Tauri 换成假桥（`dock_connection` 指向隔离 Dock）。
- `run.mjs <名字> <提示词> [预设]`：开新对话、发提示、自动回答提问 / 批准计划、截图。
- `history.mjs`：拉 `thread/history` 存成样本；样本放 `src/preview/fixtures/`。
- 桌面组的样本含本机应用 / 窗口名：不存样本。
