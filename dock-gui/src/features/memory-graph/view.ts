// 图上画什么：把 Dock 给的图（笔记 + 边）按筛选条件整理成点和线。
//
// 颜色只分三类——当前项目 / 全局 / 其他项目。图上任意两个点都可能挨着，颜色多了分不清
// （色觉障碍下尤其如此），所以别的项目共用一种颜色，靠聚在一起的位置和直接标注的项目名区分。
// 三个色值取自经过校验的分类色板前三格（两两之间在红绿色盲模拟下也分得开）。
// 主题 / 观察不用颜色区分：主题实心、观察空心。

import type { MemoryGraph, MemoryNote } from "./api";

export type NodeGroup = "current" | "global" | "other" | "ghost";

export const GROUP_COLOR: Record<NodeGroup, string> = {
  current: "#2a78d6",
  global: "#eb6834",
  other: "#1baf7a",
  ghost: "#a1a1a6",
};

export const GROUP_LABEL: Record<Exclude<NodeGroup, "ghost">, string> = {
  current: "当前项目",
  global: "全局",
  other: "其他项目",
};

export interface ViewNode {
  id: string;
  label: string;
  group: NodeGroup;
  /** 所在 scope（悬空的点跟着引用它的那篇）。 */
  scope: string;
  /** 观察：画成空心。 */
  hollow: boolean;
  /** 指向不存在的笔记的链接目标：还没有这篇。 */
  ghost: boolean;
  /** 连着几条链接（相似建议不算）：决定点的大小。 */
  degree: number;
  note: MemoryNote | null;
}

export interface ViewEdge {
  id: string;
  source: string;
  target: string;
  /** `ghost`：指向还不存在的笔记。 */
  kind: "link" | "similar" | "ghost";
  score: number;
  /** 两篇互相链接（只画一条线，两头都有箭头）。 */
  mutual: boolean;
}

export interface Filters {
  topics: boolean;
  observations: boolean;
  /** 没有任何链接的笔记（相似建议不算）。 */
  orphans: boolean;
}

export const DEFAULT_FILTERS: Filters = { topics: true, observations: true, orphans: true };

export const ghostId = (target: string) => `ghost:${target.trim().toLowerCase()}`;

export function groupOf(graph: Pick<MemoryGraph, "currentScope">, scope: string): Exclude<NodeGroup, "ghost"> {
  if (scope === "global") return "global";
  return scope === graph.currentScope ? "current" : "other";
}

export interface GraphView {
  nodes: ViewNode[];
  edges: ViewEdge[];
}

/**
 * 相似建议总是带上：画不画、画哪几条由画布按设置和悬停 / 选中决定（默认只画悬停 / 选中那篇的）。
 * `ghosts`：画不画链接指向、但还没写的笔记。
 */
export function buildView(graph: MemoryGraph, filters: Filters, { ghosts }: { ghosts: boolean }): GraphView {
  const notes = graph.notes.filter((n) => (n.kind === "topic" ? filters.topics : filters.observations));
  const nodes = new Map<string, ViewNode>();
  for (const note of notes) {
    nodes.set(note.id, {
      id: note.id,
      label: note.title,
      group: groupOf(graph, note.scope),
      scope: note.scope,
      hollow: note.kind === "observation",
      ghost: false,
      degree: 0,
      note,
    });
  }

  const edges = new Map<string, ViewEdge>();
  for (const e of graph.edges) {
    if (e.kind !== "link" || !nodes.has(e.source) || !nodes.has(e.target)) continue;
    // 反方向已经有了：并成一条双向的。
    const back = edges.get(`link:${e.target}→${e.source}`);
    if (back) {
      back.mutual = true;
      continue;
    }
    edges.set(`link:${e.source}→${e.target}`, { id: `link:${e.source}→${e.target}`, source: e.source, target: e.target, kind: "link", score: 1, mutual: false });
  }
  if (ghosts) {
    for (const u of graph.unresolved) {
      const from = nodes.get(u.source);
      if (!from) continue;
      const id = ghostId(u.target);
      if (!nodes.has(id)) {
        nodes.set(id, { id, label: u.target, group: "ghost", scope: from.scope, hollow: true, ghost: true, degree: 0, note: null });
      }
      const key = `ghost:${u.source}→${id}`;
      edges.set(key, { id: key, source: u.source, target: id, kind: "ghost", score: 1, mutual: false });
    }
  }
  for (const e of edges.values()) {
    const [a, b] = [nodes.get(e.source), nodes.get(e.target)];
    if (a) a.degree += 1;
    if (b) b.degree += 1;
  }
  if (!filters.orphans) {
    for (const [id, n] of nodes) if (n.degree === 0) nodes.delete(id);
  }
  for (const e of graph.edges) {
    if (e.kind !== "similar" || !nodes.has(e.source) || !nodes.has(e.target)) continue;
    const key = `similar:${e.source}→${e.target}`;
    edges.set(key, { id: key, source: e.source, target: e.target, kind: "similar", score: e.score, mutual: false });
  }
  return { nodes: [...nodes.values()], edges: [...edges.values()] };
}

/**
 * 局部图谱：一篇笔记和它 `depth` 跳以内的邻居（顺着链接走，不管方向），再加上它自己的相似建议。
 * 邻居之间的链接照画；别人的相似建议不画，免得小图里织成网。
 */
export function neighbourhood(view: GraphView, center: string, depth: number): GraphView {
  if (!view.nodes.some((n) => n.id === center)) return { nodes: [], edges: [] };
  const adjacent = new Map<string, string[]>();
  for (const e of view.edges) {
    if (e.kind === "similar") continue;
    for (const [a, b] of [[e.source, e.target], [e.target, e.source]]) {
      const list = adjacent.get(a);
      if (list) list.push(b);
      else adjacent.set(a, [b]);
    }
  }
  const keep = new Set([center]);
  let frontier = [center];
  for (let step = 0; step < depth; step += 1) {
    const next: string[] = [];
    for (const id of frontier) {
      for (const other of adjacent.get(id) ?? []) {
        if (!keep.has(other)) {
          keep.add(other);
          next.push(other);
        }
      }
    }
    frontier = next;
  }
  for (const e of view.edges) {
    if (e.kind === "similar" && (e.source === center || e.target === center)) keep.add(e.source === center ? e.target : e.source);
  }
  return {
    nodes: view.nodes.filter((n) => keep.has(n.id)),
    edges: view.edges.filter((e) => keep.has(e.source) && keep.has(e.target) && (e.kind !== "similar" || e.source === center || e.target === center)),
  };
}

/** 一篇笔记的相似建议（按相似度从高到低）。 */
export function suggestionsFor(graph: MemoryGraph, id: string): { note: MemoryNote; score: number }[] {
  const byId = new Map(graph.notes.map((n) => [n.id, n]));
  return graph.edges
    .filter((e) => e.kind === "similar" && (e.source === id || e.target === id))
    .map((e) => ({ note: byId.get(e.source === id ? e.target : e.source), score: e.score }))
    .filter((s): s is { note: MemoryNote; score: number } => !!s.note)
    .sort((a, b) => b.score - a.score);
}

/** 每篇笔记的出链 / 入链数（列表视图用）。 */
export function linkCounts(graph: MemoryGraph): Map<string, { out: number; in: number }> {
  const counts = new Map<string, { out: number; in: number }>();
  const at = (id: string) => {
    let c = counts.get(id);
    if (!c) counts.set(id, (c = { out: 0, in: 0 }));
    return c;
  };
  for (const e of graph.edges) {
    if (e.kind !== "link") continue;
    at(e.source).out += 1;
    at(e.target).in += 1;
  }
  return counts;
}
