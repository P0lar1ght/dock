// 浏览器面板的标签栏。设计：wonder-ui/pages/BrowserPanel 的 BrowserPanelTabs（das）、
// BrowserTabStripStates（mba）。
//
// 标签就是会话的标签页（网关 `browser/view/tabs`，顺序和 agent 的 `browser_tabs` 一致）：
// 你和 agent 共用一个浏览器，这里切到哪页，agent 接着就在哪页操作。
// agent 跑的时候只读（和画面一样，先接手）。最后一页也能关：关掉这个会话的浏览器页，
// 面板回到「还没打开网页」（和真浏览器关掉最后一个标签页一样）。
// 多了先一起变窄、最窄只留图标，再放不下就横向滚动，当前页总滚进视野。
// 标签宽度写死 `w-[200px]` 而不是 flex-basis：标签是 `@container`（尺寸隔离），内容宽度算 0，
// 只给 basis 时标签栏的内在宽度塌成最小，空间再多也挤成图标。
// 页面新开的标签页（target=_blank、window.open）滑进来并闪一下蓝边。

import { useEffect, useRef, useState, type ReactNode } from "react";
import { Globe, Plus, X } from "lucide-react";

export interface BrowserTab {
  targetId: string;
  url: string;
  title: string;
  active: boolean;
}

/** 新出现的标签页闪多久（和 styles.css 的 `.tab-enter` 一致）。 */
const FRESH_MS = 900;

export function BrowserTabs({
  tabs,
  locked,
  agentHere,
  fallback,
  lead,
  controls,
  onSwitch,
  onCloseTab,
  onNew,
  onClosePanel,
}: {
  tabs: BrowserTab[];
  /** agent 在跑、你没接手：只看不动。 */
  locked: boolean;
  /** agent 正在当前页上操作：当前页标签上亮一个蓝点。 */
  agentHere: boolean;
  /** 没有标签页时左边显示的字（「浏览器」）。 */
  fallback: string;
  lead?: ReactNode;
  controls?: ReactNode;
  onSwitch: (targetId: string) => void;
  onCloseTab: (targetId: string) => void;
  /** 不给就不显示「+」（网关不支持标签栏）。 */
  onNew?: () => void;
  onClosePanel: () => void;
}) {
  const strip = useRef<HTMLDivElement>(null);
  const fresh = useFresh(tabs);
  const active = tabs.find((t) => t.active)?.targetId;

  // 当前页总在视野里（agent 切过去、页面新开的页落在最右边时）。
  useEffect(() => {
    const el = strip.current?.querySelector<HTMLElement>(`[data-target="${CSS.escape(active ?? "")}"]`);
    el?.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  }, [active, tabs.length]);

  const closable = !locked;
  return (
    <header className="flex h-11 shrink-0 items-end gap-1 border-b border-line-soft bg-[#f2f2f4] pl-2 pr-3" data-tauri-drag-region>
      {lead && <div className="flex shrink-0 self-center">{lead}</div>}
      {tabs.length === 0 ? (
        <div className="flex h-[34px] min-w-0 items-center gap-2 px-2.5 text-ink" data-tauri-drag-region>
          <Globe size={15} className="shrink-0 text-ink-3" />
          <span className="truncate text-[13px] font-semibold">{fallback}</span>
        </div>
      ) : (
        <div ref={strip} role="tablist" className="tab-strip flex min-w-0 items-end gap-0.5 overflow-x-auto">
          {tabs.map((tab) => (
            <Tab
              key={tab.targetId}
              tab={tab}
              fresh={fresh.has(tab.targetId)}
              locked={locked}
              closable={closable}
              agentHere={agentHere && tab.active}
              onSwitch={() => !tab.active && onSwitch(tab.targetId)}
              onClose={() => onCloseTab(tab.targetId)}
            />
          ))}
        </div>
      )}
      {onNew && tabs.length > 0 && (
        <button
          type="button"
          title={locked ? "Agent 在操作，先接手" : "新标签页"}
          aria-label="新标签页"
          disabled={locked}
          onClick={onNew}
          className="motion-colors mb-[3px] flex size-7 shrink-0 items-center justify-center rounded-[7px] text-ink-3 hover:bg-black/5 hover:text-ink disabled:text-ink-5 disabled:hover:bg-transparent"
        >
          <Plus size={16} />
        </button>
      )}
      <div className="h-full min-w-4 flex-1" data-tauri-drag-region />
      <div className="flex shrink-0 items-center gap-2 self-center">
        {controls}
        {controls && <div className="h-4 w-px shrink-0 bg-line-strong" />}
        <button
          type="button"
          aria-label="关闭浏览器面板"
          title="关闭浏览器面板"
          onClick={onClosePanel}
          className="motion-colors flex size-7 items-center justify-center rounded-[7px] text-ink-4 hover:bg-black/5 hover:text-ink"
        >
          <X size={14} />
        </button>
      </div>
    </header>
  );
}

function Tab({
  tab,
  fresh,
  locked,
  closable,
  agentHere,
  onSwitch,
  onClose,
}: {
  tab: BrowserTab;
  fresh: boolean;
  locked: boolean;
  closable: boolean;
  agentHere: boolean;
  onSwitch: () => void;
  onClose: () => void;
}) {
  const label = tabLabel(tab);
  return (
    <div
      role="tab"
      aria-selected={tab.active}
      data-target={tab.targetId}
      title={tab.title && tab.url ? `${tab.title}\n${tab.url}` : label}
      onClick={() => !locked && onSwitch()}
      // 中键关（和真浏览器一样）。
      onAuxClick={(e) => e.button === 1 && closable && onClose()}
      className={`@container group relative flex h-[34px] shrink items-center gap-2 rounded-t-[9px] pl-2.5 pr-1.5 ${
        tab.active
          ? "w-[200px] min-w-[120px] bg-white shadow-[0px_-1px_0px_#e5e5ea,1px_0px_0px_#e5e5ea,-1px_0px_0px_#e5e5ea]"
          : `w-[200px] min-w-9 ${locked ? "" : "cursor-pointer hover:bg-black/[0.04]"}`
      } ${fresh ? "tab-enter" : ""}`}
    >
      <Favicon url={tab.url} label={label} />
      <span
        className={`hidden min-w-0 flex-1 truncate text-xs @min-[56px]:block ${
          tab.active ? "font-medium text-ink" : "text-ink-3"
        }`}
      >
        {label}
      </span>
      {agentHere && <span className="size-1.5 shrink-0 rounded-full bg-accent shadow-[0px_0px_0px_3px_rgba(10,132,255,0.18)]" />}
      {closable && (
        <button
          type="button"
          aria-label={`关闭 ${label}`}
          onClick={(e) => {
            e.stopPropagation();
            onClose();
          }}
          className={`motion-colors hidden size-5 shrink-0 items-center justify-center rounded-[5px] text-ink-4 hover:bg-black/[0.07] hover:text-ink @min-[96px]:flex ${
            tab.active ? "" : "opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
          }`}
        >
          <X size={12} />
        </button>
      )}
    </div>
  );
}

/** 站点图标：先试站点根下的 favicon.ico，拿不到就用首字母色块；非网页（about:、data:）用地球。 */
function Favicon({ url, label }: { url: string; label: string }) {
  const origin = webOrigin(url);
  const [failed, setFailed] = useState<string | null>(null);
  if (!origin) return <Globe size={14} className="shrink-0 text-ink-4" />;
  if (failed !== origin) {
    return (
      <img
        src={`${origin}/favicon.ico`}
        alt=""
        draggable={false}
        onError={() => setFailed(origin)}
        className="size-4 shrink-0 rounded-[3px] object-contain"
      />
    );
  }
  const letter = (hostOf(url).replace(/^www\./, "")[0] ?? label[0] ?? "·").toUpperCase();
  return (
    <span
      className="flex size-4 shrink-0 items-center justify-center rounded-[4px] text-[9px] font-bold text-white"
      style={{ background: LETTER_COLORS[hash(origin) % LETTER_COLORS.length] }}
    >
      {letter}
    </span>
  );
}

const LETTER_COLORS = ["#0a84ff", "#34c759", "#ff9500", "#af52de", "#ff3b30", "#5ac8fa", "#ff2d55", "#8e8e93"];

/** 标签上显示的字：标题，没有就主机名，再没有就「新标签页」。 */
export function tabLabel(tab: { url: string; title: string }): string {
  const title = tab.title.trim();
  if (title && title !== tab.url) return title;
  if (!tab.url || tab.url === "about:blank") return "新标签页";
  return hostOf(tab.url) || title || tab.url;
}

/** 刚出现的标签页（不算第一次拿到的那批）：闪 FRESH_MS。 */
function useFresh(tabs: BrowserTab[]): Set<string> {
  const seen = useRef<Set<string> | null>(null);
  const [fresh, setFresh] = useState<Set<string>>(new Set());
  useEffect(() => {
    const ids = tabs.map((t) => t.targetId);
    const before = seen.current;
    seen.current = new Set(ids);
    if (!before) return;
    const added = ids.filter((id) => !before.has(id));
    if (added.length === 0) return;
    setFresh(new Set(added));
    const t = window.setTimeout(() => setFresh(new Set()), FRESH_MS);
    return () => window.clearTimeout(t);
  }, [tabs]);
  // 标签全没了（换会话、视图关了）：下一批当第一次。
  useEffect(() => {
    if (tabs.length === 0) seen.current = null;
  }, [tabs.length]);
  return fresh;
}

function webOrigin(url: string): string | null {
  try {
    const u = new URL(url);
    return u.protocol === "http:" || u.protocol === "https:" ? u.origin : null;
  } catch {
    return null;
  }
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

function hash(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
  return Math.abs(h);
}
