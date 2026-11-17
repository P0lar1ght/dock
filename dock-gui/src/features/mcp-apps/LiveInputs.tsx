// 右侧面板跟着模型正在写的调用走：模型一开始写某颗 MCP App 工具的参数，就把那个界面的面板
// 打开（或切过去），参数边写边交给界面（tool-input-partial），看上去就是模型在界面里实时操作。
//
// 挂在当前显示的会话上（registerThreadWatcher），不画东西。调用开始以后归工具卡（InlineApp.tsx），
// 同一个 itemId 再送一遍完整参数和结果。

import { useEffect, useRef } from "react";
import { liveToolCall } from "dock-core";
import { store } from "../../dock/store";
import { useFeatureHost, type FeatureHost } from "../registry";
import { getTools, noticeTool } from "./api";
import { subscribeLive, type LiveUpdate } from "./live";
import { focusRemote, panelKind, reseed, route, seed, sinkKey, type RoutedCall } from "./routing";

/**
 * 已经替它自动开过面板的调用：用户在它写参数时把面板关了，就别再给它开。
 * 键带会话：没给调用 id 的供应商每次都叫 `call-0`，不带会话会在会话之间串。
 */
const opened = new Set<string>();
const openedKey = (threadId: string, toolCallId: string) => `${threadId}\n${toolCallId}`;

export function LiveInputs({ threadId }: { threadId: string }) {
  const host = useFeatureHost();
  const hostRef = useRef(host);
  hostRef.current = host;
  useEffect(() => {
    const off = subscribeLive(threadId, (update) => deliver(threadId, update, hostRef.current));
    return () => {
      off();
      // 切走这个会话：之后收不到它的 started / dropped，记下的一并清掉。
      for (const key of [...opened]) if (key.startsWith(`${threadId}\n`)) opened.delete(key);
    };
  }, [threadId]);
  return null;
}

function deliver(threadId: string, update: LiveUpdate, host: FeatureHost) {
  // 开始了：之后归工具卡（InlineApp.tsx）送完整参数和结果。
  if (update.kind === "started") {
    opened.delete(openedKey(threadId, update.stream.toolCallId));
    return;
  }
  const call = liveToolCall(update.stream);
  if (!call) return;
  const tool = getTools().find((t) => t.publicName === call.name && t.resourceUri !== null);
  if (!tool?.resourceUri) {
    // 可能是刚连上的服务器：重拉一次工具表（不是 MCP 工具的不拉）。
    noticeTool(call.name, {});
    return;
  }
  const itemId = update.stream.toolCallId;
  const mark = openedKey(threadId, itemId);
  const key = sinkKey(tool.server, tool.resourceUri, store.sessionCwd(threadId));
  const routed: RoutedCall = {
    threadId,
    itemId,
    tool,
    args: call.args,
    status: update.kind === "dropped" ? "cancelled" : "running",
    partial: true,
  };
  if (update.kind === "dropped") {
    opened.delete(mark);
    route(key, routed);
    return;
  }
  if (route(key, routed)) return;
  if (opened.has(mark)) {
    // 面板还在打开：种子换成最新的样子。
    reseed(key, routed);
    return;
  }
  opened.add(mark);
  // 已经拖成独立窗：聚焦那扇窗，不在本窗再开一份。
  if (focusRemote(key, routed)) return;
  seed(key, routed);
  host.openPanel(panelKind(tool.server, tool.resourceUri));
}
