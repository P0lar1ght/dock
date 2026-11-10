# Dock GUI 设计任务 · 第三轮：返工（给 Gemini）

通用要求同 `DESIGN-TASKS.md` / `DESIGN-TASKS-2.md`。本轮只改已有画板，不新建（第 4 项例外）。

## 0. 本轮最重要的一条：整窗不要重画外壳

第二轮的 6 张整窗（snot、cram、leto、hati、doxa、reis）都重新画了一套外壳，和真实界面对不上。整窗必须**复制 `his`（Page - Dock Shell Light Glass）再往里填内容**，外壳原样保留：

- **侧栏**：
  - 顶行：红绿灯 + 切换侧栏 / 后退 / 前进；
  - 品牌行：「Dock · 编码 ⌄」+ 搜索 + 铃铛；
  - 导航：新对话 / Pull Request / 定时任务 / 插件 / Agent 预设；
  - 「预设 · 项目」树：预设组 → 项目 → 会话；
  - 底部：「me」+ 帮助。
  - 不要画成「Dock」+ 面包屑「编码 › dock」。
- **会话头**：标题 + 预设胶囊，右侧「更多」「单独面板」「收起工具面板」。不要写「1.8k tokens」这类 Dock 没给的数。
- **输入框**：用 `act`（Composer Controls v2）：「+」、审批胶囊、模型胶囊、推理胶囊、发送键。不要只剩一个输入框。
- **右侧工具面板**：300 宽，照 `his` / `ShortcutsPane`，除非任务写了收起。
- **对话流**：照 `tebu`：不标「用户 / 助手 / Dock」角色名，用户消息是右侧灰气泡。

## 1. 整窗逐张

- `snot` Waiting For You：
  - 按第 0 条换外壳。
  - 侧栏里的「在等你」标记用 `bolo` 的样式。
  - Edit 工具卡用已有的 `cafe`（Tool Edit Collapsed）。
  - 权限条保持现在的样子（这版很好）。
- `cram` Question Pending：
  - 按第 0 条换外壳。
  - 提问卡**直接用 `coft`**（两道题那张）。现在这张是黑底胶囊按钮的另一套样式，删掉。
  - 删掉回复上方的「Dock」角色名。
  - 输入框占位改为「回答上面的问题后继续…」（保留），但输入框本体换成 `act`。
- `leto` / `hati` First Run：
  - 新对话页照已有的 `nor`（Page - Dock New Chat Select Preset）画，不要另做一套四宫格卡片。
  - 侧栏空状态保留（这版很好）。
  - `hati` 顶部的黄色提示条保留，但开头的图标要画出来。
- `doxa` Turn Failed：
  - 按第 0 条换外壳。
  - 模型胶囊放进 `act` 的底栏，红色细边标出问题。
  - 错误框、重连横条保留（这版很好）。
- `reis` Settings：
  - 侧栏按第 0 条换。
  - 左侧分组「预设」的图标要画出来。
  - 其余保留。

## 2. 组件逐个

- **图标缺失**：以下画板里有图标只剩虚线方框，换成对应的 lucide 图标（14px）：
  - `khet` Dock 已退出：circle-alert；
  - `bilo` 载入提示：loader；
  - `byth` 各行：同 `quag`；
  - `dalk` 右端：删掉这个图标；
  - `gelt` 删除确认：trash-2；
  - `snot` Edit 卡：用 `cafe` 的图标；
  - `cram` 卡头：message-circle-question；
  - `hati` 提示条：triangle-alert；
  - `reis` 预设：layers。
- `golo`：输入框里已经有「Zig」，就不再显示占位文字。
- `berg` 窄栏：
  - 照已有的 `SidebarRail` / `bar` 画：保留原来的导航图标和预设图标组，只在里面加上搜索和铃铛（铃铛带小蓝点）。
  - 不要删掉原有内容。
  - 设置齿轮去掉（设置页入口还没定）。
- `byth` 窄栏通知菜单：菜单本体**原样复用 `quag`**（分组名「在等你 · N」「刚完成」，每行：会话标题 + 右侧时间 + 下方说明），只改位置：从窄栏铃铛右侧弹出。
- `cora` 输入框带图：
  - 输入框本体用 `act`；
  - 删掉「claude-3-5-sonnet」和「Enter 发送」。
  - 缩略图保留。
- `boor` / `iten`：删掉文件名（login-page.png 等），Dock 的图片只带尺寸和大小。
  - `iten` 顶部写「1920 × 1080 · 240 KB」即可。
  - `boor` 的缩略图上不放文字。
- `bilo` 载入骨架：删掉左侧那个小方块头像（对话流没有头像）。
- `dalk` 改名：
  - 预设胶囊放回标题右边，和 `yerk` 一致；
  - 删掉「1.2k tokens」。

## 3. 保留不动

`gena` `wail` `chih` `coft` `jeel` `khat` `rimy` `bere` `bolo` `bink` `izzy` `moop` `kiln` `dray` `muid` `fuci` `dubs` `cipo` `gein` 都很好，不要改。

## 4. 交付

- 每块改完截图自查，再结束这个画板。
- 清单：画板 label → data-node-id → 「已改」/「待确认 + 原因」。
