// 浏览器面板的「更多」菜单（地址栏右边的 ⋮）：强制重新加载、缩放、窗口尺寸。
//
// 参考用户给的截图（Chrome 风格的浏览器菜单）：强制重新加载 / 缩放 − 100% + / 设备预览（自适应、
// 更多尺寸）。Dock 这一版只留能真做的那几项：
// - 强制重新加载：绕过缓存重新加载当前页（`browser/view/navigate` 的 `hard_reload`）。
// - 缩放：画面在面板里放大 / 缩小（100% = 铺满面板），每个标签页各记各的。
// - 窗口尺寸：自适应（页面视口跟着面板走）或固定成某个尺寸（每个标签页各记各的）。
//
// 菜单用 Popover 弹（点外面 / Esc 关），面板关掉就跟着关。

import { useState } from "react";
import { Check, Minus, MonitorSmartphone, Plus, RefreshCcw, RotateCcw, Smartphone, Tablet } from "lucide-react";

/** 缩放档位（相对「铺满面板」）：100% = 正好铺满。 */
export const ZOOM_STEPS = [0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4];

/** 固定尺寸的预设：给「更多尺寸」用（页面视口就是这么大，和面板大小无关）。 */
export const SIZE_PRESETS = [
  { id: "phone", label: "iPhone 15", width: 393, height: 852, icon: Smartphone },
  { id: "pad", label: "iPad", width: 1024, height: 768, icon: Tablet },
  { id: "laptop", label: "笔记本 1440×900", width: 1440, height: 900, icon: MonitorSmartphone },
  { id: "small", label: "小窗 1024×640", width: 1024, height: 640, icon: MonitorSmartphone },
] as const;

const Divider = () => <div className="mx-3 my-1 h-px bg-line-soft" />;

/** 缩放档位：往后 / 往前找一档（找不到就取边界）。 */
export function nextZoom(zoom: number, dir: 1 | -1): number {
  const eps = 1e-6;
  if (dir > 0) return ZOOM_STEPS.find((z) => z > zoom + eps) ?? ZOOM_STEPS[ZOOM_STEPS.length - 1];
  return [...ZOOM_STEPS].reverse().find((z) => z < zoom - eps) ?? ZOOM_STEPS[0];
}

export function BrowserMenu({
  zoom,
  fit,
  canHardReload,
  onZoom,
  onFit,
  onPreset,
  onHardReload,
}: {
  /** 当前缩放（1 = 铺满面板）。 */
  zoom: number;
  /** 自适应：页面视口跟着面板大小走。 */
  fit: boolean;
  /** 网关认得 `hard_reload`（老网关没有这个动作）。 */
  canHardReload: boolean;
  onZoom: (zoom: number) => void;
  onFit: (fit: boolean) => void;
  /** 选了固定尺寸（宽高）。 */
  onPreset: (size: { width: number; height: number }) => void;
  onHardReload: () => void;
}) {
  const [more, setMore] = useState(false);
  return (
    <div role="menu" className="flex flex-col py-1">
      <button
        type="button"
        role="menuitem"
        disabled={!canHardReload}
        onClick={onHardReload}
        title={canHardReload ? "绕过缓存重新加载这一页" : "当前 Dock 不支持强制重新加载"}
        className="motion-colors mx-1 flex h-8 items-center gap-2.5 rounded-md px-2.5 text-left text-[13px] text-ink hover:bg-hover disabled:text-ink-5 disabled:hover:bg-transparent"
      >
        <RefreshCcw size={14} className="shrink-0 text-ink-2" />
        <span className="min-w-0 flex-1 truncate">强制重新加载</span>
      </button>

      <Divider />

      {/* 缩放：− 百分比 + ，点百分比回到 100%。 */}
      <div className="flex h-9 items-center gap-2 px-3">
        <span className="shrink-0 text-[13px] text-ink">缩放</span>
        <div className="ml-auto flex items-center gap-1">
          <button
            type="button"
            aria-label="缩小"
            onClick={() => onZoom(nextZoom(zoom, -1))}
            className="motion-colors flex size-6 items-center justify-center rounded-md text-ink-2 hover:bg-hover hover:text-ink"
          >
            <Minus size={14} />
          </button>
          <button
            type="button"
            title="回到 100%（铺满面板）"
            onClick={() => onZoom(1)}
            className="motion-colors min-w-[52px] rounded-md px-1.5 py-1 text-center font-mono text-[12px] text-ink hover:bg-hover"
          >
            {Math.round(zoom * 100)}%
          </button>
          <button
            type="button"
            aria-label="放大"
            onClick={() => onZoom(nextZoom(zoom, 1))}
            className="motion-colors flex size-6 items-center justify-center rounded-md text-ink-2 hover:bg-hover hover:text-ink"
          >
            <Plus size={14} />
          </button>
          <button
            type="button"
            aria-label="缩放回到 100%"
            title="回到 100%（铺满面板）"
            onClick={() => onZoom(1)}
            className="motion-colors ml-0.5 flex size-6 items-center justify-center rounded-md text-ink-3 hover:bg-hover hover:text-ink"
          >
            <RotateCcw size={13} />
          </button>
        </div>
      </div>

      <Divider />

      <div className="px-3 pb-1 pt-0.5 text-[11px] font-medium text-ink-4">窗口尺寸</div>
      <button
        type="button"
        role="menuitemradio"
        aria-checked={fit}
        onClick={() => onFit(true)}
        className="motion-colors mx-1 flex h-8 items-center gap-2.5 rounded-md px-2.5 text-left text-[13px] text-ink hover:bg-hover"
      >
        <span className="min-w-0 flex-1 truncate">自适应</span>
        {fit && <Check size={14} className="shrink-0 text-accent" />}
      </button>
      <button
        type="button"
        role="menuitem"
        onClick={() => setMore((v) => !v)}
        className="motion-colors mx-1 flex h-8 items-center gap-2.5 rounded-md px-2.5 text-left text-[13px] text-ink hover:bg-hover"
      >
        <span className="min-w-0 flex-1 truncate">更多尺寸</span>
        <span className="shrink-0 text-[11px] text-ink-4">{more ? "收起" : ""}</span>
      </button>
      {more && (
        <div className="flex flex-col">
          {SIZE_PRESETS.map((p) => (
            <button
              key={p.id}
              type="button"
              role="menuitem"
              onClick={() => onPreset({ width: p.width, height: p.height })}
              className="motion-colors mx-1 flex h-8 items-center gap-2.5 rounded-md px-2.5 text-left text-[13px] text-ink-2 hover:bg-hover"
            >
              <p.icon size={14} className="shrink-0 text-ink-3" />
              <span className="min-w-0 flex-1 truncate">{p.label}</span>
              <span className="shrink-0 font-mono text-[10.5px] text-ink-4">
                {p.width}×{p.height}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
