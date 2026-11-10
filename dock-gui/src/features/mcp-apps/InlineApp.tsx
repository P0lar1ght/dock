// 工具卡下面的 MCP App 入口。
//
// - 界面一律在右侧面板：调用发生就自动打开/更新面板，用户不用点；
// - 消息流里不再摊内联界面（iframe 有 420 高，摊在流里会被后面的消息顶下去，
//   而且产物卡带 keepMounted 时它排在这一轮过程的末尾——界面于是「一直显示在下方」）；
// - 工具卡本身留在真实调用的位置（不挪到这一轮末尾，见 features/mcp-apps/index.ts）。

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { PanelRight } from "lucide-react";
import { appToolArgs, appToolFor, type McpAppTool, type ToolStatus } from "dock-core";
import { store } from "../../dock/store";
import { useFeatureHost, type ToolAttachmentProps } from "../registry";
import { appResult, getTools, noticeTool, subscribeTools } from "./api";
import type { FrameCall } from "./AppFrame";
import { focusRemote, panelKind, route, seed, sinkKey, useSinkOpen, type RoutedCall } from "./routing";

/** 这张工具卡挂的界面；不是 MCP App 工具回 null。 */
export function useAppTool(toolName: string, args: Record<string, unknown>): McpAppTool | null {
  const tools = useSyncExternalStore(subscribeTools, getTools);
  const tool = appToolFor(tools, toolName, args);
  useEffect(() => {
    if (!tool) noticeTool(toolName, args);
    // 名字变了才重新看一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tool, toolName]);
  return tool;
}

/** 一次调用交给界面的样子：完成了就去拿 Dock 存下的原始结果。没有调用回 null。 */
export function useFrameCall(
  c: { threadId: string; itemId: string; args: Record<string, unknown>; status: ToolStatus } | null,
): FrameCall | null {
  const [result, setResult] = useState<{ key: string; value: Record<string, unknown> | null } | null>(null);
  const threadId = c?.threadId ?? "";
  const itemId = c?.itemId ?? "";
  const completed = c?.status === "completed";
  useEffect(() => {
    if (!completed) return;
    let alive = true;
    appResult(threadId, itemId).then(
      (r) => alive && setResult({ key: itemId, value: r?.result ?? null }),
      () => {},
    );
    return () => {
      alive = false;
    };
  }, [completed, threadId, itemId]);
  if (!c) return null;
  const value = result?.key === c.itemId ? result.value : null;
  const cancelled =
    c.status === "cancelled" ? "已中断" : c.status === "denied" ? "用户拒绝了这次调用" : c.status === "failed" ? "调用失败" : null;
  return { key: c.itemId, args: c.args, result: value, cancelled };
}

/** 页面打开的时间：比它早开始的是历史里的调用。 */
const OPENED_AT = Date.now();

export function InlineApp({ threadId, itemId, toolName, args, status, startedAt }: ToolAttachmentProps) {
  // 这一轮刚发生的 = 页面打开之后才开始的。只看开始时间：回放历史时调用也会先经过「在跑」
  // 再到「完成」，按状态判会把历史里的调用也算成刚发生（面板会被反复重开）。
  const live = useRef(startedAt !== undefined ? startedAt >= OPENED_AT : status === "running");
  const tool = useAppTool(toolName, args);
  if (!tool?.resourceUri) return null;
  // 交给界面的是那颗工具自己的参数：经 use_tool 调的拆掉外面那层（第三方 App 实测出来的）。
  return (
    <InlineAppFor
      threadId={threadId}
      itemId={itemId}
      args={appToolArgs(toolName, args)}
      status={status}
      tool={tool}
      uri={tool.resourceUri}
      live={live.current}
    />
  );
}

function InlineAppFor({
  threadId,
  itemId,
  args,
  status,
  tool,
  uri,
  live,
}: {
  threadId: string;
  itemId: string;
  args: Record<string, unknown>;
  status: ToolStatus;
  tool: McpAppTool;
  uri: string;
  /** 这一轮刚发生的：自动在右侧打开面板。 */
  live: boolean;
}) {
  const host = useFeatureHost();
  const cwd = store.sessionCwd(threadId);
  const key = sinkKey(tool.server, uri, cwd);
  // 面板开在本窗（"here"）或拖出去的独立窗（"window"）。
  const opened = useSinkOpen(key);
  const inPanel = opened !== null;
  const routed: RoutedCall = { threadId, itemId, tool, args, status };
  const routedRef = useRef(routed);
  routedRef.current = routed;

  const openInPanel = () => {
    // 已经拖成独立窗了：聚焦那扇窗，不在本窗再开一份。
    if (focusRemote(key, routedRef.current)) return;
    seed(key, routedRef.current);
    host.openPanel(panelKind(tool.server, uri));
  };

  // 面板开着：这一轮的调用都送进面板（状态每变一次送一次，面板按 itemId 认）。
  useEffect(() => {
    if (inPanel && live) route(key, routedRef.current);
  }, [inPanel, key, status, live]);

  // 调用发生就自动在右侧打开：界面是给用户看的，不该等人点。
  const openedOnce = useRef(false);
  useEffect(() => {
    if (!live || inPanel || openedOnce.current) return;
    openedOnce.current = true;
    if (focusRemote(key, routedRef.current)) return;
    seed(key, routedRef.current);
    host.openPanel(panelKind(tool.server, uri));
  }, [live, inPanel, key, host, threadId, itemId, tool, uri]);

  if (inPanel) {
    return (
      <button
        type="button"
        onClick={openInPanel}
        className="motion-colors enter-fade ml-[26px] mt-0.5 flex h-8 w-fit items-center gap-2 rounded-lg border border-line-strong bg-white px-2.5 text-[12px] text-ink-3 hover:bg-hover"
      >
        <PanelRight size={13} className="text-accent" />
        {opened === "window"
          ? live
            ? "已在面板窗口中更新"
            : "在面板窗口中查看这次结果"
          : live
            ? "已在右侧面板中更新"
            : "在右侧面板中查看这次结果"}
        <span className="text-ink-5">{tool.server}</span>
      </button>
    );
  }
  return (
    <button
      type="button"
      onClick={openInPanel}
      className="motion-colors enter-fade ml-[26px] mt-0.5 flex h-8 w-fit items-center gap-2 rounded-lg border border-line-strong bg-white px-2.5 text-[12px] text-ink-3 hover:bg-hover"
    >
      <PanelRight size={13} className="text-accent" />
      在右侧面板打开
      <span className="text-ink-5">{tool.server}</span>
    </button>
  );
}
