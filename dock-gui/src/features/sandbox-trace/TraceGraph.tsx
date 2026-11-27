// 链路图：会话 → 工具 → 程序 → 主机，四栏。边越粗次数越多，出错的标红；悬停 / 选中一个节点
// 高亮它上下游连着的整条链路（其余淡出），选中的还会过滤下面的时间线。
// 布局自己算（节点不多，几十个）：每栏按上游的平均位置排，少交叉；宽度跟着面板走。

import { useEffect, useMemo, useRef, useState } from "react";
import { humanBytes, type NodeKind, type TraceEdge, type TraceNode } from "./api";

const ROW = 30;
const NODE_H = 22;
const PAD_Y = 10;
/** 四栏的宽度占比：会话窄，其余平分。 */
const COL_WEIGHT = [0.13, 0.27, 0.27, 0.33];
const GAP = 22;

const columnOf = (kind: NodeKind) =>
  kind === "session" ? 0 : kind === "tool" || kind === "source" ? 1 : kind === "program" ? 2 : 3;

const HEADERS = ["会话", "工具", "程序", "目的主机"];

interface Placed extends TraceNode {
  col: number;
  x: number;
  y: number;
  w: number;
}

/** 一个字大概多宽（11.5px 字号）：中文按满宽，别的按六成五。 */
function textWidth(s: string): number {
  let w = 0;
  for (const ch of s) w += /[⺀-￿]/.test(ch) ? 11.5 : 6.7;
  return w;
}

function fit(label: string, px: number): string {
  if (textWidth(label) <= px) return label;
  let out = "";
  for (const ch of label) {
    if (textWidth(`${out}${ch}…`) > px) break;
    out += ch;
  }
  return `${out}…`;
}

function layout(nodes: TraceNode[], edges: TraceEdge[], width: number) {
  const cols: TraceNode[][] = [[], [], [], []];
  for (const n of nodes) cols[columnOf(n.kind)].push(n);
  const parents = new Map<string, string[]>();
  for (const e of edges) parents.set(e.to, [...(parents.get(e.to) ?? []), e.from]);
  const yIndex = new Map<string, number>();
  cols.forEach((col, c) => {
    if (c >= 2) {
      // 按上游的平均位置排：同一个程序的主机挨在一起，线少交叉。
      const center = (n: TraceNode) => {
        const ps = (parents.get(n.id) ?? []).map((p) => yIndex.get(p)).filter((v): v is number => v !== undefined);
        return ps.length ? ps.reduce((a, b) => a + b, 0) / ps.length : Number.MAX_SAFE_INTEGER;
      };
      col.sort((a, b) => center(a) - center(b));
    }
    col.forEach((n, i) => yIndex.set(n.id, i));
  });
  const rows = Math.max(1, ...cols.map((c) => c.length));
  const height = PAD_Y * 2 + 18 + rows * ROW;
  const usable = width - GAP * 3;
  const widths = COL_WEIGHT.map((w) => Math.max(48, usable * w));
  const xs = widths.map((_, i) => widths.slice(0, i).reduce((a, b) => a + b, 0) + GAP * i);
  const placed = new Map<string, Placed>();
  cols.forEach((col, c) => {
    // 这一栏比最高那栏短：竖着居中。
    const offset = ((rows - col.length) * ROW) / 2;
    col.forEach((n, i) => {
      placed.set(n.id, { ...n, col: c, x: xs[c], y: PAD_Y + 18 + offset + i * ROW + (ROW - NODE_H) / 2, w: widths[c] });
    });
  });
  return { placed, height, xs };
}

/** `id` 往上游、往下游连着的节点和边。 */
function connected(id: string, edges: TraceEdge[]) {
  const nodes = new Set([id]);
  const lines = new Set<number>();
  const walk = (from: "from" | "to", to: "from" | "to") => {
    const queue = [id];
    while (queue.length) {
      const cur = queue.shift()!;
      edges.forEach((e, i) => {
        if (e[from] === cur) {
          lines.add(i);
          if (!nodes.has(e[to])) {
            nodes.add(e[to]);
            queue.push(e[to]);
          }
        }
      });
    }
  };
  walk("from", "to");
  walk("to", "from");
  return { nodes, lines };
}

export function TraceGraph({
  nodes,
  edges,
  selected,
  onSelect,
}: {
  nodes: TraceNode[];
  edges: TraceEdge[];
  selected: string | null;
  onSelect: (id: string | null) => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(560);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setWidth(Math.max(320, Math.floor(entry.contentRect.width))));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const [hover, setHover] = useState<string | null>(null);
  const { placed, height, xs } = useMemo(() => layout(nodes, edges, width), [nodes, edges, width]);
  const focus = hover ?? selected;
  const lit = useMemo(() => (focus ? connected(focus, edges) : null), [focus, edges]);
  const maxN = Math.max(1, ...edges.map((e) => e.n));

  return (
    <div ref={box} className="w-full">
      <svg width={width} height={height} className="block select-none" role="img" aria-label="行为链路图">
        {HEADERS.map((h, i) => (
          <text key={h} x={xs[i] + 2} y={PAD_Y + 8} className="fill-ink-4" fontSize={10.5} fontWeight={500}>
            {h}
          </text>
        ))}
        {edges.map((e, i) => {
          const a = placed.get(e.from);
          const b = placed.get(e.to);
          if (!a || !b) return null;
          const x1 = a.x + a.w;
          const y1 = a.y + NODE_H / 2;
          const x2 = b.x;
          const y2 = b.y + NODE_H / 2;
          const mid = (x1 + x2) / 2;
          const on = !lit || lit.lines.has(i);
          const stroke = e.errors > 0 ? "#ff6b5e" : on && lit ? "var(--color-accent)" : "#c7c7cc";
          const strokeWidth = 1 + (3.5 * Math.log(e.n + 1)) / Math.log(maxN + 1);
          return (
            <path
              key={i}
              d={`M${x1},${y1} C${mid},${y1} ${mid},${y2} ${x2},${y2}`}
              fill="none"
              stroke={stroke}
              strokeWidth={strokeWidth}
              strokeLinecap="round"
              style={{ opacity: on ? 0.9 : 0.15, transition: "opacity 160ms ease, stroke 160ms ease" }}
            >
              <title>{`${e.n} 次${e.bytes ? ` · ${humanBytes(e.bytes)}` : ""}${e.errors ? ` · 出错 ${e.errors}` : ""}`}</title>
            </path>
          );
        })}
        {[...placed.values()].map((n) => {
          const on = !lit || lit.nodes.has(n.id);
          const isSel = selected === n.id;
          const count = n.kind === "host" || n.kind === "others" ? (n.bytes ? humanBytes(n.bytes) : `${n.n}`) : n.n ? `${n.n}` : "";
          const countW = count ? textWidth(count) * 0.9 + 8 : 0;
          const labelW = n.w - 14 - countW - (n.errors ? 14 : 0);
          return (
            <g
              key={n.id}
              transform={`translate(${n.x},${n.y})`}
              className="cursor-pointer"
              style={{ opacity: on ? 1 : 0.3, transition: "opacity 160ms ease" }}
              onMouseEnter={() => setHover(n.id)}
              onMouseLeave={() => setHover(null)}
              onClick={() => onSelect(isSel || n.kind === "session" ? null : n.id)}
            >
              <title>{`${n.label}\n${n.n} 次${n.bytes ? ` · ${humanBytes(n.bytes)}` : ""}${n.errors ? ` · 出错 ${n.errors}` : ""}`}</title>
              <rect
                width={n.w}
                height={NODE_H}
                rx={n.kind === "host" || n.kind === "others" ? NODE_H / 2 : 6}
                fill={isSel ? "#eaf3ff" : n.kind === "session" ? "#f0f0f2" : "#ffffff"}
                stroke={isSel ? "var(--color-accent)" : n.errors ? "#ffb3ab" : "#e5e5ea"}
                strokeWidth={isSel ? 1.5 : 1}
                style={{ transition: "fill 160ms ease, stroke 160ms ease" }}
              />
              <text x={8} y={NODE_H / 2 + 4} fontSize={11.5} className={n.kind === "others" || n.kind === "source" ? "fill-ink-4" : "fill-ink-2"}>
                {fit(n.label, Math.max(20, labelW))}
              </text>
              {n.errors > 0 && <circle cx={n.w - countW - 9} cy={NODE_H / 2} r={3.5} fill="#ff3b30" />}
              {count && (
                <text x={n.w - 6} y={NODE_H / 2 + 3.5} fontSize={10} textAnchor="end" className="fill-ink-4">
                  {count}
                </text>
              )}
            </g>
          );
        })}
      </svg>
    </div>
  );
}
