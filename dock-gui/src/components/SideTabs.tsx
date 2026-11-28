// 右侧那一列顶部的标签行：当前打开的面板各一个页签，点一下翻到那一页；
// 最右是折叠这一栏的按钮。形态照 minimax code 的浏览器标签行（那行右边也是折叠按钮）。
//
// 标签顺序就是下面那本书的书页顺序（翻页不挪位）：翻到的那页左边的标签，书页在左边；右边的在右边。
// 悬停标签和悬停书页共用一个状态（`hovered`）：标签亮起，那一页翻开一半给看一眼。
//
// 任何宽度都摆得下：放不下时先只留翻到的那页的名字、再全成图标，图标也放不下就把多的收进「⌄」。
// 右边是窗口动作（再叠一页、在新窗口打开，对着翻到的那页）和折叠按钮。

import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { ChevronDown, type LucideIcon } from "lucide-react";
import type { SidePanel } from "./ToolPanel";
import { Popover } from "./Popover";

export interface SideTabMeta {
  label: string;
  icon: LucideIcon;
}

/** 名字最长画多宽（再长截断）。 */
const LABEL_MAX = 140;
/** 只有图标的页签宽（w-7）、页签间距（gap-1）、「⌄」宽。 */
const ICON_TAB = 28;
const GAP = 4;
const MORE = 28;

type Fit = { labels: "all" | "active" | "none"; hidden: SidePanel[] };

let measureCtx: CanvasRenderingContext2D | null = null;
/** 量一段字在页签字体下多宽。 */
function textWidth(text: string, font: string): number {
  measureCtx ??= document.createElement("canvas").getContext("2d");
  if (!measureCtx) return text.length * 12;
  measureCtx.font = font;
  return measureCtx.measureText(text).width;
}

export function SideTabs({
  kinds,
  active,
  meta,
  badge,
  hovered = null,
  onHover,
  onPick,
  actions,
  sideToggle,
}: {
  /** 打开着的面板，标签顺序 = 书页顺序。 */
  kinds: SidePanel[];
  /** 翻到的那页。 */
  active: SidePanel;
  meta: (kind: SidePanel) => SideTabMeta;
  /** 页签上的蓝色数字徽标（在跑的子代理数）。 */
  badge?: (kind: SidePanel) => number;
  /** 悬停着的那一页（标签或书页上）：没翻到的亮起来。 */
  hovered?: SidePanel | null;
  onHover?: (kind: SidePanel | null) => void;
  /** 点页签：翻到这一页。 */
  onPick: (kind: SidePanel) => void;
  /** 折叠按钮左边：对着翻到的那页的窗口动作（再叠一页、在新窗口打开）。 */
  actions?: ReactNode;
  /** 最右：折叠右侧这一栏。 */
  sideToggle?: ReactNode;
}) {
  const row = useRef<HTMLDivElement>(null);
  const [fit, setFit] = useState<Fit>({ labels: "all", hidden: [] });
  const labels = kinds.map((k) => meta(k).label);
  const counts = kinds.map((k) => badge?.(k) ?? 0);
  const signature = `${kinds.join("|")}#${active}#${labels.join("|")}#${counts.join(",")}`;

  useLayoutEffect(() => {
    const el = row.current;
    if (!el) return;
    const measure = () => {
      const cs = getComputedStyle(el);
      const font = `500 12px ${cs.fontFamily}`;
      const avail = el.clientWidth;
      const wide = (i: number) =>
        20 + 13 + 6 + Math.min(LABEL_MAX, textWidth(labels[i], font)) + (counts[i] > 0 ? 6 + 16 : 0);
      const total = (ws: number[]) => ws.reduce((a, b) => a + b, 0) + GAP * Math.max(0, ws.length - 1);
      const at = kinds.indexOf(active);
      let next: Fit;
      if (total(kinds.map((_, i) => wide(i))) <= avail) next = { labels: "all", hidden: [] };
      else if (total(kinds.map((_, i) => (i === at ? wide(i) : ICON_TAB))) <= avail) next = { labels: "active", hidden: [] };
      else if (total(kinds.map(() => ICON_TAB)) <= avail) next = { labels: "none", hidden: [] };
      else {
        // 图标也放不下：留得下几个留几个（翻到的那页一定在），其余进「⌄」。
        const slots = Math.max(1, Math.floor((avail - MORE) / (ICON_TAB + GAP)));
        const shown = new Set<SidePanel>([active]);
        for (const k of kinds) if (shown.size < slots) shown.add(k);
        next = { labels: "none", hidden: kinds.filter((k) => !shown.has(k)) };
      }
      setFit((prev) =>
        prev.labels === next.labels && prev.hidden.join("|") === next.hidden.join("|") ? prev : next,
      );
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
    // signature 涵盖 kinds / active / 名字 / 徽标。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature]);

  const pick = (kind: SidePanel) => {
    onHover?.(null);
    onPick(kind);
  };

  return (
    <div className="flex h-11 shrink-0 items-center gap-1 border-b border-line-soft pl-2 pr-3" data-tauri-drag-region>
      <div ref={row} className="flex min-w-0 flex-1 items-center gap-1 overflow-hidden" data-tauri-drag-region>
        {kinds.map((kind, i) => {
          if (fit.hidden.includes(kind)) return null;
          const m = meta(kind);
          const Icon = m.icon;
          const on = kind === active;
          const lit = !on && kind === hovered;
          const count = counts[i];
          const labeled = fit.labels === "all" || (fit.labels === "active" && on);
          return (
            <button
              key={kind}
              type="button"
              title={m.label}
              aria-label={m.label}
              onClick={() => pick(kind)}
              onMouseEnter={() => onHover?.(kind)}
              onMouseLeave={() => onHover?.(null)}
              className={`motion-colors relative flex h-7 shrink-0 items-center rounded-lg text-[12px] font-medium ${
                labeled ? "gap-1.5 px-2.5" : "w-7 justify-center"
              } ${
                on
                  ? "bg-selected text-ink shadow-[inset_0_0_0_1px_var(--color-accent)]"
                  : lit
                    ? "bg-[color-mix(in_srgb,var(--color-accent)_10%,transparent)] text-accent"
                    : "text-ink-3 hover:bg-hover hover:text-ink"
              }`}
            >
              <Icon size={13} strokeWidth={1.9} className="shrink-0" />
              {labeled && <span className="max-w-[140px] truncate">{m.label}</span>}
              {count > 0 &&
                (labeled ? (
                  <span className="flex size-[16px] shrink-0 items-center justify-center rounded-full bg-accent text-[10px] font-semibold text-white tabular-nums">
                    {count}
                  </span>
                ) : (
                  <span className="absolute right-1 top-1 size-1.5 rounded-full bg-accent" />
                ))}
            </button>
          );
        })}
        {fit.hidden.length > 0 && (
          <Popover
            title={`还有 ${fit.hidden.length} 页`}
            side="bottom"
            align="left"
            trigger={
              <span className="flex items-center gap-0.5 text-[11px] font-medium tabular-nums">
                +{fit.hidden.length}
                <ChevronDown size={11} />
              </span>
            }
            className="flex h-7 min-w-7 items-center justify-center rounded-lg px-1 text-ink-3 hover:bg-hover hover:text-ink aria-expanded:bg-selected"
            panelClassName="min-w-[180px] p-1.5"
          >
            {(close) =>
              fit.hidden.map((kind) => {
                const m = meta(kind);
                const Icon = m.icon;
                const count = badge?.(kind) ?? 0;
                return (
                  <button
                    key={kind}
                    type="button"
                    onClick={() => {
                      close();
                      pick(kind);
                    }}
                    onMouseEnter={() => onHover?.(kind)}
                    onMouseLeave={() => onHover?.(null)}
                    className="motion-colors flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-left text-ink hover:bg-hover"
                  >
                    <Icon size={14} className="shrink-0 text-ink-3" />
                    <span className="min-w-0 flex-1 truncate text-[13px]">{m.label}</span>
                    {count > 0 && <span className="text-[11px] font-semibold text-accent tabular-nums">{count}</span>}
                  </button>
                );
              })
            }
          </Popover>
        )}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-0.5">{actions}</div>}
      {sideToggle}
    </div>
  );
}
