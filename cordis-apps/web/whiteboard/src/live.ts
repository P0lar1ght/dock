// Agent 还在写参数时的预览（宿主发来的 tool-input-partial / tool-input）：算出要画的草稿骨架、
// 要动的现有图形、Agent 光标停在哪。纯函数，不碰 Excalidraw（Node 自带 test 直接跑：npm test）。
//
// - 规范不说是哪颗工具，按参数的形状认：`add/update/delete/clear` 是 whiteboard_edit，
//   `nodes/edges` 是 whiteboard_graph，带 `ids` / `id` 的是排版类工具；
// - 草稿的位置照服务器算（scene.rs 的 add_one / free_slot / graph_positions），结果到了换成真的
//   不会跳；还在写（`final` 为假）时数组最后一项可能没写完，先不画；
// - 草稿 id 带 LIVE_PREFIX：不存回服务器、不交给模型，结果到了整批换掉。

import type { El } from "./geometry";

/* eslint-disable @typescript-eslint/no-explicit-any */
export const LIVE_PREFIX = "dock-live:";
export const isLive = (e: { id: string }) => e.id.startsWith(LIVE_PREFIX);

// 与 cordis-apps/src/whiteboard/scene.rs 同步。
const GAP = 60;
const DEFAULT_W = 180;
const DEFAULT_H = 80;
const GRAPH_W = 200;
const GRAPH_H = 80;
const GRAPH_GAP_X = 60;
const GRAPH_GAP_Y = 80;
const SHAPES = ["rectangle", "ellipse", "diamond", "text", "arrow", "line"];
const NODE_SHAPES = ["rectangle", "ellipse", "diamond", "text"];

export interface LivePlan {
  /** 草稿骨架（`convertToExcalidrawElements` 的输入），id 带 LIVE_PREFIX。 */
  skeletons: Record<string, any>[];
  /** 这次调用要动的现有图形（高亮成 Agent 选中的样子）。 */
  targets: string[];
  /** Agent 光标停在哪（场景坐标）；`null` = 还不知道。 */
  pointer: { x: number; y: number } | null;
  /** 一句话：Agent 在干什么。 */
  status: string;
}

interface Frame {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const str = (v: unknown): string | null => (typeof v === "string" && v.trim() ? v.trim() : null);
const list = (v: unknown): unknown[] => {
  if (Array.isArray(v)) return v;
  // 服务器也认的 `{item: [...]}` 包装（模型常这么写）。
  if (v && typeof v === "object") {
    const values = Object.values(v as Record<string, unknown>);
    if (values.length === 1 && Array.isArray(values[0])) return values[0];
  }
  return [];
};
const obj = (v: unknown): Record<string, any> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, any>) : {});

function frameOf(e: El): Frame {
  return { id: e.id, x: e.x ?? 0, y: e.y ?? 0, w: Math.abs(e.width ?? 0), h: Math.abs(e.height ?? 0) };
}

/** 现有内容 + 已经排好的草稿的外框（服务器 `Scene::bounds`）。 */
function bounds(frames: readonly Frame[]): { left: number; top: number; right: number; bottom: number } | null {
  if (frames.length === 0) return null;
  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  for (const f of frames) {
    left = Math.min(left, f.x);
    top = Math.min(top, f.y);
    right = Math.max(right, f.x + f.w);
    bottom = Math.max(bottom, f.y + f.h);
  }
  return { left, top, right, bottom };
}

/** 服务器 `free_slot`：现有内容右边起，横着找一块不撞的空地。 */
function freeSlot(frames: readonly Frame[], w: number, h: number): [number, number] {
  const b = bounds(frames);
  const [sx, sy] = b ? [b.right + GAP * 2, b.top] : [100, 100];
  const ww = Math.max(w, 24);
  const hh = Math.max(h, 24);
  const hits = (x: number, y: number) => frames.some((f) => x < f.x + f.w && x + ww > f.x && y < f.y + f.h && y + hh > f.y);
  let y = sy;
  for (let row = 0; row < 200; row++) {
    for (let x = sx; x < sx + 6000; x += GAP) if (!hits(x, y)) return [x, y];
    y += hh + GAP;
  }
  return [sx, y];
}

/** 服务器 `connect`：箭头从 a 的边指向 b 的边。两个中心重合时回 null。 */
function connect(a: Frame, b: Frame): { x: number; y: number; width: number; height: number; points: number[][] } | null {
  const GAP_END = 8;
  const [ax, ay, aw, ah] = [a.x + a.w / 2, a.y + a.h / 2, a.w / 2, a.h / 2];
  const [bx, by, bw, bh] = [b.x + b.w / 2, b.y + b.h / 2, b.w / 2, b.h / 2];
  const [dx, dy] = [bx - ax, by - ay];
  const len = Math.hypot(dx, dy);
  if (len < 1) return null;
  const [ux, uy] = [dx / len, dy / len];
  const exit = (hw: number, hh: number) => Math.min(Math.abs(ux) > 1e-9 ? hw / Math.abs(ux) : Infinity, Math.abs(uy) > 1e-9 ? hh / Math.abs(uy) : Infinity);
  let ta = exit(aw, ah) + GAP_END;
  let tb = exit(bw, bh) + GAP_END;
  if (ta + tb >= len) ta = tb = len * 0.4;
  const [sx, sy] = [ax + ux * ta, ay + uy * ta];
  const [ex, ey] = [bx - ux * tb, by - uy * tb];
  return { x: sx, y: sy, width: Math.abs(ex - sx), height: Math.abs(ey - sy), points: [[0, 0], [ex - sx, ey - sy]] };
}

const COLOR = /^(#([0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})|transparent|[A-Za-z]{1,20})$/;

/** 服务器 `style_into`：Agent 的叫法 → Excalidraw 字段。认不出的颜色不画（服务器会报错）。 */
function style(s: Record<string, any>, shape: Record<string, any>) {
  const stroke = str(shape.stroke);
  if (stroke && COLOR.test(stroke)) s.strokeColor = stroke;
  const fill = str(shape.fill);
  if (fill && COLOR.test(fill)) {
    s.backgroundColor = fill;
    if (fill !== "transparent") s.fillStyle = "solid";
  }
  const line = str(shape.strokeStyle);
  if (line && ["solid", "dashed", "dotted"].includes(line)) s.strokeStyle = line;
}

/** 一个要加的图形 → 草稿骨架（服务器 `add_one`）。认不出的类型回 null。 */
function addShape(shape: Record<string, any>, index: number, frames: Frame[], byId: Map<string, Frame>): Record<string, any> | null {
  const kind = str(shape.type);
  if (!kind || !SHAPES.includes(kind)) return null;
  const linear = kind === "arrow" || kind === "line";
  const w = kind === "text" ? 120 : (num(shape.width) ?? (linear ? 160 : DEFAULT_W));
  const h = kind === "text" ? 30 : (num(shape.height) ?? (linear ? 0 : DEFAULT_H));
  const b = bounds(frames);
  const sx = num(shape.x);
  const sy = num(shape.y);
  const [x, y] =
    sx !== null && sy !== null
      ? [sx, sy]
      : sx !== null
        ? [sx, b ? b.top : 100]
        : sy !== null
          ? [b ? b.right + GAP * 2 : 100, sy]
          : freeSlot(frames, w, h);
  const own = str(shape.id) ?? `#${index}`;
  const s: Record<string, any> = { type: kind, id: `${LIVE_PREFIX}${own}`, x, y };
  if (kind !== "text") Object.assign(s, { width: w, height: h });
  style(s, shape);
  const text = typeof shape.text === "string" ? shape.text : null;
  if (text !== null) {
    if (kind === "text") s.text = text;
    else if (text) s.label = { text };
  }
  if (kind === "text" && num(shape.fontSize) !== null) s.fontSize = num(shape.fontSize);
  if (linear) {
    if (Array.isArray(shape.points)) s.points = shape.points;
    else if (sx === null && sy === null) {
      const from = byId.get(str(shape.from) ?? "");
      const to = byId.get(str(shape.to) ?? "");
      const line = from && to ? connect(from, to) : null;
      if (line) Object.assign(s, line);
    }
  }
  const frame = { id: own, x: s.x, y: s.y, w: Math.abs(s.width ?? w), h: Math.abs(s.height ?? h) };
  frames.push(frame);
  byId.set(own, frame);
  return s;
}

/** 服务器 `graph_positions`：节点的左上角。 */
export function graphPositions(
  nodes: { id: string; w: number; h: number }[],
  edges: { from: string; to: string }[],
  cfg: { kind: string; columns: number; gapX: number; gapY: number; originX: number; originY: number },
): [number, number][] {
  const n = nodes.length;
  const pos: [number, number][] = nodes.map(() => [cfg.originX, cfg.originY]);
  if (n === 0) return pos;
  const packRow = (indices: number[], y0: number) => {
    const rowH = Math.max(0, ...indices.map((i) => nodes[i].h));
    let x = cfg.originX;
    indices.forEach((i, slot) => {
      pos[i] = [x, y0 + (rowH - nodes[i].h) / 2];
      x += nodes[i].w;
      if (slot + 1 !== indices.length) x += cfg.gapX;
    });
  };
  if (cfg.kind === "row") {
    packRow(nodes.map((_, i) => i), cfg.originY);
  } else if (cfg.kind === "column") {
    const colW = Math.max(0, ...nodes.map((s) => s.w));
    let y = cfg.originY;
    nodes.forEach((s, i) => {
      pos[i] = [cfg.originX + (colW - s.w) / 2, y];
      y += s.h;
      if (i + 1 !== n) y += cfg.gapY;
    });
  } else if (cfg.kind === "grid") {
    const cols = Math.max(1, cfg.columns);
    const rows = Math.ceil(n / cols);
    const colW = new Array<number>(cols).fill(0);
    const rowH = new Array<number>(rows).fill(0);
    nodes.forEach((s, i) => {
      colW[i % cols] = Math.max(colW[i % cols], s.w);
      rowH[Math.floor(i / cols)] = Math.max(rowH[Math.floor(i / cols)], s.h);
    });
    const sum = (a: number[], k: number) => a.slice(0, k).reduce((p, q) => p + q, 0);
    nodes.forEach((s, i) => {
      const c = i % cols;
      const r = Math.floor(i / cols);
      pos[i] = [cfg.originX + sum(colW, c) + cfg.gapX * c + (colW[c] - s.w) / 2, cfg.originY + sum(rowH, r) + cfg.gapY * r + (rowH[r] - s.h) / 2];
    });
  } else {
    const idx = new Map(nodes.map((s, i) => [s.id, i]));
    const adj: number[][] = nodes.map(() => []);
    for (const e of edges) {
      const a = idx.get(e.from);
      const b = idx.get(e.to);
      if (a !== undefined && b !== undefined && a !== b) adj[a].push(b);
    }
    // 灰节点上的边是回边，不进分层；树边 / 前向边 / 交叉边留下，再取最长路。
    const color = new Array<number>(n).fill(0);
    const forward: [number, number][] = [];
    const walk = (u: number) => {
      color[u] = 1;
      for (const v of adj[u]) {
        if (color[v] === 1) continue;
        forward.push([u, v]);
        if (color[v] === 0) walk(v);
      }
      color[u] = 2;
    };
    for (let i = 0; i < n; i++) if (color[i] === 0) walk(i);
    const indeg = new Array<number>(n).fill(0);
    const out: number[][] = nodes.map(() => []);
    for (const [u, v] of forward) {
      out[u].push(v);
      indeg[v] += 1;
    }
    const level = new Array<number>(n).fill(0);
    const queued = new Array<boolean>(n).fill(false);
    const queue: number[] = [];
    for (let i = 0; i < n; i++)
      if (indeg[i] === 0) {
        queue.push(i);
        queued[i] = true;
      }
    while (queue.length) {
      const u = queue.shift()!;
      for (const v of out[u]) {
        level[v] = Math.max(level[v], level[u] + 1);
        indeg[v] = Math.max(0, indeg[v] - 1);
        if (indeg[v] === 0 && !queued[v]) {
          queued[v] = true;
          queue.push(v);
        }
      }
    }
    if (queued.includes(false)) {
      const extra = Math.max(...level) + 1;
      for (let i = 0; i < n; i++) if (!queued[i]) level[i] = extra;
    }
    const perLevel: number[][] = Array.from({ length: Math.max(...level) + 1 }, () => []);
    level.forEach((l, i) => perLevel[l].push(i));
    let y = cfg.originY;
    for (const row of perLevel) {
      if (row.length === 0) continue;
      packRow(row, y);
      y += Math.max(0, ...row.map((i) => nodes[i].h)) + cfg.gapY;
    }
  }
  return pos;
}

const center = (f: Frame) => ({ x: f.x + f.w / 2, y: f.y + f.h / 2 });

/**
 * 一次调用（补全过的参数）的预览。`final` = 参数已经写完（tool-input）：数组最后一项也画。
 * `scene` 是画布上现有的元素（草稿会被跳过）。
 */
export function livePlan(args: Record<string, unknown>, final: boolean, scene: readonly El[]): LivePlan {
  const existing = scene.filter((e) => !e.isDeleted && !isLive(e));
  const frames: Frame[] = existing.filter((e) => !e.containerId).map(frameOf);
  const byId = new Map(frames.map((f) => [f.id, f]));
  const settled = <T>(items: T[]) => (final ? items : items.slice(0, -1));
  const plan: LivePlan = { skeletons: [], targets: [], pointer: null, status: "Agent 正在改白板…" };
  const point = (ids: string[]) => {
    const hit = ids.map((id) => byId.get(id)).filter((f): f is Frame => !!f);
    if (hit.length) plan.pointer = center(hit[hit.length - 1]);
  };

  // whiteboard_graph：节点照服务器排好，边从节点的边连到边。
  if (Array.isArray(args.nodes) || (args.nodes && typeof args.nodes === "object")) {
    const rawNodes = settled(list(args.nodes).map(obj)).filter((n) => str(n.id));
    const rawEdges = settled(list(args.edges).map(obj)).filter((e) => str(e.from) && str(e.to));
    const nodes = rawNodes.map((n) => ({
      id: str(n.id)!,
      kind: NODE_SHAPES.includes(str(n.type) ?? "") ? str(n.type)! : "rectangle",
      text: typeof n.text === "string" ? n.text : "",
      w: Math.max(1, num(n.width) ?? GRAPH_W),
      h: Math.max(1, num(n.height) ?? GRAPH_H),
    }));
    const b = bounds(frames);
    const layout = ["row", "column", "grid", "layered"].includes(str(args.layout) ?? "") ? str(args.layout)! : "layered";
    const columns = num(args.columns);
    const positions = graphPositions(
      nodes,
      rawEdges.map((e) => ({ from: str(e.from)!, to: str(e.to)! })),
      {
        kind: layout,
        columns: columns !== null && columns >= 1 ? Math.floor(columns) : layout === "grid" ? 3 : 1,
        gapX: num(args.gap_x) ?? GRAPH_GAP_X,
        gapY: num(args.gap_y) ?? GRAPH_GAP_Y,
        originX: b ? b.right + GAP * 2 : 100,
        originY: b ? b.top : 100,
      },
    );
    nodes.forEach((n, i) => {
      const [x, y] = positions[i];
      const s = addShape({ type: n.kind, id: n.id, x, y, width: n.w, height: n.h, ...(n.text ? { text: n.text } : {}) }, i, frames, byId);
      if (s) plan.skeletons.push(s);
    });
    rawEdges.forEach((e, i) => {
      const s = addShape({ type: "arrow", from: e.from, to: e.to, id: str(e.id) ?? `e-${str(e.from)}-${str(e.to)}#${i}`, ...(typeof e.text === "string" ? { text: e.text } : {}) }, i, frames, byId);
      if (s) plan.skeletons.push(s);
    });
    const last = plan.skeletons.filter((s) => s.type !== "arrow").at(-1);
    if (last) plan.pointer = { x: last.x + (last.width ?? 0) / 2, y: last.y + (last.height ?? 0) / 2 };
    plan.status = `Agent 正在画流程图：${nodes.length} 个节点、${rawEdges.length} 条边`;
    return plan;
  }

  // whiteboard_edit：先删、再改、再加（服务器的顺序）。
  const isEdit = ["add", "update", "delete", "clear"].some((k) => k in args);
  if (isEdit) {
    if (args.clear === true) {
      const b = bounds(frames);
      if (b) plan.pointer = { x: (b.left + b.right) / 2, y: (b.top + b.bottom) / 2 };
      frames.length = 0;
      byId.clear();
    }
    const deletes = settled(list(args.delete)).map(str).filter((id): id is string => !!id);
    const updates = settled(list(args.update).map(obj))
      .map((u) => str(u.id))
      .filter((id): id is string => !!id);
    plan.targets = [...deletes, ...updates].filter((id) => byId.has(id));
    point(plan.targets);
    settled(list(args.add).map(obj)).forEach((a, i) => {
      const s = addShape(a, i, frames, byId);
      if (!s) return;
      plan.skeletons.push(s);
      plan.pointer = { x: s.x + (s.width ?? 0) / 2, y: s.y + (s.height ?? 0) / 2 };
    });
    const parts = [
      args.clear === true ? "清空" : "",
      deletes.length ? `删 ${deletes.length} 个` : "",
      updates.length ? `改 ${updates.length} 个` : "",
      plan.skeletons.length ? `画 ${plan.skeletons.length} 个` : "",
    ].filter(Boolean);
    if (parts.length) plan.status = `Agent 正在${parts.join("、")}图形…`;
    return plan;
  }

  // 排版类（group / align / style / layer / fit_text / distribute / layout / place / duplicate）：
  // 指着要动的那几个。
  const ids = [...settled(list(args.ids)).map(str), str(args.id), str(args.ref)].filter((id): id is string => !!id && byId.has(id));
  plan.targets = [...new Set(ids)];
  point(plan.targets);
  if (plan.targets.length) plan.status = `Agent 正在调整 ${plan.targets.length} 个图形…`;
  return plan;
}
