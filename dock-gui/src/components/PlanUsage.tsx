// 订阅额度的画法：输入框旁的小圆环（PlanUsageBadge）、详情卡（PlanUsageCard，设置页和
// 圆环的弹层共用）。数据来自 dock/planUsage.ts（`auth/usage`）。
//
// 上游只给百分比：卡上只写百分比、重置时间、周期，不编 token 数。
// 水面高度 = 剩余百分比；水波一直慢慢漂（prefers-reduced-motion 时停）。

import { useId } from "react";
import { CalendarDays, Clock, Crown, Droplet, Info, LoaderCircle, RotateCw, Wallet, Waves } from "lucide-react";
import type { PlanUsage } from "../dock/settingsApi";
import {
  formatMoment,
  formatPercent,
  formatSpan,
  levelOf,
  periodWord,
  refreshPlanUsage,
  remainingPercent,
  type PlanUsageState,
} from "../dock/planUsage";
import { Popover } from "./Popover";

/** 三档水色：充足（蓝）、偏少（橙）、见底（红）。 */
const WATER = {
  ok: { top: "#7dd3fc", bottom: "#0284c7", ring: ["#38bdf8", "#2563eb"], text: "#0369a1" },
  low: { top: "#fcd34d", bottom: "#ea580c", ring: ["#fbbf24", "#ea580c"], text: "#b45309" },
  empty: { top: "#fca5a5", bottom: "#dc2626", ring: ["#f87171", "#dc2626"], text: "#b91c1c" },
} as const;

/** 一条可平铺的水波（宽 200 = 两个周期，横移 100 刚好接上）。`y` 是波心高度。 */
const wave = (y: number, amp: number) =>
  `M0 ${y} Q 25 ${y - amp} 50 ${y} T 100 ${y} T 150 ${y} T 200 ${y} V 140 H 0 Z`;

/** 一颗水球：玻璃外壳 + 按剩余百分比的水面（两层波反向漂）+ 高光。`children` 叠在正中。 */
export function WaterOrb({
  size,
  remaining,
  children,
}: {
  size: number;
  /** 0–100；不知道就画空球。 */
  remaining: number | null;
  children?: React.ReactNode;
}) {
  const id = useId().replace(/:/g, "");
  const water = WATER[levelOf(remaining)];
  // 水面高度：100% 齐顶（留 4 的边），0% 见底。
  const level = remaining == null ? 100 : 4 + (1 - remaining / 100) * 92;
  return (
    <div className="relative shrink-0" style={{ width: size, height: size }}>
      <svg viewBox="0 0 100 100" className="absolute inset-0 size-full overflow-visible" aria-hidden>
        <defs>
          <clipPath id={`orb-clip-${id}`}>
            <circle cx="50" cy="50" r="45.5" />
          </clipPath>
          <linearGradient id={`orb-water-${id}`} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={water.top} />
            <stop offset="100%" stopColor={water.bottom} />
          </linearGradient>
          <radialGradient id={`orb-shell-${id}`} cx="35%" cy="28%" r="80%">
            <stop offset="0%" stopColor="#ffffff" stopOpacity="0.95" />
            <stop offset="60%" stopColor="#eaf6ff" stopOpacity="0.75" />
            <stop offset="100%" stopColor="#cfe9fb" stopOpacity="0.8" />
          </radialGradient>
        </defs>
        {/* 外壳与外圈的光晕 */}
        <circle cx="50" cy="50" r="48.5" fill={`url(#orb-shell-${id})`} stroke="#ffffff" strokeOpacity="0.9" strokeWidth="1.5" />
        <circle cx="50" cy="50" r="48.5" fill="none" stroke={water.bottom} strokeOpacity="0.18" strokeWidth="1" />
        <g clipPath={`url(#orb-clip-${id})`}>
          <g className="orb-level" style={{ transform: `translateY(${level}px)` }}>
            <g className="water-drift-slow">
              <path d={wave(0, 4.5)} fill={water.top} opacity="0.55" />
            </g>
            <g className="water-drift">
              <path d={wave(3, 5)} fill={`url(#orb-water-${id})`} opacity="0.92" />
            </g>
          </g>
          {/* 几颗慢慢上浮的气泡 */}
          {remaining != null && remaining > 8 && (
            <g fill="#ffffff" className="orb-bubbles">
              <circle cx="34" cy="86" r="1.6" opacity="0.7" />
              <circle cx="62" cy="90" r="1.1" opacity="0.6" />
              <circle cx="48" cy="94" r="2" opacity="0.5" />
            </g>
          )}
        </g>
        {/* 玻璃高光 */}
        <ellipse cx="35" cy="22" rx="15" ry="7" fill="#ffffff" opacity="0.55" transform="rotate(-24 35 22)" />
      </svg>
      {children && <div className="absolute inset-0 flex flex-col items-center justify-center text-center">{children}</div>}
    </div>
  );
}

/** 小圆环：外圈按剩余百分比画弧，圈里是一颗小水珠。 */
export function UsageRing({ remaining, size = 18 }: { remaining: number | null; size?: number }) {
  const id = useId().replace(/:/g, "");
  const water = WATER[levelOf(remaining)];
  const r = 15;
  const c = 2 * Math.PI * r;
  const shown = remaining ?? 0;
  const level = 6 + (1 - shown / 100) * 24;
  return (
    <svg viewBox="0 0 36 36" width={size} height={size} className="shrink-0 overflow-visible" aria-hidden>
      <defs>
        <linearGradient id={`ring-${id}`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor={water.ring[0]} />
          <stop offset="100%" stopColor={water.ring[1]} />
        </linearGradient>
        <clipPath id={`ring-clip-${id}`}>
          <circle cx="18" cy="18" r="10.5" />
        </clipPath>
      </defs>
      <circle cx="18" cy="18" r={r} fill="none" stroke={water.ring[0]} strokeOpacity="0.2" strokeWidth="3.4" />
      <circle
        cx="18"
        cy="18"
        r={r}
        fill="none"
        stroke={`url(#ring-${id})`}
        strokeWidth="3.4"
        strokeLinecap="round"
        strokeDasharray={c}
        strokeDashoffset={c * (1 - shown / 100)}
        transform="rotate(-90 18 18)"
        className="ring-arc"
      />
      <g clipPath={`url(#ring-clip-${id})`}>
        <circle cx="18" cy="18" r="10.5" fill={water.ring[0]} fillOpacity="0.1" />
        <g className="orb-level" style={{ transform: `translateY(${level}px)` }}>
          <g className="water-drift">
            <path d={`M0 0 Q 6 -1.6 12 0 T 24 0 T 36 0 T 48 0 V 40 H 0 Z`} fill={water.bottom} opacity="0.75" />
          </g>
        </g>
      </g>
    </svg>
  );
}

/** 输入框旁的额度标识：圆环 + 剩余百分比；点开是详情卡。 */
export function PlanUsageBadge({
  provider,
  providerName,
  state,
}: {
  provider: string;
  providerName: string;
  state: PlanUsageState;
}) {
  const u = state.usage;
  const remaining = remainingPercent(u);
  const tip = [
    `${providerName}${u?.plan ? ` · ${u.plan}` : ""}`,
    remaining == null ? "额度：上游没给已用多少" : `${periodWord(u)}剩余 ${formatPercent(remaining)}%`,
    resetText(u),
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <Popover
      title={tip}
      align="right"
      side="top"
      className="flex h-7 items-center gap-1.5 rounded-full bg-[#f5f5f7] pl-1.5 pr-2.5 text-xs font-medium text-ink-3 hover:bg-[#ececf0]"
      panelClassName="w-[320px] max-h-[80vh] !py-0"
      trigger={
        <>
          <UsageRing remaining={remaining} />
          <span className="tabular-nums" style={{ color: remaining != null ? WATER[levelOf(remaining)].text : undefined }}>
            {remaining == null ? "—" : `${formatPercent(remaining)}%`}
          </span>
          <span className="hidden text-ink-4 @[60rem]:inline">{u?.plan ?? providerName}</span>
        </>
      }
    >
      {() => (
        <div className="p-3">
          <PlanUsageCard provider={provider} providerName={providerName} state={state} compact />
          <button
            type="button"
            onClick={() =>
              window.dispatchEvent(new CustomEvent("dock:open-settings", { detail: "models" }))
            }
            className="motion-colors mt-2 w-full rounded-lg px-2 py-1.5 text-left text-xs text-ink-3 hover:bg-[#f2f2f7]"
          >
            在设置 › 模型里查看订阅与账号 ›
          </button>
        </div>
      )}
    </Popover>
  );
}

/** 「2 天 14 小时后重置（11-24 10:21）」；没有周期就空。 */
function resetText(u: PlanUsage | null | undefined): string {
  const end = u?.period?.end;
  if (!end) return "";
  const left = end - Math.floor(Date.now() / 1000);
  return left > 0 ? `${formatSpan(left)}后重置（${formatMoment(end)}）` : `已到重置时间（${formatMoment(end)}）`;
}

/** 多久前查的。 */
function checkedAgo(ms: number): string {
  if (!ms) return "";
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 60) return "刚刚查过";
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前查的`;
  return `${Math.floor(s / 3600)} 小时前查的`;
}

/** 额度详情卡：水球 + 剩余 / 已用 / 重置 / 周期 / 预付余额。设置页与圆环弹层共用。 */
export function PlanUsageCard({
  provider,
  providerName,
  state,
  compact = false,
}: {
  provider: string;
  providerName: string;
  state: PlanUsageState;
  /** 弹层里的窄版：水球小一点、竖排。 */
  compact?: boolean;
}) {
  const u = state.usage;
  const remaining = remainingPercent(u);
  const used = u?.usedPercent ?? null;
  const water = WATER[levelOf(remaining)];
  const word = periodWord(u);
  // 水面没过球心才用白字；水少时字落在浅色的球壳上，用深色。
  const onWater = remaining != null && remaining >= 55;
  const firstLoad = !u && state.loading;
  const period = u?.period;
  return (
    <div
      className={`enter-fade relative overflow-hidden rounded-2xl border border-line-soft bg-[#f0f7ff] ${
        compact ? "p-3" : "p-4"
      }`}
    >
      {/* 右上角一抹水纹（装饰） */}
      <svg
        viewBox="0 0 200 60"
        className="pointer-events-none absolute -right-6 -top-2 h-16 w-56 opacity-60"
        aria-hidden
      >
        <path d="M0 40 C 40 10, 80 60, 120 30 S 180 10, 200 25" fill="none" stroke="#bae6fd" strokeWidth="2" />
        <path d="M10 52 C 50 25, 90 70, 130 42 S 185 25, 200 38" fill="none" stroke="#dbeafe" strokeWidth="2" />
      </svg>

      {/* 头：图标、套餐名、状态、刷新 */}
      <div className="relative flex items-center gap-2.5">
        <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-gradient-to-br from-[#38bdf8] to-[#2563eb] text-white shadow-[0_4px_12px_rgba(37,99,235,0.3)]">
          <Crown className="size-4" strokeWidth={2.2} />
        </span>
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="flex items-center gap-1.5">
            <span className="truncate text-[13.5px] font-semibold text-ink">{u?.plan ?? providerName}</span>
            <span className="rounded-full bg-[#eaf8ee] px-1.5 py-px text-[10.5px] font-medium text-[#15803d]">生效中</span>
          </span>
          <span className="truncate text-[11.5px] text-ink-4">
            {providerName} 订阅 · {period?.kind === "weekly" ? "按周计" : period?.kind === "monthly" ? "按月计" : "额度"}
          </span>
        </div>
        <button
          type="button"
          title={checkedAgo(state.checkedAt) || "刷新"}
          aria-label="刷新额度"
          disabled={state.loading}
          onClick={() => void refreshPlanUsage(provider, true)}
          className="motion-colors flex size-7 shrink-0 items-center justify-center rounded-full text-ink-4 hover:bg-white/70 hover:text-ink-2 disabled:opacity-60"
        >
          {state.loading ? <LoaderCircle className="size-3.5 animate-spin" /> : <RotateCw className="size-3.5" />}
        </button>
      </div>

      {firstLoad ? (
        <div className="relative flex items-center gap-2 py-6 text-xs text-ink-4">
          <LoaderCircle className="size-3.5 animate-spin" />
          正在查额度…
        </div>
      ) : !u ? (
        <div className="relative py-3 text-xs text-[#c0392b]">{state.error ?? "没查到额度"}</div>
      ) : (
        <div className={`relative mt-3 flex ${compact ? "flex-col items-center gap-3" : "items-center gap-5"}`}>
          <WaterOrb size={compact ? 112 : 136} remaining={remaining}>
            <span
              className={`font-semibold tabular-nums leading-none ${compact ? "text-[26px]" : "text-[32px]"}`}
              style={{
                color: onWater ? "#ffffff" : water.text,
                textShadow: onWater ? "0 1px 6px rgba(3,105,161,0.35)" : undefined,
              }}
            >
              {remaining == null ? "—" : formatPercent(remaining)}
              <span className={compact ? "text-[14px]" : "text-[16px]"}>%</span>
            </span>
            <span className="mt-1 text-[11px] font-medium" style={{ color: onWater ? "rgba(255,255,255,0.92)" : water.text }}>
              剩余额度
            </span>
          </WaterOrb>

          <div className="flex w-full min-w-0 flex-1 flex-col gap-2.5">
            <div className="flex items-center gap-1 text-[12.5px] font-medium text-ink-2">
              {word}剩余额度
              <span title="xAI 订阅额度池（与 Grok Build 共用、按它计量）。上游只给百分比。" className="text-ink-4">
                <Info className="size-3.5" />
              </span>
            </div>
            <div className="flex items-baseline gap-1.5">
              <span className="text-[22px] font-semibold tabular-nums leading-none text-ink">
                {remaining == null ? "—" : `${formatPercent(remaining)}%`}
              </span>
              <span className="text-xs text-ink-4">/ 100%</span>
            </div>
            <div className="h-2 overflow-hidden rounded-full bg-[#dbeafe]">
              <div
                className="usage-bar h-full rounded-full"
                style={{
                  width: `${remaining ?? 0}%`,
                  backgroundImage: `linear-gradient(90deg, ${water.ring[0]}, ${water.ring[1]})`,
                }}
              />
            </div>
            <div className="flex flex-col gap-1.5 text-xs text-ink-3">
              <StatRow icon={<Droplet className="size-3.5" />} label="剩余" value={remaining == null ? "—" : `${formatPercent(remaining)}%`} />
              <StatRow icon={<Waves className="size-3.5" />} label="已用" value={used == null ? "—" : `${formatPercent(used)}%`} />
              {u.prepaidBalanceCents != null && u.prepaidBalanceCents > 0 && (
                <StatRow icon={<Wallet className="size-3.5" />} label="预付余额" value={`$${(u.prepaidBalanceCents / 100).toFixed(2)}`} />
              )}
            </div>
            {period?.end != null && (
              <div className="flex items-center gap-1.5 rounded-lg bg-white px-2.5 py-1.5 text-xs text-ink-3">
                <Clock className="size-3.5 shrink-0 text-ink-4" />
                <span className="truncate">{resetText(u)}</span>
              </div>
            )}
            {!compact && period?.start != null && period?.end != null && (
              <div className="flex items-center gap-1.5 px-0.5 text-[11.5px] text-ink-4">
                <CalendarDays className="size-3.5 shrink-0" />
                计费周期 {formatMoment(period.start)} 至 {formatMoment(period.end)}
              </div>
            )}
            {state.error && <span className="text-[11.5px] text-[#c0392b]">刷新失败：{state.error}（显示的是上次的数）</span>}
          </div>
        </div>
      )}
    </div>
  );
}

function StatRow({ icon, label, value }: { icon: React.ReactNode; label: string; value: string }) {
  return (
    <div className="flex items-center gap-2">
      <span className="flex size-5 items-center justify-center rounded-full bg-[#eaf3ff] text-[#0284c7]">{icon}</span>
      <span>{label}</span>
      <span className="ml-auto font-medium tabular-nums text-ink-2">{value}</span>
    </div>
  );
}
