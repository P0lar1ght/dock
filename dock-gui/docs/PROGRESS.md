# 进度记录

新记录追加在最上面。一条一行，写清日期。

## 2026-11-10

- 画布面板能翻历史画布：顶栏和空状态各一个「打开历史画布」入口，列出别的会话画过的画布，点一下直接切过去打开。
  - 画布跟着会话目录走（`<会话目录>/canvas/<canvas-N>/`），网关的 `canvas/list` 按 threadId 找目录，关着的会话从名册找 —— 不用开页、不读本机磁盘（远程 Dock 一样）。
  - 打开走 `store.openCanvas(threadId, canvasId)`：和对话流里的画布卡同一条路（切会话 + 打开面板 + 选中那块）。
  - 扫描：最近 60 个会话、并发 8 个 `canvas/list`；读不到的会话跳过；结果留 10 秒，顶栏和空状态两个入口共用一次。

## 2026-11-09

- 排队 / 插话等待期间也显示图片缩略图（图和文字在同一个气泡里，正文里不再有「[Image #1]」标记）。
  - 队列项从网关带回图片摘要（`turn/queue/list` 的 `images`），按摘要用 `item/image` 取像素（复用 `dock/userImages.ts` 的缓存）。
  - 正文里的图片标记统一由 `dock/chips.ts` 的 `stripImageChips` 去掉（对话流、轮次导航、排队项共用）。

- 侧栏底部的个人信息块贴住底：之前侧栏这一列高度=内容高，块跟着内容跑（列表短时浮在中间，长了直接掉出视口）。
  - `Sidebar.tsx` 的 `aside` 补上 `h-full min-h-0`：整列撑满窗口高，会话树自己滚（`overflow-y-auto`），底部块靠 `mt-auto` 贴底。
  - 预览加 `#tree-long`（40 个会话）+ `#tree` 改用 App 同样的外壳，方便复现和防回归。

## 2026-11-09

- 只有交互在等用户（权限 / 提问 / 计划 / MCP elicitation）时，不再显示成「运行中 / 正在思考」。
  - 根因在 Dock：没有轮可挂时，网关给这类交互单开一轮（`turn/started` + running），那一轮只有交互项，agent 其实停着等用户。
  - `dock-core` 的 `isRunning` 收紧成「在跑 **且** 真在干活（用户消息 / 正文 / 思考 / 工具）」，新增导出 `turnHasActivity`。
  - 会话头、侧栏、对话流都用它：交互卡片照旧在会话底部等用户回答，不再显示成 agent 在忙。
- 右侧工具面板列出挂着的插件面板和 MCP 应用（原来只有面板头栏的分屏菜单里有）。
  - registry 加一张「右侧面板入口组」表，宿主 ToolPanel 加一个插槽（行样式 ToolRow，和内置项同款）。
  - features/plugins 登记「插件面板」组（`surface/list`），features/mcp-apps 登记「MCP 应用」组（带界面的工具表）。
  - 点一行在右侧开一页（同分屏菜单，不弹独立窗）；一个都没挂时整组不出现。
  - 预览：`preview.html#tool-panel`（假数据摆出这两组）。
- 用户消息里的图刷新后也能显示：事件带内容摘要（`digest`），没有本地预览时按摘要向 Dock 要像素（`item/image { threadId, digest }`）。
  - Dock 侧把像素按内容寻址存进 `$DOCK_HOME/user-images/`，会话日志只写摘要；协议附件多带 `index` 与 `digest`。
- 侧栏收起改成「整列收起」（参考 minimax code）：点左上角的折叠按钮 / ⌘B，左侧整列宽度过渡到 0。
  - 不再画窄图标栏（SidebarRail 删掉）：收起后窗口左上角只剩红绿灯 +「展开侧栏 ⌘B」+ 前进 / 后退。
  - 收起时不卸载侧栏（宽度过渡 + inert），展开回来还在原来的位置。
  - 会话头 / 工作台头栏 / 子代理页里的透明占位重新对齐（和固定的那组按钮同起点）。

## 2026-10-28

- 右侧面板改成叠放的玻璃页（components/PanelStack.tsx，设计：Wonder「Panel Glass Stack」三张）。
  - 一次只摊开一页；别的页压在底下，每页露出 48px 页边（图标 + 竖排名字 + 在跑的子代理数）。
  - 悬停页边 140ms 后那页滑出（最宽 380px）盖在前页上；点页边 / 在探出的页里按下 = 翻到最前。
  - 页都挂着、按 kind 做 key，翻页不重挂；拖出成独立窗口照旧走头栏拖手。
  - 打开已开着的面板 = 翻到最前；快捷键开关：摊开着就合上，压着就翻上来。
  - 功能面板登记加 `label` / `icon`（页边用）、`single`（同前缀只留一页：两个「审查」合成一页）。
- 每轮改动卡 + 右侧「审查」面板（features/turn-changes）；子代理输入框换主会话样式 + 单独审批。
- 差异视图文件头：窄的时候先截目录，文件名留全。

## 2026-10-27

- 修：玻璃材质下设置页「预设管理」点「编辑」没反应——`settings-open` 把窗口里除设置层外的子节点都藏了，编辑器也被藏了（styles.css）。
  - 编辑器开着时改成藏设置层；同一个预设再点「编辑」重挂、现读盘（App 的 `studio.opened`），不再沿用没关掉的旧草稿。
  - 真机（cua）走查：设置 → 编辑调研员 → 改工具 → 保存 → 关 → 再开，一致。
- 预设创建改成「用户主导、AI 接入步骤」（设计：Hofmann「3 · 预设」x=7000 一列）。
  - 新建框默认手动：起名 + 图标 + 底子 →「创建并开始配置」；「让 AI 先起草一份」收成一行。
  - 编辑器头栏「问 AI」：右侧侧聊，带当前草稿和所在步骤（Dock `preset/ask`），只给参考。
  - 会话里「帮我做个 xx agent」：模型调 `propose_preset`，对话里出草稿卡，点开进编辑器审、自己保存。
  - 新对话页按所填目录重列预设（`preset/list { cwd }`），项目层预设也能选；每轮结束重拉预设。
  - 预览：`preview.html#ai-create` `#ai-failed` `#preset-ask` `#preset-ask-empty` `#preset-draft-card`。
- 子代理进侧栏（参考 MiniMax code）：components/SubagentRows.tsx + SubagentPage.tsx。
  - 会话行左边的箭头 + 个数，默认收起；有子代理在跑自动展开；行上转圈 / 红点（出错）/ 灰点（停下）。
  - 点子代理：主区域整页看它的对话，面包屑「主会话 › 描述 · 角色」，点主会话回去；右侧面板保留。
  - 结束的、Dock 重启前的照样能发消息：Dock 同一个 id 接着聊（会话关着先开页）；面板也一样。
  - Dock 侧：子代理对话落盘（`<会话>/subagents/<id>/`）、subagent/list|history 读盘、thread/list 带计数、同 id 复活。
  - 无头真机：假模型派子代理 → 侧栏 / 整页 → 重启 Dock → 从盘读 → 接着聊（父会话没被叫醒）。

- 左下角个人信息能点了（用户给的截图）：components/ProfileMenu.tsx + Avatar.tsx。
  - 菜单：头像 / 名字 / 本机或远程 · 版本，个性化、设置、模型与密钥、快捷键、使用文档、反馈问题。
  - 没账号，所以不做订阅 / 签到 / 退出登录；问号按钮改成帮助小菜单（原来点了没反应）。
  - 窄栏头像同一个菜单；Popover 靠右弹时触发点在下半屏就底边对齐（原来掉出窗口）。
- 设置 › 个性化（settings/Personalization.tsx）：
  - 个人资料：头像（缩成 128px 存 prefs）、名字（只在界面显示）。
  - 自定义指令 = ~/.dock/AGENTS.md，关于你 = ~/.dock/USER.md（Dock 新增这一层），走 profile/get|set。
  - 记忆两个开关同记忆页的键；「管理」跳记忆页。
  - 无头验过：菜单、改名、传头像、两份文本保存（按钮 / ⌘S）、重载读回、窄栏菜单。

- 模型设置改成按供应商（用户给的截图）：settings/Providers.tsx + dock/providerCatalog.ts。
  - 目录用 models.dev（opencode 在用）：打包快照 + 「更新目录」经 Dock 拉新的。
  - 快照：node scripts/update-provider-catalog.mjs（232 家、8946 个模型，按需加载）。
  - 添加供应商 = 选一家 + 填 API Key；密钥进 Dock 的 secrets.json，config 只记名字。
  - 卡片里模型逐个开关：开 = 写 [model."<供应商>/<模型>"]，能力 / 单价抄目录。
  - 「管理模型」用这把密钥问上游 GET /models，目录里没有的也能开；还能手填模型名。
  - 第一个开的模型自动设默认；删供应商连模型和密钥一起删。
  - 原来的逐条表单挪到「自定义模型」页签（自建网关、本地服务）。
  - Dock 侧：[provider.<id>]、env_key 回落 secrets.json、网关 provider/*。
  - 无头验过：自定义假端点（列模型、开关、测连接）、DeepSeek（目录）、编辑、删除。

- 对话流左边加消息导航（用户给的截图）：components/TurnNav.tsx。
  - 每条用户消息一根短横线；当前看到的那条加长加深。
  - 悬停弹出消息列表；指着的那条展开：时间 + 回复开头。
  - 点横线或列表项：逐帧滚到那一轮顶上，不再跟随底部（流式时也不被拽回）。
  - 少于 2 条或不用滚时不画；侧边聊天（compact）不画。
  - 预览：preview.html#transcript-nav；无头 Chrome 验过悬停、跳转、流式不回拽。

## 2026-10-24

- 玻璃材质：组件尽量都是玻璃（用户要求）。
  - 设置页那套「中性底色换玻璃」推广到所有 glass-pane / glass-sidebar，按色系分组。
  - 白 → 半透白；灰 → 烟灰；浅蓝红橙绿紫粉 → 同色半透；悬停 / 停用 / 菜单开着也换。
  - 右侧面板（文件、浏览器、桌面、画布、插件、MCP App）根节点加 glass-pane。
  - 整页（PR、定时任务、预设编辑器）成玻璃面；glass-page 把后面的会话区藏起来。
  - 工作台格子 glass-sheet（玻璃边 + 顶光 + 落影）；弹出的面板 / 会话窗底透出 CSS 玻璃。
  - 修：格子自己也挂了 glass-pane，和里面的会话区叠两层，比壳内白一截；格子改成不铺底。
- 拖出来的独立窗（面板 / 会话）也用原生玻璃：lib.rs detached_builder 建成透明窗，syncNativeGlass 不再只认 main。
  - 独立窗最外层挂 glass-window（原生时透明）；主窗口改了材质 / 透明度，独立窗经 storage 事件跟着换。
  - 无头验：假 label `panel-files-x` / `chat-x` 都调了 set_theme + set_effects，面板底下各层全透明。
  - 原生材质实际观感要在真窗口里看（cargo check / test 已过）。
  - 保持不透：PDF 页、表格、画布 iframe、浏览器画面（glass-keep）。
  - 工具卡代码块走 --code-bg 半透；玻璃下行号栏不钉住（免得代码从数字底下透出）。
  - 计划卡折起：白渐变换成正文 mask 淡出；「回到底部」按钮、新建预设遮罩、分屏线也换了。
  - 验证：隔离 dock serve + 无头 Chrome，透明窗口后垫彩色底。
  - 逐屏扫不透浅色块：新对话、会话、6 个面板、2 个整页、搜索、12 个设置页、菜单、对话框。
  - 只剩头像圆和黑 / 灰的按钮（有意不动）。平面模式规则都在 `[data-material="glass"]` 下，不受影响。

- 侧栏只列当前预设的会话（用户要求）：树从「预设 → 项目 → 会话」改成「项目 → 会话」。
  - 当前预设 = 品牌行下拉（同「新对话默认预设」那个 key）；标题「预设 · 项目」改「项目」。
  - 打开别的预设的会话（搜索 / 通知 / 新对话选了别的预设）：自动切到它的预设。
  - 只跟着换会话走：之后在下拉里换预设不会被拽回来。
  - 窄栏预设图标：点了切预设 + 展开侧栏，高亮当前预设。
  - 没登记 / 不可用的预设（含「未记录预设」）切不过去，那些会话只在搜索里找。
  - 预览：preview.html#tree（下拉切「极简」树跟着换）。

## 2026-10-23

- 启动提速：「连接 Dock」从约 10 秒降到 0.3 秒内（Dock 侧改动）。
  - 实测瓶颈：Dock 挨个等 MCP 握手（wonder 8.6s + sungods-vps 1.8s）才报 ready。
  - Dock 改成后台并发连；GUI 首屏链路实测 ready 0.07s → 会话列表 0.26s。
- 启动画面 LaunchSplash：连得慢（远程 / 冷启动）才出现，品牌标描线 → 填色 → 呼吸。
  - 400ms 内连上不出现；出现后至少 800ms；最多 8s 后让位给连接横条。
  - 标的矢量轮廓 src/assets/markPath.ts（由 assets/logo/mark.png 描边）。
  - store 新增 `booted`：首次预设 + 会话列表铺完（失败也算）。
  - 预览：preview.html#splash（「重放」「连上」）。

- MCP App 常驻面板能拖出成独立窗（拖手拖到窗外 / 「在新窗口打开」）。
  - Rust open_panel_window 认 `mcp-app:<server>:<uri>`：校验两段是 encodeURIComponent 输出，
    label `panel-mcpapp-<fnv8>-<会话>`，URL 里 kind 再编码一层；有单测。
  - registry 面板多 `perSession`：独立窗跟着拖出时的会话（PanelWindow 的 SessionFeaturePanelWindow）。
  - 跨窗口路由（routing.ts，Tauri 事件 mcp-app/sink|query|route）：各窗广播自己开着的面板；
    内联卡认得「开在面板窗口」，调用送过去，点它聚焦那扇窗；新窗出生时补上最后一次调用。
  - 关窗：pagehide 广播常发不出去 → Rust 在窗口 Destroyed 时 emit `window/destroyed`，别的窗清掉。
    （真窗口先测出：关了独立窗，主窗口还以为开着。）
  - 测：scripts/mcp-apps-e2e/popout-check.mjs（两页 + 假事件总线，11 项全过，含独立窗里的界面工具）；
    真窗口 cua drag 拖手到窗外 → 松手处开独立窗、Agent 改白板独立窗跟着变、关窗后主窗口恢复。
- 顺带修（Dock #240）：白板清空后同 id 重画，框里标签被墓碑删掉。label-redraw.mjs 复现 / 验证。

## 2026-10-22

- 修：设置 › MCP 服务卡片的「⋯」菜单被裁掉 / 被下一张卡盖住（卡片 overflow-hidden + 玻璃下各自层叠）。
  - 通用 Popover 的面板改挂 body、按触发按钮 fixed 定位，滚动 / 改窗口大小时跟着挪；点外面判断带上面板。
  - 9 处用到的都受益。scripts/mcp-apps-e2e/popover-check.mjs：旧代码只露顶边 [true,false,false]，修后全露。
  - 输入框模型下拉（向上弹）复查：在按钮上方、全露、点外面收起（composer-menu-check.mjs）。

- MCP Apps 第 5 步：界面自己的工具交给模型（Dock #238 协议 + #239 白板界面工具）。
  - AppFrame：界面 ready 且声明了 appCapabilities.tools → tools/list → mcp/app/view/expose。
  - viewId 每次挂上新生成；list_changed、重连（租约换了）重报；卸下报空收回。报 / 收回排成一队。
  - api.ts：mcp/app/view/call 推送按 viewId 交给自己的界面（bridge.callTool），结果 / 原因送回。
  - 租约由 Dock 按连接定（审阅后改的），GUI 不用传；连接断开 Dock 收回这条连接报的界面。
  - 白板只在面板里给 4 颗：get_selection / get_viewport / focus / snapshot。
  - 无头真 GUI 代码 + 假模型四颗全通；deepseek-flash 自己按提醒调 get_selection、focus 都对。
  - 测试脚本 scripts/mcp-apps-e2e/view-tools.mjs（FAKE=1 走假模型）。
  - 真窗口（Tauri）合并后也走过：手点「找回密码」→ 模型调 get_selection 答对；focus 放大并选中「进入首页」。
- InlineApp 「显示过才记」：工具表晚到时卡片先挂一次再挪出折叠，那一次不算显示过。
  - 之前按「挂过」记，假模型一跑内联白板就成了收起的「显示」（回归，已修）。
- 第三方 map 示例真窗口复看：地图瓦片正常（#237 之后）。

- MCP Apps 宿主（Dock #232–#234）：features/mcp-apps/。
  - Gen UI：带界面的工具卡下面内联界面（registry 新插槽 Attachment），一轮结束不折进「用时」（isArtifact）。
  - UI Plugin：界面请求 fullscreen / 点「在面板中编辑」→ 常驻面板 mcp-app:<server>:<uri>，按项目（会话 cwd）一份。
  - 面板开着时同项目、同资源的调用送进面板，对话流只留「已在右侧面板中更新」。
  - 内联：这一轮刚发生的直接展开；历史里的先是「交互界面 · 显示」（startedAt 早于页面打开）。
  - 桥用官方 AppBridge（@modelcontextprotocol/ext-apps 2.0.3），AppFrame 懒加载（单独 239 KB 的块）。
  - 分屏「+」菜单多一组「MCP 应用」。面板还不能拖出窗口（src-tauri 没认这种 kind）。
  - 端到端：scripts/mcp-apps-e2e（start-env.sh 起假模型 + 隔离 Dock + 前端 1430，run.mjs 10 项）。
  - 真模型（deepseek-flash）走过：画图、进面板、用户画框、模型说得出改了哪、在哪。
- MCP Apps 第三方实测（官方 ext-apps 示例，隔离真 GUI + deepseek-flash）：
  - debug / budget / video / map 正常（map 黑屏 Dock #237 修，真窗口看过）；basic-vanillajs 未测。
  - 独立源：src-tauri/src/mcp_app.rs 的 mcpapp://<server 键>/<id> 吐界面文档（CSP 走响应头）。
  - 原因：srcdoc + 只给 allow-scripts 是不透明源，localStorage 抛错、Cesium 的 Worker 起不来。
  - 现在 iframe 带 allow-same-origin：源跟 GUI 不同，碰不到 GUI 的 DOM / 存储 / Tauri IPC（probe 实测）。
  - localStorage dock-gui.mcpAppSrcdoc=1 退回 srcdoc（排查用）。
  - 交给界面的参数拆 use_tool 外层（Dock #235 appToolArgs）；界面读任意资源走 mcp/app/read（#236）。
  - containerDimensions：内联 {width, maxHeight:720}，面板 {width, height}；换显示模式也发。
  - 工具表晚到时 bumpToolCards，历史卡也能认出界面。
  - 界面卡这一轮跑着和跑完都留在折叠外（跑完再挪会重挂 iframe，App 加载时发的消息会发两遍）。
  - 只在第一次挂上时自动展开：切走会话再切回 / HMR 重挂按历史处理。
    - 实测旧代码：probe 一次调用后切会话再切回，ui/message 多发一条（11 → 12）；修后不变。
  - 「回到底部」改成自己逐帧滚：原生 smooth 在 WKWebView 里点一下只挪几像素就停。

- 玻璃可调（用户：不能调透明度、颜色偏灰偏不透）。设置 › 外观（材质 = 玻璃时出现）：
  - 玻璃样式：透明 = Liquid Glass clear（默认）/ 标准 = Liquid Glass regular / 磨砂 = 原来的侧栏材质。
  - Tauri 2.12 的 setEffects 直接支持 liquidGlassClear / Regular；列表里带 Sidebar，macOS 26 以前自动退回磨砂。
  - 透明度滑块 0–100（prefs.glassClarity → <html> 的 --glass-alpha = 1 − 它/100）。各层白按它缩放，50% = 原来观感。
  - 侧栏 / 终端栏在原生模式下改为一层随透明度走的薄白（50% 时 30%）：透明款几乎无色，全透读不清。
  - 浮着的（弹层、对话框、输入框）不跟滑块走；阅读面、卡片、设置控件有最低值，100% 也不至于全透。
  - 无头 WebKit 量过各层算出来的颜色：0 / 50 / 100% 三档都对；真窗口观感待用户看。
  - 待看：社区报告 Liquid Glass 失焦变白、挡拖动区（那个 demo 把 webview 挪进了玻璃视图；Tauri 这条路不挪）。

## 2026-10-17

- 设置页玻璃：内容区 glass-pane glass-settings；卡片（cardClass）加 md-surface；当前分组是 glass-state；搜索框 glass-field。
  - 各页手写的中性底色在 glass-settings 里统一换玻璃（白 → 半透白，#fafafc / #f5f5f7 / #f2f2f7 / fill → 淡烟灰），彩色底不动。
  - 浮着的（absolute / fixed / glass-popover / glass-dialog）和 glass-keep（开关圆钮）不换，免得半透看不清。
  - 侧栏里的分隔线也换半透细线。真窗口核对：常规、模型与密钥、MCP（失败卡片保留红边）。

- 玻璃下会话里的东西也成玻璃（用户：md 渲染没有毛玻璃效果）。只在 glass-pane 里生效，不加模糊（在滚动区里，每帧重算）。
  - md-surface：代码块、表格、工具输出面板、你发的消息之外的卡片（权限 / 提问、子代理、压缩、笔记、目标条）。
  - md-surface-head：代码块语言栏、表头、「展开」条；md-quote、md-inline-code：淡烟灰；glass-bubble：你发的消息。
  - 代码正文底色改走 --code-bg（.md-code 基础规则平面也用）；玻璃下浅色主题 40%、深色主题 90%（data-dark）。
  - 阅读面里的 border-line* / bg-line 换成半透深色细线；红色报错 / 警告面板保持原样。
  - 真窗口核对：表格、行内代码、气泡、Solarized 代码块、diff、Mermaid。

- 玻璃下阅读面（会话区、侧边聊天、子代理、终端）改成最厚的一层玻璃（glass-pane，白 42%）：用户嫌白底和玻璃色差不舒服。
  - 真窗口取色：会话区约 rgb(227,229,233)，侧栏约 rgb(209,212,218)，同色相、更浅一档。
  - 终端底透明（xterm allowTransparency + 透明 theme，切材质不重建）；xterm.css 给 .xterm-viewport 的 #000 要盖掉，否则整块黑。
  - 滚对话实测仍是 120 帧（8.35ms 均值，240 帧里 1 帧 >12ms）。
- 修：页面重载（热更新 / ⌘R）后旧终端的 shell 不死，越积越多（实测积到 6 个）。
  - pty 记下开它的 webview；on_page_load Started 时 kill_owned_by。实测重载前后都只有 1 个 shell。

- 120 帧：建 webview 前关 WebKit 的 PreferPageRenderingUpdatesNear60FPSEnabled（src-tauri/src/webkit.rs，私有 SPI，先 respondsToSelector）。
  - 主窗口改为 setup 里按 tauri.conf 的 main 配置建（"create": false）+ with_webview_configuration。
  - 弹出的聊天 / 面板窗口同样带；配置只能主线程建，build 挪到 run_on_main_thread。
  - 真窗口实测：主窗口和弹出聊天窗都 8.33ms/帧（120 帧），之前 16.7ms。
  - objc2 / objc2-foundation / objc2-web-kit 直接依赖：Cargo.lock 里本来就有的版本，只开用到的 feature。
- 玻璃扩到其它组件（设计取舍：窗口边框用原生薄片，读的面不透，浮着的东西带玻璃边）：
  - 右侧工具面板：原生材质 + 白玻璃小块（glass-tile），开着的那块是按下去的烟灰玻璃。
  - 工作台底、终端标签栏：原生材质（glass-backdrop / glass-bar）；终端内容不透。
  - 输入框：卡片厚度的玻璃边（glass-card），底下不滚东西所以不加模糊。
  - 对话框（新建项目、删除确认、设置里的确认、预设编辑器 / 工作室、AI 起草、文件树、浏览器提示）：glass-dialog。
  - 文件面板快速打开、表格 / PDF 浮条：glass-popover。
  - backdrop-filter 会让元素成为 fixed 后代的包含块：查过，玻璃元素里没有 fixed 后代。
  - 真窗口核对：工具面板、终端栏、输入框、新建项目对话框都截图看过。

- 玻璃升级为 macOS 原生侧栏材质（Tauri windowEffects = window-vibrancy，系统合成，不走 backdrop-filter）。
  - 主窗口建成透明（transparent + macOSPrivateApi + tauri 的 macos-private-api feature）；代价：不能上 Mac App Store。
  - prefs.ts syncNativeGlass：玻璃 → setTheme(light) + setEffects(sidebar, 跟随窗口激活)，成功才挂 <html data-native-glass>。
  - 原生时窗口底 / 侧栏透明，会话区不透；侧栏悬停改半透灰；设置页开着时藏起底下应用，设置侧栏也是原生材质。
  - 只给 main 窗口；浏览器预览 / 弹出窗口退回 CSS 玻璃。窗口不在前台时侧栏变平灰是系统行为。
  - 真窗口核对：全屏截图里侧栏透出背后窗口；切平面后侧栏是不透的 #f7f7f8，和改之前一样。
- 玻璃材质（设置 › 外观 › 材质，默认平面）：手法取自 Solid Glass（MIT），只抄材质不引整份主题。
  - 整份 solid-glass.css 不能用：靠 shadcn 的 data-slot（我们一个没有），还会把 bg-accent 改成灰、改圆角和字体。
  - 侧栏半透 + 边缘高光，选中行是一小块透明玻璃；侧栏不加模糊（背后只有窗口底）。
  - 菜单 / 弹层毛玻璃：Popover、斜杠菜单与提示、上下文环、会话右键菜单、浏览器下拉。
  - 类名：glass-sidebar / glass-state / glass-popover / glass-popover-fill，只在 [data-material="glass"] 下生效。
- 滚动「不够顺」查清了，和玻璃无关：真窗口探针量到平面 / 玻璃一样，都是稳定 60 帧、不掉帧。
  - 原因：屏幕是 120Hz（ProMotion），WebKit 默认开着 PreferPageRenderingUpdatesNear60FPSEnabled，网页只跑 60 帧。
  - 实测（独立 WKWebView 小程序）：建 webview 前关掉这个 feature → 120 帧；页面加载后再关不生效。
  - 待做：在 Tauri 建 webview 时关它（私有 SPI `_setEnabled:forFeature:`），主窗口和弹出面板窗口都要。
- docs 清理：第 1–8 轮设计任务书挪进 docs/archive/；现行规则合成 docs/DESIGN-BRIEF.md。

## 2026-10-14

- 浏览器面板：下拉框改 GUI 画菜单（SelectMenu）；拖动带 buttons、按下捕获指针；⌘C / ⌘X 经 ClipboardItem 写剪贴板。
  - 视口最窄 800（原 1280）；帧先 decode 再换（解码时来了新帧就丢旧的）。
  - 有头 Dock：open 回 headed，地址栏旁「在窗口中打开」调 browser/view/focus。
  - 无头 Chrome 跑整个 GUI 实测（隔离 dock serve --origin）：下拉选中、拖选复制、⌘A⌘C、滚动 2 倍帧都过。

- bash / monitor 卡片显示描述：行头只放描述，右侧灰色程序徽标（`find`、`cargo test`）；预览 `#bash-description`。
- 权限条：首行完整命令（最多三行），描述另起一行；时间线记录、通知也说描述。
  - 摘要格式来自 Dock spine 的 `permission_summary`（`$ 命令` + 换行 + 描述）。

## 2026-10-13（续）

- 侧栏品牌行用上仓库 logo（`assets/logo/mark.png`）。APP 图标是放大后的圆角白底标，按墨迹重心居中（`src-tauri/icons`）。
- 修：开发时第一次点 xlsx 卡一下然后整页重载回开始页——Worker 里的依赖没被预构建（optimizeDeps.include）。
- 表格能改：csv / tsv（改格子、插删行列、撤销，照原分隔符 / 换行 / BOM 存）、xlsx（只改格子值，样式图表不动）。
  - 公式格不让改；GBK 的 csv 存成 UTF-8 带 BOM 并提示；没读完的大表不让改。
  - 大文件分块上传（Dock 侧 PR #223）：37 MB csv 存 1.1 s、30 MB xlsx 存 1.3 s。

## 2026-10-13

- 文件面板第 3 步：PDF（pdf.js 按需取段）、PowerPoint（@jvmr/pptx-to-html，沙箱 + CSP）、用默认应用打开（仅本机）。
  - 实测：42 MB / 120 页 PDF 首屏 0.22 s，滚到底也只取了 7 MB；200 页 PPTX 首屏 0.8 s。
  - pptx-preview 试过：对 pptxgenjs 生成的文件静默出 0 页，换掉了。
- 文件面板能改：CodeMirror 编辑 + ⌘S、冲突提示、右键 / 头栏新建、F2 改名、拖动移动、移到废纸篓。
  - Dock 侧 PR #222（fs/write|mkdir|rename|trash + spine 插件 user-edits 提醒 agent）。
  - 无头端到端全过（编辑、冲突覆盖、没保存的提示、嵌套新建、拖动、改名、真进废纸篓）。

## 2026-10-12

- 文件面板看表格 / 文档：CSV、TSV、xlsx / xlsm / xls / ods、docx（features/file-preview）。
  - Dock 侧：fs/read { raw, offset, length } 分块读字节（能力 workspaceFilesRaw）。
  - 实测（本机 debug Dock）：50 万行 CSV 首屏 0.15 s、全部 0.7 s。
  - 20 万行 xlsx（30 MB）：读完 0.9 s，前 200 行 1.4 s，整张 3.7 s。
  - 300 章 docx 0.3 s；第一次开 docx 0.11 s（去掉 Suspense 前 0.83 s）。

## 2026-10-11

- 工具卡美化（清单 docs/TOOL-CARDS.md；用户定：直接写代码、逐个美化、真实调用实测）。
  - 画法拆到 components/tools/{parts,specs}.tsx；ToolCard.tsx 只留外壳四态。
  - 读 / 写 / 编辑按文件类型高亮；grep 按文件分组标命中；diff 标行内改动；bash 终端样式。
  - 补齐原来走兜底的：job / monitor / kill_task、计划、目标、子代理、技能、记忆、LSP、定时、Cordis。
  - 浏览器 / 桌面 MCP 原来全是通用卡（名字带 mcp_ 前缀没认出）：按动作画，截图走 item/image 缩略图。
  - 实测：隔离 Dock + 无头 Chrome 跑完整 GUI，9 组提示词真模型调用；样本存 src/preview/fixtures/。
  - 修：use_tool 调的工具正文拿错参数（monitor 命令空着）；diff 平手先加后删。

- 文件面板美化：按类型着色的文件图标（树、快速打开、预览头栏），文件夹蓝色、展开换打开的图标。
  - 源码视图走 Shiki 高亮，跟「代码高亮配色」设置；超过 300 KB / 6000 行不高亮，底栏说明。
  - Markdown 默认看排版后的预览；SVG 预览看图（棋盘底）；CSV / TSV 预览看表（数字列右对齐）。
  - 类型表在 components/fileType.ts（图标、颜色、底栏名、高亮语言）。

- 回复里行内代码写的文件路径能点：在文件面板打开，`a.ts:42` / `#L42` 跳到那一行并标底色。
  - 只认像文件的（带扩展名；不带 `/` 的只认常见类型）；绝对路径要在会话目录下。
  - 路径对不上按文件名找：唯一就开，同名多个弹快速打开并填好名字。
  - 侧边聊天按主会话目录开；子代理面板同样能点。预览 `preview.html#files-link`。
- 修：侧栏会话行「运行中」的圈不转（enter-fade 和 animate-spin 抢 animation）。
- 修：停止键「正在停止」的脉冲同理被 pop-in 盖掉。
- 上下文圆环保持原样（用户定：不改）。

- 对话流美化（用户定：直接写代码，加 Shiki / Mermaid / KaTeX）。
  - 代码块：头栏（语言 · 换行 · 复制）+ Shiki 高亮，按需加载、按代码缓存。
  - ```mermaid 画成图（可切源码）；$…$ / $$…$$ / \( \) / \[ \] 用 KaTeX 排。
  - 修：`---` 画成粗黑线；任务列表圆点 + 原生勾选框叠在一起。
  - 标题 / 列表记号 / 行内代码 / 表格 / 引用重排；流式正文末尾闪烁光标。
  - 回复按顶层块切开 memo：流式只重渲染最后一块。
  - 对话和输入区居中同一列（780px）；轮次间距加大；底边渐隐。
  - 你发的消息超过 10 行折起；侧边聊天正文 14px。
  - 设置 › 外观 › 字体与代码：对话字体、代码字体、高亮配色，带实时预览。
  - 预览：`#transcript-reply`（各种 Markdown）、`#transcript-stream-text`（流式光标）。

- 修：流式（思考 / 回复）时对话流往上滚不动。
  - 原因：每个增量起一段平滑滚到底，触控板小步上滚离底不到 80px 就被拽回。
  - 改：往上滚一下就停止跟随；跟随改为每帧一次、直接定位。
  - 预览 `preview.html#transcript-stream`：每 30ms 进一段思考，用来测滚动。

## 2026-10-10

- 侧边聊天（配合 Dock `thread/aside/*`，能力 `sideChat`；设计 DESIGN-TASKS-8 → wonder-ui/chat/SideChat）。
  - 右侧面板 `sidechat:current`（features/side-chat），工具面板「侧边聊天」/ ⌥⌘S 打开。
  - 从当前会话分叉的只读旁问；每个主会话一个，切走再切回还在；关面板就结束。
  - 每条回复下「带回输入框」（fillComposer append，只填不发）、「复制」。
  - 头栏「写进主线」：起草 → 可编辑预览（≤2000 字）→ 写入；主会话里出现笔记卡（SideNoteCard）。
  - 主线在等批准 / 回答时面板顶上常驻横幅；主线跑完 / 出错横幅 3 秒。
  - 侧聊权限条不给「始终允许」（Pending `noAlways`）；对话流不给撤回（会把主线快照带出来）。
  - 侧聊不发系统通知、不进「刚完成」；主会话关掉 / Dock 重启后面板显示「已结束」。
- 工具面板删掉「审查」入口（用户定）。

## 2026-10-09

- 插话 / 排队 / 停止并发送（配合 Dock #205 / #206，网关能力 `turnSteer`）。
  - 运行中 Enter 一律排队；去掉输入框里的「打断并发送」按钮。
  - 排队条每条加「立即插话」（turn/queue/steer）：Dock 在下一个步骤边界并进这一轮，不打断。
  - 插话送达前：蓝色实线气泡 +「插话 · 等待合适时机送达」，可取消（turn/queue/remove）。
  - ⌘Enter 保留为停止并发送（在跑时的 turn/start），快捷键 id 改 `stopAndSend`。
  - 被插话截开的 `turn/completed { steered }` 不通知、不算刚完成；用时行写「收到插话，接着往下」。
  - 插话的用户气泡下方带「插话」标。旧版 Dock（无 `turnSteer`）不显示「立即插话」。

## 2026-10-06

- 远程连接内置 SSH 隧道：编辑框「连接方式」SSH 隧道 / 地址。
  - src-tauri/src/tunnel.rs：`ssh -N -L 127.0.0.1:<随机>:127.0.0.1:<远端端口>`，BatchMode、accept-new。
  - 当前连接一条隧道（各窗口共用），断了下次连接时重开；测试连接开临时隧道，测完关。
  - 隧道通后经它发 HTTP 探活；远端没在跑就经 SSH 跑「启动命令」（默认 systemctl start dock），等 40 秒。
  - 认证 / 主机指纹 / 私钥错误、远端起不来 → `[fatal] ` 前缀，前端不再重连（原 `[keychain] ` 并进来）。
  - 真机：VPS 先 systemctl stop dock → 测试连接自动拉起 → 使用 → 杀 ssh 后自动重建并重连。
  - VPS 已配 /etc/systemd/system/dock.service（DOCK_HOME=/opt/dock-build/home，127.0.0.1:18990）。
- 远程 Dock 连接：GUI 能连 `dock serve --remote`（用户授权设备令牌受信，设置跟随远端）。
  - Tauri：src-tauri/src/remote.rs 管 `remotes.json` + 钥匙串（keyring，service `dock-gui`）。
  - 令牌读过一次缓存在进程里；读钥匙串失败带 `[keychain] ` 前缀，前端当 fatal 不再重连。
  - `dock_connection` 按当前连接回 ticket 或 token；切到远程收掉本机 Dock，切回再拉起。
  - 切换 / 改了当前连接后发 `dock://connection`，各窗口整页重载。
  - client.ts：令牌鉴权；4401 / 4429 / unauthenticated 记 `fatal` 不再重连；远程退避到 15 秒。
  - 启动时没连上也会自己重连；第一次连上铺会话列表（也补热更新换 store 的情况）。
  - 设置 → 网关与设备 → 连接（Connections.tsx）；连接横条加远程文案和「令牌失效」红条。
  - 远程时藏起：终端、各处「在访达中显示」、Dock 路径；MCP OAuth 改提示去远端登录。
  - 项目名单 localStorage 按连接分 key（`@<连接 id>`），本机项目不串到远端。
  - 选目录走 RemoteDirectoryPicker（`fs/dirs`）。
  - 真机走查（本机起 dock serve --remote 当远端）：添加 / 测试 / 使用 / 选目录开会话 /
    设置页读远端 / 撤销令牌红条且不重连，都过了。
  - 坑：开发构建每次重编签名都变，读钥匙串会弹授权框（安全输入，cua 点不了，要人点）。

- 插件 web 面板（方案 B，用户拍板放开「GUI 不跑第三方代码」）：features/plugins/WebPanel.tsx。
  - 沙箱 iframe（只 allow-scripts）+ 注入 CSP（不许联网）+ postMessage 窄桥（threads.list / history / watch、panel.action）。
  - 主窗口不往 GUI 的订阅里塞别的会话（store 会把任何线程的事件当真）；独立窗自己订阅。
  - 实测：iframe 里没有 __TAURI_INTERNALS__，fetch 本机端口被拦。
- 示例「会话轨迹」（examples/plugins/trajectory，PR #201）：真 GUI 里看了用户的真实长会话，检查器看参数 / 结果。
  - 已知：从磁盘回放的历史计时不准（落盘 ts 是秒、模型那行记在请求开始），实时的准。
- 插件面板能弹出成独立窗（拖手拖出窗口，或头栏「在新窗口打开」）。
  - Rust：open_panel_window 认 `plugin:<id>`，label `panel-plugin-<id>`，全局一扇（不跟会话），440×680。
  - 独立窗头栏多「置顶」（setAlwaysOnTop，capability 加 allow-set-always-on-top）。
  - 点状态项时面板已在独立窗：focus_panel_window 提到前面，不在主窗口再开一份。
  - 插件自己关面板：独立窗直接关。
- 示例插件「Agent 活动」（/tmp/bvtest/activity）：状态、工具用量、最近活动、暂停 / 清零、设置卡。
  - 真 Tauri 窗口里走过：状态项 → 面板 → 弹出 → 置顶浮在别的应用上 → 发消息实时更新 →
    独立窗里暂停，主窗口状态项跟着变。
- 插件视图按第七轮稿落地（wonder-ui/plugins，15 张；#181 阶段 4f）。
- 新目录 src/features/：功能按类型登记到宿主，宿主不 import 功能目录。
  - registry.ts：右侧面板、分屏菜单分组、工具卡、设置页插件行、输入框工具栏。
  - FeatureHost（App 提供）：openPanel / closePanel / toast。
- features/plugins/：
  - ViewTree：dock.view.1 通用画法（图鉴 hum 逐节点对过）。
  - 状态项：工具栏右组最前，量剩余宽度摆几个，多的「+N」，再挤就藏。
  - 工具卡视图：插件工具换 Puzzle 图标，展开拉 tool/view，「视图 | 原始」切换。
  - 设置卡：插件行「配置」就地展开，按 schema 生成，dock-core 同文案校验。
  - 插件面板：kind `plugin:<id>`，底部固定按钮行，执行中禁用，插件关面板出 toast。
- 宿主改动：Composer 右组插槽、ToolCard 头图标 / 正文插槽（带 threadId / itemId）、
  Plugins.tsx 行插槽、PanelControls 分组插槽、App 面板兜底与 toast、Markdown 加 view 尺寸。
- 验证：隔离 DOCK_HOME + 磁盘插件 deploy + 假模型 + 无头 Chrome（/tmp/bvtest/plugins/e2e.mjs）。
  - 四个扩展点都用真数据走过；修了工具栏拥挤（量宽度）与测量副本是真按钮的问题。
- 没做：插件面板拖出 / 新窗口（要改 src-tauri，先问用户）；
  工具卡 Fallback 的「插件未加载」提示（契约里拿不到工具属于哪个插件）。

## 2026-10-05

- 用户消息「撤回」：这条和之后的对话删掉，正文和图片回输入框（Dock 侧 PR #178 `thread/rewind`）。
  - 有后续或已有输出要确认（RewindConfirm）；最后一轮还没输出直接撤。
  - 确认框走 portal 挂 body：气泡的进场动画带 transform，fixed 遮罩会被圈在气泡里。
  - `store.rewind` 撤完整份重拉；`fillComposer` 多一个 images 参数。
  - 真机走查过：撤回、改了再发、全部撤回后落盘删掉。
- 输入法回车误发：WebKit 里确认候选词那次 keydown 的 isComposing 是 false、keyCode 229。
  - 新增 `src/dock/ime.ts` 的 `imeComposing`，替换全部 16 处只看 isComposing 的判断。
  - 待用户用中文输入法实测。

## 2026-10-04

- PR 页提速：网关一次 GraphQL + 列表 60 秒缓存 + PR 没变时复用详情（#173 bf02395）；
  GUI 先画上次的列表再后台刷新，「刷新」= force，悬停侧栏 / 某一行时预取。
- 侧栏「Pull Request」「定时任务」两个整页按第六轮稿实现（navPage / activePage）。
  - 真机走查过：PR 列表与详情用真实 gh 数据；新建 / 删除定时任务写进 schedules.json；
    别的进程改文件后列表自动刷新（schedule/changed），「由其他窗口运行」行也对。
  - 修了：悬停按钮不再占位（胶囊贴右）；新建后「8 天后过期」→ 拉数据时刷新 now。
- 侧栏「插件」接上：打开设置页「插件与技能」（onOpenPlugins）。
- Dock 侧 PR #172：定时任务落盘、属于会话、跨重启；dock.1 `schedule/*` + `schedule/changed`。
- Dock 侧 PR #173：`vcs/pr/list | get`（经本机 gh，只读；没 gh 回 available:false）。
- 数据层：`src/dock/scheduleApi.ts`、`src/dock/prApi.ts`。
- 第六轮设计需求 `docs/DESIGN-TASKS-6.md` 已发给 Grok Bot · UI 设计助手（PR 页 + 定时任务页，12 张）。
- 待办：设计稿到了按稿实现两个页面，接侧栏「Pull Request」「定时任务」。
- 目标 / 上下文按 Wonder 稿 had 实现（wonder-ui/chat/ComposerGoalContext）。
- 输入框「目标」胶囊（/goal 不带参数也是它）：发出去走 turn/start 的 goal intent。
- GoalBar：进行中走表 / 展开看进展、改目标 / 暂停 / 继续 / 清除；完成变绿 4 秒收起。
- 用户消息下「设为目标」：在跑就 thread/goal/set，空闲就把这句话当目标再发。
- ContextRing：模型左边的圆环，点开（或 /context）拉 thread/context/get 看明细。
- Dock 侧（分支 feat/gui-goal-context）：goal 投影 completed + elapsedMs；新方法 thread/context/get。
- 环境在一轮开始 / 结束、压缩结束时立刻重拉，工具结果后节流 2 秒重拉。
- 验证：preview.html#goal-context 截图；真 dock serve + 假模型无头端到端（/tmp/bvtest/goalctx/e2e.mjs）。

## 2026-11-18

- 右侧那一栏：折叠 / 展开合并成窗口右上角一个「侧边面板」按钮（位置固定，展开收起都不动）。
- 折叠改成过渡：宽度 0↔实际宽度（slow）+ 内容淡出，不再瞬间消失。
- 修右侧展开不满、右边空一大块：那一栏改按实测像素定宽（flex item 的百分比宽度会塌成内容宽），会话列吃满剩余（两边 flex-grow 加起来要满 100%）。
- 快捷面板头部去掉两个重复的折叠按钮，改成标题「工具」。
- 「再叠一页」菜单补全：功能面板（侧边聊天、审查）也列进去（features/registry.ts 新增 registerPanelMenuItem）。
- 右侧那一列改成标签页形态：顶部单独一行标签（当前打开的面板，点一下翻过去），折叠按钮在这一行最右；下面才是面板内容。折叠按钮展开 / 收起都在窗口右上角同一坐标（右边距 12px、y=8）。
- 面板栈原来的展示不动（页边、悬停探出照旧），只在其上面加一行标签。

## 2026-10-03

- 技能同病已修：按会话项目找；技能页按项目分组（内置与用户级一组）。
- 修了多页时 `/技能` 正文写进第 1 页的问题（Dock 侧）。
- 修了「已保存」跨页残留：只认本页打开之后的保存。
- 真机走查（cua）：常规 / 模型 / 插件（立即加载 cyberspace → 运行中）/ 技能 / MCP / CUA / 网关 / 外观 / 关于。
- 插件页拆成「永久插件」（用户级 + 每个项目分组）和「动态插件」（标出来自哪个会话）。
- 修了项目插件认不到：Dock 的项目插件根原来按进程 cwd，GUI 下是数据目录。
- 开某个项目的会话就加载它的永久插件；未加载的可「立即加载」。
- 实测：隔离 DOCK_HOME 开 dock 仓库的会话，cyberspace 加载并跑起来，工具进了目录。
- 设置页按第五轮设计重写（components/settings/），29 张全部落地。
- 模型与密钥、MCP、记忆、浏览器、CUA、插件、配对、设备都能在 GUI 里改。
- Dock 侧：dock.1 新增设置方法，只给受信 ticket（见 cordis-gateway README）。
- GUI 偏好：dock/prefs.ts（新对话默认、通知、外观）+ dock/shortcuts.ts。
- 系统通知：加了 tauri-plugin-notification。
- 实测：隔离 DOCK_HOME 起 dock serve，Python 跑 28 项协议用例全过。
- 实测：preview.html#settings-* 无头 Chrome 截图逐页对过设计。
- 没在真窗口里点过（锁屏）：解锁后要用 cua 走一遍。
- 设置页第五轮首交 25 张已验收：27 条反馈写进 DESIGN-TASKS-5 末尾，已发回 Grok。

## 2026-10-02

- 设置页第五轮设计需求：`docs/DESIGN-TASKS-5.md`，已发给 Grok Bot · UI 设计助手。
- 产出约定导出到 `wonder-ui/settings/<画板名>/`；模型与密钥改成 GUI 可增删改。
- 权限条：补「始终允许」（本会话这个工具不再问，子代理共用）；子代理发的标「来自子代理 · 角色」。
- 预览页加 `#permission`：只摆两种权限条（无头 Chrome 截图验过）。
- 子代理：对话流卡片 / 并行组卡、右侧子代理面板（列表 / 详情 / 插话 / 打断 / 结束）。
- 子代理：快捷面板「子代理」带运行数徽标；主代理停了子代理还在跑，会话头写「N 个子代理在跑」。
- 子代理：数据来自网关新加的 `subagent/*`（dock-core `subagents.ts` 归约）。
- 真机测过：并行两个后台 explore、一个前台 explore；卡片、组卡、面板、回报卡都对。
- 已知（Dock 侧旧问题）：分页派的子代理回报进了第 1 页的信箱，主代理不会被叫醒汇总。
- 子代理画面：返工验收通过，19 张都有 jsx + png（整窗输入框 740 固定宽只是 Wonder 限制，实现用真 Composer）。
- 子代理画面：Grok Bot 交了 19 张（PNG），已回 5 处返工：补 jsx、整窗输入框溢出、占位图标、面板头 ×、结束态收尾句。
- 子代理画面：需求写进 docs/DESIGN-TASKS-4.md，经 cua 发给 Grok Bot「UI 设计助手」在 Wonder 画。
- 浏览器标签栏：每页都能关，关最后一页 = 关掉会话的浏览器页，面板回到「还没打开网页」。
- Dock 侧：Chromium 不再自报被自动化（webdriver / HeadlessChrome），Google 少弹人机验证。
- 浏览器地址栏：不像网址的输入走 Google 搜索（以前「抖音」被补成 https://抖音）。
- 浏览器面板：头栏换成标签栏（BrowserTabs），会话的全部标签页，能切、新开、关。
- 标签栏和 agent 共用当前页（用户拍板）；agent 跑的时候只读，当前页亮蓝点。
- 页面自己开的新页（target=_blank、window.open）收进会话：滑进来、闪蓝边，画面跟过去。
- 状态和「接手 / 交还」挪到地址栏右边；「+」之后地址栏直接可输入。
- 修：重启 GUI 后面板一直「浏览器已退出」——名册里是被杀掉的旧 MCP 进程的页。
- 修：地址栏输 `localhost:3000`、`127.0.0.1:8765` 补 http（以前补 https 打不开）。
- 真机测过：面板里点 target=_blank / window.open、切换、关闭、新开；agent 点链接后切回。
- Wonder 补画 `das`（标签栏主稿）和 `mba`（标签栏四态），导出到 pages/BrowserPanel。
- 桌面面板：实时画面叠 agent 光标（AgentCursor），轨迹 / 涟漪 / 拖动影子 / 键帽。
- 桌面面板：执行中的像素点击先滑过去，做完按网关的真实位置校准。
- 桌面面板：标题行显示画面状态（没有窗口 / 截图失败 / 断开），不再一直「正在连接」。
- 修：桌面画面一直「正在连接」——连发两次 open 时晚做完的旧 open 顶掉新的（网关按收到顺序定序）。
- 修：光标层按画面实际占的矩形铺（object-contain 留白会把横坐标拉偏）；靠边时标签翻面。
- 真机测过：Dock agent 用 cua 按计算器 1+2=、AC 7×8=，光标落点、轨迹、空闲淡出都对。
- Wonder 补画 `web`（实时画面 + 光标）和 `rfc`（光标状态表），导出到 pages/DesktopPanel。
- 浏览器面板：页面视口宽度至少 1280，窄面板整页缩小显示（B 站、微博不再截掉右边）。
- 补静止帧倍数按缩放比算，滚轮位移按缩放比放大。
- 滚动跟手：排队的滚轮并成一个（停手后积压 605ms → 16ms）；推流质量 80 → 60，帧小约四成。
- Dock 侧：同一页的 2x 静止帧排队截（两个视图并发会把视口翻倍、回不去）。
- 浏览器面板：视口跟着面板走（2x 清晰）、空闲直接可操作、没标签页时地址栏直接开页。
- 桌面面板：实时画面（desktop/view/*）为默认，点步骤回看截图，最近的点击叠圈。
- Dock 侧：Mcp::call_as、browser_resize 加 deviceScaleFactor、网关 browser/view 视口 + desktop/view。
- 修：Canvas 面板旁的分隔线拖不动（iframe 吞 pointermove）；拖动时 iframe 不接指针，每帧最多更新一次。
- 画了 Wonder「Page - Side Panels Split」（sow-gos）+「Side Panels Split States」（blo-ama），导出到 wonder-ui/pages/SidePanelsSplit。
- 右侧面板可多开并排：快捷键 / 快捷面板各管一个开关，新开的加在最右边，增减后均分。
- 面板头栏加「分屏」菜单（列出还没开的）和「在新窗口打开」；左侧拖手拖到窗口外开成独立窗口。
- 面板之间的分隔线：SplitDivider 的 pair 模式，只分左右两栏，双击两栏均分。
- 独立面板窗：PanelWindow，跟着拖出时的会话；Rust open_panel_window，pop_out_drag 加 panel 参数。
- 修：独立窗里 window.close() 关不掉（聊天窗删会话后也是）；改 getCurrentWindow().close() + allow-close。
- 真机实测：菜单分屏、⌘T 加栏、关栏补位、栏间拖动 / 双击均分、新窗口按钮、拖手拖出、独立窗关闭。

## 2026-10-01

- 画了 Wonder「Feed Compaction States」（tarr，工具卡状态旁），导出到 wonder-ui/chat/FeedCompaction。
- 压缩卡 CompactionCard：进行中（阶段 + 已写 tokens + 计时）、重试提示、失败（可重试）、已停止。
- 动效：标题 shimmer-title、不确定扫光 sweep；压缩时不挂「正在思考」。
- 真机（无头 Chrome + mock invoke + 假模型）验过：重试 → 写摘要 → 完成标记；失败卡点「重试」能再压。
- 压缩标记：对话流里「上下文已（自动）压缩 · 182k → 21k · 9.1s」（前后占用只实时有）。
- 空闲时手动压缩单独成一轮：只画标记，不配「用时」行。
- 真机实测（Tauri 窗口 + 隔离 DOCK_HOME + 假的 chat/completions 模型，cua-driver 点按截图）：
- 画布：建两块、标题下拉切换、agent 改数据就地重绘、页面里 setData 写回、dock.send 进输入框都对。
- 画布导出改成真存盘：Tauri 命令 save_text_file 弹「存储为」，路径只由对话框定；导出文件离线能开。
- 在访达中显示：打开画布目录，对。
- 修：画布面板窄时顶栏溢出（关闭按钮被挤掉）；容器查询收起「导出」字样、刷新、画布 | 源码。
- 修：对话流重载时画布面板会跳回最后一块（跟随逻辑把「变空再回来」当成新写入）。
- 修：MCP 工具经 use_tool 调时，桌面面板步骤、浏览器面板「正在操作」认不出（只认本名）。
- 新增 toolCall.ts：calledTool 拆开 use_tool 的 tool_name / tool_input。
- 修：文件面板同一文件再改一次不自动重读（触发条件按路径集合，改成按写的次数）。
- 修：桌面面板还没截图、agent 在做第一步时显示「这几步没有截图」，改成「等 agent 截第一张图」。
- 浏览器接手：真窗口里点输入框、打字、退格、回车提交表单都对。
- 文件面板：agent 写文件 → 批准 → 树里出新文件和蓝点；正在看的文件自动重读。
- 桌面面板：agent 经 use_tool 调 cua-driver 截图 → 批准 → 实时出步骤和画面。
- Dock 侧 PR #158：browser/view 连发 open 留最后一个并推 closed(replaced)，input 按收到顺序。
- 清掉 9/17 的孤儿 Chromium（占着 ~/.dock/browser/user-data）。

- 画布：设计 sert（并排面板）+ coin（卡片四态）+ orby（版本 / 源码 / 导出 / 切换）+ khan（出错）+ mira（交回 agent）。
- Dock 侧（分支 feat/canvas）：工具 canvas_create / edit / data / read + 网关 canvas/list、get、setData、rollback。
- 落盘：会话目录 canvas/<id>/ 下 meta.json + v<n>.html + data.json；回退把旧版拷成新版。
- CanvasPanel.tsx：沙箱 iframe（allow-scripts，不同源）跑 agent 写的 HTML。
- canvasDoc.ts：注入桥 window.dock（data / onData / setData / send），dock:lib/chart.js 换成随包的 data: URL。
- 桥放在 <head> 那一行、不加换行：报错行号就是 vN.html 的行号。
- 沙箱里碰 localStorage 会抛错：桥换成内存版。
- 只改数据（同一版）不重载页面，推给 dock.onData 就地重绘。
- 顶栏：标题下拉切画布（orby 方案 A）、版本下拉（看旧版 / 回退）、画布 | 源码、刷新、导出。
- 导出：自包含 HTML / 数据 JSON 存盘、复制 HTML 源码、在访达中显示。
- 运行出错：红横条 + 「查看源码」（定位到那一行）+ 「让 Agent 修复」（填进输入框，不自动发）。
- agent 正在改：紫横条；看旧版：灰横条带「回到最新」「回退到这一版」。
- CanvasChip.tsx：对话流卡片，生成中 / 已生成 vN / 已更新至 vN（说明）/ 数据已更新 / 运行时报错。
- 画布卡是产物，轮次结束后不随过程折起来。
- 卡片「打开」开画布面板并切到那块；面板开着时卡片标「右侧已展开 ✓」。
- 入口：快捷面板「画布」或 ⌘⇧C；和浏览器 / 文件 / 桌面共用右侧槽位。
- 随包的库：chart.js（新依赖，按 dock:lib 引用才加载）。
- 实测：假的 chat/completions 模型按脚本调工具，真 dock serve 跑 agent 循环，无头 Chrome 看 GUI。
- 建画布 → Chart.js 出图；页面里 setData 写回 data.json；dock.send 填进输入框。
- canvas_data → 同一个 iframe 就地重绘；改坏 → 红横条、红卡、修复话术带行号；回退 → v3 正常。
- 未实测：「生成中」卡片（Dock 等工具调用流完才投影，工具本身很快）。

## 2026-09-30

- 桌面（CUA）面板：设计 lake（主稿）+ ian-bbc（四态），导出到 wonder-ui/pages/DesktopPanel。
- Dock 侧 PR #156：工具结果里的截图进 dock.1（tool_completed.attachments + item/image）。
- DesktopPanel.tsx：上面是 agent 截到的画面（默认最新），下面是 cua-driver 每一步的操作记录。
- 点带相机图标的步骤看当时的画面；有新截图自动跳回最新。
- 驱动状态看 mcp/list 的 cua-driver 行：未安装（给安装命令 + 重新检测）/ 连不上（原因 + 重新连接）/ 已停用。
- 入口：快捷面板新增「桌面」（设计稿没有这项）或 ⌘⇧D；和浏览器 / 文件共用右侧槽位。
- 实测：隔离 DOCK_HOME 里按 persist.rs 格式造一份落盘会话（5 步 + 1 张截图），截图、失败步、未安装态都对。
- 未实测：真 agent 跑 cua-driver 时的实时刷新与「正在操作」浮条。
- 文件面板：设计 bam-alk（主稿）+ wut-mes（四态），导出到 wonder-ui/pages/FilesPanel。
- Dock 侧 PR #155：网关 fs/list、fs/read、fs/find（只读，路径不出会话 cwd）。
- FilesPanel.tsx：左树（照 .gitignore，懒加载）右预览（行号源码 / md 预览 / 图片 / 二进制提示）。
- ⌘P 或树上的搜索框：快速打开，↵ 打开并展开到那一层，⌘↵ 插入输入框。
- 蓝点 = 这个会话里 agent 用写文件工具改过的文件（从对话流取）；改完自动重读。
- 「插入输入框」往输入框末尾加 `@路径`（store.fillComposer 加 append 模式）。
- 右侧大面板改成一个槽位：浏览器 / 文件同时只开一个，开着时替掉快捷面板。
- 窄的时候预览头栏先藏目录、再把「插入输入框」收成图标（容器查询）。
- 实测（同一套无头 Chrome 办法）：列树、读 md / csv、⌘P 找 flig → 打开、看图、插入路径都对。
- 未实测：蓝点与自动重读（要 agent 真跑写文件）、在访达中显示。
- 浏览器面板：设计 turm（主稿）+ alo-ali（四态），导出到 wonder-ui/pages/BrowserPanel。
- BrowserPanel.tsx：走 dock.1 browser/view/*，看会话正在用的标签页。
- 入口：右侧快捷面板「浏览器」或 ⌘T；和会话区并排，中间可拖，双击回 42%。
- 默认只读；「接手操作」后鼠标、滚轮、键盘发给页面，「交还 Agent」回只读。
- 接手时这一轮还在跑：黄条提示可能同时操作，可「停止这一轮」。
- 键盘走隐藏 textarea：可打印字符和输入法走 insertText，其余键走按键。
- 状态：连接中（骨架）/ 未打开网页（示例可填入输入框）/ 已退出（压暗最后一帧）。
- 没标签页、浏览器退出时每 2.5 秒重试：agent 一开网页画面自己出来。
- 运行中画面底部浮「Agent 正在操作 · 最近的浏览器工具」。
- 修：同一会话两次 open 并发时，网关会悄悄关掉先开的那个（dev 下 StrictMode 必现）。
- 改成 open 延迟 120ms 只发最后一次；输入回「没有视图」就自动重开。
- 输入串行发：网关每个请求各开任务，按下 / 抬起可能乱序；连续移动只留最新。
- store.fillComposer / onFill：别的面板往会话输入框填字（不发送）。
- 实测（隔离 DOCK_HOME + 无头 Chrome 跑 vite 页，mock Tauri invoke）：
- 出画面、接手点链接跳转、打「hello 世界」+ 退格 + 回车提交表单都对。
- 关标签页 → 未打开网页；再开 → 自动恢复；杀 Chromium → 已退出。
- 未实测：真 Tauri 窗口里的接手打字、agent 真跑一轮时的浮条。

## 2026-09-26

- 思考过程：网关新增 item/reasoning_delta（transcript.rs，照 message_delta 推增量），dock-core 加 reasoning 项。
- 对话流：想的时候「正在思考 · Ns」流光 + 最新三行滚动；想完折成「思考了 Ns ›」，点开看全文。
- 一轮在跑但没东西在动（刚发出、工具跑完等下一步）：底部挂一行流光「正在思考」。
- 验证：网关新测试（去掉改动会挂）、dock-core 新测试；另起 dock serve 回放旧会话能吐出思考增量。
- 要用上得重启 Dock（设置页或连接条的重启），旧进程不会推思考。
- 斜杠命令手感：结果卡从输入框里挪到上方浮起（Esc / 点外面收，短回执 3 秒自己收），不再挤输入框。
- /help 不再问 Dock（它的帮助混着终端专用命令），改成打开下拉；下拉分「命令」「技能与工作流」两组。
- /model、/effort 后面直接列可选模型 / 档位，选中走 thread/model/set、reasoning/set（和右下角菜单同一个）。
- 输入框附件：「+」选图 / 粘贴 / 拖进来，最多 4 张；规范成 PNG/JPEG/WebP ≤2MB ≤4096 再 imageInputs/put。
- 本窗口发的图在气泡里有缩略图、点开看大图；历史图 Dock 不回像素，只画尺寸块。
- 斜杠命令：「/」弹下拉（slash/list，去掉 /new /resume），整行走 slash/execute；结果分填回 / 提示 / 已发 / 当普通消息。
- 计划模式开关放在审批旁（thread/plan/set，环境里读 plan.enabled）。
- 运行中：Enter 排队，⌘Enter 或「打断并发送」走 turn/steer——Dock 会停掉当前这一轮再发，不是往这一轮里插话。
- 实测：另起 dock serve 用脚本调 slash/list、/help、/context、/goal、plan/set、imageInputs/put 均通；App 里计划开关来回切一次。
- 未实测：真在 App 里发图、打「/」、打断并发送（前台打字才进得了 WKWebView）。
- 新对话页目录框加「选择…」（点文件夹图标也行）：原生选文件夹框，从当前填的目录打开；手填照旧。
- 选目录抽成 pickDirectory.ts，和「+ 创建项目」共用。
- 修：窄栏时预设对话框遮罩盖住会话头，「切换侧栏 / 后退 / 前进」没了；工作站视图也一直没有。
- 改成 App 固定画在左上角（FloatingWindowControls，z-35），会话头 / 工作站头栏只留透明占位。
- 侧栏项目右键菜单（设计 aru，参考 Codex）：新对话 / 置顶 / 改名 / 复制路径 / 访达 / 断开全部 / 移除项目。
- 项目行悬停出「…」和「新对话」；置顶的行尾标钉子，同组排最前。
- 置顶、移除、改名都存 localStorage；移除只是不显示，会话留盘，再建或在那开新对话就回来。
- 右键菜单抽成通用 ContextMenu；删除 / 移除确认共用 Confirm。
- 侧栏其余地方不再弹 WebKit 原生菜单。
- 实测（AX）：项目右键出菜单、置顶后菜单变「取消置顶」；已恢复原状。
- 工作台标题改「Dock · 工作站」，去掉两行说明。
- 修：拖手拖不出独立窗口。原先靠 WebView 的 pointerup，光标出窗口后收不到，按下还变成选字。
- 现在按下即调 Rust pop_out_drag：轮询系统鼠标左键（CoreGraphics），松手在窗口外就在光标处开窗。
- 不拖也能拆：格子头栏「在新窗口打开」、侧栏右键「在新窗口打开」。
- Wonder 稿已同步：ber 加「在新窗口打开」，map 标题改「Dock · 工作站」、去掉说明。
- 侧栏「预设 · 项目」+：创建项目（设计 pdf / yeo）。选预设 + 原生选文件夹，可起名。
- 新增依赖 tauri-plugin-dialog（Rust + npm，capability dialog:allow-open）。
- 项目存 localStorage（dock-gui.projects）：没会话也留在树里，点「新对话」预填预设和目录。
- 会话行加 select-none：右键不再顺手选中一个字。
- 实测：右键菜单、点开即连显示模型 / 推理、+ 弹框、原生选目录框都起来了。
- 侧栏会话右键菜单（设计 ber，Wonder 原无稿，照 dubs 画）：打开 / 分屏 / 单独面板 / 改名 / 复制 ID / 访达 / 断开 / 删除。
- 改名在侧栏行内；删除走同一个确认框；「断开」只给开着且说过话的会话。
- 修：点开即连后撞 Dock「最多 9 页」。开页前满了就关最久没看的一页（makeRoom）。
- makeRoom 只挑说过话、没在跑、没在等、没在看的；没拉过记录的先拉一次确认没在跑。
- 点开关着的会话直接开页（store.select → ensureOpen），一打开就有模型 / 推理强度。
- Dock 侧：meta.json 记模型、推理强度，开页时切回；切换后立刻写盘。
- 输入框浮起（ComposerFloating）：内容短时跟着内容浮，内容长时钉底。
- 做法是 Transcript 不再 flex-1，改成可收缩；空态 / 骨架照旧占满。
- 工作台（pages/Workbenches）：「单独面板」把会话拆成网格，每格一套会话面板。
- 每格左上角拖手拖到窗口外，松手开一扇独立聊天窗（ChatWindow）。
- 独立窗口是另一个 WebView：自己要一张 ticket 连一份 WS，连的是同一个 Dock。
- 两边订阅同一批线程，网关各推一份事件，状态自然对齐，不用改协议。
- Rust 侧新增 open_chat_window（label chat-<id>，已开只聚焦，落点夹在屏幕内）。
- capabilities 的 windows 加了 chat-*，新窗口才有 invoke 权限。
- 实测：拆出 → 拖出 → 独立窗口历史完整、输入框可用。

## 2026-09-25

- AI 起草接进 GUI：新建框描述 → 生成中 → 失败 / 草稿进编辑器（6 张画板）。
- 编辑器支持没 id 的 AI 草稿：保存时先 preset/create 再 preset/update。
- 草稿标记：页头「AI 起草」、侧栏「AI 生成」、提示条 +「重新生成」、工具行 AI 理由。
- 提示词「AI 润色 / AI 扩写」：左右对比，接受后可「撤销改写」。
- 工具「按描述推荐」：逐项采纳 / 忽略、全部接受；侧栏显示「N 待确认」。
- 和设计的取舍：不画假进度 / 匹配度 / 分析段（Dock 没这些数据）。
- 实测真模型（GUI）：起草→润色→推荐→保存成 custom-2，盘上对；测试预设已删。
- wonder-ui 按功能分目录：shell / chat / preset / tools / pages。
- Dock #149：preset/draft、preset/rewrite、preset/suggestTools（AI 起草，不写盘）。
- 这三个方法另起任务回帧：真模型起草 6.6s 期间 preset/list 1ms 回。
- 真模型探针：「只读审代码」只挑读类工具、图标 eye、提示词四段齐。
- GUI 已备：model.ts AiPresetDraft；store.draftPreset / rewritePersona / suggestTools。
- 待设计稿：生成入口、生成中、「AI 生成」标记、润色对比、推荐确认、失败态。
- 待做：编辑器要支持「还没 id 的草稿」，保存时先 preset/create 再 preset/update。
- 预设编辑器（10 张画板）：基本信息、提示词、工具、子代理、内置、未保存、校验、损坏。
- 入口：设置「预设管理」每行「编辑」；新建预设后直接进编辑器。
- 工具目录按类分组、中文简介；全部 / 自选 / 不用三态；未挂载标红。
- 提示词编辑：行号、标题着色、Markdown 预览、字符数。
- Dock #148：preset/get、preset/update、tool/catalog。
- 预览：preview.html#preset-*（11 个状态），假数据，无头 Chrome 截图对过。
- 实测协议：独立 dock serve + 临时 DOCK_HOME，走 GUI 同形请求，写盘正确。
- 实测 GUI（新 Dock）：新建→进编辑器→改提示词 / 去掉浏览器组→加子代理→保存，盘上文件对。
- 实测：清空名字报错并挡住保存；未保存弹窗三个按钮；内置「编码」提示条与只读；Esc 只关编辑器。
- 修：自选里清空全部不再跳成「不用工具」（picking 状态，见 util.modeOf）。
- 修：子代理 id 框关掉自动大写。
- 修：非组件导出挪到 preset/util.ts，改代码不再整页重挂（Fast Refresh）。
- 收尾：测试预设已删，新对话默认预设设回「编码」。
- Dock #147：线程级方法对关着的会话按需开页（发消息 / 订阅 / 改设置）；停止、队列回空；GUI 的 ensureOpen 仍留着，用来重拉状态。
- 修：关着的会话（历史、Dock 重启后）改模型/权限/思考、答卡片报 thread_not_open → store.ensureOpen 先开页；停止/撤回排队对关着的页不报错。
- 新建自定义预设（CustomPresetEditor / bos）：起名、图标库 24 个、以某预设为底；存 ~/.dock/presets。
- 入口：新对话页「新建预设」、侧栏「Agent 预设」、品牌行菜单；建好设为新对话默认。
- 设置「预设管理」：列出全部预设；自定义的可删，改过的内置预设是「恢复内置」。
- 图标：agent.yml 的 icon → presetIcon 注册表，侧栏 / 窄栏 / 会话头都认。
- Dock #146：preset/create、preset/delete、preset/list 带 icon / builtin。
- 实测：新建 → 写盘（icon: rocket，人设工具照「编码」）→ 设置里删除 → 目录没了。
- 分屏（SplitPanels）：⌘ 点侧栏 / 搜索结果在右侧打开；各自会话头、对话流、输入框。
- 分屏竖线可拖（30%–70%，双击复原，记住比例）；「关闭面板」关左边时右边转正。
- 分屏时两个会话都算「正在看」（store.focus 收一组 id）；连接横条只画左边。
- 输入框用 @container：窄了收起审批文字、「推理」前缀，模型名截短。
- 设置页（照 reis 全部分组）：接了 MCP 服务、模型与密钥（只读）、关于；其余即将推出。
- 入口：侧栏 / 窄栏底部齿轮、⌘,；盖在最上层，底下会话和终端不卸载。
- Dock #145：mcp/list、mcp/reload（原来是空壳）、mcp/reconnect、model/list。
- 实测 MCP：cua-driver 已连接 56 个工具；重连、重载配置、经 use_tool 调 list_windows。
- 工具卡：search_tool 列找到的工具；use_tool 行头写「服务器 动作」，不出现 use_tool。
- 快捷面板对齐 ShortcutsPane（未做的入口文字不再发灰）。
- 工具卡按真实输出：Read 补行号 + 截断说明、grep 各模式、Todo 清单、Ask 问答、Fetch 状态码。
- preview.html 加 #tools / #menu / #rail / #search；ToolCard 加 initialOpen（预览用）。
- 真模型实测（Atria）：在等你、排队、失败 / 中断 / 拒绝工具、改名、删除、重连都过。
- 错误框加「重试」：重发出错那一轮的用户消息（只给最后一轮）。
- 修：重连时过期的订阅把连接拖死（一直「正在连接」）。
- 修：删除吞掉 not_found；空会话关掉即删。
- Dock #142：关着的会话在别的目录也能改名、删除。
- Dock #143：工具被拒报 denied；GUI 画「已拒绝」。
- Dock #144：thread/search；GUI 搜索带消息片段，中文可搜。
- 访达：opener 放行 open-path，菜单改回「打开项目目录」。
- 侧栏会话行：在等你（橙）/ 在跑 / 刚跑完（已完成·出错）/ 开着。
- 会话区顶部连接横条：连接中 / 重连第 N 次 / Dock 已退出 + 重新启动。
- Tauri 命令 dock_restart 已装；实测杀掉 Dock → 红条 → 重新启动 → 重连上。
- 载入历史时画骨架，晚 150ms 淡入。
- 工具行通用状态：运行中用时、执行失败、已中断。
- 排队中的消息：turn/queue/list 拉取，可取消（turn/queue/remove）。
- 窄栏加铃铛和搜索；Popover 加 side=right。
- 会话头「更多」：改名（也可双击标题）、复制 ID、访达显示、删除确认。
- 侧栏搜索：按标题，按预设分组。
- 提问 / 计划 / MCP 确认卡照 Gemini 稿重做（Pending.tsx）。
- 加 preview.html：假数据摆组件状态，无头 Chrome 截图对照。
- 架构（用户批）：3 工具错误标记、4 关着的会话回放、1 共享核心、2 store。
- Dock #131 turn/completed 带 status / error。
- Dock #132 工具结果 is_error 落盘，tool_completed 报 failed/cancelled。
- Dock #133 关着的会话 history 也回放 events；重开会话不再挂着一轮。
- Dock #134 新增 dock-core（类型化事件 + 线程 reducer，9 个测试）。
- GUI 改用 dock-core：删 project.ts、looksFailed、stopping 猜测。
- GUI 状态层改 DockStore + useSyncExternalStore。
- GUI 新增 Pending.tsx：权限、提问、计划、elicitation 都能处理。
- 实测：关着的会话回放出工具卡；旧数据按兜底认出「已中断」。
- 实测：Atria 实跑，权限卡、允许记录、停止后「已停止」都来自 Dock。
- 坑：vite 需放行 ../dock-core，否则 webview 黑屏。
- 修：红绿灯与左上角图标行对齐（中心 y=22，占位 62px）。
- Dock #136 一轮结果落盘（turn-end 行），关着的会话也见停止 / 出错。
- Dock #137 dock-core 进 CI；#138 模型名写错直说。
- 铃铛：点一下跳到下一个在等你的会话，提示写数量。
- 提问卡：数字键选项、Enter 提交、出现时拿焦点（打字中不抢）。
- 实测：Atria 提问 → 点选 → 提交 → 回复，通过。
- 未验：键盘路径（后台注入的按键只进原焦点）。
- Dock #140 提问支持多选（multiSelect / values + other）。
- 提问卡：多选可勾多个再补一句；实测 Atria 复述正确。
- 设计：通知菜单 quag / 空状态 wiss（用户确认：刚完成算通知）。
- 通知菜单：在等你 + 刚完成，点一条跳过去，看过移出。
- Popover 抽成通用外壳，MenuButton 建在它上面。
- 实测（AX）：等权限 → 行出现 → 点击跳转 → 批准 → 后台完成 → 刚完成 → 点开移出 → 空状态。
- 未验：外观截图（窗口被遮挡时 webview 不重绘，截图是旧帧）。
- Dock #141 没有对话内容的会话不落盘。

## 2026-09-24

- 实测通过：工具卡展开、运行中计时、停止、排队、用时折叠。
- 实测通过：会话中切模型（Atria），协议层 probe 也验证了。
- 修：事件 timestamp 是毫秒数字串，之前按 ISO 解析失败。
- 停止的轮次标「已停止」；被打断的工具卡标「已中断」。
- 空轮（无任何产出）提示可能出错。
- 缺口：dock.1 不回报轮次错误，GUI 看不到失败原因。
- 未解：dock-gui 会话里 Atria 一轮 3s 空结束，原因待查。
- 阶段 1c 完成：底部终端接真 shell（src-tauri/src/pty.rs）。
- 依赖：portable-pty；@xterm/xterm、@xterm/addon-fit。
- 终端：多标签、收起不断 shell、exit 关标签、拖顶边改高。
- 字体优先 Liga SFMono Nerd Font，提示符图标能画出来。
- 实测：输入输出、新标签、exit 只关自己、无孤儿 shell。
- 对话流改 Codex 式（用户截图）：气泡、日期行、用时折叠。
- 用户消息时间取事件 timestamp；关着的会话历史没有时间。
- 模型配置挪到 ~/.dock/config.toml（备份在旁）。
- GUI 从中性目录启动也能看到两个模型。
- 设计调整（用户确认）：见 wonder-ui/README.md。
- 新增 scripts/wonder-save.py：导出结果存档。
- UI 对齐：侧栏 / 会话头 / 对话流 / 工具卡 / 权限条 / 输入框。
- 动效：进场、折叠、弹出、脉冲、按下缩放（MOTION.md）。
- 输入框：审批 / 模型 / 推理下拉，运行中停止键。
- 运行中发送改为 turn/enqueue（排队）。
- 新对话默认预设：GUI 本地记住（localStorage）。
- Markdown：新增依赖 react-markdown、remark-gfm。

- 设计稿源码按组件导出到 wonder-ui/。
- 新增 docs/MOTION.md、docs/DESIGN-MAP.md。
- 模型下拉：按会话切换（thread/model/set）。
- 工具失败：按输出猜（同 TUI looks_failed）。
- Dock 启动目录改为 App 数据目录。
- 原因：在 $HOME 启动会把 ~/.dock 当项目层。
- 开发开关 DOCK_GUI_CWD：指定 Dock 启动目录。
- 真实模型实测通过：工具卡、权限条、换模型。
- 发现并修复 Dock 回复重复、一轮被切碎（#130）。
- 阶段 1a：Tauri 拉起 dock serve，WS 接入。
- 阶段 1b：会话树 / 新对话 / 对话流接真数据。
- mock.ts 已删除。

## 待办

- 按 DESIGN-MAP.md「已知差距」继续对齐。
- 窄栏：通知铃铛未放（待定）。
- 快捷键面板、其它工具的输出样式。
