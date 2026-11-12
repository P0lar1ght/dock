// 右侧大面板（浏览器 / 文件 / 桌面 / 画布 / 子代理）共用的头栏控件。设计：wonder-ui/pages/SidePanelsSplit。
//
// 头栏统一成「拖手 · 标题 … 分屏 · 新窗口 | 关闭」：各面板只留 `lead` / `controls` 两个插槽，
// 控件本身在这里画，面板自己不知道分屏和独立窗口。
// 拖手往窗口外拖 = 开成独立窗口（同工作台：交给 Rust pop_out_drag 问系统鼠标，不靠 pointerup）。

import { type PointerEvent as ReactPointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Bot, Columns2, ExternalLink, Folder, Globe, GripVertical, LayoutDashboard, Monitor, type LucideIcon } from "lucide-react";
import { IconButton } from "./chrome";
import { Popover } from "./Popover";
import type { BuiltinPanel, SidePanel } from "./ToolPanel";
import { usePanelMenuItems, useSplitGroups } from "../features/registry";
import { gatewayArg } from "../dock/store";

export const PANELS: { kind: BuiltinPanel; label: string; icon: LucideIcon; shortcut: string }[] = [
  { kind: "browser", label: "浏览器", icon: Globe, shortcut: "⌘T" },
  { kind: "files", label: "文件", icon: Folder, shortcut: "⌘P" },
  { kind: "desktop", label: "桌面", icon: Monitor, shortcut: "⌘⇧D" },
  { kind: "canvas", label: "画布", icon: LayoutDashboard, shortcut: "⌘⇧C" },
  { kind: "subagents", label: "子代理", icon: Bot, shortcut: "" },
];

/** 不拖、直接开成独立窗口（落在系统默认位置）。开不了回 false，面板留在原处。 */
export function openPanelWindow(kind: SidePanel, sessionId: string): Promise<boolean> {
  return invoke("open_panel_window", { panel: kind, sessionId, gateway: gatewayArg(), x: 0, y: 0 }).then(
    () => true,
    () => false,
  );
}

/** 头栏最左边的拖手：按下就交给 Rust 等系统鼠标松开，松手时光标在窗口外就开窗。 */
export function PanelGrip({
  kind,
  sessionId,
  onDragging,
  onPopOut,
}: {
  kind: SidePanel;
  sessionId: string;
  onDragging: (on: boolean) => void;
  onPopOut: () => void;
}) {
  const down = async (e: ReactPointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    onDragging(true);
    try {
      if (await invoke<boolean>("pop_out_drag", { sessionId, panel: kind, gateway: gatewayArg() })) onPopOut();
    } catch {
      // 开窗失败就当没拖出去，面板留在原处。
    } finally {
      onDragging(false);
    }
  };
  return (
    <button
      type="button"
      title="拖到窗口外开成独立窗口"
      aria-label="拖出"
      onPointerDown={down}
      className="motion-colors -ml-1.5 flex size-6 shrink-0 cursor-grab touch-none select-none items-center justify-center rounded text-ink-5 hover:bg-hover hover:text-ink-3 active:cursor-grabbing"
    >
      <GripVertical size={14} />
    </button>
  );
}

/** 关闭按钮左边：分屏菜单（列出还没开的面板）+ 在新窗口打开。 */
export function PanelControls({
  open,
  onSplit,
  onPopOut,
}: {
  /** 已经开着的面板：菜单里灰掉。 */
  open: SidePanel[];
  onSplit: (kind: SidePanel) => void;
  /** `null` = 没有会话，开不了独立窗口。 */
  onPopOut: (() => void) | null;
}) {
  const splitGroups = useSplitGroups();
  // 功能面板（「侧边聊天」「审查」这些）：自己登记的一行，跟内置面板一起列。
  const menuItems = usePanelMenuItems();
  return (
    <>
      <Popover
        title="再开一页"
        side="bottom"
        trigger={<Columns2 size={14} strokeWidth={2} />}
        className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-hover hover:text-ink aria-expanded:bg-selected"
        panelClassName="w-[220px] p-1.5"
      >
        {(close) => (
          <>
            <div className="px-2.5 pb-1 pt-1.5 text-[11px] font-medium text-ink-4">再叠一页（翻到最前）</div>
            {PANELS.map(({ kind, label, icon: Icon, shortcut }) => {
              const opened = open.includes(kind);
              return (
                <button
                  key={kind}
                  type="button"
                  disabled={opened}
                  onClick={() => {
                    close();
                    onSplit(kind);
                  }}
                  className="motion-colors flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-left hover:bg-hover disabled:hover:bg-transparent"
                >
                  <Icon size={14} className={opened ? "text-ink-5" : "text-ink-3"} />
                  <span className={`flex-1 text-[13px] ${opened ? "text-ink-5" : "text-ink"}`}>{label}</span>
                  <span className={`text-[11px] ${opened ? "text-ink-5" : "font-mono text-ink-4"}`}>
                    {opened ? "已打开" : shortcut}
                  </span>
                </button>
              );
            })}
            {/* 功能登记的面板（侧边聊天、审查…）：跟内置面板一起列。 */}
            {menuItems.map((item) => {
              const Icon = item.icon;
              const opened = open.some((k) => k === item.kind || k.startsWith(`${item.prefix}:`));
              return (
                <button
                  key={item.prefix}
                  type="button"
                  disabled={opened}
                  onClick={() => {
                    close();
                    onSplit(item.kind);
                  }}
                  className="motion-colors flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-left hover:bg-hover disabled:hover:bg-transparent"
                >
                  {Icon && <Icon size={14} className={opened ? "text-ink-5" : "text-ink-3"} />}
                  <span className={`flex-1 text-[13px] ${opened ? "text-ink-5" : "text-ink"}`}>{item.label}</span>
                  {opened && <span className="text-[11px] text-ink-5">已打开</span>}
                </button>
              );
            })}
            {/* 功能登记的分组（插件面板），见 features/registry.ts。 */}
            {splitGroups.map((Group, i) => (
              <Group key={i} open={open} onSplit={(kind) => onSplit(kind as SidePanel)} close={close} />
            ))}
          </>
        )}
      </Popover>
      {onPopOut && <IconButton icon={ExternalLink} label="在新窗口打开" size={14} onClick={onPopOut} className="size-7" />}
    </>
  );
}
