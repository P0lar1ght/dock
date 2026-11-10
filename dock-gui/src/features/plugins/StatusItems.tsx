// 插件状态项：输入框工具栏右组最前面的一串胶囊。设计：wonder-ui/plugins/StatusItemsComposer（uni）、
// StatusItemsOverflow（tum）。
// - 文字是插件给的原文：只截断（最大宽 120），不改写。
// - 图标由色调决定（success / warning / danger / accent），default / muted 不带图标。
// - 最多 3 个，工具栏放不下就少摆几个，多的收进「+N」；再挤就只留「+N」，连它都放不下就先藏起来。
// - 悬停 tooltip 往上弹：被截断时第一行是完整原文，再接插件的说明。点它打开对应的插件面板。

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { CircleCheck, CircleX, Loader, TriangleAlert, type LucideIcon } from "lucide-react";
import type { StatusItem, Tone } from "dock-core";
import { Popover } from "../../components/Popover";
import { useFeatureHost } from "../registry";
import { onPluginPush, statusList } from "./api";
import { TONE_PILL } from "./ViewTree";
import { surfaceKind } from "./panelKind";

const TONE_ICON: Partial<Record<Tone, LucideIcon>> = {
  success: CircleCheck,
  warning: TriangleAlert,
  danger: CircleX,
  accent: Loader,
};

/** 最多摆几个，多的收进「+N」。 */
const MAX = 3;
/** 工具栏连「+N」都放不下：整组藏起来。 */
const HIDDEN = -1;
/** 胶囊之间、和工具栏别的按钮之间的间距（gap-1.5）。 */
const GAP = 6;

/** 状态项是全局的（不分会话）：拉一次，`status/changed` 时重拉。 */
function useStatusItems(): StatusItem[] {
  const [items, setItems] = useState<StatusItem[]>([]);
  useEffect(() => {
    let alive = true;
    const load = () =>
      statusList().then(
        (list) => alive && setItems(list),
        () => alive && setItems([]),
      );
    void load();
    const off = onPluginPush((p) => p.kind === "status" && void load());
    return () => {
      alive = false;
      off();
    };
  }, []);
  return items;
}

/**
 * 按工具栏里实际剩下的宽度决定摆几个：工具栏一行 = 左组（不换行的自然宽）+ 右组（我们 + 别的按钮）。
 * 胶囊宽度从一份隐藏的测量副本里量。放不下时先收进「+N」，至少留一个。
 */
function useFit(
  wrapRef: React.RefObject<HTMLDivElement | null>,
  measureRef: React.RefObject<HTMLDivElement | null>,
  /** 状态项的文字签名：文字变了胶囊宽度就变，要重量。 */
  signature: string,
): number {
  const [fit, setFit] = useState(MAX);
  useLayoutEffect(() => {
    const wrap = wrapRef.current;
    const right = wrap?.parentElement;
    const row = right?.parentElement;
    const left = row?.firstElementChild as HTMLElement | null;
    if (!wrap || !right || !row || !left || signature === "") return;
    const measure = () => {
      const kids = [...(measureRef.current?.children ?? [])] as HTMLElement[];
      const plus = kids.pop()?.offsetWidth ?? 0;
      const widths = kids.map((k) => k.offsetWidth);
      const others = [...right.children].filter((c) => c !== wrap).reduce((sum, c) => sum + (c as HTMLElement).offsetWidth + GAP, 0);
      // 左组的自然宽：临时不换行量一次再还原（同步，不会画出来）。常驻 nowrap 会把整列撑宽。
      const wrapMode = left.style.whiteSpace;
      left.style.whiteSpace = "nowrap";
      const leftWidth = left.scrollWidth;
      left.style.whiteSpace = wrapMode;
      // 行内左右两组之间 gap-2；我们自己右边有 pr-1.5 + 分隔线（7）。
      const free = row.clientWidth - leftWidth - 8 - others;
      const used = (k: number) => {
        const pills = widths.slice(0, k).reduce((a, b) => a + b, 0) + GAP * Math.max(0, k - 1);
        const more = k < widths.length ? plus + (k > 0 ? GAP : 0) : 0;
        return pills + more + 7 + GAP;
      };
      // 放得下几个摆几个（最多 3）；一个都放不下就只留「+N」；连它都放不下就整组藏起来，绝不压住别的按钮。
      let k = Math.min(MAX, widths.length);
      while (k > 0 && used(k) > free) k -= 1;
      if (k === 0 && used(0) > free) k = HIDDEN;
      setFit(k);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(row);
    ro.observe(left);
    return () => ro.disconnect();
  }, [wrapRef, measureRef, signature]);
  return fit;
}

export function StatusItems() {
  return <StatusItemsView items={useStatusItems()} />;
}

/** 不连 Dock 的画法（预览页直接喂假数据）。 */
export function StatusItemsView({ items }: { items: StatusItem[] }) {
  const ref = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLDivElement>(null);
  const host = useFeatureHost();
  const limit = useFit(ref, measureRef, items.map((i) => `${i.tone}:${i.text}`).join("\n"));
  const shown = items.slice(0, Math.max(0, limit));
  const rest = items.slice(Math.max(0, limit));
  const open = (item: StatusItem) => item.surface && host.openPanel(surfaceKind(item.surface));
  // 同一个外层元素一直挂着（`useFit` 从它找工具栏那一行），没有状态项时藏起来。
  return (
    <div
      ref={ref}
      className={
        items.length === 0
          ? "hidden"
          : `enter-fade relative flex shrink-0 items-center gap-1.5 border-r border-line-strong pr-1.5 ${limit === HIDDEN ? "invisible absolute" : ""}`
      }
    >
      {/* 测量副本：全部胶囊 + 一个「+N」，看不见、不占位，只用来量宽度。 */}
      <div aria-hidden className="pointer-events-none invisible absolute size-0 overflow-hidden">
      <div ref={measureRef} className="flex w-max">
        {items.map((item) => (
          <span key={item.id} className={pillClass(item.tone)}>
            <PillFace item={item} />
          </span>
        ))}
        <span className="flex h-6 items-center px-2 text-[11px] font-medium">+{items.length}</span>
      </div>
      </div>
      {shown.map((item) => (
        <StatusPill key={item.id} item={item} onOpen={() => open(item)} />
      ))}
      {rest.length > 0 && (
        <Popover
          title={`其它状态 · ${rest.length}`}
          className="flex h-6 items-center rounded-full bg-[#e8e8ec] px-2 text-[11px] font-medium text-ink hover:bg-[#d1d1d6] aria-expanded:bg-[#d1d1d6]"
          panelClassName="w-[300px] max-h-80"
          trigger={<>+{rest.length}</>}
        >
          {(close) => (
            <>
              <div className="px-2.5 pb-1 pt-2 text-[11px] font-semibold tracking-[0.275px] text-ink-4">其它状态 · {rest.length}</div>
              {rest.map((item) => {
                const Icon = TONE_ICON[item.tone];
                return (
                  <button
                    key={item.id}
                    type="button"
                    disabled={!item.surface}
                    onClick={() => {
                      close();
                      open(item);
                    }}
                    className="motion-colors flex items-start gap-2.5 px-2.5 py-2 text-left hover:bg-[#0a84ff14] disabled:hover:bg-transparent"
                  >
                    <span className={`flex size-6 shrink-0 items-center justify-center rounded-full ${Icon ? TONE_PILL[item.tone] : ""}`}>
                      {Icon && <Icon size={14} className={item.tone === "accent" ? "animate-spin" : ""} />}
                    </span>
                    <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                      <span className="truncate text-[13px] font-medium leading-[1.38] text-ink">{item.text}</span>
                      {item.tooltip && <span className="truncate text-xs leading-[1.33] text-ink-4">{item.tooltip}</span>}
                    </span>
                  </button>
                );
              })}
            </>
          )}
        </Popover>
      )}
    </div>
  );
}

const pillClass = (tone: Tone) =>
  `motion-colors flex h-6 max-w-[120px] shrink-0 items-center gap-1 rounded-full px-2.5 text-[11px] font-medium leading-none ${TONE_PILL[tone]}`;

/** 胶囊里面：色调图标 + 截断的原文。测量副本和真按钮共用。 */
function PillFace({ item, textRef }: { item: StatusItem; textRef?: React.Ref<HTMLSpanElement> }) {
  const Icon = TONE_ICON[item.tone];
  return (
    <>
      {Icon && <Icon size={12} className={`shrink-0 ${item.tone === "accent" ? "animate-spin" : ""}`} />}
      <span ref={textRef} className="min-w-0 truncate">
        {item.text}
      </span>
    </>
  );
}

function StatusPill({ item, onOpen }: { item: StatusItem; onOpen: () => void }) {
  const textRef = useRef<HTMLSpanElement>(null);
  const [cut, setCut] = useState(false);
  return (
    <span className="group relative flex">
      <button
        type="button"
        disabled={!item.surface}
        onClick={onOpen}
        onMouseEnter={() => {
          const el = textRef.current;
          setCut(!!el && el.scrollWidth > el.clientWidth);
        }}
        aria-label={item.text}
        className={`${pillClass(item.tone)} disabled:cursor-default enabled:hover:shadow-[0_0_0_1px_currentColor]`}
      >
        <PillFace item={item} textRef={textRef} />
      </button>
      {(cut || item.tooltip) && (
        <span className="pointer-events-none absolute bottom-[calc(100%+8px)] right-0 z-30 flex w-max max-w-[296px] flex-col gap-1 rounded-lg bg-ink px-2.5 py-1.5 opacity-0 shadow-lg transition-opacity delay-300 duration-[var(--dur-fast)] group-hover:opacity-100">
          {cut && <span className="text-[11px] font-medium leading-none text-white">{item.text}</span>}
          {item.tooltip && (
            <span className={`text-[11px] leading-none ${cut ? "text-[#aeaeb2]" : "font-medium text-white"}`}>{item.tooltip}</span>
          )}
        </span>
      )}
    </span>
  );
}
