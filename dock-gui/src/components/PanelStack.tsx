// 右侧面板叠成一摞玻璃页（设计：Wonder「Panel Glass Stack」三张：静止 / 悬停探出 / 拖出）。
//
// - 一次只摊开一页（`kinds` 的最后一个），占满右侧；别的页压在它底下，每页露出 EDGE 宽的页边：
//   图标 + 竖排名字 + 状态（在跑的子代理数）。同一平面上靠玻璃和叠放多给信息，不再把右侧切成一条条窄栏。
// - 悬停页边：那一页从它的位置滑出、盖在摊开那页上面（加宽到 PEEK），直接看内容；移开收回。
// - 点页边 / 在探出的页里按下：翻到最前（onFront）。
// - 每页一直挂着（只切可见性和位置）：画布 iframe、浏览器画面不重连。页按 kind 做 key，翻页不重挂。
// - 拖出成独立窗口照旧走各面板头栏的拖手（PanelGrip）。

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import type { SidePanel } from "./ToolPanel";

/** 每页露出的页边宽。 */
const EDGE = 48;
/** 页与容器边缘的空隙。 */
const PAD = 8;
/** 悬停探出时的宽度上限。 */
const PEEK = 380;
/** 悬停多久才探出（掠过不打扰）。 */
const HOVER_MS = 140;

export interface PanelMeta {
  label: string;
  icon: LucideIcon;
}

export function PanelStack({
  kinds,
  render,
  meta,
  badge,
  overlay,
  onFront,
}: {
  /** 叠放顺序：前面的压在下面，最后一个摊开。 */
  kinds: SidePanel[];
  render: (kind: SidePanel) => ReactNode;
  meta: (kind: SidePanel) => PanelMeta;
  /** 页边上的蓝色数字（0 不画）。 */
  badge?: (kind: SidePanel) => number;
  /** 盖在那一页上的东西（拖出窗口时的描边提示）。 */
  overlay?: (kind: SidePanel) => ReactNode;
  onFront: (kind: SidePanel) => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [peek, setPeek] = useState<SidePanel | null>(null);
  const timer = useRef<number | null>(null);
  /** 刚翻过页：光标停在原地，底下换成了别的页边，别马上把它探出来。 */
  const coolUntil = useRef(0);

  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  // 换了摊开的页、探出的那页被关掉：收回。
  const front = kinds[kinds.length - 1];
  useEffect(() => {
    if (peek && (peek === front || !kinds.includes(peek))) setPeek(null);
  }, [kinds, front, peek]);
  useEffect(() => () => void (timer.current && clearTimeout(timer.current)), []);

  const hover = (kind: SidePanel | null) => {
    if (timer.current) clearTimeout(timer.current);
    if (kind !== null && Date.now() < coolUntil.current) return;
    timer.current = kind === null ? null : window.setTimeout(() => setPeek(kind), HOVER_MS);
    if (kind === null) setPeek(null);
  };
  const flip = (kind: SidePanel) => {
    hover(null);
    coolUntil.current = Date.now() + 600;
    onFront(kind);
  };
  // 页太多、放不下：页边按比例收窄，摊开那页至少留一半。
  const edge = kinds.length > 1 ? Math.max(28, Math.min(EDGE, (width * 0.5) / (kinds.length - 1))) : EDGE;

  return (
    <div ref={box} className="relative min-w-0 flex-1 overflow-hidden">
      {kinds.map((kind, i) => {
        const isFront = i === kinds.length - 1;
        const peeking = peek === kind && !isFront;
        const left = PAD + i * edge;
        const full = Math.max(0, width - left - PAD);
        const w = peeking ? Math.min(PEEK, full) : full;
        const m = meta(kind);
        const Icon = m.icon;
        const count = badge?.(kind) ?? 0;
        return (
          <div
            key={kind}
            data-stack-layer={isFront ? "front" : peeking ? "peek" : "back"}
            onMouseLeave={peeking ? () => hover(null) : undefined}
            onPointerDownCapture={peeking ? () => flip(kind) : undefined}
            className={`stack-layer absolute flex overflow-hidden rounded-[14px] transition-[left,width,box-shadow,transform] duration-[var(--dur-base)] ease-[var(--ease-out)] ${
              peeking ? "stack-layer-peek" : isFront ? "stack-layer-front" : ""
            }`}
            style={{ left, width: w, top: PAD, bottom: PAD, zIndex: peeking ? 60 : i + 1 }}
          >
            {/* 面板本身：压在下面时藏起来但不卸。 */}
            <div className={`relative flex min-h-0 min-w-0 flex-1 ${isFront || peeking ? "" : "invisible"}`}>
              {render(kind)}
              {overlay?.(kind)}
            </div>
            {!isFront && !peeking && (
              <button
                type="button"
                title={`${m.label}：悬停看一眼，点一下翻到这一页`}
                onMouseEnter={() => hover(kind)}
                onMouseLeave={() => peek !== kind && hover(null)}
                onClick={() => flip(kind)}
                className="stack-edge absolute inset-y-0 left-0 flex flex-col items-center gap-1 pt-3.5 text-ink-2"
                style={{ width: edge }}
              >
                <Icon size={15} strokeWidth={1.75} className="mb-1 shrink-0" />
                {edge >= 36 &&
                  [...m.label].slice(0, 6).map((c, j) => (
                    <span key={j} className="text-[12px] font-medium leading-[15px]">
                      {c}
                    </span>
                  ))}
                {count > 0 && (
                  <span className="mt-2 flex size-[18px] items-center justify-center rounded-full bg-accent text-[10px] font-semibold text-white">
                    {count}
                  </span>
                )}
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
