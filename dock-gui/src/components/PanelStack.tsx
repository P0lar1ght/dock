// 右侧面板叠成一本书（2026-11-28 定：翻书 + 3D，原型见对话里的 /tmp/dock-book）。
//
// - 书页顺序就是顶上标签的顺序（`kinds`，开的先后，翻页不挪位）；`active` 是翻到的那页，平铺在中间。
//   它前面的页向左、后面的页向右，绕这一栏的左右边（书脊）转开、往纵深退去，露出压扁了的真实内容；
//   离翻到的那页越近翻得越开（多露一点），越远越合。
// - 两侧书页一共最多占这一栏的两成（且不超过 120px），翻到的那页至少八成：在这个前提下尽量翻开。
//   太窄（< 380px）、页太多、系统要求减少动效时退回平铺：每页只露一条窄页边（名字在标签上）。
// - 悬停书页或它的标签（`hovered`，和标签行共用）：那一页翻开到 30° 浮到最上面看一眼；移开收回。
// - 点一下（书页或标签）：翻到那一页。跨过好几页时一页接一页翻过去。
// - 没翻到的页只是预览：内容 inert，整页是一个「翻到这页」的按钮。
// - 每页一直挂着（只换位置和角度）：画布 iframe、浏览器画面不重连。页按 kind 做 key。
// - 拖出成独立窗口照旧走各面板头栏的拖手（PanelGrip）。

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import type { SidePanel } from "./ToolPanel";

/** 页与容器边缘的空隙。 */
const PAD = 8;
/** 同一侧相邻两页的书脊错开多少。 */
const GAP = 8;
/** 自动角度的范围：最开 / 最合。 */
const MIN_DEG = 55;
const MAX_DEG = 82;
/** 每远一页多合上几度（扇形）。 */
const FAN_DEG = 5;
/** 再合也不超过这个角度（再大就成一条线了）。 */
const CAP_DEG = 86;
/** 悬停看一眼时翻开到的角度。 */
const PEEK_DEG = 30;
/** 两侧书页一共最多占这一栏的多少（比例 / 像素，取小的）：翻到的那页才是用户干活的地方。 */
const SIDE_SHARE = 0.2;
const SIDE_MAX = 120;
/** 比这窄、或开的页比这多，退回平铺。 */
const BOOK_MIN_WIDTH = 380;
const BOOK_MAX_PAGES = 7;
/** 平铺时每页露出的页边宽（页多时按比例收窄，一共不超过栏宽的 FLAT_SHARE）。 */
const EDGE = 14;
const FLAT_SHARE = 0.12;
/** 页边窄过这个就不画图标了（只剩一条，名字和图标在标签上）。 */
const EDGE_ICON = 11;
/** 平铺时悬停探出的宽度上限。 */
const PEEK = 380;
/** 悬停多久才翻开看（掠过不打扰）。 */
const HOVER_MS = 140;
/** 翻一页的时长；跨好几页时每页错开 STAGGER_MS。 */
const FLIP_MS = 420;
const STAGGER_MS = 50;
const EASE = "cubic-bezier(0.2, 0.8, 0.2, 1)";

export interface PanelMeta {
  label: string;
  icon: LucideIcon;
}

type Side = "left" | "front" | "right";

interface Slot {
  side: Side;
  left: number;
  width: number;
  /** 绕哪条边转（书脊在哪边）。 */
  origin: "left" | "right";
  /** rotateY 的角度：左侧正、右侧负，翻到的那页 0。 */
  deg: number;
}

/** 透视距离：栏越宽越远，免得宽的时候书页拉得太夸张。 */
const perspectiveFor = (width: number) => Math.max(900, width * 2);

/**
 * 3D 书：每页的位置和角度。放不下（太窄 / 页太多）回 null，退回平铺。
 * 书页绕自己的书脊转开后投影的宽度取决于翻到的那页有多宽（所有页同宽），
 * 那页的宽度又取决于两侧占了多少：迭代几次就收敛。
 */
function bookSlots(n: number, at: number, width: number): Slot[] | null {
  if (width < BOOK_MIN_WIDTH || n > BOOK_MAX_PAGES) return null;
  const left = at;
  const right = n - 1 - at;
  const p = perspectiveFor(width);
  const cx = width / 2;
  const cap = Math.min(width * SIDE_SHARE, SIDE_MAX);
  const fit = (deg: number) => {
    const th = (deg * Math.PI) / 180;
    let fl = PAD;
    let fr = width - PAD;
    let total = 0;
    for (let i = 0; i < 6; i++) {
      const w = fr - fl;
      // 书页远端退到纵深 w·sinθ，投影时往视点中心缩。
      const k = p / (p + w * Math.sin(th));
      const farLeft = cx + (PAD + (left - 1) * GAP + w * Math.cos(th) - cx) * k;
      const farRight = cx + (width - PAD - (right - 1) * GAP - w * Math.cos(th) - cx) * k;
      const ls = left ? farLeft - PAD + 6 : 0;
      const rs = right ? width - PAD - farRight + 6 : 0;
      total = ls + rs;
      // 超了就按比例让：书页远端藏到翻到的那页后面去。
      const s = total > cap ? cap / total : 1;
      fl = PAD + ls * s;
      fr = width - PAD - rs * s;
    }
    return { fl, fr, fits: total <= cap + 0.5 };
  };
  let deg = MAX_DEG;
  let box = fit(MAX_DEG);
  for (let d = MIN_DEG; d < MAX_DEG; d++) {
    const b = fit(d);
    if (b.fits) {
      deg = d;
      box = b;
      break;
    }
  }
  const w = Math.round(box.fr - box.fl);
  return Array.from({ length: n }, (_, i): Slot => {
    if (i === at) return { side: "front", left: box.fl, width: w, origin: "left", deg: 0 };
    const fan = Math.min(CAP_DEG, deg + (Math.abs(i - at) - 1) * FAN_DEG);
    if (i < at) return { side: "left", left: PAD + i * GAP, width: w, origin: "left", deg: fan };
    return { side: "right", left: width - PAD - (n - 1 - i) * GAP - w, width: w, origin: "right", deg: -fan };
  });
}

/** 平铺退路：没翻到的页压在翻到的那页底下，左右各露一条页边。 */
function flatSlots(n: number, at: number, width: number): { slots: Slot[]; edge: number } {
  const edge = n > 1 ? Math.max(4, Math.min(EDGE, (width * FLAT_SHARE) / (n - 1))) : EDGE;
  const fl = PAD + at * edge;
  const fr = width - PAD - (n - 1 - at) * edge;
  const w = Math.max(0, fr - fl);
  const slots = Array.from({ length: n }, (_, i): Slot => {
    if (i === at) return { side: "front", left: fl, width: w, origin: "left", deg: 0 };
    if (i < at) return { side: "left", left: PAD + i * edge, width: w, origin: "left", deg: 0 };
    return { side: "right", left: width - PAD - (n - 1 - i) * edge - w, width: w, origin: "right", deg: 0 };
  });
  return { slots, edge };
}

function useReducedMotion() {
  const query = "(prefers-reduced-motion: reduce)";
  const [reduced, setReduced] = useState(() => window.matchMedia?.(query).matches ?? false);
  useEffect(() => {
    const m = window.matchMedia?.(query);
    if (!m) return;
    const on = () => setReduced(m.matches);
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  }, []);
  return reduced;
}

export function PanelStack({
  kinds,
  active,
  hovered,
  onHover,
  render,
  meta,
  badge,
  overlay,
  onFront,
}: {
  /** 书页顺序（= 标签顺序）。 */
  kinds: SidePanel[];
  /** 翻到的那页。 */
  active: SidePanel;
  /** 悬停着的那一页（标签或书页上，和标签行共用）。 */
  hovered: SidePanel | null;
  onHover: (kind: SidePanel | null) => void;
  render: (kind: SidePanel) => ReactNode;
  meta: (kind: SidePanel) => PanelMeta;
  /** 书页上的蓝色数字（0 不画）。 */
  badge?: (kind: SidePanel) => number;
  /** 盖在那一页上的东西（拖出窗口时的描边提示）。 */
  overlay?: (kind: SidePanel) => ReactNode;
  onFront: (kind: SidePanel) => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [peek, setPeek] = useState<SidePanel | null>(null);
  /** 刚翻过页：光标停在原地，底下换成了别的页，别马上把它翻开。 */
  const coolUntil = useRef(0);
  const reduced = useReducedMotion();

  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  // 悬停（标签或书页）停够 HOVER_MS 才翻开看；移开、翻到它、它被关掉：立刻收回。
  useEffect(() => {
    if (!hovered || hovered === active || !kinds.includes(hovered) || Date.now() < coolUntil.current) {
      setPeek(null);
      return;
    }
    const t = window.setTimeout(() => setPeek(hovered), HOVER_MS);
    return () => window.clearTimeout(t);
  }, [hovered, active, kinds]);

  const flip = (kind: SidePanel) => {
    onHover(null);
    setPeek(null);
    coolUntil.current = Date.now() + 600;
    onFront(kind);
  };

  const n = kinds.length;
  const at = Math.max(0, kinds.indexOf(active));
  const book = reduced ? null : bookSlots(n, at, width);
  const flat = book ? null : flatSlots(n, at, width);
  const slots = book ?? flat!.slots;

  // 翻页时跨过的那几页一页接一页翻：往后翻是旧的那页先翻走，往前翻是离旧页近的先翻回来。
  const prevAt = useRef(at);
  const from = prevAt.current;
  useEffect(() => {
    prevAt.current = at;
  });
  const delayOf = (i: number) => {
    if (from === at || from >= n || reduced) return 0;
    if (at > from && i >= from && i <= at) return (i - from) * STAGGER_MS;
    if (at < from && i <= from && i >= at) return (from - i) * STAGGER_MS;
    return 0;
  };
  // 翻到的那页绕它来的那一侧的书脊转平：记住每页上次在哪边。
  const origins = useRef(new Map<SidePanel, "left" | "right">());

  return (
    <div
      ref={box}
      className={`relative min-w-0 flex-1 overflow-hidden ${book ? "stack-book" : ""}`}
      style={book ? { perspective: perspectiveFor(width) } : undefined}
    >
      {kinds.map((kind, i) => {
        const slot = slots[i];
        const isFront = slot.side === "front";
        const peeking = !isFront && peek === kind;
        if (!isFront) origins.current.set(kind, slot.origin);
        const origin = isFront ? (origins.current.get(kind) ?? "left") : slot.origin;
        let { left, width: w, deg } = slot;
        if (peeking && book) deg = slot.side === "left" ? PEEK_DEG : -PEEK_DEG;
        if (peeking && !book) {
          // 平铺：从自己那条页边往里探出（左边的往右、右边的往左）。
          const wide = Math.min(PEEK, width - 2 * PAD);
          if (slot.side === "right") left = left + w - wide;
          w = wide;
        }
        const m = meta(kind);
        const Icon = m.icon;
        const count = badge?.(kind) ?? 0;
        const delay = delayOf(i);
        const motion = reduced
          ? undefined
          : ["left", "width", "transform"].map((prop) => `${prop} ${FLIP_MS}ms ${EASE} ${delay}ms`).join(", ") +
            ", box-shadow var(--dur-base) var(--ease-out)";
        return (
          <div
            key={kind}
            data-stack-layer={isFront ? "front" : peeking ? "peek" : "back"}
            className={`stack-layer absolute flex overflow-hidden rounded-[14px] ${
              isFront ? "stack-layer-front" : peeking ? "stack-layer-peek" : book ? "stack-layer-book" : ""
            }`}
            style={{
              left,
              width: w,
              top: PAD,
              bottom: PAD,
              zIndex: isFront ? 100 : peeking ? 150 : slot.side === "left" ? 10 + i : 60 - i,
              transformOrigin: `${origin} center`,
              // 翻到的那页静止时不留 transform：不然一直是合成层，字会糊。
              transform: deg ? `rotateY(${deg}deg)` : "none",
              transition: motion,
            }}
          >
            {/* 面板本身：没翻到的只是预览（3D 里看得见压扁的内容，平铺里藏起来），不接收输入。 */}
            <div
              inert={!isFront}
              className={`relative flex min-h-0 min-w-0 flex-1 ${isFront || peeking || book ? "" : "invisible"}`}
            >
              {render(kind)}
              {overlay?.(kind)}
            </div>
            {!isFront && (
              <button
                type="button"
                title={`${m.label}：悬停看一眼，点一下翻到这一页`}
                aria-label={m.label}
                onMouseEnter={() => onHover(kind)}
                onMouseLeave={() => onHover(null)}
                onClick={() => flip(kind)}
                className={`absolute inset-0 flex ${slot.side === "right" ? "justify-end" : "justify-start"} ${
                  book ? "stack-shade" : ""
                } ${peeking ? "stack-shade-off" : ""}`}
              >
                {!book &&
                  !peeking && (
                    <span
                      className="stack-edge flex h-full shrink-0 flex-col items-center gap-1.5 pt-3.5 text-ink-2"
                      style={{ width: flat!.edge }}
                    >
                      {flat!.edge >= EDGE_ICON && <Icon size={11} strokeWidth={1.9} className="shrink-0" />}
                      {count > 0 && <span className="size-1.5 shrink-0 rounded-full bg-accent" />}
                    </span>
                  )}
              </button>
            )}
          </div>
        );
      })}
      {/* 3D 里每页在书脊那头一个书签（像活页夹的索引签）：不跟着页转，转得再合也认得出、点得着。
          同一侧按书页顺序自上而下错开；悬停和顶上标签联动。 */}
      {book &&
        kinds.map((kind, i) => {
          const slot = slots[i];
          const isFront = slot.side === "front";
          const order = slot.side === "left" ? i : i - at - 1;
          const m = meta(kind);
          const Icon = m.icon;
          const count = badge?.(kind) ?? 0;
          const edge = slot.side === "left" ? slot.left : width - (slot.left + slot.width);
          return (
            <button
              key={`tag:${kind}`}
              type="button"
              tabIndex={isFront ? -1 : 0}
              aria-hidden={isFront || undefined}
              title={`${m.label}：悬停看一眼，点一下翻到这一页`}
              onMouseEnter={() => onHover(kind)}
              onMouseLeave={() => onHover(null)}
              onClick={() => flip(kind)}
              className={`stack-tag absolute flex h-6 items-center gap-1 rounded-full px-1.5 text-ink-2 ${
                hovered === kind ? "stack-tag-lit" : ""
              } ${isFront ? "pointer-events-none opacity-0" : "opacity-100"}`}
              style={{
                top: PAD + 10 + Math.max(0, order) * 30,
                ...(slot.side === "right" ? { right: edge } : { left: edge }),
                zIndex: 120,
                transition: reduced
                  ? undefined
                  : `top ${FLIP_MS}ms ${EASE}, left ${FLIP_MS}ms ${EASE}, right ${FLIP_MS}ms ${EASE}, opacity var(--dur-base) var(--ease-out)`,
              }}
            >
              <Icon size={13} strokeWidth={1.9} />
              {count > 0 && <span className="text-[10px] font-semibold text-accent tabular-nums">{count}</span>}
            </button>
          );
        })}
    </div>
  );
}
