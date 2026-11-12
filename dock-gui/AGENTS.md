# AGENTS.md（dock-gui）

根 `AGENTS.md` 仍然适用；本文件只补 `dock-gui/` 的约束。

## 现状

- 已接真 Dock（`dock serve` + dock.1）。
- 进度看 `docs/PROGRESS.md`。

## 设计

- 外观以 `wonder-ui/` 为准（按组件分目录）。
- `wonder-ui/` 只读：不 import、不手改。
- 对齐情况记在 `docs/DESIGN-MAP.md`。
- 动效按 `docs/MOTION.md`，不要生硬切换。
- 玻璃材质只写在 styles.css 的 `[data-material="glass"]` 下；平面模式必须与设计稿一致。
- 主窗口和拖出的独立窗都是透明窗口（原生玻璃用）：平面靠 body / `.glass-window` 的不透底；新的全窗覆盖层要自带不透背景。
- 独立窗的最外层也挂 glass-window（原生玻璃时放透明）；偏好靠 storage 事件跨窗口同步。
- 原生材质只在 `[data-native-glass]` 下放透明（prefs.ts 的 syncNativeGlass 成功才挂）。
- 玻璃层的白色透明度都写成 `calc(var(--glass-alpha) * …)`（设置里的透明度滑块）；浮层、对话框、输入框用固定值。
- 玻璃类：glass-sidebar / glass-backdrop / glass-bar（原生时透明）、glass-pane（阅读面，白 42%；里面的块用 md-surface / md-surface-head / md-quote / md-inline-code / glass-bubble，不加模糊）、glass-tile / glass-card（玻璃边）、glass-popover / glass-dialog（带模糊）。
- 带 backdrop-filter 的元素里不要放 position: fixed 的后代（会被它当包含块，位置跑偏）。
- glass-pane / glass-sidebar 里手写的浅色底按色系统一换玻璃（styles.css「玻璃面…手写的浅色底」）。
- 新用的浅色底色值，加进那里对应的色系；不用组件里一个个写。
- 浮着的要带 absolute / fixed；要保持不透的（开关圆钮、PDF 页、表格、画布）加 glass-keep。
- 新面板 / 整页的根节点带 glass-pane；盖在会话区上的整页再加 glass-page（藏起后面的兄弟）。
- 折起的长内容别用白渐变盖：加 glass-fold（正文淡出）+ glass-fade（去掉渐变）。
- 主窗口在 lib.rs 的 setup 里按 tauri.conf 的 main 配置建（"create": false）：新开窗口都要带 webkit::configuration，否则掉回 60 帧。
- enter-fade / enter-up / pop-in 不和 animate-* 写在同一元素：都占 animation，进场类不在 layer 里会赢。
- 未做的入口保留外观，提示「即将推出」。
- 红绿灯中心在 y=22（`tauri.conf.json` y=24），左上角那一行也要居中在 22。
- 红绿灯占位 62px（`chrome.tsx` 的 TrafficLights，真窗口和浏览器预览各一处）。
- 窄栏的窗口按钮由 App 固定画（FloatingWindowControls）；头栏里只放透明占位，别再画一份。
- 对话流和输入区同一列宽：Transcript.tsx 的 READING_WIDTH / COMPOSER_WIDTH。
- 左边消息导航在 TurnNav.tsx：按轮次根节点的 data-nav-turn 找位置，别删这个属性。
- 回复渲染在 Markdown.tsx + components/markdown/（Shiki 高亮、Mermaid、KaTeX）。
- Shiki / Mermaid 第一次用到才 import：别在启动路径上静态引入。
- 对话字体 / 代码字体 / 高亮配色是 GUI 偏好（prefs.ts），走 CSS 变量 --font-chat / --font-mono。
- 回复里路径可点要有 PathLinkContext（Transcript / 子代理面板给）；没给就是普通行内代码。
- 工具卡画法在 components/tools/specs.tsx：按 Dock 实际输出写，认不出就退回原文，别丢信息。
- 新工具卡先录真实输出（docs/TOOL-CARDS.md「重跑实测」），样本放 src/preview/fixtures/。
- MCP App 界面只经 AppFrame（官方 AppBridge）：server 由宿主按实例定死，不信 iframe 传来的。
- MCP App iframe 只给 allow-scripts；CSP 用 dock-core appCsp，权限只照给写剪贴板。
- 桥要先 connect 再给 srcDoc：界面一加载就发 ui/initialize。
- 测试脚本的 addInitScript 会进所有 frame：只在 window.top 里塞假桥。

## 功能目录

- 新功能放 `src/features/<name>/`，在自己的 index.ts 往 `features/registry.ts` 登记。
- 宿主（App、Composer、ToolCard、设置页…）只调登记表，不 import 功能目录。
- 加一种扩展：registry 加一张表 + 宿主加一个插槽。
- 工具卡插槽：useIcon（图标）、Body（展开后的正文）、Attachment（标题行下、不用展开）、isArtifact（不随过程折起）、keepMounted（跑着时也不挪，只给含 iframe 的）。
- 轮次脚注插槽（registerTurnFooter）：一轮结束后、回复操作行上面；每轮改动卡用它（features/turn-changes）。
- 登记都在启动时完成：工具卡的 `useIcon` 是 hook，宿主按固定顺序调。
- 老组件碰到再迁，不搬家式重构。
- 登记的面板 `poppable: true` 才能弹出：Rust 侧 open_panel_window 也要认那种 kind。
- `perSession: true` 的面板拖出后跟着会话（URL 带 session）；MCP App 面板就是这种。
- 跨窗口状态别只靠关窗那扇自己广播（pagehide 常发不出去）：听 Rust 的 `window/destroyed`。

## 与 Dock 的边界

- 独立工程：不进根 `Cargo.toml` members。
- 不 path-dep `cordis-*` / `dock-*` crate。
- 唯一接口是 dock.1（`src/dock/client.ts`）。
- 事件语义只用 `dock-core`（`file:../dock-core`，源码软链）。
- 不在 GUI 里解析事件、推导轮次或猜工具成败。
- 缺什么语义，先改网关投影 + dock-core，再改 GUI。
- 不复用 embed-sdk 的模型：它脱敏、截断，丢原始参数。
- Gateway 只连 loopback，不放宽鉴权。

## 状态层

- `src/dock/store.ts`：`DockStore` 在 React 外。
- 本机加每台打开的远程各一份 `DockClient` / `DockStore`（`src/dock/gateways.ts`）。`store` / `dock` 是前台代理。切走不关套接字、不拆别的隧道、不重载页面；本机 Dock 跟 GUI 同寿命。
- 组件用 `useApp` / `useThread(id)` / `useEnv(id)` 订阅。
- 流式增量只重渲染订阅那个会话的组件。
- 会话级集合（running / waiting）只在翻转时换新。
- 开页后网关 seq 重编：旧状态丢掉整份重拉。
- 断线重连：已加载的会话整份重拉。
- vite `server.fs.allow` 要含 `../dock-core`，否则开发时白屏。
- 「刚完成」只记实时推来的结束；`store.focus` 标记正在看的会话。
- 排队消息没有推送：一轮开始 / 结束时重拉 turn/queue/list。
- 删会话：开着的先 thread/close；空会话关掉即删（不落盘），不再调 delete。
- 点开会话即开页；Dock 最多 9 页，满了 store.makeRoom 关最久没看的（别关在跑的）。
- 项目在 Dock 里不是实体：侧栏 + 建的空项目只存 GUI 的 localStorage。
- 原生选文件夹面板是系统进程画的：cua 拿不到树，后台发不进键。
- 拖到窗口外别靠 WebView 的 pointer 事件：交给 Rust pop_out_drag 问系统鼠标状态。
- 在跑时三种发法：turn/enqueue 排队、turn/queue/steer 插话（步骤边界并进这一轮）、turn/start 停止并发送。
- 插话要看网关能力 `turnSteer`；旧版 Dock 的 turn/steer 是停掉再发。
- `turn/completed { steered }` 不是真的结束：别通知、别算刚完成。
- 用户消息的图：事件带尺寸 + 内容摘要 `digest`，像素用 `item/image { threadId, digest }` 取；本窗口刚发的用本地预览更快。历史里的图 Dock 侧按摘要存着（`$DOCK_HOME/user-images/`），刷新后照样能显示。
- 改 src-tauri 会让 tauri dev 重启整个 App：用户在用时先别改。
- 重连时订不上的旧订阅要丢掉，不能让它把连接拖死（client.ts open）。
- 删预设前看 builtin：改过的内置预设删掉是恢复内置，文案要说清楚。
- 预设编辑器整份写回（preset/update）；内置自带的子代理删不掉。
- 预设编辑器盖在设置页上：Esc 要在捕获阶段拦，否则先关掉设置页。
- 工具中文简介在 preset/toolGroups.ts；Dock 给的是英文。
- AI 起草 / 润色 / 推荐要等几秒：不要假进度，只显示真实用时。
- AI 结果只是建议：必须用户点接受 / 采纳 / 保存才落到草稿或盘上。
- 设计稿按功能放在 wonder-ui/{shell,chat,preset,tools,pages}。
- 设置页写 Dock 配置走 dock.1 设置方法（dock/settingsApi.ts），只有 GUI 的受信 ticket 能调。
- 密钥只进不出：表单留空 = 不改，界面永远拿不到原值。
- GUI 自己的偏好（常规 / 外观 / 快捷键）在 dock/prefs.ts，不进 config.toml。
- 全局快捷键只从 dock/shortcuts.ts 取，别在组件里写死键位。
- 模型按供应商管（settings/Providers.tsx）：目录只用来填表，开了的模型以 config 为准。
- 供应商目录快照 src/dock/providerCatalog.snapshot.json 别手改：重跑 scripts/update-provider-catalog.mjs。
- 目录字段白名单和 Dock 的 llm/http/catalog.rs 保持一致。
- 去设置某一页用 `dock:open-settings` 事件（ProfileMenu 就这么做），别往侧栏加回调。
- 用户名 / 头像是 GUI 偏好（prefs.profileName / avatar），用 Avatar 组件和 displayName 取。
- 侧栏会话下面的子代理行在 SubagentRows.tsx（Sidebar 只换了这一处调用）；整页在 SubagentPage.tsx，对话 / 输入框复用 SubagentPanel 的。
- 给结束了的子代理发消息 = 复活：store.sendToSubagent 会先开页，别在组件里自己开。
- 设置页分组一个文件：components/settings/<分组>.tsx；预览 preview.html#settings-<分组>。
- 实测：cua 前台打字才进得了 WKWebView；坐标用 AX frame 中心或元素 token。
- dock-core 链到仓库 checkout：Dock 切分支会影响 GUI 类型检查。
- 浏览器面板只走 dock.1 browser/view/*：不直连 CDP（WKWebView 没有，远程也连不到）。
- browser/view/open 连发时网关留最后收到的那个，顶掉的推 closed(replaced)。
- browser/view/input 网关按收到顺序处理（#158 起）；GUI 仍串行发，兼容旧网关。
- 首帧可能早于 open 的回包：推送按 threadId 收，不要只认 viewId。
- 文件面板只走 dock.1 fs/*：不读本机磁盘（远程 Dock 的文件不在本机）。
- 面板能改文件（能力 workspaceEdit）：调用都在 dock/fsEdit.ts，路径相对会话 cwd。
- 保存已有文件总带 baseModifiedMs：被 agent 改过回 conflict，让人选覆盖还是放弃。
- 有没保存的修改时，换文件 / 关面板 / 动它之前都要经 guarded 问一句。
- 树上拖动用 pointer 事件（files/TreeOps.tsx），别换成 HTML5 拖放（Tauri 会接管）。
- 表格（csv / xlsx）在预览里改：预览器登记 editable，宿主给 edit（保存、冲突、没保存的提示都是宿主的）。
- 预览器的 key 只在文件被别人改了 / 放弃修改时换：自己存的不重来，预览器的加载 effect 别依赖 size / 修改时间。
- xlsx 只改被改的格子的 XML（xlsxPatch.ts），别用 SheetJS 整份写回：社区版会丢样式、图表。
- 表格格子上盖着透明 textarea 拿焦点：div 拿焦点时中文输入法组不了字。
- 大文件按字节存走 saveBytes：分块 fs/upload，提交带 size。
- 只在 Worker 里或懒加载才用的依赖要进 vite.config.ts 的 optimizeDeps.include：不然开发时第一次用到会整页重载。
- 表格 / 文档预览在 features/file-preview，经 registerFilePreview 登记，宿主只调登记表。
- 大文件走 fs/read { raw } 分块（stream.ts）：首块小、后面加大、两个请求并行。
- 解析放 Worker（csv / xlsx），主线程不拼整份字节；行分批发、表格虚拟滚动。
- 预览器 / 编辑器别用 React.lazy + Suspense：React 19 显示 Suspense 内容会节流约 300 ms（用 features/deferred.tsx）。
- Word / PowerPoint 在沙箱 iframe（只 allow-scripts）+ CSP（nonce 脚本、不联网）里画：见 file-preview/frame.ts。
- PPTX 库预先打成 IIFE（scripts/bundle-frame-libs.mjs → file-preview/vendor/），升级后重跑。
- PDF 在主窗口画（pdf.js，isEvalSupported 关）：大文件按需取段，字体映射用 glob 成资源。
- 文本编辑器是 CodeMirror（features/file-edit，registerFileEditor）；看文件仍走只读视图。
- vite dev 第一次用到新依赖会整页重载：无头测之前先预热一遍。
- 无头测的 addInitScript 也在 iframe 里跑：碰 localStorage 前先判 window.top。
- 浏览器 / 文件 / 桌面 / 画布在右侧排成一排（App 的 panes），每种最多一个；开着时不画快捷面板。
- 面板一排是兄弟节点、按种类做 key：别改成嵌套，否则加一栏会重挂别的面板（画布 iframe 重载）。
- 面板头栏控件在 PanelChrome.tsx（拖手 / 分屏 / 新窗口）；面板只留 lead / controls 插槽。
- 拖出的面板窗：`index.html?session=<id>&panel=<种类>`（PanelWindow），label `panel-*` 要在 capabilities 里。
- 关窗用 `getCurrentWindow().close()`（要 core:window:allow-close）：WKWebView 里 `window.close()` 关不掉 Tauri 开的窗。
- 画布 iframe 只开 allow-scripts：别加 allow-same-origin（同源就能碰到 Tauri invoke）。
- 画布报错行号靠桥脚本不换行：改 canvasDoc.ts 的桥别加换行，库别内联。
- 画布刷新看对话流 canvas_* 工具项（网关不推 canvas 通知）；工具输出格式见 canvasDoc.ts 的正则。
- 认工具项要用 toolCall.ts 的 calledTool：MCP 工具平时经 use_tool 调，本名在 tool_name 里。
- 写文件的命令只有 save_text_file（Rust 侧弹存储对话框）：别加让前端指定路径的写盘命令。
- 真机测试：cua-driver 后台点击走 AX，不产生 pointer 事件；点浏览器画面要 delivery_mode foreground。
- 桌面面板实时画面走 desktop/view/*（网关经 cua-driver 只截图）；老网关只有 agent 截的图。
- 实时画面看 agent 最近操作的窗口，没有就看最前面的窗口；点击圈按 agent 截图的 size= 换算。
- 网关有 desktopControl 时：desktop/view/windows 列窗口，pin 钉住（空闲不追前台），input 把点击、滚动、打字送回那一帧的窗口。打字走画面下的输入条，按上次点击的位置送出。画面铺满面板。agent 在跑时先接手。
- 浏览器面板视口跟着面板走（browser/view/resize，拖分隔线时等松手）；空闲直接可操作，agent 跑时要「接手」。
- 浏览器面板没标签页时地址栏能开页（browser/view/open 带 url，经浏览器 MCP，和 agent 共用）。

## 文档纪律

- 每完成一步，在 `docs/PROGRESS.md` 追加一行。
- 对齐一个组件，改 `docs/DESIGN-MAP.md` 一行。
- 一行只写一件事，行要短（便于小范围替换）。

## 检查

- 前端：`npx tsc --noEmit`、`npm run build`。
- 看样子：`preview.html`（假数据，不连 Dock）+ 无头 Chrome `--screenshot`。
- 用户在用 GUI 窗口时别动它；截图走 preview。
- 协议核心：`cd ../dock-core && npm test`。
- Rust：在 `src-tauri/` 下 `cargo build`。
- 根目录的 fmt / clippy 门禁不覆盖本目录。
