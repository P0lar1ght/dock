// 弹出层：从触发按钮弹出（scale 0.96→1 + 淡入），点外面或 Esc 关闭。
// `Popover` 是通用外壳；`MenuButton` 是其上的单选下拉菜单。
//
// 面板挂到 body 上、按触发按钮的位置 fixed 定位：放在 `overflow-hidden` 的卡片里不会被裁掉，
// 也不会被后面那张卡片（玻璃材质下各自是层叠上下文）盖住。滚动、窗口变大小时跟着挪。

import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, type LucideIcon } from "lucide-react";

export function Popover({
  trigger,
  title,
  align = "right",
  side = "top",
  className = "",
  wrapperClassName = "shrink-0",
  panelClassName = "min-w-[200px] max-h-72",
  children,
}: {
  trigger: ReactNode;
  title: string;
  /** 触发按钮外面那层（默认不伸缩；要占满一行时传 `min-w-0 flex-1`）。 */
  wrapperClassName?: string;
  align?: "left" | "right";
  /** 面板在触发按钮的哪一侧；`right` 时顶边对齐（触发点在右边时用），`align` 不起作用。 */
  side?: "top" | "bottom" | "right";
  className?: string;
  panelClassName?: string;
  /** 面板内容；拿到 `close` 以便点了某项后收起。 */
  children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<CSSProperties | null>(null);

  // 按触发按钮现在的位置摆面板；滚动（任一层）、窗口变大小时重摆。
  useLayoutEffect(() => {
    if (!open) {
      setPos(null);
      return;
    }
    let frame = 0;
    const place = () => {
      const r = ref.current?.getBoundingClientRect();
      if (!r) return;
      const { innerWidth: w, innerHeight: h } = window;
      // 靠右弹：触发按钮在窗口下半截（窄栏底部的头像）就底边对齐，不然面板掉出窗口。
      if (side === "right") setPos(r.top > h / 2 ? { left: r.right + 8, bottom: h - r.bottom } : { left: r.right + 8, top: r.top });
      else
        setPos({
          ...(side === "top" ? { bottom: h - r.top + 6 } : { top: r.bottom + 6 }),
          ...(align === "right" ? { right: w - r.right } : { left: r.left }),
        });
    };
    const later = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(place);
    };
    place();
    window.addEventListener("resize", later);
    window.addEventListener("scroll", later, true);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", later);
      window.removeEventListener("scroll", later, true);
    };
  }, [open, side, align]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!ref.current?.contains(t) && !panel.current?.contains(t)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const origin =
    side === "right" ? (pos && "bottom" in pos ? "left bottom" : "left top") : `${side === "top" ? "bottom" : "top"} ${align}`;
  return (
    <div ref={ref} className={`relative ${wrapperClassName}`}>
      <button
        type="button"
        title={title}
        aria-label={title}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className={`motion-colors ${className}`}
      >
        {trigger}
      </button>
      {open &&
        pos &&
        createPortal(
          <div
            ref={panel}
            style={{ ...pos, transformOrigin: origin }}
            className={`glass-popover pop-in fixed z-50 flex flex-col overflow-y-auto rounded-[10px] border border-line-strong bg-white/95 py-1 shadow-[0px_8px_28px_rgba(0,0,0,0.12)] backdrop-blur-xl thin-scroll ${panelClassName}`}
          >
            {children(() => setOpen(false))}
          </div>,
          document.body,
        )}
    </div>
  );
}

export interface MenuOption {
  id: string;
  label: string;
  hint?: string;
  icon?: LucideIcon;
}

export function MenuButton({
  trigger,
  title,
  options,
  value,
  onPick,
  header,
  footer,
  align = "right",
  side = "top",
  className = "",
}: {
  trigger: ReactNode;
  title: string;
  options: MenuOption[];
  value: string;
  onPick: (id: string) => void;
  header?: string;
  footer?: ReactNode;
  align?: "left" | "right";
  /** 面板在触发按钮的哪一侧；`right` 时顶边对齐（触发点在右边时用），`align` 不起作用。 */
  side?: "top" | "bottom" | "right";
  className?: string;
}) {
  return (
    <Popover trigger={trigger} title={title} align={align} side={side} className={className}>
      {(close) => (
        <>
          {header && (
            <div className="px-2.5 pb-1 pt-1.5 text-[11px] font-semibold tracking-[0.275px] text-ink-4">{header}</div>
          )}
          {options.map((o) => (
            <button
              key={o.id}
              type="button"
              onClick={() => {
                close();
                if (o.id !== value) onPick(o.id);
              }}
              className={`motion-colors flex h-8 shrink-0 items-center gap-2.5 px-2.5 text-left text-[13px] ${
                o.id === value ? "bg-[#0a84ff14] text-ink" : "text-ink-2 hover:bg-hover"
              }`}
            >
              {o.icon && <o.icon size={14} className={o.id === value ? "text-accent" : "text-ink-3"} />}
              <span className="min-w-0 flex-1 truncate">{o.label}</span>
              {o.hint && <span className="shrink-0 font-mono text-[10px] text-ink-4">{o.hint}</span>}
              {o.id === value && <Check size={13} className="shrink-0 text-accent" />}
            </button>
          ))}
          {footer && <div className="mt-1 border-t border-line-soft px-2.5 pb-1 pt-1.5">{footer}</div>}
        </>
      )}
    </Popover>
  );
}
