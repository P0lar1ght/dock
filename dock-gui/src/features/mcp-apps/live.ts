// 模型还在写工具参数时（dock.1 推送 `item/tool_input_delta`，能力 `toolInputStream`）：
// 拼起来、补全成参数对象，交给右侧面板里的界面（规范 ui/notifications/tool-input-partial）。
//
// - 每个会话一份：toolCallId → 还在写的流（dock-core toolInput.ts 拼接、补全）；
// - 节流：同一个会话 80ms 最多交一次（补全要从头扫一遍参数，别每段都扫）；
// - 调用开始（`item/tool_started`）就交给工具卡（InlineApp.tsx 照旧送完整参数和结果），这里拿掉；
// - 一轮结束还没开始的调用（`llm/stream` 改写了、采样中途失败、被停下）：作废，界面收 tool-cancelled。

import { feedToolInput, parseEvent, type ToolInputStream } from "dock-core";
import { onForegroundNotification } from "../../dock/client";

export type LiveUpdate =
  | { kind: "input"; stream: ToolInputStream }
  /** 调用开始了：之后归工具卡。 */
  | { kind: "started"; stream: ToolInputStream }
  /** 这一轮结束了还没开始：作废。 */
  | { kind: "dropped"; stream: ToolInputStream };

type Listener = (update: LiveUpdate) => void;

const FLUSH_MS = 80;

/** threadId → (toolCallId → 流)。 */
const streams = new Map<string, Map<string, ToolInputStream>>();
/** threadId → 等着交出去的 toolCallId。 */
const dirty = new Map<string, Set<string>>();
const timers = new Map<string, number>();
const listeners = new Map<string, Set<Listener>>();

/** 订一个会话里还在写的调用。返回取消函数。 */
export function subscribeLive(threadId: string, fn: Listener): () => void {
  let set = listeners.get(threadId);
  if (!set) listeners.set(threadId, (set = new Set()));
  set.add(fn);
  return () => {
    set.delete(fn);
    if (set.size === 0) listeners.delete(threadId);
  };
}

function notify(threadId: string, update: LiveUpdate) {
  for (const fn of listeners.get(threadId) ?? []) fn(update);
}

function flush(threadId: string) {
  timers.delete(threadId);
  const ids = dirty.get(threadId);
  dirty.delete(threadId);
  const live = streams.get(threadId);
  if (!ids || !live) return;
  for (const id of ids) {
    const stream = live.get(id);
    if (stream && !stream.broken) notify(threadId, { kind: "input", stream });
  }
}

function forget(threadId: string, toolCallId: string) {
  streams.get(threadId)?.delete(toolCallId);
  dirty.get(threadId)?.delete(toolCallId);
}

onForegroundNotification((n) => {
  if (n.method !== "item/tool_input_delta" && n.method !== "item/tool_started" && n.method !== "turn/completed") return;
  const threadId = typeof n.params.threadId === "string" ? n.params.threadId : "";
  const event = threadId ? parseEvent(n.method, n.params) : null;
  if (!event) return;
  if (event.method === "item/tool_input_delta") {
    let live = streams.get(threadId);
    if (!live) streams.set(threadId, (live = new Map()));
    live.set(event.toolCallId, feedToolInput(live.get(event.toolCallId), event));
    let ids = dirty.get(threadId);
    if (!ids) dirty.set(threadId, (ids = new Set()));
    ids.add(event.toolCallId);
    if (!timers.has(threadId)) timers.set(threadId, window.setTimeout(() => flush(threadId), FLUSH_MS));
    return;
  }
  if (event.method === "item/tool_started") {
    const stream = streams.get(threadId)?.get(event.toolCallId);
    forget(threadId, event.toolCallId);
    if (stream) notify(threadId, { kind: "started", stream });
    return;
  }
  // 被插话截开的那一段不是真的结束：这一轮接着跑。
  if (event.method !== "turn/completed" || event.steered) return;
  const live = streams.get(threadId);
  streams.delete(threadId);
  dirty.delete(threadId);
  for (const stream of live?.values() ?? []) if (!stream.broken) notify(threadId, { kind: "dropped", stream });
});
