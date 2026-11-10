// 各面板共用的小件：图标按钮、红绿灯占位、平台判断。

import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

/** 在 Tauri 窗口里红绿灯是系统画的；浏览器里预览时画一个假的占位，保持布局一致。 */
export const IN_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function TrafficLights() {
  if (IN_TAURI) {
    // 系统红绿灯叠在这块空白上（tauri.conf.json 的 trafficLightPosition）。
    return <div className="w-[62px] shrink-0" data-tauri-drag-region />;
  }
  return (
    <div className="flex w-[62px] shrink-0 items-center gap-2">
      <span className="size-3 rounded-full bg-[#ff5f57]" />
      <span className="size-3 rounded-full bg-[#febc2e]" />
      <span className="size-3 rounded-full bg-[#28c840]" />
    </div>
  );
}

interface IconButtonProps {
  icon: LucideIcon;
  label: string;
  onClick?: () => void;
  disabled?: boolean;
  size?: number;
  className?: string;
  children?: ReactNode;
}

export function IconButton({
  icon: Icon,
  label,
  onClick,
  disabled,
  size = 16,
  className = "",
  children,
}: IconButtonProps) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      disabled={disabled}
      className={`relative flex items-center justify-center rounded-md text-ink-3 transition-colors hover:bg-hover hover:text-ink disabled:text-ink-5 disabled:hover:bg-transparent ${className}`}
    >
      <Icon size={size} strokeWidth={2} />
      {children}
    </button>
  );
}

/** 第一版还没实现的入口：保留外观，悬停提示「即将推出」。 */
export const COMING_SOON = "即将推出";

/** token 数：`182k` / `1.2M`。设置页和对话流的压缩标记共用。 */
export function formatTokens(n: number): string {
  return n >= 1_000_000 ? `${+(n / 1_000_000).toFixed(1)}M` : n >= 1000 ? `${Math.round(n / 1000)}k` : String(n);
}

/** 用时：`3m 29s` / `12s`。会话头计时和对话流「用时」行共用。 */
export function formatElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(s / 60);
  return m ? `${m}m ${String(s % 60).padStart(2, "0")}s` : `${s}s`;
}
