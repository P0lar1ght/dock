// 右侧那一列顶部的标签行：当前打开的面板各一个页签，点一下翻到那一页；
// 最右是折叠这一栏的按钮。形态照 minimax code 的浏览器标签行（那行右边也是折叠按钮）。
//
// 下面才是面板本身（各面板的头栏、页边堆叠照旧，没动）。

import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import type { SidePanel } from "./ToolPanel";

export interface SideTabMeta {
  label: string;
  icon: LucideIcon;
}

export function SideTabs({
  kinds,
  active,
  meta,
  badge,
  onPick,
  sideToggle,
}: {
  /** 打开着的面板（最后一个是摊开的那页）。 */
  kinds: SidePanel[];
  /** 当前摊开的那页。 */
  active: SidePanel;
  meta: (kind: SidePanel) => SideTabMeta;
  /** 页签上的蓝色数字徽标（在跑的子代理数）。 */
  badge?: (kind: SidePanel) => number;
  /** 点页签：翻到这一页。 */
  onPick: (kind: SidePanel) => void;
  /** 最右：折叠右侧这一栏。 */
  sideToggle?: ReactNode;
}) {
  return (
    <div className="flex h-11 shrink-0 items-center gap-1 border-b border-line-soft pl-2 pr-3" data-tauri-drag-region>
      <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto" data-tauri-drag-region>
        {kinds.map((kind) => {
          const m = meta(kind);
          const Icon = m.icon;
          const on = kind === active;
          const count = badge?.(kind) ?? 0;
          return (
            <button
              key={kind}
              type="button"
              title={m.label}
              onClick={() => onPick(kind)}
              className={`motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-[12px] font-medium ${
                on ? "bg-selected text-ink" : "text-ink-3 hover:bg-hover hover:text-ink"
              }`}
            >
              <Icon size={13} strokeWidth={1.9} className="shrink-0" />
              <span className="max-w-[140px] truncate">{m.label}</span>
              {count > 0 && (
                <span className="flex size-[16px] shrink-0 items-center justify-center rounded-full bg-accent text-[10px] font-semibold text-white tabular-nums">
                  {count}
                </span>
              )}
            </button>
          );
        })}
      </div>
      {sideToggle}
    </div>
  );
}
