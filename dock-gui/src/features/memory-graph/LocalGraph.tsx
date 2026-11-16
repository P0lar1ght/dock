// 抽屉里的局部图谱：选中这篇和它一两跳以内的邻居，加上它自己的相似建议。
// 点一篇就跳过去（抽屉换成那篇，大图也挪到它）；点还没写的那篇去新建。

import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronRight } from "lucide-react";
import type { MemoryGraph } from "./api";
import { GraphEngine } from "./engine";
import { setGraphSettings, type GraphSettings } from "./settings";
import { buildView, DEFAULT_FILTERS, neighbourhood, type ViewNode } from "./view";

const HEIGHT = 210;

export function LocalGraph({
  graph,
  id,
  settings,
  colors,
  onOpen,
  onCreate,
}: {
  graph: MemoryGraph;
  id: string;
  settings: GraphSettings;
  colors: Map<string, string> | null;
  onOpen: (id: string) => void;
  /** 点了还没写的那篇。 */
  onCreate: (title: string, scope: string) => void;
}) {
  const depth = settings.localDepth;
  const open = settings.localOpen;
  // 不受大图的筛选影响：这里要看这篇的全部关系。
  const view = useMemo(() => neighbourhood(buildView(graph, DEFAULT_FILTERS, { ghosts: settings.ghosts }), id, depth), [graph, id, depth, settings.ghosts]);
  const notes = view.nodes.filter((n) => !n.ghost).length;

  return (
    <section className="flex flex-col gap-1.5">
      <div className="flex items-center gap-1.5 px-2">
        <button
          type="button"
          aria-expanded={open}
          data-disclosure
          onClick={() => setGraphSettings({ localOpen: !open })}
          className="flex items-center gap-1 text-xs font-semibold text-ink-4 hover:text-ink-2"
        >
          <ChevronRight size={13} className={`transition-transform duration-150 ${open ? "rotate-90" : ""}`} />
          局部图谱
          <span className="font-medium text-ink-5">{Math.max(0, notes - 1)}</span>
        </button>
        <span className="flex-1" />
        {open && (
          <div className="flex items-center gap-0.5 rounded-md bg-[#eaeaee] p-0.5" role="group" aria-label="看几跳">
            {([1, 2] as const).map((d) => (
              <button
                key={d}
                type="button"
                aria-pressed={depth === d}
                title={d === 1 ? "只看直接相连的" : "再往外看一层"}
                onClick={() => setGraphSettings({ localDepth: d })}
                className={`motion-colors h-5 rounded px-1.5 text-[11px] ${depth === d ? "bg-white font-semibold text-ink shadow-sm" : "text-ink-3 hover:text-ink"}`}
              >
                {d} 跳
              </button>
            ))}
          </div>
        )}
      </div>
      {open &&
        (view.nodes.length <= 1 ? (
          <div className="flex h-16 items-center justify-center rounded-lg border border-line-soft bg-[#fafafa] text-[12px] text-ink-4">还没有和这篇相连的笔记</div>
        ) : (
          <LocalCanvas view={view} center={id} settings={settings} colors={colors} onOpen={onOpen} onCreate={onCreate} />
        ))}
    </section>
  );
}

function LocalCanvas({
  view,
  center,
  settings,
  colors,
  onOpen,
  onCreate,
}: {
  view: ReturnType<typeof neighbourhood>;
  center: string;
  settings: GraphSettings;
  colors: Map<string, string> | null;
  onOpen: (id: string) => void;
  onCreate: (title: string, scope: string) => void;
}) {
  const wrap = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const engine = useRef<GraphEngine | null>(null);
  const [cursor, setCursor] = useState("default");
  const latest = useRef({ onOpen, onCreate, center });
  latest.current = { onOpen, onCreate, center };

  useEffect(() => {
    if (!canvas.current || !wrap.current) return;
    const created = new GraphEngine(
      canvas.current,
      {
        onSelect: (node: ViewNode | null) => {
          if (!node || node.id === latest.current.center) return;
          if (node.ghost) latest.current.onCreate(node.label, node.scope);
          else latest.current.onOpen(node.id);
        },
        onLink: () => {},
        onEdge: () => {},
        onTip: () => {},
        onCursor: setCursor,
      },
      { local: true },
    );
    engine.current = created;
    const box = wrap.current;
    const sync = () => created.resize(box.clientWidth, box.clientHeight);
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
    engine.current?.setSettings(settings);
  }, [settings]);
  useEffect(() => {
    engine.current?.setColors(colors);
  }, [colors]);
  useEffect(() => {
    const e = engine.current;
    if (!e) return;
    e.setData(view.nodes, view.edges, new Map(), null);
    e.setSelected(center);
  }, [view, center]);
  // 换了一篇、换了深度：等新邻居落定再把整张小图对进框里。
  useEffect(() => {
    engine.current?.fitWhenSettled();
  }, [center, settings.localDepth]);

  return (
    <div ref={wrap} className="relative overflow-hidden rounded-lg border border-line-soft bg-[#fcfcfd]" style={{ height: HEIGHT }}>
      <canvas
        ref={canvas}
        role="img"
        aria-label={`局部图谱：和这篇相连的 ${view.nodes.length - 1} 篇`}
        className="size-full touch-none select-none outline-none"
        style={{ cursor }}
      />
    </div>
  );
}
