// 右侧工具面板。设计：wonder-ui/shell/ShortcutsPane。「子代理」「终端」「浏览器」「文件」「桌面」「画布」
// 「侧边聊天」（chat/SideChat/SideChatEntry：当前会话有侧聊时右侧一个蓝点）是真的，其余保留原样外观，悬停提示「即将推出」。
// 「审查」（2026-10-28 用户要回来的）：开功能面板 `changes:worktree`，看工作区的 git diff 和 code-review 批注。「子代理」照 chat/SubagentEntrances（ram）：
// 右侧蓝色徽标是在跑的个数。

import { Bot, GitCompareArrows, Globe, LayoutDashboard, MessageSquare, Monitor, SquareTerminal, Folder, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { COMING_SOON } from "./chrome";
import { useApp } from "../dock/store";
import { usePanelEntryGroups } from "../features/registry";

export type BuiltinPanel = "browser" | "files" | "desktop" | "canvas" | "subagents";
/** 右侧面板的种类：内置的，或功能登记的 `前缀:参数`（插件面板 `plugin:<id>`，见 features/registry.ts）。 */
export type SidePanel = BuiltinPanel | `${string}:${string}`;

interface Tool {
  icon: LucideIcon;
  label: string;
  shortcut: string;
  /** 没有 onClick 的就是「即将推出」。 */
  onClick?: () => void;
  active?: boolean;
  /** 右侧蓝色数字徽标（代替快捷键）。 */
  badge?: number;
  /** 右侧蓝色小圆点（代替快捷键）。 */
  dot?: boolean;
}

/** 工具面板里的一行：内置项和功能登记的入口（插件面板、MCP 应用）共用。 */
export function ToolRow({
  icon: Icon,
  label,
  hint,
  title,
  onClick,
}: {
  icon: LucideIcon;
  label: string;
  /** 右侧小字（面板 id、界面 uri）：面板 id 一样时好分辨。 */
  hint?: string;
  title?: string;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      title={title ?? label}
      onClick={onClick}
      className="motion-colors glass-tile flex h-10 items-center gap-2.5 rounded-[10px] border border-line-soft bg-white px-2.5 text-left hover:bg-hover"
    >
      <Icon size={15} className="shrink-0 text-ink-3" />
      <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-ink">{label}</span>
      {hint && <span className="max-w-[76px] shrink-0 truncate font-mono text-[11px] text-ink-4">{hint}</span>}
    </button>
  );
}

export function ToolPanel({
  terminalOpen,
  onToggleTerminal,
  onToggleBrowser,
  onToggleFiles,
  onToggleDesktop,
  onToggleCanvas,
  onToggleSubagents,
  subagentsRunning,
  onToggleSideChat,
  sideChatActive = false,
  onToggleReview,
  onOpenFeature,
  open = true,
  sideToggle,
}: {
  terminalOpen: boolean;
  onToggleTerminal: () => void;
  onToggleBrowser: () => void;
  onToggleFiles: () => void;
  onToggleDesktop: () => void;
  onToggleCanvas: () => void;
  onToggleSubagents: () => void;
  subagentsRunning: number;
  /** 不给 = Dock 不支持侧边聊天（旧版），保留「即将推出」。 */
  onToggleSideChat?: () => void;
  /** 当前会话已经有一段侧边聊天。 */
  sideChatActive?: boolean;
  /** 不给 = Dock 不支持工作区 diff（旧版），保留「即将推出」。 */
  onToggleReview?: () => void;
  /** 在右侧打开一个功能登记的面板（插件面板、MCP 应用）。 */
  onOpenFeature?: (kind: string) => void;
  /** 右侧区域是否展开：收起时内容淡出（外层同时把宽度收到 0）。 */
  open?: boolean;
  /** 头栏最右边（折叠右侧这一栏）。 */
  sideToggle?: ReactNode;
}) {
  // 终端是本机 shell：连远程 Dock 时会话目录在远端，不给开。
  const remote = useApp().remote;
  // 功能登记的入口组（插件面板、MCP 应用）：挂载了才有，没有就不占地方。
  const entryGroups = usePanelEntryGroups();
  const tools: Tool[] = [
    { icon: Bot, label: "子代理", shortcut: "", onClick: onToggleSubagents, badge: subagentsRunning },
    ...(remote ? [] : [{ icon: SquareTerminal, label: "终端", shortcut: "^`", onClick: onToggleTerminal, active: terminalOpen }]),
    { icon: Globe, label: "浏览器", shortcut: "⌘T", onClick: onToggleBrowser },
    { icon: Folder, label: "文件", shortcut: "⌘P", onClick: onToggleFiles },
    // 设计稿 ShortcutsPane 没有这一项：桌面面板是后加的（wonder-ui/pages/DesktopPanel）。
    { icon: Monitor, label: "桌面", shortcut: "⌘⇧D", onClick: onToggleDesktop },
    { icon: LayoutDashboard, label: "画布", shortcut: "⌘⇧C", onClick: onToggleCanvas },
    { icon: MessageSquare, label: "侧边聊天", shortcut: "⌥⌘S", onClick: onToggleSideChat, dot: sideChatActive },
    { icon: GitCompareArrows, label: "审查", shortcut: "", onClick: onToggleReview },
  ];

  return (
    // 折叠 / 展开统一由窗口右上角的「侧边面板」按钮管，这里不再重复画折叠按钮：
    // 外层把这一栏的宽度收到 0，这里把内容淡出（见 docs/MOTION.md「面板」）。
    <aside
      className="glass-sidebar glass-sidebar-right flex min-w-0 w-full flex-1 flex-col border-l border-line bg-sidebar transition-[opacity,transform] duration-[var(--dur-base)] ease-[var(--ease-out)]"
      style={{ opacity: open ? 1 : 0, transform: open ? undefined : "translateX(12px)" }}
    >
      <div className="flex h-11 shrink-0 items-center justify-between border-b border-line-soft px-3" data-tauri-drag-region>
        <span className="text-[13px] font-medium text-ink-3">工具</span>
        {sideToggle}
      </div>
      <div className="flex flex-col gap-1 p-3">
        {tools.map(({ icon: Icon, label, shortcut, onClick, active, badge, dot }) => (
          <button
            key={label}
            type="button"
            title={onClick ? label : `${label} · ${COMING_SOON}`}
            onClick={onClick}
            className={`motion-colors flex h-10 items-center gap-2.5 rounded-[10px] border px-2.5 text-left ${
              active ? "glass-tile glass-tile-on border-line-strong bg-selected" : "glass-tile border-line-soft bg-white hover:bg-hover"
            }`}
          >
            <Icon size={15} className="shrink-0 text-ink-3" />
            <span className="flex-1 text-[13px] font-medium text-ink">{label}</span>
            {badge ? (
              <span className="enter-fade flex h-[18px] min-w-[18px] shrink-0 items-center justify-center rounded-full bg-accent px-1.5 text-[11px] font-semibold text-white tabular-nums">
                {badge}
              </span>
            ) : dot ? (
              <span className="enter-fade mr-1 size-2 shrink-0 rounded-full bg-accent" />
            ) : (
              <span className="shrink-0 font-mono text-[11px] text-ink-4">{shortcut}</span>
            )}
          </button>
        ))}
        {/* 功能登记的入口（插件面板、MCP 应用）：挂在的面板各一行。 */}
        {entryGroups.map((Group, i) => (
          <Group key={i} onOpen={(kind) => onOpenFeature?.(kind)} />
        ))}
      </div>
    </aside>
  );
}
