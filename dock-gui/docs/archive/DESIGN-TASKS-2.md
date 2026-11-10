# Dock GUI 设计任务 · 第二轮（给 Gemini）

通用要求同 `DESIGN-TASKS.md`（页面、放置列 x ≥ 5400、取值、字号、按钮）。本轮追加四条硬规矩：

- **卡片里不放作者标签**。画板 label 可以带 `Gemini-` 前缀，但卡片内部不要出现写着画板名的小标签（上一轮每张卡右上角都有一个 `Author Tag`）。
- **Markdown 按渲染后的样子画**：标题画成加粗的标题，不要把 `###` 原样写出来；列表、行内代码同理。
- **不编 Dock 给不了的数据**。每张卡能显示什么字段，下面都写明了；没写的（如「优先级」「目标团队」）不要加。
- **新画板放在上一轮画板的下方**：从 y = 1600 起往下排，间隔 60。

---

## A. 返工：第一轮的 9 块

整体很好，风格和取值都对。只改下面几处，直接在原画板上改，不要新建：

1. **全部 9 块**：删掉卡头右侧的 `Author Tag`：
   `gena.amla`、`golo.ahet`、`coft.birk`、`wail.asci`、`chih.avow`、`jeel.argo`、`khat.brit`、`rimy.byon`、`bere.faba`。
2. **`gena` 单选**：选项下方补一个输入框，占位文字「或者写你自己的回答」。单选题也能不选选项、自己写一句，两者二选一；写了字之后选项的选中态取消。
3. **`golo` 多选**：输入框不要画成「+ 标签」的样子。Dock 每题只收一句自己写的话，画成普通单行输入框，框里的文字是「Zig」，占位文字「还可以补一句你自己的回答」。`coft` 里的多选题同样改。
4. **`jeel` / `khat` 计划**：去掉 `###`，改成渲染后的标题样式（15px 加粗）；列表和行内代码保持。
5. **`bere` MCP 确认**：删掉第二行「MCP Server: Linear · 目标团队 · 优先级」。Dock 只给三样：
   - 服务名 server；
   - 标题 heading；
   - 正文 message。
6. **`rimy` 空计划**：「批准并执行」不要禁用。空计划也可以批准，由模型接着处理。

---

## B. 第一轮没画的（照 `DESIGN-TASKS.md` 第 4–12 项）

请把 `DESIGN-TASKS.md` 里第二、三、四批全部画完：

- 4 侧栏会话四态 `Sidebar Sessions Waiting`
- 5 连接状态三张 `Connection Connecting / Reconnecting / Exited`
- 6 加载历史 `Transcript Loading`
- 7 窄栏铃铛 `Sidebar Rail With Bell` / `Rail Notification Menu Open`
- 8 图片 `Composer With Images` / `User Bubble With Images` / `Image Preview`
- 9 工具卡通用状态 `Tool Card Running / Failed / Interrupted / Denied`
- 10 排队中 `User Turn Queued`
- 11 更多菜单 `Session More Menu` / `Session Rename Inline` / `Session Delete Confirm`
- 12 搜索 `Session Search Open` / `Session Search Empty`

第 5 项定一下位置：**画在会话区顶部**，宽 900、高 36 的横条，连接恢复后自动消失。

---

## C. 新任务：整窗组合（1440 × 900）

目的：检验单个组件放在一起是否协调。每张都以 `his`（Page - Dock Shell Light Glass）为底：
- 左侧栏 248；
- 会话区；
- 右侧工具面板 300；
- 底部终端（可收起）。

复用已有组件，不要重画一套。

### 13. `Page - Waiting For You`（有会话在等你）

- 侧栏：「编码 › dock」下有 3 个会话。
  - 「重构 store」在跑（转圈）；
  - 「修复登录页样式」在等你（第 4 项的标记）；
  - 第三个普通。
  - 选中的是「修复登录页样式」。
- 铃铛带蓝点。
- 会话区：
  - 标题「修复登录页样式」，预设胶囊「编码」；
  - 对话流：一条用户消息「把登录页按钮改成圆角并跑一遍构建」，然后是折叠的「用时 18s ›」和一张 Edit 工具卡（src/pages/Login.tsx，+3 −1）；
  - 底部输入框上方是权限条：允许 bash？ `npm run build`，按钮「允许本次」「拒绝」；
  - 输入框占位「追加指令，本轮结束后发送」。
- 右侧工具面板收起，终端收起。

### 14. `Page - Question Pending`（模型在提问）

同 13 的外壳：
- 侧栏选中「重构 store」（在等你）。
- 对话流：用户消息「帮我把 store 拆成几个模块」，助手回复一段话「可以拆成三块，先确认两个问题：」。
- 输入框上方放 `coft`（两道题）的返工版。
- 右侧工具面板打开，显示快捷面板（参照 `ShortcutsPane`）。

### 15. `Page - First Run`（第一次打开）

- 侧栏：没有任何会话。树的位置写一段空状态：「还没有会话」+「在右边选一个预设开始」。铃铛无点。
- 会话区：新对话页（参照 `nor`，Page - Dock New Chat Select Preset）：
  - 预设四张卡「编码 / 极简 / 创造 / 守望」；
  - 项目目录输入框为空；
  - 「继续」按钮禁用。
- 另画一个变体 `Page - First Run No Model`：
  - 预设卡全部置灰，上方一条黄色提示「还没有可用的模型：在 ~/.dock/config.toml 里配置 [model.*]」，附链接「打开配置说明」。
  - 黄色：底 #fff8e6、边 #f5d48a、文字 #8a5a00。

### 16. `Page - Turn Failed`（一轮出错）

同 13 的外壳：
- 对话流最后一轮是用户消息「只回复 hi」，下方是「用时 1s · 出错」，再下方是红色错误框「模型「atria-dawn」不在模型目录里：检查模型名，或在 config.toml 里加上 [model."atria-dawn"] 并配好 api_base_url。」。参照 `tebu` 的 `Turn Error Box`。
- 输入框正常可用，模型胶囊显示「atria-dawn」，并用红色细边提示有问题。
- 会话区顶部叠一条第 5 项的 `Connection Reconnecting` 横条，看两种错误同时出现时是否还清楚。

### 17. `Page - Settings`（设置页，探索稿，功能还没定）

从侧栏底部「me」旁的齿轮进入，占满会话区。左侧一列分组（模型 / 预设 / MCP 服务 / 外观 / 关于），右侧内容。只画「MCP 服务」这一组：
- 列表每行：服务名、状态点（已连接 绿 / 连接失败 红 / 已停用 灰）、工具数、一个开关。
- 示例：
  - `cua-driver` 已连接 · 56 个工具；
  - `linear` 已连接 · 23 个工具；
  - `figma` 连接失败 · 「401 未授权」；
  - `sqlite` 已停用。
- 连接失败那行展开，显示错误原文和「重试」按钮。

---

## 交付

- 每块画完截一张图确认，再结束这个画板。
- 最后给一张清单：画板 label → data-node-id，A 部分注明「已改」。
