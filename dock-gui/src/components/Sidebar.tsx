// 左侧栏：展开态（预设 → 项目 → 会话树）。收起时整列宽度过渡到 0，
// 只剩窗口左上角那组固定按钮（见 FloatingWindowControls）。
// 设计：wonder-ui/shell/{SidebarHeader,SidebarNav,SidebarTree,SidebarFooter,
// SidebarSessionsState,BrandDefaultPresetMenu}。

import { useCallback, useEffect, useRef, useState, type MouseEvent, type ReactNode } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Bot,
  ChevronDown,
  ChevronRight,
  Ellipsis,
  Clock,
  Folder,
  GitPullRequest,
  LoaderCircle,
  PanelLeft,
  PenLine,
  Pin,
  Plus,
  Search,
  Settings,
  SquareDashed,
  SquarePen,
  type LucideIcon,
} from "lucide-react";
import type { PresetGroup, PresetOption, Project } from "../model";
import { COMING_SOON, IconButton, TrafficLights } from "./chrome";
import { HelpButton, ProfileButton } from "./ProfileMenu";
import { SessionWithSubagents, type SubagentTarget } from "./SubagentRows";
import { MenuButton } from "./Popover";
import { presetIcon } from "./presetIcon";
import { ProjectContextMenu, SessionContextMenu, type ProjectAction, type SessionAction } from "./SessionMenu";
import { SessionSearch } from "./SessionSearch";
import { imeComposing } from "../dock/ime";
import markUrl from "../assets/mark.png";

export interface SidebarProps {
  groups: PresetGroup[];
  activeSessionId: string | null;
  /** 正在跑的会话（转圈）。 */
  runningIds: ReadonlySet<string>;
  /** 在等你处理的会话（权限 / 提问 / 计划 / elicitation）。 */
  waitingIds: ReadonlySet<string>;
  /** 不在看的时候跑完、还没打开过的会话。 */
  finished: ReadonlyMap<string, { status: string }>;
  /** 展开的项目 id。 */
  expanded: Set<string>;
  /** 连接状态 / 错误，显示在树的上方。 */
  status: string | null;
  presets: PresetOption[];
  /** 新对话默认预设（品牌行显示它）。 */
  newChatPreset: string | null;
  onSetNewChatPreset: (id: string) => void;
  /** 铃铛（通知菜单，见 NotificationMenu.tsx）：品牌行右侧。 */
  notifications: (rail: boolean) => ReactNode;
  canGoBack: boolean;
  canGoForward: boolean;
  onToggleSidebar: () => void;
  onBack: () => void;
  onForward: () => void;
  onNewChat: () => void;
  onToggle: (nodeId: string) => void;
  /** 点会话：换掉主面板；`split` = ⌘ 点，在右侧分屏打开。 */
  onSelectSession: (sessionId: string, split?: boolean) => void;
  /** 主区域正在看的子代理（侧栏高亮它、它的会话行不再高亮）。 */
  activeSubagent: SubagentTarget | null;
  /** 侧栏点了会话下面的一个子代理。 */
  onSelectSubagent: (target: SubagentTarget) => void;
  /** 会话行右键菜单里的动作（改名走 `onRenameSession`）。 */
  onSessionAction: (sessionId: string, action: Exclude<SessionAction, "rename">) => void;
  onRenameSession: (sessionId: string, title: string) => void;
  /** 「项目」右边的 +：创建项目（预设 + 目录）。 */
  onCreateProject: () => void;
  /** 在某个项目下开新对话（新对话页预填预设和目录）。 */
  onNewChatIn: (presetId: string, cwd: string) => void;
  /** 项目行右键菜单里的动作（改名、新对话走各自的回调）。 */
  onProjectAction: (groupId: string, project: Project, action: Exclude<ProjectAction, "rename" | "new-chat">) => void;
  onRenameProject: (groupId: string, project: Project, name: string) => void;
  /** 右侧分屏里开着的会话（侧栏里一样标成选中）。 */
  splitSessionId: string | null;
  /** 侧栏搜索开着（替换导航和会话树）。 */
  searching: boolean;
  /** 打开 / 关闭搜索；侧栏收起时点搜索会先展开侧栏。 */
  onSearch: (open: boolean) => void;
  /** 打开「新建自定义预设」。 */
  onNewPreset: () => void;
  /** 打开设置页（⌘,）。 */
  onOpenSettings: () => void;
  /** 侧栏「插件」：打开设置页的「插件与技能」。 */
  onOpenPlugins: () => void;
  /** 侧栏「Pull Request」「定时任务」：打开对应的整页。 */
  onOpenPage: (page: NavPage) => void;
  /** 正开着的整页（导航里那一项画选中态）。 */
  activePage: NavPage | null;
  /** 指针移到某个整页入口上：预取它的数据（Pull Request 列表）。 */
  onPrefetchPage?: (page: NavPage) => void;
}

interface NavItem {
  icon: LucideIcon;
  label: string;
  action?: "new-chat" | "new-preset" | "plugins" | NavPage;
}

const NAV: NavItem[] = [
  { icon: PenLine, label: "新对话", action: "new-chat" },
  { icon: GitPullRequest, label: "Pull Request", action: "pull-requests" },
  { icon: Clock, label: "定时任务", action: "schedules" },
  { icon: SquareDashed, label: "插件", action: "plugins" },
  { icon: Bot, label: "Agent 预设", action: "new-preset" },
];

/** 侧栏导航能打开的整页（盖住会话区，侧栏留着）。 */
export type NavPage = "pull-requests" | "schedules";

/** 导航项点了做什么。 */
function navAction(
  props: Pick<SidebarProps, "onNewChat" | "onNewPreset" | "onOpenPlugins" | "onOpenPage">,
  action: NavItem["action"],
) {
  if (action === "new-chat") return props.onNewChat;
  if (action === "new-preset") return props.onNewPreset;
  if (action === "plugins") return props.onOpenPlugins;
  if (action === "pull-requests" || action === "schedules") return () => props.onOpenPage(action);
  return undefined;
}

/** 窗口左上角那一行：红绿灯 + 折叠侧栏 + 前进后退。展开侧栏和侧栏收起时共用。 */
export function WindowControls({
  onToggleSidebar,
  onBack,
  onForward,
  canGoBack,
  canGoForward,
  collapsed = false,
}: Pick<SidebarProps, "onToggleSidebar" | "onBack" | "onForward" | "canGoBack" | "canGoForward"> & {
  /** 侧栏已经收起：按钮变成「展开侧栏」。 */
  collapsed?: boolean;
}) {
  return (
    <div className="flex items-center gap-3" data-tauri-drag-region>
      <TrafficLights />
      <div className="flex items-center gap-1.5">
        <IconButton
          icon={PanelLeft}
          label={collapsed ? "展开侧栏 ⌘B" : "折叠侧栏 ⌘B"}
          onClick={onToggleSidebar}
          className="size-6"
        />
        <IconButton icon={ArrowLeft} label="后退" onClick={onBack} disabled={!canGoBack} className="size-6" />
        <IconButton icon={ArrowRight} label="前进" onClick={onForward} disabled={!canGoForward} className="size-6" />
      </div>
    </div>
  );
}

/**
 * 侧栏收起时的「切换侧栏 / 后退 / 前进」：固定在窗口左上角，层级高过预设对话框（z-30），
 * 低过设置页和预设编辑页（它们盖满整窗、有自己的关闭）。会话头 / 工作站头栏里是同尺寸
 * 的透明占位；位置和占位对齐（占位从 x=20 起，和这里一致）。
 */
export function FloatingWindowControls(props: Pick<SidebarProps, "onToggleSidebar" | "onBack" | "onForward" | "canGoBack" | "canGoForward">) {
  return (
    <div className="fixed left-5 top-0 z-[35] flex h-11 items-center" data-tauri-drag-region>
      <WindowControls {...props} collapsed />
    </div>
  );
}

/** 品牌标。源图是仓库 `assets/logo/mark.png`，这里是去白底后的裁切，颜色跟文字走。 */
function BrandMark() {
  return (
    <span
      aria-hidden
      className="inline-block size-5 shrink-0 bg-current"
      style={{
        maskImage: `url("${markUrl}")`,
        WebkitMaskImage: `url("${markUrl}")`,
        maskSize: "contain",
        WebkitMaskSize: "contain",
        maskRepeat: "no-repeat",
        WebkitMaskRepeat: "no-repeat",
        maskPosition: "center",
        WebkitMaskPosition: "center",
      }}
    />
  );
}

function BrandMenu(props: SidebarProps) {
  const current = props.presets.find((p) => p.id === props.newChatPreset);
  return (
    <MenuButton
      title="切换预设"
      side="bottom"
      align="left"
      header="当前预设"
      options={props.presets
        .filter((p) => p.available)
        .map((p) => ({ id: p.id, label: p.label, icon: presetIcon(p.id) }))}
      value={props.newChatPreset ?? ""}
      onPick={props.onSetNewChatPreset}
      className="flex min-w-0 items-center gap-1.5 rounded-md text-[17px] font-semibold leading-none tracking-[-0.3px] text-ink"
      trigger={
        <>
          <BrandMark />
          <span className="truncate">Dock{current ? ` · ${current.label}` : ""}</span>
          <ChevronDown size={14} className="shrink-0 text-ink-4" />
        </>
      }
      footer={
        <div className="flex flex-col gap-1">
          <span className="text-[11px] text-ink-4">侧栏只列这个预设的会话，新对话也用它</span>
          <button type="button" onClick={props.onNewPreset} className="text-left text-xs font-medium text-accent hover:underline">
            新建自定义预设…
          </button>
        </div>
      }
    />
  );
}

export function Sidebar(props: SidebarProps) {
  // 右键菜单（钉在鼠标位置）与侧栏里的内联改名。
  const [menu, setMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  // 项目行的右键菜单与改名（id 是会话树里的项目 id）。
  const [projectMenu, setProjectMenu] = useState<{ groupId: string; project: Project; x: number; y: number } | null>(
    null,
  );
  const [renamingProject, setRenamingProject] = useState<string | null>(null);
  const closeProjectMenu = useCallback(() => setProjectMenu(null), []);
  const { groups, activeSessionId, expanded, notifications } = props;
  // 树里只列当前预设（品牌行下拉选的）的项目和会话；别的预设的会话在搜索里找。
  const current = groups.find((g) => g.id === props.newChatPreset) ?? emptyGroup(props);

  return (
    <aside
      // 侧栏里不弹 WebKit 自带的菜单（Reload / Inspect）：会话和项目行有自己的，别处就什么都不弹。
      // 输入框（搜索、行内改名）照旧，好用拷贝粘贴。
      onContextMenu={(e) => !(e.target instanceof HTMLInputElement) && e.preventDefault()}
      // h-full：侧栏自己撑满窗口高，底部的个人信息块靠 mt-auto 贴底 ——
      // 不给高度的话，这一列高=内容高，块会跟着内容跑（列表长了就掉出视口）。
      className="glass-sidebar flex h-full min-h-0 w-[248px] shrink-0 flex-col gap-1 border-r border-line bg-sidebar px-2.5 pb-2"
    >
      <div className="flex shrink-0 flex-col gap-1.5 pb-1 pt-2.5" data-tauri-drag-region>
        <div className="flex h-6 items-center px-1.5" data-tauri-drag-region>
          <WindowControls {...props} />
        </div>
        <div className="mt-1 flex h-9 items-center justify-between px-1.5" data-tauri-drag-region>
          <BrandMenu {...props} />
          <div className="flex items-center gap-1">
            {props.searching ? (
              <span className="enter-fade pr-1 text-[10px] text-ink-4">Esc 关闭</span>
            ) : (
              <IconButton icon={Search} label="搜索会话" onClick={() => props.onSearch(true)} className="size-7" />
            )}
            {notifications(false)}
          </div>
        </div>
      </div>

      {props.searching ? (
        <SessionSearch
          groups={groups}
          activeSessionId={activeSessionId}
          onSelect={(id, split) => {
            props.onSearch(false);
            props.onSelectSession(id, split);
          }}
          onClose={() => props.onSearch(false)}
        />
      ) : (
        <>
          <nav className="mt-1 flex shrink-0 flex-col gap-0.5">
            {NAV.map(({ icon: Icon, label, action }) => (
              <button
                key={label}
                type="button"
                title={action ? label : COMING_SOON}
                onClick={navAction(props, action)}
                onMouseEnter={() => (action === "pull-requests" || action === "schedules") && props.onPrefetchPage?.(action)}
                aria-current={action === props.activePage ? "page" : undefined}
                className={`motion-colors flex h-8 items-center gap-2.5 rounded-lg px-2.5 text-left text-[13px] font-medium ${
                  action === props.activePage ? "bg-[#0a84ff14] text-accent" : "text-ink hover:bg-hover"
                }`}
              >
                <Icon size={15} className={`shrink-0 ${action === props.activePage ? "text-accent" : "text-ink-3"}`} />
                {label}
              </button>
            ))}
          </nav>

          <div className="mt-3 flex min-h-0 flex-1 flex-col gap-1">
            <div className="flex h-6 shrink-0 items-center justify-between pl-2.5 pr-1 text-[11px] font-semibold tracking-[0.275px] text-ink-4">
              项目
              <IconButton icon={Plus} label="创建项目" size={14} onClick={props.onCreateProject} className="size-6 text-ink-4" />
            </div>
            {props.status && <div className="enter-fade px-2.5 text-[11px] leading-[1.4] text-ink-4">{props.status}</div>}
            {/* 换预设时整列按 key 重挂，带进场淡入。 */}
            <div key={current?.id} className="thin-scroll enter-fade flex min-h-0 flex-1 flex-col gap-px overflow-y-auto">
              {current && current.projects.length === 0 && (
                <div className="px-2.5 py-1 text-[11px] leading-[1.4] text-ink-4">「{current.label}」下还没有会话</div>
              )}
              {current?.projects.map((project) => {
                const open = expanded.has(project.id);
                return (
                  <div key={project.id} className="flex flex-col gap-px">
                    {renamingProject === project.id ? (
                      <RowRename
                        title={project.name}
                        onDone={(name) => {
                          setRenamingProject(null);
                          if (name !== null && name !== project.name) props.onRenameProject(current.id, project, name);
                        }}
                      />
                    ) : (
                      <ProjectRow
                        project={project}
                        open={open}
                        menuOpen={projectMenu?.project.id === project.id}
                        onToggle={() => props.onToggle(project.id)}
                        onNewChat={() => props.onNewChatIn(current.id, project.path)}
                        onMenu={(x, y) => setProjectMenu({ groupId: current.id, project, x, y })}
                      />
                    )}
                    <Fold open={open}>
                      <div className="flex flex-col gap-px pl-3">
                        {project.sessions.length === 0 && (
                          <button
                            type="button"
                            onClick={() => props.onNewChatIn(current.id, project.path)}
                            className="motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-[7px] px-2.5 text-left text-[12.5px] text-ink-4 hover:bg-hover hover:text-ink-2"
                          >
                            <PenLine size={12} className="shrink-0" />
                            新对话
                          </button>
                        )}
                        {project.sessions.map((session) => (
                          <SessionWithSubagents
                            key={session.id}
                            session={session}
                            active={props.activeSubagent}
                            onSelect={props.onSelectSubagent}
                            row={{
                              title: session.title,
                              active:
                                (session.id === activeSessionId && !props.activeSubagent) ||
                                session.id === props.splitSessionId,
                              running: props.runningIds.has(session.id),
                              waiting: props.waitingIds.has(session.id),
                              finished: props.finished.get(session.id)?.status,
                              open: session.open,
                              menuOpen: menu?.id === session.id,
                              renaming: renaming === session.id,
                              onRename: (title) => {
                                setRenaming(null);
                                if (title && title !== session.title) props.onRenameSession(session.id, title);
                              },
                              onClick: (e) => props.onSelectSession(session.id, e.metaKey),
                              onContextMenu: (e) => {
                                e.preventDefault();
                                setMenu({ id: session.id, x: e.clientX, y: e.clientY });
                              },
                            }}
                          />
                        ))}
                      </div>
                    </Fold>
                  </div>
                );
              })}
            </div>
          </div>
        </>
      )}

      {projectMenu && (
        <ProjectContextMenu
          x={projectMenu.x}
          y={projectMenu.y}
          pinned={!!projectMenu.project.pinned}
          openCount={projectMenu.project.sessions.filter((s) => s.open).length}
          onClose={closeProjectMenu}
          onAction={(action) => {
            const { groupId, project } = projectMenu;
            if (action === "rename") setRenamingProject(project.id);
            else if (action === "new-chat") props.onNewChatIn(groupId, project.path);
            else props.onProjectAction(groupId, project, action);
          }}
        />
      )}
      {menu && (
        <SessionContextMenu
          x={menu.x}
          y={menu.y}
          canDisconnect={canDisconnect(props.groups, menu.id)}
          onClose={closeMenu}
          onAction={(action) => (action === "rename" ? setRenaming(menu.id) : props.onSessionAction(menu.id, action))}
        />
      )}

      <div className="mt-auto flex h-10 shrink-0 items-center gap-1">
        <ProfileButton />
        <div className="flex items-center gap-0.5">
          <IconButton icon={Settings} label="设置 ⌘," size={15} onClick={props.onOpenSettings} className="size-6 text-ink-4" />
          <HelpButton className="size-6" />
        </div>
      </div>
    </aside>
  );
}

/** 折叠 / 展开：grid 行高过渡（.fold），收起后不渲染以免 Tab 键扫到。 */
function Fold({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div className="fold" data-open={open} aria-hidden={!open}>
      <div>{children}</div>
    </div>
  );
}

/** 会话行三态：运行中转圈 / 开着绿点 / 已关无标记。设计：SidebarSessionsState。 */
/**
 * 会话行。状态从强到弱：在等你（橙点 +「在等你」）> 在跑（转圈）> 刚跑完没看
 * （绿点「已完成」/ 红点「出错」）> 在 Dock 里开着（小绿点）。
 * 设计：SidebarSessionsState、SidebarSessionsWaiting（等你的颜色取整窗稿 snot 的橙色）。
 */
export function SessionRow({
  title,
  active,
  running,
  waiting,
  finished,
  open,
  menuOpen = false,
  renaming = false,
  onRename,
  onClick,
  onContextMenu,
}: {
  title: string;
  active: boolean;
  running: boolean;
  waiting: boolean;
  finished?: string;
  open: boolean;
  /** 这一行的右键菜单开着：描一圈蓝边（设计 ber）。 */
  menuOpen?: boolean;
  /** 在行内改名；`onRename(null)` = 取消。 */
  renaming?: boolean;
  onRename?: (title: string | null) => void;
  onClick: (e: MouseEvent) => void;
  onContextMenu?: (e: MouseEvent) => void;
}) {
  const failed = finished === "failed";
  let lead: ReactNode = null;
  let tail: ReactNode = null;
  if (waiting) {
    lead = <span className="pop-in size-2 shrink-0 rounded-full bg-[#ff9500]" />;
    tail = (
      <span className="pop-in ml-auto shrink-0 rounded-full bg-[#ff95001a] px-1.5 py-0.5 text-[10px] font-medium leading-none text-[#ff9500]">
        在等你
      </span>
    );
  } else if (running) {
    // 进场淡入放外层：和 animate-spin 写在同一个元素上，两个 animation 只剩 enter-fade，圈不转。
    lead = (
      <span className="enter-fade flex shrink-0">
        <LoaderCircle size={12} className="animate-spin text-accent" />
      </span>
    );
  } else if (finished) {
    lead = (
      <span
        className={`pop-in size-2 shrink-0 rounded-full ${
          failed ? "bg-[#ff3b30] shadow-[0_0_0_2px_rgba(255,59,48,0.2)]" : "bg-[#34c759] shadow-[0_0_0_2px_rgba(52,199,89,0.2)]"
        }`}
      />
    );
    tail = (
      <span className={`enter-fade ml-auto shrink-0 text-[10px] leading-none ${failed ? "text-[#ff3b30]" : "text-[#34c759]"}`}>
        {failed ? "出错" : "已完成"}
      </span>
    );
  } else if (open) {
    lead = <span title="在 Dock 里开着" className="enter-fade size-1.5 shrink-0 rounded-full bg-[#34c759]" />;
  }
  const strong = active || waiting || !!finished;
  if (renaming) return <RowRename title={title} onDone={(t) => onRename?.(t)} />;
  return (
    <button
      type="button"
      title={`${title}\n⌘ 点击：在右侧分屏打开`}
      onClick={onClick}
      onContextMenu={onContextMenu}
      className={`motion-colors flex h-7 shrink-0 select-none items-center gap-1.5 rounded-[7px] px-2.5 text-left text-[12.5px] ${
        active ? `bg-[#e8e8ed] ${menuOpen ? "" : "glass-state"}` : menuOpen ? "bg-hover" : "hover:bg-hover"
      } ${menuOpen ? "shadow-[0_0_0_1px_rgba(10,132,255,0.4)]" : ""} ${strong ? "font-medium text-ink" : "text-ink-2"}`}
    >
      {lead}
      <span className="min-w-0 truncate">{title}</span>
      {tail}
    </button>
  );
}

/**
 * 项目行：点了展开 / 收起；悬停（或菜单开着）时右边出「…」和「新对话」；置顶的在行尾标钉子。
 * 右键和「…」都弹项目菜单。设计：wonder-ui/shell/SidebarProjectMenu（aru）。
 */
function ProjectRow({
  project,
  open,
  menuOpen,
  onToggle,
  onNewChat,
  onMenu,
}: {
  project: Project;
  open: boolean;
  menuOpen: boolean;
  onToggle: () => void;
  onNewChat: () => void;
  onMenu: (x: number, y: number) => void;
}) {
  const actions = useRef<HTMLSpanElement>(null);
  return (
    <div
      title={project.path}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(e.clientX, e.clientY);
      }}
      className={`group motion-colors flex h-7 shrink-0 select-none items-center rounded-[7px] ${
        menuOpen ? "bg-hover shadow-[0_0_0_1px_rgba(10,132,255,0.4)]" : "hover:bg-hover"
      }`}
    >
      <button type="button" onClick={onToggle} className="flex h-full min-w-0 flex-1 items-center gap-2 pl-2 text-left">
        <ChevronRight
          size={12}
          data-open={open}
          className={`rotate-open shrink-0 ${open ? "text-ink-4" : "text-ink-5"}`}
        />
        <Folder size={14} className={`shrink-0 ${open ? "text-ink-3" : "text-ink-4"}`} />
        <span className="truncate text-[12.5px] text-ink-2">{project.name}</span>
      </button>
      <span
        ref={actions}
        className={`mr-1 flex shrink-0 items-center gap-0.5 ${menuOpen ? "" : "hidden group-hover:flex"}`}
      >
        <IconButton
          icon={Ellipsis}
          label="更多"
          size={13}
          onClick={() => {
            const r = actions.current?.getBoundingClientRect();
            if (r) onMenu(r.left, r.bottom + 4);
          }}
          className="size-5 text-ink-3"
        />
        <IconButton icon={SquarePen} label="新对话" size={13} onClick={onNewChat} className="size-5 text-ink-3" />
      </span>
      {project.pinned && !menuOpen && (
        <Pin size={11} className="mr-2 shrink-0 text-ink-4 group-hover:hidden" aria-label="已置顶" />
      )}
    </div>
  );
}

/** 侧栏行内改名：Enter 保存、Esc 取消、点外面也算保存（同会话头 RenameInput）。 */
function RowRename({ title, onDone }: { title: string; onDone: (title: string | null) => void }) {
  const [value, setValue] = useState(title);
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => ref.current?.select(), []);
  const finish = (t: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(t);
  };
  return (
    <input
      ref={ref}
      value={value}
      aria-label="会话名"
      onChange={(e) => setValue(e.target.value)}
      onBlur={() => finish(value.trim() || null)}
      onKeyDown={(e) => {
        if (imeComposing(e)) return;
        if (e.key === "Enter") finish(value.trim() || null);
        if (e.key === "Escape") finish(null);
      }}
      className="pop-in h-7 w-full shrink-0 rounded-[7px] border border-accent bg-white px-2.5 text-[12.5px] font-medium text-ink shadow-sm outline-none selection:bg-[#0a84ff33] select-text"
    />
  );
}

/** 当前预设还没有项目时的空组：树里出空提示，「+」照样建在这个预设下。 */
function emptyGroup(props: SidebarProps): PresetGroup | null {
  const preset = props.presets.find((p) => p.id === props.newChatPreset);
  return preset ? { id: preset.id, label: preset.label, projects: [] } : null;
}

/** 「断开」只给开着、有对话的会话：空会话 Dock 关掉不落盘，断开就等于删了。 */
function canDisconnect(groups: PresetGroup[], id: string): boolean {
  for (const g of groups)
    for (const p of g.projects) {
      const s = p.sessions.find((x) => x.id === id);
      if (s) return s.open && s.title !== "新对话";
    }
  return false;
}
