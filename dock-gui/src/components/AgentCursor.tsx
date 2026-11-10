// 实时画面上的 agent 光标。设计：wonder-ui/pages/DesktopPanel/DesktopAgentCursorStates（rfc）、
// DesktopPanelLiveCursor（web）。
//
// 窗口截图里没有 agent 光标（cua-driver 画在屏幕浮层上），这里叠在画面上自己画。
// 位置是窗口里的相对坐标（0–1），用容器查询单位走 transform：只动 transform / opacity。
// 每个事件带 seq：轨迹、涟漪、影子、键帽按 seq 重新挂载，新动作来了直接打断旧的。
// 系统「减少动态效果」时只瞬移，不画轨迹和涟漪（styles.css 里的 media query）。

import { useEffect, useRef, useState } from "react";

/** 一次光标事件：网关的 `desktop/view/cursor`，或 agent 正在做的像素点击（提前滑过去）。 */
export interface CursorEvent {
  seq: number;
  /** 窗口里的相对位置；`null` = 不动（按键这类），留在原处。 */
  x: number | null;
  y: number | null;
  /** `move` / `click` / `double_click` / `right_click` / `drag` / `scroll` / `type` / `key`。 */
  action: string;
  label: string | null;
  keys: string[];
}

/** 滑过去要多久（和 styles.css 的 `.agent-cursor` transition 一致）。 */
const GLIDE_MS = 420;
/** 停手多久后变淡、多久后隐藏（隐藏跟 cua-driver 的 idle_hide 一致）。 */
const DIM_MS = 3000;
const HIDE_MS = 20000;

const VERB: Record<string, string> = {
  move: "移动",
  click: "点击",
  double_click: "双击 ×2",
  right_click: "右键",
  drag: "拖动",
  scroll: "滚动",
  type: "输入",
  key: "按键",
};

const SCROLL: Record<string, string> = { up: "向上滚动", down: "向下滚动", left: "向左滚动", right: "向右滚动" };

const KEY_GLYPH: Record<string, string> = {
  cmd: "⌘",
  command: "⌘",
  shift: "⇧",
  option: "⌥",
  alt: "⌥",
  ctrl: "⌃",
  control: "⌃",
  return: "↩",
  enter: "↩",
  tab: "⇥",
  escape: "⎋",
  esc: "⎋",
  delete: "⌫",
  backspace: "⌫",
  space: "␣",
  up: "↑",
  down: "↓",
  left: "←",
  right: "→",
};

function keyGlyph(key: string): string {
  const k = key.toLowerCase();
  return KEY_GLYPH[k] ?? (key.length === 1 ? key.toUpperCase() : key);
}

function tagText(e: CursorEvent): string {
  // 提前滑过去时 label 是那一步的动词（「点击」）。
  if (e.action === "move" && e.label) return e.label;
  if (e.action === "type" && e.label) return `输入「${e.label}」`;
  if (e.action === "scroll") return SCROLL[e.label ?? ""] ?? "滚动";
  return VERB[e.action] ?? e.action;
}

/** 窗口外一点点也画（贴边的按钮），再远就不画。 */
function onCanvas(x: number, y: number): boolean {
  return x >= -0.05 && x <= 1.05 && y >= -0.05 && y <= 1.05;
}

/** 光标层铺在哪：画面在 <img> 里实际占的矩形（不含 object-contain 的留白）。 */
export interface CursorRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export function AgentCursor({ event, rect }: { event: CursorEvent | null; rect: CursorRect }) {
  // 光标当前位置和上一个位置（画轨迹用）。
  const [at, setAt] = useState<{ x: number; y: number } | null>(null);
  const [from, setFrom] = useState<{ x: number; y: number } | null>(null);
  const [idle, setIdle] = useState<"active" | "dim" | "hidden">("active");
  const atRef = useRef(at);
  atRef.current = at;

  useEffect(() => {
    if (!event) return;
    if (event.x !== null && event.y !== null && onCanvas(event.x, event.y)) {
      const next = { x: event.x, y: event.y };
      const prev = atRef.current;
      // 同一点（提前滑过去之后、真实位置回来）不再画轨迹。
      const moved = !prev || Math.hypot(prev.x - next.x, prev.y - next.y) > 0.01;
      setFrom(moved ? prev : null);
      setAt(next);
    } else {
      setFrom(null);
    }
    setIdle("active");
    const dim = window.setTimeout(() => setIdle("dim"), DIM_MS);
    const hide = window.setTimeout(() => setIdle("hidden"), HIDE_MS);
    return () => {
      window.clearTimeout(dim);
      window.clearTimeout(hide);
    };
  }, [event]);

  if (!event || !at) return null;
  const still = event.action === "key";
  const showsEffect = event.action !== "move";
  // 靠右 / 靠下时标签和键帽翻到另一边，不被画面边缘裁掉。
  const flipX = at.x > 0.72;
  const flipY = at.y > 0.84;
  return (
    <div
      className={`agent-cursor-layer pointer-events-none absolute ${idle === "hidden" ? "opacity-0" : ""}`}
      style={{ ...rect, transition: "opacity var(--dur-slow) var(--ease-out)" }}
    >
      {from && <Trail key={`trail-${event.seq}`} from={from} to={at} dot={event.action === "drag"} />}
      {showsEffect && <Ripple key={`ripple-${event.seq}`} at={at} action={event.action} />}
      <div
        className="agent-cursor absolute left-0 top-0"
        style={{ transform: `translate(${at.x * 100}cqw, ${at.y * 100}cqh)` }}
      >
        <div
          key={`arrow-${event.seq}`}
          className={`agent-cursor-arrow ${showsEffect && !still ? "agent-cursor-press" : ""}`}
          style={{ opacity: still || idle === "dim" ? 0.55 : 1 }}
        >
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none" aria-hidden className="-ml-[4px] -mt-[2.5px]">
            <path
              d="M4 2.5 L4 19.5 L8.6 15.2 L11.6 21.8 L14.6 20.5 L11.6 13.9 L17.8 13.9 Z"
              fill="#0a84ff"
              stroke="#ffffff"
              strokeWidth="1.6"
              strokeLinejoin="round"
            />
          </svg>
        </div>
        {event.action === "drag" && (
          <div
            key={`ghost-${event.seq}`}
            className="agent-cursor-ghost absolute left-[16px] top-[18px] flex h-[34px] w-[28px] items-end justify-center rounded-[3px] border border-[#d1d1d6] bg-white pb-1 shadow-[0px_6px_14px_rgba(0,0,0,0.18)]"
          >
            <span className="h-1 w-3 rounded-full bg-[#d1d1d6]" />
          </div>
        )}
        {event.action === "scroll" && (
          <div
            key={`scroll-${event.seq}`}
            className="agent-cursor-nudge absolute -left-[30px] -top-[4px] flex size-7 items-center justify-center rounded-full border border-[#e5e5ea] bg-white shadow-[0px_4px_10px_rgba(0,0,0,0.12)]"
            style={{ ["--nudge-rot" as string]: scrollRotation(event.label) }}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#0a84ff" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
              <polyline points="6 9 12 15 18 9" />
            </svg>
          </div>
        )}
        {still && event.keys.length > 0 ? (
          <div
            key={`keys-${event.seq}`}
            className={`agent-cursor-keys absolute flex items-center gap-1 ${flipX ? "right-[-6px]" : "left-[-10px]"} ${
              flipY ? "bottom-[8px]" : "top-[28px]"
            }`}
          >
            {event.keys.map((k, i) => (
              <span
                key={i}
                className="flex h-6 min-w-6 items-center justify-center rounded-md bg-[#1c1c1e] px-1.5 text-xs font-medium text-white shadow-[0px_2px_0px_#000]"
              >
                {keyGlyph(k)}
              </span>
            ))}
          </div>
        ) : (
          <div
            className={`absolute flex max-w-[220px] items-center gap-1 whitespace-nowrap rounded-full bg-[#0a84ff] py-0.5 pl-1.5 pr-2 shadow-[0px_4px_10px_rgba(10,132,255,0.35)] ${
              flipX ? "right-[6px]" : "left-[20px]"
            } ${flipY ? "bottom-[6px]" : "top-[24px]"}`}
            style={{ opacity: idle === "dim" ? 0 : 1, transition: "opacity var(--dur-base) var(--ease-out)" }}
          >
            <span className="text-[10px] font-semibold text-white">Agent</span>
            <span className="truncate text-[10px] text-white/80">· {tagText(event)}</span>
          </div>
        )}
      </div>
    </div>
  );
}

function scrollRotation(direction: string | null): string {
  switch (direction) {
    case "up":
      return "180deg";
    case "left":
      return "90deg";
    case "right":
      return "-90deg";
    default:
      return "0deg";
  }
}

/** 从上一个位置到这里的虚线弧，到点后淡出。坐标系是窗口的 0–100。 */
function Trail({ from, to, dot }: { from: { x: number; y: number }; to: { x: number; y: number }; dot: boolean }) {
  const [x1, y1, x2, y2] = [from.x * 100, from.y * 100, to.x * 100, to.y * 100];
  // 控制点：中点往「上方」的法线方向偏，弧度跟距离成比例。
  const [mx, my] = [(x1 + x2) / 2, (y1 + y2) / 2];
  const len = Math.hypot(x2 - x1, y2 - y1) || 1;
  let [nx, ny] = [-(y2 - y1) / len, (x2 - x1) / len];
  if (ny > 0) [nx, ny] = [-nx, -ny];
  const bend = Math.min(len * 0.35, 18);
  const d = `M${x1} ${y1} Q ${mx + nx * bend} ${my + ny * bend} ${x2} ${y2}`;
  return (
    <svg className="agent-cursor-trail absolute inset-0 h-full w-full overflow-visible" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden>
      <path d={d} stroke="#0a84ff" strokeOpacity="0.55" strokeWidth="2" strokeDasharray="3 6" strokeLinecap="round" fill="none" vectorEffect="non-scaling-stroke" />
      {dot && <circle cx={x1} cy={y1} r="0.6" fill="#0a84ff" fillOpacity="0.45" />}
    </svg>
  );
}

/** 到点后的涟漪：点击实心、双击两圈、右键虚线、拖动松手落点。 */
function Ripple({ at, action }: { at: { x: number; y: number }; action: string }) {
  if (!["click", "double_click", "right_click", "drag"].includes(action)) return null;
  const ring = action === "right_click" ? "border-2 border-dashed border-[rgba(10,132,255,0.55)]" : "border-2 border-[rgba(10,132,255,0.45)]";
  const rings = action === "double_click" ? [0, 120] : [0];
  return (
    <div className="agent-cursor absolute left-0 top-0" style={{ transform: `translate(${at.x * 100}cqw, ${at.y * 100}cqh)`, transition: "none" }}>
      {rings.map((delay) => (
        <span
          key={delay}
          className={`agent-cursor-ripple absolute -left-6 -top-6 size-12 rounded-full ${ring} ${action === "click" ? "bg-[rgba(10,132,255,0.12)]" : ""}`}
          style={{ animationDelay: `${GLIDE_MS + delay}ms` }}
        />
      ))}
    </div>
  );
}
