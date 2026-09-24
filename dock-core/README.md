# dock-core

dock.1 协议核心：线程事件的类型化契约 + 线程状态 reducer。纯 TypeScript，无 DOM、无运行时依赖。任何 dock.1 客户端都在它上面做展示。

## 为什么单独一层

网关投影出的线程事件（`cordis-gateway/src/transcript.rs` 里每个 `push`）以前只有松散的 `Record<string, unknown>`，各客户端各自解析。这里把它写成一份类型（`src/events.ts`），状态怎么随事件变化也只写一份（`src/thread.ts`）。

- **保真**：工具的原始参数、完整输出都留着。脱敏、截断是展示层的事（embed-sdk 嵌进第三方页面要脱敏，就在自己那层做）。
- **按轮组织**：`Turn { status, error, startedAt, endedAt, items }`，`items` 按出现顺序排：用户消息、助手文字、工具、权限、提问、计划、elicitation。
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
