// 工具配置（设计 tway / tamp）：「全部工具 / 自选 / 不用工具」三态；自选时左边按类分组的目录
// （勾选、每组全选 / 清空、搜索），右边已选清单。`null` = 全部已注册工具，`[]` = 不用工具。

import { useMemo, useState } from "react";
import { Check, ChevronRight, Info, SearchX, Search, X } from "lucide-react";
import type { CatalogTool } from "../../model";
import { groupOf, groupTools, toolSummary } from "./toolGroups";
import { Segmented } from "./ui";
import { modeOf, type Mode } from "./util";


export function ToolPicker({
  value,
  onChange,
  catalog,
  onOpenMcp,
  compact = false,
  modeSwitch = true,
  picking = false,
  onPicking = () => {},
  reasons,
}: {
  value: string[] | null;
  onChange: (tools: string[] | null) => void;
  catalog: CatalogTool[] | null;
  onOpenMcp: () => void;
  /** AI 挑这个工具的理由（AI 起草的草稿），画在行里。 */
  reasons?: Record<string, string>;
  /** 子代理弹窗里用：去掉页脚说明。 */
  compact?: boolean;
  /** 三态切换画在这里；分区自己画在标题行时传 false。 */
  modeSwitch?: boolean;
  /** 见 {@link modeOf}；三态切换和列表要共用同一份。 */
  picking?: boolean;
  onPicking?: (on: boolean) => void;
}) {
  const [query, setQuery] = useState("");
  const mode = modeOf(value, picking);
  const selected = value ?? [];
  const localized = useMemo(
    () => catalog && catalog.map((t) => ({ ...t, summary: toolSummary(t.name, t.summary) })),
    [catalog],
  );
  const known = useMemo(() => new Set((catalog ?? []).map((t) => t.name)), [catalog]);
  // 名单里有、目录里没有的：未挂载（TUI 画布同款），照样列出来、可以去掉。
  const unmounted: CatalogTool[] = selected
    .filter((n) => catalog && !known.has(n))
    .map((name) => ({ name, summary: "当前进程没有挂这个工具", kind: "resident" }));

  const q = query.trim().toLowerCase();
  const visible = (localized ?? []).filter(
    (t) => !q || t.name.toLowerCase().includes(q) || t.summary.toLowerCase().includes(q),
  );
  const groups = groupTools(visible);
  const summaryOf = (name: string) => localized?.find((t) => t.name === name)?.summary ?? "当前进程没有挂这个工具";

  const toggle = (name: string) =>
    onChange(selected.includes(name) ? selected.filter((n) => n !== name) : [...selected, name]);
  const setGroup = (names: string[], on: boolean) =>
    onChange(on ? [...selected, ...names.filter((n) => !selected.includes(n))] : selected.filter((n) => !names.includes(n)));

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4">
      {modeSwitch && (
        <div className="flex shrink-0 justify-end">
          <ToolModeSwitch value={value} onChange={onChange} catalog={catalog} picking={picking} onPicking={onPicking} />
        </div>
      )}

      {mode !== "pick" ? (
        <div className="enter-fade flex flex-col gap-2 rounded-xl border border-line-strong bg-[#fafafc] p-5">
          <span className="text-[13px] font-semibold text-ink">
            {mode === "all" ? "使用全部已注册工具" : "不给这个 Agent 任何工具"}
          </span>
          <span className="text-xs leading-relaxed text-ink-4">
            {mode === "all"
              ? `当前 ${catalog?.length ?? "…"} 个工具都可用；以后新挂上的工具也会自动可用。想收窄就切到「自选工具」。`
              : "它只能对话，不能读写文件、跑命令或上网。已开启的 MCP 工具和运行中动态插件的工具不受这个限制。"}
          </span>
        </div>
      ) : (
        <div className="enter-fade flex min-h-0 flex-1 gap-4">
          <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-xl border border-line-strong bg-[#fafafc] shadow-[0px_1px_0px_rgba(0,0,0,0.05)]">
            <label className="flex h-11 shrink-0 items-center gap-2 border-b border-line bg-white px-3.5">
              <Search size={16} className="shrink-0 text-ink-4" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="搜索工具名称或描述..."
                className="min-w-0 flex-1 bg-transparent text-[12.5px] text-ink outline-none select-text placeholder:text-ink-4"
              />
              {query && (
                <button type="button" onClick={() => setQuery("")} title="清空搜索" className="rounded-full bg-fill p-0.5 text-ink-4 hover:text-ink">
                  <X size={11} />
                </button>
              )}
              <span className="shrink-0 font-mono text-[11px] text-ink-4">
                {q ? `${visible.length} 个匹配` : `共 ${catalog?.length ?? 0} 个可用`}
              </span>
            </label>
            <div className="thin-scroll flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-3">
              {catalog === null && <span className="px-1 text-[12.5px] text-ink-4">正在读取工具目录…</span>}
              {catalog && visible.length === 0 && q && (
                <div className="enter-fade flex flex-1 flex-col items-center justify-center gap-3 px-8 py-10 text-center">
                  <span className="flex size-10 items-center justify-center rounded-xl bg-fill text-ink-3">
                    <SearchX size={18} />
                  </span>
                  <span className="text-[13.5px] font-semibold text-ink">未找到与 “{query.trim()}” 相关的工具</span>
                  <span className="max-w-[320px] text-xs leading-relaxed text-ink-4">
                    内置工具里没有匹配项。MCP 工具不在这里选：开启的 MCP 服务的工具总是可用。
                  </span>
                  <button
                    type="button"
                    onClick={() => setQuery("")}
                    className="motion-colors mt-1 h-8 rounded-lg border border-line-strong bg-white px-3 text-xs font-medium text-ink shadow-sm hover:bg-hover"
                  >
                    清空搜索条件
                  </button>
                  <button type="button" onClick={onOpenMcp} className="flex items-center gap-0.5 text-xs text-accent hover:underline">
                    前往设置配置 MCP 服务 <ChevronRight size={12} />
                  </button>
                </div>
              )}
              {groups.map(({ group, tools }) => {
                const names = tools.map((t) => t.name);
                return (
                  <div key={group.id} className="flex flex-col gap-1.5">
                    <div className="flex items-center justify-between px-1">
                      <span className="text-[11px] font-semibold tracking-[0.2px] text-ink-4">{group.label}</span>
                      <span className="flex gap-2 text-[11px]">
                        <button type="button" onClick={() => setGroup(names, true)} className="text-accent hover:underline">
                          全选
                        </button>
                        <span className="text-ink-5">·</span>
                        <button type="button" onClick={() => setGroup(names, false)} className="text-ink-4 hover:text-ink">
                          清空
                        </button>
                      </span>
                    </div>
                    {tools.map((t) => (
                      <ToolRow
                        key={t.name}
                        tool={t}
                        tag={group.tag}
                        on={selected.includes(t.name)}
                        reason={reasons?.[t.name]}
                        onToggle={() => toggle(t.name)}
                      />
                    ))}
                  </div>
                );
              })}
              {unmounted.length > 0 && !q && (
                <div className="flex flex-col gap-1.5">
                  <span className="px-1 text-[11px] font-semibold tracking-[0.2px] text-ink-4">名单里但没挂上</span>
                  {unmounted.map((t) => (
                    <ToolRow key={t.name} tool={t} tag="" on unmounted onToggle={() => toggle(t.name)} />
                  ))}
                </div>
              )}
            </div>
          </div>

          <div className="flex w-[300px] min-h-0 shrink-0 flex-col overflow-hidden rounded-xl border border-line-strong bg-white shadow-[0px_1px_0px_rgba(0,0,0,0.05)]">
            <div className="flex h-11 shrink-0 items-center justify-between border-b border-line bg-[#fafafc] px-3.5">
              <span className="flex items-center gap-1.5">
                <span className="text-[13px] font-semibold text-ink">已选工具清单</span>
                <span key={selected.length} className="pop-in rounded-full bg-[#0a84ff1a] px-2 py-0.5 text-[11px] font-semibold text-accent">
                  {selected.length}
                </span>
              </span>
              <button type="button" onClick={() => onChange([])} className="text-[11.5px] text-[#ff3b30] hover:underline">
                清空全部
              </button>
            </div>
            <div className="thin-scroll flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto p-2.5">
              {selected.length === 0 && (
                <span className="px-1 py-2 text-[11.5px] text-ink-4">还没选工具。一个都不选等于「不用工具」。</span>
              )}
              {selected.map((name) => (
                <div
                  key={name}
                  className="enter-fade flex items-center justify-between gap-2 rounded-lg border border-line-strong bg-[#fbfbfb] px-2.5 py-2"
                >
                  <span className="flex min-w-0 flex-col">
                    <span className={`truncate font-mono text-xs font-medium ${known.has(name) ? "text-ink" : "text-ink-4"}`}>{name}</span>
                    <span className="truncate text-[11px] text-ink-4">{reasons?.[name] ? `AI：${reasons[name]}` : summaryOf(name)}</span>
                  </span>
                  <button
                    type="button"
                    onClick={() => toggle(name)}
                    title="移除"
                    className="motion-colors flex size-5 shrink-0 items-center justify-center rounded text-ink-4 hover:bg-hover hover:text-ink"
                  >
                    <X size={13} />
                  </button>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}

      {!compact && (
        <div className="flex h-8 shrink-0 items-center justify-between border-t border-line-soft pt-2 text-[11.5px] text-ink-4">
          <span className="flex items-center gap-1.5">
            <Info size={14} className="text-accent" />
            已开启的 MCP 工具和运行中动态插件的工具总是可用。
          </span>
          <button type="button" onClick={onOpenMcp} className="text-accent hover:underline">
            管理外部 MCP 服务 →
          </button>
        </div>
      )}
    </div>
  );
}

/** 「全部工具 / 自选 / 不用工具」三态切换。 */
export function ToolModeSwitch({
  value,
  onChange,
  catalog,
  picking = false,
  onPicking = () => {},
}: {
  value: string[] | null;
  onChange: (tools: string[] | null) => void;
  catalog: CatalogTool[] | null;
  picking?: boolean;
  onPicking?: (on: boolean) => void;
}) {
  const mode = modeOf(value, picking);
  const setMode = (m: Mode) => {
    if (m === mode) return;
    onPicking(m === "pick");
    // 从「全部」切到自选：先把整份目录勾上（同 TUI：从全部里去掉一项 = 快照目录）；
    // 从「不用工具」切过来从空开始勾。
    onChange(m === "all" ? null : m === "none" ? [] : value === null ? (catalog ?? []).map((t) => t.name) : []);
  };
  return (
    <Segmented
      value={mode}
      onChange={setMode}
      options={[
        { id: "all", label: "全部工具" },
        { id: "pick", label: mode === "pick" ? `自选工具 (${(value ?? []).length})` : "自选工具" },
        { id: "none", label: "不用工具" },
      ]}
    />
  );
}

function ToolRow({
  tool,
  tag,
  on,
  unmounted = false,
  reason,
  onToggle,
}: {
  tool: CatalogTool;
  tag: string;
  on: boolean;
  unmounted?: boolean;
  reason?: string;
  onToggle: () => void;
}) {
  const label = tool.kind === "deferred" ? "按需" : tool.kind === "dynamic" ? "动态" : tag || groupOf(tool.name).tag;
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={on}
      onClick={onToggle}
      className={`motion-colors flex items-center justify-between gap-3 rounded-lg border p-2.5 text-left ${
        unmounted
          ? "border-line-strong bg-[#f2f2f780]"
          : on
            ? "border-[#0a84ff40] bg-white shadow-[0px_1px_0px_rgba(0,0,0,0.05)]"
            : "border-line-strong bg-white hover:border-[#d1d1d6]"
      }`}
    >
      <span className="flex min-w-0 items-center gap-2.5">
        <span
          className={`flex size-4 shrink-0 items-center justify-center rounded-sm border transition-colors duration-[var(--dur-fast)] ${
            on ? "border-accent bg-accent text-white" : "border-[#d1d1d6] bg-white"
          }`}
        >
          {on && <Check size={11} strokeWidth={3} className="pop-in" />}
        </span>
        <span className="flex min-w-0 flex-col">
          <span className={`truncate font-mono text-[12.5px] ${on && !unmounted ? "font-semibold text-ink" : unmounted ? "text-ink-4" : "text-ink-2"}`}>
            {tool.name}
          </span>
          <span className={`truncate text-[11.5px] ${unmounted ? "text-[#aeaeb2]" : "text-ink-4"}`}>{tool.summary}</span>
          {reason && (
            <span className="mt-1 truncate rounded-md bg-[#f5f8ff] px-2 py-0.5 text-[11px] text-ink-3">
              <span className="font-semibold text-accent">AI 理由：</span>
              {reason}
            </span>
          )}
        </span>
      </span>
      {unmounted ? (
        <span className="shrink-0 rounded-sm bg-[#ff3b301a] px-2 py-0.5 text-[10.5px] font-medium text-[#ff3b30]">未挂载</span>
      ) : (
        <span className={`shrink-0 rounded-sm bg-[#f2f2f7] px-1.5 py-0.5 font-mono text-[10.5px] ${on ? "text-ink-3" : "text-ink-4"}`}>
          {label}
        </span>
      )}
    </button>
  );
}
