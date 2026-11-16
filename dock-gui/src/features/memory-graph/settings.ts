// 图谱设置：显示（箭头、相似建议、标签、点大小、线粗）、力导向、按条件着色、局部图谱。
// 记在本机（localStorage），下次打开还是这样；改了立即生效。

import { useSyncExternalStore } from "react";

/**
 * 按条件着色能用的颜色：经过校验的分类色板前三格。
 * 图上任意两个点都可能挨着，三种以上在色觉障碍下就分不清了，所以最多三组；
 * 着色时没命中的点变灰，三组颜色不会和范围的颜色同时出现。
 */
export const GROUP_SLOTS = ["#2a78d6", "#eb6834", "#1baf7a"] as const;
export const MAX_GROUPS = GROUP_SLOTS.length;

export interface ColorGroup {
  /** 和搜索框一样：标题、文件名、正文里都要出现每个词。 */
  query: string;
  /** 用 `GROUP_SLOTS` 的第几格：建组时定下，删了前面的组颜色也不变。 */
  slot: number;
}

export interface GraphSettings {
  /** 平时也画箭头；关着时只有悬停 / 选中那篇的线画方向。 */
  arrows: boolean;
  /** 全图都画相似建议；关着时只画悬停 / 选中那篇的。 */
  allSuggestions: boolean;
  /** 画出链接指向、但还没写的笔记。 */
  ghosts: boolean;
  /** 0–100：标签多少。0 只标悬停 / 选中的那几篇。 */
  labels: number;
  /** 百分比。 */
  nodeSize: number;
  lineWidth: number;
  /** 0–100。 */
  center: number;
  repel: number;
  linkStrength: number;
  /** 布局像素。 */
  linkDistance: number;
  groups: ColorGroup[];
  /** 抽屉里的局部图谱看几跳。 */
  localDepth: 1 | 2;
  localOpen: boolean;
}

export const DEFAULT_SETTINGS: GraphSettings = {
  arrows: false,
  allSuggestions: false,
  ghosts: true,
  labels: 50,
  nodeSize: 100,
  lineWidth: 100,
  center: 50,
  repel: 50,
  linkStrength: 50,
  linkDistance: 90,
  groups: [],
  localDepth: 1,
  localOpen: true,
};

/** 「恢复默认」管的那些（着色分组和局部图谱的开关不动）。 */
export const RESETTABLE = ["arrows", "allSuggestions", "ghosts", "labels", "nodeSize", "lineWidth", "center", "repel", "linkStrength", "linkDistance"] as const;

export const RANGES = {
  labels: [0, 100],
  nodeSize: [50, 200],
  lineWidth: [50, 300],
  center: [0, 100],
  repel: [0, 100],
  linkStrength: [0, 100],
  linkDistance: [20, 240],
} as const satisfies Partial<Record<keyof GraphSettings, readonly [number, number]>>;

const KEY = "dock-gui.memory-graph";
const listeners = new Set<() => void>();
let current = load();

function load(): GraphSettings {
  try {
    return sanitize(JSON.parse(localStorage.getItem(KEY) ?? "{}"));
  } catch {
    return DEFAULT_SETTINGS;
  }
}

/** 存下来的东西可能是旧版本或手改过的：认得的键才收，数值夹回范围里。 */
function sanitize(raw: unknown): GraphSettings {
  const src = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const out: GraphSettings = { ...DEFAULT_SETTINGS };
  for (const key of ["arrows", "allSuggestions", "ghosts", "localOpen"] as const) {
    if (typeof src[key] === "boolean") out[key] = src[key];
  }
  for (const [key, [min, max]] of Object.entries(RANGES) as [keyof typeof RANGES, readonly [number, number]][]) {
    const value = src[key];
    if (typeof value === "number" && Number.isFinite(value)) out[key] = Math.min(max, Math.max(min, value));
  }
  if (src.localDepth === 1 || src.localDepth === 2) out.localDepth = src.localDepth;
  if (Array.isArray(src.groups)) {
    const used = new Set<number>();
    out.groups = src.groups
      .filter((g): g is ColorGroup => !!g && typeof g.query === "string" && Number.isInteger(g.slot) && g.slot >= 0 && g.slot < MAX_GROUPS)
      .filter((g) => !used.has(g.slot) && used.add(g.slot))
      .slice(0, MAX_GROUPS)
      .map((g) => ({ query: g.query, slot: g.slot }));
  }
  return out;
}

export function setGraphSettings(patch: Partial<GraphSettings>) {
  current = sanitize({ ...current, ...patch });
  try {
    localStorage.setItem(KEY, JSON.stringify(current));
  } catch {
    // 存不下（隐私模式、配额满）：这次照样生效，只是下次不记得。
  }
  for (const l of listeners) l();
}

export function resetGraphSettings() {
  const patch: Partial<GraphSettings> = {};
  for (const key of RESETTABLE) Object.assign(patch, { [key]: DEFAULT_SETTINGS[key] });
  setGraphSettings(patch);
}

export function useGraphSettings(): GraphSettings {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}

/** 新的一组用哪格颜色：第一格没被占的。都占了是 null。 */
export function freeSlot(groups: ColorGroup[]): number | null {
  for (let i = 0; i < MAX_GROUPS; i += 1) if (!groups.some((g) => g.slot === i)) return i;
  return null;
}
