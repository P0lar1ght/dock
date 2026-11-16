// 图谱设置面板（画布右上角的齿轮）：着色、显示、力导向三节，改了立即生效、记在本机。
// 开着也能照常在图上点、拖：边调边看效果。

import { useState, type CSSProperties, type ReactNode } from "react";
import { ChevronRight, Plus, RotateCcw, X } from "lucide-react";
import { Range, Toggle } from "../../components/settings/ui";
import { freeSlot, GROUP_SLOTS, MAX_GROUPS, RANGES, resetGraphSettings, setGraphSettings, type ColorGroup, type GraphSettings } from "./settings";

export function GraphSettingsPanel({
  settings,
  hits,
  style,
  onClose,
}: {
  settings: GraphSettings;
  /** 每组命中几篇（和 `settings.groups` 一一对应）；还在找是 null。 */
  hits: (number | null)[];
  style: CSSProperties;
  onClose: () => void;
}) {
  const [open, setOpen] = useState({ groups: true, display: true, forces: false });
  const toggle = (key: keyof typeof open) => setOpen((o) => ({ ...o, [key]: !o[key] }));
  const groups = settings.groups;
  const setGroups = (next: ColorGroup[]) => setGraphSettings({ groups: next });
  const slot = freeSlot(groups);

  const slider = (key: keyof typeof RANGES, label: string, format: (v: number) => string = (v) => `${v}`) => (
    <Row label={label}>
      <Range value={settings[key]} min={RANGES[key][0]} max={RANGES[key][1]} step={key === "linkDistance" ? 5 : key === "nodeSize" || key === "lineWidth" ? 10 : 1} format={format} label={label} width={118} onChange={(v) => setGraphSettings({ [key]: v })} />
    </Row>
  );

  return (
    <div
      role="dialog"
      aria-label="图谱设置"
      className="glass-popover pop-in absolute z-20 flex max-h-[calc(100%-136px)] w-[284px] flex-col overflow-hidden rounded-xl border border-line-strong bg-white/95 shadow-[0px_8px_28px_rgba(0,0,0,0.12)] backdrop-blur-xl"
      style={{ ...style, transformOrigin: "top right" }}
      onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), onClose())}
    >
      <div className="flex h-10 shrink-0 items-center gap-1 border-b border-line-soft pl-3.5 pr-1.5">
        <span className="flex-1 text-[13px] font-semibold text-ink">图谱设置</span>
        <IconButton label="恢复默认（着色分组不动）" onClick={resetGraphSettings}>
          <RotateCcw size={13} />
        </IconButton>
        <IconButton label="关闭" onClick={onClose}>
          <X size={14} />
        </IconButton>
      </div>
      <div className="thin-scroll flex min-h-0 flex-col overflow-y-auto pb-2">
        <Section title="着色" open={open.groups} onToggle={() => toggle("groups")}>
          <p className="px-3.5 pb-1 text-[11.5px] leading-snug text-ink-4">按搜索条件给笔记上色，没命中的变灰；和搜索框一样查标题和正文。</p>
          {groups.map((g, i) => (
            <div key={g.slot} className="flex h-8 items-center gap-2 px-3.5">
              <span className="size-2.5 shrink-0 rounded-full" style={{ background: GROUP_SLOTS[g.slot] }} />
              <input
                value={g.query}
                autoFocus={g.query === ""}
                onChange={(e) => setGroups(groups.map((x) => (x.slot === g.slot ? { ...x, query: e.target.value } : x)))}
                placeholder="比如：支付"
                aria-label={`第 ${i + 1} 组的搜索条件`}
                className="h-7 min-w-0 flex-1 select-text rounded-md border border-line-strong bg-white px-2 text-[12.5px] text-ink outline-none placeholder:text-ink-4 focus:border-accent"
              />
              <span className="w-7 shrink-0 text-right text-[11px] tabular-nums text-ink-4">{g.query.trim() ? (hits[i] ?? "…") : ""}</span>
              <IconButton label="删掉这一组" onClick={() => setGroups(groups.filter((x) => x.slot !== g.slot))}>
                <X size={12} />
              </IconButton>
            </div>
          ))}
          <div className="px-2.5 pt-0.5">
            <button
              type="button"
              disabled={slot === null}
              title={slot === null ? `最多 ${MAX_GROUPS} 组：颜色再多，挨着的点就分不清了` : undefined}
              onClick={() => slot !== null && setGroups([...groups, { query: "", slot }])}
              className="motion-colors flex h-7 items-center gap-1.5 rounded-md px-1.5 text-[12.5px] font-medium text-accent hover:bg-[#0a84ff14] disabled:cursor-not-allowed disabled:text-ink-4 disabled:hover:bg-transparent"
            >
              <Plus size={13} />
              新的分组
            </button>
          </div>
        </Section>

        <Section title="显示" open={open.display} onToggle={() => toggle("display")}>
          <Row label="箭头" hint="关着时只有悬停 / 选中那篇的链接画方向">
            <Toggle on={settings.arrows} label="箭头" onChange={(on) => setGraphSettings({ arrows: on })} />
          </Row>
          <Row label="全图显示相似建议" hint="关着时只画悬停 / 选中那篇的">
            <Toggle on={settings.allSuggestions} label="全图显示相似建议" onChange={(on) => setGraphSettings({ allSuggestions: on })} />
          </Row>
          <Row label="还没写的笔记" hint="链接指向、但还不存在的那几篇">
            <Toggle on={settings.ghosts} label="还没写的笔记" onChange={(on) => setGraphSettings({ ghosts: on })} />
          </Row>
          {slider("labels", "标签", (v) => (v === 0 ? "关" : `${v}`))}
          {slider("nodeSize", "点大小", (v) => `${v}%`)}
          {slider("lineWidth", "线粗", (v) => `${v}%`)}
        </Section>

        <Section title="力导向" open={open.forces} onToggle={() => toggle("forces")}>
          {slider("center", "向心力")}
          {slider("repel", "斥力")}
          {slider("linkStrength", "连线拉力")}
          {slider("linkDistance", "连线长度")}
        </Section>
      </div>
    </div>
  );
}

function Section({ title, open, onToggle, children }: { title: string; open: boolean; onToggle: () => void; children: ReactNode }) {
  return (
    <section className="flex flex-col border-b border-line-soft py-1 last:border-b-0">
      <button type="button" aria-expanded={open} onClick={onToggle} className="flex h-8 items-center gap-1 px-2.5 text-left text-xs font-semibold text-ink-3 hover:text-ink">
        <ChevronRight size={13} className={`shrink-0 transition-transform duration-150 ${open ? "rotate-90" : ""}`} />
        {title}
      </button>
      {open && <div className="flex flex-col pb-1">{children}</div>}
    </section>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="flex min-h-8 items-center gap-2 px-3.5" title={hint}>
      <span className="min-w-0 flex-1 truncate text-[12.5px] text-ink-2">{label}</span>
      {children}
    </div>
  );
}

function IconButton({ label, onClick, children }: { label: string; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className="motion-colors flex size-7 shrink-0 items-center justify-center rounded-md text-ink-3 hover:bg-hover hover:text-ink"
    >
      {children}
    </button>
  );
}
