// 窗口外壳：左侧栏 | 会话区 + 工具面板 | 底部终端。数据全部来自 Dock（dock/store.ts）。
// 这里只订阅应用级快照（`useApp`）；会话内容由 SessionPane 自己订阅，流式增量
// 不会让整个窗口重渲染。

import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { IN_TAURI, IconButton } from "./components/chrome";
import { openPath } from "@tauri-apps/plugin-opener";
import { FeatureHostContext, panelFor, type FeatureHost } from "./features/registry";
import {
  gatewayArg,
  lastSessionKey,
  presetKey,
  projectId,
  store,
  useApp,
  useForegroundStore,
  useGatewayId,
  useSubagents,
} from "./dock/store";
import { prefs, usePrefs } from "./dock/prefs";
import { matches } from "./dock/shortcuts";
import { pairingAccept } from "./dock/settingsApi";
import type { AiPresetDraft, Project, SessionSummary } from "./model";
import { LaunchSplash } from "./components/LaunchSplash";
import { PresetEditor } from "./components/PresetEditor";
import { PresetStudio } from "./components/preset/PresetStudio";
import { Settings, type SettingsSection } from "./components/settings/index";
import { DEFAULT_SPLIT, SplitDivider } from "./components/SplitDivider";
import { FloatingWindowControls, Sidebar, type SidebarProps } from "./components/Sidebar";
import { DeleteConfirm, RemoveProjectConfirm } from "./components/SessionMenu";
import { CreateProject } from "./components/CreateProject";
import { SubagentPage } from "./components/SubagentPage";
import type { SubagentTarget } from "./components/SubagentRows";
import { SessionPane } from "./components/SessionPane";
import { Workbench } from "./components/Workbench";
import { NewChat } from "./components/NewChat";
import { NotificationMenu } from "./components/NotificationMenu";
import { ToolPanel, type SidePanel } from "./components/ToolPanel";
import { BrowserPanel } from "./components/BrowserPanel";
import { FilesPanel } from "./components/FilesPanel";
import { DesktopPanel } from "./components/DesktopPanel";
import { CanvasPanel } from "./components/CanvasPanel";
import { SubagentPanel } from "./components/SubagentPanel";
import { TerminalDock } from "./components/TerminalDock";
import { PANELS, PanelControls, PanelGrip, openPanelWindow } from "./components/PanelChrome";
import { PanelStack } from "./components/PanelStack";
import { SideTabs, type SideTabMeta } from "./components/SideTabs";
import { PanelRight } from "lucide-react";
import { PullRequestsPage } from "./components/PullRequestsPage";
import { SchedulesPage } from "./components/SchedulesPage";
import { prefetchPrList } from "./dock/prApi";
import type { NavPage } from "./components/Sidebar";
import type { PathTarget } from "./components/markdown/pathLink";


const SPLIT_KEY = "dock-gui.splitRatio";
const SIDE_KEY = "dock-gui.sidePanelRatio";
/** 侧边聊天面板（features/side-chat 登记的 `sidechat`），跟着当前会话走。 */
const SIDE_CHAT = "sidechat:current";
/** 审查面板（features/turn-changes 登记的 `changes`）：工作区的 git diff + code-review 批注。 */
const REVIEW = "changes:worktree";
/** 开浏览器 / 文件 / 桌面面板时会话区占的比例（设计 turm、bam-alk：540 / 1440 偏窄，留够面板）。 */
const DEFAULT_SIDE = 0.42;
/** 右侧快捷面板（没开具体面板时那一列）的宽度。 */
const TOOL_PANEL_WIDTH = 220;

/** 切走时留下的工作区。终端、栏宽不在里面：那是这扇窗口的，不跟某台网关走。 */
interface WorkspaceSnap {
  history: { stack: (string | null)[]; index: number };
  subagentView: SubagentTarget | null;
  expanded: Set<string>;
  searching: boolean;
  navPage: NavPage | null;
  bench: string[];
  benchOpen: boolean;
  split: string | null;
  panes: { kinds: SidePanel[]; weights: number[] };
  panelOpen: boolean;
  finding: boolean;
  canvasTarget: { canvasId: string | null; seq: number } | null;
  fileTarget: (PathTarget & { threadId: string; seq: number }) | null;
  subagentTarget: { agentId: string | null; seq: number } | null;
  newChatSeed: { presetId: string; cwd: string; key: number } | null;
  savedPreset: string | null;
}

const workspaceSnaps = new Map<string, WorkspaceSnap>();

function freshWorkspace(gw: string): WorkspaceSnap {
  return {
    history: { stack: [null], index: 0 },
    subagentView: null,
    expanded: new Set(),
    searching: false,
    navPage: null,
    bench: [],
    benchOpen: false,
    split: null,
    panes: { kinds: [], weights: [] },
    panelOpen: true,
    finding: false,
    canvasTarget: null,
    fileTarget: null,
    subagentTarget: null,
    newChatSeed: null,
    savedPreset: localStorage.getItem(presetKey(gw)),
  };
}

export default function App() {
  const dock = useApp();
  const gw = useGatewayId();
  const fg = useForegroundStore();

  // 会话导航历史，给左上角的前进 / 后退用。`null` = 新对话页。
  const [history, setHistory] = useState<{ stack: (string | null)[]; index: number }>({ stack: [null], index: 0 });
  const activeSessionId = history.stack[history.index] ?? null;
  /** 主区域在看会话下面的一个子代理（侧栏点进来）；换会话就收起。 */
  const [subagentView, setSubagentView] = useState<SubagentTarget | null>(null);

  const [expanded, setExpanded] = useState(() => new Set<string>());
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => prefs.get().sidebarCollapsed);
  const [searching, setSearching] = useState(false);
  // 设置页开着时占满窗口；`null` = 没开。
  const [settings, setSettings] = useState<SettingsSection | null>(null);
  const [presetEditor, setPresetEditor] = useState(false);
  /** 侧栏导航打开的整页（Pull Request / 定时任务）：盖住会话区，去看会话就收起。 */
  const [navPage, setNavPage] = useState<NavPage | null>(null);
  // 预设编辑器（人设 / 工具 / 子代理）：盖在设置页上面，关掉回到设置页。
  // `id: null` = 还没保存的 AI 草稿。
  // `origin`：从会话里的草稿卡（`propose_preset`）打开的，保存后告诉那张卡。
  const [studio, setStudio] = useState<{
    id: string | null;
    ai?: { draft: AiPresetDraft; description: string };
    origin?: string;
    /** 每次打开一个号：同一个预设再点「编辑」也要重挂、现读盘，不能沿用上一次没关掉的草稿。 */
    opened?: number;
  } | null>(null);
  // 设置页「预设管理」的新建按钮：关掉设置、打开新建框；「编辑」打开预设编辑器。
  useEffect(() => {
    const open = () => {
      setSettings(null);
      setPresetEditor(true);
    };
    const edit = (e: Event) => setStudio({ id: (e as CustomEvent<string>).detail, opened: Date.now() });
    // 会话里的预设草稿卡「在编辑器中打开」：草稿不写盘，进编辑器审。
    const review = (e: Event) => {
      const d = (e as CustomEvent<{ draft: AiPresetDraft; description: string; origin: string }>).detail;
      setStudio({ id: null, ai: { draft: d.draft, description: d.description }, origin: d.origin, opened: Date.now() });
    };
    // 连接横条「连接设置」：令牌失效时去网关页换一个。
    const openSection = (e: Event) => setSettings((e as CustomEvent<SettingsSection>).detail);
    window.addEventListener("dock:new-preset", open);
    window.addEventListener("dock:edit-preset", edit);
    window.addEventListener("dock:review-preset-draft", review);
    window.addEventListener("dock:open-settings", openSection);
    return () => {
      window.removeEventListener("dock:new-preset", open);
      window.removeEventListener("dock:edit-preset", edit);
      window.removeEventListener("dock:review-preset-draft", review);
      window.removeEventListener("dock:open-settings", openSection);
    };
  }, []);
  // 拆到工作台的会话（会话头「单独面板」）；`benchOpen` = 会话区正在画工作台。
  const [bench, setBench] = useState<string[]>([]);
  const [benchOpen, setBenchOpen] = useState(false);
  // 右侧分屏里的会话（⌘ 点侧栏会话打开）；`null` = 没分屏。
  const [split, setSplit] = useState<string | null>(null);
  const [splitRatio, setSplitRatio] = useState(() => Number(localStorage.getItem(SPLIT_KEY)) || DEFAULT_SPLIT);
  const saveSplitRatio = (r: number) => localStorage.setItem(SPLIT_KEY, String(r));
  /** 右侧区域（快捷面板 / 面板栈）是否展开；收起时这一栏宽度过渡到 0。 */
  const [panelOpen, setPanelOpen] = useState(true);
  // 打开右侧的面板：先把右侧区域展开（收着用快捷键也要能开出来），再翻页。
  const showSide = useCallback(() => setPanelOpen(true), []);
  /** 右侧这一栏所在的行：分割条的比例按整行算（栏收进 wrapper 后不再按父元素算）。 */
  const rowRef = useRef<HTMLDivElement>(null);
  /**
   * 主行的实际宽度（像素）：右侧那一栏按比例算成像素宽度。
   * 不用百分比宽度：flex item 上的 `width: 58%` 在 WKWebView 里会塌成内容宽度，
   * 面板就只占内容那点、右边空一大片（左侧栏也是按像素给的）。
   */
  const [rowWidth, setRowWidth] = useState(0);
  useLayoutEffect(() => {
    const el = rowRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setRowWidth(el.clientWidth));
    ro.observe(el);
    setRowWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  /**
   * 和会话区并排的大面板，左→右一排（设计 SidePanelsSplit）；每种最多一个，开着时替掉右侧快捷面板。
   * `weights` 是各栏宽度权重（和为 1）；增减面板就均分。
   */
  const [panes, setPanes] = useState<{ kinds: SidePanel[]; weights: number[] }>({ kinds: [], weights: [] });
  const sidePanels = panes.kinds;
  /** 正在拖出窗口的那一栏（描蓝 + 提示）。 */
  const [poppingPanel, setPoppingPanel] = useState<SidePanel | null>(null);
  /** 对话流里的画布卡点进来要看哪块。 */
  const [canvasTarget, setCanvasTarget] = useState<{ canvasId: string | null; seq: number } | null>(null);
  /** 对话流里点了文件路径：文件面板要打开的文件（`seq` 变了就开）。 */
  const [fileTarget, setFileTarget] = useState<(PathTarget & { threadId: string; seq: number }) | null>(null);
  /** 子代理卡点进来要看的子代理（`seq` 变了面板就切过去）。 */
  const [subagentTarget, setSubagentTarget] = useState<{ agentId: string | null; seq: number } | null>(null);
  const [sideRatio, setSideRatio] = useState(() => Number(localStorage.getItem(SIDE_KEY)) || DEFAULT_SIDE);
  const saveSideRatio = (r: number) => localStorage.setItem(SIDE_KEY, String(r));
  /** 文件面板的快速打开（⌘P）开着没有。 */
  const [finding, setFinding] = useState(false);
  const [terminalOpen, setTerminalOpen] = useState(true);
  const [starting, setStarting] = useState(false);
  // 侧栏「+」创建项目的对话框。
  const [creatingProject, setCreatingProject] = useState(false);
  // 从项目进新对话：新对话页预填这个预设和目录（`key` 让新对话页按新值重来）。
  const [newChatSeed, setNewChatSeed] = useState<{ presetId: string; cwd: string; key: number } | null>(null);
  const newChatIn = (presetId: string, cwd: string) => {
    setNewChatSeed({ presetId, cwd, key: Date.now() });
    navigate(null);
  };
  // 侧栏项目右键「移除项目…」等确认的项目。
  const [removingProject, setRemovingProject] = useState<{ groupId: string; project: Project } | null>(null);
  // 侧栏右键「删除…」等确认的会话。
  const [deleting, setDeleting] = useState<string | null>(null);

  // 当前预设：GUI 自己记住（品牌行下拉），没选过就用 Dock 的默认。新对话用它，侧栏只列它的会话。
  // 每台网关一份 key，几台同时开着不会互相覆盖。
  const [savedPreset, setSavedPreset] = useState(() => localStorage.getItem(presetKey(gw)));
  // 网关一切，同一帧里换回那台的工作区（会话、面板、预设）。栏宽和终端留在这扇窗口上。
  const [boundGw, setBoundGw] = useState(gw);
  if (gw !== boundGw) {
    workspaceSnaps.set(boundGw, {
      history,
      subagentView,
      expanded: new Set(expanded),
      searching,
      navPage,
      bench: [...bench],
      benchOpen,
      split,
      panes,
      panelOpen,
      finding,
      canvasTarget,
      fileTarget,
      subagentTarget,
      newChatSeed,
      savedPreset,
    });
    const next = workspaceSnaps.get(gw) ?? freshWorkspace(gw);
    setHistory(next.history);
    setSubagentView(next.subagentView);
    setExpanded(next.expanded);
    setSearching(next.searching);
    setNavPage(next.navPage);
    setBench(next.bench);
    setBenchOpen(next.benchOpen);
    setSplit(next.split);
    setPanes(next.panes);
    setPanelOpen(next.panelOpen);
    setFinding(next.finding);
    setCanvasTarget(next.canvasTarget);
    setFileTarget(next.fileTarget);
    setSubagentTarget(next.subagentTarget);
    setNewChatSeed(next.newChatSeed);
    setSavedPreset(next.savedPreset);
    setSettings(null);
    setPresetEditor(false);
    setStudio(null);
    setCreatingProject(false);
    setRemovingProject(null);
    setDeleting(null);
    setPoppingPanel(null);
    setStarting(false);
    setBoundGw(gw);
  }
  const newChatPreset =
    (savedPreset && dock.presets.some((p) => p.id === savedPreset && p.available) ? savedPreset : null) ??
    dock.defaultPreset;
  const pickNewChatPreset = (id: string) => {
    localStorage.setItem(presetKey(gw), id);
    setSavedPreset(id);
  };
  /** 切到会话 / 项目所在的预设（侧栏才看得到它）；没登记 / 不可用的预设不切。 */
  const followPreset = (id: string | null | undefined) => {
    if (id && id !== newChatPreset && dock.presets.some((p) => p.id === id && p.available)) pickNewChatPreset(id);
  };
  // 设置页「常规 › 默认预设」改的是同一个 key。
  useEffect(() => {
    const onPick = (e: Event) => setSavedPreset((e as CustomEvent<string>).detail);
    window.addEventListener("dock:new-chat-preset", onPick);
    return () => window.removeEventListener("dock:new-chat-preset", onPick);
  }, []);

  const activeSession = dock.sessions.find((s) => s.id === activeSessionId) ?? null;
  const subagentState = useSubagents(activeSession?.id ?? null);
  const subagentsRunning = subagentState.order.filter((id) => subagentState.byId[id].info.status === "running").length;
  // 告诉 store 正在看哪个会话：它跑完不算通知，看了就从「刚完成」里移出。
  useEffect(() => store.focus([activeSessionId, split]), [activeSessionId, split, gw]);
  // 最近看过的会话的目录：新对话页默认归到这里。
  const [lastCwdSeen, setLastCwd] = useState("");
  useEffect(() => {
    if (activeSession) setLastCwd(activeSession.cwd);
  }, [activeSession]);
  // 没有最近会话可跟时用「常规 › 默认项目目录」。
  const projectDir = usePrefs().projectDir;
  const lastCwd = lastCwdSeen || projectDir;
  /** PR 页的项目下拉：侧栏「项目」里的目录（所有预设），同一目录只算一个。 */
  const prProjects = useMemo(() => {
    const seen = new Map<string, { name: string; path: string }>();
    for (const g of dock.groups) for (const p of g.projects) if (!seen.has(p.path)) seen.set(p.path, { name: p.name, path: p.path });
    return [...seen.values()];
  }, [dock.groups]);

  // 记住在看的会话；启动拿到会话列表后按偏好回到它（只做一次）。
  useEffect(() => {
    if (activeSessionId) localStorage.setItem(lastSessionKey(gw), activeSessionId);
  }, [activeSessionId, gw]);
  const restored = useRef(new Set<string>());
  useEffect(() => {
    if (restored.current.has(gw) || dock.sessions.length === 0) return;
    restored.current.add(gw);
    const last = localStorage.getItem(lastSessionKey(gw));
    if (prefs.get().restoreSession && last && !activeSessionId && dock.sessions.some((x) => x.id === last)) navigate(last);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dock.sessions, gw]);

  // 「网关与设备 › 允许网页连接」只作用在本机 Dock：每次连上按偏好再设一次。
  useEffect(() => {
    if (gw === "local" && dock.connection === "open" && !prefs.get().acceptPairing) void pairingAccept(false).catch(() => {});
  }, [dock.connection, gw]);

  // 选中的会话：切到它的预设、展开它的项目。放在 effect 里：新开的会话要等
  // `thread/list` 刷新后才出现在树里。只跟着换会话走，之后在下拉里换预设不会被拽回来。
  const revealKey = activeSession ? `${activeSession.presetId ?? ""}\n${activeSession.projectId}` : "";
  useEffect(() => {
    if (!revealKey) return;
    const [group, project] = revealKey.split("\n");
    followPreset(group);
    setExpanded((prev) => (prev.has(project) ? prev : new Set(prev).add(project)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revealKey]);

  const navigate = useCallback(
    (sessionId: string | null) => {
      // 看壳内的会话就从工作台视图回来（工作台里的格子照旧留着）；整页也收起。
      setBenchOpen(false);
      setNavPage(null);
      setSubagentView(null);
      if (sessionId === activeSessionId) return;
      // 要看的正是分屏里那个：挪到主面板，分屏关掉（同一个会话不开两份）。
      if (sessionId && sessionId === split) setSplit(null);
      setHistory(({ stack, index }) => ({ stack: [...stack.slice(0, index + 1), sessionId], index: index + 1 }));
      if (sessionId) store.select(sessionId);
    },
    [activeSessionId, split],
  );

  // 对话流里的画布卡「打开」：开画布面板，切到那个会话（卡片在分屏里时）和那块画布。
  useEffect(
    () =>
      store.onOpenCanvas((threadId, canvasId) => {
        navigate(threadId);
        showSide();
        setPanes((p) => frontPane(p, "canvas"));
        setCanvasTarget((t) => ({ canvasId, seq: (t?.seq ?? 0) + 1 }));
      }),
    [navigate, showSide, gw],
  );

  // 对话流里点了行内代码里的文件路径：开文件面板，切到那个会话，打开那个文件。
  useEffect(
    () =>
      store.onOpenFile((threadId, target) => {
        navigate(threadId);
        setFinding(false);
        showSide();
        setPanes((p) => frontPane(p, "files"));
        setFileTarget((t) => ({ ...target, threadId, seq: (t?.seq ?? 0) + 1 }));
      }),
    [navigate, showSide, gw],
  );

  // Agent 请用户在浏览器里接手（handoff_to_user）：当前会话有还没答的接手请求就打开浏览器
  // 面板（它跟着这个会话的活动标签页）。实时推来的、切过去时历史里才加载出来的都算；
  // 别的会话发来的不抢焦点，侧栏照样标「等你回答」。用户关掉面板后不再自己弹回来。
  const handoffPending = useSyncExternalStore(fg.subscribe, () =>
    activeSessionId ? fg.pendingHandoff(activeSessionId) : false,
  );
  useEffect(() => {
    if (handoffPending) setPanes((p) => frontPane(p, "browser"));
  }, [handoffPending, activeSessionId]);

  // 子代理卡 / 会话头「N 个子代理在跑」：开子代理面板，切到那个会话和那个子代理。
  useEffect(
    () =>
      store.onOpenSubagent((threadId, agentId) => {
        navigate(threadId);
        showSide();
        setPanes((p) => frontPane(p, "subagents"));
        setSubagentTarget((t) => ({ agentId, seq: (t?.seq ?? 0) + 1 }));
      }),
    [navigate, showSide, gw],
  );

  /** ⌘ 点会话：在右侧分屏打开（已经在主面板的不重复开）。 */
  const openSplit = (sessionId: string) => {
    if (sessionId === activeSessionId) return;
    setSplit(sessionId);
    store.select(sessionId);
  };
  /** 会话头「单独面板」：拆到工作台，会话区切到工作台视图。 */
  const detach = (id: string) => {
    setBench((b) => (b.includes(id) ? b : [...b, id]));
    setBenchOpen(true);
    if (id === split) setSplit(null);
  };
  const undock = (id: string) => setBench((b) => b.filter((x) => x !== id));
  const benchSessions = bench
    .map((id) => dock.sessions.find((s) => s.id === id))
    .filter((s): s is SessionSummary => !!s);
  // 工作台空了（收回、关掉、会话被删）就回壳内视图。
  useEffect(() => {
    if (benchSessions.length === 0) setBenchOpen(false);
  }, [benchSessions.length]);

  const splitSession = dock.sessions.find((s) => s.id === split) ?? null;
  // 分屏的会话被删了 / 不见了：收起分屏。
  useEffect(() => {
    if (split && dock.sessions.length > 0 && !splitSession) setSplit(null);
  }, [split, splitSession, dock.sessions.length]);

  const goto = (index: number) => {
    const target = history.stack[index];
    setHistory((h) => ({ ...h, index }));
    if (target) store.select(target);
  };

  const toggle = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  /** PR 页「让 Agent review」：用编码预设在那个项目开新会话，把 review 请求发进去。 */
  const agentReview = async (cwd: string, prompt: string) => {
    const presetId = dock.presets.some((p) => p.id === "code") ? "code" : newChatPreset;
    if (!presetId) return;
    const id = await store.startThread(cwd, presetId);
    if (!id) return;
    navigate(id);
    await store.send(id, prompt);
  };

  const start = async (presetId: string, cwd: string) => {
    setStarting(true);
    const id = await store.startThread(cwd, presetId);
    setStarting(false);
    if (id) navigate(id);
  };

  const toggleSidebar = useCallback(() => setSidebarCollapsed((v) => !v), []);
  // 终端是本机 shell：连远程 Dock 时不开（会话目录在远端）。
  const toggleTerminal = useCallback(() => !dock.remote && setTerminalOpen((v) => !v), [dock.remote]);
  const toggleBrowser = useCallback(() => {
    showSide();
    setPanes((p) => togglePane(p, "browser"));
  }, [showSide]);
  const toggleDesktop = useCallback(() => {
    showSide();
    setPanes((p) => togglePane(p, "desktop"));
  }, [showSide]);
  const toggleCanvas = useCallback(() => {
    showSide();
    setPanes((p) => togglePane(p, "canvas"));
  }, [showSide]);
  const toggleSubagents = useCallback(() => {
    showSide();
    setPanes((p) => togglePane(p, "subagents"));
  }, [showSide]);
  // 侧边聊天是功能面板（features/side-chat），跟着当前会话走。
  const toggleSideChat = useCallback(() => {
    showSide();
    setPanes((p) => togglePane(p, SIDE_CHAT));
  }, [showSide]);
  const toggleReview = useCallback(() => {
    showSide();
    setPanes((p) => togglePane(p, REVIEW));
  }, [showSide]);
  const toggleFiles = useCallback(() => {
    setFinding(false);
    showSide();
    setPanes((p) => togglePane(p, "files"));
  }, [showSide]);
  const addPanel = (kind: SidePanel) => {
    showSide();
    setPanes((p) => frontPane(p, kind));
  };
  const closePanel = (kind: SidePanel) => {
    if (kind === "files") setFinding(false);
    setPanes((p) => (p.kinds.includes(kind) ? evenPanes(p.kinds.filter((k) => k !== kind)) : p));
  };
  // 功能（插件面板等）的轻提示：会话区顶部居中，2 秒淡出。
  const [toast, setToast] = useState<{ text: string; key: number } | null>(null);
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 2000);
    return () => clearTimeout(t);
  }, [toast]);
  const featureHost: FeatureHost = {
    // 插件面板已经开成独立窗：提到前面，别在右侧再开一份。
    openPanel: (kind) => {
      if (!IN_TAURI || !panelFor(kind)?.poppable) return addPanel(kind as SidePanel);
      void invoke<boolean>("focus_panel_window", { panel: kind, gateway: gatewayArg() }).then(
        (focused) => focused || addPanel(kind as SidePanel),
        () => addPanel(kind as SidePanel),
      );
    },
    closePanel: (kind) => closePanel(kind as SidePanel),
    toast: (text) => setToast({ text, key: Date.now() }),
  };

  useEffect(() => {
    // 键位可在「设置 › 键盘快捷键」改（dock/shortcuts.ts）。
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      if (matches(e, "toggleSidebar")) {
        e.preventDefault();
        toggleSidebar();
      } else if (matches(e, "openSettings")) {
        e.preventDefault();
        setSettings((s) => (s ? null : "general"));
      } else if (matches(e, "toggleTerminal")) {
        e.preventDefault();
        toggleTerminal();
      } else if (matches(e, "toggleBrowser")) {
        e.preventDefault();
        toggleBrowser();
      } else if (matches(e, "toggleDesktop")) {
        e.preventDefault();
        toggleDesktop();
      } else if (matches(e, "toggleCanvas")) {
        e.preventDefault();
        toggleCanvas();
      } else if (matches(e, "toggleSideChat") && store.canSideChat()) {
        e.preventDefault();
        toggleSideChat();
      } else if (matches(e, "quickOpen")) {
        // 打开文件面板并弹快速打开（已经开着也弹）。
        e.preventDefault();
        setPanes((p) => frontPane(p, "files"));
        setFinding(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleSidebar, toggleTerminal, toggleBrowser, toggleDesktop, toggleCanvas, toggleSideChat]);

  /**
   * 折叠右侧这一栏的按钮：展开时它在右侧面板的头栏最右（跟分屏 / 新窗口 / 关闭同一组、同一基线），
   * 收起时它在会话顶栏右端 —— 两处都在窗口右上角同一个位置。
   */
  const sideToggle = useMemo(
    () => (
      <IconButton
        icon={PanelRight}
        label={panelOpen ? "收起侧边面板" : "展开侧边面板"}
        size={15}
        onClick={() => setPanelOpen((v) => !v)}
        className="size-7"
      />
    ),
    [panelOpen],
  );

  /** 右侧一栏：面板本身 + 头栏的拖手（拖出窗口）、分屏菜单、在新窗口打开。 */
  const renderPanel = (kind: SidePanel) => {
    const sessionId = activeSession?.id ?? null;
    const onClose = () => closePanel(kind);
    const lead = sessionId && (
      <PanelGrip
        kind={kind}
        sessionId={sessionId}
        onDragging={(on) => setPoppingPanel(on ? kind : null)}
        onPopOut={onClose}
      />
    );
    const controls = (
      <PanelControls
        open={sidePanels}
        onSplit={addPanel}
        onPopOut={sessionId ? () => void openPanelWindow(kind, sessionId).then((ok) => ok && onClose()) : null}
      />
    );
    if (kind === "browser")
      return <BrowserPanel sessionId={sessionId} onClose={onClose} lead={lead} controls={controls} />;
    if (kind === "desktop")
      return <DesktopPanel sessionId={sessionId} onClose={onClose} lead={lead} controls={controls} />;
    if (kind === "canvas")
      return <CanvasPanel sessionId={sessionId} target={canvasTarget} onClose={onClose} lead={lead} controls={controls} />;
    if (kind === "subagents")
      return <SubagentPanel sessionId={sessionId} target={subagentTarget} onClose={onClose} lead={lead} controls={controls} />;
    // 功能登记的面板（插件面板 `plugin:<id>`）。拖出 / 新窗口要 Rust 侧认这种 kind，没认的就不给。
    const feature = panelFor(kind);
    if (feature) {
      const Panel = feature.Panel;
      return (
        <Panel
          kind={kind}
          sessionId={sessionId}
          onClose={onClose}
          lead={feature.poppable ? lead : null}
          controls={feature.poppable ? controls : <PanelControls open={sidePanels} onSplit={addPanel} onPopOut={null} />}
        />
      );
    }
    return (
      <FilesPanel
        key={sessionId ?? "none"}
        sessionId={sessionId}
        cwd={activeSession?.cwd ?? null}
        target={fileTarget?.threadId === sessionId ? fileTarget : null}
        // 开过就清掉：面板换会话 / 关了再开会重挂，别把上一次点的又开一遍。
        onTargetHandled={() => setFileTarget(null)}
        finding={finding}
        onFinding={setFinding}
        onClose={onClose}
        lead={lead}
        controls={controls}
      />
    );
  };

  // 连接状态画在会话区顶部的横条里（ConnectionBanner）；侧栏只报连上之后的错误。
  const status = dock.connection === "open" ? dock.error : null;

  const sidebarProps: SidebarProps = {
    groups: dock.groups,
    activeSessionId,
    expanded,
    runningIds: dock.running,
    waitingIds: dock.waiting,
    finished: dock.finished,
    status,
    presets: dock.presets,
    newChatPreset,
    onSetNewChatPreset: pickNewChatPreset,
    notifications: (rail) => <NotificationMenu app={dock} onOpen={navigate} rail={rail} />,
    canGoBack: history.index > 0,
    canGoForward: history.index < history.stack.length - 1,
    onToggleSidebar: toggleSidebar,
    onBack: () => goto(Math.max(0, history.index - 1)),
    onForward: () => goto(Math.min(history.stack.length - 1, history.index + 1)),
    onNewChat: () => {
      setNewChatSeed(null);
      navigate(null);
    },
    onToggle: toggle,
    onSelectSession: (id, asSplit) => (asSplit ? openSplit(id) : navigate(id)),
    activeSubagent: subagentView,
    onSelectSubagent: (target) => {
      navigate(target.threadId);
      setSubagentView(target);
    },
    onSessionAction: (id, action) => {
      const session = dock.sessions.find((s) => s.id === id);
      if (action === "open") navigate(id);
      else if (action === "split") openSplit(id);
      else if (action === "detach") {
        store.select(id);
        detach(id);
      } else if (action === "window")
        invoke("open_chat_window", { sessionId: id, gateway: gatewayArg(), x: 0, y: 0 }).catch(store.reportError);
      else if (action === "copy-id") navigator.clipboard.writeText(id);
      else if (action === "finder") session && !dock.remote && openPath(session.cwd).catch(() => {});
      else if (action === "disconnect") store.disconnect(id);
      else if (action === "delete") setDeleting(id);
    },
    onRenameSession: (id, title) => store.rename(id, title),
    onCreateProject: () => setCreatingProject(true),
    onRenameProject: (groupId, project, name) => store.renameProject(groupId, project.path, name),
    onProjectAction: (groupId, project, action) => {
      if (action === "pin" || action === "unpin") store.pinProject(project.id, action === "pin");
      else if (action === "copy-path") navigator.clipboard.writeText(project.path);
      else if (action === "finder") !dock.remote && openPath(project.path).catch(store.reportError);
      else if (action === "disconnect-all") store.disconnectProject(project.id);
      else if (action === "remove") setRemovingProject({ groupId, project });
    },
    onNewChatIn: newChatIn,
    splitSessionId: split,
    onOpenSettings: () => setSettings("general"),
    onOpenPlugins: () => setSettings("plugins"),
    onOpenPage: (page) => {
      setBenchOpen(false);
      setNavPage(page);
    },
    activePage: navPage,
    onPrefetchPage: (page) => {
      const cwd = activeSession?.cwd ?? lastCwd ?? prProjects[0]?.path;
      if (page === "pull-requests" && cwd) prefetchPrList(cwd);
    },
    onNewPreset: () => setPresetEditor(true),
    searching,
    onSearch: (open) => {
      if (open) setSidebarCollapsed(false);
      setSearching(open);
    },
  };

  return (
    <FeatureHostContext.Provider value={featureHost}>
    <div className={`glass-window relative flex h-full w-full overflow-hidden bg-window ${settings ? "settings-open" : ""}`}>
      {/* 设置页盖在最上层：底下的会话、终端照常挂着，关掉回来什么都不丢。 */}
      {settings && (
        <div className="settings-layer absolute inset-0 z-40">
          <Settings section={settings} onSection={setSettings} onClose={() => setSettings(null)} />
        </div>
      )}
      {studio && (
        // glass-page：玻璃材质下编辑器是半透的，后面的侧栏、会话区先藏起来。
        <div className="glass-page absolute inset-0 z-50">
          <PresetStudio
            key={`${studio.id ?? "ai-draft"}:${studio.opened ?? 0}`}
            presetId={studio.id}
            aiDraft={studio.ai}
            onClose={() => setStudio(null)}
            onSaved={(id) => {
              if (studio.origin) {
                // 会话里起草的：留在会话里，只让那张草稿卡变成「已保存」。
                window.dispatchEvent(new CustomEvent("dock:preset-draft-saved", { detail: { origin: studio.origin, id } }));
                return;
              }
              // AI 草稿存成了新预设：设成新对话默认（同手动新建）。
              pickNewChatPreset(id);
              navigate(null);
            }}
            onOpenMcp={() => {
              setStudio(null);
              setSettings("mcp");
            }}
          />
        </div>
      )}
      {/* 侧栏：折叠 = 整列收起（宽度过渡到 0），收起的部分不再参与交互（inert）。
          收起后左上角只剩 App 固定画的那组窗口按钮（见 FloatingWindowControls）。 */}
      <div
        inert={sidebarCollapsed}
        aria-hidden={sidebarCollapsed || undefined}
        className={`shrink-0 overflow-hidden transition-[width] duration-[var(--dur-slow)] ease-[var(--ease-out)] ${
          sidebarCollapsed ? "w-0" : "w-[248px]"
        }`}
      >
        <Sidebar {...sidebarProps} />
      </div>
      {sidebarCollapsed && <FloatingWindowControls {...sidebarProps} />}
      {/* 第一次连 Dock 慢时盖住整窗；本机一般连得快，根本不出现。 */}
      <LaunchSplash app={dock} />
      {creatingProject && (
        <CreateProject
          presets={dock.presets}
          defaultPreset={newChatPreset}
          onCancel={() => setCreatingProject(false)}
          onCreate={(p) => {
            setCreatingProject(false);
            store.addProject(p);
            // 切到它的预设、展开到这个项目，新对话页预填好，直接能发第一条。
            followPreset(p.presetId);
            setExpanded((prev) => new Set(prev).add(projectId(p.presetId, p.cwd)));
            newChatIn(p.presetId, p.cwd);
          }}
        />
      )}
      {removingProject && (
        <RemoveProjectConfirm
          name={removingProject.project.name}
          sessions={removingProject.project.sessions.length}
          onCancel={() => setRemovingProject(null)}
          onConfirm={async () => {
            const { groupId, project } = removingProject;
            setRemovingProject(null);
            await store.removeProject(groupId, project.path);
            // 正在看的会话属于这个项目：回新对话页；分屏里的收起。
            if (project.sessions.some((s) => s.id === activeSessionId)) navigate(null);
            if (project.sessions.some((s) => s.id === split)) setSplit(null);
          }}
        />
      )}
      {deleting && (
        <DeleteConfirm
          onCancel={() => setDeleting(null)}
          onConfirm={async () => {
            const id = deleting;
            setDeleting(null);
            if (!(await store.remove(id))) return;
            // 删的正开着：主面板回新对话页，分屏 / 工作台里的收起。
            if (id === activeSessionId) navigate(null);
            if (id === split) setSplit(null);
            undock(id);
          }}
        />
      )}
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <div ref={rowRef} className="relative flex min-h-0 min-w-0 flex-1">
          <Fragment key={gw}>
          {presetEditor && (
            <PresetEditor
              presets={dock.presets}
              defaultBase={newChatPreset}
              onClose={() => setPresetEditor(false)}
              onCreated={(id) => {
                setPresetEditor(false);
                // 新预设设成新对话默认，回到新对话页就是选中它。
                pickNewChatPreset(id);
                navigate(null);
                // 建好直接进编辑器配人设、工具和子代理。
                setStudio({ id });
              }}
              onDrafted={(draft, description) => {
                // AI 起草的还没写盘：进编辑器审，保存时才建。
                setPresetEditor(false);
                setStudio({ id: null, ai: { draft, description } });
              }}
              onOpenModels={() => {
                setPresetEditor(false);
                setSettings("models");
              }}
            />
          )}
          {navPage === "pull-requests" && (
            <PullRequestsPage
              projects={prProjects}
              initialPath={activeSession?.cwd ?? lastCwd ?? null}
              onAgentReview={agentReview}
              insetLeft={sidebarCollapsed}
            />
          )}
          {navPage === "schedules" && (
            <SchedulesPage
              sessions={dock.sessions}
              onOpenSession={navigate}
              initialSessionId={activeSessionId}
              insetLeft={sidebarCollapsed}
            />
          )}
          {benchOpen ? (
            <Workbench
              sessions={benchSessions}
              windowControls={sidebarCollapsed ? sidebarProps : null}
              onDock={(id) => {
                undock(id);
                navigate(id);
              }}
              onClose={undock}
              onPopOut={undock}
            />
          ) : (
            <div className="relative flex min-w-0" style={{ flex: "1 1 0%" }}>
              {/* 会话列吃满剩余：右侧那一栏按像素给定宽度（见下面 wrapper 的 style），
                  这里若再按 sideRatio 分一份 grow，两边加起来不满 100%，右边会空一块。 */}
              {toast && (
                <div key={toast.key} className="toast-2s pointer-events-none absolute inset-x-0 top-14 z-20 flex justify-center">
                  <span className="rounded-lg bg-ink/90 px-3 py-1.5 text-xs font-medium text-white shadow-lg">{toast.text}</span>
                </div>
              )}
              <div className="flex min-w-0 flex-col" style={{ flex: split ? `${splitRatio} 1 0%` : "1 1 0%" }}>
                {subagentView && activeSession && subagentView.threadId === activeSession.id ? (
                  <SubagentPage
                    session={activeSession}
                    agentId={subagentView.agentId}
                    onBack={() => setSubagentView(null)}
                    windowControls={sidebarCollapsed ? sidebarProps : null}
                    panelOpen={panelOpen}
                    sideToggle={panelOpen ? undefined : sideToggle}
                  />
                ) : (
                <SessionPane
                  session={activeSession}
                  windowControls={sidebarCollapsed ? sidebarProps : null}
                  panelOpen={panelOpen}
                  sideToggle={panelOpen || split ? undefined : sideToggle}
                  onDetach={activeSession ? () => detach(activeSession.id) : undefined}
                  onDeleted={() => navigate(null)}
                  // 关左边：右边那个挪过来当主面板。
                  onClosePane={
                    split
                      ? () => {
                          const next = split;
                          setSplit(null);
                          navigate(next);
                        }
                      : undefined
                  }
                  empty={
                    <NewChat
                      key={newChatSeed?.key ?? 0}
                      presets={dock.presets}
                      defaultPreset={newChatSeed?.presetId ?? newChatPreset}
                      defaultCwd={newChatSeed?.cwd ?? lastCwd}
                      busy={starting}
                      onStart={start}
                      onNewPreset={() => setPresetEditor(true)}
                    />
                  }
                />
                )}
              </div>
              {split && splitSession && (
                <>
                  <SplitDivider onRatio={setSplitRatio} onCommit={saveSplitRatio} />
                  <div className="enter-fade flex min-w-0 flex-col" style={{ flex: `${1 - splitRatio} 1 0%` }}>
                    <SessionPane
                      session={splitSession}
                      windowControls={null}
                      panelOpen={panelOpen}
                      sideToggle={panelOpen ? undefined : sideToggle}
                      onDetach={() => detach(splitSession.id)}
                      onDeleted={() => setSplit(null)}
                      onClosePane={() => setSplit(null)}
                      showConnection={false}
                      empty={null}
                    />
                  </div>
                </>
              )}
            </div>
          )}
          {/* 右侧区域：没开具体面板时是快捷面板，开了是面板栈。收起 = 这一栏宽度过渡到 0、
              内容淡出（窗口右上角那个「侧边面板」按钮统一管，见 docs/MOTION.md「面板」）。 */}
          {!benchOpen && (
            <div
              inert={!panelOpen}
              aria-hidden={!panelOpen || undefined}
              className="flex min-w-0 shrink-0 overflow-hidden transition-[width] duration-[var(--dur-slow)] ease-[var(--ease-out)]"
              style={{ width: panelOpen ? (sidePanels.length ? Math.round(rowWidth * (1 - sideRatio)) : TOOL_PANEL_WIDTH) : 0 }}
            >
              {sidePanels.length > 0 ? (
                <>
                  <SplitDivider
                    measureRef={rowRef}
                    onRatio={setSideRatio}
                    onCommit={saveSideRatio}
                    defaultRatio={DEFAULT_SIDE}
                  />
                  {/* 上面一行标签（当前打开的面板，点一下翻过去），下面才是面板内容。 */}
                  <div className="side-stack flex min-w-0 w-full flex-1 flex-col border-l border-line-soft">
                    <SideTabs
                      kinds={sidePanels}
                      active={sidePanels[sidePanels.length - 1]}
                      meta={panelMeta}
                      badge={(kind) => (kind === "subagents" ? subagentsRunning : 0)}
                      onPick={addPanel}
                      sideToggle={sideToggle}
                    />
                    <PanelStack
                      kinds={sidePanels}
                      render={renderPanel}
                      meta={panelMeta}
                      badge={(kind) => (kind === "subagents" ? subagentsRunning : 0)}
                      overlay={(kind) => (poppingPanel === kind ? <PoppingOverlay /> : null)}
                      onFront={addPanel}
                    />
                  </div>
                </>
              ) : (
                <ToolPanel
                  terminalOpen={terminalOpen}
                  onToggleTerminal={toggleTerminal}
                  onToggleBrowser={toggleBrowser}
                  onToggleFiles={toggleFiles}
                  onToggleDesktop={toggleDesktop}
                  onToggleCanvas={toggleCanvas}
                  onToggleSubagents={toggleSubagents}
                  subagentsRunning={subagentsRunning}
                  onToggleSideChat={store.canSideChat() ? toggleSideChat : undefined}
                  onToggleReview={store.canReview() ? toggleReview : undefined}
                  // 入口行（插件面板、MCP 应用）：在右侧开一页，不弹独立窗。
                  onOpenFeature={(kind) => addPanel(kind as SidePanel)}
                  sideChatActive={!!activeSession && dock.asides.has(activeSession.id)}
                  open={panelOpen}
                  sideToggle={sideToggle}
                />
              )}
            </div>
          )}
          </Fragment>
        </div>
        <TerminalDock
          open={terminalOpen && !dock.remote}
          cwd={activeSession?.cwd ?? lastCwd ?? "~"}
          onClose={() => setTerminalOpen(false)}
        />
      </div>
    </div>
    </FeatureHostContext.Provider>
  );
}

/** 各栏等宽。 */
function evenPanes(kinds: SidePanel[]): { kinds: SidePanel[]; weights: number[] } {
  return { kinds, weights: kinds.map(() => 1 / kinds.length) };
}

/**
 * 翻到这一页（右侧面板叠成一摞，最后一个是摊开的那页）：开着的挪到最前，没开的加进来。
 * 登记成 `single` 的功能面板同前缀只留一页（两个「审查」合成一页，用面板里的下拉切）。
 */
function frontPane(p: { kinds: SidePanel[] }, kind: SidePanel) {
  const prefix = kind.includes(":") ? kind.slice(0, kind.indexOf(":")) : null;
  const single = prefix !== null && panelFor(kind)?.single;
  const rest = p.kinds.filter((k) => k !== kind && !(single && k.startsWith(`${prefix}:`)));
  return evenPanes([...rest, kind]);
}

/** 快捷键 / 快捷面板：每种面板一个开关。摊开着就合上，压在下面就翻到最前，没开就打开。 */
function togglePane(p: { kinds: SidePanel[] }, kind: SidePanel) {
  const prefix = kind.includes(":") ? kind.slice(0, kind.indexOf(":")) : null;
  const single = prefix !== null && panelFor(kind)?.single;
  const same = (k: SidePanel) => k === kind || (single && k.startsWith(`${prefix}:`));
  const open = p.kinds.filter(same);
  if (open.length && same(p.kinds[p.kinds.length - 1])) return evenPanes(p.kinds.filter((k) => !same(k)));
  return frontPane(p, open[open.length - 1] ?? kind);
}

/** 拖出窗口时盖在那一栏上：描蓝 + 提示（同工作台的格子）。 */
/** 标签行上的名字和图标：内置面板查 PANELS，功能面板查它登记的 label / icon。 */
function panelMeta(kind: SidePanel): SideTabMeta {
  const builtin = PANELS.find((p) => p.kind === kind);
  if (builtin) return { label: builtin.label, icon: builtin.icon };
  const feature = panelFor(kind);
  const label = typeof feature?.label === "function" ? feature.label(kind) : feature?.label;
  return { label: label || kind.split(":")[0], icon: feature?.icon ?? PanelRight };
}

function PoppingOverlay() {
  return (
    <>
      <div className="pointer-events-none absolute inset-0 z-10 rounded-lg ring-2 ring-inset ring-accent" />
      <div className="enter-fade pointer-events-none absolute inset-x-0 top-14 z-10 flex justify-center">
        <span className="rounded-full bg-ink/80 px-3 py-1 text-[11px] font-medium text-white shadow-lg">
          拖到窗口外松手，开成独立窗口
        </span>
      </div>
    </>
  );
}
