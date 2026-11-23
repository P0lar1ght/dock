# Changelog

本文件记录用户可见的变化。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号用[语义化版本](https://semver.org/lang/zh-CN/)。

0.x 期间不承诺 `config.toml` 的键与 `dock.1` 协议的向后兼容：破坏性变更会写进对应版本，
并在升级说明里给出改法。

## [Unreleased]

## [0.3.0] - 2026-11-23

### 变更

- **写插件改成「插件就是一个目录」，对齐 DSH 的 Plugin Manager**：模型侧只剩两颗工具——只读的 `cordis_inspect` 和管理插件目录的 `cordis_plugin`（`reload` / `disable`）。`cordis_define` / `cordis_run` / `cordis_call` / `cordis_stop` / `cordis_promote` 退役。本机 13 个用过这组工具的会话里，一次修改要 define → run → call 三次调用、来回 8–10 轮；插件 id 跟着分裂（`cyberspace` → `cyber-2`，磁盘上留下项目级和用户级两份）；define 接受的路径 run 又拒绝。DSH 撤掉同一套工具的理由是「生成代码工具在普通已安装组合包之外引入了第二套插件生命周期」。
  - 流程：用文件工具写 `.dock/plugins/<id>/source.rhai`（`plugin.toml` 可选）→ `cordis_plugin reload <id>` → 用 `use_tool` 调新工具验证。改了文件就再 reload。
  - reload 把那一刻的文件拍成快照跑起来；**新版跑不起来时自动回到上一版继续跑**，用户手上的工具不会因为改坏一行一起消失。回执列出这一版登记的工具 / 斜杠 / 面板 / 钩子，最后一行告诉模型下一步怎么验证或怎么修。
  - 确认按插件记：「始终允许」只放行这一个插件，迭代它不用每次点；确认框列出它挂的钩子和声明的 HTTP host。批的就是这些，所以插件多挂一个钩子、多声明一个 HTTP host（之后对它免逐次确认）会再问一次，只改代码照旧免问。
  - **项目自带的插件第一次加载前要你点头**：克隆来的仓库里的 `.dock/plugins/<id>/` 能改流程、免确认出网、花模型额度、叫醒会话，以前打开项目就直接跑，同仓库的 `.dock/hooks.json` 却要先点头。现在和项目 hooks 一样，在打开它的那一页上问（列出插件目录、钩子、HTTP host；自动批准不算数），按「插件目录 + 文件内容指纹」记在 `$DOCK_HOME/plugins-trust.json`，下次打开不再问，文件一改（比如 `git pull`）就重新问；拒了的这次不再问，设置页里标着「待确认」，点「立即加载」就是同意。你让模型 `cordis_plugin reload` 过、或在设置页加载 / 启用过的那一版算点过头。用户目录 `~/.dock/plugins/` 的不问。
  - `plugin.toml` 每个键都可选（没有这个文件也行，名字取目录名），新增 `[permissions] http = ["api.example.com", "*.example.com"]`：声明过的 host 不再逐次确认。认不得的键 reload 时会提醒。
  - 创造模式里这两颗工具常驻（以前要先 `search_tool` 再 `use_tool`），人设改成「先交付能跑的小版本、用真能力验证」。
  - 网关去掉 `plugin/promote`、`plugin/discard` 和 `plugin/list` 的 `session` 字段；设置页去掉「动态插件」一栏，已装载的插件多一个「重新加载」按钮。
- **`/code-review` 精简成「一个审查员 + 一个核实员」，快很多**：以前一次审查起 8–11 个子代理（范围、五个角度的审查员、去重、核实），五个审查员各自把同一份 diff 翻页重读。本机记录里一次 19 个文件、两百来行的 PR 跑了 108 分钟（一个审查员跑了 111 次 `git diff` / `git show`），跑得最久的审查员还交不回结果，那个角度直接没审到。
  - 现在默认「标准」档：一个审查员自己定范围、把改动打一次读一遍、按一份准则（正确性、安全、性能、测试、项目规约）审，再由一个核实员一次核完。「快速」档（`/code-review 快速` 或 `level: "quick"`）只审不核实。
  - 报告多一行档位和「整体判断」（能不能合）；核实员没交回结果时发现照列、标「未核实」，不再整份丢掉。
  - `angles` 不再按角度拆审查员，原文并进关注点。结构化结果（`data`）的字段不变，`angles` 里现在是类别（如「安全」），界面批注照常画。
- **一轮不再有 256 步上限，改成检测原地打转**：以前一轮最多采样 256 步（每次 turn-end 续跑各算一段），长任务做到一半就被截断，滚动区留一句「已停下」，用户只能再发一句「继续」。现在主会话和子代理都不设步数上限（grok-build 默认不限，Codex 也不设）。兜底改成连续相同步检测：模型连着几步发出完全相同的工具调用（参数键的顺序、同一步里几颗调用的顺序不算变化），先提醒一次，还不改就停下这一轮。
  - 一步里全是 `read_file` / `todo_write`：第 4 次提醒、第 8 次停；别的工具第 8 次提醒、第 12 次停；只有一颗 `bash true` 的空转第 4 次直接停。模型回了正文就重新数。
  - 停下记成失败，滚动区留一条 `◆ 已停下` 的说明（不进模型历史），目标（goal）不会自动接着跑；子代理交给父级的是失败、带这条说明。A、B 两步来回切换的空转抓不到：主会话可以 Esc，子代理可以在任务面板里停。
  - **落盘**：`chat_history.jsonl` 的 `llm` 行多一个可选键 `stopped_by_loop`，只在循环自己停下这一轮时写 `true`（上面的原地打转，以及「修复」里连续 3 次只给推理）。旧会话没有它，照常读；用更早的版本打开含它的会话，这个键被忽略，标题照旧是「请求失败」。
  - 插件作者注意：`agent/turn-end` 载荷的 `ended_with_text` 现在恒为 `true`（「步数耗尽」那条 `false` 路径没了），字段保留。Rust 侧 `Error` 新增 `Stationary { tool, run }`：对 `Error` 做穷尽 `match`、没有通配分支的代码要补一个分支（`Error` 没有 `#[non_exhaustive]`）。`Error::MaxSteps` 不再产生，保留只是让已经写了这个分支的代码还能编过。
- **后台任务跑完会自动告诉模型**：以前后台命令（`is_background`、前台等到点转入后台的、`monitor`）只能靠模型拿 `job` 加长等待去收结果，这一轮挂在那里干等，用户插话也进不来，等到点还常常没跑完。现在任务一结束，就把退出码、跑了多久和输出结尾（至多 2KB，完整输出指向文件或 `job`）送到起它的那一页：那一页在跑就在下一步带上，闲着就自动开一轮接着处理，不用你再发话。
  - 模型已经用 `job` 读到结果、或者自己用 `kill_task` 停掉的，不再通知；在任务面板里被停掉的会告诉模型，但不为它开一轮。子代理起的任务只告诉那个子代理。一秒内前后脚跑完的几条合成一次。
  - `job` 的等待遇到插话会在 2 秒后先交回（和前台 bash 一样），按停止立刻交回；工具描述改成「跑完会通知你，不用干等」。
  - 插件可以听新事件 `job/finished`（载荷 `JobFinished`）和 `job/reported`。
- **`search_tool` 少占上下文**：搜索结果留在历史里、之后每轮重发。实测一次会话里模型为找常驻的 `job` 连搜六次，结果共 47KB（约 1.2 万 token），大半是不相干的命中。
  - 查询点名了已经在工具表里的工具（如 `job`、`bash`，或照 MCP 公名猜的 `jobs__job`）时，直接告诉模型「已经有了，直接调」——按需目录本来就搜不到常驻工具。查询只有常驻工具名时只回这一句；精确点名的隐藏工具（如同名的 `mcp_fs__read_file`）照旧回它的 schema。
  - 分数不到头名一半的命中不再发；输出改成紧凑 JSON、去掉分数；schema 已在上下文里的命中只剩名字（以前还带一段描述）。按那次会话估算，同样的六次搜索从 47KB 降到 7KB 左右。
  - 输出预算从 20KB（缩进 JSON）换算成 15KB（紧凑 JSON），装得下的命中数不变。
- **ChatGPT 订阅通道的前缀缓存能命中了**：以前这条通道几乎每次请求都按全价计额度。原因是这条通道按请求头把同一会话的请求路由到同一台机器，Dock 只带了 `prompt_cache_key`（取会话 id），没带这些头，请求就随机落到别的机器上。现在跟官方客户端 Codex 一样带 `session-id` / `thread-id` / `x-client-request-id`（取会话 id），并把上游给的 `x-codex-turn-state` 在同一轮里原样带回去（新一轮不带旧的）。2026-11-21 用 gpt-6-luna 实测：后续步骤从 20 步 0 次命中变成 60 步 51 次命中，每次约占输入的 95%；用真 Dock 跑两轮 6 次请求，整段 79%（冷启动那一次照旧全价）。实测记录见 `docs/CHATGPT-SIWC.md`「前缀缓存」。
  - `DOCK_CACHE_DEBUG` 日志每次请求多一行「路由」（带了哪些会话头、有没有回传 turn-state）；订阅通道的 system 在顶层 `instructions` 里，日志以前没比它，现在也比。
  - 子代理（含工作流里的审查员、研究员）以前没有会话 id，请求既不带 `prompt_cache_key` 也不带会话头，`/code-review` 这类大头在子代理里的会话命中率被拖得很低（一次审查里审查员发了 22 次请求，按字节估算约占整段会话重发量的六成多）。现在子代理用自己的 agent id（resume 后不变）。
- **工作流的上报 / 收尾卡在模型输出途中到达时，那一步的工具调用丢了**：卡片是异步追加进会话的，模型正在流式输出时插进来，这一步的正文和工具调用就写不进记录。工具照样跑了，结果却没有对应的调用，下一步回放时被丢掉，模型既不记得自己调过、也看不到结果（实际会话里表现为审查跑完后回答「输出被截断、无法确认测试是否通过」）。现在卡片留在它到达的位置，这一步照常记全。

- **todo 回合末只记账、不再逼模型继续干活**：原来以文字收尾时还有未完成项，回合末的 todo 关卡会塞一条「Todos NOT complete — continue working…别停在这里向用户汇报」再续跑最多两轮——用户说「先讨论方案」之后模型也会被推着开工。现在改成**一步记账**：只把 `todo_write` 交给模型，提醒按实际进度改状态（做完的 `completed`、等用户的 `awaiting_user`），不开始新工作，记完这一轮就结束；每条用户消息最多一次。本轮问过用户（`ask_user_question` / `handoff_to_user`，子代理的不算）、计划模式开着、或剩下的全在等用户时不记账。目标或插件要求继续干活时，记账让路。被后台任务托底的项也照记——记账只改状态，不推模型去干活，没有理由把它从欠账里豁免出去。
  - 回合中：有进行中的项、刚跑完测试或构建命令却没更新清单时提示「做完了就标完成」（命令跑挂了不提醒——还没做完呢）；连续 10 步没动清单才提醒。清单刚变过时不再另插一条提醒——写入结果已经逐项列了状态，「清单只是记录，用户的话优先」这句纪律跟着写入走。
  - 记账那一步模型顺嘴说的话不算第二条回复：不显示、不进下一轮历史，这一轮回的仍是之前那段正文。清单自上次记账后没动过、这一轮也没干别的活，就不再记账；跳过的记账（见下）不算记过；当前预设没有 `todo_write`、上下文已到自动压缩门槛时也不记；记账时按停止，这一轮仍算完成。
  - 下一轮开头：上一轮还剩未完成项时提醒一句，用户的新消息优先，方向变了先更新或清空清单。
  - `todo_write` 同时只允许一个 `in_progress`，写出两个直接报错、清单不动；全部完成时结果写「清单已全部完成」，全部取消时写「清单已全部关闭」。工具描述不再要求「别带着未完成项结束回合」，改为要求如实记账。
- **插件作者注意（破坏性）**：回合末 todo 提醒的**第一行**从 `Todos NOT complete — continue working. Next step:`（`TODO_GATE_SENTINEL`）换成 `Todo bookkeeping only — update statuses.`（`TODO_BOOKKEEPING_SENTINEL`）。Rhai 钩子里按旧开头匹配 todo 提醒的，改成匹配新开头——哨兵排在 `<system-reminder>` 之后第一行，`session/event` 给的 80 字预览一定截得到。注意 **`agent/turn-end` 的载荷里没有提醒正文**（只有 `text` / `rounds` / `ended_with_text` / `queued_followups` / `identity` / `main`），想在回合末认出记账提醒只能从 `session/event` 的预览里认。这条提醒现在只要求记账，不再表示「这一轮会继续干活」。Rust 侧的 API 变化与改法见 `docs/tools/todo.md`「API 变化」。
- **待办新状态 `awaiting_user`（在等用户）**：TUI 待办卡用 `○`、GUI 待办卡用问号气泡图标、轨迹面板的 `todo_write` 行显示「等用户N」。它不算完成，也不算模型欠的活。**落盘**：只出现在 `chat_history.jsonl` 里 `todo_write` 的参数与结果文本中；不含它的旧会话照常读。用更早的版本打开含它的会话：旧版显示成和 pending 一样（GUI 是空心圆，TUI 是 `□`）；模型侧的清单不从落盘恢复，不受影响。

- **常驻角色（`agents/<id>.yml` 的 `keep_alive: true`）**：普通子代理跑完一轮 park idle，**空闲满 15 分钟**就被回收——`list_agents` 不再列它，`send_message` 报 `is done; message was not delivered`。观察者一类的角色要整场会话保持可继续，被悄悄回收就接不上了。写 `keep_alive: true` 的角色不参与回收清扫；内置 `warden` 的 `观` 默认开着。GUI 的角色表单里也有「常驻角色」这一项。被回收的角色还能用 `task`（`resume_from: "<agent_id>"`）从盘上的对话复活接着聊，复活后这份 `keep_alive` 仍然生效。同一页最多留 8 个常驻孩子（超出的按普通孩子回收，不封顶会一直涨）；内置角色被用户层的旧文件覆盖时，没写 `keep_alive` 就沿用内置的值，不会被悄悄抹掉。
- **子代理面板显示上下文占用**：子代理以前只占一行状态，看不出它自己的上下文用了多少、快到压缩没有。面板的详情页给了一个小圆环（过了自动压缩门槛是橙色），列表每行一个百分比，悬停写明「已用 / 窗口 · 距压缩还剩 X」。数字跟主会话同一套算法（上游 usage 锚点加锚点之后的本地估算），所以面板上的数就是它真正触发压缩的那个数；锚点取的是子代理自己的会话，读不到父会话的用量。每轮跑完算一次，占用变化不到 1% 不推（并行跑多个孩子时别把前端刷爆）。协议上 `subagent/list` 与 `subagent/updated` 的子代理项多了可选的 `context`（`{ used, window, compactAt }`），没算出来时这个键整个不出现，老客户端不受影响。TUI 的任务面板里，子代理那一行也带占用百分比（配色与主线程状态栏同一套断点，85% 是自动压缩门槛）。子代理**实际在用的模型与推理强度**也一并报上来（workflow 的 `agent(model:)` 覆写优先，否则跟父会话），放在**聊天框那一排**（跟主会话底栏同排：环 + 模型 + 推理强度），不是上方抬头 —— 主界面整页看子代理时也一眼能看到。TUI 的子代理卡片抬头也带着。
- **模型看到的目录改走消息提醒**：技能、工作流、子代理名册不再进系统提示 / `task` 描述，改在每一轮采样前以 `<system-reminder>` 注入；内容和历史里最近一份一致就只留一份，压缩后重注。切到调不动 loader 的预设时补一句「目录不再适用」。
  - MCP 工具名同样走提醒：第一次列全量，之后只发新增 / 移除；按服务器分组，单台服务器超过 12 个工具只铺前 12 个，其余用 `search_tool` 搜。内置 browser / whiteboard / cua-driver 各保留一句用途。
  - `/context` 的技能 / 工作流分类改标「已计入消息」；系统提示按段展开只剩基座 / 人设 / 插件钩子。

- **子代理面板显示思考过程**：子代理的对话以前只画用户气泡、正文和工具调用，思考被丢在一边（`Entry` 的 `default` 分支直接返回 null）。现在子代理面板也画思考：想的时候是流光的「正在思考 · Ns」加最新几行，想完折成「思考了 Ns ›」，点开看全文；「设置 › 外观 › 默认展开思考过程」照样管它。数据侧没动——子代理的 `subagent/history` 与 `thread/history` 同形，本来就带 `reasoning` 条目。

### 修复

- **定时任务的时刻越跑越晚**：每次触发后下一次按「这次真正触发的时刻 + 间隔」排，一秒一查加上开页等待，每次晚多少就往后漂多少；电脑睡醒补跑那一次之后，整个任务就挪到醒来的时刻了。现在按建任务时的相位排，晚触发不影响下一次。
- **终端里关掉的页，桌面端还当它开着**：终端 `/tab close`（或别处）关页时 Dock 不出声，网关留着那一页的投影，桌面端也不知道，往里发消息报「没开」。现在分页一关就发 `tabs/page-closed`，网关丢掉投影并推连接级 `thread/closed`，桌面端标成关着，再发消息时自动重新打开。
- **平面材质下定时任务 / Pull Request 页上透出会话的消息导航**：整页和会话区左边的导航条同一层（z-20），导航条在后面就叠了上来；玻璃材质本来会把整页后面的东西藏起来，平面材质没有。现在两种材质都藏。
- **会话忙着时定时任务越积越多**：一轮跑得比间隔还长（或者你一直在和它聊），每次到点都往队列里再排一条同样的提问，忙完一口气连跑好几遍。现在上一次还排在队里没轮到，这次就不再叠。
- **桌面端的推理强度菜单每个模型都一样**：网关把档位写死成 none / low / medium / high，ChatGPT 的 xhigh / max / ultra、自己配的 `reasoning_efforts`（如 DeepSeek 的 max）在桌面端都选不到，终端 `/effort` 却是对的。现在两边同一份：当前模型的档位，前面加「关」（订阅模型只有档位表里有 none 才给「关」）；明说没有档位的模型不显示菜单。切档也认模型自己声明的值（如 `ultra`）。
  - 从 models.dev 目录开模型时把推理档位写进 `reasoning_efforts`（以前漏写，一律通用档位）；已配的模型在供应商卡片上「按目录补全推理档位」，编辑页「按目录填」。目录缓存升了版本，会重拉一次。
  - 编辑页的「可选档位」补上 none / minimal / max（以前只有 low / medium / high / xhigh，手动也勾不出 max）。
- **全新安装时先登录订阅，不用重启就能用**：以前启动时目录里一个模型都没有（没写 config、还没登录），Dock 就定死用离线的假采样器；之后 `/auth login` 登上了、模型也出现在 `/model` 里，发出去的话收到的却是假回复（`no workspace tool for: …`），要重启才好。现在每次采样都再看一眼目录，有能连的模型就走真的请求。ChatGPT 和 Grok 两条订阅通道都受益。
- 订阅通道的「token 失效、换一份重试」以前只认 401；provider 现在能自己认（Grok 的 token 校验不过回的是 403 `unauthenticated:*`）。
- **hooks / 插件定时器遇到超大秒数不再 panic**：hooks 在项目确认前解析 `timeout: 1e20` / `1e308` 时改用可失败的转换，最多 24 小时；正数超限或换算后不足 1 纳秒的命令跳过并记警告。`host.every` / `host.after` 最多 30 天，超限返回脚本错误，不占定时器名额。不改项目 hooks 的信任流程。

- **插件里的确认框弹在发起调用的那一页**：插件脚本里 `host.call_tool`（比如调 `bash`）和 `http_request` 的确认以前取插件所在的根页，桌面 GUI 里那一页没人看，调用一直挂着（实测会话里 `bash git status` 挂了 307 秒、调 FOFA 挂了 95 秒，用户反馈「卡住了，没有弹框」）。现在落在发起调用的会话页上。
- **只认 session 的 HTTP MCP 服务器连得上、过期了能接上**（如 Yaklang / Yakit 的 MCP）：这类服务器对没带 `Mcp-Session-Id` 的请求回 HTTP 400 + `-32602 Missing session ID`，以前握手把这个错误原样甩出来，一个工具都列不出；现在退回 initialize 拿 session。session 过期（比如 Yakit 重启）它回 `-32602 Invalid session ID` 而不是 404，以前此后每次调用都失败到重连为止；现在同 404 一样重新握手、重发一次。顺带修了一个让重新握手白做的老问题：服务器不给 GET 流时，重新握手拿到的新 session id 会被悄悄丢掉，一直带着作废的那个发请求。
- **重开会话后「已编辑 N 个文件」卡不再消失**：每轮改动的记录按「这一轮那条用户消息的时刻」认轮次，活会话里是微秒，会话从盘上读回来（Dock 重启、桌面端重新打开）时只到秒，对不上就被当成撤回作废，连同盘上的 `turn_changes/<序号>.json` 一起删掉，之后也撤销不了。现在读回来的会话按秒认；活会话照旧精确比，撤回后同一秒里再发一条也分得开。已经被删掉的记录找不回来。
- **只回了推理的一步不再当成回复收尾**：模型偶尔会回一个只有思考、正文和工具调用都空的采样（比如把工具调用写进了推理通道）。以前它被当成「答完了」：todo 清单还开着就插一步记账，模型记完账这一轮就结束，聊天框里什么都没有，用户只能自己发一句「继续」。现在只有正文非空的回复才算收尾。只有推理的一步会先落一条提醒说明上一步什么都没交出来，再重采；连着 3 次都这样就按失败收尾并写明原因（滚动区标「已停下」，不标「请求失败」）。
  - 采样本身出错（如 429）也不再触发回合末记账。以前记账那次采样一成功，这一轮的失败就被记成「完成」。
  - 子代理的采样出错（包括上面连着 3 次只有推理）时，交给父级的结果也是失败、带上原因。以前面板标了失败，父级模型拿到的却是「完成」、正文为空；后台子代理的回合结束通知里也看不出出了错。
- **压缩后上下文真正降下来**：压缩后的模型历史只留首条和最近一条用户消息 + 摘要，不再保留最后一条用户消息之后的整段工具链和推理（自动压缩正好发生在工具循环里，那段原样留着等于没压）。
  - 摘要请求带上工具结果的开头（每条 8KB）和调用实参，最多最近 40 条；按窗口的 70% 收，上游仍报上下文超限时下一次少带一半，不原样重发。
  - 摘要末尾指出被摘要掉的原文和原始日志在哪（`compaction/segment_NNN.md`、`chat_history.jsonl`），模型缺什么按需查；子代理同样有。
  - 摘要不比原文小就不压，历史不动，也不重试。
- **记忆的向量索引不再被无故清空**：嵌入维度不是 1024 时，`/flush`、`/remember`、没带查询嵌入的检索（没配嵌入，或嵌入接口这一次失败）都会按默认维度重开索引，被当成「换了维度」而把向量表整张重建，下一次又要全部重新嵌入。现在不指定维度时沿用库里已有的。
- **`/dream` 不再丢掉纯中文标题的主题**：整理出来的主题按标题起文件名，以前只留 ASCII，标题全是中文时文件名为空、这一节被直接跳过，而它的来源观察随后会被归档。现在这种标题用原文字当文件名；带英文的标题文件名不变。
- **`task` 按发起调用的那一页校验角色名**：名册校验跑在子代理协调器的后台 task 里，tokio 的 task-local 传不进那个 task，于是退回**根**的预设去校验——磁盘上没有 `roster.yml` 时根停在默认预设（`code`，三个通用角色），而当前页切成了别的模式（内置 `warden` 的九个汉字角色，或用户自写名册）时，模型照页面上给的 `subagent_type` 派活一律被说成「Unknown subagent type」，只列得出三个通用角色。校验现在用发起调用的那一页的名册。内置模式的角色**不用**在磁盘上物化：`~/.dock/presets` 里没有 `warden/` 目录是正常的，九个角色随二进制发布。
- **轨迹面板按模型实际顺序排上下文**：开场插在首条用户消息之前的注入块（规约 / 记忆 / 技能 / 工作流 / 名册）原来显示在「你」之后。`item/system_reminder` 新增可选字段 `placement`（`"preamble"` / `"tail"`），实时与回放都给（还没有用户消息时就到的前导块两边都标 `preamble`、记在即将开的那一轮上，刷新前后一致）；面板每轮先排开轮用的那份系统提示（中途变了才推的留在原位）、再排 preamble 块，然后才是用户消息。
- **轨迹面板与上下文面板同一口径**：`item/system_prompt` / `item/system_reminder` 新增可选字段 `tokens`（按原文全文估，`chars` 保留），面板显示 `≈N tok`。三份重复的文本 token 估算收成一个 `cordis_spine::estimate_text_tokens`；HTTP 后端在官方用量回来前的暂估改为逐段估再相加（与 `/context` 一致，每段最多多算 1）。
- **`llm/stream` 的改写写回会话**：钩子换掉的助手文本会盖掉已经流进会话的原文，界面和下一轮历史看到的是改写后的那份。
- **`agent/pre-step` 的拒绝理由写在开轮的那一页**：子代理或分页被拒时，不再把「这一轮没有开始」写进主会话。
- **桌面操作只认受信连接**：配对来的网页仍可申请看画面；`desktop/view/windows`、`pin`、`input` 回 `forbidden`。
- **插件的 `tools/execute` 钩子在完整应用树里不生效**：`tool-lsp` 挂在 `tools/execute` 上的监听没有把控制权交给下一环，链在它那儿断掉，排在它后面的监听者（包括插件脚本）全都拿不到这次结果 —— 插件想截断 / 脱敏工具输出，界面上看着像没挂上。已改成往下传，并补了回归测试（完整应用树里，磁盘插件的 `tools/execute` 钩子仍能替换结果）。

### 新增

- **定时任务能按日历、按条件、按文件变化跑，也能从脚本里触发**：以前只有「每隔 N 分钟」一种，而且 7 天后一律过期，「工作日早上 9 点汇总提交」「每周一出周报」「明天 10 点提醒我」「CI 挂了叫我」「docs 改了就同步英文版」都做不了。
  - 六种触发方式：
    - 固定间隔：盯部署、等 CI 这类。
    - cron 表达式：按本机时区，`0 9 * * 1-5` = 工作日 9 点。
    - 只跑一次：`30m` 后，或 `2026-11-23 09:00`。
    - **条件命令**：定时在项目里跑一条命令（`gh pr checks 12`、`curl …`），结果变了 / 成功 / 失败 / 输出匹配正则时才发提问，并把命令输出带给模型。平时只跑命令、不花 token；除了「结果变了」，都是从不满足变成满足时触发一次（建好时就已经满足的，第一次检查就触发）。
      检查最快每分钟一次；命令跑完或超时（2 分钟）时，它起的后台进程一起停掉。
    - **文件变化**：监听目录或 glob，改动安静下来（默认 10 秒）后发提问，附上改了哪些文件。任务自己那一轮跑着时的改动不算，免得它改文件又叫醒自己。
      目录是递归听的，所以 `/`、家目录这类太宽的范围直接拒。
    - **只手动**：桌面端「立即运行」，或者在脚本、git hook、本机 CI 里 `dock schedule run <id> [--input …]`。
  - `/loop 工作日 9 点 汇总昨天的提交`、`/loop CI 挂了就叫我` 这样说就行，模型会选对；`scheduler_create` 多了 `cron` / `at` / `watch` / `files` / `manual` / `new_session` / `paused` 等参数。
  - **每次新开会话**：日报、周报这类例行事可以选每次在同一个项目里开一个新会话，上下文干净；前几次的会话都留在侧栏里。定时任务替你开出来的页（新会话、为送提问重新打开的会话）闲置 10 分钟就关掉，终端里不会每跑一次多一个分页；会话还在，点开接着看。
  - **立即运行 / 暂停**：桌面端每行有「立即运行」「暂停 / 继续」；终端 `/tasks` 里选中定时任务按 `r` / `p`；命令行 `dock schedule run / pause / resume`。手动跑的不占次数、不动排期；继续时跳过暂停期间的时刻。
  - **运行记录**：每个任务留最近 20 次——为什么跑（到点 / 补跑 / 手动 / 条件满足 / 文件改动）、送到没有、送进了哪个会话，桌面端点开行就能看、能直接打开那次会话。条件命令还显示上次检查的退出码。
  - 过期改成按节奏定：比一天还勤的循环（含条件命令的检查间隔）默认仍是 7 天，其余不过期；可以自己定，或设 `max_runs` 跑满就停（「CI 过了告诉我一次」）。
  - 错过了（Dock 没开、电脑睡着）**最多补一次**，补的那次会告诉模型晚了多久。默认 cron 和一次性补、固定间隔不补（同以前），可改。一次性任务错过又设了不补，会在会话里留一句说明。
  - **权限**：模型建条件命令等于以后无人看着时反复跑这条 shell，所以单独一个权限键 `scheduler_create:watch`，弹窗首行是完整命令；之前对 `scheduler_create` 点过「始终允许」不覆盖它。到点跑起来的那一轮照常过权限门。
    监听项目目录外的文件同理，单独一个 `scheduler_create:files`，弹窗列出那些路径（改了哪些文件会写进提问）。
  - 桌面端「定时任务」页：新建 / 编辑时选「间隔 / 定时 / 一次 / 命令 / 文件 / 手动」和「发到这个会话 / 每次新开会话」；按时间可选每天、工作日、每周几、每月几号或直接写 cron。列表行显示中文说明（「工作日 09:00」「命令 · 每 5 分钟查一次，失败时触发」）。终端 `/tasks` 里显示成 `Cron 工作日 09:00` / `Watch 5m $ gh pr checks 12` 等。
  - **落盘**：`schedules.json` 升到 version 2（触发方式记在 `schedule` 里，多了暂停、目标、运行记录、检查结果等字段）。version 1 的文件照读，下一次写入时升级。**降级注意**：旧版 Dock 读不了 version 2 的文件，会报「解析失败」并且不碰它（不会覆盖掉），升级回来就好。比本版更新的 version 也一样拒绝写回。
  - **`dock.1`**：`schedule/*` 的任务多了 `schedule`（`{kind: every|cron|at|watch|files|manual, …}`）、`scheduleLabel`、`runs` / `maxRuns` / `catchUp` / `finished` / `target` / `preset` / `paused` / `pending` / `history` / `watch` / `holderLive`；`nextAtMs` 和 `expiresAtMs` 可以是 `null`，非固定间隔的 `everySecs` 是 `null`。`schedule/create` / `update` 收新的触发方式和选项，新增 `schedule/run`。依赖新增 `croner`（cron 解析，处理夏令时）；`cordis-spine` 多依赖了已在用的 `notify`。
- **Grok 订阅通道（SuperGrok / X Premium）**：登录 xAI 账号后，模型用订阅额度推理，不需要 API key。
  - 和 xAI 点名接入的 OpenCode、Kilo Code 走同一条路：`auth.x.ai` 的 OAuth，推理打公开 API `https://api.x.ai/v1/responses`。
  - 不打 Grok Build 自用的 `cli-chat-proxy.grok.com`，不带它的身份头，不读 `~/.grok/auth.json`。
  - 登录：`/auth login grok`，走 device code（不起回环监听，VPS 上一样用）；`/auth logout grok`、`/auth models grok`。
  - 登录后 `grok/<slug>` 模型自动出现在 `/model` 里（上下文长度、能否收图取上游给的）。
  - 带 `prompt_cache_key`（取会话 id）：实测后续请求 34 次中 27 次命中，命中时约占输入的 95% 以上；不带只中 1/3。
  - 推理档位按 models.dev 的 xai 条目填：grok-4.6 是 low…xhigh（没有 max），grok-4.20-reasoning 不列档位，non-reasoning 不发推理字段。
  - 模型仍不收某一档时（目录过时、拉不到目录），去掉 `reasoning.effort` 重发一次并记住。
  - 凭据存 `$DOCK_HOME/credentials/grok/<账号>.json`（0600）。`DOCK_GROK_SUBSCRIPTION=off` 关掉整个通道。
  - **剩余额度**：终端 `/auth usage`；桌面端输入框旁是剩余额度的小圆环（点开看详情），设置 › 模型 › 订阅通道里有一张额度卡（水球、剩余 / 已用、多久后重置、计费周期）。
    - 公开 API 查不到订阅额度，只读查 Grok Build 自用后端的 `/billing?format=credits`（只带 Bearer，推理仍走 `api.x.ai`）。
    - 上游只给百分比，界面上不编 token 数。网关新方法 `auth/usage`（只认受信连接）。
  - 实测记录见 `docs/GROK-SUBSCRIPTION.md`。
- **插件脚本能力补强**：
  - `host.call_tool_result(name, args)`：同 `host.call_tool`，但回 `#{ name, text, json, is_error, images }`——知道这次调用是不是失败了（被拒、被拦、报错、非零退出），工具回的是 JSON 时直接拿到解析好的值。插件要读文件、抓网页、操作浏览器、调 MCP 服务器，直接调对应的工具就行，不用等新的宿主函数。`host.call_tool` 不变。
  - 脚本里多了两个只做计算的 Rhai 生态包：`rhai-url`（`Url("…")` 解析、改查询参数）和 `rhai-rand`（随机数、打乱、抽样）。读写文件、发请求这类包没有加——它们会绕过权限门。
- **插件能改 agent 本身的行为了——MCP 做不到的那一面**：MCP 要等模型来调，插件包在模型外面，每一次调用、每一轮都会经过它。这次把插件宿主补齐，写插件不再只是「加一颗工具」：
  - `tools/pre-execute` 能返回 `#{ ask: "理由" }`：这次调用要用户点头才跑，**自动批准和「始终允许」都不算数**。适合「`git push` 要我确认」这类规矩。
  - `tools/execute` 钩子带上了这次调用的参数（`args`），判断「刚跑完的是哪条命令」不用再自己记。
  - 新钩子 `session/entry`：结构化的会话事件，带页身份；助手那一条来自采样收尾，带最终文本、工具调用、模型、用量、缓存命中率和费用——成本 / 缓存统计插件靠它。
  - `host.followup(text)`：插件主动找 agent。那一页在跑就在下一步带上，闲着就叫醒它开一轮（一页 5 秒内最多叫醒一次）。配合新的定时器 `host.every` / `host.after`，插件能定时看 CI、看 PR、看监控，出事了叫 agent 来处理。`host.followup(identity, text)` 只能送到插件正在替它跑的那个会话（那一页和它的子代理）：插件是整个进程共用的，不能借钩子里看到的别的会话 id 往那边塞话、叫醒它。
  - `host.llm(prompt)`：用这一页的模型问一句（不带工具、不进会话），做复审、分类、摘要。
  - `host.load` / `host.save`：跨 reload 和重启的小存储（`$DOCK_HOME/plugin-state/`，不写进项目）。
  - `host.card(...)`：在一轮回答下面挂一张卡片——标题、正文、视图树、按钮，按钮点了在那一页上跑插件的回调。GUI 画在回答下面（新协议 `turn/cards/list` / `action` + 推送 `turn/cards/updated`，能力位 `turnCards`），终端发卡时记一条通知。
  - 技能 `cordis-plugin-development` 改成以行为插件为主线（「以后每次…」「不许…」「改完必须…」就是在要它，写进记忆只是建议），带三个有测试的例子：`guard.rhai`（项目规约守门）、`done-check.rhai`（改了 `.rs` 没跑测试不许收尾 + 「跑测试」卡片）、`trim-output.rhai`（去 ANSI、超长输出只留头尾）。创造模式人设同步。
- **Claude Code / Codex 的 hooks 配置直接能用**：新内置插件 `hooks-bridge` 跑 command hook，语义对齐 Claude Code / Codex（以及 DSH 的两个桥接）：`SessionStart`、`UserPromptSubmit`、`PreToolUse`、`PostToolUse`、`Stop` / `SubagentStop`；退出码 2 拦下、stderr 是理由；JSON 回答里的 `permissionDecision`（`deny` / `ask` / `allow`）、`additionalContext`、`decision: block`；多条取最严。matcher 按 Claude 的工具名（`Bash`、`Edit`…）或 Dock 的都认。
  - `~/.dock/hooks.json` 和项目 `.dock/hooks.json` 一直读；Claude Code（`~/.claude/settings.json`、项目 `.claude/settings{,.local}.json`）和 Codex（`~/.codex/hooks.json`、项目 `.codex/hooks.json`）在用户级 `~/.dock/config.toml` 写 `[hooks] claude_code = true` / `codex = true` 才读——项目配置开不了这两个开关。
  - **项目里的 hooks 文件第一次跑之前要你点头**（列出要跑的命令和各自的超时；自动批准不算数），按内容指纹记在 `$DOCK_HOME/hooks-trust.json`，文件一改就重新问。克隆来的仓库自带的 hooks 不会悄悄在本机跑。
  - `allow` 不会绕过 Dock 的权限门；hook 坏了（非 2 的退出码、超时、起不来）只记一句，不拦。新命令 `/hooks` 列出生效的配置。详见 `docs/HOOKS.md`。
- **MCP App 面板跟着模型实时操作**：模型一开始写带界面工具（MCP Apps）的参数，桌面端右侧面板就打开或切过去，参数边写边交给界面（规范的 `tool-input-partial`），不用等参数写完。
  - 内置白板边写边画半透明草稿，「Agent」光标停在正在画的地方、镜头跟着走；改 / 删 / 排版类先高亮要动的图形；结果到了同一帧换成真的。
  - 要上游把工具参数逐段流过来才看得到（如 `deepseek-flash`）；把参数攒成一整块才发的端点退回原来的效果。
  - `dock.1` 新推送 `item/tool_input_delta`（能力 `toolInputStream`，只推不记）；Rust 侧 `StreamDelta` 新增 `ToolInput` 变体：对它做穷尽 `match` 的代码要补一个分支。
- **记忆图谱**：记忆笔记之间可以互相链接，桌面端侧栏新增「记忆」页，把笔记画成一张关系图并在里面管理。
  - 链接语法和 Obsidian 一样：`[[文件名]]`、`[[文件名|显示的字]]`。记忆目录可以直接当 vault 打开。
  - agent 写记忆时会顺手链到相关的笔记（记忆提醒、`/flush`、`/dream` 的提示词都加了这条约定）。
  - `memory_get` 的 `path` 现在也认 `[[链接]]`、笔记名和索引里的相对路径，模型可以顺着链接读下去。
  - 图上：点是笔记（颜色分当前项目 / 全局 / 其他项目，主题实心、观察空心，大小跟链接数走），实线是链接。
  - 「内容相近、还没链上」的建议默认只在悬停 / 选中那篇时画成虚线；标签随缩放出现。
  - 抽屉顶上有局部图谱（1–2 跳）；画布右上角的设置能调显示和力导向，还能按搜索条件给笔记上色（最多 3 组）。
  - 管理：预览与编辑、新建、重命名（指向它的链接跟着改写）、观察升为主题、移到全局、遗忘、建链 / 断链。
  - 默认看「当前项目 + 全局」，可切到全部项目；有图谱和列表两种视图，能按标题和正文搜。
  - agent 写了新记忆，图上实时多出一个点。记忆功能关着也能看、能改。
  - 协议：新增 `memory/graph|search`、`memory/note/get|save|create|rename|move|forget`、`memory/link|unlink` 与推送 `memory/changed`，能力 `memoryGraph`。全部只认受信连接。
  - 细节见 `docs/tools/memory.md`「笔记之间的链接」「记忆图谱与管理」。

- **订阅通道（ChatGPT 套餐）**：按 OpenAI「Sign in with ChatGPT」开源通道的官方流程登录 ChatGPT，模型用套餐额度推理，不需要 API key（实测 Go 套餐可用；Free 账号授权页会显示「所需权限不可用」）。
  - 登录：终端 `/auth`（不带参数 = 直接开始登录，打开官方授权页，在后台等，完成后弹提示）；子命令 `login [new|identity|<账号>]` / `logout` / `cancel` / `status` / `list` / `use` / `models` / `import`。桌面端在设置 › 模型 ›「订阅通道」，按官方 UI 规范有「Continue with ChatGPT」「添加账号」「Manage usage」；输入框旁标「Using ChatGPT plan」。
  - **登录后就有模型**：活动账号能用的模型自动出现在 `/model` 和桌面端模型菜单里（`chatgpt/<slug>`，带上下文窗口、推理档位、能否收图），切账号重拉，登出或没开套餐额度就拿掉；config 里写同 id 的 `[model."chatgpt/<slug>"]` 可以逐字段覆盖。手写的模型条目写 `auth = "chatgpt"` 也走这条通道；**不写 `auth` 的模型行为不变**，与配 `api_key` 的模型可以并存。
  - 登出只清 token、保留注册，下次登录直接复用；多个 Dock 进程共用凭据，刷新互不打架。浏览器回调页会显示真实结果（连上了哪个账号、套餐额度开没开、还差一步或没连上的原因与下一步）。
  - VPS 场景：本机登录后 `/auth import` 把凭据交给远端，远端用自己的 host id 接管刷新。
  - 凭据存 `$DOCK_HOME/credentials/chatgpt/<client_id>.json`（0600），可选模型缓存在 `$DOCK_HOME/auth_models.json`。`DOCK_CHATGPT_SUBSCRIPTION=off` 关掉整个通道（provider 不注册，自动发现的模型也不进目录）。
  - 协议：新增 `auth/status|login|logout|cancel|models`（只给受信连接）与推送 `auth/changed`；`model/list` 新增 `authProvider`、`discovered`。
  - MCP 服务器的 OAuth 登录一起修了：回调只认回调路径（无关请求不会废掉这次登录）、`mcp_credentials.json` 创建即 0600、刷新持跨进程锁、上游没给新 refresh token 时沿用旧的，浏览器那页同样显示真实结果。
  - 自查脚本 `scripts/chatgpt-siwc-check.py`（公开端点 + 用活动账号的最小请求），详见 `docs/CHATGPT-SIWC.md`。
- **桌面画面可以操作**：`desktopControl` 能力，以及 `desktop/view/windows`、`pin`、`input`。钉住一个窗口后，点击、滚动、打字打到那一帧的像素上；预览是只显示的 JPEG，不替换 agent 的元素快照。操作只认桌面 GUI 和设备令牌。
- **本机和多台远程可以同时开着**：`remotes.json` 的 `active` 迁成 `opened` 与 `foreground`（`null` 是本机）。切换只换前台连接，不关掉本机 Dock，也不重载页面。
- **插件能拦流程了**：磁盘来源的 Cordis 插件（插件目录装上 / 设置里启用 / 审批过的 `cordis_promote` 落盘）可以在 `host.on` 上挂五个新钩子，把 agent loop 的关键位点接出来：
  - `agent/pre-step`：拒绝一整轮，理由写进会话（模型看得见为什么没跑）；
  - `system-prompt/assemble`：往系统提示加一段（固定槽位、按插件 id 排序 → 提示词字节稳定，前缀缓存不作废）；
  - `tools/pre-execute`：在权限门**之前**拒绝一次工具调用，或改道到别的工具（拒绝是单调的，先拒的赢；改道后按新名字重过允许名单、计划门、权限门，改道绕不过门）；
  - `tools/execute`：事后替换模型看到的结果内容（压缩、脱敏）；
  - `llm/stream`：改写采样输出（只能改输出，改不了请求 —— 要改请求走 `system-prompt/assemble`）。

  原来的三个事件（`session/event`、`agent/step-start`、`agent/turn-end`）任何插件都能挂，行为不变。**会话里 `cordis_define` 的包打不开这五个**：否则模型能给自己开闸门、能改自己的系统提示，那是自我提权。详见内置技能 `cordis-plugin-development` 的 `references/hooks.md`。

- **桌面 GUI 进仓库**：`dock-gui/`（Tauri 2 + React）的核心代码随源码提交（设计稿、文档不进仓库），见 `dock-gui/README.md`。独立工程（`src-tauri/` 自带 `[workspace]`，不进根 workspace），启动时拉起 `dock serve` 走 dock.1。还没签名，不带自动更新。供应商 / 模型目录不随应用打包：设置页第一次打开时经 Dock 从 models.dev 拉一份缓存起来，离线拉不到就是空目录，照样能手填自定义供应商。

- **聊天里发的图片落盘，刷新后照旧看得到**：用户消息里的图以前只活在本窗口的内存里（事件只带尺寸），刷新、重开 Dock、换机器连远程，全看不到原图。
  - 像素按内容寻址存进 `$DOCK_HOME/user-images/<摘要前两位>/<摘要>`，会话日志（`chat_history.jsonl`）只写摘要，不写 base64、也不写路径；关着的会话只读引用，恢复会话或取图时才按摘要读像素。
  - 协议：用户消息的附件带 `index` 和 `digest`（sha256 十六进制）；`item/image` 支持 `item/image { threadId, digest }` 取用户图片（工具结果那张图的老用法不变）。
  - 旧会话照旧读（没有图就是没有图）。

- **核对模型上下文的旁路**：dock.1 新增 `item/system_prompt` / `item/system_reminder`，把每轮组装的系统提示和注入块投给客户端（不是聊天消息）。系统提示前缀没变就不重复投；正文超过 20000 字截断并标出原长度。子代理给父级的话、插话提示、侧边笔记不重复投。只投主会话（子代理会话两样都不投）。注入块里有长期记忆索引、工程规约副本，`thread/history` 和 `dock serve --remote` 的设备令牌客户端都能读到（见 `cordis-gateway/README.md`「harness 上下文」）。轨迹面板能看见这些记录，多一条「上下文」轨道。

- **白板布局工具**：`whiteboard_group` / `align` / `style` / `layer` / `fit_text` / `distribute` / `layout` / `place` / `graph`（独立工具，避免塞进 edit 撑爆 schema）；`whiteboard_read` 多返回可复用的 `patches`。
  - `distribute`：≥3 个图形两端固定中间均分；`layout`：网格重排；`place`：相对参照放置；`graph`：nodes+edges 自动排版（layered/row/column/grid）。
  - 挪动连线端点时箭头 **复用原 id**，不再改成 `id~rev`。

- **MCP elicitation 支持 MRTR**（MCP `2026-07-28`）：服务器在 `tools/call` 的结果里回 `resultType: "input_required"` + `inputRequests` 时，Dock 把提问弹给用户，收齐答复后**用新 id 重发**原请求（params 带 `inputResponses` 与原样回传的 `requestState`）。
  - 以前这种结果被当成普通工具输出（找不到 `content` 字段，整段 JSON 塞进工具文本）：用户看不到表单，服务器永远等不到答复。
  - 提问归**发起这次调用的那一页**，别的页看不到也抢不走。
  - 服务器反复要输入最多 8 轮；用户拒了或取消就不再重试（服务器不该假设客户端一定会答）。
  - 仍兼容老服务器的 server-initiated `elicitation/create`。

- **MCP elicitation 的 URL 模式**：`elicit/open` 网关方法（桌面端与浏览器 UI 的「打开链接」按钮），在服务器那台机器上开浏览器。
  - server-initiated 模式：用户点「打开链接」就**立刻**回 MCP 服务器「同意」（规范要求不等 `notifications/elicitation/complete` —— 那是 MAY，服务器不发就一直挂）。
  - `-32042`（服务器用错误码要 URL 交互）：等 `complete` 通知到了才重试原请求，不提前回。
  - 界面显示完整地址 + 域名（高亮，防子域仿冒）+ 可疑警告（Punycode / 明文 http）。

- 每轮改动：agent 一轮里用 `search_replace` / `write_file` 改过的文件按轮记下（落盘在会话目录，重开还在）。
  - GUI：一轮结束后出改动卡「已编辑 N 个文件 +x -y」，查看文件、撤销（还是那一轮改完样子的文件才恢复）、审查。
  - GUI 右侧工具面板加回「审查」：看工作区的 git diff（含未跟踪的新文件）或某一轮的改动，code-review 的批注落在行上，「让 Agent 审查」直接跑 code-review。
  - 网关：能力 `turnChanges`（`turn/changes/list|diff|revert` + 推送 `turn/changes/updated`）、`worktreeDiff`（`vcs/worktree/diff`）。
  - bash 里改的文件不算进每轮改动。

- 子代理可以单独设审批档位：GUI 子代理面板的输入框换成和主会话一样的样式，底栏「审批」胶囊选跟随会话 / 请求批准 / 自动批准；它自己的权限请求也在面板里批。
  - 落在自动批准上（单独设的或跟会话的）的只读子代理（探索、code-review 审查员等），会改东西的命令直接拒绝，不再弹窗。以前主会话开着自动批准也照样弹。
  - 网关：`subagent/approval/set`；子代理投影多出 `approval`、`readOnly`。

- 内置工作流 `code-review`（`/code-review [范围或关注点]`，`/code-review #42` 审 GitHub PR）：定范围 → 正确性 / 安全 / 性能 / 可维护性 / 测试五个角度并行审查 → 合并重复发现 → 逐条对照代码核实 → 按严重程度出报告。全程只读，不用等批准。
  - GUI：工作流卡「在改动上看批注」打开差异视图，批注落在行上；PR 页抽屉「查看改动」看 PR 的 diff 和 Agent 批注，「让 Agent review」改为跑 code-review。
  - `gh pr view|diff|list|checks|status`、`gh repo view`、`gh issue view|list` 算只读命令（不用等批准；`gh api`、`--web` 不算）。
  - 网关：`vcs/pr/diff`、`vcs/diff`（受信连接，只认只读 diff 命令、不经 shell）；`workflow/updated` 带 `resultData`（脚本 `complete` 的 `data`）。

- GUI 对话流里的工作流卡：阶段、各阶段派的子代理（点开看过程）、在跑几个、用时、停止，结束后给结果或失败原因。
  - 对照 grok：一页最多 4 条在跑的工作流；`validate_only` 按这次的 `agent_budget` 冒烟；名字不认识时列出可用的工作流；run 显示这次的查询而不是脚本描述。
  - 网关新增能力 `workflows`：`workflow/list` / `workflow/stop` + 推送 `workflow/updated`；dock-core 加 `workflows.ts`。

- **会话里起草预设交给用户审**：新模型工具 `propose_preset`（按需）只起草、不写盘；桌面端出草稿卡，点开进预设编辑器逐项审、自己保存。终端照旧写文件。
- **预设编辑器「问 AI」**（桌面端）：右侧侧聊，带当前草稿和所在步骤，只给参考，不改任何字段。新建预设默认手动分步，AI 起草收成次要入口。
- 网关 `preset/ask`：上面那个侧聊的后端，无状态，只回参考答案、不写盘。
- `preset/list` 可带 `cwd`：列出那个项目的 `.dock/presets` 里的预设。
- **子代理对话落盘**：父会话落盘时，子代理的对话写在 `<会话>/subagents/<agent-id>/`。
  - `subagent/list` / `history` 内存里没有就读盘：关掉的会话、Dock 重启后都看得到（`persisted: true`）。
  - `thread/list { scope: "all" }` 每项多出 `subagentCount` / `subagentsRunning`。
- **子代理接着聊**：`subagent/send` 发给已经收掉的、或只在盘上的子代理，同一个 id 从它的对话接着起一轮。
  - 用户开的一轮（发给闲着的、复活的）结果送进父信箱但不叫醒父级。
- **关于用户**：`~/.dock/USER.md` 和用户级 `AGENTS.md` 同一个规约提醒块注入，排在它后面。
  - 网关 `profile/get | set` 读写这两份（只认受信连接）；GUI 设置 › 个性化编辑。
- **模型供应商**：`config.toml` 新增 `[provider."<id>"]`，同一家的模型共用一份连接。
  - 模型写 `provider = "<id>"` 继承地址、协议、鉴权、密钥；自己写了的键优先。
  - `env_key` 没有对应环境变量时，再找 `$DOCK_HOME/secrets.json` 里同名的那项。
  - 网关 `provider/list | save | delete | models | catalog`（只认受信连接）。
  - `provider/models` 用这把密钥问上游 `GET /models`；`provider/catalog` 拉 models.dev 目录。
  - `model/list` 每条多出 `provider`；`keyEnvSet` 也认 secrets.json。

- **MCP Apps**（`io.modelcontextprotocol/ui`）：MCP 工具可以带交互界面。
  - 客户端握手声明 MCP Apps 能力；认工具 `_meta.ui`，只给界面用的工具不进模型工具表。
  - 模型调挂界面的工具时，原始结果存进会话目录 `mcp-apps/`，界面要原样那份。
  - 本机 stdio 服务器的调用 `_meta` 带 `dock/cwd`（按项目存数据用）。
  - 网关 `mcp/app/tools | resource | read | call | result | context`（能力 `mcpApps`，只认受信连接）。
  - 界面状态（`ui/update-model-context`）下一步交给模型（spine 插件 `mcp-app-context`）。
  - 界面自己登记的工具在界面开着时交给模型：`app_<server>__<name>`，经 `use_tool` 调。
    网关 `mcp/app/view/expose | result`、推送 `mcp/app/view/call`（能力 `mcpAppViewTools`）。
- **内置白板**：`dock mcp whiteboard`（新 crate `cordis-apps`，Excalidraw），内置行 `[mcp_servers.whiteboard]`。
  - 模型工具 `whiteboard_read` / `whiteboard_edit`；场景按项目存在 `$DOCK_HOME/apps/whiteboard/`。
  - Agent 与用户同时改，按元素版本合并。
  - 默认开启：升级后多一台 MCP 服务器和两颗工具。
    不要的话在 `/mcps`（或 GUI 设置 › MCP）里停用，会写进 `config.toml`；或设 `DOCK_WHITEBOARD_MCP=off`。
  - 面板里的界面工具：`get_selection` / `get_viewport` / `focus` / `snapshot`。
- **`dock init` / `dock doctor` / `dock upgrade`**：
  - `init` 初始化 `~/.dock`；`doctor` 健康诊断；`upgrade` 原子升级，`dock update` / `--check` 只检查。
  - `DOCK_CHANNEL=pre` 走预发布通道，没有可用预发布时直接报错，不回落 latest。
  - 版本 tag 只认 `vX.Y.Z` / `vX.Y.Z-{alpha|beta|rc}.N`；预发布标成 prerelease，stable 通道看不到。
  - `install.sh` 显示三阶段进度。

- **嵌入页（embed-sdk）补齐要用户拍板的几处**：计划模式提交的计划（批准并执行 / 要求修改 / 退出计划）、
  MCP 服务的确认（接受 / 拒绝 / 取消）；会话菜单能重命名、归档、删除。`npm test` 跑 SDK 单测。

- **浏览器面板强制重新加载**：`browser/view/navigate` 支持 `action: "hard_reload"`（绕过缓存，和 Shift+刷新一样），能力 `browserHardReload`。
- 插件插槽斜杠在 GUI 直开右侧面板：`kind: "slot"` 的插件斜杠（`/cyberspace` 这类）以前只会回「请在 Dock 终端打开」，现在网关投 `openSlot` 响应，GUI 直接开右侧的插件面板。

### 改进

- **改动卡能直接看 diff，工作区外改过的文件也能打开**：一轮结束后的「已编辑 N 个文件」卡，展开「查看文件」以前只有文件名和行数，点工作区外的文件（如 `/tmp/…`、`~/.dock/memory/…`）在文件面板里什么也打不开。
  - 每个文件行首的箭头在卡里展开这个文件的 diff；点文件名在右侧「审查」里定位到它（滚过去、描一下边）；行尾按钮在文件面板打开。
  - 文件面板树下多一组「工作区外 · agent 改过（只读）」：这个会话里 agent 用 `search_replace` / `write_file` 改过的工作区外文件，点开只读看，不能编辑。
  - 协议：`fs/read` 也收这些文件的绝对路径（能力 `workspaceFilesOutside`），别的绝对路径照旧拒；绝对路径只认受信连接（桌面 GUI、设备令牌），配对来的网页回 `forbidden`。旧版 Dock 没这个能力，桌面端不出行尾按钮和那一组。
- **轨迹面板**：计时总览增加「等用户」轨道（提问 / 计划审批 / MCP 询问）；记录表能看笔记、提问、计划、MCP、子代理、工作流和文件改动。压缩进展 / 子代理 / 工作流 / 文件改动是「只推不记」的，面板按 id 覆盖同一条，不追加成一串。

- **浏览器面板按标签页记视口**：每个标签页记住自己的尺寸，切回去还是那个大小；没改过的页用最近一次要的尺寸。

- **CI 按路径跳过无关门禁**：只改文档 / 纯配置时不再跑完整 Rust 测试；`dock-core` 仅在 `dock-core/**` 有改动时跑。job 名仍是 `test` / `dock-core`（required checks），无相关改动时 echo 跳过仍报 success。Release 的 tag 门禁保持全量。

- **发布产物不再包含 Intel Mac**（`x86_64-apple-darwin`）：只发 Apple Silicon（`aarch64-apple-darwin`，构建机 `macos-15`）与 Linux x86_64 / aarch64。`install.sh` / `dock upgrade` 在 Intel Mac 上会说明请自行从源码构建。

- **插件开发技能改成内置、分层按需读**：`cordis-plugin-development` 随二进制发，任何项目都能加载（以前只在 dock 仓库里才找得到）。
  - `SKILL.md` 只放流程与路由；细节拆进 `references/`（host API、界面、钩子、HTTP、版本、Rhai 坑位、报错对照），例子在 `examples/`。
  - `skill` 工具列出附属文件的绝对目录和子目录里的相对路径。

- 嵌入页首次配对不再显示「连接出错」/「授权已失效」：宠物是「等待在 Dock 里批准连接」，卡片写的是本页的 Origin。
  - `dock:state` 只在变了时报（以前连上一次刷十几条一样的），`label` 改中文；宿主按 `visual` 判断。`dock:ready` 每次连上只报一次。
  - 斜杠菜单：输入就是那条命令时 Enter 直接发；菜单太长自己滚，不再顶出面板。
  - 会话标题跟着第一条消息更新；「已完成」只亮几秒；代码块在浅色主题下可读；界面文案统一中文。
- 启动不再等 MCP 握手：后台并发连，`dock serve` / TUI 马上就绪。
  - 实测带两台远程 HTTP 服务器的配置：GUI「连接 Dock」约 10 秒 → 0.1 秒内。
  - 没连完时 `/mcps` 显示「连接中…」，`search_tool` 报 `partial`；工具连上即登记。

- GUI 浏览器面板更像真浏览器：
  - 下拉框能点开（GUI 自己画菜单）；拖选文字、⌘A / ⌘C / ⌘X / ⌘Z 可用，复制进本机剪贴板。
  - 连本机 Dock 时滚动、打字过程中也是 Retina 清晰画面；远程照旧省流，停下补清晰帧更快（150ms）。
  - GUI 侧：页面按面板宽度排版（最窄 800），字是正常大小；拖出画面松手不再卡住按键。
  - 有头浏览器（`[browser].headed`）：面板多一个「在窗口中打开」，切到真窗口直接操作。

- bash 卡片显示命令的描述（模型给的 `description`）：TUI 与 GUI 标题是描述 + 主程序名（`find`、`cargo test`），完整命令展开看。
- 权限弹窗里 bash / monitor 显示完整命令，描述另起一行；原先是截到 120 字的原始 JSON。
- **长会话流式不卡，开着的会话不再限 9 个**：会话轨迹事件增量折叠、只重画变了的轮次；对话流只画最近 12 轮、翻到顶附近再补；贴底跟随改用 ResizeObserver，只有用户往上翻才放手。无头 Chrome 实测 40 轮合成会话：流式 3.7 fps / 点击延迟约 450ms → 60 fps / 约 1ms，打开长会话 DOM 24838 → 1739。
  - `thread/history` / `subagent/history` 把同一轮连着的流式增量并起来再给（3717 条 → 58 条），按「不大于已见最大 seq 就丢」去重的客户端回放状态不变。
  - 桌面端开页不再设 9 个上限（终端自己开页仍 9 页，Alt+1..9 直达）；后台超过 16 张闲页时自动收最久没看的，在跑 / 在等 / 在看 / 有子代理在跑的页不收。

### 修复

- **嵌入页：只有交互在等用户的那一轮，回应完不再一直显示「正在使用本地能力 / 正在思考」**：网关在没有轮可挂时会给 MCP elicitation、权限、提问、计划单开一轮，那一轮只有交互项。`embed-sdk` 回应交互后以前一律切到 `working` / `thinking`，这一轮又不会再有 `turn/completed`，于是一直显示忙碌；现在这种纯交互轮回应完回到空闲（同一轮还有没回应的交互就接着等），`hasPendingWork` 也和 `dock-core` 的 `turnHasActivity` 同一语义，不再把纯交互轮当成有活在跑。交互卡片照旧显示、照旧可答。

- **排队 / 插话时发的图跟着消息走**：入队 → 插话 → 送达 → 下一轮采样都带着同一张图。以前排队消息的图寄放在会话的「图片槽」里（只有一格），多条排队消息各自带图会互相顶掉，插话还会把图落下。
  - 网关：入队 / 插话时就把图按内容寻址落盘，`turn/queue/list` 每项带 `images[]`（图片摘要）；`item/image { threadId, digest }` 在送达之前也能取到像素，仍是会话级凭据。
  - TUI：发送 / 插话时图绑在这条消息上，不再先放进图片槽（排队期间别的消息送达会把它顶掉）。

- **只有交互在等用户时，不再显示成「运行中 / 正在思考」**：Dock 在没有轮可挂时，会给 MCP elicitation、权限、提问、计划这类交互单开一轮；那一轮只有交互项，agent 其实停着等用户回答。`dock-core` 的 `isRunning` 收紧成「最后一轮在跑 **且** 这一轮真在干活（用户消息 / 正文 / 思考 / 工具）」，纯交互轮不再算运行中，基于 `dock-core` 的客户端（用 `isRunning` 判忙碌的会话头、侧栏、对话流等）也就不再说 agent 在忙。
  - 交互卡片照旧显示在会话底部等用户回答；发新消息时那一轮会被正常停掉重开。
  - `dock-core` 新增导出 `turnHasActivity(turn)`，客户端要自己判「这轮有没有在干活」时用它。

- **白板复制、分层和查找**：`whiteboard_duplicate` 改的是嵌套绑定（不再写出扁平的 `startBinding.elementId`），副本用新的 z 序键，`place` 按选区外框平移；两端都在选区里的箭头跟着复制，`texts` 只改请求里的图形（自动带上的箭头保留原标签），副本的 `boundElements` 不再挂着没复制的外连箭头，待画图形的标签留在 pending。`whiteboard_graph` 的回边不再把下游拽回上层，节点按真实宽高占位。`whiteboard_find` 的 `near` 必须是已有 id，命中按整段相等 / 前缀 / 包含再按距离排。`distribute` 在图形总尺寸超过两端跨度时报错且不改坐标。`related` 带上指向该图形的箭头。

- **白板箭头 id**：移动连线端点后重连仍复用原箭头 id（不再生成 `id~rev`）。

- **白板 MCP**：重连的箭头留墓碑而不是删掉，界面把旧版本存回来时按版本比较，不会出现两个同 id 的箭头；
  `whiteboard_fit_text` 只改尺寸时不再重连箭头；`whiteboard_graph` 画的新图排到现有内容右边（不再压住已有图形），
  两端太近时边也有几何；间距参数（`gap` / `gap_x` / `gap_y`）不再接受负数。
- **白板 MCP 更扛得住模型的参数写法**：数组被拍平成对象、套多层 `{"item": ...}`、单个 id 写成字符串，都当数组收下；
  坐标写成数字字符串也认，写成别的会报错（不再默默挪到默认位置）；图形对象里写了不认识的字段会回一句提示；
  工具结果不再回全量场景，只讲这次改了什么（全量清单用 `whiteboard_read`）。
- **白板 MCP 学了 wonder 的几招**：新增 `whiteboard_find`（按文字 / 类型 / 颜色 / 「在谁附近」找图形，不用背 id）与
  `whiteboard_duplicate`（复制整棵：标签、组、箭头绑定一起复制，可改文案、可相对原件放置，回新 id）；
  `whiteboard_read` 支持分层读（`overview` 总览 / `ids` 只看这几个 / `related` 连容器、标签、箭头两端、同组一起给）；
  `add` 不给坐标时自动找空位，不再都叠成一列；界面工具 `snapshot` 加 `mode`（`thumbnail` / `full`）。
- **白板界面工具的参数也认模型端的写法**：`snapshot` / `focus` 的 `ids` 现在认 `{"item": [...]}` 包装、单元素裸值、
  嵌套数组（之前会退化成「截全部」），`select` 认 `"true"`。
- **白板 MCP 删得干净**：删掉的元素留的墓碑版本现在远高于界面手里那份 —— 版本一样时 Excalidraw 合并会保留
  本地的，已删的图形会留在画布上又冒出来；副本 id 也统一成一族（`<id>-copy` / `<id>-copy2` / `<id>-copy3`）。
- **白板 MCP 的界面改走右侧面板**：调用发生就自动在右侧打开 / 更新面板，消息流里不再摊开内联界面（原来 420 高的 iframe
  会被后面的消息顶下去）；工具卡也留在真实调用的位置，不再被挪到这一轮末尾堆成一堆。

- **白板 MCP**：`add`/`update` 的 schema `items` 改为 `additionalProperties: true`（字段说明进 description）；自动剥恰好单键 `{"item":[...]}` 包装并提示「已自动去掉 item 包装层」；错误信息带例子与下标。

- **回放加密 thinking 的 signature**：Anthropic Messages / 兼容 chat 路径落盘 `thinking_blocks`，下一轮原样回传签名，工具循环里思维链不断。
- **思考卡片不再画密文或 signature 残片**：展示优先用 `thinking_blocks` 里的摘要；旧会话里缺开标签的 `{signature}</signature>\n摘要` 也会剥干净，只留可读正文。

- **MCP elicitation 的表单认不全规范的 schema**：带标题的单选（`oneOf: [{const, title}]`）之前退化成自由文本框，带标题的多选（`items.anyOf`）直接把整个表单判死（用户看到的是「服务器拒绝了」，不是一个选择框）。两种写法现在都认。
- **MCP elicitation 不按 schema 校验用户的回答**：现在按 `minLength` / `maxLength` / `minimum` / `maximum` / `format`（`email` / `uri` / `date` / `date-time`）/ `minItems` / `maxItems` 校验，不合就说明原因让用户改。
- **MCP elicitation 的可选数字字段不能留空**：留空必报「请输入数字」，表单永远走不完（工具调用和整轮对话一起卡死）。不在 `required` 里的字段现在可以留空 —— 数字类整个键不出现，让服务器用它自己的默认值。
- **MCP elicitation 缺「明确拒绝」的入口**：终端里只有「取消」（关掉），没有「拒绝」—— 服务器分不清「用户不想要」和「用户走开了」。现在 `d` 键是拒绝。
- MCP 服务器发客户端没声明的 elicitation mode 时，之前默默当成表单处理；现在按规范回 `-32602`（Invalid params）。
- 写进主线的子代理通知 / 工作流收尾在 `<system-reminder>` 里，正文中的 `</system-reminder>` 现在先转义（审外部 PR 时 PR 原文可能带它，能提前闭合提醒块）。code-review 的 diff 命令和 `vcs/diff`、只读命令判定都拒 `--ext-diff` / `--textconv`（会跑 git 配置里的外部程序），前两者也拒 `--no-index`。
- 子代理接着聊（复活）：被拒（角色不在了、workflow 派的）不再把子代理从内存里删掉；协调器回绝时收掉槽位并写明原因，不再挂着半活的 running；同一个 id 并发复活只起一次；用时和对话时刻接上第一次启动的；关着的线程按它自己的预设叫角色名。
- 只读场景（只读子代理、计划模式、explore / plan 角色）的命令判定没认反斜杠转义：`echo \"a | rm -rf x \"` 这类命令被判成只读，不问用户就会跑后面的命令。现在按 shell 的引号与转义规则切段；顺带引号里的 `>`（如 `grep -n "=>"`）不再被当成重定向。
- code-review 审 PR 时子代理为看 PR 版本的文件把仓库拷到磁盘上、一次审查弹几十次批准：PR 的提交在本地就用只读的 `git show <sha>:<路径>`，否则只看 diff，提示里写明只用只读命令。
- 在第 1 页以外的会话（GUI 的每个会话）起工作流，子代理、上报、收尾卡和收尾通知全落到第 1 页：GUI 什么都看不到，起它的会话也等不到结果。现在 run 归起它的那一页。
- `workflow` 的 `validate_only` 冒烟会调 host 函数的脚本（如内置 deep-research）时整个 Dock 崩溃（SIGABRT）：冒烟改到阻塞线程上跑，同正式运行。
- deep-research 几乎总报 Partial：研究员写的任何一条「不确定」都会让整份报告变 Partial（12 条结论全过核验也一样）。现在按问题覆盖评分，不确定之处只进覆盖说明。
  - 研究员和 verifier 只按来源核对，不再写文件、编译跑程序（每条都要停下等用户批准，实验也不是可引用的来源）。
  - verifier 多交 / 漏交一条 verdict 不再让它那一片结论全部作废；候选结论上限在问题间轮流分，后面的问题不再被挤掉；没覆盖的问题会点名。
- 自建预设（没有 `general-purpose` 角色）里工作流的每个 `agent()` 都以 `unknown subagent type general-purpose` 失败：没写角色时同 `task` 用预设的第一个角色；没有角色时说清怎么补。
- `workflow` 的 `args` 被模型序列化成字符串时脚本拿不到参数；`source` 漏写 `type` 被拒。现在都按意图读。
- GUI 里工作流卡在一轮跑着时一直挤在最下面，不在调用发生的位置。
- 一条工作流在跑时，再起第二条（别的会话或同一页）要等前一条跑完才拿到回执。
- 分页（`main#N`）派的子代理回报指令写的是 `main`，第一次 `send_message` 回话会被拒。
- 结束的工作流用时一直是 0（收尾通知里也是）。
- GUI 看不到会话里新写的预设：`preset/list` 以前只回启动时读的那份，现在每次现读盘，GUI 每轮结束后重拉。
  - `thread/start { presetId }` 按目标项目的 `.dock/presets` 校验，项目层预设能开会话了。

- 内置技能升级后不刷新：`$DOCK_HOME/bundled/skills/` 里已有的文件以前不会被新版覆盖，现在按内容比对重写，并删掉旧版留下的文件。

- 一轮在等权限时被停掉，那条请求会一直挂在队首：之后的权限弹窗（TUI、GUI、嵌入页）显示的是旧命令的摘要，
  点「允许」放掉的也是那条死请求。现在等的一方没了请求就撤出队列。
- 网关 `thread/start`（第 1 页开新会话）、`thread/archive`、`thread/restore` 先停下在跑的一轮再换：
  以前那一轮会接着跑进新会话，新消息只能排队。开新会话、归档也照 `/new` 收掉旧会话的子代理、目标和计划；嵌入页归档只在有排队消息时拦。
- `turn/start` / `turn/enqueue` / `turn/steer` 回的 `turnId` 和投影里的轮次一致（以前按 seq 猜，第二轮起对不上，
  嵌入页的截图预览会挂到别的消息上）。
- 嵌入页：开新会话 / 归档 / 恢复后不再显示旧会话的消息；`npm run dev:host` 的示例页能加载 `dock-embed.js`。
- MCP App：交给界面的工具输入拆掉 `use_tool` 那层；CSP 让 Worker 也认资源域名（CesiumJS 等地图能出图）。
- 白板清空后用同样的 id 重画，框里的标签不再丢。
- 握手途中停用 / 重连 / 改配置的 MCP 服务器，晚到的握手结果不再写回。
- **工具结果里的内联图计入上下文占用估算**：截图类工具（`browser_screenshot` 等）返回的图随历史每次请求内联重发，估算漏掉它，截图多的会话占用被低估、自动压缩迟迟不触发；压缩「摘要不比原文小」的判定也会少算原文。现在按张计 765 token；上下文面板把这一份从「消息」挪到「推理 / 开销」的「图片」行。
- **问答卡题多、字长时不再撑破会话区**：`ask_user_question` 一次问 3 道长题时，卡片比窗口还高，会话头栏被顶出去、「提交回答」和输入框跑到窗口外点不到。现在输入框上方那一列最多占会话区 75% 且可以被压矮，卡头和「提交回答」钉住、题目区卡内滚动，选项标题完整换行、长串可断行；侧边聊天里的问答卡同样限高。
- **输入框固定在会话区底部**：长对话里输入框被对话流按比例挤扁、掉出窗口（1500×900 下从 93px 压到 32px）。现在对话流填满头栏和输入区之间，输入框贴底，内容短时也不再跟着最后一条消息浮起来；侧边聊天同一套布局。
- 内置插件示例 guard / done-check 认 `write_file` 的 `target_file`（以前只读 `search_replace` 的 `file_path`，改冻结副本或改 `.rs` 不会触发）；欢迎页 Shift+Tab 提示与三档模式对齐。

### 破坏性变更（升级注意）

- 模型工具 `cordis_define` / `cordis_run` / `cordis_call` / `cordis_stop` / `cordis_promote` 退役：插件改成「一个目录」（`.dock/plugins/<id>/source.rhai`，`plugin.toml` 可选），写文件 → `cordis_plugin reload` → `use_tool` 验证。已有的 `.dock/plugins` 目录照常自动装载，不用迁移。网关去掉 `plugin/promote`、`plugin/discard`，`plugin/list` 去掉 `session` 字段；设置页去掉「动态插件」一栏。
- 插件作者：回合末 todo 提醒的第一行从 `Todos NOT complete — continue working. Next step:` 换成 `Todo bookkeeping only — update statuses.`，按旧开头匹配的 Rhai 钩子要改；`agent/turn-end` 载荷的 `ended_with_text` 恒为 `true`。Rust 侧 `Error` 新增 `Stationary { tool, run }`、`StreamDelta` 新增 `ToolInput` 变体：穷尽 `match` 且没有通配分支的要补一个分支（两者都没有 `#[non_exhaustive]`）。
- `$DOCK_HOME/schedules.json` 升到 version 2：旧版 Dock 读不了会报「解析失败」且不碰它（不会覆盖掉），升级回来照常；比本版更新的 version 也一样拒绝写回。

## [0.2.1] - 2026-10-13

### 新增

- 网关 `fs/dirs`（能力 `directoryPicker`）：列任意绝对路径下的子目录，给远程 GUI 选会话目录。
  - 只认受信连接；配对来的网页 `forbidden`。
- 网关 `fs/read { raw: true, offset?, length? }`（能力 `workspaceFilesRaw`）：分块原样读字节。
  - GUI 文件面板拿它渐进加载大表格、解析 xlsx / docx。
- 网关 `fs/write` / `fs/mkdir` / `fs/rename` / `fs/trash`（能力 `workspaceEdit`）：文件面板里直接改工作区。
  - 只认受信连接；保存带 `baseModifiedMs` 防覆盖 agent 的改动，删除是移到系统废纸篓。
  - 改完 agent 下一步会收到一条提醒（spine 插件 `user-edits`），不会拿旧内容覆盖。
- `fs/write` 收字节（`data` / 分块上传的 `uploadId`）+ `fs/upload`（能力 `workspaceEditBytes`）：GUI 改表格文件。
- **侧边聊天**（能力 `sideChat`）：从开着的会话分叉一张只读旁问页（`thread/aside/start | handback | merge`）。
  - 不落盘、不进会话列表、不占分页名额；结论经确认后写回主线。TUI 用 `/tab merge`。
- **插话**：一轮进行中的消息可以在下一步边界并进这一轮，不再停掉重来。
  - 网关 `turn/queue/steer` 把排队里的一条提前并入；TUI 生成中 `Ctrl+Enter` 是插话。
  - 发给正在跑的子代理的紧急消息走它自己的收件箱，不再取消重开。
- **`browser_js`**：用 Playwright 跑一段脚本（`page` / `state` / `log` / `snapshot` / `screenshot`）。
  - 脚本里可以用 `onDialog` 处理对话框；`tabs.list / get / new / select` 管多标签页。
  - `readPages` 用临时标签页同时读几个网址。`browser_navigate` 没有页面时会先打开一页。
- **`handoff_to_user`**：登录、验证码、二次验证时请用户在浏览器面板里接手，模型不硬闯。
- **目标模式**可以一直跑，不必隔一阵点继续。
  - 连续 3 次续跑都没做实事就暂停；每 50 次请求让模型对照完成标准。
  - `/goal budget 2h | 90m | 300 | off`，默认不限。

### 变更

- 设备令牌（`dock device add`）改为受信连接：能调设置页、插件面板等 `trusted_only` 方法。
  - 远程 GUI 的设置页改的是远端那台 Dock；令牌本来就能跑命令，不多给能力。
  - 配对来的网页仍然 `forbidden`。
- 纯文本模型不再收到用户或工具结果里的图片，正文里会说明图片被拿掉了。
- 停止一轮时，没执行的工具调用不进历史；正在跑的工具有 1 秒收尾，并记一条停止提示。
- 没改过名的会话标题去掉 Markdown 记号、只取第一行有字的内容。
  - 和旧规则完全一致的已保存标题，读盘时重算；自己改过的名字保持原样。实时页用同一套规则。
- 系统提示要求：说了要做的事就在同一次回复里调用工具。只针对被要求的事，越界或破坏性的下一步仍先问。
- `browser_file_upload` 把本机文件交给网页，现在和 `bash` 同级过权限门，弹窗列出要传的文件。
- 一次回复里彼此不依赖的只读调用并行执行，结果仍按原顺序记。
- 远程连接文档补了桌面 GUI 的 SSH 隧道和 systemd 用法。

### 破坏性变更（升级注意）

- `dock.1`：`turn/steer` 改为插话（到下一步边界再并入）。要停掉当前轮仍用原来的停止。

## [0.2.0] - 2026-10-06

### 新增（插件视图）

- **插件视图 `dock.view.1`**：插件（含 Rhai 动态插件）交一棵**声明式视图树**（JSON），TUI 与 GUI 各自画出来，插件不写 ratatui、不往 GUI 塞代码。契约见 `docs/PLUGIN-VIEWS.md`，完整示例 `examples/plugins/activity/`。
  - 节点：布局（`stack` / `row` / `section`）、文本（`text` / `markdown` / `code`）、数据（`kv` / `table` / `list`）、状态（`badge` / `progress`）、交互（`button` / `link`）、`divider` / `empty`。
  - 不认识的 `type` 画灰字占位、不报错；深度 8 层、节点 500 个、单串 20 000 字符封顶。
  - `link` 只放行 `http` / `https` / `mailto`，别的协议画成「已拦下不安全的链接」。
  - 终端只给可见的动作编号（折叠段里的不编号），按数字键 1–9 点。
- **插件面板 `surface/*`**（能力 `surfaces`）：面板不再只在终端里看得到。
  - slot 声明动作（`actions: [#{ id, label }]`），网关经 `surface/list | get | action` 投给 GUI；动作只认面板声明过的、且只给受信 ticket（动作会跑插件脚本）。
  - 推送 `surface/changed { id }`；Rhai `host.slot_changed(id)` 说「正文变了」。
  - GUI 插件面板可弹出成独立窗：不跟会话（一个面板全局一扇，适合挂副屏看进度）、头栏可置顶、插件自关面板时窗口跟着关。
- **状态项 `"status.items"`**：插件登记一个小状态（文字 + 色调）。
  - 终端画在底栏右侧快捷键条那行，放不下的折成「+N」；GUI 摆输入框工具栏右组最前，最多 3 个，多的收进「+N」。
  - `surface` 指向点击时要打开的面板；Rhai `host.set_status(#{ … })` 登记 / 改、`host.clear_status(id)` 去掉，包停了自动消失。
  - 网关 `status/list` + 推送 `status/changed`（只认受信 ticket）。
- **工具卡视图 `"tool.views"`**：按工具名登记渲染函数，拿这一次调用的参数、输出、成败，回一棵视图树。
  - 只给人看，不进模型历史；同名只能登记一个；经 `use_tool` 调的按需工具也认（按里面那颗的名字和参数找视图）。
  - 渲染函数要是纯函数（不调工具、不读会话），每次重画都可能再跑一遍。
  - 终端展开工具卡时头照旧、正文换成视图；GUI `tool/views` 列出有视图的工具，展开时按需 `tool/view`（只认受信 ticket），右上角「视图 | 原始」切换。
- **设置卡 `"plugin.settings"`**：插件声明配置项 schema（`string` / `text` / `number` / `boolean` / `select` / `secret`），界面按它生成表单。
  - 普通值存 `$DOCK_HOME/plugin-settings.json`，`secret` 存密钥库（界面拿不到原值，插件用 `host.secret(key)` 读）；写入按 schema 校验，有一个不合法整组不写。
  - 终端 `/cordis` 看当前值、`/cordis set <插件> <key> <值>` 改一项；GUI 设置 › 插件，`plugin/settings/list | get | set`（只认受信 ticket）。
- **插件 web 面板**：视图树画不了的交互（时间轴拖选、虚拟滚动、搜索框、图表）走自带的 HTML / JS。
  - 只有磁盘插件能用：`register_slot(#{ web: "panel.html" })`，文件必须在插件目录里（绝对路径、`..`、软链跑出去都让 `apply` 失败），每次现读、上限 2 MiB。
  - GUI 放进沙箱 iframe：只给 `allow-scripts`，碰不到 GUI 的 DOM / 存储 / Tauri IPC；文档 CSP 不许联网，脚本样式只能内联；和 Dock 只经 `postMessage` 桥说话（`threads.list` / `threads.history` / `threads.watch` / `panel.action`，加 `activeThread` 与设计 token）。
  - 网关 `surface/web { id }` → `{ html }`（只认受信 ticket，能力 `surfaceWeb`）；终端照旧画 `view()` / `render()`。
  - 示例 `examples/plugins/trajectory/`（会话轨迹：按轮次的记录表、计时总览、检查器、搜索、折叠、实时追加，纯 HTML/JS 无构建）。
- **示例插件「Agent 活动」**（`examples/plugins/activity/`）：面板 + 状态项 + 设置卡三件套的参考实现。
- **上下文明细 `thread/context/get`**（能力 `contextBreakdown`）：和 TUI `/context` 同一份数，按 system / tools / messages / overhead / free 分片，互不重叠、加起来是整个窗口；技能·工作流、规约·记忆作为所在片的 includes，MCP / 本地按需单列 `onDemand`；明细每组最多 40 行。
- **目标记下完成与用时**：`update_goal(completed)` 不再只是停跑——留着标题、把附的 message 记成最后一条进展，`environment.goal` 多 `status=completed` 与 `elapsedMs`（只算在跑的时段，暂停不计、完成停表）。

### 新增

- **撤回任意一条消息**：`dock.1` 新增 `thread/rewind`（能力 `threadRewind`）。
  - 这条和它之后的对话全部删掉（含落盘），正文和图片还回来，改完再发。
  - 正在跑先停下；撤回点早于压缩时压缩一并作废。
  - 工具已经做过的事（改过的文件、跑过的命令）不会撤销。
- **网关能列出项目的 GitHub PR**：`vcs/pr/list` / `vcs/pr/get`（能力 `pullRequests`），只读，经本机 `gh`。
  - 标出「我的」「请我 review」，汇总 CI 检查；详情含 review、改动量、逐项检查。
  - 没装 / 没登录 gh、不是 git 仓库、没有 GitHub 远端时回 `available: false` + 中文提示，不报错。
  - 从访达启动的桌面端也找得到 Homebrew 装的 gh（`DOCK_GH` 可指定）。
  - 列表一次 GraphQL 拿齐（原来三次往返），按项目缓存 60 秒；PR 没变时详情直接复用，`force` 强制刷新。
- **定时任务落盘、属于发起它的会话**：`/loop` / `scheduler_create` 建的任务存进 `$DOCK_HOME/schedules.json`，Dock 重启后继续跑。
  - 到点送进建它的那个会话；会话关着就在后台开成一页再送（以前一律送进第一页）。
  - 停机期间错过的不补跑；仍 7 天过期。多个 Dock 共用 `~/.dock` 时只有持有它的进程触发，持有者退出后由下一个进程接管。
  - 模型的 `scheduler_*` 只看得到、改得了本会话的任务。
  - 网关新增 `schedule/list | create | update | delete` 与连接级推送 `schedule/changed`（能力 `schedules`）。
- **桌面 GUI 设置页能直接改配置**：`dock.1` 新增一组设置方法（只给 `dock serve` 的父进程）。
  - 模型目录增删改、设默认、测试连接；MCP 服务增删改、开关、OAuth 登录。
  - 记忆、浏览器有头、`[toolset.web_fetch]` 的白名单键；CUA 安装 / 授权进度。
  - 插件启停 / 删除 / 写成永久，技能列表，插件密钥，网页配对审批，远程设备令牌。
  - 只写 `~/.dock/config.toml`，注释与不认识的键原样保留；文件解析失败时拒绝写。
  - 配对来的网页、远程设备调这些方法回 `forbidden`。
- 设置页「测试连接」按真实采样同样的地址 / 鉴权 / 上游模型名发一次最小请求。
- 设置页「插件」分永久插件（用户级 + 每个项目）与动态插件（会话里定义的）：启停、删除、立即加载、保存为永久、丢弃。

### 修复

- **Anthropic Messages 后端的 effort 档位实际上游**：以前只发 `thinking.adaptive`、不发 `output_config.effort`，`/effort` 设的档位对这条路径完全不起作用（Responses 后端一直发）。现在与 Responses 对齐：`Reasoning::On` 且 effort 非空且不是 `none` 时带 `output_config`；空 = 用上游默认，`none` = 两样都不发。
- **Rhai 插件的 `session/event` 回调按发出顺序逐条跑**：以前每条事件各开一个 `spawn_blocking`，同一颗插件的回调并发乱序——脚本里攒的列表顺序会乱、并发的 push 互相盖掉（40 条只剩 25–36 条）、「先改计数再刷状态项」的两步被插进来。现在一个登记一条队列、一个干活的任务，卸下时连同发送端一起丢掉。
- 上下文占用估算把工具调用的参数算了两遍（#176）：写大文件的会话估算虚高近一半，没有上游用量时会提前触发自动压缩。
- 上下文明细与总量对不上（#176）：总量是上游真账、分项是估算，分项加起来会超过总量。
  - 现在分项按比例缩进总量里，百分比不再超过 100%，工具定义不再显示 0。
- 工具输出过长时模型会重读看过的内容（#174）：截断提示只报字节，不报行号。
  - `bash`、`use_tool` 落盘后给模型的只有开头 2KB 预览（同 Claude Code）。
  - 提示写明全文行数、从第几行接着读（`read_file offset=…`）、看结尾用哪个 offset。
  - TUI 里看任务输出不受影响，仍保留结尾。
  - grep 搜到上限就停，不再把和内联一样的内容落盘、再叫模型去读。
  - grep 正文过长时，`at least N` 页脚不再被字节帽切掉。
- 桌面 GUI 里项目插件认不出来：项目级插件根用的是进程 cwd（GUI 的数据目录），现在跟着会话的项目目录。
  - 打开某个项目的会话时自动加载它的 `.dock/plugins`，和 TUI 从那里启动一样。
  - `cordis_promote` 存到项目级、`cordis_define` 的 `source_path` 也按会话的项目目录。
- 技能同样只认进程 cwd：桌面 GUI 里项目的 `skills/`、`.agents/skills/`、`.dock/skills/` 都看不到。
  - 现在 listing、`skill` 工具、`/name` 都按会话的项目目录找；设置页按项目分组列出。
- 多分页时技能的 reminder 写错页：在第 N 页 `/name` 会把正文追加到第 1 页的会话里。
- 两个 Dock 共用 `~/.dock` 时新建预设会撞 id：各自都起 `custom`，写进同一个目录互相覆盖。
  - 现在起 id 时在磁盘上原子占住目录，占不到换下一个。
- 设置页按规范化路径停用 / 删除插件时，经符号链接装上的插件对不上：停不掉、删了目录还在跑。
- `search_tool` 的描述把常驻的 memory / skill / workflow 说成按需工具，模型会白搜一次。

### 变更

- **「一切皆插件」改造收口**：会话提交口、分页、网关配对契约从 TUI 搬进 spine，无头 `dock serve`、网关、定时任务驱动不再为此依赖终端 UI；`tui()` 只跑事件循环，视图件由 `cordis-app` 组合挂载。
  - 斜杠命令进 `"slash"` 命令表（各功能插件登记，附加命令撞名直接失败）；网关 `slash/list | execute` 只做投影，不再自己实现一遍。终端专属命令由 TUI 的 `tui.commands` 登记，`dock serve` 不挂 TUI 时表里没有它们。
  - `dock.1` 功能方法是 `"gateway.methods"` 方法表：`vcs`、定时任务、设置页、预设、工作区文件、画布都改成往表里登记的插件，网关只留协议骨架；`MethodPolicy` 标 `detached` / `trusted_only` / `opens_thread`。方法表 API 对外导出，仓外插件也能加方法。
  - 分页的 inject 键由 `tui.tabs` 改为 `tabs`（仓内组合根与 `cordis-app` 测试已同步，磁盘插件配置不引用此键）。
- **常驻 / 按需工具改成在预设 YAML 里声明**：`agent.yml` / `agents/<id>.yml` 新增 `on_demand_tools`（写法同 `resident_tools`，常驻优先），调整不用改代码。
  - 内置 `code` / `cordis` 把 `canvas_*`、`workflow`、`kill_task`、`list_agents`、`interrupt_agent` 也设成按需；`code` 预设工具定义从约 7.3K token 降到约 5.4K。
  - `search_tool` 的描述按这一页实际的按需集合列出名字，模型按精确名搜一次就拿到 schema。
  - 守望（`warden`）不写这一项，调度工具全部常驻。旧的用户层覆盖没写这一项时沿用内置名单。
  - 网关 `preset/get` / `preset/update` 多一个 `onDemandTools`；不传保持原样。
- 工具描述去掉与参数说明重复的内容；`bash` 的 schema 不再登记 `block_until_ms`（仍然接受）。
- **浏览器工具改成内置 MCP**：`dock mcp browser`（新 crate `cordis-browser`）。
  - Dock 自动注入内置行 `[mcp_servers.browser]`，零配置；`DOCK_BROWSER_MCP=off` 关掉。
  - 工具公名从 `browser_*` 变成 `mcp_browser__browser_*`，照旧经 `search_tool` / `use_tool`。
  - 进程内的 `browser_*` 已删除；预设允许名单里不再列它们。
  - 权限门随之改名：`mcp_browser__browser_evaluate` 与 bash 同级。
- **按会话分标签页**：一个 Chromium、共用登录态，每个会话只看得到、只关得掉自己的标签页。
- **多个 Dock 共用一个 Chromium**：第二个进程读 `DevToolsActivePort` 连上已开的那个，不再抢 profile。
- `/browser` 驾驶舱只显示内置 MCP 的状态与有头 / 无头偏好，不再列标签页和截图。

### 新增

- **权限请求标出是哪个子代理发的**：`permission/requested` 带 `agentId`（主会话发的是 `null`）。
  - 客户端可以提供「始终允许」（`permission/resolve { always: true }`），子代理与派它的页共用。
  - TUI 权限浮层标题上方写「来自子代理 · 角色 · 任务」；embed-sdk 的权限卡标「来自子代理」。
- **子代理上网关**（能力 `subagents`）：GUI / 嵌入页能看子代理在干什么。
  - 推送 `subagent/updated`（状态）与 `subagent/event`（子代理自己的对话，包在父线程上）。
  - `subagent/list` / `history` 补中途接入；`subagent/send` / `interrupt` / `stop` 能插话、打断、收掉。
  - 子代理的过程只在内存里，Dock 重启后不保留。
- **压缩进展**：压缩那几秒看得到它在干什么，不再像卡住。
  - TUI 状态行：`正在压缩上下文 · 生成摘要 · 第 2/3 次（上次摘要过短）· 12s`，`↓` 是摘要已写的 token。
  - 完成那行：`✓ 上下文已自动压缩 · 182K → 21K`（前后占用只在实时显示）。
  - 网关：推送 `context/compacted`、完成标记 `item/compaction`、`context.lastCompaction`（能力 `compactionProgress`）。
  - 「已压缩上下文。」与「自动压缩失败：…」不再作为助手消息推给客户端。
  - embed-sdk：上下文浮层跟着 `context/compacted` 走——压缩中按钮显示「正在压缩」，完成显示前后占用，停止了也有说明。
- **预设常驻工具** `resident_tools`：写全名或 `前缀*`（如 `mcp_browser__*`）。
  - 藏在 `search_tool` 后面的工具（MCP、按需、动态包）直接进模型工具表。
  - 网关 `preset/get|update` 多了 `residentTools`（不传 = 保持原样）；`tool/catalog {includeMcp:true}` 带 MCP 行。
- **网关浏览器画面** `browser/view/*`（能力 `browserView`）：看会话正在用的标签页、接手操作。
  - CDP screencast 推 JPEG 帧；鼠标 / 滚轮 / 按键 / 文字转回页面；地址栏、前进后退刷新。
  - agent 换标签页画面跟着换；帧写出去才让 Chrome 发下一帧。
  - 本地、远程、网页端同一条路，不需要客户端碰 CDP 端口。
- **画布**：模型写自包含 HTML，Dock 桌面端在会话旁的沙箱 iframe 里跑。
  - 工具 `canvas_create` / `canvas_edit`（局部替换或整页重写，每次一版）/ `canvas_data`（只换数据）/ `canvas_read`。
  - 落在会话目录 `canvas/<id>/`：`meta.json` + `v<n>.html` + `data.json`。
  - 网关 `canvas/list` / `get` / `setData` / `rollback`（能力 `canvas`）。
- **网关工作区文件** `fs/list` / `fs/read` / `fs/find`（能力 `workspaceFiles`）：只读，给文件面板。
  - 路径相对会话 cwd，出了目录就拒；照 `.gitignore`，默认不列点文件。
  - 文本截在字符边界；常见图片回 base64；其它二进制只给大小。
- **工具结果里的图进 dock.1**（能力 `toolImages`）：`item/tool_completed.attachments` + `item/image`。
  - 以前网关把工具截图丢了，客户端看不到 agent 看到的画面（cua-driver 截图）。
  - dock-core 的工具项多了 `images`。

### 新增（远程）

- **`dock serve --remote`**：给别的机器上的 GUI / 浏览器 UI 连的常驻网关。
  - 仍只绑回环（默认 `127.0.0.1:18990`，占用直接报错）；TLS 交给反向代理或 Tailscale。
  - 只认设备令牌，不挂配对与 ticket 的 HTTP 路由；跑到 SIGTERM 为止。
- **设备令牌** `dock device add|list|revoke`：盘上只存 sha256；撤销立刻生效并断开连接。
  - 同一条连接鉴权失败 5 次断开。
- SDK：`DockClient({ deviceToken, gatewayUrl: 'https://…' })`；令牌模式对非回环地址强制 https。
- 部署说明：`docs/REMOTE.md`（Caddy / nginx / systemd / Tailscale）。

### 修复

- **子代理回报后主代理不再被多叫醒一轮**（#166）：回报过的回合结束通知不叫醒父级，下一次采样照样带上。
- **分页派的子代理回话回到派活的那一页**：以前全落到第 1 页，派活的会话（GUI 里的每个会话）等不到汇总，第 1 页还拿别人的回报自己跑一轮。
- **推理模型自动压缩不触发**（#154）：占用估算漏了回放给上游的推理，上下文撞满窗口也不压。
  - 现在以上游报的 prompt + completion 为锚点，之后追加的内容按估算补上（同 Grok / Codex）。
  - 压缩后不再沿用压缩前的用量，长工具循环里第二次写满窗口也会再压。
  - `/resume` 换会话不再带着上一个会话的占用。
  - 顶栏 / `/context` 显示的就是判定用的那个数。
- 网关浏览器画面：同一会话连发 `browser/view/open` 不再悄悄关掉已回给客户端的那个视图。
  - 最后收到的留下；被顶掉的推 `closed { reason: "replaced" }`，开到一半就过时的回 `superseded`。
- 网关浏览器画面：同一视图连发的 `browser/view/input` 按收到的顺序进页面（以前按下 / 抬起、连打的字会乱序）。
- MCP 工具的 `isError: true` 现在会把结果标成失败；以前只有正文以 `Error:` 开头才算。

### 破坏性变更（升级注意）

- 浏览器工具改成内置 MCP 后改了名：`browser_*` → `mcp_browser__browser_*`。
  - 写死旧名的提示词、预设、`resident_tools` 要改；常驻可写 `mcp_browser__*`。
  - 权限门 `browser_evaluate` → `mcp_browser__browser_evaluate`；预设允许名单不再列 `browser_*`（MCP 工具本就不走允许名单）。
  - 不想要内置浏览器：`DOCK_BROWSER_MCP=off`。
- `dock.1`：「已压缩上下文。」与「自动压缩失败：…」不再作为助手消息推送，客户端改看 `context/compacted` / `item/compaction`。
- `dock.1`：`dock serve` 不挂 TUI 时，`slash/list` 里没有终端专属命令。
- 模型的 `scheduler_*` 只看得到、改得了本会话建的定时任务。
- Rust 组合 API（只影响仓外自己组装插件树的人）：
  - 分页的 inject 键 `tui.tabs` → `tabs`（事件 `tabs/page-opened`）；`SessionPort` / `GatewayPort` / `Tabs` 等搬到 `cordis_spine::host`。
  - `tui()` 不再自挂视图件，要按 `cordis_tui::views()` 逐个挂（参照 `cordis-app`）。
  - spine 去掉 `BrowserSession` / `BrowserTabInfo` / `BROWSER_TOOL_NAMES`，改用 `BrowserState` / `BROWSER_MCP_PREFIX`。

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
