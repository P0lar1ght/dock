// 分屏中间的竖线：拖动改左右比例（设计 SplitPanels 默认 55 : 45）。命中区比线宽，
// 悬停 / 拖动时线变蓝。双击恢复默认。
// `pair`：比例只算左右相邻两栏（右侧多面板一排兄弟节点，见 SidePanelsSplit），默认按父行算。
//
// 拖动期间 <html> 挂 `split-dragging`（styles.css）：iframe 不接指针。旁边的画布是 iframe，
// 鼠标一进去 pointermove 就发给 iframe 的 document，主窗口收不到，线就不跟手，松手也收不到。

import { useRef, useState, type RefObject } from "react";

export const DEFAULT_SPLIT = 0.55;
const MIN = 0.3;
const MAX = 0.7;

export function SplitDivider({
  onRatio,
  onCommit,
  defaultRatio = DEFAULT_SPLIT,
  pair = false,
  min = MIN,
  max = MAX,
  measureRef,
}: {
  /** 拖动中每帧最多一次。 */
  onRatio: (ratio: number) => void;
  /** 松手 / 双击时的最终比例，用来落盘。 */
  onCommit?: (ratio: number) => void;
  /** 双击恢复到的比例（浏览器面板和分屏各有各的默认）。 */
  defaultRatio?: number;
  /** 比例 = 左邻占「左邻 + 右邻」多少，而不是占整行。 */
  pair?: boolean;
  /** 比例的测量基准（一般传整行那个元素）；不给就用父元素。 */
  measureRef?: RefObject<HTMLElement | null>;
  min?: number;
  max?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    const el = ref.current;
    const base = measureRef?.current ?? el?.parentElement;
    const span = pair ? pairBox(el) : base?.getBoundingClientRect();
    if (!span) return;
    e.preventDefault();
    const box = { left: span.left, width: span.width };
    const root = document.documentElement;
    root.classList.add("split-dragging");
    setDragging(true);
    let latest: number | null = null;
    let frame = 0;
    const move = (ev: PointerEvent) => {
      latest = Math.min(max, Math.max(min, (ev.clientX - box.left) / box.width));
      if (!frame)
        frame = requestAnimationFrame(() => {
          frame = 0;
          if (latest !== null) onRatio(latest);
        });
    };
    const end = () => {
      cancelAnimationFrame(frame);
      root.classList.remove("split-dragging");
      setDragging(false);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      window.removeEventListener("blur", end);
      if (latest !== null) {
        onRatio(latest);
        onCommit?.(latest);
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
    window.addEventListener("blur", end);
  };

  return (
    <div
      ref={ref}
      role="separator"
      aria-orientation="vertical"
      title="拖动调整宽度，双击恢复"
      onPointerDown={onPointerDown}
      onDoubleClick={() => {
        onRatio(defaultRatio);
        onCommit?.(defaultRatio);
      }}
      className="group relative z-10 -mx-[3px] w-[7px] shrink-0 cursor-col-resize"
    >
      <div
        className={`motion-colors absolute inset-y-0 left-[3px] w-px ${
          dragging ? "bg-accent" : "glass-hairline bg-line-strong group-hover:bg-accent"
        }`}
      />
    </div>
  );
}

/** 左右相邻两栏合起来的横向范围。 */
function pairBox(el: HTMLDivElement | null): { left: number; width: number } | null {
  const a = el?.previousElementSibling?.getBoundingClientRect();
  const b = el?.nextElementSibling?.getBoundingClientRect();
  return a && b ? { left: a.left, width: b.right - a.left } : null;
}
