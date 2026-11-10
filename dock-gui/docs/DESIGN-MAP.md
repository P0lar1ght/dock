# 设计稿 → 源码对照

左：`wonder-ui/` 目录。右：`src/` 文件。
「—」= 还没实现。对齐一个就改一行。

## 侧栏

- shell/SidebarHeader → components/Sidebar.tsx
- shell/SidebarNav → components/Sidebar.tsx
- shell/SidebarTree → components/Sidebar.tsx
  - 偏离（用户定）：只列当前预设的项目，没有预设那一层；标题「项目」。
- shell/SidebarFooter → components/Sidebar.tsx ✓（整列撑满窗口高，会话树自己滚，底部个人信息块贴底）
- shell/SidebarRail → 收起态整列收起（宽 248 → 0），不用窄图标栏
- shell/SidebarBrandDropdown → Sidebar.tsx BrandMenu
- shell/BrandDefaultPresetMenu → Sidebar.tsx BrandMenu ✓
- shell/SidebarSessionsState → Sidebar.tsx SessionRow ✓
- shell/SidebarSessionsWaiting → Sidebar.tsx SessionRow ✓（等你用橙色，取 snot）
- shell/UserTurnQueued → components/Queued.tsx ✓（带图时图和文字同一个气泡）
- shell/SidebarRailBell → Sidebar.tsx BrandMenu 右侧铃铛（NotificationMenu）✓
- shell/SessionSearch → SessionSearch.tsx ✓（标题 + 用户消息片段）

## 会话区

- shell/SessionHeader → SessionPane.tsx ✓
- chat/Transcript → Transcript.tsx ✓
  - 对话形态改照用户截图（Codex 式），不再按 ToolCardsStream：
  - 无角色标签；用户消息右侧灰气泡，悬停出时间 + 复制。
  - 隔 30 分钟插日期行；一轮结束过程折进「用时 X ›」。
  - 正文 15px；离开底部出现「回到底部」圆钮。
  - 已回写画板 `tebu`：wonder-ui/chat/TranscriptCodexStyle。
  - 偏离（用户定，没出设计稿）：居中 780px 阅读列、轮次间距、底边渐隐、长消息折叠。
  - 偏离（同上）：代码块头栏 + 高亮、Mermaid、公式、表格 / 引用 / 任务列表样式。
- chat/Feed* → 已被 TranscriptCodexStyle 取代
- chat/PermissionStrip → Pending.tsx ✓
- shell/NotificationMenu → NotificationMenu.tsx ✓（quag / wiss）
- chat/QuestionCard → Pending.tsx QuestionCard ✓
- chat/PlanCard → Pending.tsx PlanCard ✓
- chat/ElicitCard → Pending.tsx ✓
- shell/ConnectionBanner → ConnectionBanner.tsx ✓
- （无设计稿）LaunchSplash.tsx：仿 Codex 启动画面，按 MOTION.md「启动画面」自拟。
- chat/TranscriptLoading → Transcript.tsx LoadingSkeleton ✓
- chat/ToolCardStates → ToolCard.tsx ✓（行式外形）
  - 偏离（用户定，没出设计稿）：正文改圆角浅框 + 展开全部；各工具专用画法见 docs/TOOL-CARDS.md。
- chat/UserTurnQueued → Queued.tsx ✓
  - 「立即插话」按钮与插话等待态是在原稿上加的，没有设计稿（按取消胶囊的样式）。
- shell/SessionMoreMenu → SessionMenu.tsx ✓（无归档）
- shell/SidebarSessionMenu → SessionMenu.tsx SessionContextMenu ✓（单独面板图标沿用会话头）
- shell/CreateProject → CreateProject.tsx ✓
- shell/SidebarProjectMenu → SessionMenu.tsx ProjectContextMenu + Sidebar.tsx ProjectRow ✓
- chat/Images → Composer.tsx 缩略图、Transcript.tsx UserBubble、ImageViewer.tsx ✓
- chat/ComposerV3（gis：斜杠下拉 / 计划开关 / 打断并发送）→ Composer.tsx ✓
  - 「打断并发送」按钮已去掉：插话改在排队条上点；⌘Enter 仍是停止并发送。
- chat/ComposerGoalContext（had：目标胶囊 / 目标条 / 上下文圆环）→ GoalBar.tsx、ContextRing.tsx、Composer.tsx ✓
- chat/Images → —（协议待定）
- chat/Composer → 已被 ComposerV2 取代
- chat/ComposerV2 → Composer.tsx ✓
- chat/ComposerRunning → Composer.tsx ✓（停止键）
- chat/ComposerFloating → Transcript.tsx ✓（内容短时输入框跟着内容浮起）
- shell/SessionHeaderRunning → SessionPane.tsx Running ✓
- chat/SubagentCard（she–pet 八态）→ SubagentCard.tsx ✓（需求见 archive/DESIGN-TASKS-4）
- chat/SubagentGroup（pah / lof）→ SubagentCard.tsx SubagentGroup ✓
- chat/SubagentEntrances ram → ToolPanel.tsx「子代理」+ 徽标 ✓
- chat/SubagentEntrances may → SessionPane.tsx SubagentsRunning ✓
- chat/SubagentEntrances are（权限条「来自子代理」）→ Pending.tsx ✓
- pages/Subagents（zat–gpl 面板）→ SubagentPanel.tsx ✓
- 差距：权限条多了「始终允许」（设计稿只有允许本次 / 拒绝；样式同次按钮，语义同 TUI）。
- 差距：子代理卡展开的迷你步骤只在运行中给（设计 ona 也是运行中）。

## 侧栏整页（第六轮）

- pages/PagePRList → components/PullRequestsPage.tsx ✓
- pages/PagePRDetail → PullRequestsPage.tsx Drawer ✓（列表加了变暗遮罩）
- pages/PagePRLoading → PullRequestsPage.tsx Skeleton ✓
- pages/PagePRNoGh / Unauthenticated / NotGitHub / Empty → PullRequestsPage.tsx Unavailable ✓
- 出错条（稿里没画）→ PullRequestsPage.tsx ErrorBar（照文档 1.3）
- pages/PageSchedulesList → components/SchedulesPage.tsx ✓
- pages/PageSchedulesNew / Edit → SchedulesPage.tsx TaskDialog ✓（盖在真列表上）
- pages/PageSchedulesEmpty / DeleteConfirm → SchedulesPage.tsx ✓
- 页头 / 空状态卡 / 相对时间 → components/navPage.tsx
- 预览：preview.html#page-<状态>（src/preview/navPages.tsx）

## 其它

- shell/ShortcutsPane → components/ToolPanel.tsx
- shell/TerminalDock → TerminalDock.tsx ✓（真 PTY + xterm.js）
- preset/NewChatPresetPicker → components/NewChat.tsx
- preset/CustomPresetEditor → PresetEditor.tsx ✓（新建，建好进编辑器）
- preset/PresetEditor* → preset/PresetStudio.tsx ✓（10 张画板）
- PresetEditorBasic / Builtin / Validation → preset/sections.tsx BasicSection
- PresetEditorPrompt → sections.tsx PromptSection + PromptEditor.tsx
- PresetEditorTools / ToolSearchEmpty → ToolPicker.tsx
- PresetEditorSubagents / SubagentEdit → sections.tsx AgentsSection / AgentEditor
- PresetEditorUnsaved / Broken → PresetStudio.tsx
- PresetAiPrompt / Generating / Failed → PresetEditor.tsx（新建框三态）
- PresetAiDraft → PresetStudio.tsx 草稿模式 + preset/ai.tsx DraftBanner
- PresetAiPromptDiff → preset/ai.tsx PromptRewrite
- PresetAiToolsRecommend → preset/ai.tsx SuggestPanel
- preset/PresetAskPanel → preset/AskPanel.tsx ✓（`pus` 对话 / `hit` 空状态）
- preset/PresetCreateManual → PresetEditor.tsx ✓（`can` 收起 / `was` 展开）
- preset/PresetDraftCard → features/preset-draft ✓（`los` 待审 / `jet` 已保存）
- 差距：生成进度只显示用时，不画百分比和分步耗时（一次调用）
- 差距：不画匹配度、提示词局限分析、推荐依据分析（后端没有）
- 差距：子代理「模型档位」「工作区模式」「独立沙箱」不做（Dock 不支持）
- 差距：保存位置只有全局；「当前项目」灰掉（即将推出）
- 差距：新建时不能自定 id（Dock 自动起 custom-N）
- shell/SplitPanels → App.tsx 分屏 + SplitDivider.tsx ✓（⌘ 点开）
- chat/WardenSessionPane → —
- tools/* → ToolCard.tsx ✓（按工具分卡，可展开）
- tools/Bash/failed.jsx → ToolCard.tsx ✓（红图标 + exit 码）
- pages/Workbenches → Workbench.tsx ✓（「单独面板」拆出来的会话网格）
- pages/Workbenches 的拖出 → ChatWindow.tsx + Rust `open_chat_window` ✓
- pages/BrowserPanel → BrowserPanel.tsx ✓（turm 主稿 + alo-ali 四态）
- pages/BrowserPanel（das + mba）→ BrowserTabs.tsx ✓（标签栏，和 agent 共用当前页）
  - 2026-11-10 加地址栏右边的「更多」菜单（强制重新加载 / 缩放 / 窗口尺寸）：设计稿没有，按用户给的
    Chrome 风格菜单截图做 → components/BrowserMenu.tsx；缩放与固定尺寸按标签页各记各的。
- pages/FilesPanel → FilesPanel.tsx ✓（bam-alk 主稿 + wut-mes 四态）
- pages/DesktopPanel → DesktopPanel.tsx ✓（lake 主稿 + ian-bbc 四态）
- pages/DesktopPanel（web + rfc）→ AgentCursor.tsx + LiveView ✓（实时画面上的 agent 光标）
- pages/Canvas → CanvasPanel.tsx ✓（sert 主稿 + orby 扩展 + khan 出错 + mira 交回 agent）
  - 2026-11-10 加「打开历史画布」下拉（顶栏图标 + 空状态按钮）：设计稿没有，按 orby 的下拉样式做。
- pages/SidePanelsSplit → App.tsx panes + PanelChrome.tsx + PanelWindow.tsx ✓（sow-gos 主稿 + blo-ama 四态）
  - 2026-10-28 多栏并排改成叠放：Wonder「Panel Glass Stack」see / aaa / war → components/PanelStack.tsx
- chat/FeedCanvasChip → CanvasChip.tsx ✓（coin 四态，34px 基底）
- chat/FeedCompressedNote → Transcript.tsx Item 的 compaction ✓（完成态）
- chat/FeedCompaction（tarr）→ CompactionCard.tsx ✓（进行中 / 重试 / 失败 / 已停止）
- pages/Report → CanvasPanel.tsx（报告就是一块画布，没有单独页面）
- pages/ToolCardsStream → ToolCard.tsx ✓（整页示例，逐张核对过）
- 画板 reis（Page - Settings）→ 已被第五轮 settings/ 取代。
- settings/（第五轮 29 张）→ components/settings/ ✓，分组见下。
- SettingsNav（ser）→ settings/index.tsx：分组、页内项搜索、MCP 数、CUA 圆点。
- Models List/Empty/Delete/Parse Error/Edit/Validation → settings/Models.tsx。
- General（goy）/ Write Error（rrp）→ General.tsx + ui.tsx 的写入失败红条。
- Appearance（zad）→ Appearance.tsx；深色 / 跟随系统标「即将推出」。
- Appearance 差距：Mermaid 渲染一行没做（只影响 TUI，GUI 不画 mermaid）。
- Appearance 多出「材质：平面 / 玻璃」一行：设计稿没有，平面 = 设计稿原样（默认）。
- 材质 = 玻璃时再多「玻璃样式」「透明度」两行：设计稿也没有。
- 玻璃模式下侧栏 / 工具面板 / 终端栏 / 输入框 / 对话框的玻璃外观设计稿都没有；平面模式不受影响。
- Shortcuts / Recording（lab / was）→ Shortcuts.tsx；输入框里的键只展示。
- Memory（eft）→ Memory.tsx。
- MCP List/Add/Add HTTP/OAuth（gel / bud / aud / has）→ Mcp.tsx。
- MCP 差距：「测试连接」并进「保存并连接」，没连上就留在表单里说原因。
- MCP 差距：OAuth 登录中没有「取消」（Dock 侧等授权回来，取消不了）。
- Plugins / Skills / Secrets（ose / bis / yox）→ Plugins.tsx 三个页签。
- CUA 五态（sms / ton / guy / jog / cos）→ Cua.tsx。
- CUA 差距：安装步骤不逐步打勾（Dock 只给脚本输出行），跑完才全勾。
- CUA 差距：driver 路径只显示，改要设 DOCK_CUA_DRIVER。
- Browser（cag）→ Browser.tsx；Gateway / Device Name / Token（hah / joy / its）→ Gateway.tsx。

## 插件视图（第七轮）

- plugins/PluginViewGallery（hum）→ features/plugins/ViewTree.tsx ✓
- plugins/PanelPlugin{Deploy,Busy,Closed,Error,Picker}（dvd / gul / chi / now / acm）
  → features/plugins/PluginPanel.tsx ✓（Closed 的 toast 在 App）
- plugins/ToolPlugin{Collapsed,Expanded}（oxy / gan）→ features/plugins/ToolViews.tsx ✓
- plugins/ToolPluginFallback（pci）：只画原始输出，没有「未加载」提示（契约里没有归属）。
- plugins/SettingsPluginsConfig{,Invalid,Saved}（sir / but / say）→ features/plugins/SettingsCard.tsx ✓
- plugins/StatusItems{Composer,Overflow}（uni / tum）→ features/plugins/StatusItems.tsx ✓
- 右侧工具面板的插件面板 / MCP 应用入口 → features/plugins/PluginPanel.tsx PluginEntryGroup + features/mcp-apps/AppPanel.tsx AppEntryGroup（设计稿 ShortcutsPane 没有这两组）。
- 差距：表格「状态」列的色调字（契约里格子是纯文字）。

## 已知差距（待对齐）

- 右侧快捷面板：已对齐 ShortcutsPane（终端、浏览器、文件能用）。
- 快捷面板头部的折叠 / 关闭按钮去掉，改由窗口右上角的「侧边面板」按钮统一折叠那一栏（minimax code 的做法）。
- 右侧「再叠一页」菜单：功能面板（侧边聊天、审查）在 features/registry.ts 用 registerPanelMenuItem 登记，宿主只调登记表。
- 右侧那一列的顶部标签行（components/SideTabs.tsx）：形态照 minimax code 的浏览器标签行，当前打开的面板各一个页签，点一下翻过去；折叠按钮在这一行最右。下面的面板堆叠（页边 / 悬停探出）保持原样。
- 快捷面板多了「桌面」「画布」两项（设计稿没有，样式照其它项）。
- 快捷面板还多了「插件面板」「MCP 应用」两组（设计稿没有，挂着的插件 / 带界面的 MCP 服务各一行）。
- 画布卡：coin 的「生成中 342 行」进度没做（Dock 不流式投影工具参数），只有「正在生成…」。
- 画布导出：没有 PNG / PDF；HTML / JSON 经「存储为」对话框存盘。
- 画布：没做全屏放大；mira 的补丁预览（代码 diff）没做，只有紫横条。
- 会话头「单独面板」：已落地（拆到工作台，可再拖出成独立窗口）。
- 未开的会话：输入框没有模型 / 推理 chip（打开后才有）。
- 工具卡：Bash / Read / List / grep / glob / write / edit / todo / ask / fetch 已按真实输出核对；MCP（search_tool / use_tool）已核对；browser / task 还没。
- chat/SideChat（eme / die / jun / bad / iso / big / led / cat / aln / col / sop / did）→ features/side-chat/SideChatPanel.tsx ✓
  - 偏离：主线横幅不带「去看看」链接（主线就在左边，权限条已在输入框上方）。
  - 偏离：回复下的「带回输入框 / 复制」常驻显示（淡色），不是悬停才出现。
- chat/SideChat（aal / two）→ components/SideNoteCard.tsx ✓
