// 主区域整页看一个子代理（侧栏会话下面点进来）。和右侧子代理面板同一套对话 / 输入框，
// 只是放宽到阅读列。头上是面包屑「主会话 › 描述 · 角色」，点主会话回去。
// 结束了的、只在盘上的照样能发消息：Dock 用同一个 id 接着聊（会话关着先开页）。

import { useEffect, useState, type ReactNode } from "react";
import { Bot, ChevronRight, LoaderCircle } from "lucide-react";
import { elapsedMs } from "dock-core";
import type { SessionSummary } from "../model";
import { store, useSubagents } from "../dock/store";
import { WindowControls, type SidebarProps } from "./Sidebar";
import { PathLinkContext } from "./markdown/pathLink";
import { usePathLinker } from "./markdown/usePathLinker";
import { clock, RolePill, useNow } from "./SubagentCard";
import { Composer, Conversation, STATUS_TEXT } from "./SubagentPanel";

export function SubagentPage({
  session,
  agentId,
  onBack,
  windowControls,
  panelOpen,
  sideToggle,
}: {
  /** 派它的会话。 */
  session: SessionSummary;
  agentId: string;
  onBack: () => void;
  windowControls: Pick<SidebarProps, "onToggleSidebar" | "onBack" | "onForward" | "canGoBack" | "canGoForward"> | null;
  /** 右侧栏是否展开：收起时顶栏右侧给「侧边面板」按钮让位。 */
  panelOpen?: boolean;
  /** 折叠右侧那一栏的按钮（收起时才画在这儿）。 */
  sideToggle?: ReactNode;
}) {
  const threadId = session.id;
  const state = useSubagents(threadId);
  const sub = state.byId[agentId] ?? null;
  const [loading, setLoading] = useState(false);
  const running = sub?.info.status === "running";
  const now = useNow(running);
  const linker = usePathLinker(threadId);

  // 列表没铺过就铺；对话不全（中途接入、只在盘上）就拉一次完整的。
  useEffect(() => store.ensureSubagents(threadId), [threadId]);
  const complete = sub?.complete ?? false;
  const known = !!sub;
  useEffect(() => {
    if (!known || complete) return;
    setLoading(true);
    void store.loadSubagentHistory(threadId, agentId).finally(() => setLoading(false));
  }, [threadId, agentId, known, complete]);

  const info = sub?.info;
  const status = info ? STATUS_TEXT[info.status] : null;
  const alive = info && (info.status === "running" || info.status === "idle") && !info.persisted;

  return (
    <section key={`${threadId}:${agentId}`} className="glass-pane enter-fade flex min-h-0 min-w-0 flex-1 flex-col bg-pane">
      <header
        className={`relative flex h-11 shrink-0 items-center justify-between gap-3 border-b border-line-soft pl-4 ${
          // 右侧栏收起时，右上角空出来给那个「侧边面板」按钮（展开时它在右侧那一列的头栏里）。
          panelOpen ? "pr-4" : "pr-3"
        }`}
        data-tauri-drag-region
      >
        <div className="flex min-w-0 items-center gap-1.5" data-tauri-drag-region>
          {windowControls && (
            <div className="invisible ml-1 mr-1 shrink-0" aria-hidden>
              <WindowControls {...windowControls} collapsed />
            </div>
          )}
          <button
            type="button"
            title="回到主会话"
            onClick={onBack}
            className="motion-colors min-w-0 max-w-[40%] shrink truncate rounded-md px-1.5 py-0.5 text-sm text-ink-3 hover:bg-hover hover:text-ink"
          >
            {session.title}
          </button>
          <ChevronRight size={14} className="shrink-0 text-ink-5" />
          <Bot size={15} className={`shrink-0 ${running ? "text-accent" : "text-ink-3"}`} />
          <span className="min-w-0 truncate text-sm font-semibold text-ink" title={info?.description}>
            {info?.description || "子代理"}
          </span>
          {info && <RolePill text={info.role} />}
          {info && status && (
            <span className="ml-1 flex shrink-0 items-center gap-1.5">
              <span className={`size-1.5 rounded-full ${status.dot} ${running ? "breathe" : ""}`} />
              <span className={`text-[11.5px] font-medium ${status.color}`}>{status.text}</span>
              {sub && <span className="font-mono text-[11.5px] text-ink-4">{clock(elapsedMs(sub, now))}</span>}
            </span>
          )}
        </div>
        {alive && info && (
          <div className="flex shrink-0 items-center gap-1.5">
            {running && (
              <button
                type="button"
                onClick={() => void store.interruptSubagent(threadId, agentId)}
                className="motion-colors flex h-7 items-center rounded-md border border-line-strong bg-white px-2.5 text-[12px] font-medium text-ink-2 hover:bg-hover"
              >
                打断
              </button>
            )}
            <button
              type="button"
              title="收掉它；之后再发消息会从它的对话接着来"
              onClick={() => void store.stopSubagent(threadId, agentId)}
              className="motion-colors flex h-7 items-center rounded-md border border-line-strong bg-white px-2.5 text-[12px] font-medium text-ink-2 hover:bg-hover"
            >
              结束
            </button>
          </div>
        )}
        {sideToggle}
      </header>

      <PathLinkContext.Provider value={linker}>
        {!sub || (loading && sub.thread.turns.length === 0) ? (
          <div className="flex flex-1 items-center justify-center gap-2 text-[13px] text-ink-4">
            <LoaderCircle size={14} className="animate-spin text-accent" />
            正在读取子代理的对话…
          </div>
        ) : sub.thread.turns.length === 0 && !running ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 text-center">
            <span className="text-[13px] text-ink-4">过程没有保留（这个子代理在对话落盘之前跑的）</span>
            {sub.info.output && (
              <div className="max-w-[640px] whitespace-pre-wrap break-words rounded-xl border border-line-soft bg-white px-4 py-3 text-left text-[13px] leading-[1.6] text-ink-2">
                {sub.info.output}
              </div>
            )}
          </div>
        ) : (
          <Conversation sub={sub} running={running} wide />
        )}
      </PathLinkContext.Provider>

      {sub && <Composer threadId={threadId} agentId={agentId} ended={!alive} wide />}
    </section>
  );
}
