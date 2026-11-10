// 侧栏搜索：标题（本地，改名后立刻准）+ 用户消息（Dock 的 thread/search），结果按
// 预设分组，第二行是命中处的片段（只命中标题时写项目目录名），查询词高亮。
// 设计：wonder-ui/shell/SessionSearch（cipo / gein）。

import { useEffect, useRef, useState, type ReactNode } from "react";
import { Search, SearchX, X } from "lucide-react";
import type { PresetGroup } from "../model";
import { store } from "../dock/store";
import { imeComposing } from "../dock/ime";

/** 停手这么久才去问 Dock。 */
const DEBOUNCE_MS = 200;

export function SessionSearch({
  groups,
  activeSessionId,
  onSelect,
  onClose,
}: {
  groups: PresetGroup[];
  activeSessionId: string | null;
  onSelect: (sessionId: string, split?: boolean) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);

  const q = query.trim().toLowerCase();
  const terms = q.split(/\s+/).filter(Boolean);

  // Dock 那边的内容命中；对应的是哪次查询，免得旧结果套到新查询上。
  const [remote, setRemote] = useState<{ q: string; hits: Map<string, string | null> } | null>(null);
  useEffect(() => {
    if (!q) return;
    const t = window.setTimeout(() => {
      store
        .search(q)
        .then((hits) => setRemote({ q, hits }))
        .catch(() => setRemote({ q, hits: new Map() }));
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(t);
  }, [q]);
  const content = remote?.q === q ? remote.hits : null;

  const results = groups
    .map((g) => ({
      group: g,
      hits: g.projects.flatMap((p) =>
        p.sessions
          .filter((s) => q && (terms.every((w) => s.title.toLowerCase().includes(w)) || content?.has(s.id)))
          .map((s) => ({ session: s, project: p.name, snippet: content?.get(s.id) ?? null })),
      ),
    }))
    .filter((r) => r.hits.length > 0);
  const first = results[0]?.hits[0]?.session.id;

  return (
    <div className="enter-fade flex min-h-0 flex-1 flex-col gap-2.5">
      <div
        className={`motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-md border bg-white px-2 shadow-sm ${
          query ? "border-accent" : "border-line-strong"
        }`}
      >
        <Search size={13} className="shrink-0 text-ink-4" />
        <input
          ref={input}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (imeComposing(e)) return;
            if (e.key === "Escape") onClose();
            if (e.key === "Enter" && first) onSelect(first);
          }}
          placeholder="搜索标题和消息"
          className="min-w-0 flex-1 bg-transparent text-xs font-medium text-ink outline-none select-text placeholder:font-normal placeholder:text-ink-4"
        />
        {query && (
          <button
            type="button"
            title="清空"
            onClick={() => {
              setQuery("");
              input.current?.focus();
            }}
            className="pop-in flex size-4 shrink-0 items-center justify-center rounded-full bg-[#e8e8ec] text-ink-3"
          >
            <X size={10} />
          </button>
        )}
      </div>

      {q && results.length === 0 && content && (
        <div className="enter-fade flex flex-col items-center gap-1.5 py-6">
          <SearchX size={20} className="text-ink-5" />
          <span className="mt-1 text-xs font-medium text-ink-4">未找到相关会话</span>
          <span className="text-[11px] text-ink-5">换个关键词试试</span>
        </div>
      )}

      <div className="thin-scroll flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">
        {results.map(({ group, hits }) => (
          <div key={group.id} className="enter-fade flex flex-col gap-1">
            <div className="px-1 text-[10px] font-semibold text-ink-4">
              {group.label} ({hits.length})
            </div>
            {hits.map(({ session, project, snippet }) => (
              <button
                key={session.id}
                type="button"
                onClick={(e) => onSelect(session.id, e.metaKey)}
                className={`motion-colors flex flex-col gap-0.5 rounded-md p-1.5 text-left ${
                  session.id === activeSessionId ? "bg-[#e8e8ed]" : "hover:bg-hover"
                }`}
              >
                <span className="truncate text-xs font-medium text-ink">
                  <Highlight text={session.title} terms={terms} />
                </span>
                <span className="truncate text-[10px] text-ink-3">
                  {snippet ? <Highlight text={snippet} terms={terms} small /> : project}
                </span>
              </button>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

/** 把每个查询词（不分大小写）标出来。 */
function Highlight({ text, terms, small = false }: { text: string; terms: string[]; small?: boolean }) {
  const lower = text.toLowerCase();
  const marks: [number, number][] = [];
  for (const w of terms) {
    for (let at = lower.indexOf(w); at >= 0 && w; at = lower.indexOf(w, at + w.length)) marks.push([at, at + w.length]);
  }
  marks.sort((a, b) => a[0] - b[0]);
  const out: ReactNode[] = [];
  let pos = 0;
  for (const [a, b] of marks) {
    if (a < pos) continue;
    out.push(text.slice(pos, a));
    out.push(
      <mark
        key={a}
        className={`rounded-sm bg-[#0a84ff14] text-accent ${small ? "px-px font-medium" : "px-0.5 font-semibold"}`}
      >
        {text.slice(a, b)}
      </mark>,
    );
    pos = b;
  }
  out.push(text.slice(pos));
  return <>{out}</>;
}
