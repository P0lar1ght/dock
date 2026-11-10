// 拖出工作台后的独立聊天窗（`index.html?session=<id>`）。
//
// 另一个 WebView = 另一份 store 和另一条 WS（ticket 一次性，复用不了），但连的是
// 同一个 Dock 进程，两边订阅同一批线程，事件各推一份，状态自然对齐。
// 窗口只有这一个会话：没有侧栏、工具面板和终端。

import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { store, useApp } from "../dock/store";
import { SessionPane } from "./SessionPane";

export function ChatWindow({ sessionId }: { sessionId: string }) {
  const dock = useApp();
  const session = dock.sessions.find((s) => s.id === sessionId) ?? null;

  useEffect(() => {
    store.select(sessionId);
    store.focus([sessionId]);
  }, [sessionId]);

  if (!session) {
    return (
      <div className="glass-pane flex h-full w-full items-center justify-center bg-pane text-xs text-ink-4" data-tauri-drag-region>
        {dock.connection === "open" ? "这个会话不在了" : "正在连接 Dock…"}
      </div>
    );
  }

  return (
    <div className="glass-window flex h-full w-full flex-col overflow-hidden bg-window">
      <SessionPane
        session={session}
        windowControls={null}
        onDeleted={() => void getCurrentWindow().close()}
        empty={null}
        // 红绿灯（titleBarStyle: Overlay）占住会话头左边，标题往右让一让。
        dragHandle={<span className="w-[62px] shrink-0" data-tauri-drag-region />}
      />
    </div>
  );
}
