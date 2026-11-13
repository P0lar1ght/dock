// 上下文圆环 + 点开的占用详情。设计：wonder-ui/chat/ComposerGoalContext（had ⑦⑧）。
// 圆环弧长 = 已用 / 窗口；颜色只说离自动压缩阈值多远（灰 → 阈值前 10% 橙 → 过阈值红）。
// 详情（`thread/context/get`）：按类别着色的甜甜圈 + 横条 + 阈值线；分「固定前缀」「会话」两组，
// 行点开看明细。数和 TUI `/context` 同一份。

import { useEffect, useRef, useState, type ReactNode } from "react";
import { ChevronRight, Shrink } from "lucide-react";
import type { ContextBreakdown, ContextDetailGroup, ContextSlice, ContextUsage } from "../model";

const SLICE_COLOR: Record<ContextSlice["id"], string> = {
  system: "#5e5ce6",
  tools: "#0a84ff",
  messages: "#ff375f",
  overhead: "#ff9f0a",
  free: "#f2f2f7",
};

const GROUPS: { id: ContextSlice["group"]; title: string }[] = [
  { id: "prefix", title: "固定前缀 · 每轮原样重发、可命中缓存" },
  { id: "session", title: "会话 · 随对话增长，压缩只压这部分" },
];

/** 离阈值多近算「快满了」（百分点）。 */
const NEAR_POINTS = 10;

function ringTone(usage: ContextUsage): { arc: string; track: string } {
  if (usage.percent >= usage.triggerPercent) return { arc: "#ff3b30", track: "#ffd6d3" };
  if (usage.percent >= usage.triggerPercent - NEAR_POINTS) return { arc: "#ff9f0a", track: "#e5e5ea" };
  return { arc: "#636366", track: "#e5e5ea" };
}

/** `812` / `18.4K` / `128K`。 */
export function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n >= 100_000) return `${Math.round(n / 1000)}K`;
  return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}K`;
}

function fmtShare(tokens: number, total: number): string {
  if (total <= 0) return "";
  const p = (tokens / total) * 100;
  if (p === 0) return "0%";
  return p < 1 ? `${p.toFixed(1)}%` : `${Math.round(p)}%`;
}

/** 只画环：弧长 = 已用 / 窗口。 */
export function RingSvg({
  percent,
  tone,
  size = 18,
}: {
  percent: number;
  tone: { arc: string; track: string };
  size?: number;
}) {
  const C = 2 * Math.PI * 7;
  const arc = Math.min(1, percent / 100) * C;
  return (
    <svg width={size} height={size} viewBox="0 0 18 18" aria-hidden>
      <circle cx="9" cy="9" r="7" fill="none" stroke={tone.track} strokeWidth="2.25" className="transition-[stroke] duration-300" />
      <circle
        cx="9"
        cy="9"
        r="7"
        fill="none"
        stroke={tone.arc}
        strokeWidth="2.25"
        strokeLinecap="round"
        strokeDasharray={`${arc} ${C}`}
        transform="rotate(-90 9 9)"
        className="transition-[stroke-dasharray,stroke] duration-300 ease-[var(--ease-out)]"
      />
    </svg>
  );
}

/** 子代理那套颜色：过了自动压缩门槛是橙的（主会话那边是红）。 */
function arcTone(usage: ContextUsage): { arc: string; track: string } {
  if (usage.percent >= usage.triggerPercent) return { arc: "#ff9f0a", track: "#e5e5ea" };
  return { arc: "#636366", track: "#e5e5ea" };
}

/**
 * 只读的一个小环 + 悬停提示：子代理面板用（点不开明细，也不能替它压缩）。
 */
export function ContextArc({ usage, className }: { usage: ContextUsage; className?: string }) {
  const tone = arcTone(usage);
  const left = Math.max(0, Math.round((usage.total * usage.triggerPercent) / 100) - usage.used);
  const over = usage.percent >= usage.triggerPercent;
  return (
    <div className={`group/arc relative shrink-0 ${className ?? ""}`}>
      <div
        className="flex size-6 items-center justify-center"
        aria-label={`上下文 ${usage.percent}%`}
        title={`已用 ${fmtTokens(usage.used)} / ${fmtTokens(usage.total)} · 距压缩还剩 ${fmtTokens(left)}`}
      >
        <RingSvg percent={usage.percent} tone={tone} size={15} />
      </div>
      <div
        role="tooltip"
        className="pointer-events-none absolute bottom-[calc(100%+6px)] left-1/2 z-30 -translate-x-1/2 whitespace-nowrap rounded-md bg-ink px-2 py-1 text-[11px] text-white opacity-0 transition-opacity delay-0 duration-[var(--dur-fast)] group-hover/arc:opacity-100 group-hover/arc:delay-300"
      >
        已用 {fmtTokens(usage.used)} / {fmtTokens(usage.total)} ·{" "}
        {over ? "已过自动压缩门槛，下一轮会先压缩" : `距压缩还剩 ${fmtTokens(left)}`}
      </div>
    </div>
  );
}

export function ContextRing({
  usage,
  open,
  onOpenChange,
  load,
  onCompact,
}: {
  usage: ContextUsage | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 拉一次明细。 */
  load: () => Promise<ContextBreakdown>;
  onCompact: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [data, setData] = useState<ContextBreakdown | null>(null);
  const [error, setError] = useState<string | null>(null);

  // 打开时拉；开着的时候占用变了（一轮结束、压缩完）跟着重拉。
  useEffect(() => {
    if (!open) return;
    let live = true;
    load().then(
      (d) => live && (setData(d), setError(null)),
      (e) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, usage?.used]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && onOpenChange(false);
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onOpenChange(false);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open, onOpenChange]);

  if (!usage) return null;
  const tone = ringTone(usage);
  const left = Math.max(0, Math.round((usage.total * usage.triggerPercent) / 100) - usage.used);
  const hint =
    usage.percent >= usage.triggerPercent
      ? "已过自动压缩阈值，下一轮会先压缩"
      : `距自动压缩 ${fmtTokens(left)}`;
  return (
    <div ref={ref} className="group/ring relative shrink-0">
      <button
        type="button"
        aria-label={`上下文 ${usage.percent}%`}
        aria-expanded={open}
        onClick={() => onOpenChange(!open)}
        className={`motion-colors flex size-7 items-center justify-center rounded-full hover:bg-[#f5f5f7] ${open ? "bg-[#ececf0]" : ""}`}
      >
        <RingSvg percent={usage.percent} tone={tone} />
      </button>
      {!open && (
        <div
          role="tooltip"
          className="pointer-events-none absolute bottom-[calc(100%+6px)] right-0 z-30 whitespace-nowrap rounded-md bg-ink px-2 py-1 text-[11px] text-white opacity-0 transition-opacity delay-0 duration-[var(--dur-fast)] group-hover/ring:opacity-100 group-hover/ring:delay-300"
        >
          上下文 {fmtTokens(usage.used)} / {fmtTokens(usage.total)}（{usage.percent}%）· {hint}
        </div>
      )}
      {open && (
        <div
          role="dialog"
          aria-label="上下文占用"
          style={{ transformOrigin: "bottom right" }}
          className="glass-popover pop-in thin-scroll absolute bottom-[calc(100%+10px)] right-0 z-30 flex max-h-[min(640px,70vh)] w-[400px] max-w-[calc(100vw-32px)] flex-col overflow-y-auto rounded-[16px] border border-line-strong bg-white shadow-[0px_12px_36px_rgba(0,0,0,0.14)]"
        >
          {data ? (
            <Breakdown
              data={data}
              onCompact={() => {
                onCompact();
                onOpenChange(false);
              }}
            />
          ) : error ? (
            <div className="px-4 py-5 text-[13px] text-[#c0392b]">没拿到上下文明细：{error}</div>
          ) : (
            <div className="flex flex-col gap-3 px-4 py-4">
              <div className="h-16 w-full animate-pulse rounded-xl bg-fill" />
              <div className="h-2 w-full animate-pulse rounded-full bg-fill" />
              <div className="h-28 w-full animate-pulse rounded-xl bg-fill" />
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function Breakdown({ data, onCompact }: { data: ContextBreakdown; onCompact: () => void }) {
  const [openRow, setOpenRow] = useState<string | null>(null);
  const span = Math.max(data.total, data.used, 1);
  const used = data.slices.filter((s) => s.group !== "free" && s.tokens > 0);
  const free = data.slices.find((s) => s.id === "free");
  const left = Math.max(0, data.triggerTokens - data.used);
  const toggle = (id: string) => setOpenRow((cur) => (cur === id ? null : id));

  return (
    <>
      <div className="flex items-center gap-4 px-4 pb-3 pt-4">
        <Donut slices={used} span={span} percent={data.percent} />
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className="text-[13px] font-semibold text-ink">上下文窗口</span>
          <span className="text-[15px] tabular-nums text-ink">
            {fmtTokens(data.used)}
            <span className="text-ink-4"> / {fmtTokens(data.total)} tokens</span>
          </span>
          <span className="text-xs text-ink-4">
            剩余 {fmtTokens(Math.max(0, data.total - data.used))} ·{" "}
            {data.percent >= data.triggerPercent ? "已过自动压缩阈值" : `距自动压缩还有 ${fmtTokens(left)}`}
          </span>
        </div>
      </div>

      <div className="flex flex-col gap-1 px-4 pb-3">
        <div className="relative flex h-2 w-full overflow-hidden rounded-full bg-[#f2f2f7]">
          {used.map((s) => (
            <div
              key={s.id}
              title={`${s.label} ${fmtTokens(s.tokens)}`}
              className="h-full shrink-0 transition-[width] duration-300 ease-[var(--ease-out)]"
              style={{ width: `${(s.tokens / span) * 100}%`, background: SLICE_COLOR[s.id] }}
            />
          ))}
        </div>
        <div className="relative h-4 w-full">
          <div
            className="absolute -top-3 h-3 w-px bg-ink-4"
            style={{ left: `${Math.min(100, data.triggerPercent)}%` }}
          />
          <span
            className="absolute top-0 -translate-x-full whitespace-nowrap pr-1 text-[10px] text-ink-4"
            style={{ left: `${Math.min(100, data.triggerPercent)}%` }}
          >
            自动压缩 {data.triggerPercent}%
          </span>
        </div>
      </div>

      {GROUPS.map((g) => {
        const rows = data.slices.filter((s) => s.group === g.id);
        if (rows.length === 0) return null;
        const sum = rows.reduce((n, s) => n + s.tokens, 0);
        return (
          <div key={g.id} className="flex flex-col border-t border-[#f2f2f7] px-2 py-2">
            <div className="flex items-center justify-between px-2 py-1">
              <span className="text-[11px] font-semibold text-ink-4">{g.title}</span>
              <span className="text-[11px] tabular-nums text-ink-4">{fmtTokens(sum)}</span>
            </div>
            {rows.map((s) => (
              <SliceRow
                key={s.id}
                slice={s}
                span={span}
                open={openRow === s.id}
                onToggle={() => toggle(s.id)}
              />
            ))}
            {g.id === "session" && free && <FreeRow slice={free} span={span} />}
          </div>
        );
      })}

      {data.onDemand.length > 0 && (
        <div className="flex flex-col border-t border-[#f2f2f7] px-2 py-2">
          <div className="px-2 py-1 text-[11px] font-semibold text-ink-4">按需加载 · 用到才发，不占窗口</div>
          {data.onDemand.map((o) => (
            <ExpandRow
              key={o.id}
              open={openRow === o.id}
              onToggle={() => toggle(o.id)}
              detail={o.detail}
              head={
                <>
                  <span className="size-2 shrink-0 rounded-full border border-[#d1d1d6]" />
                  <span className="text-[13px] text-ink-2">{o.label}</span>
                  {o.note && <span className="min-w-0 truncate text-[11px] text-ink-4">{o.note}</span>}
                </>
              }
            />
          ))}
        </div>
      )}

      <div className="sticky bottom-0 flex items-center justify-between gap-2 border-t border-[#f2f2f7] bg-white px-4 py-2.5">
        <span className="min-w-0 truncate text-[11px] text-ink-4">
          {[data.model, data.compactions > 0 ? `已压缩 ${data.compactions} 次` : "", "估算值"].filter(Boolean).join(" · ")}
        </span>
        <button
          type="button"
          title="现在就压缩上下文（/compact）"
          onClick={onCompact}
          className="motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-full bg-[#f5f5f7] px-3 text-xs font-medium text-ink-2 hover:bg-[#ececf0] active:scale-95"
        >
          <Shrink size={14} />
          压缩上下文
        </button>
      </div>
    </>
  );
}

/** 按类别着色的甜甜圈，中间写总占用百分比。 */
function Donut({ slices, span, percent }: { slices: ContextSlice[]; span: number; percent: number }) {
  const r = 26;
  const C = 2 * Math.PI * r;
  let offset = 0;
  return (
    <div className="relative flex size-16 shrink-0 items-center justify-center">
      <svg width="64" height="64" viewBox="0 0 64 64" aria-hidden>
        <circle cx="32" cy="32" r={r} fill="none" stroke="#f2f2f7" strokeWidth="6" />
        <g transform="rotate(-90 32 32)">
          {slices.map((s) => {
            const len = (s.tokens / span) * C;
            const el = (
              <circle
                key={s.id}
                cx="32"
                cy="32"
                r={r}
                fill="none"
                stroke={SLICE_COLOR[s.id]}
                strokeWidth="6"
                strokeDasharray={`${len} ${C}`}
                strokeDashoffset={-offset}
                className="transition-[stroke-dasharray,stroke-dashoffset] duration-300 ease-[var(--ease-out)]"
              />
            );
            offset += len;
            return el;
          })}
        </g>
      </svg>
      <span className="absolute inset-0 flex items-center justify-center text-[15px] font-semibold tabular-nums text-ink">
        {percent}%
      </span>
    </div>
  );
}

function SliceRow({
  slice,
  span,
  open,
  onToggle,
}: {
  slice: ContextSlice;
  span: number;
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <ExpandRow
      open={open}
      onToggle={onToggle}
      detail={slice.detail}
      includes={slice.includes}
      head={
        <>
          <span className="size-2 shrink-0 rounded-full" style={{ background: SLICE_COLOR[slice.id] }} />
          <span className={`shrink-0 text-[13px] text-ink ${open ? "font-medium" : ""}`}>{slice.label}</span>
          {slice.note && <span className="min-w-0 truncate text-[11px] text-ink-4">{slice.note}</span>}
          <span className="ml-auto shrink-0 text-[13px] tabular-nums text-ink-3">{fmtTokens(slice.tokens)}</span>
          <span className="w-9 shrink-0 text-right text-[11px] tabular-nums text-ink-4">{fmtShare(slice.tokens, span)}</span>
        </>
      }
    />
  );
}

function FreeRow({ slice, span }: { slice: ContextSlice; span: number }) {
  return (
    <div className="flex h-7 items-center gap-2 px-2">
      <span className="size-2 shrink-0 rounded-full border border-[#d1d1d6] bg-[#f2f2f7]" />
      <span className="text-[13px] text-ink-4">{slice.label}</span>
      <span className="ml-auto text-[13px] tabular-nums text-ink-4">{fmtTokens(slice.tokens)}</span>
      <span className="w-9 text-right text-[11px] tabular-nums text-ink-4">{fmtShare(slice.tokens, span)}</span>
      <span className="size-3" />
    </div>
  );
}

/** 一行，点开在下面展开明细（grid 行高过渡）。 */
function ExpandRow({
  head,
  open,
  onToggle,
  detail,
  includes = [],
}: {
  head: ReactNode;
  open: boolean;
  onToggle: () => void;
  detail: ContextDetailGroup[];
  includes?: ContextSlice["includes"];
}) {
  const expandable = detail.some((g) => g.rows.length > 0) || includes.length > 0;
  return (
    <div className={`motion-colors flex flex-col rounded-md ${open ? "bg-[#f5f5f7]" : ""}`}>
      <button
        type="button"
        disabled={!expandable}
        aria-expanded={open}
        onClick={onToggle}
        className="motion-colors flex h-7 w-full items-center gap-2 rounded-md px-2 text-left enabled:hover:bg-[#f5f5f7]"
      >
        {head}
        <ChevronRight
          size={12}
          data-open={open}
          className={`rotate-open shrink-0 ${expandable ? "text-ink-4" : "text-transparent"}`}
        />
      </button>
      <div className="fold" data-open={open} aria-hidden={!open}>
        <div>
          <div className="flex flex-col gap-2 pb-2 pl-[26px] pr-2 pt-0.5">
            {includes.length > 0 && (
              <div className="flex flex-col gap-0.5">
                <span className="text-[10px] font-medium text-ink-4">其中</span>
                {includes.map((i) => (
                  <DetailLine key={i.label} label={i.label} note={i.note} tokens={i.tokens} />
                ))}
              </div>
            )}
            {detail
              .filter((g) => g.rows.length > 0)
              .map((g, gi) => (
                <div key={`${gi}:${g.heading}`} className="flex flex-col gap-0.5">
                  {g.heading && <span className="text-[10px] font-medium text-ink-4">{g.heading}</span>}
                  {g.rows.map((r, ri) => (
                    <DetailLine key={`${ri}:${r.label}`} label={r.label} note={r.note} tokens={r.tokens} />
                  ))}
                </div>
              ))}
          </div>
        </div>
      </div>
    </div>
  );
}

function DetailLine({ label, note, tokens }: { label: string; note: string | null; tokens: number | null }) {
  return (
    <div className="flex items-baseline gap-2 text-xs text-ink-3" title={note ?? undefined}>
      <span className="shrink-0 font-mono text-[11.5px] text-ink-2">{label}</span>
      {note && <span className="min-w-0 truncate text-[11px] text-ink-4">{note}</span>}
      <span className="ml-auto shrink-0 tabular-nums">{tokens === null ? "" : fmtTokens(tokens)}</span>
    </div>
  );
}
