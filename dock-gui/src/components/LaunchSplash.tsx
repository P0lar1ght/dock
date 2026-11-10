// 启动画面：第一次连 Dock 慢时盖住整窗，品牌标描线 → 填色 → 呼吸，连上后放大淡出。
// - 本机通常 0.3 秒内连上：等 GRACE 还没好才出现，不闪一下；
// - 出现了至少留 MIN_VISIBLE，让描线走完；
// - 最多留 MAX_VISIBLE：还没连上就让位给会话区顶部的连接横条（那里有重连 / 设置入口）；
// - Dock 退出、远程令牌失效：立刻让位，横条上有处理按钮。
// 只管第一次启动；之后的断线重连只走横条。

import { useEffect, useRef, useState } from "react";
import type { AppState } from "../dock/store";
import { MARK_PATH, MARK_VIEWBOX } from "../assets/markPath";

const GRACE = 400;
const MIN_VISIBLE = 800;
const MAX_VISIBLE = 8000;
/** 和 styles.css `.launch-splash` 的离场时长一致。 */
const LEAVE = 240;

type Phase = "wait" | "shown" | "leaving" | "gone";

export function LaunchSplash({ app }: { app: AppState }) {
  const ready = app.booted || app.exited !== null || app.fatal !== null;
  const [phase, setPhase] = useState<Phase>(ready ? "gone" : "wait");
  const shownAt = useRef(0);

  useEffect(() => {
    if (phase === "wait") {
      if (ready) return setPhase("gone");
      const t = window.setTimeout(() => {
        shownAt.current = performance.now();
        setPhase("shown");
      }, GRACE);
      return () => window.clearTimeout(t);
    }
    if (phase === "shown") {
      const elapsed = performance.now() - shownAt.current;
      const wait = ready ? Math.max(0, MIN_VISIBLE - elapsed) : Math.max(0, MAX_VISIBLE - elapsed);
      const t = window.setTimeout(() => setPhase("leaving"), wait);
      return () => window.clearTimeout(t);
    }
    if (phase === "leaving") {
      const t = window.setTimeout(() => setPhase("gone"), LEAVE);
      return () => window.clearTimeout(t);
    }
  }, [phase, ready]);

  if (phase === "wait" || phase === "gone") return null;
  const target = app.remote ? `远程 Dock「${app.remote.name}」` : "Dock";
  return (
    <div
      className="launch-splash absolute inset-0 z-[60] flex items-center justify-center"
      data-leaving={phase === "leaving" || undefined}
      data-tauri-drag-region
      role="status"
      aria-label={`正在连接 ${target}`}
    >
      <svg className="launch-mark" viewBox={MARK_VIEWBOX} aria-hidden>
        <path className="launch-mark-fill" d={MARK_PATH} />
        <path className="launch-mark-stroke" d={MARK_PATH} pathLength={1} />
      </svg>
      {/* 连得久才出现的一行字：本机冷启动看不到，远程慢的时候告诉用户在等谁。 */}
      <div className="launch-caption pointer-events-none absolute inset-x-0 top-1/2 mt-12 flex flex-col items-center gap-1 px-8 text-center">
        <span className="text-xs font-medium text-ink-3">正在连接 {target}…</span>
        {app.error && <span className="max-w-md truncate text-[11px] text-ink-4">{app.error}</span>}
      </div>
    </div>
  );
}
