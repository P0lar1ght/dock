// 列表视图：同一批笔记按范围分组列出来（图的「表格版」——逐篇扫、按时间找、读屏都靠它）。

import { useMemo } from "react";
import { ArrowDownLeft, ArrowUpRight } from "lucide-react";
import { ago, fullTime } from "../../components/navPage";
import { scopeName, type MemoryGraph, type MemoryNote } from "./api";
import { GROUP_COLOR, groupOf, linkCounts } from "./view";

export function NoteList({
  graph,
  notes,
  selectedId,
  snippets,
  insetRight,
  onSelect,
}: {
  graph: MemoryGraph;
  /** 已经按类别和搜索筛过的笔记。 */
  notes: MemoryNote[];
  selectedId: string | null;
  /** 搜索命中的那一行（按内容搜到的才有）。 */
  snippets: Map<string, string> | null;
  /** 右边被抽屉盖住的宽度：列表让开它，右边几列不被挡住。 */
  insetRight: number;
  onSelect: (id: string) => void;
}) {
  const counts = useMemo(() => linkCounts(graph), [graph]);
  const groups = useMemo(() => {
    const byScope = new Map<string, MemoryNote[]>();
    for (const n of notes) byScope.set(n.scope, [...(byScope.get(n.scope) ?? []), n]);
    // 当前项目在前，然后全局，其余按名字。
    const rank = (id: string) => (id === graph.currentScope ? 0 : id === "global" ? 1 : 2);
    return [...byScope.entries()]
      .sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b))
      .map(([id, rows]) => ({
        id,
        scope: graph.scopes.find((s) => s.id === id),
        rows: rows.sort((a, b) => (b.modifiedMs ?? 0) - (a.modifiedMs ?? 0)),
      }));
  }, [notes, graph.currentScope, graph.scopes]);

  if (notes.length === 0) {
    return <div className="flex flex-1 items-center justify-center text-[12.5px] text-ink-4">没有符合条件的笔记</div>;
  }
  return (
    <div
      className="thin-scroll absolute bottom-0 left-0 top-0 flex flex-col gap-5 overflow-y-auto px-8 py-5 transition-[right] duration-200 ease-out"
      style={{ right: insetRight }}
    >
      {groups.map((g) => (
        <section key={g.id} className="flex max-w-[880px] flex-col gap-1">
          <div className="flex items-center gap-2 px-2">
            <span className="size-2.5 rounded-full" style={{ background: GROUP_COLOR[groupOf(graph, g.id)] }} />
            <span className="text-[13px] font-semibold text-ink">{g.scope ? scopeName(g.scope) : g.id}</span>
            {g.scope?.current && <span className="text-xs text-ink-4">当前项目</span>}
            <span className="text-xs font-medium text-ink-5">{g.rows.length}</span>
          </div>
          {g.rows.map((n) => {
            const c = counts.get(n.id);
            const color = GROUP_COLOR[groupOf(graph, n.scope)];
            const sub = snippets?.get(n.id) ?? (n.summary !== n.title ? n.summary : "");
            return (
              <button
                key={n.id}
                type="button"
                onClick={() => onSelect(n.id)}
                aria-current={n.id === selectedId ? "true" : undefined}
                className={`motion-colors flex min-h-11 items-center gap-3 rounded-lg px-2 py-1.5 text-left ${n.id === selectedId ? "bg-[#0a84ff14]" : "hover:bg-hover"}`}
              >
                <span
                  title={n.kind === "topic" ? "主题" : "观察"}
                  className={`size-2.5 shrink-0 rounded-full ${n.kind === "observation" ? "border-2" : ""}`}
                  style={n.kind === "observation" ? { borderColor: color } : { background: color }}
                />
                <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                  <span className="truncate text-[13px] font-medium text-ink">{n.title}</span>
                  {sub && <span className="truncate text-xs text-ink-4">{sub}</span>}
                </span>
                <span className="w-10 shrink-0 text-xs text-ink-4">{n.kind === "topic" ? "主题" : "观察"}</span>
                <span className="flex w-[72px] shrink-0 items-center gap-2 text-xs tabular-nums text-ink-4">
                  <span className="flex items-center gap-0.5" title={`链接到 ${c?.out ?? 0} 篇`}>
                    <ArrowUpRight size={11} />
                    {c?.out ?? 0}
                  </span>
                  <span className="flex items-center gap-0.5" title={`被 ${c?.in ?? 0} 篇引用`}>
                    <ArrowDownLeft size={11} />
                    {c?.in ?? 0}
                  </span>
                </span>
                <span className="w-[72px] shrink-0 text-right text-xs text-ink-4" title={n.modifiedMs ? fullTime(n.modifiedMs) : undefined}>
                  {n.modifiedMs ? ago(n.modifiedMs) : ""}
                </span>
              </button>
            );
          })}
        </section>
      ))}
    </div>
  );
}
