// 常驻的 MCP App 面板（UI Plugin）：右侧面板槽里的一种，kind `mcp-app:<server>:<uri>`。
// - 按项目一份：项目取当前会话的 cwd；换到别的项目的会话，界面换成那个项目的那一份（重载）。
// - 这个项目里挂同一界面的调用（内联卡送过来的）陆续交给同一个界面，用户和 Agent 在同一处改。
// - 界面里的调用、发消息、交上下文都记在当前会话上。
// 头栏同插件面板：拖手 · 图标 + 名字 … 分屏 · 新窗口 | 关闭。
// 拖成独立窗后跟着拖出时的会话（项目随之定下）；主窗口的调用经 routing.ts 的跨窗口事件送过来。

import { lazy, Suspense, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { AppWindow, X } from "lucide-react";
import { appKey, type McpAppTool } from "dock-core";
import { IconButton } from "../../components/chrome";
import { ToolRow } from "../../components/ToolPanel";
import { store } from "../../dock/store";
import type { PanelProps, PanelEntryGroupProps, SplitGroupProps } from "../registry";
import { getTools, subscribeTools } from "./api";
import { useFrameCall } from "./InlineApp";
import { panelKind, parsePanelKind, registerSink, sinkKey, type RoutedCall } from "./routing";

// 官方 AppBridge（含 zod、MCP 客户端）第一次用到界面才加载，不进启动包。
const AppFrame = lazy(() => import("./AppFrame").then((m) => ({ default: m.AppFrame })));

export function AppPanel({ kind, sessionId, onClose, lead, controls, detached }: PanelProps) {
  const target = parsePanelKind(kind);
  const cwd = sessionId ? store.sessionCwd(sessionId) : null;
  if (!target) return null;
  // 独立窗刚开、会话列表还没到：项目未知，先别挂界面（不然先按「没有项目」登记一份）。
  const waiting = detached && !!sessionId && !cwd;
  return (
    <aside className="glass-pane enter-fade relative flex min-h-0 min-w-0 flex-1 flex-col bg-[#fafafa]">
      <header className="flex h-11 shrink-0 items-center gap-2 border-b border-[#ececf0] px-3" data-tauri-drag-region>
        {lead}
        <AppWindow size={16} className="shrink-0 text-ink-3" />
        <span className="min-w-0 truncate text-[13px] font-semibold text-ink">{target.server}</span>
        <span className="min-w-0 truncate text-[11px] text-ink-4">{cwd ? cwd.split("/").pop() : ""}</span>
        <div className="min-w-2 flex-1" data-tauri-drag-region />
        {controls}
        <span className="mx-0.5 h-4 w-px bg-line" />
        <IconButton icon={X} label={`关闭${target.server}`} size={14} onClick={onClose} className="size-7 text-ink-4" />
      </header>
      {/* 换项目 = 换一份界面。 */}
      {waiting ? (
        <div className="m-4 h-24 animate-pulse rounded-lg bg-fill" />
      ) : (
        <PanelBody key={`${cwd ?? ""}`} kind={kind} server={target.server} uri={target.uri} cwd={cwd} sessionId={sessionId} />
      )}
    </aside>
  );
}

function PanelBody({
  kind,
  server,
  uri,
  cwd,
  sessionId,
}: {
  kind: string;
  server: string;
  uri: string;
  cwd: string | null;
  sessionId: string | null;
}) {
  const [routed, setRouted] = useState<RoutedCall | null>(null);
  useEffect(() => registerSink(sinkKey(server, uri, cwd), setRouted, { kind, sessionId }), [kind, server, uri, cwd, sessionId]);
  const tools = useSyncExternalStore(subscribeTools, getTools);
  // 菜单里直接打开、还没有调用：拿这个界面的第一颗带界面的工具当 toolInfo。
  const fallback = useMemo<McpAppTool | null>(
    () => tools.find((t) => t.server === server && t.resourceUri === uri) ?? null,
    [tools, server, uri],
  );
  const call = useFrameCall(routed);
  // 界面自己发起的调用、上下文记在当前会话上（同一项目）；还没选会话时记在送调用来的那个会话上。
  const threadId = sessionId ?? routed?.threadId ?? null;
  // 一直是同一个界面：从菜单打开后 Agent 再调，界面不重建，只交给它这次调用。
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <Suspense fallback={<div className="h-24 animate-pulse rounded-lg bg-fill" />}>
      <AppFrame
        server={server}
        uri={uri}
        threadId={threadId}
        tool={routed?.tool ?? fallback}
        call={call}
        displayMode="fullscreen"
        onRequestMode={() => "fullscreen"}
        fill
      />
      </Suspense>
    </div>
  );
}

/** 带界面的 MCP 服务：每个界面一行（同一个界面只列一次）。 */
function useMcpApps(): { server: string; uri: string }[] {
  const tools = useSyncExternalStore(subscribeTools, getTools);
  return useMemo(() => {
    const seen = new Map<string, { server: string; uri: string }>();
    for (const t of tools) if (t.resourceUri && !seen.has(appKey(t.server, t.resourceUri))) seen.set(appKey(t.server, t.resourceUri), { server: t.server, uri: t.resourceUri });
    return [...seen.values()];
  }, [tools]);
}

/** 分屏菜单里的「MCP 应用」一组：带界面的 MCP 服务，每个界面一行；没有就整组不出现。 */
export function AppSplitGroup({ open, onSplit, close }: SplitGroupProps) {
  const apps = useMcpApps();
  if (apps.length === 0) return null;
  return (
    <>
      <div className="mx-2.5 my-1 h-px bg-line-soft" />
      <div className="px-2.5 pb-1 pt-1.5 text-[11px] font-medium text-ink-4">MCP 应用</div>
      {apps.map((a) => {
        const kind = panelKind(a.server, a.uri);
        const opened = open.includes(kind);
        return (
          <button
            key={kind}
            type="button"
            disabled={opened}
            onClick={() => {
              close();
              onSplit(kind);
            }}
            className="motion-colors flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-left hover:bg-hover disabled:hover:bg-transparent"
          >
            <AppWindow size={14} className={opened ? "text-ink-5" : "text-ink-3"} />
            <span className={`min-w-0 truncate text-[13px] ${opened ? "text-ink-5" : "text-ink"}`}>{a.server}</span>
            <span className={`min-w-0 flex-1 truncate text-[11px] ${opened ? "text-ink-5" : "text-ink-4"}`}>{a.uri}</span>
            {opened && <span className="shrink-0 text-[11px] text-ink-5">已打开</span>}
          </button>
        );
      })}
    </>
  );
}

/** 右侧工具面板里的「MCP 应用」一组：带界面的 MCP 服务各一行；没有就整组不出现。 */
export function AppEntryGroup({ onOpen }: PanelEntryGroupProps) {
  const apps = useMcpApps();
  if (apps.length === 0) return null;
  return (
    <>
      <div className="mx-1 my-1.5 h-px bg-line-soft" />
      <div className="px-1 pb-1 pt-1.5 text-[11px] font-medium text-ink-4">MCP 应用</div>
      {apps.map((a) => (
        <ToolRow
          key={panelKind(a.server, a.uri)}
          icon={AppWindow}
          label={a.server}
          // 侧栏窄：只显示界面名（uri 的最后一段），完整 uri 在 tooltip 里。
          hint={a.uri.split("/").pop() || a.uri}
          title={`${a.server} · ${a.uri}`}
          onClick={() => onOpen(panelKind(a.server, a.uri))}
        />
      ))}
    </>
  );
}
