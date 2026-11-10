// 侧栏导航打开的整页（Pull Request、定时任务）共用的小件：页头、空状态卡、相对时间、外链。
// 取值照 wonder-ui/pages/PagePR* / PageSchedules*（Workspace Column / Page Header / … Card）。

import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { IN_TAURI } from "./chrome";

/**
 * 整页外壳：白底，页头（标题 + 副标题 + 右侧按钮）+ 可滚动的正文。
 * 玻璃材质下是一层阅读面（glass-pane）；它盖在会话区上，glass-page 让后面的兄弟节点先藏起来，免得透出来。
 */
export function NavPageShell({
  title,
  subtitle,
  actions,
  insetLeft,
  children,
}: {
  title: string;
  subtitle: string;
  actions?: ReactNode;
  /** 侧栏收起时左上角浮着红绿灯和前进后退：页头让开。 */
  insetLeft?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="glass-pane glass-page enter-fade absolute inset-0 z-20 flex min-h-0 min-w-0 flex-col bg-white">
      <div
        className={`flex shrink-0 items-start justify-between gap-4 border-b border-[#ececf0] pb-4 pr-8 ${insetLeft ? "pl-8 pt-12" : "pl-8 pt-6"}`}
        data-tauri-drag-region
      >
        <div className="flex min-w-0 flex-col gap-1">
          <h1 className="text-xl font-semibold tracking-[-0.2px] text-ink">{title}</h1>
          <p className="text-[12.5px] text-ink-4">{subtitle}</p>
        </div>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </div>
      {children}
    </div>
  );
}

/** 居中的空状态卡：图标 + 标题 + 说明 + 其余（命令块、按钮）。 */
export function EmptyCard({
  icon: Icon,
  title,
  body,
  children,
}: {
  icon: LucideIcon;
  title: string;
  body?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="flex flex-1 items-center justify-center p-8">
      <div className="pop-in flex w-[380px] flex-col items-center gap-3 rounded-2xl border border-[#ececf0] bg-white px-6 py-7 text-center shadow-sm">
        <div className="flex size-10 items-center justify-center rounded-full bg-[#f2f2f7] text-ink-3">
          <Icon size={18} />
        </div>
        <div className="text-[15px] font-semibold text-ink">{title}</div>
        {body && <div className="text-[12.5px] leading-relaxed text-ink-4">{body}</div>}
        {children}
      </div>
    </div>
  );
}

/** 列表分组标题：「我的 2」。 */
export function GroupHeading({ title, count, right }: { title: ReactNode; count?: number; right?: ReactNode }) {
  return (
    <div className="flex items-center gap-2 px-1">
      <span className="text-xs font-semibold text-ink-4">{title}</span>
      {count !== undefined && <span className="text-xs font-medium text-[#aeaeb2]">{count}</span>}
      {right && <span className="ml-auto">{right}</span>}
    </div>
  );
}

/** 在系统浏览器里开链接（浏览器预览里退回 window.open）。 */
export function openExternal(url: string) {
  if (IN_TAURI) openUrl(url).catch(() => window.open(url, "_blank"));
  else window.open(url, "_blank");
}

/** 过去的时间：「刚刚」「3 分钟前」「2 小时前」「昨天」「3 天前」。 */
export function ago(at: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - at) / 1000));
  if (s < 60) return "刚刚";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分钟前`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} 小时前`;
  const d = Math.floor(h / 24);
  return d === 1 ? "昨天" : `${d} 天前`;
}

/** 将来的时间：「不到 1 分钟」「12 分钟后」「3 小时后」，跨天写「明天 09:00」「10月6日 09:00」。 */
export function until(at: number, now = Date.now()): string {
  const s = Math.round((at - now) / 1000);
  if (s < 60) return "不到 1 分钟";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分钟后`;
  const target = new Date(at);
  const today = new Date(now);
  const hhmm = `${String(target.getHours()).padStart(2, "0")}:${String(target.getMinutes()).padStart(2, "0")}`;
  const dayDiff = Math.round(
    (new Date(target.getFullYear(), target.getMonth(), target.getDate()).getTime() -
      new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime()) /
      86_400_000,
  );
  if (dayDiff === 0) return `${Math.floor(m / 60)} 小时后`;
  if (dayDiff === 1) return `明天 ${hhmm}`;
  return `${target.getMonth() + 1}月${target.getDate()}日 ${hhmm}`;
}

/** 悬停看完整时间。 */
export function fullTime(at: number): string {
  return new Date(at).toLocaleString("zh-CN", { hour12: false });
}

/** 路径最后一段（项目名）。 */
export function baseName(path: string | null | undefined): string {
  if (!path) return "";
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
