// 订阅额度（`auth/usage`）：按「网关 + provider」缓存一份，输入框旁的圆环和设置页共用。
//
// 查询时机：第一次有人要（挂载）、过了 STALE_MS 再有人要、每 POLL_MS 一次（有人订着时）、
// 一轮结束（refreshPlanUsage，最短间隔 MIN_GAP_MS）、`auth/changed`（登录 / 登出 / 切账号）。
// 每次都是上游的当前值，Dock 那头不缓存。

import { useEffect, useSyncExternalStore } from "react";
import { authUsage, errorText, onAuthChanged, type PlanUsage } from "./settingsApi";
import { gatewayArg } from "./gateways";
import { RpcError } from "./client";

export interface PlanUsageState {
  usage: PlanUsage | null;
  /** 这份额度是哪个账号的（Dock 回的活动账号 id）。 */
  account: string | null;
  /** 查不了（这一家没有额度接口，或老 Dock 没有这个方法）。 */
  unsupported: boolean;
  error: string | null;
  loading: boolean;
  /** 上次查完的时刻（ms）。 */
  checkedAt: number;
}

const EMPTY: PlanUsageState = { usage: null, account: null, unsupported: false, error: null, loading: false, checkedAt: 0 };
const STALE_MS = 60_000;
const POLL_MS = 5 * 60_000;
const MIN_GAP_MS = 20_000;

const states = new Map<string, PlanUsageState>();
const inflight = new Map<string, Promise<void>>();
const listeners = new Set<() => void>();
const watchers = new Map<string, number>();

const keyOf = (provider: string) => `${gatewayArg() ?? "local"}:${provider}`;
const emit = () => listeners.forEach((fn) => fn());
/** 上次查到的额度存一份：重开 GUI 先画它（不先闪一下「Using … plan」），后台再刷新。 */
const SAVED = "dock-gui.planUsage.";

/** 这个键现在的状态；第一次看时从上次存的那份起步（`checkedAt` 是 0，挂载后会重查）。 */
function stateOf(key: string): PlanUsageState {
  const known = states.get(key);
  if (known) return known;
  let saved: { account: string | null; usage: PlanUsage } | null = null;
  try {
    const raw = localStorage.getItem(SAVED + key);
    if (raw) saved = JSON.parse(raw);
  } catch {
    saved = null;
  }
  const first = saved?.usage ? { ...EMPTY, usage: saved.usage, account: saved.account ?? null } : EMPTY;
  states.set(key, first);
  return first;
}

function set(key: string, next: Partial<PlanUsageState>) {
  states.set(key, { ...stateOf(key), ...next });
  emit();
}

function save(key: string, account: string | null, usage: PlanUsage | null) {
  try {
    if (usage) localStorage.setItem(SAVED + key, JSON.stringify({ account, usage }));
    else localStorage.removeItem(SAVED + key);
  } catch {
    // 存不了就每次重查，不影响显示。
  }
}

/** 查一次。`force` = 不管多久前查过；否则 MIN_GAP_MS 内只查一次。 */
export function refreshPlanUsage(provider: string, force = false): Promise<void> {
  const key = keyOf(provider);
  const running = inflight.get(key);
  if (running) return running;
  const prev = stateOf(key);
  if (!force && Date.now() - prev.checkedAt < MIN_GAP_MS) return Promise.resolve();
  set(key, { loading: true });
  const task = authUsage(provider)
    .then((res) => {
      const account = res.account ?? null;
      save(key, account, res.usage);
      set(key, { usage: res.usage, account, unsupported: !res.usage, error: null });
    })
    .catch((e) => {
      const code = e instanceof RpcError ? e.code : undefined;
      // 老 Dock 没有 auth/usage：当作查不了，不报错，也不留旧数。
      if (code === "method_not_found") {
        save(key, null, null);
        return set(key, { usage: null, account: null, unsupported: true, error: null });
      }
      // 登出了 / 没资格：上一个账号的数不能留着（会被当成现在这个账号的）。
      if (code === "needs_login" || code === "not_eligible") {
        save(key, null, null);
        return set(key, { usage: null, account: null, error: errorText(e) });
      }
      // 网络、上游临时失败：留着上次的数，标出刷新失败。
      set(key, { error: errorText(e) });
    })
    .finally(() => {
      inflight.delete(key);
      set(key, { loading: false, checkedAt: Date.now() });
    });
  inflight.set(key, task);
  return task;
}

/** 订阅 `provider` 的额度（`null` = 不订）。挂着就会查、会定时刷新。 */
export function usePlanUsage(provider: string | null | undefined): PlanUsageState {
  const key = provider ? keyOf(provider) : "";
  const state = useSyncExternalStore(
    (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    () => (key ? stateOf(key) : EMPTY),
  );
  useEffect(() => {
    if (!provider) return;
    const k = keyOf(provider);
    watchers.set(k, (watchers.get(k) ?? 0) + 1);
    if (Date.now() - stateOf(k).checkedAt > STALE_MS) void refreshPlanUsage(provider, true);
    const timer = window.setInterval(() => void refreshPlanUsage(provider, true), POLL_MS);
    // 登录 / 登出 / 切账号：先把旧数清掉再重查，不先画上一个账号的额度。
    const off = onAuthChanged(() => {
      save(k, null, null);
      set(k, { usage: null, account: null });
      void refreshPlanUsage(provider, true);
    });
    return () => {
      window.clearInterval(timer);
      off();
      const n = (watchers.get(k) ?? 1) - 1;
      if (n <= 0) watchers.delete(k);
      else watchers.set(k, n);
    };
  }, [provider]);
  return state;
}

/** 剩余百分比（0–100），不知道就 null。 */
export const remainingPercent = (u: PlanUsage | null | undefined) =>
  u?.usedPercent == null ? null : Math.max(0, Math.min(100, 100 - u.usedPercent));

/** 百分比的显示：整数不带小数，否则一位。 */
export const formatPercent = (p: number) => {
  const v = Math.max(0, Math.min(100, p));
  return Math.abs(v - Math.round(v)) < 0.05 ? String(Math.round(v)) : v.toFixed(1);
};

/** 一段时长：`2 天 14 小时` / `5 小时 3 分` / `12 分`。 */
export function formatSpan(secs: number): string {
  const d = Math.floor(secs / 86_400);
  const h = Math.floor((secs % 86_400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d} 天 ${h} 小时`;
  if (h > 0) return `${h} 小时 ${m} 分`;
  return `${Math.max(1, m)} 分`;
}

/** unix 秒 → `11-24 10:21`（本地时间）。 */
export function formatMoment(secs: number): string {
  const d = new Date(secs * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 「本周」「本月」「本周期」。 */
export const periodWord = (u: PlanUsage | null | undefined) =>
  u?.period?.kind === "weekly" ? "本周" : u?.period?.kind === "monthly" ? "本月" : "本周期";

/** 剩余越少越警示：充足蓝、偏少橙、见底红。 */
export function levelOf(remaining: number | null): "ok" | "low" | "empty" {
  if (remaining == null || remaining > 20) return "ok";
  return remaining > 5 ? "low" : "empty";
}
