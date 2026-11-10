// MCP Apps 的 dock.1 方法（cordis-gateway/README.md「MCP Apps」）。外形由 dock-core 解析。
// 全部只认受信 ticket（GUI 有）。旧版 Dock 没有能力位 `mcpApps`：工具表当空的。

import {
  looksLikeMcpTool,
  parseMcpAppCallRecord,
  parseMcpAppResource,
  parseMcpAppTools,
  parseViewCall,
  type McpAppCallRecord,
  type McpAppResource,
  type McpAppTool,
  type McpAppViewCall,
  type McpAppViewTool,
} from "dock-core";
import { dock, RpcError } from "../../dock/client";
import { bumpToolCards } from "../registry";

export const supported = () => !!dock.capabilities.mcpApps;

export async function appTools(): Promise<McpAppTool[]> {
  await dock.connect();
  return supported() ? dock.call("mcp/app/tools").then(parseMcpAppTools) : [];
}

export async function appResource(server: string, uri: string): Promise<McpAppResource> {
  const r = parseMcpAppResource(await dock.call("mcp/app/resource", { server, uri }));
  if (!r) throw new Error("界面资源是空的");
  return r;
}

/** 界面读自己服务器的资源（视频、数据……），原样的 ReadResourceResult。旧版 Dock 没有这个方法时回 null。 */
export async function appRead(server: string, uri: string): Promise<Record<string, unknown> | null> {
  try {
    return await dock.call<Record<string, unknown>>("mcp/app/read", { server, uri });
  } catch (e) {
    if (e instanceof RpcError && e.code === "method_not_found") return null;
    throw e;
  }
}

export const appCall = (threadId: string, server: string, tool: string, args: Record<string, unknown>) =>
  dock.call<Record<string, unknown>>("mcp/app/call", { threadId, server, tool, arguments: args });

export const appResult = (threadId: string, toolCallId: string): Promise<McpAppCallRecord | null> =>
  dock.call("mcp/app/result", { threadId, toolCallId }).then(parseMcpAppCallRecord);

export const appContext = (
  threadId: string,
  server: string,
  resourceUri: string,
  content: unknown[] | undefined,
  structuredContent: Record<string, unknown> | undefined,
) => dock.call("mcp/app/context", { threadId, server, resourceUri, content: content ?? null, structuredContent: structuredContent ?? null });

// ---- 界面自己的工具（规范 appCapabilities.tools）：报给 Dock，模型调用时交给界面跑 ----

/** 连着的 Dock 认不认界面工具（旧版没有能力位 `mcpAppViewTools`）。 */
export const viewToolsSupported = () => !!dock.capabilities.mcpAppViewTools;

/** 报（替换）一个界面实例的工具；`tools` 空 = 收回。Dock 按这条连接记，断开时一并收回（重连后要重报）。 */
export const appViewExpose = (p: { threadId: string; viewId: string; server: string; resourceUri: string; tools: McpAppViewTool[] }) =>
  dock.call<{ tools: string[] }>("mcp/app/view/expose", p);

type ViewHandler = (call: McpAppViewCall) => Promise<Record<string, unknown>>;
const viewHandlers = new Map<string, ViewHandler>();

/** 这个界面实例来接 Dock 推来的调用；返回取消函数。 */
export function handleViewCalls(viewId: string, fn: ViewHandler): () => void {
  viewHandlers.set(viewId, fn);
  return () => {
    if (viewHandlers.get(viewId) === fn) viewHandlers.delete(viewId);
  };
}

// Dock 推来的调用：是本窗口的界面就交给它，结果（或一句原因）送回去。别的窗口的界面不管。
dock.onNotification((n) => {
  if (n.method !== "mcp/app/view/call") return;
  const call = parseViewCall(n.params);
  const fn = call && viewHandlers.get(call.viewId);
  if (!call || !fn) return;
  fn(call).then(
    (result) => dock.call("mcp/app/view/result", { callId: call.callId, result }),
    (e: unknown) => dock.call("mcp/app/view/result", { callId: call.callId, error: e instanceof Error ? e.message : String(e) }),
  ).catch(() => {});
});

// ---- 带界面的工具表：全局一份 ----

let tools: McpAppTool[] = [];
const listeners = new Set<() => void>();
let loadedAt = 0;
let loading: Promise<void> | null = null;

function load() {
  if (loading) return loading;
  loading = appTools()
    .then(
      (list) => {
        tools = list;
        listeners.forEach((fn) => fn());
        // 对话流里哪些调用算界面产物（不折进「用时」）跟着工具表变。
        bumpToolCards();
      },
      () => {},
    )
    .finally(() => {
      loadedAt = Date.now();
      loading = null;
    });
  return loading;
}

export function subscribeTools(fn: () => void) {
  if (loadedAt === 0) void load();
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export const getTools = () => tools;

/**
 * 遇到不认识的 `mcp_*` 工具：服务器可能刚连上、刚换了工具表，重拉一次（10 秒内不重复）。
 * 网关不推工具表的变化。
 */
export function noticeTool(toolName: string, args: unknown) {
  if (!looksLikeMcpTool(toolName, args)) return;
  if (Date.now() - loadedAt < 10_000) return;
  void load();
}

// 重连之后重拉：Dock 可能重启过，服务器和工具表都换了。
dock.onState((state) => {
  if (state === "open" && loadedAt !== 0) void load();
});
