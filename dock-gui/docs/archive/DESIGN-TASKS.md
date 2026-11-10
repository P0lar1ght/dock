# Dock GUI 设计任务（给 Gemini，在 Wonder 里画）

## 通用要求（每一项都适用）

- 文件：Dock 的 Wonder 页（pageId `01a0cd30-3663-7d81-b559-fd22e37e65a1`）。
- 放置：新画板统一放在 x ≥ 5400 的空白列，从 y = -1000 往下排，间隔 60，不要压到已有画板。
- 每个状态单独一个画板；画板 label 用英文，格式 `<组件> <状态>`，如 `Question Card Multi`。
- 画板宽度按组件本身：会话区里的卡片 868px（会话区 900px，左右各 16 留白）；侧栏 248px；窄栏 52px；弹出菜单 300px。
- 文案用中文，用下面给的真实示例文字，不要 Lorem ipsum。
- 风格沿用现有画板，不另起一套。参照：
  - 权限条 `his.cowy`（Permission Strip）：卡片外观的基准；
  - 通知菜单 `quag`：弹出层的基准；
  - 对话流 `tebu`：对话区的基准；
  - 侧栏会话状态 `poly`：侧栏行的基准。
- 取值（直接用这些，别换色）：
  - 背景：窗口 #f5f5f7、侧栏 #f7f7f8、会话区 #fafafa、填充 #f0f0f2、悬停 #ededf0、选中 #e8e8ec
  - 线：#e8e8ec / #ececf0 / #e5e5ea
  - 文字：#1c1c1e / #3a3a3c / #636366 / #8e8e93 / #c7c7cc
  - 强调色 #0a84ff（浅底 #0a84ff14 / #0a84ff1a）；成功 #34c759；错误 #ff3b30（文字 #c0392b，浅底 #fef2f2，边 #fecaca）
  - 卡片：圆角 10、边 1px #e5e5ea、底 #f5f5f7、内边距 12×10
  - 按钮：高 26、圆角 7、11px 中粗；主按钮黑底白字 #1c1c1e，次按钮白底描边
  - 字号：正文 13、说明 12、小字 11、标题 15
  - 图标：lucide，14px，线宽 2
- 不要画动效；需要动的地方用一行注释写清楚（如「出现时从下方淡入」）。

---

## 第一批：交互卡（最优先）

这三张卡都出现在会话区底部、输入框上方，宽 868。

### 1. 提问卡 Question Card

模型用提问工具问用户。卡头写「Dock 在问你」，配问号气泡图标。

每个状态单独一个画板：
- `Question Card Single`：单选题。
  - 题目：「喜欢哪种颜色？」
  - 选项：红 / 绿 / 蓝。每个选项下面带一行说明（红色 / 绿色 / 蓝色），说明要直接显示出来，不要藏在悬停提示里。
  - 选中「绿」。
- `Question Card Multi`：多选题。
  - 题目：「你会哪些语言？」，旁边标「可多选」。
  - 选项：Rust / TypeScript / Go。选中 Rust 和 Go，选中的带对勾。
  - 下方输入框里写着「Zig」（用户自己补的回答）。
- `Question Card Two Questions`：同一张卡上两道题（上面一道多选、下面一道单选），题与题之间要分得开。
- `Question Card Not Ready`：还有题没答，「提交回答」按钮是禁用样式。
- `Question Card Keyboard Hint`：可选方案。每个选项前面有数字键提示（1 / 2 / 3），右下角有「Enter 提交」提示。

### 2. 计划审批卡 Plan Card

模型写好计划，等用户批准。卡头写「计划待批准」，右侧显示计划文件路径 `.dock/plans/refactor-store.md`。

- `Plan Card Short`：计划正文是 Markdown，约 6 行，包含：
  - 一个标题；
  - 三条列表；
  - 一段行内代码。
  - 底部按钮从左到右：退出计划（次按钮）、要求修改（次按钮）、批准并执行（主按钮）。
- `Plan Card Long`：正文约 40 行时怎么处理：限高滚动，或者折叠后加「展开全部」，由你设计。
- `Plan Card Empty`：计划文件是空的，正文区只显示一行灰字「计划文件是空的。」

### 3. MCP 确认卡 Elicit Card

MCP 服务在对话中途请求用户确认。

- `Elicit Card`：
  - 卡头：插头图标 + 「linear 请求确认」，右侧写服务名 `linear`。
  - 正文：「将在 Linear 创建 issue：修复登录页样式（团队 WEB）」。
  - 按钮：拒绝（次）、接受（主）。

---

## 第二批：状态与反馈

### 4. 侧栏里等你处理的会话 Sidebar Session Waiting

参照 `poly`。侧栏会话行现在有两种状态：
- 选中；
- 正在跑（转圈）。

需要新增「在等你」：会话在等权限、提问或计划审批。

- `Sidebar Sessions Waiting`：宽 228，画 4 行：
  - 选中的普通会话；
  - 正在跑的会话；
  - 等你处理的会话；
  - 在后台跑完、还没看过的会话。

  4 种状态要一眼能分开；「在等你」要比「在跑」更显眼。

### 5. 连接状态 Connection State

Dock 是 GUI 在后台拉起的一个进程。三种状态，每种一个画板，画在侧栏顶部（宽 248）或会话区顶部（宽 900）都可以，由你选：
- `Connection Connecting`：「正在连接 Dock…」
- `Connection Reconnecting`：「Dock 连接已断开，正在重连…」
- `Connection Exited`：「Dock 已退出（exit status: 1）」+「重新启动」按钮。

### 6. 加载历史 Transcript Loading

- `Transcript Loading`：会话区宽 900、高 600。打开一个关着的旧会话、正在读取历史时的占位：骨架屏或居中加载提示。风格要和 `tebu` 一致。

### 7. 窄栏上的铃铛 Rail Notifications

参照 `bar`（收起侧栏的整页）和 `SidebarRail`。

- `Sidebar Rail With Bell`：宽 52，窄栏上加铃铛和搜索两个入口；铃铛带小蓝点。
- `Rail Notification Menu Open`：从窄栏铃铛弹出通知菜单时的位置。菜单内容复用 `quag`。

---

## 第三批：对话流元素

### 8. 图片 Images

- `Composer With Images`：输入框里贴了 2 张图，缩略图 56×56，每张可以删掉；输入框里还有一行字「看看这两张截图哪里不对」。参照 `act`（Composer Controls v2）。
- `User Bubble With Images`：对话里的用户气泡带 2 张图 + 一行字。参照 `tebu` 的用户气泡。
- `Image Preview`：点开大图的样子（遮罩 + 居中大图 + 关闭按钮），可选。

### 9. 工具卡通用状态 Tool Card States

`wonder-ui/tools` 里每种工具都有展开 / 收起两个画板，但运行中、出错、已中断只画了 Bash（`whan`）。请画一套能套给所有工具的通用状态，用 Read 工具举例：
- `Tool Card Running`：运行中（右侧用时在跳，图标在转）。
- `Tool Card Failed`：失败，输出是「Error: 文件不存在：src/app.ts」。
- `Tool Card Interrupted`：被停止打断，带「已中断」角标，输出是「已中断。」
- `Tool Card Denied`：用户拒绝了权限，输出是「用户拒绝了这次调用」。

### 10. 排队中的消息 Queued Message

参照 `tebu` 的 `User Turn Queued`。

- `User Turn Queued`：气泡旁边标「排队中 · 本轮结束后发送」，带一个取消按钮（×），悬停时才显示也可以。

---

## 第四批：还只有外观的入口

### 11. 会话头「更多」菜单 Session More Menu

参照 `yerk`（Session Header Running）和 `quag` 的弹出层。

- `Session More Menu`：宽 220，从会话头右上角的「…」弹出。菜单项：
  - 改名；
  - 复制会话 ID；
  - 在访达中打开项目目录；
  - 分隔线；
  - 归档；
  - 删除（红字）。
- `Session Rename Inline`：改名时，会话头标题变成输入框的样子。
- `Session Delete Confirm`：删除前的确认框（「删除这个会话？记录会从磁盘移除。」，按钮：取消 / 删除）。

### 12. 搜索会话 Session Search

- `Session Search Open`：侧栏顶部点搜索后，搜索框出现在品牌行下方；输入「store」，下面是结果列表。
  - 每条显示会话标题 + 一行命中的片段，关键词高亮。
  - 结果按预设分组。
- `Session Search Empty`：没有结果时的样子。

---

## 交付

- 每个画板画完截一张图确认，然后结束这个画板。
- 全部画完后列一张清单：画板 label → data-node-id。我按清单导出到 `wonder-ui/` 并实现。
