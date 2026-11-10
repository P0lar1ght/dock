// 对话流左边的消息导航：每条用户消息一根短横线，当前看到的那条加长加深。
// 悬停弹出消息列表，指着的那条展开（时间 + 回复开头）；点一下滚到那条消息。
// 只认用户消息，内容都来自 dock-core 的轮次，不在这里推导别的。

import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import type { Turn, TurnItem } from "dock-core";
import { stripImageChips } from "../dock/chips";

/** 对话流里每轮的根节点带这个属性（值是轮次 id），导航按它找位置。 */
export const NAV_ATTR = "data-nav-turn";

/** 当前那条 = 顶边在视口上方这个比例以内的最后一条。 */
const ACTIVE_LINE = 0.3;
/** 离开横线 / 列表这么久才收起：从横线挪进列表中间有空隙。 */
const CLOSE_MS = 160;

export interface NavEntry {
  turnId: string;
  text: string;
  at: number;
  reply: string;
  running: boolean;
}

type UserItem = Extract<TurnItem, { kind: "user" }>;

// dock-core 保证没变的轮次是同一个引用：流式增量只重算在跑的那一轮。
const cache = new WeakMap<Turn, NavEntry | null>();

export function navEntries(turns: Turn[]): NavEntry[] {
  const out: NavEntry[] = [];
  for (const turn of turns) {
    let entry = cache.get(turn);
    if (entry === undefined) {
      entry = toEntry(turn);
      cache.set(turn, entry);
    }
    if (entry) out.push(entry);
  }
  return out;
}

function toEntry(turn: Turn): NavEntry | null {
  const users = turn.items.filter((i): i is UserItem => i.kind === "user");
  if (users.length === 0) return null;
  const text = users
    .map((u) => stripImageChips(u.text))
    .join(" ")
    .replace(/\s+/g, " ")
    .trim();
  // 回复 = 最后一段连续的助手文字（同对话流里不折进过程的那段）。
  const rest = turn.items.filter((i) => i.kind !== "user" && i.kind !== "sideNote");
  let cut = rest.length;
  while (cut > 0 && rest[cut - 1].kind === "text") cut--;
  const reply = rest
    .slice(cut)
    .map((i) => (i.kind === "text" ? i.text : ""))
    .join(" ");
  return {
    turnId: turn.id,
    text: text || (users[0].attachments.length ? "[图片]" : ""),
    at: users[0].at,
    reply: plain(reply),
    running: turn.status === "running",
  };
}

/** 摘要用：去掉常见 Markdown 记号，压成一行。 */
function plain(md: string): string {
  return md
    .replace(/```[\s\S]*?(```|$)/g, " ")
    .replace(/^\s{0,3}(#{1,6}|>|[-*+]|\d+\.)\s+/gm, "")
    .replace(/\*\*|__|`/g, "")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

export function TurnNav({
  entries,
  scrollRef,
  onJump,
  formatTime,
}: {
  entries: NavEntry[];
  scrollRef: RefObject<HTMLDivElement | null>;
  onJump: (turnId: string) => void;
  formatTime: (ms: number) => string;
}) {
  const [active, setActive] = useState(-1);
  const [overflow, setOverflow] = useState(false);
  const [open, setOpen] = useState(false);
  /** 展开的那条：指着的横线或列表项；没指着时是当前那条。 */
  const [pointed, setPointed] = useState<number | null>(null);
  const closeTimer = useRef(0);
  const frame = useRef(0);
  const railRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // 跟着滚动算当前那条；内容长高（流式、展开）也要重算，一帧最多一次。
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = () => {
      frame.current = 0;
      setOverflow(el.scrollHeight > el.clientHeight + 1);
      const top = el.getBoundingClientRect().top;
      const nodes = el.querySelectorAll<HTMLElement>(`[${NAV_ATTR}]`);
      // 到底了就是最后一条：最后几轮很短时顶边够不到判定线。
      if (el.scrollHeight - el.scrollTop - el.clientHeight <= 1) {
        setActive(nodes.length - 1);
        return;
      }
      let idx = 0;
      nodes.forEach((n, i) => {
        if (n.getBoundingClientRect().top - top <= el.clientHeight * ACTIVE_LINE) idx = i;
      });
      setActive(idx);
    };
    const schedule = () => {
      if (!frame.current) frame.current = requestAnimationFrame(measure);
    };
    schedule();
    el.addEventListener("scroll", schedule, { passive: true });
    const ro = new ResizeObserver(schedule);
    ro.observe(el);
    if (el.firstElementChild) ro.observe(el.firstElementChild);
    return () => {
      el.removeEventListener("scroll", schedule);
      ro.disconnect();
      cancelAnimationFrame(frame.current);
      frame.current = 0;
    };
  }, [scrollRef, entries.length]);

  useEffect(() => () => window.clearTimeout(closeTimer.current), []);

  // 横线多了竖条装不下：把当前那条挪到中间。
  useLayoutEffect(() => {
    const rail = railRef.current;
    const line = rail?.children[active] as HTMLElement | undefined;
    if (!rail || !line || rail.scrollHeight <= rail.clientHeight) return;
    rail.scrollTop = line.offsetTop - rail.clientHeight / 2;
  }, [active, entries.length]);

  const expanded = pointed ?? active;

  // 列表打开 / 换了展开的那条：让它在列表里看得见。
  useLayoutEffect(() => {
    if (!open) return;
    const item = listRef.current?.children[expanded] as HTMLElement | undefined;
    item?.scrollIntoView({ block: "nearest" });
  }, [open, expanded]);

  if (entries.length < 2 || !overflow) return null;

  const show = () => {
    window.clearTimeout(closeTimer.current);
    setOpen(true);
  };
  const hide = () => {
    window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => {
      setOpen(false);
      setPointed(null);
    }, CLOSE_MS);
  };
  const jump = (i: number) => {
    onJump(entries[i].turnId);
    setOpen(false);
    setPointed(null);
  };

  return (
    <div
      className="absolute left-1.5 top-1/2 z-20 -translate-y-1/2"
      onMouseEnter={show}
      onMouseLeave={hide}
    >
      <div
        ref={railRef}
        role="navigation"
        aria-label="消息导航"
        // 横线盖在对话流的左留白上：在它上面滚轮照样滚对话。
        onWheel={(e) => scrollRef.current?.scrollBy({ top: e.deltaY })}
        className="flex max-h-[min(50vh,360px)] flex-col overflow-hidden py-1"
      >
        {entries.map((e, i) => (
          <button
            key={e.turnId}
            type="button"
            aria-label={`跳到：${e.text}`}
            aria-current={i === active || undefined}
            onClick={() => jump(i)}
            onMouseEnter={() => setPointed(i)}
            className="group flex h-2 w-7 shrink-0 items-center pl-1"
          >
            <span
              className={`h-[2px] rounded-full transition-[width,background-color] duration-[var(--dur-fast)] ease-[var(--ease-out)] ${
                i === active ? "w-4 bg-ink-2" : i === expanded && open ? "w-3.5 bg-ink-3" : "w-2.5 bg-ink-5 group-hover:bg-ink-3"
              }`}
            />
          </button>
        ))}
      </div>
      {open && (
        // 定位和动画分两层：pop-in 占 transform，和 -translate-y-1/2 写在一起会被盖掉。
        <div className="absolute left-full top-1/2 -translate-y-1/2 pl-1.5">
          <div
            ref={listRef}
            className="glass-popover pop-in thin-scroll flex max-h-[min(60vh,480px)] w-[340px] origin-left flex-col items-start gap-1.5 overflow-y-auto rounded-[16px] border border-line-strong bg-white p-2 shadow-[0px_12px_36px_rgba(0,0,0,0.14)]"
            onMouseLeave={() => setPointed(null)}
          >
            {entries.map((e, i) => {
              const big = i === expanded;
              return (
                <button
                  key={e.turnId}
                  type="button"
                  onClick={() => jump(i)}
                  onMouseEnter={() => setPointed(i)}
                  className={`motion-colors flex max-w-full flex-col rounded-[12px] px-3 py-2 text-left ${
                    big ? "w-full bg-fill" : "hover:bg-hover"
                  }`}
                >
                  <span
                    className={`min-w-0 text-[13.5px] leading-[1.5] ${big ? "line-clamp-2 font-medium" : "truncate"} ${
                      i === active ? "text-accent" : big ? "text-ink" : "text-ink-2"
                    }`}
                  >
                    {e.text}
                  </span>
                  <span className="fold" data-open={big}>
                    <span className="flex flex-col gap-1 pt-1">
                      {e.at > 0 && <span className="text-[12px] text-ink-4">{formatTime(e.at)}</span>}
                      {(e.reply || e.running) && (
                        <span className="line-clamp-3 text-[12.5px] leading-[1.55] text-ink-3">
                          {e.reply || "正在回复…"}
                        </span>
                      )}
                    </span>
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
