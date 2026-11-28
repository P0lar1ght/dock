// 右侧面板的头栏：任何宽度都摆得下，放不下的按顺序收进关闭按钮左边的「⋯」。
// 各面板的头栏都用它（不管是在右侧那本书里、工作台里还是独立窗口里），子元素用 data-fold 标角色（chrome.tsx 的 FoldRole）。
// 不依赖功能登记（features/registry）：功能面板也引它，免得循环引用。

import { useLayoutEffect, useRef, useState, type ElementType, type ReactNode } from "react";
import { Check, MoreHorizontal } from "lucide-react";
import { Popover } from "./Popover";

/** 「⋯」按钮的宽（size-7）。 */
const MORE_WIDTH = 28;
/** 收的先后：数小的先收；同一档从右往左。 */
const FOLD_RANK: Record<string, number> = { title: -100, early: -10, late: 10 };

/** 一个子元素现在的布局宽（不受 3D 书页的 transform 影响）。 */
function widthOf(el: Element): number {
  if (el instanceof HTMLElement) return el.offsetWidth;
  // lucide 图标是 svg：没有 offsetWidth，取它的 width 属性（= size）。
  return Number(el.getAttribute("width")) || 16;
}

/**
 * 量一遍头栏，决定收哪些：先让上下文（`shrink`）压到 data-min，还不够就按 FOLD_RANK 一件件收。
 * 收起来的打上 data-folded（CSS 挪到行尾叠着、不可见），回菜单里要列的那些（不含 `title`）。
 */
function foldHeader(header: HTMLElement): Element[] {
  const cs = getComputedStyle(header);
  const gap = parseFloat(cs.columnGap) || 0;
  const avail = header.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
  type Item = { el: Element; w: number; rank: number | null; index: number };
  const items: Item[] = [];
  [...header.children].forEach((el, index) => {
    if (el instanceof HTMLElement && el.dataset.foldMore !== undefined) return;
    if (getComputedStyle(el).display === "none") return;
    const role = el.getAttribute("data-fold");
    // 弹簧（flex-1 的空 div）：只算它的最小宽，不收。
    const spring = !role && el instanceof HTMLElement && parseFloat(getComputedStyle(el).flexGrow) > 0;
    const w = spring
      ? parseFloat(getComputedStyle(el).minWidth) || 0
      : role === "shrink"
        ? Number(el.getAttribute("data-min")) || 48
        : widthOf(el);
    const foldable = !spring && w > 0 && role !== "keep" && role !== "end" && role !== "shrink";
    items.push({ el, w, rank: foldable ? (FOLD_RANK[role ?? ""] ?? 0) : null, index });
  });
  let total = items.reduce((sum, it) => sum + it.w, 0) + gap * Math.max(0, items.length - 1);
  const order = items
    .filter((it) => it.rank !== null)
    .sort((a, b) => a.rank! - b.rank! || b.index - a.index);
  const folded = new Set<Element>();
  let menu = false;
  for (const it of order) {
    if (total <= avail) break;
    folded.add(it.el);
    total -= it.w + gap;
    if (!menu && it.rank !== FOLD_RANK.title) {
      menu = true;
      total += MORE_WIDTH + gap;
    }
  }
  for (const it of items) {
    if (folded.has(it.el)) it.el.setAttribute("data-folded", "");
    else it.el.removeAttribute("data-folded");
  }
  return items.filter((it) => folded.has(it.el) && it.rank !== FOLD_RANK.title).map((it) => it.el);
}

/**
 * 右侧面板的头栏：任何宽度都摆得下。放不下时按顺序收：面板名（标签上有）→ 窗口动作（分屏、新窗口）→
 * 其余动作从右往左（`late` 的最后）；收起来的统一进关闭按钮左边的「⋯」。子元素用 data-fold 标角色（见 FoldRole）。
 *
 * 收起来的元素不卸载，挪到行尾叠着、不可见：菜单里点一项就是点它本身（弹层也按它的位置——行尾——摆）。
 */
export function PanelHeader({
  as: Tag = "header",
  className = "",
  children,
}: {
  as?: ElementType;
  className?: string;
  children: ReactNode;
}) {
  const ref = useRef<HTMLElement>(null);
  const [folded, setFolded] = useState<Element[]>([]);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    let frame = 0;
    const run = () => {
      const next = foldHeader(el);
      setFolded((prev) => (prev.length === next.length && prev.every((x, i) => x === next[i]) ? prev : next));
    };
    const later = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(run);
    };
    run();
    // 栏变宽窄、头栏里的东西出现 / 消失 / 换字：重量。
    const ro = new ResizeObserver(later);
    ro.observe(el);
    const mo = new MutationObserver(later);
    mo.observe(el, { childList: true, subtree: true, characterData: true });
    return () => {
      cancelAnimationFrame(frame);
      ro.disconnect();
      mo.disconnect();
    };
  }, []);
  return (
    <Tag ref={ref} className={`panel-header relative ${className}`} data-tauri-drag-region>
      {children}
      <span data-fold-more="" className={folded.length ? "flex" : "hidden"}>
        <MoreMenu folded={folded} />
      </span>
    </Tag>
  );
}

/** 菜单里的一项：收起来的那个按钮。 */
interface FoldEntry {
  button: HTMLButtonElement;
  label: string;
  icon: SVGElement | null;
  on: boolean;
}

/** 一个收起来的元素里能点的：它自己是按钮就是它，是一组（分段切换、窗口动作）就是里面的按钮。 */
function entriesOf(folded: Element[]): FoldEntry[] {
  return folded.flatMap((el) => {
    const buttons = el instanceof HTMLButtonElement ? [el] : [...el.querySelectorAll("button")];
    return buttons.map((button) => {
      const text = button.textContent?.trim() ?? "";
      const name = button.getAttribute("aria-label") || button.title || "";
      // 弹出菜单的触发（下拉）：名字 + 现在选的。
      const label = button.hasAttribute("aria-expanded") && text && name ? `${name} · ${text}` : text || name;
      const on = ["aria-pressed", "aria-checked", "aria-selected"].some((a) => button.getAttribute(a) === "true");
      return { button, label, icon: button.querySelector("svg"), on };
    });
  });
}

function MoreMenu({ folded }: { folded: Element[] }) {
  return (
    <Popover
      title="更多"
      side="bottom"
      trigger={<MoreHorizontal size={15} strokeWidth={2} />}
      className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-hover hover:text-ink aria-expanded:bg-selected"
      panelClassName="min-w-[200px] max-w-[300px] p-1.5"
    >
      {(close) =>
        // 打开时才读：按钮的字、图标、开关状态都是现在的。
        entriesOf(folded).map((entry, i) => (
          <button
            key={i}
            type="button"
            disabled={entry.button.disabled}
            onClick={() => {
              close();
              // 等菜单收了再点：点的是弹出菜单的触发时，新弹层不会被这次点击当成「点外面」关掉。
              requestAnimationFrame(() => entry.button.click());
            }}
            className="motion-colors flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-left text-ink hover:bg-hover disabled:text-ink-5 disabled:hover:bg-transparent"
          >
            <FoldIcon svg={entry.icon} />
            <span className="min-w-0 flex-1 truncate text-[13px]">{entry.label}</span>
            {entry.on && <Check size={13} className="shrink-0 text-accent" />}
          </button>
        ))
      }
    </Popover>
  );
}

/** 菜单项前面的图标：拷一份那个按钮自己的。 */
function FoldIcon({ svg }: { svg: SVGElement | null }) {
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    ref.current?.replaceChildren(...(svg ? [svg.cloneNode(true)] : []));
  }, [svg]);
  return <span ref={ref} className="flex size-[14px] shrink-0 items-center justify-center text-ink-3 [&>svg]:size-[14px]" />;
}
