// 拆出的对话工作台：把会话从壳里拆出来，平铺成网格。设计：wonder-ui/pages/Workbenches。
//
// 每格就是壳内那套会话面板（头栏 + 流 + 输入框），只是把「单独面板」换成「重新停靠」。
// 左上角的拖手往窗口外拖，松手就给这个会话开一扇独立聊天窗（Rust 的 pop_out_drag）；
// 头栏的「在新窗口打开」不用拖。

import { useState, type PointerEvent as ReactPointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { GripVertical } from "lucide-react";
import type { SessionSummary } from "../model";
import { gatewayArg } from "../dock/store";
import { SessionPane } from "./SessionPane";
import { WindowControls, type SidebarProps } from "./Sidebar";

export function Workbench({
  sessions,
  windowControls,
  onDock,
  onClose,
  onPopOut,
}: {
  sessions: SessionSummary[];
  /** 侧栏收起时：左上角固定着窗口按钮，头栏给它留同尺寸的透明占位（同 SessionPane）。 */
  windowControls: Pick<SidebarProps, "onToggleSidebar" | "onBack" | "onForward" | "canGoBack" | "canGoForward"> | null;
  /** 收回壳内（并选中它）。 */
  onDock: (id: string) => void;
  /** 从工作台移掉（会话本身还在）。 */
  onClose: (id: string) => void;
  /** 拖到窗口外：已经开成独立窗口了，从工作台移掉。 */
  onPopOut: (id: string) => void;
}) {
  return (
    <div className="glass-backdrop thin-scroll flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto bg-window">
      <header
        className={`flex shrink-0 items-center px-6 ${windowControls ? "h-11 mb-2" : "pb-4 pt-5"}`}
        data-tauri-drag-region
      >
        {windowControls && (
          // 头栏从 x=76 起，固定按钮在 x=20：留一块同样大小的透明占位（同 SessionPane）。
          <div className="invisible -ml-1 mr-3 shrink-0" aria-hidden>
            <WindowControls {...windowControls} collapsed />
          </div>
        )}
        <h1 className="text-xl font-semibold text-ink" data-tauri-drag-region>
          Dock · 工作站
        </h1>
      </header>
      {/* 只有一格就占满，别留半边空白；两格起才分两列。 */}
      <div
        className={`grid min-h-0 flex-1 gap-4 px-6 pb-6 [grid-auto-rows:minmax(340px,1fr)] ${
          sessions.length > 1 ? "grid-cols-1 xl:grid-cols-2" : "grid-cols-1"
        }`}
      >
        {sessions.map((s) => (
          <Cell key={s.id} session={s} onDock={() => onDock(s.id)} onClose={() => onClose(s.id)} onPopOut={() => onPopOut(s.id)} />
        ))}
      </div>
    </div>
  );
}

function Cell({
  session,
  onDock,
  onClose,
  onPopOut,
}: {
  session: SessionSummary;
  onDock: () => void;
  onClose: () => void;
  onPopOut: () => void;
}) {
  const [dragging, setDragging] = useState(false);
  return (
    <div
      className={`glass-sheet enter-fade relative flex min-h-0 min-w-0 flex-col overflow-hidden rounded-xl border bg-pane shadow-[0px_6px_20px_rgba(0,0,0,0.06)] motion-colors ${
        dragging ? "border-accent ring-2 ring-[#0a84ff26]" : "border-line-strong"
      }`}
    >
      {dragging && (
        <div className="enter-fade pointer-events-none absolute inset-x-0 top-12 z-10 flex justify-center">
          <span className="rounded-full bg-ink/80 px-3 py-1 text-[11px] font-medium text-white shadow-lg">
            拖到窗口外松手，开成独立窗口
          </span>
        </div>
      )}
      <SessionPane
        session={session}
        windowControls={null}
        onPopOut={() => popOut(session.id).then((ok) => ok && onPopOut())}
        onDock={onDock}
        onClosePane={onClose}
        closeLabel="关闭"
        onDeleted={onClose}
        showConnection={false}
        empty={null}
        dragHandle={<PopOutHandle sessionId={session.id} onDragging={setDragging} onPopOut={onPopOut} />}
      />
    </div>
  );
}

/** 不拖、直接开成独立窗口（落在系统默认位置）。开不了回 false，格子留在工作台。 */
function popOut(sessionId: string): Promise<boolean> {
  return invoke("open_chat_window", { sessionId, gateway: gatewayArg(), x: 0, y: 0 }).then(
    () => true,
    () => false,
  );
}

/**
 * 拖手：按下就交给 Rust（pop_out_drag）等系统鼠标松开，松手时光标在窗口外就开窗。
 * 光标出了窗口，WebView 自己的 pointerup 靠不住；按下时拦掉默认行为，免得变成选字。
 */
function PopOutHandle({
  sessionId,
  onDragging,
  onPopOut,
}: {
  sessionId: string;
  onDragging: (on: boolean) => void;
  onPopOut: () => void;
}) {
  const down = async (e: ReactPointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    onDragging(true);
    try {
      if (await invoke<boolean>("pop_out_drag", { sessionId, gateway: gatewayArg() })) onPopOut();
    } catch {
      // 开窗失败就当没拖出去，格子留在工作台。
    } finally {
      onDragging(false);
    }
  };

  return (
    <button
      type="button"
      title="拖到窗口外开成独立窗口"
      aria-label="拖出"
      onPointerDown={down}
      className="motion-colors -ml-1 flex size-6 shrink-0 cursor-grab touch-none select-none items-center justify-center rounded text-ink-5 hover:bg-hover hover:text-ink-3 active:cursor-grabbing"
    >
      <GripVertical size={14} />
    </button>
  );
}
