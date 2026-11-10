// 会话区顶部的连接横条。设计：wonder-ui/shell/ConnectionBanner（bink / izzy / khet）。
// - 第一次连（从没连上过）：灰条「正在连接 Dock…」；
// - 连上过又断了：黄条「正在重连…」+ 第几次；
// - Dock 进程退出：红条 + 「重新启动」；
// - 远程 Dock 的令牌无效 / 被撤销（不再重连）：红条 + 「连接设置」。
// 连远程时文案带上连接名。连上就收起（高度折叠）。

import { useRef } from "react";
import { CircleAlert, LoaderCircle, RefreshCw } from "lucide-react";
import type { AppState } from "../dock/store";
import { remoteWhere } from "../dock/remote";

export function ConnectionBanner({ app, onRestart }: { app: AppState; onRestart: () => void }) {
  const now = app.fatal
    ? "fatal"
    : app.exited
      ? "exited"
      : app.connection === "open"
        ? null
        : app.reconnects > 0
          ? "reconnecting"
          : "connecting";
  const target = app.remote ? `远程 Dock「${app.remote.name}」` : "Dock";
  // 收起时还画着上一种，折叠动画才有内容可收。
  const last = useRef(now);
  if (now) last.current = now;
  const kind = last.current;
  return (
    <div className="fold shrink-0" data-open={now !== null} aria-hidden={now === null}>
      <div>
        {kind === "connecting" && (
          <div className="flex h-9 items-center justify-between gap-2 border-b border-line-strong bg-fill px-4">
            <span className="flex items-center gap-2 text-xs font-medium text-ink-2">
              <LoaderCircle size={14} className="animate-spin text-ink-4" />
              正在连接 {target}…
            </span>
            <span className="truncate text-[11px] text-ink-4">
              {app.error ?? (app.remote ? remoteWhere(app.remote) : "等待本地 Dock 就绪")}
            </span>
          </div>
        )}
        {kind === "reconnecting" && (
          <div className="mx-4 mt-2 flex h-9 items-center justify-between gap-2 rounded-md border border-[#f5d48a] bg-[#fff8e6] px-3.5">
            <span className="flex items-center gap-2 text-xs font-medium text-[#8a5a00]">
              <RefreshCw size={14} className="animate-spin [animation-duration:2s]" />
              {target} 连接已断开，正在重连…
            </span>
            <span className="text-[11px] text-[#a16207]">正在尝试重新建立连接（第 {app.reconnects} 次）</span>
          </div>
        )}
        {kind === "fatal" && (
          <div className="mx-4 mt-2 flex h-9 items-center justify-between gap-2 rounded-md border border-[#fecaca] bg-[#fef2f2] px-3.5">
            <span className="flex min-w-0 items-center gap-2 text-xs font-medium text-[#c0392b]">
              <CircleAlert size={14} className="shrink-0" />
              <span className="truncate">
                连不上{target}：{app.fatal}
              </span>
            </span>
            <button
              type="button"
              onClick={() => window.dispatchEvent(new CustomEvent("dock:open-settings", { detail: "gateway" }))}
              className="motion-colors flex h-6 shrink-0 items-center rounded-md border border-line-strong bg-white px-2.5 text-[11px] font-medium text-ink shadow-sm hover:bg-hover active:scale-[0.97]"
            >
              连接设置
            </button>
          </div>
        )}
        {kind === "exited" && (
          <div className="mx-4 mt-2 flex h-9 items-center justify-between gap-2 rounded-md border border-[#fecaca] bg-[#fef2f2] px-3.5">
            <span className="flex min-w-0 items-center gap-2 text-xs font-medium text-[#c0392b]">
              <CircleAlert size={14} className="shrink-0" />
              <span className="truncate">Dock 已退出（{app.exited}）</span>
            </span>
            <button
              type="button"
              onClick={onRestart}
              className="motion-colors flex h-6 shrink-0 items-center rounded-md border border-line-strong bg-white px-2.5 text-[11px] font-medium text-ink shadow-sm hover:bg-hover active:scale-[0.97]"
            >
              重新启动
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
