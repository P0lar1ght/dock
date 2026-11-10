// 侧栏里会话下面挂它派出的子代理（参考用户给的 MiniMax code 截图）。
// 默认收起：会话行左边的箭头 + 个数；有子代理在跑就自动展开。点子代理在主区域整页看它的对话。
// 计数先用 thread/list 给的（关着的会话 Dock 只数落盘目录），展开后拉 subagent/list 铺开。

import { useEffect, useState, type ComponentProps } from "react";
import { Bot, ChevronRight, LoaderCircle } from "lucide-react";
import type { Subagent } from "dock-core";
import type { SessionSummary } from "../model";
import { store, useSubagents } from "../dock/store";
import { SessionRow } from "./Sidebar";

export interface SubagentTarget {
  threadId: string;
  agentId: string;
}

export function SessionWithSubagents({
  session,
  row,
  active,
  onSelect,
}: {
  session: SessionSummary;
  /** 会话行本身（Sidebar 原来给 SessionRow 的那些）。 */
  row: ComponentProps<typeof SessionRow>;
  /** 正在主区域看的子代理。 */
  active: SubagentTarget | null;
  onSelect: (target: SubagentTarget) => void;
}) {
  const state = useSubagents(session.id);
  const live = state.order.map((id) => state.byId[id]);
  const count = Math.max(session.subagentCount, live.length);
  const running = live.some((s) => s.info.status === "running") || session.subagentsRunning > 0;
  const [open, setOpen] = useState(false);

  // 有子代理在跑：自动展开（跑完不自动收，免得列表跳）。
  useEffect(() => {
    if (running) setOpen(true);
  }, [running]);
  // 看着其中一个子代理时它的会话得展开。
  useEffect(() => {
    if (active?.threadId === session.id) setOpen(true);
  }, [active?.threadId, session.id]);
  useEffect(() => {
    if (open && count > 0) store.ensureSubagents(session.id);
  }, [open, count, session.id]);

  if (count === 0) return <SessionRow {...row} />;
  return (
    <div className="flex flex-col gap-px">
      <div className="relative">
        <button
          type="button"
          aria-label={open ? "收起子代理" : `展开 ${count} 个子代理`}
          aria-expanded={open}
          onClick={() => setOpen((o) => !o)}
          className="absolute -left-3 top-0 z-10 flex h-7 w-3 items-center justify-center text-ink-4 hover:text-ink-2"
        >
          <ChevronRight size={11} className="rotate-open" data-open={open} />
        </button>
        <SessionRow {...row} />
        {!open && !row.waiting && !row.finished && (
          <span className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 text-[10.5px] tabular-nums text-ink-5">
            {count}
          </span>
        )}
      </div>
      <div className="fold" data-open={open}>
        <div className="flex flex-col gap-px pl-3">
          {open &&
            (live.length === 0 ? (
              <div className="flex h-7 items-center gap-1.5 px-2.5 text-[11.5px] text-ink-4">
                <LoaderCircle size={11} className="animate-spin" />
                正在读取子代理…
              </div>
            ) : (
              live.map((sub) => (
                <SubagentRow
                  key={sub.info.agentId}
                  sub={sub}
                  active={active?.threadId === session.id && active.agentId === sub.info.agentId}
                  onClick={() => onSelect({ threadId: session.id, agentId: sub.info.agentId })}
                />
              ))
            ))}
        </div>
      </div>
    </div>
  );
}

const STATUS_HINT: Record<string, string> = {
  running: "在跑",
  idle: "这一轮做完了，能接着聊",
  completed: "已结束，发消息会接着聊",
  failed: "出错了",
  cancelled: "被停下了",
};

function SubagentRow({ sub, active, onClick }: { sub: Subagent; active: boolean; onClick: () => void }) {
  const { info } = sub;
  const label = info.description || info.role || "子代理";
  let mark = null;
  if (info.status === "running") {
    mark = (
      <span className="enter-fade flex shrink-0">
        <LoaderCircle size={11} className="animate-spin text-accent" />
      </span>
    );
  } else if (info.status === "failed") {
    mark = <span className="size-1.5 shrink-0 rounded-full bg-[#ff3b30]" />;
  } else if (info.status === "cancelled") {
    mark = <span className="size-1.5 shrink-0 rounded-full bg-ink-5" />;
  }
  return (
    <button
      type="button"
      title={`${info.role} · ${label}\n${STATUS_HINT[info.status] ?? ""}`}
      onClick={onClick}
      className={`enter-fade motion-colors flex h-7 shrink-0 select-none items-center gap-1.5 rounded-[7px] px-2.5 text-left text-[12px] ${
        active ? "glass-state bg-[#e8e8ed] font-medium text-ink" : "text-ink-3 hover:bg-hover hover:text-ink-2"
      }`}
    >
      <Bot size={13} className={`shrink-0 ${info.status === "running" ? "text-accent" : "text-ink-4"}`} />
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {mark}
    </button>
  );
}
