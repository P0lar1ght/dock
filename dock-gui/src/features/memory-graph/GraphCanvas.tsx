// 图谱画布的 React 外壳：挂引擎、跟着容器变大小、画悬停提示 / 图例 / 缩放按钮 / 设置面板。
// 画点连线、布局、指针交互都在 engine.ts。

import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { Maximize, Minus, Plus, Settings2 } from "lucide-react";
import { GraphEngine, type GraphTip } from "./engine";
import { GraphSettingsPanel } from "./GraphSettingsPanel";
import type { GraphSettings } from "./settings";
import { GROUP_COLOR, GROUP_LABEL, type NodeGroup, type ViewEdge, type ViewNode } from "./view";

export interface GraphHandle {
  /** 把一篇笔记挪到可见区域中间。 */
  focus: (id: string) => void;
  fit: () => void;
}

/** 按条件着色的一组（图例用）。 */
export interface LegendGroup {
  query: string;
  color: string;
}

export interface GraphCanvasProps {
  nodes: ViewNode[];
  edges: ViewEdge[];
  /** scope id → 项目名：同时显示三个以上 scope 时标在各自那一簇上。 */
  scopeLabels: Map<string, string>;
  currentScope: string | null;
  selectedId: string | null;
  /** 搜索命中的点（其余变淡）；没在搜就是 null。 */
  highlight: Set<string> | null;
  settings: GraphSettings;
  /** 按条件着色：id → 颜色；没在着色是 null。 */
  colors: Map<string, string> | null;
  legendGroups: LegendGroup[];
  /** 每组命中几篇（设置面板里显示）。 */
  groupHits: (number | null)[];
  /** 右边被抽屉盖住的宽度。 */
  insetRight: number;
  onSelect: (node: ViewNode | null) => void;
  onLink: (from: ViewNode, to: ViewNode) => void;
  onEdge: (edge: ViewEdge, source: ViewNode, target: ViewNode, at: { x: number; y: number }) => void;
}

export const GraphCanvas = forwardRef<GraphHandle, GraphCanvasProps>(function GraphCanvas(props, ref) {
  const wrap = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const engine = useRef<GraphEngine | null>(null);
  const [tip, setTip] = useState<GraphTip | null>(null);
  const [cursor, setCursor] = useState("default");
  const [width, setWidth] = useState(0);
  const [panel, setPanel] = useState(false);
  // 回调每次渲染都是新的：引擎拿一个不变的壳，里面现取最新的。
  const latest = useRef(props);
  latest.current = props;

  useEffect(() => {
    if (!canvas.current || !wrap.current) return;
    const created = new GraphEngine(canvas.current, {
      onSelect: (node) => latest.current.onSelect(node),
      onLink: (from, to) => latest.current.onLink(from, to),
      onEdge: (edge, source, target, at) => latest.current.onEdge(edge, source, target, at),
      onTip: setTip,
      onCursor: setCursor,
    });
    created.setSettings(latest.current.settings);
    engine.current = created;
    const box = wrap.current;
    const sync = () => {
      created.resize(box.clientWidth, box.clientHeight);
      setWidth(box.clientWidth);
    };
    sync();
    const observer = new ResizeObserver(sync);
    observer.observe(box);
    return () => {
      observer.disconnect();
      created.destroy();
      engine.current = null;
    };
  }, []);

  useEffect(() => {
    engine.current?.setInsetRight(props.insetRight);
  }, [props.insetRight]);
  useEffect(() => {
    engine.current?.setSettings(props.settings);
  }, [props.settings]);
  useEffect(() => {
    engine.current?.setData(props.nodes, props.edges, props.scopeLabels, props.currentScope);
  }, [props.nodes, props.edges, props.scopeLabels, props.currentScope]);
  useEffect(() => {
    engine.current?.setSelected(props.selectedId);
  }, [props.selectedId]);
  useEffect(() => {
    engine.current?.setHighlight(props.highlight);
  }, [props.highlight]);
  useEffect(() => {
    engine.current?.setColors(props.colors);
  }, [props.colors]);

  useImperativeHandle(ref, () => ({
    focus: (id) => engine.current?.focus(id),
    fit: () => engine.current?.fit(),
  }));

  const groups = [...new Set(props.nodes.map((n) => n.group))];
  const has = {
    topics: props.nodes.some((n) => !n.ghost && !n.hollow),
    observations: props.nodes.some((n) => !n.ghost && n.hollow),
    ghosts: props.nodes.some((n) => n.ghost),
    links: props.edges.some((e) => e.kind === "link"),
    similar: props.edges.some((e) => e.kind === "similar"),
  };
  const right = props.insetRight + 12;

  return (
    <div ref={wrap} className="absolute inset-0 overflow-hidden">
      <canvas
        ref={canvas}
        role="img"
        aria-label={`记忆关系图：${props.nodes.filter((n) => !n.ghost).length} 篇笔记，${props.edges.filter((e) => e.kind === "link").length} 条链接。逐篇查看请切到列表视图。`}
        className="size-full touch-none select-none outline-none"
        style={{ cursor }}
      />
      {tip && <Tip tip={tip} flip={tip.x > width - props.insetRight - 300} />}
      <Legend
        groups={groups}
        colored={props.colors ? props.legendGroups : null}
        has={has}
        allSuggestions={props.settings.allSuggestions}
        canLink={props.nodes.filter((n) => n.note?.writable).length > 1}
      />
      {panel ? (
        <GraphSettingsPanel settings={props.settings} hits={props.groupHits} style={{ top: 12, right }} onClose={() => setPanel(false)} />
      ) : (
        <button
          type="button"
          title="图谱设置"
          aria-label="图谱设置"
          onClick={() => setPanel(true)}
          className="motion-colors absolute top-3 flex size-8 items-center justify-center rounded-lg border border-line-strong bg-white text-ink-3 shadow-sm transition-[right] duration-200 ease-out hover:bg-hover hover:text-ink"
          style={{ right }}
        >
          <Settings2 size={14} />
        </button>
      )}
      <div
        className="absolute bottom-4 flex flex-col overflow-hidden rounded-lg border border-line-strong bg-white shadow-sm transition-[right] duration-200 ease-out"
        style={{ right: props.insetRight + 16 }}
      >
        <ZoomButton label="放大" onClick={() => engine.current?.zoomBy(1.3)}>
          <Plus size={14} />
        </ZoomButton>
        <ZoomButton label="缩小" onClick={() => engine.current?.zoomBy(1 / 1.3)}>
          <Minus size={14} />
        </ZoomButton>
        <ZoomButton label="适应窗口（双击空白处）" onClick={() => engine.current?.fit()}>
          <Maximize size={13} />
        </ZoomButton>
      </div>
    </div>
  );
});

function ZoomButton({ label, onClick, children }: { label: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className="motion-colors flex size-8 items-center justify-center text-ink-3 hover:bg-hover hover:text-ink [&:not(:last-child)]:border-b [&:not(:last-child)]:border-line-soft"
    >
      {children}
    </button>
  );
}

/** 悬停提示：标题在前，范围 / 类别在后。文字都是数据，走 React 的文本节点。 */
function Tip({ tip, flip }: { tip: GraphTip; /** 贴着右边：翻到左侧，免得伸到抽屉底下。 */ flip: boolean }) {
  const top = Math.max(8, tip.y);
  const style = flip ? { right: `calc(100% - ${tip.left}px)`, top } : { left: tip.x, top };
  if (tip.node) {
    const n = tip.node;
    return (
      <div className="glass-popover pointer-events-none absolute z-10 flex max-w-[280px] flex-col gap-0.5 rounded-lg border border-line-strong bg-white/95 px-2.5 py-2 shadow-[0px_6px_20px_rgba(0,0,0,0.10)]" style={style}>
        <span className="text-[12.5px] font-semibold leading-snug text-ink">{n.label}</span>
        {n.ghost ? (
          <span className="text-[11.5px] text-ink-4">还没有这篇笔记 · 点击新建</span>
        ) : (
          <>
            <span className="text-[11.5px] text-ink-4">
              {n.note?.kind === "topic" ? "主题" : "观察"} · {n.degree} 条链接
            </span>
            {n.note?.summary && n.note.summary !== n.label && (
              <span className="line-clamp-2 text-[11.5px] leading-snug text-ink-3">{n.note.summary}</span>
            )}
          </>
        )}
      </div>
    );
  }
  if (!tip.edge) return null;
  const { view, source, target } = tip.edge;
  return (
    <div className="glass-popover pointer-events-none absolute z-10 flex max-w-[300px] flex-col gap-0.5 rounded-lg border border-line-strong bg-white/95 px-2.5 py-2 shadow-[0px_6px_20px_rgba(0,0,0,0.10)]" style={style}>
      <span className="text-[12.5px] font-semibold leading-snug text-ink">
        {view.kind === "similar" ? `内容相近 ${Math.round(view.score * 100)}%` : view.kind === "ghost" ? "指向还没有的笔记" : view.mutual ? "互相链接" : "链接"}
      </span>
      <span className="text-[11.5px] leading-snug text-ink-3">
        {source.label} {view.kind === "similar" || view.mutual ? "↔" : "→"} {target.label}
      </span>
      <span className="text-[11.5px] text-ink-4">{view.kind === "similar" ? "点击建立链接" : "点击查看"}</span>
    </div>
  );
}

/**
 * 图例：颜色 = 笔记属于哪儿（着色时是哪一组），实心 / 空心 = 主题 / 观察，实线 / 虚线 = 链接 / 相似建议。
 */
function Legend({
  groups,
  colored,
  has,
  allSuggestions,
  canLink,
}: {
  groups: NodeGroup[];
  /** 按条件着色的那几组；没在着色是 null。 */
  colored: LegendGroup[] | null;
  has: { topics: boolean; observations: boolean; ghosts: boolean; links: boolean; similar: boolean };
  allSuggestions: boolean;
  /** 图上有两篇以上能改的笔记：提示怎么拖线建链。 */
  canLink: boolean;
}) {
  const shown = (["current", "global", "other"] as const).filter((g) => groups.includes(g));
  if (shown.length === 0) return null;
  return (
    <div className="pointer-events-none absolute bottom-4 left-4 flex max-w-[360px] flex-col gap-1.5 rounded-lg border border-line-soft bg-white/90 px-3 py-2.5 text-[11.5px] text-ink-3 shadow-sm">
      {colored ? (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          {colored.map((g) => (
            <span key={g.color} className="flex min-w-0 items-center gap-1.5">
              <span className="size-2.5 shrink-0 rounded-full" style={{ background: g.color }} />
              <span className="max-w-[120px] truncate">{g.query}</span>
            </span>
          ))}
          <span className="flex items-center gap-1.5">
            <span className="size-2.5 rounded-full bg-[#a1a1a6]" />
            其他
          </span>
        </div>
      ) : (
        // 只有一种颜色时不用说颜色代表什么：页头已经写了范围。
        shown.length > 1 && (
          <div className="flex items-center gap-3">
            {shown.map((g) => (
              <span key={g} className="flex items-center gap-1.5">
                <span className="size-2.5 rounded-full" style={{ background: GROUP_COLOR[g] }} />
                {GROUP_LABEL[g]}
              </span>
            ))}
          </div>
        )
      )}
      <div className="flex items-center gap-3">
        {has.topics && (
          <span className="flex items-center gap-1.5">
            <span className="size-2.5 rounded-full bg-ink-3" />
            主题
          </span>
        )}
        {has.observations && (
          <span className="flex items-center gap-1.5">
            <span className="size-2.5 rounded-full border-2 border-ink-3" />
            观察
          </span>
        )}
        {has.ghosts && (
          <span className="flex items-center gap-1.5">
            <span className="size-2.5 rounded-full border border-dashed border-ink-4" />
            还没写
          </span>
        )}
      </div>
      {(has.links || has.similar) && (
        <div className="flex items-center gap-3">
          {has.links && (
            <span className="flex items-center gap-1.5">
              <span className="h-px w-4 bg-ink-3" />
              链接
            </span>
          )}
          {has.similar && (
            <span className="flex items-center gap-1.5">
              <span className="w-4 border-t border-dashed border-ink-4" />
              {allSuggestions ? "相似建议" : "相似建议（悬停 / 选中时）"}
            </span>
          )}
        </div>
      )}
      {canLink && <div className="border-t border-line-soft pt-1.5 text-[11px] text-ink-4">按住 ⌥ 从一篇拖到另一篇：建立链接</div>}
    </div>
  );
}
