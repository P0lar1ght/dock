# dock-core

dock.1 协议核心：线程事件的类型化契约 + 线程状态 reducer。纯 TypeScript，无 DOM、无运行时依赖。任何 dock.1 客户端都在它上面做展示。

## 为什么单独一层

网关投影出的线程事件（`cordis-gateway/src/transcript.rs` 里每个 `push`）以前只有松散的 `Record<string, unknown>`，各客户端各自解析。这里把它写成一份类型（`src/events.ts`），状态怎么随事件变化也只写一份（`src/thread.ts`）。

- **保真**：工具的原始参数、完整输出都留着。脱敏、截断是展示层的事（embed-sdk 嵌进第三方页面要脱敏，就在自己那层做）。
- **按轮组织**：`Turn { status, error, startedAt, endedAt, items }`，`items` 按出现顺序排：用户消息、助手文字、工具、权限、提问、计划、elicitation。
- 工具项带 `images`（结果里的截图等，只有元数据）；像素用 `item/image` 按需取。
- 压缩：`item/compaction` 是轮里的 `compaction` 项；`context/compacted` 是线程级的
  `ThreadState.compaction`（只推不记，`seq` 为 0）。
  - 完成推送把发起方、前后占用、用时补到最近那个标记上；回放历史时它们是 `null`。
  - 连压两次只有一个标记（Dock 不重复追加），它跟着最新那次覆盖。
  - 没压成 / 被停掉的进展在下一轮 `turn/started` 时清掉。
- 权限项带 `agentId`：子代理发的请求是它的 id，主会话发的是 `null`（旧网关不带也是 `null`）。
- 子代理（`src/subagents.ts`）：`subagent/updated` 是状态，`subagent/event` 是它自己的对话。
  - 每个子代理的对话用同一个 `reduceThread`；父线程的 `reduceThread` 忽略这两种事件。
  - 中途接入时对话不全（`complete: false`），实时事件先攒着；`withHistory` 回放
    `subagent/history` 再接上。
  - `taskPrompt` 去掉 Dock 给任务加的开头和回报说明。
- 插件给界面的东西（`src/plugins.ts`，契约见 `docs/PLUGIN-VIEWS.md`）：
  - `parseView`：视图树 dock.view.1，规范化与 `cordis-base/src/view.rs` 一一对应
    （深度 8、节点含条目 500、文字 20000，超出截掉补一行说明；链接只放行 http / https / mailto）。
  - `viewActions`：能点的动作，默认跳过折叠的 `section`。
  - `splitPanelFooter`：根 `stack` 最后一行全是按钮 → 固定在面板底部。
  - 面板 `surface/*`、状态项 `status/list`、工具卡 `tool/views` / `tool/view` 的结果解析；
    `viewToolName` 透过 `use_tool` 找视图（同 spine 的 `effective_call`）。
  - `parsePluginPush`：`surface/changed`、`status/changed`、`tool/views/changed`（连接级，不用订阅）。
- 插件设置卡（`src/pluginSettings.ts`）：`plugin/settings/*` 的结果解析；
  `checkSettingsField` / `settingsErrors` 与 `cordis-base/src/plugin_settings.rs` 同一套校验与文案，
  界面先标错、禁用保存，最终以 Dock 回的 `errors` 为准。
- **不猜**：一轮的结果看 `turn/completed.status`，工具的结果看 `item/tool_completed.status`，都由 Dock 给出。

## 用法

```ts
import { parseEvent, reduceThread, replayHistory, isRunning, pendingItems } from 'dock-core';

let state = replayHistory(history.events);          // thread/history（开着、关着的会话同一种）
const event = parseEvent(note.method, note.params); // 实时推送；认不出的方法返回 null
if (event) state = reduceThread(state, event);      // seq 去重，没变的轮次保持同一引用
```

## 命令

```bash
npm ci
npm test          # node --test（Node 22.18+ 直接跑 .ts）
npm run typecheck
```

## 约定

- 协议只做加法：新方法先在网关投影，再在 `events.ts` 加一支；`parseEvent` 对认不出的方法返回 `null`。
- 改了网关某个事件的载荷，同步改 `events.ts` 的类型与解析，并补测试。
