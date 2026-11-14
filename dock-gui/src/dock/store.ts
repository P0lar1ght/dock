// GUI 的全部 Dock 状态，放在 React 外面（`DockStore`），组件用 hooks 按需订阅：
// - `useApp()`：连接、预设、会话树、谁在跑、谁在等用户。只在这些变了时重渲染。
// - `useThread(id)`：一个会话的对话状态（dock-core 的 `ThreadState`）。流式增量
//   只让正在看它的组件重渲染。
// - `useEnv(id)`：这个会话的审批 / 模型 / 推理。
// - `useSubagents(id)`：这个会话派出的子代理（状态 + 各自的对话）。
// - `useWorkflows(id)`：这个会话起的工作流 run（阶段、子代理行、结果）。
//
// 事件语义全在 dock-core（`reduceThread` / `replayHistory`），这里只管拉取、订阅、
// 合并和动作。

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  EMPTY_SUBAGENTS,
  EMPTY_THREAD,
  EMPTY_WORKFLOWS,
  codeReviewOf,
  isRunning,
  parseEvent,
  pendingItems,
  reduceSubagents,
  reduceThread,
  reduceWorkflows,
  replayHistory,
  seedSubagents,
  seedWorkflows,
  seedTurnChanges,
  reduceTurnChanges,
  parseTurnChanges,
  EMPTY_TURN_CHANGES,
  withHistory,
  type DockEvent,
  type SubagentApproval,
  type SubagentsState,
  type ThreadState,
  type TurnChangesState,
  type WorkflowsState,
} from "dock-core";
import type {
  AiPresetDraft,
  CatalogTool,
  ComposerSettings,
  ContextBreakdown,
  GoalInfo,
  ImageInput,
  PresetDetail,
  PresetDraft,
  PresetGroup,
  PresetOption,
  SessionSummary,
  SlashCommand,
  SlashResult,
} from "../model";
import { RpcError, type ConnectionState, type DockClient } from "./client";
import type { Remote } from "./remote";
import { notify, prefs } from "./prefs";
import { savePrReview } from "./prReviews";
import { registerPresetIcons } from "../components/presetIcon";
import { prepareImage } from "../components/imageInput";
import type { PathTarget } from "../components/markdown/pathLink";

/** 输入框的外部填充：别的面板塞一段话、撤回消息时连图一起放回。 */
type ComposerFill = (text: string, mode: "replace" | "append", images?: ImageInput[]) => void;

/** 一题的回答（`interaction/respond`）：选中的选项（多选题可以多个），外加自己写的回答。 */
export interface QuestionAnswer {
  questionId: string;
  values: string[];
  other?: string;
}

interface WireThread {
  id: string;
  alias?: string;
  title: string;
  cwd?: string;
  open: boolean;
  presetId?: string | null;
  updatedAt?: number;
  subagentCount?: number;
  subagentsRunning?: number;
}

interface Environment {
  model?: {
    id?: string;
    label?: string;
    options?: { id: string; label: string; available?: boolean; authProvider?: string | null }[];
  };
  approval?: { mode?: string };
  reasoning?: { effort?: string; options?: string[] };
  plan?: { enabled?: boolean };
  goal?: {
    status?: string;
    summary?: string;
    progressSummary?: string;
    elapsedMs?: number;
    requests?: number;
    budget?: { requests?: number; ms?: number } | null;
  };
  context?: {
    estimatedTokens?: number;
    maxContextTokens?: number;
    usagePercent?: number;
    compactionTriggerPercent?: number;
  };
}

/** `environment.goal` → 界面用的目标；没有目标为 `null`。 */
function goalOf(g: Environment["goal"]): GoalInfo | null {
  const status = g?.status;
  if (status !== "active" && status !== "paused" && status !== "completed") return null;
  return {
    status,
    summary: g?.summary ?? "",
    progress: g?.progressSummary ?? "",
    elapsedMs: g?.elapsedMs ?? 0,
    requests: g?.requests ?? 0,
    budget: g?.budget ?? null,
    at: Date.now(),
  };
}

/** 一轮里多久最多重拉一次环境（上下文圆环、目标进展跟着工具调用走）。 */
const ENV_REFRESH_MS = 2000;

/** 恢复暂停的目标时替用户发的那句话（Dock 要一轮才能接着干）。 */
const GOAL_RESUME_MESSAGE = "继续完成目标。";

/** 发送方式：`auto` 空闲就发、在跑就排队；`stop` 停掉在跑的这一轮，立即发这一条。 */
export type SendMode = "auto" | "stop";

/**
 * 不进下拉的斜杠命令：/new、/resume 会换掉这一页的会话，和侧栏打架；
 * /help 由输入框自己处理（打开下拉就是帮助，Dock 的帮助文本还混着终端专用命令）。
 */
const SLASH_HIDDEN = new Set(["new", "resume", "help"]);

export interface AppState {
  connection: ConnectionState;
  error: string | null;
  presets: PresetOption[];
  defaultPreset: string | null;
  sessions: SessionSummary[];
  groups: PresetGroup[];
  /** 最后一轮还在跑的会话。 */
  running: ReadonlySet<string>;
  /** 有交互在等用户（权限 / 提问 / 计划 / elicitation）的会话。 */
  waiting: ReadonlySet<string>;
  /** 按了停止、还没等到这一轮结束的会话（停止键给反馈用）。 */
  stopping: ReadonlySet<string>;
  /** 不在看的时候跑完的会话（通知菜单的「刚完成」），打开它就移出。 */
  finished: ReadonlyMap<string, FinishedTurn>;
  /** 排队等本轮结束再发的消息（`turn/queue/list`），按会话。 */
  queued: ReadonlyMap<string, QueuedPrompt[]>;
  /** 正在拉历史的会话（会话区显示载入骨架）。 */
  loadingIds: ReadonlySet<string>;
  /** Dock 进程退出了：退出状态（如 `exit status: 1`）；重新拉起后清空。 */
  exited: string | null;
  /** 连上过又断了之后的重连次数；连上清零。 */
  reconnects: number;
  /** 连的是远程 Dock（`dock serve --remote`）就是那一条；本机为 `null`。 */
  remote: Remote | null;
  /** 不会自己好的连接失败（令牌无效 / 被撤销）：不再重连，等用户去设置里处理。 */
  fatal: string | null;
  /** 第一次连上后预设、会话列表都铺过了（失败也算，错误另报）：启动画面看它收起。 */
  booted: boolean;
  /** 主会话 → 它的侧边聊天（线程 id）。只在这次连接里有效，Dock 不落盘。 */
  asides: ReadonlyMap<string, string>;
  /** 主会话关掉 / Dock 重启后跟着结束的侧边聊天（按主会话记），面板显示「已结束」。 */
  asidesEnded: ReadonlySet<string>;
}

/** 设置页：一台 MCP 服务器（`mcp/list`）。 */
export interface McpServerInfo {
  name: string;
  status: "connected" | "failed" | "needs_auth" | "disabled";
  detail: string;
  toolCount: number;
  enabledToolCount: number;
  tools: { name: string; description: string; enabled: boolean }[];
  /** 写在 ~/.dock/config.toml 里（设置页能编辑 / 删除）。 */
  editable: boolean;
  /** Dock 内置的行（cua-driver）：没写进配置时只能开关。 */
  builtin: boolean;
}

/** 设置页：config.toml 里的一个模型（`model/list`）。 */
export interface ModelInfo {
  id: string;
  label: string;
  description: string;
  apiBase: string | null;
  contextWindow: number | null;
  backends: string[];
  auth: "key" | "env" | "none";
  /** 读哪个环境变量取密钥；本进程里有没有设。 */
  keyEnv: string | null;
  keyEnvSet: boolean;
  /** 写在 ~/.dock/config.toml 里（设置页能改）。 */
  editable: boolean;
  default: boolean;
  /** 连接继承自哪个供应商（`[provider.<id>]`）；老 Dock 没有这个字段。 */
  provider?: string | null;
  /** 走订阅通道的模型（`auth = "chatgpt"`）的 provider id；老 Dock 没有这个字段。 */
  authProvider?: string | null;
  /** 订阅通道自动发现的（登录后从上游拉的，不在 config 里）；老 Dock 没有这个字段。 */
  discovered?: boolean;
}

/** 排队 / 插话消息带的图（还没送达：像素按摘要向 Dock 要）。 */
export interface QueuedImage {
  digest: string;
  mimeType: string;
  width: number;
  height: number;
}

export interface QueuedPrompt {
  id: string;
  message: string;
  /** `queue` 本轮结束后单独发；`steer` 已改成插话，等下一个步骤边界并进这一轮。 */
  kind: "queue" | "steer";
  /** 这条消息带的图。等待送达期间也要显示缩略图。 */
  images: QueuedImage[];
}

/** 一轮在后台结束了：完成或出错（自己按的停止不算）。 */
export interface FinishedTurn {
  status: "completed" | "failed";
  error: string | null;
  elapsedMs: number;
  at: number;
}

/** 侧栏「+」建的项目（预设 + 目录）。Dock 没有项目这个实体、空会话也不落盘，GUI 自己记住。 */
export interface LocalProject {
  presetId: string;
  cwd: string;
  /** 自己起的名字；空 = 用文件夹名。 */
  name: string;
}

const PROJECTS_KEY = "dock-gui.projects";
/** 置顶的项目 / 从侧栏移除的项目（都是会话树里的项目 id）。 */
const PINNED_KEY = "dock-gui.projects.pinned";
const REMOVED_KEY = "dock-gui.projects.removed";

/**
 * 项目名单按连接分开存：路径属于哪台机器就只在连那台时出现。本机沿用原来的 key，
 * 远程 Dock 加 `@<连接 id>` 后缀。每份 DockStore 构造时定死，几台同时开着也不会互相覆盖。
 */

/** 侧栏怎么摆项目：本地建的、置顶的、移除的。 */
export interface ProjectPrefs {
  projects: LocalProject[];
  pinned: ReadonlySet<string>;
  removed: ReadonlySet<string>;
}

/** Dock 一个进程最多开几页（cordis-tui `MAX_TABS`，含 `dock serve` 起步的第 1 页）。 */
const MAX_PAGES = 9;

export const NO_ENV: ComposerSettings = {
  approval: "ask",
  model: "",
  modelId: "",
  models: [],
  reasoning: "none",
  reasoningOptions: [],
  plan: false,
  goal: null,
  context: null,
};

export class DockStore {
  /** 远程连接 id；本机是空串（localStorage 不加后缀）。 */
  readonly scope: string;
  private app: AppState;
  private wire: WireThread[] = [];
  private projects: LocalProject[];
  private pinned: Set<string>;
  private removed: Set<string>;
  /** 没有界面订阅时也留着，后台网关才不会停掉 `start`。 */
  private retained: (() => void) | null = null;

  constructor(
    readonly client: DockClient,
    remote: Remote | null,
  ) {
    this.scope = remote?.id ?? "";
    this.projects = this.readProjects();
    this.pinned = this.readIds(PINNED_KEY);
    this.removed = this.readIds(REMOVED_KEY);
    this.app = {
      connection: client.state,
      error: null,
      presets: [],
      defaultPreset: null,
      sessions: [],
      groups: [],
      running: new Set(),
      waiting: new Set(),
      stopping: new Set(),
      finished: new Map(),
      queued: new Map(),
      loadingIds: new Set(),
      exited: null,
      reconnects: 0,
      remote,
      fatal: client.fatal,
      booted: false,
      asides: new Map(),
      asidesEnded: new Set(),
    };
  }

  /** 没有界面订阅也保持连接（后台网关）。 */
  retain() {
    if (!this.retained) this.retained = this.subscribe(() => {});
  }

  /** 关掉这条连接的套接字，并丢掉保活订阅。 */
  release() {
    this.retained?.();
    this.retained = null;
    this.client.close();
  }

  /** 名单里的名字晚到了（独立窗）：只更新显示，不换存储范围。 */
  setRemote(remote: Remote | null) {
    this.setApp({ remote });
  }

  private scoped(key: string) {
    return this.scope ? `${key}@${this.scope}` : key;
  }

  private readIds(key: string): Set<string> {
    try {
      const raw = JSON.parse(localStorage.getItem(this.scoped(key)) ?? "[]");
      return new Set(Array.isArray(raw) ? raw.filter((x) => typeof x === "string") : []);
    } catch {
      return new Set();
    }
  }

  private readProjects(): LocalProject[] {
    try {
      const raw = JSON.parse(localStorage.getItem(this.scoped(PROJECTS_KEY)) ?? "[]");
      return Array.isArray(raw) ? raw.filter((p) => typeof p?.presetId === "string" && typeof p?.cwd === "string") : [];
    } catch {
      return [];
    }
  }
  private threads = new Map<string, ThreadState>();
  private env = new Map<string, ComposerSettings>();
  private envTimers = new Map<string, number>();
  /** 正在拉历史的会话：期间推来的事件先攒着，拉完按 seq 合进去。 */
  private loading = new Map<string, DockEvent[]>();
  private subscribed = new Set<string>();
  /** 侧边聊天 → 它的主会话。侧边聊天不进 `thread/list`，开着与否看这里。 */
  private asideParent = new Map<string, string>();
  /** 正在看的会话（`focus`，分屏时两个）；它们跑完不算通知。 */
  private viewing = new Set<string>();
  /** 每个会话最后一次在看的时间：页数满了先关最久没看的（`makeRoom`）。 */
  private seenAt = new Map<string, number>();
  /** 每个会话开始等用户的时间（通知菜单写「N 分钟前」）。 */
  private waitingAt = new Map<string, number>();
  /** 本窗口发出去的图的本地预览：`线程 id#用户消息 id` → object URL。事件只带尺寸不带像素。 */
  private previews = new Map<string, string[]>();
  /** 发出去、还没等到 `item/user_message` 的预览，按线程排队（一条消息一组）。 */
  private pendingPreviews = new Map<string, string[][]>();
  private slashCache: SlashCommand[] | null = null;
  private listeners = new Set<() => void>();
  private started = false;
  /** 别的面板要往某个会话的输入框里填字（浏览器面板的示例）。 */
  private fills = new Map<string, Set<ComposerFill>>();
  private canvasOpeners = new Set<(threadId: string, canvasId: string | null) => void>();
  private fileOpeners = new Set<(threadId: string, target: PathTarget) => void>();
  private canvas: CanvasState = { shown: null, errors: new Map() };
  /** 每个会话派出的子代理（只在 Dock 内存里，会话开着才有）。 */
  private subagents = new Map<string, SubagentsState>();
  /** 用户在子代理面板里发出去的话（按 agent id）：子代理的对话里区分「你说」和「主代理说」。 */
  private sentToSubagent = new Map<string, string[]>();
  private subagentOpeners = new Set<(threadId: string, agentId: string | null) => void>();
  /** 每个会话起的工作流 run（只在 Dock 内存里，会话开着才有）。 */
  private workflows = new Map<string, WorkflowsState>();
  /** 每个会话每轮改了哪些文件（Dock 落盘；会话开着才拉）。 */
  private turnChanges = new Map<string, TurnChangesState>();

  // ---- 订阅（给 useSyncExternalStore） ----

  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    this.start();
    return () => this.listeners.delete(fn);
  };
  getApp = () => this.app;
  getThread = (id: string | null) => (id && this.threads.get(id)) || EMPTY_THREAD;
  getEnv = (id: string | null) => (id && this.env.get(id)) || NO_ENV;
  getSubagents = (id: string | null) => (id && this.subagents.get(id)) || EMPTY_SUBAGENTS;
  getWorkflows = (id: string | null) => (id && this.workflows.get(id)) || EMPTY_WORKFLOWS;
  getTurnChanges = (id: string | null) => (id && this.turnChanges.get(id)) || EMPTY_TURN_CHANGES;

  private emit() {
    this.listeners.forEach((fn) => fn());
  }

  private setApp(patch: Partial<AppState>) {
    this.app = { ...this.app, ...patch };
    this.emit();
  }

  /**
   * 往会话的输入框填一段话（不发送）：`replace` 换掉草稿，`append` 接在后面。给了 `images` 就连图一起换掉
   * （撤回消息时）。没有输入框在听就什么也不做。
   */
  fillComposer = (id: string, text: string, mode: "replace" | "append" = "replace", images?: ImageInput[]) =>
    this.fills.get(id)?.forEach((fn) => fn(text, mode, images));

  onFill = (id: string, fn: ComposerFill) => {
    const set = this.fills.get(id) ?? new Set();
    set.add(fn);
    this.fills.set(id, set);
    return () => {
      set.delete(fn);
      if (set.size === 0) this.fills.delete(id);
    };
  };

  // ---- 文件面板 ----

  /** 对话流里点了文件路径：App 开文件面板、切到那个会话、打开那个文件。 */
  openFile = (threadId: string, target: PathTarget) => this.fileOpeners.forEach((fn) => fn(threadId, target));

  onOpenFile = (fn: (threadId: string, target: PathTarget) => void) => {
    this.fileOpeners.add(fn);
    return () => {
      this.fileOpeners.delete(fn);
    };
  };

  // ---- 浏览器接手 ----

  /** 这个会话有没有还没答的浏览器接手请求（切过去时据此打开浏览器面板）。 */
  pendingHandoff = (id: string): boolean =>
    (this.threads.get(id)?.turns ?? []).some((t) => t.items.some((i) => i.kind === "question" && i.browserHandoff && !i.resolved));

  /** 会话的工作目录（名册里的）；不认识的线程（侧边聊天的旁问线程等）是 `null`。 */
  sessionCwd = (id: string): string | null => this.app.sessions.find((s) => s.id === id)?.cwd ?? null;

  // ---- 画布 ----

  /** 对话流里的画布卡要打开画布面板：App 在听。`canvasId` 为空就开面板看最新的。 */
  openCanvas = (threadId: string, canvasId: string | null) => this.canvasOpeners.forEach((fn) => fn(threadId, canvasId));

  onOpenCanvas = (fn: (threadId: string, canvasId: string | null) => void) => {
    this.canvasOpeners.add(fn);
    return () => {
      this.canvasOpeners.delete(fn);
    };
  };

  getCanvas = () => this.canvas;

  /** 画布面板正在显示哪块（`线程#画布 id`），卡片据此标「右侧已展开」。 */
  setCanvasShown = (key: string | null) => {
    if (this.canvas.shown === key) return;
    this.canvas = { ...this.canvas, shown: key };
    this.emit();
  };

  /** 画布页面跑出来的报错（只在面板里看得到），卡片和面板共用。`null` 清掉。 */
  setCanvasError = (key: string, error: CanvasRuntimeError | null) => {
    const prev = this.canvas.errors.get(key) ?? null;
    if (prev === error || (prev && error && prev.version === error.version && prev.message === error.message)) return;
    const errors = new Map(this.canvas.errors);
    if (error) errors.set(key, error);
    else errors.delete(key);
    this.canvas = { ...this.canvas, errors };
    this.emit();
  };

  /** 给 store 外的动作（开窗之类）报错用：显示在侧栏状态行。 */
  reportError = (e: unknown) => this.fail(e);

  private fail = (e: unknown) => {
    const message = e instanceof RpcError ? e.body.message : e instanceof Error ? e.message : String(e);
    this.setApp({ error: message });
  };

  // ---- 启动与连接 ----

  private start() {
    if (this.started) return;
    this.started = true;
    let wasOpen = false;
    this.client.onState((s) => {
      if (s === "open") this.setApp({ connection: s, error: null, exited: null, reconnects: 0 });
      else if (s === "closed" && wasOpen) this.setApp({ connection: s, reconnects: this.app.reconnects + 1, fatal: this.client.fatal });
      else this.setApp({ connection: s, fatal: this.client.fatal });
      // 第一次连上（启动时没连上、后来重连成功的也算）：铺预设和会话列表。
      if (s === "open" && !wasOpen) this.initialLoad().catch(this.fail);
      // 断线重连：推送可能漏了，Dock 也可能重启过（seq 重新编号）——已加载的会话整份重拉。
      // Dock 可能重启过（换了版本）：命令表重拉。
      if (s === "open") this.slashCache = null;
      if (s === "open" && wasOpen) this.reloadLoaded();
      if (s === "open") wasOpen = true;
    });
    // 本机 Dock 进程退出 / 被重新拉起。远程连接没有这个进程，也不听换连接的整页重载。
    if (!this.client.gatewayId) {
      listen<string>("dock://exit", (e) => this.setApp({ exited: e.payload || "未知" })).catch(() => {});
      listen("dock://ready", () => this.setApp({ exited: null })).catch(() => {});
    }
    this.client.onNotification((n) => {
      const id = typeof n.params.threadId === "string" ? n.params.threadId : "";
      const event = id ? parseEvent(n.method, n.params) : null;
      if (!event) return;
      // 子代理的推送不属于会话自己的时间线，也不进拉历史时的缓冲。
      if (event.method === "subagent/updated" || event.method === "subagent/event") {
        this.applySubagent(id, event);
        return;
      }
      if (event.method === "workflow/updated") {
        this.applyWorkflow(id, event);
        return;
      }
      if (event.method === "turn/changes/updated") {
        this.turnChanges.set(id, reduceTurnChanges(this.getTurnChanges(id), event));
        this.emit();
        return;
      }
      const buffer = this.loading.get(id);
      if (buffer) buffer.push(event);
      else this.apply(id, event);
      // 一轮结束：标题 / 更新时间可能变了。
      // 预设也重拉：模型可能刚在这一轮里写了一份预设文件，`preset/list` 每次现读盘。
      if (event.method === "turn/completed") {
        this.refreshThreads().catch(() => {});
        this.refreshPresets().catch(() => {});
      }
      // 上下文圆环、目标条跟着走。
      if (event.method === "turn/started" || event.method === "turn/completed") this.envSoon(id, true);
      else if (event.method === "context/compacted" && event.progress.status !== "running") this.envSoon(id, true);
      else if (event.method === "item/tool_completed") this.envSoon(id);
      // 队列没有推送：一轮开始 / 结束时（排队的消息可能刚被发出去）重新拉。
      if ((event.method === "turn/started" || event.method === "turn/completed") && this.app.queued.has(id)) {
        this.refreshQueue(id);
      }
    });
    // 连接已经开着（热更新换了 store）时收不到 `open` 事件：连上后再补一次。
    this.client
      .connect()
      .then(() => this.initialLoad())
      .catch(this.fail);
  }

  private initialLoaded = false;

  /** 第一次连上：铺预设和会话列表（只做一次，重连走 `reloadLoaded`）。 */
  private async initialLoad() {
    if (this.initialLoaded) return;
    this.initialLoaded = true;
    try {
      await this.refreshPresets();
      await this.refreshThreads();
    } catch (e) {
      this.initialLoaded = false;
      throw e;
    } finally {
      if (!this.app.booted) this.setApp({ booted: true });
    }
  }

  private applySubagent(id: string, event: DockEvent) {
    const before = this.getSubagents(id);
    const after = reduceSubagents(before, event);
    if (after === before) return;
    this.subagents.set(id, after);
    this.emit();
  }

  private applyWorkflow(id: string, event: DockEvent) {
    const before = this.getWorkflows(id);
    const after = reduceWorkflows(before, event);
    if (after === before) return;
    this.workflows.set(id, after);
    this.emit();
    // 审完一个 PR：批注按「项目 + PR 号」存一份，PR 页的差异视图之后也看得到（run 重启就没了）。
    if (event.method === "workflow/updated" && event.run.status === "completed") {
      const review = codeReviewOf(event.run);
      const cwd = this.sessionCwd(id);
      if (review && review.pr > 0 && cwd) {
        savePrReview(cwd, review.pr, {
          at: Date.now(),
          threadId: id,
          runId: event.run.runId,
          diffCommand: review.diffCommand,
          findings: review.findings,
        });
      }
    }
  }

  /** 开着的会话铺一次每轮的文件改动。旧 Dock 没有这个方法就当没有。 */
  private async loadTurnChanges(id: string) {
    if (!this.client.capabilities.turnChanges) return;
    try {
      const r = await this.client.call<{ turns: Record<string, unknown>[] }>("turn/changes/list", { threadId: id });
      this.turnChanges.set(id, seedTurnChanges(r.turns));
      this.emit();
    } catch {
      // 页刚关掉：保持原样。
    }
  }

  /** 一轮改动的统一 diff。 */
  turnDiff = (threadId: string, turnId: string) =>
    this.client.call<{ diff: string }>("turn/changes/diff", { threadId, turnId }).then((r) => r.diff);

  /** 项目 git 仓库相对 HEAD 的改动 + 未跟踪的新文件。不是 git 仓库时 `available: false`。 */
  worktreeDiff = (threadId: string) =>
    this.client.call<{ available: boolean; diff?: string; truncated?: boolean; root?: string; hint?: string; reason?: string; untrackedOmitted?: number }>(
      "vcs/worktree/diff",
      { threadId },
    );

  /**
   * 撤销一轮的文件改动：还是那一轮改完样子的文件恢复成原文，之后又被改过的不动（列在
   * `conflicts`）。成功后 Dock 推一条 `turn/changes/updated`；这里也按回包先改上。
   */
  revertTurn = async (threadId: string, turnId: string) => {
    const r = await this.client.call<{ restored: string[]; conflicts: { path: string; reason: string }[]; changes: Record<string, unknown> }>(
      "turn/changes/revert",
      { threadId, turnId },
    );
    const changes = parseTurnChanges(r.changes);
    if (changes) {
      const state = this.getTurnChanges(threadId);
      this.turnChanges.set(threadId, { byTurn: { ...state.byTurn, [turnId]: changes } });
      this.emit();
    }
    return { restored: r.restored, conflicts: r.conflicts };
  };

  /** 开着的会话铺一次工作流 run 的现状。旧 Dock 没有这个方法就当没有 run。 */
  private async loadWorkflows(id: string) {
    if (!this.client.capabilities.workflows) return;
    try {
      const r = await this.client.call<{ runs: Record<string, unknown>[] }>("workflow/list", { threadId: id });
      this.workflows.set(id, seedWorkflows(this.getWorkflows(id), r.runs));
      this.emit();
    } catch {
      // 页刚关掉：保持原样。
    }
  }

  /** 开着的会话铺一次子代理的现状（推送只报变化）。旧 Dock 没有这个方法就当没有子代理。 */
  private async loadSubagents(id: string) {
    try {
      const r = await this.client.call<{ agents: Record<string, unknown>[] }>("subagent/list", { threadId: id });
      this.subagents.set(id, seedSubagents(this.getSubagents(id), r.agents));
      this.emit();
    } catch {
      // 不支持子代理的 Dock、或页刚关掉：保持原样。
    }
  }

  private apply(id: string, event: DockEvent) {
    if (event.method === "item/user_message" && event.attachments.length > 0) {
      const urls = this.pendingPreviews.get(id)?.shift();
      if (urls) this.previews.set(`${id}#u${event.seq}`, urls);
    }
    const before = this.threads.get(id) ?? EMPTY_THREAD;
    const after = reduceThread(before, event);
    if (after === before) return;
    this.threads.set(id, after);
    if (event.method === "turn/completed" && !event.steered && this.app.stopping.has(id)) {
      const stopping = new Set(this.app.stopping);
      stopping.delete(id);
      this.app = { ...this.app, stopping };
    }
    // 被插话截开的那一段不是真的结束：这一轮接着跑，不通知、不算「刚完成」。
    if (event.method === "turn/completed" && event.steered) {
      this.syncFlags(id, after);
      this.emit();
      return;
    }
    // 侧边聊天在右侧面板里看着：不发系统通知、不进「刚完成」。
    const aside = this.asideParent.has(id);
    // 系统通知（常规 › 通知）：只看实时推来的结束；在不在前台由偏好决定。
    if (event.method === "turn/completed" && event.status !== "cancelled" && !aside) {
      void notify("done", this.titleOf(id), event.status === "failed" ? `出错了：${event.error ?? "这一轮失败"}` : "这一轮完成了");
    }
    // 只看实时推来的结束（拉历史不走这里），自己按的停止不算。
    if (event.method === "turn/completed" && !this.viewing.has(id) && event.status !== "cancelled" && !aside) {
      const turn = after.turns[after.turns.length - 1];
      const finished = new Map(this.app.finished).set(id, {
        status: event.status,
        error: event.error ?? null,
        elapsedMs: turn ? event.at - turn.startedAt : 0,
        at: event.at || Date.now(),
      });
      this.app = { ...this.app, finished };
    }
    const wasWaiting = this.app.waiting.has(id);
    this.syncFlags(id, after);
    if (!wasWaiting && this.app.waiting.has(id) && !aside) void notify("input", this.titleOf(id), "等你审批或回答问题");
    this.emit();
  }

  private titleOf(id: string): string {
    return this.app.sessions.find((x) => x.id === id)?.title ?? "Dock";
  }

  /** 会话级标记（跑 / 等）只在翻转时换新集合，侧栏才不会跟着每个增量重渲染。 */
  private syncFlags(id: string, state: ThreadState) {
    const flip = (set: ReadonlySet<string>, on: boolean) => {
      if (set.has(id) === on) return set;
      const next = new Set(set);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    };
    const running = flip(this.app.running, isRunning(state));
    const waiting = flip(this.app.waiting, pendingItems(state).length > 0);
    if (waiting.has(id) && !this.waitingAt.has(id)) this.waitingAt.set(id, Date.now());
    if (!waiting.has(id)) this.waitingAt.delete(id);
    if (running !== this.app.running || waiting !== this.app.waiting) {
      this.app = { ...this.app, running, waiting };
    }
  }

  // ---- 会话列表 ----

  private async refreshThreads() {
    const r = await this.client.call<{ threads: WireThread[] }>("thread/list", { scope: "all" });
    this.wire = r.threads;
    this.rebuildSessions();
  }

  private rebuildSessions() {
    const label = (id: string | null | undefined) =>
      this.app.presets.find((p) => p.id === id)?.label ?? id ?? "未记录预设";
    const sessions: SessionSummary[] = this.wire
      // 第 1 页是 `dock serve` 起步时的那一页，GUI 不用它开会话。
      .filter((t) => t.cwd && t.alias !== "live")
      .map((t) => ({
        id: t.id,
        projectId: projectId(t.presetId ?? null, t.cwd!),
        // 还没说过话的会话，网关给的是占位标题「当前会话」。
        title: t.title && t.title !== "当前会话" ? t.title : "新对话",
        preset: label(t.presetId),
        presetId: t.presetId ?? null,
        cwd: t.cwd!,
        open: t.open,
        // 开着的页 `updatedAt` 是 0（还没落盘）：当作最新，排在最上面。
        updatedAt: t.open && !t.updatedAt ? Number.MAX_SAFE_INTEGER : t.updatedAt ?? 0,
        subagentCount: t.subagentCount ?? 0,
        subagentsRunning: t.subagentsRunning ?? 0,
      }));
    this.setApp({ sessions, groups: groupSessions(sessions, this.app.presets, {
        projects: this.projects,
        pinned: this.pinned,
        removed: this.removed,
      }) });
  }

  private isOpen(id: string) {
    if (this.asideParent.has(id)) return true;
    return this.wire.find((t) => t.id === id)?.open ?? false;
  }

  // ---- 加载会话 ----

  /**
   * 拉一个会话的历史并替换它的状态。开着的先订阅再拉，期间的推送攒着、拉完按 seq
   * 合进去（dock-core 去重）。关着的会话网关也给 events（从落盘回放），同一条路。
   */
  private async load(id: string) {
    const open = this.isOpen(id);
    this.loading.set(id, []);
    this.setApp({ loadingIds: new Set(this.loading.keys()) });
    try {
      if (open && !this.subscribed.has(id)) {
        this.subscribed.add(id);
        await this.client.subscribe(id);
      }
      const h = await this.client.call<{ events: Record<string, unknown>[] }>("thread/history", { threadId: id });
      let state = replayHistory(h.events);
      for (const e of this.loading.get(id) ?? []) state = reduceThread(state, e);
      this.threads.set(id, state);
      this.syncFlags(id, state);
    } finally {
      this.loading.delete(id);
      this.setApp({ loadingIds: new Set(this.loading.keys()) });
    }
    if (open) await Promise.all([this.loadEnv(id), this.loadSubagents(id), this.loadWorkflows(id), this.loadTurnChanges(id)]);
  }

  private async loadEnv(id: string) {
    const env = await this.client.call<Environment>("thread/environment/get", { threadId: id });
    this.env.set(id, {
      approval: env.approval?.mode ?? "ask",
      model: env.model?.label ?? env.model?.id ?? "",
      modelId: env.model?.id ?? "",
      models: (env.model?.options ?? [])
        .filter((o) => o.available !== false)
        .map(({ id, label, authProvider }) => ({ id, label, authProvider })),
      reasoning: env.reasoning?.effort ?? "none",
      reasoningOptions: env.reasoning?.options ?? ["none", "low", "medium", "high"],
      plan: env.plan?.enabled ?? false,
      goal: goalOf(env.goal),
      context: env.context?.maxContextTokens
        ? {
            used: env.context.estimatedTokens ?? 0,
            total: env.context.maxContextTokens,
            percent: env.context.usagePercent ?? 0,
            triggerPercent: env.context.compactionTriggerPercent ?? 85,
          }
        : null,
    });
    this.emit();
  }

  /**
   * 一轮当中占用和目标进展会变（工具结果进历史、模型报 `update_goal`）：节流着重拉环境。
   * 一轮开始 / 结束、压缩结束时立刻拉。
   */
  private envSoon(id: string, now = false) {
    if (!this.isOpen(id)) return;
    const pending = this.envTimers.get(id);
    if (pending !== undefined) {
      if (!now) return;
      window.clearTimeout(pending);
    }
    const timer = window.setTimeout(
      () => {
        this.envTimers.delete(id);
        this.loadEnv(id).catch(() => {});
      },
      now ? 0 : ENV_REFRESH_MS,
    );
    this.envTimers.set(id, timer);
  }

  private reloadLoaded() {
    this.refreshThreads()
      .then(() => {
        // Dock 可能重启过：主会话没开着了，它的侧边聊天也就没了（只活在 Dock 内存里）。
        for (const parent of [...this.app.asides.keys()]) if (!this.isOpen(parent)) this.forgetAside(parent, true);
        // Dock 可能重启过：已经不开着的会话不再算订阅着，load 会按新状态处理。
        for (const id of [...this.subscribed]) if (!this.isOpen(id)) this.subscribed.delete(id);
        // 子代理只活在 Dock 内存里：重启过就全没了，断线期间的变化也可能漏了。整份重铺。
        this.subagents.clear();
        this.workflows.clear();
        return Promise.all([...this.threads.keys()].map((id) => this.load(id)));
      })
      .catch(this.fail);
  }

  // ---- 动作 ----

  /**
   * 点开一个会话。关着的直接开页：Dock 按会话记下的模型 / 推理强度恢复，
   * 一打开输入框就有真实设置可看可切（不开页拿不到 environment）。
   */
  select = (id: string) => (this.isOpen(id) ? this.load(id) : this.ensureOpen(id)).catch(this.fail);

  /** 正在看哪些会话（主面板 + 分屏；`null` = 新对话页）。看了就从「刚完成」里移出。 */
  focus = (ids: (string | null)[]) => {
    const now = Date.now();
    for (const id of this.viewing) this.seenAt.set(id, now);
    this.viewing = new Set(ids.filter((id): id is string => !!id));
    for (const id of this.viewing) this.seenAt.set(id, now);
    const seen = [...this.viewing].filter((id) => this.app.finished.has(id));
    if (seen.length) {
      const finished = new Map(this.app.finished);
      for (const id of seen) finished.delete(id);
      this.setApp({ finished });
    }
  };

  /** 这个会话从什么时候开始等用户；没在等为 `null`。 */
  waitingSince = (id: string) => this.waitingAt.get(id) ?? null;

  startThread = async (cwd: string, presetId: string): Promise<string | null> => {
    try {
      const r = await this.client.call<{ thread: WireThread }>("thread/start", { cwd, presetId });
      // 新对话默认值（常规 › 新对话）：和 Dock 默认一样的就不发。
      const p = prefs.get();
      if (p.approval === "auto") await this.client.call("thread/approval/set", { threadId: r.thread.id, mode: "auto" }).catch(() => {});
      if (!p.thinking) await this.client.call("thread/reasoning/set", { threadId: r.thread.id, effort: "none" }).catch(() => {});
      // 在移除过的项目里开了新对话：项目回到侧栏，不然新会话看不见。
      if (this.removed.delete(projectId(presetId, cwd))) localStorage.setItem(this.scoped(REMOVED_KEY), JSON.stringify([...this.removed]));
      await this.refreshThreads();
      await this.load(r.thread.id);
      return r.thread.id;
    } catch (e) {
      this.fail(e);
      return null;
    }
  };

  /**
   * 线程级方法只认开着的页（否则网关回 `thread_not_open`）。关着的会话（历史记录、
   * Dock 重启后原先开着的页）先开页再操作；同时来的几个操作共用一次开页。
   */
  private opening = new Map<string, Promise<void>>();
  private ensureOpen(id: string): Promise<void> {
    if (this.isOpen(id)) return Promise.resolve();
    let p = this.opening.get(id);
    if (!p) {
      p = (async () => {
        await this.makeRoom();
        await this.client.call("thread/open", { threadId: id });
        await this.refreshThreads();
        // 开页后网关按落盘重建投影，seq 重新编号：旧状态作废，整份重拉。
        // 子代理只活在原来那一页的内存里，一起作废。
        this.threads.delete(id);
        this.subagents.delete(id);
        this.workflows.delete(id);
        await this.load(id);
      })().finally(() => this.opening.delete(id));
      this.opening.set(id, p);
    }
    return p;
  }

  /**
   * Dock 一个进程最多开 {@link MAX_PAGES} 页。满了就关掉最久没看的一页腾位置：
   * 只挑有对话（关掉不丢，下次点开再连）、没在跑、没在等你、没在看的；
   * 从没看过的（别处开的、GUI 重载前开的）排最前。
   * 挑不出来就照开，让 Dock 报「最多 9 页」。
   */
  private async makeRoom() {
    const open = this.wire.filter((t) => t.open);
    if (open.length < MAX_PAGES) return;
    const candidates = open
      .filter(
        (t) =>
          t.alias !== "live" &&
          !this.viewing.has(t.id) &&
          !this.opening.has(t.id) &&
          // 关主会话会连它的侧边聊天一起关掉。
          !this.app.asides.has(t.id) &&
          // 网关给还没说过话的页标题「当前会话」；GUI 重载后没拉过记录也认得出。
          t.title !== "当前会话",
      )
      .sort((a, b) => (this.seenAt.get(a.id) ?? 0) - (this.seenAt.get(b.id) ?? 0));
    for (const t of candidates) {
      // 没拉过记录的（GUI 重载前开的、别处开的）不知道在不在跑：先拉一次再判断，
      // 关掉正在跑的页会把那一轮停掉。
      if (!this.threads.has(t.id)) await this.load(t.id);
      if (isRunning(this.getThread(t.id)) || this.app.waiting.has(t.id)) continue;
      await this.closePage(t.id);
      return;
    }
  }

  /**
   * 断开：关掉那一页，记录留在磁盘，下次点开再连。已经拉下来的记录和设置留着，
   * 正在看的话画面不变。空会话 Dock 关掉不落盘，等于删了：菜单不给空会话这一项。
   */
  disconnect = (id: string) => this.closePage(id).catch(this.fail);

  private async closePage(id: string) {
    await this.client.call("thread/close", { threadId: id });
    this.forgetAside(id, true);
    this.subscribed.delete(id);
    await this.refreshThreads();
  }

  /**
   * 发一条消息，可带最多 4 张图（先 `imageInputs/put` 换成 id，两分钟内有效，所以发之前才传）。
   * `auto`：空闲就发，在跑就排队；`stop`：停掉在跑的这一轮，立即发这一条（在跑时的 turn/start）。
   * 插话不从这里发：先排队，再在排队条上点「立即插话」（steerQueued）。
   * 成功回 true；失败显示在侧栏状态行、回 false（输入框据此决定清不清空）。
   */
  send = async (
    id: string,
    text: string,
    images: ImageInput[] = [],
    mode: SendMode = "auto",
    intent?: Record<string, unknown>,
  ): Promise<boolean> => {
    try {
      await this.ensureOpen(id);
      const imageInputs = [];
      for (const img of images) {
        const r = await this.client.call<{ imageInputId: string; digest: string }>("imageInputs/put", {
          workspaceId: "default",
          threadId: id,
          mimeType: img.mimeType,
          digest: img.digest,
          dataBase64: img.base64,
          width: img.width,
          height: img.height,
          byteLength: img.byteLength,
        });
        imageInputs.push({ id: r.imageInputId, digest: r.digest });
      }
      const busy = isRunning(this.getThread(id));
      const method = busy && mode === "auto" ? "turn/enqueue" : "turn/start";
      if (images.length > 0) {
        const queue = this.pendingPreviews.get(id) ?? [];
        queue.push(images.map((i) => i.url));
        this.pendingPreviews.set(id, queue);
      }
      const r = await this.client.call<{ status?: string }>(method, {
        threadId: id,
        message: text,
        ...(imageInputs.length ? { imageInputs } : {}),
        ...(intent ? { intent } : {}),
      });
      if (r?.status === "queued") await this.refreshQueue(id);
      return true;
    } catch (e) {
      this.fail(e);
      return false;
    }
  };

  /** 这条用户消息的图在本窗口的预览（本窗口发的才有）；没有回 `null`。 */
  imagePreviews = (threadId: string, itemId: string) => this.previews.get(`${threadId}#${itemId}`) ?? null;

  // ---- 斜杠命令 ----

  /** 输入框下拉用的命令表（连上后拉一次）。只留这里能跑的，GUI 自己有入口的去掉。 */
  slashCommands = async (): Promise<SlashCommand[]> => {
    if (!this.slashCache) {
      const r = await this.client.call<{ commands: SlashCommand[] }>("slash/list");
      this.slashCache = r.commands.filter((c) => c.surface === "gateway" && !SLASH_HIDDEN.has(c.name));
    }
    return this.slashCache;
  };

  /**
   * 跑一条斜杠命令（整行原样给 Dock）。结果归成四种：发出去了 / 填回输入框 / 提示一段话 /
   * 不是命令（当普通消息发）。设置类命令跑完重读环境（模型、计划、强度可能变了）。
   */
  runSlash = async (id: string, text: string): Promise<SlashResult | null> => {
    try {
      await this.ensureOpen(id);
      const r = await this.client.call<{
        kind: string;
        fill?: string;
        menu?: string;
        notice?: { title?: string; body?: string };
      }>("slash/execute", { threadId: id, text });
      await this.loadEnv(id).catch(() => {});
      switch (r.kind) {
        case "submitted":
          return { kind: "submitted" };
        case "filled":
          return { kind: "filled", fill: r.fill ?? "" };
        case "passthrough":
          return { kind: "passthrough" };
        case "notice":
        case "applied":
          return { kind: "notice", title: r.notice?.title ?? "", body: r.notice?.body ?? "" };
        case "menu":
          return await this.menuNotice(id, r.menu ?? "");
        default:
          return { kind: "notice", title: "", body: `Dock 回了不认识的结果：${r.kind}` };
      }
    } catch (e) {
      this.fail(e);
      return null;
    }
  };

  /** Dock 让客户端「打开某个菜单」：GUI 没有对应面板，就把那份信息写成一段提示。 */
  private async menuNotice(id: string, menu: string): Promise<SlashResult> {
    if (menu === "model") return { kind: "notice", title: "模型", body: "用输入框右下角的模型菜单切换。" };
    const env = await this.client.call<Environment>("thread/environment/get", { threadId: id });
    if (menu === "context") {
      const c = env.context ?? {};
      const used = (c.estimatedTokens ?? 0).toLocaleString();
      const total = (c.maxContextTokens ?? 0).toLocaleString();
      return { kind: "notice", title: "上下文", body: `已用 ${used} / ${total} tokens（${c.usagePercent ?? 0}%）` };
    }
    if (menu === "goal") {
      const g = env.goal ?? {};
      if (!g.status || g.status === "none") return { kind: "notice", title: "目标", body: "没有目标。用 /goal <目标> 设一个。" };
      const state = g.status === "paused" ? "已暂停" : "进行中";
      const progress = g.progressSummary ? `\n进度：${g.progressSummary}` : "";
      return { kind: "notice", title: `目标（${state}）`, body: `${g.summary ?? ""}${progress}` };
    }
    return { kind: "notice", title: "", body: `这个命令要打开「${menu}」面板，桌面端还没有。` };
  }

  setPlan = (id: string, enabled: boolean) => this.setEnv(id, "thread/plan/set", { enabled });

  // ---- 目标 / 上下文 ----

  /** 把这句话立成目标并发出去（在跑就排队，轮到它时才立）。 */
  startGoal = async (id: string, text: string, images: ImageInput[] = []) => {
    const ok = await this.send(id, text, images, "auto", { goal: { operation: "start", objective: text } });
    if (ok) this.envSoon(id, true);
    return ok;
  };

  /**
   * 对话里某句话的「设为目标」：在跑就直接立目标（这一轮结束后接着干，不重发）；
   * 空闲就把这句话当目标再发一次，开一轮去做。
   */
  /**
   * 撤回 `turnId` 那条用户消息：它和之后的对话在 Dock 里全部删掉（含落盘），正文和图片放回输入框。
   * 在跑的话 Dock 先停下这一轮。投影重建、seq 重新编号，所以整份重拉。失败回中文原因。
   */
  rewind = async (id: string, turnId: string): Promise<string | null> => {
    try {
      await this.ensureOpen(id);
      const r = await this.client.call<{ message: string; images: { mimeType: string; dataBase64: string }[] }>(
        "thread/rewind",
        { threadId: id, turnId },
      );
      this.threads.delete(id);
      this.subagents.delete(id);
      this.workflows.delete(id);
      this.turnChanges.delete(id);
      await this.load(id);
      const images = await Promise.all(
        r.images.map((img) => {
          const bytes = Uint8Array.from(atob(img.dataBase64), (c) => c.charCodeAt(0));
          return prepareImage(new Blob([bytes], { type: img.mimeType }));
        }),
      );
      this.fillComposer(id, r.message, "replace", images);
      return null;
    } catch (e) {
      return e instanceof Error ? e.message : String(e);
    }
  };

  setGoalFrom = async (id: string, text: string) => {
    if (isRunning(this.getThread(id))) await this.setEnv(id, "thread/goal/set", { content: text });
    else await this.startGoal(id, text);
  };

  pauseGoal = (id: string) => this.setEnv(id, "thread/goal/pause", {});

  /** 继续暂停的目标：空闲时开一轮让它接着干；在跑就只把目标接上，这一轮结束后自己续。 */
  resumeGoal = async (id: string) => {
    if (isRunning(this.getThread(id))) {
      await this.runSlash(id, "/goal resume");
      return;
    }
    if (await this.send(id, GOAL_RESUME_MESSAGE, [], "auto", { goal: { operation: "resume" } })) this.envSoon(id, true);
  };

  clearGoal = (id: string) => this.setEnv(id, "thread/goal/clear", {});

  editGoal = (id: string, content: string) => this.setEnv(id, "thread/goal/edit", { content });

  /** 上下文圆环点开的明细（`thread/context/get`）。 */
  contextBreakdown = async (id: string): Promise<ContextBreakdown> => {
    const r = await this.client.call<{
      model?: string;
      usedTokens?: number;
      maxContextTokens?: number;
      usagePercent?: number;
      compactionTriggerPercent?: number;
      compactionTriggerTokens?: number;
      compactionCount?: number;
      slices?: ContextBreakdown["slices"];
      onDemand?: ContextBreakdown["onDemand"];
    }>("thread/context/get", { threadId: id });
    return {
      model: r.model ?? "",
      used: r.usedTokens ?? 0,
      total: r.maxContextTokens ?? 0,
      percent: r.usagePercent ?? 0,
      triggerPercent: r.compactionTriggerPercent ?? 85,
      triggerTokens: r.compactionTriggerTokens ?? 0,
      compactions: r.compactionCount ?? 0,
      slices: r.slices ?? [],
      onDemand: r.onDemand ?? [],
    };
  };

  // ---- 本地项目 ----

  /** 记下一个项目（同预设同目录的已有就只改名字；移除过的重新显示）；回它在会话树里的 id。 */
  addProject = (project: LocalProject): string => {
    const id = projectId(project.presetId, project.cwd);
    const rest = this.projects.filter((p) => !(p.presetId === project.presetId && p.cwd === project.cwd));
    this.projects = [project, ...rest];
    this.removed.delete(id);
    this.saveProjects();
    return id;
  };

  /** 项目改名（只改侧栏显示，目录不动）；空名字 = 回到文件夹名。 */
  renameProject = (presetId: string, cwd: string, name: string) => {
    const known = this.projects.find((p) => p.presetId === presetId && p.cwd === cwd);
    if (known) known.name = name;
    else this.projects = [{ presetId, cwd, name }, ...this.projects];
    this.saveProjects();
  };

  pinProject = (id: string, on: boolean) => {
    if (on) this.pinned.add(id);
    else this.pinned.delete(id);
    this.saveProjects();
  };

  /**
   * 从侧栏移除项目：本地记的删掉，落过盘的会话照旧留在磁盘上（搜索找得到），
   * 只是这个项目不再显示。开着的会话先断开。用「+」再建同一个项目就回来。
   */
  removeProject = async (presetId: string, cwd: string) => {
    const id = projectId(presetId, cwd);
    await this.disconnectProject(id);
    this.projects = this.projects.filter((p) => !(p.presetId === presetId && p.cwd === cwd));
    this.pinned.delete(id);
    this.removed.add(id);
    this.saveProjects();
  };

  /** 断开一个项目下所有开着的会话（腾 Dock 的页数）。在跑 / 在等你的不动。 */
  disconnectProject = async (id: string) => {
    const open = this.app.sessions.filter(
      (s) => s.projectId === id && s.open && !this.app.running.has(s.id) && !this.app.waiting.has(s.id),
    );
    for (const s of open) await this.closePage(s.id).catch(this.fail);
  };

  private saveProjects() {
    localStorage.setItem(this.scoped(PROJECTS_KEY), JSON.stringify(this.projects));
    localStorage.setItem(this.scoped(PINNED_KEY), JSON.stringify([...this.pinned]));
    localStorage.setItem(this.scoped(REMOVED_KEY), JSON.stringify([...this.removed]));
    this.rebuildSessions();
  }

  // ---- 预设 ----

  private refreshPresets = async () => {
    const p = await this.client.call<{ presets: PresetOption[]; defaultId: string }>("preset/list");
    registerPresetIcons(p.presets);
    this.setApp({ presets: p.presets, defaultPreset: p.defaultId });
    this.rebuildSessions();
  };

  /** 在某个项目里能选的预设（多出它 `.dock/presets` 里的项目层）。 */
  presetsAt = async (cwd: string) => {
    const p = await this.client.call<{ presets: PresetOption[] }>("preset/list", { cwd });
    registerPresetIcons(p.presets);
    return p.presets;
  };

  /** 新建自定义预设（写 ~/.dock/presets）；回新预设的 id。 */
  createPreset = async (name: string, icon: string, basedOn: string | null): Promise<string> => {
    const r = await this.client.call<{ preset: PresetOption }>("preset/create", {
      name,
      icon,
      ...(basedOn ? { basedOn } : {}),
    });
    await this.refreshPresets();
    return r.preset.id;
  };

  /** 一个预设的整份定义（编辑器用）。 */
  getPreset = async (id: string) => (await this.client.call<{ preset: PresetDetail }>("preset/get", { id })).preset;

  /** 整份写回；内置预设写成用户层覆盖。校验不过抛 Dock 给的原因。 */
  updatePreset = async (id: string, preset: PresetDraft) => {
    const agents = preset.agents.map(({ builtin: _b, fresh: _f, ...a }) => a);
    await this.client.call("preset/update", { id, preset: { ...preset, agents } });
    await this.refreshPresets();
  };

  /** 预设能选的工具（不含 MCP）。 */
  toolCatalog = async () => (await this.client.call<{ tools: CatalogTool[] }>("tool/catalog")).tools;

  /** AI 起草整份预设（Dock 用默认模型采样一次，不写盘）。`icons` 是图标库，模型只能从里面选。 */
  draftPreset = async (description: string, icons: string[]) =>
    (await this.client.call<{ draft: AiPresetDraft }>("preset/draft", { description, icons })).draft;

  /** AI 润色 / 扩写角色提示词。 */
  rewritePersona = async (persona: string, mode: "polish" | "expand", description: string) =>
    (await this.client.call<{ persona: string }>("preset/rewrite", { persona, mode, description })).persona;

  /** AI 按描述推荐工具（都在 tool/catalog 里）。 */
  suggestTools = async (description: string, persona: string) =>
    (await this.client.call<{ tools: { name: string; reason: string }[] }>("preset/suggestTools", { description, persona }))
      .tools;

  /** 预设编辑器旁的侧聊：带上之前的问答、当前草稿和所在步骤，回参考答案（不改草稿、不写盘）。 */
  askPreset = async (question: string, history: AskTurn[], draft: PresetDraft, step: string) =>
    (await this.client.call<{ answer: string }>("preset/ask", { question, history, draft, step })).answer;

  /** 删除自定义预设（内置的 Dock 会拒）。 */
  deletePreset = async (id: string) => {
    await this.client.call("preset/delete", { id });
    await this.refreshPresets();
  };

  // ---- 设置页 ----

  mcpList = async () => (await this.client.call<{ servers: McpServerInfo[] }>("mcp/list")).servers;

  /** 重读配置里的 MCP 服务器；回一行摘要和新状态。 */
  mcpReload = async () => {
    const r = await this.client.call<{ summary: string; servers: McpServerInfo[] }>("mcp/reload");
    return { summary: r.summary, servers: r.servers };
  };

  /** 重连一台；连不上抛 Dock 给的原因。 */
  mcpReconnect = (name: string) => this.client.call("mcp/reconnect", { name });

  modelList = async () => (await this.client.call<{ models: ModelInfo[] }>("model/list")).models;

  serverInfo = () => this.client.serverInfo;

  /** 按标题和用户消息搜落盘会话（`thread/search`）：会话 id → 命中片段（只命中标题为 null）。 */
  search = async (query: string): Promise<Map<string, string | null>> => {
    const r = await this.client.call<{ hits?: { threadId: string; snippet?: string | null }[] }>("thread/search", {
      query,
      limit: 50,
    });
    return new Map((r.hits ?? []).map((h) => [h.threadId, h.snippet ?? null]));
  };

  /** 改名：开着和关着的会话都行。 */
  rename = async (id: string, title: string) => {
    await this.client.call("thread/rename", { threadId: id, title }).catch(this.fail);
    await this.refreshThreads().catch(this.fail);
  };

  /**
   * 删除会话记录（从磁盘移除）。开着的先关掉（网关不删开着的页）；跑着的一轮不等，
   * 由 thread/close 收尾。
   */
  remove = async (id: string): Promise<boolean> => {
    try {
      // 没有对话内容的会话关掉时不落盘（Dock 的规则），关掉就等于删了。
      const open = this.isOpen(id);
      const empty = this.getThread(id).turns.length === 0;
      if (open) await this.client.call("thread/close", { threadId: id });
      if (!(open && empty)) await this.client.call("thread/delete", { threadId: id });
    } catch (e) {
      this.fail(e);
      return false;
    }
    this.threads.delete(id);
    this.subagents.delete(id);
    this.workflows.delete(id);
    this.subscribed.delete(id);
    this.forgetAside(id, true);
    const finished = new Map(this.app.finished);
    finished.delete(id);
    this.setApp({ finished });
    await this.refreshThreads().catch(this.fail);
    return true;
  };

  // ---- 侧边聊天 ----

  /** 网关认不认侧边聊天（`sideChat`）。 */
  canSideChat = () => !!this.client.capabilities.sideChat;
  /** Dock 认 `vcs/worktree/diff`（右侧「审查」面板）。 */
  canReview = () => !!this.client.capabilities.worktreeDiff;

  /**
   * 开（或回到）主会话的侧边聊天：从主会话分叉的只读旁问，带着它的上下文，不打断它。
   * 每个主会话最多一个；只活在 Dock 内存里，关面板 / 退出就没了。
   */
  startAside = async (parentId: string): Promise<string | null> => {
    try {
      await this.ensureOpen(parentId);
      const r = await this.client.call<{ thread: WireThread }>("thread/aside/start", { threadId: parentId });
      const asideId = r.thread.id;
      this.asideParent.set(asideId, parentId);
      const asidesEnded = new Set(this.app.asidesEnded);
      asidesEnded.delete(parentId);
      this.setApp({ asides: new Map(this.app.asides).set(parentId, asideId), asidesEnded });
      await this.load(asideId);
      return asideId;
    } catch (e) {
      this.fail(e);
      return null;
    }
  };

  /** 在主会话的侧边聊天里发一条（还没开就先开）。在跑时排队，同主会话的 Enter。 */
  sendAside = async (parentId: string, text: string, images: ImageInput[] = []): Promise<boolean> => {
    const asideId = this.app.asides.get(parentId) ?? (await this.startAside(parentId));
    return asideId ? this.send(asideId, text, images, "auto") : false;
  };

  /** 结束主会话的侧边聊天（关掉那一页，内容不保存）。 */
  closeAside = async (parentId: string) => {
    const asideId = this.app.asides.get(parentId);
    if (asideId) await this.client.call("thread/close", { threadId: asideId }).catch(() => {});
    this.forgetAside(parentId, false);
  };

  /** 「已结束」看过了：面板回到空态。 */
  dismissAsideEnded = (parentId: string) => {
    if (!this.app.asidesEnded.has(parentId)) return;
    const asidesEnded = new Set(this.app.asidesEnded);
    asidesEnded.delete(parentId);
    this.setApp({ asidesEnded });
  };

  /** 侧边聊天整理一段写进主线的笔记（调一次模型，只起草不写）。失败抛出，面板自己显示。 */
  draftSideNote = async (asideId: string): Promise<string> =>
    (await this.client.call<{ note: string }>("thread/aside/handback", { threadId: asideId })).note;

  /** 把（用户确认 / 改过的）笔记写进主会话：在跑就下一步并入，闲着落进历史；不开新的一轮。 */
  mergeSideNote = async (parentId: string, note: string): Promise<"nextStep" | "history"> =>
    (await this.client.call<{ delivery: "nextStep" | "history" }>("thread/aside/merge", { threadId: parentId, note })).delivery;

  /** 主会话的侧边聊天不在了（主会话关掉 / 删除、Dock 重启）：`ended` 让面板显示「已结束」。 */
  private forgetAside(parentId: string, ended: boolean) {
    const asideId = this.app.asides.get(parentId);
    if (!asideId) return;
    this.asideParent.delete(asideId);
    this.threads.delete(asideId);
    this.subscribed.delete(asideId);
    const asides = new Map(this.app.asides);
    asides.delete(parentId);
    const asidesEnded = new Set(this.app.asidesEnded);
    if (ended) asidesEnded.add(parentId);
    else asidesEnded.delete(parentId);
    this.setApp({ asides, asidesEnded });
  }

  // ---- 子代理 ----

  /** 对话流里的子代理卡要打开子代理面板：App 在听。`agentId` 为空就开列表。 */
  openSubagent = (threadId: string, agentId: string | null) => this.subagentOpeners.forEach((fn) => fn(threadId, agentId));

  onOpenSubagent = (fn: (threadId: string, agentId: string | null) => void) => {
    this.subagentOpeners.add(fn);
    return () => {
      this.subagentOpeners.delete(fn);
    };
  };

  /** 拉一个子代理的完整对话（中途接入时不全）。拿不到就保持原样。 */
  loadSubagentHistory = async (threadId: string, agentId: string) => {
    try {
      const h = await this.client.call<{ agent: Record<string, unknown>; events: Record<string, unknown>[] }>("subagent/history", {
        threadId,
        agentId,
      });
      this.subagents.set(threadId, withHistory(this.getSubagents(threadId), agentId, h));
      this.emit();
    } catch (e) {
      this.fail(e);
    }
  };

  /**
   * 侧栏展开一个会话的子代理：还没铺过就拉一次 `subagent/list`。关着的会话 Dock 从
   * 落盘读（`persisted`），不用先开页。
   */
  ensureSubagents = (threadId: string) => {
    if (this.subagents.has(threadId) || this.subagentLoads.has(threadId)) return;
    this.subagentLoads.add(threadId);
    void this.loadSubagents(threadId).finally(() => this.subagentLoads.delete(threadId));
  };
  private subagentLoads = new Set<string>();

  /**
   * 用户对子代理说一句：在跑就在下一步读到，空闲就开下一轮；已经结束的（或 Dock
   * 重启前的）Dock 用同一个 id 接着聊——那要挂到开着的页上，关着的会话先开页。
   */
  sendToSubagent = async (threadId: string, agentId: string, text: string): Promise<boolean> => {
    try {
      if (!this.isOpen(threadId)) await this.ensureOpen(threadId);
      this.sentToSubagent.set(agentId, [...(this.sentToSubagent.get(agentId) ?? []), text.trim()]);
      await this.client.call("subagent/send", { threadId, agentId, message: text });
      return true;
    } catch (e) {
      this.fail(e);
      return false;
    }
  };

  /** 这句话是不是用户在面板里发给这个子代理的（否则是主代理发的）。 */
  sentByUser = (agentId: string, text: string) => this.sentToSubagent.get(agentId)?.includes(text.trim()) ?? false;

  interruptSubagent = (threadId: string, agentId: string) =>
    this.client.call("subagent/interrupt", { threadId, agentId }).catch(this.fail);

  stopSubagent = (threadId: string, agentId: string) => this.client.call("subagent/stop", { threadId, agentId }).catch(this.fail);

  /**
   * 单独给一个子代理设审批档位（`inherit` 跟这一页 / `ask` / `auto`）。活着的 Dock 会推
   * `subagent/updated`；只在盘上的没有推送，按回包改。会话关着先开页（档位记在那一页上）。
   */
  setSubagentApproval = async (threadId: string, agentId: string, mode: SubagentApproval) => {
    try {
      if (!this.isOpen(threadId)) await this.ensureOpen(threadId);
      const r = await this.client.call<{ approval?: SubagentApproval }>("subagent/approval/set", { threadId, agentId, mode });
      const state = this.getSubagents(threadId);
      const sub = state.byId[agentId];
      if (sub && r.approval && sub.info.approval !== r.approval) {
        this.subagents.set(threadId, { ...state, byId: { ...state.byId, [agentId]: { ...sub, info: { ...sub.info, approval: r.approval } } } });
        this.emit();
      }
    } catch (e) {
      this.fail(e);
    }
  };

  /** 停下一条在跑的工作流 run（连同它派的子代理）。 */
  stopWorkflow = (threadId: string, runId: string) => this.client.call("workflow/stop", { threadId, runId }).catch(this.fail);

  /** 网关认不认插话（`turnSteer`）：旧版 Dock 的 turn/steer 是停掉再发，不给「立即插话」。 */
  canSteer = () => !!this.client.capabilities.turnSteer;

  /** 把一条排队的消息改成插话：Dock 在下一个步骤边界把它并进正在跑的这一轮。 */
  steerQueued = async (id: string, queueId: string) => {
    await this.client.call("turn/queue/steer", { threadId: id, queueId }).catch(this.ignoreClosed);
    await this.refreshQueue(id);
  };

  /** 取消一条排队的消息（或还没送达的插话）。 */
  unqueue = async (id: string, queueId: string) => {
    await this.client.call("turn/queue/remove", { threadId: id, queueId }).catch(this.ignoreClosed);
    await this.refreshQueue(id);
  };

  private refreshQueue = async (id: string) => {
    const r = await this.client
      .call<{ items?: { id: unknown; message?: string; kind?: string; images?: QueuedImage[] }[] }>(
        "turn/queue/list",
        { threadId: id },
      )
      .catch(() => null);
    const items = (r?.items ?? []).map((q) => ({
      id: String(q.id),
      message: q.message ?? "",
      kind: q.kind === "steer" ? ("steer" as const) : ("queue" as const),
      // 老网关没有这个字段：没有图就没有图，不报错。
      images: (Array.isArray(q.images) ? q.images : []).filter((im) => im && typeof im.digest === "string"),
    }));
    const queued = new Map(this.app.queued);
    if (items.length) queued.set(id, items);
    else queued.delete(id);
    this.setApp({ queued });
  };

  /** 手动压缩（压缩卡失败时的「重试」）。进展经 `context/compacted` 推回来。 */
  compact = async (id: string) => {
    await this.client.call("thread/context/compact", { threadId: id }).catch(this.ignoreClosed);
  };

  cancel = async (id: string) => {
    this.setApp({ stopping: new Set(this.app.stopping).add(id) });
    await this.client.call("turn/cancel", { threadId: id }).catch((e) => {
      const stopping = new Set(this.app.stopping);
      stopping.delete(id);
      this.setApp({ stopping });
      this.ignoreClosed(e);
    });
  };

  /**
   * 停止 / 撤回排队对关着的页没有意义（页关了，这一轮和队列也就没了）：不报错，
   * 重拉一次让界面上过时的「运行中」「排队中」消失。
   */
  private ignoreClosed = (e: unknown) => {
    if (!(e instanceof RpcError && e.code === "thread_not_open")) return this.fail(e);
    this.reloadLoaded();
  };

  /** `always`：本会话里这个工具以后不再问（Dock 的「始终允许」，子代理共用）。 */
  resolvePermission = (id: string, allow: boolean, always = false) =>
    this.act(id, "permission/resolve", { decision: allow ? "approve" : "deny", always });

  answer = (id: string, answers: QuestionAnswer[]) =>
    this.act(id, "interaction/respond", { answers });

  resolvePlan = (id: string, decision: "approve" | "revise" | "quit") => this.act(id, "plan/resolve", { decision });

  resolveElicit = (id: string, action: "accept" | "decline" | "cancel") => this.act(id, "elicit/resolve", { action });

  /** URL 模式：开浏览器，并按规范把「用户同意」回给服务器。 */
  openElicit = (id: string) => this.act(id, "elicit/open", {});

  /** 设置都是按会话的（每页一份）；改完重读一次环境。 */
  setModel = (id: string, modelId: string) => this.setEnv(id, "thread/model/set", { modelId });
  setApproval = (id: string, mode: string) => this.setEnv(id, "thread/approval/set", { mode });
  setReasoning = (id: string, effort: string) => this.setEnv(id, "thread/reasoning/set", { effort });

  clearError = () => this.setApp({ error: null });

  /** 重新拉起退出了的 Dock；连接由 client 的重连接上。 */
  restartDock = () => invoke("dock_restart").catch(this.fail);

  private async act(id: string, method: string, params: Record<string, unknown>) {
    await this.ensureOpen(id)
      .then(() => this.client.call(method, { threadId: id, ...params }))
      .catch(this.fail);
  }

  private async setEnv(id: string, method: string, params: Record<string, unknown>) {
    try {
      await this.ensureOpen(id);
      await this.client.call(method, { threadId: id, ...params });
      await this.loadEnv(id);
    } catch (e) {
      this.fail(e);
    }
  }
}

/** 画布页面的运行时报错。`line` 是 vN.html 的行号。 */
export interface CanvasRuntimeError {
  version: number;
  message: string;
  line: number | null;
}

export interface CanvasState {
  shown: string | null;
  errors: ReadonlyMap<string, CanvasRuntimeError>;
}

let bound: DockStore | null = null;

/** 主界面当前这份 store。切换网关时由 host 换上。 */
export function bindCurrentStore(next: DockStore) {
  bound = next;
}

export function currentStore(): DockStore {
  if (!bound) throw new Error("连接还没准备好");
  return bound;
}

/** 前台 store。方法绑到当时的那一份，切走之后旧闭包仍打在原来那台。 */
export const store: DockStore = new Proxy({} as DockStore, {
  get(_target, prop) {
    const current = currentStore();
    const value = Reflect.get(current, prop, current) as unknown;
    return typeof value === "function" ? (value as (...args: unknown[]) => unknown).bind(current) : value;
  },
});

/** `preset/ask` 的一条历史。 */
export interface AskTurn {
  role: "user" | "assistant";
  text: string;
}

export {
  useCanvas,
  useApp,
  useThread,
  useSubagents,
  useTurnChanges,
  useWorkflows,
  useEnv,
  useGatewayId,
  useGateways,
  useForegroundStore,
  foregroundStore,
  subscribeHost,
  focusGateway,
  openGateway,
  closeGateway,
  gatewayArg,
  presetKey,
  lastSessionKey,
} from "./gateways";
export type { GatewayAttention, GatewayHost, GatewayView } from "./gateways";

/** 预设 → 项目（cwd）→ 会话，会话按更新时间新到旧；预设顺序跟 `preset/list`。 */
export function groupSessions(
  sessions: SessionSummary[],
  presets: PresetOption[],
  prefs: ProjectPrefs = { projects: [], pinned: new Set(), removed: new Set() },
): PresetGroup[] {
  const { projects, pinned, removed } = prefs;
  const order = new Map(presets.map((p, i) => [p.id, i]));
  const groups = new Map<string, PresetGroup>();
  // 本地项目先占位：还没有会话也要出现在树里；有会话的照常合进来，名字用自己起的。
  for (const lp of projects) {
    let g = groups.get(lp.presetId);
    if (!g) {
      g = { id: lp.presetId, label: presets.find((p) => p.id === lp.presetId)?.label ?? lp.presetId, projects: [] };
      groups.set(lp.presetId, g);
    }
    const id = projectId(lp.presetId, lp.cwd);
    if (!g.projects.some((p) => p.id === id)) {
      g.projects.push({ id, name: lp.name || basename(lp.cwd), path: lp.cwd, sessions: [] });
    }
  }
  for (const s of sessions) {
    if (removed.has(s.projectId)) continue;
    const gid = s.presetId ?? "";
    let g = groups.get(gid);
    if (!g) {
      g = { id: gid, label: s.preset, projects: [] };
      groups.set(gid, g);
    }
    let p = g.projects.find((x) => x.id === s.projectId);
    if (!p) {
      p = { id: s.projectId, name: basename(s.cwd), path: s.cwd, sessions: [] };
      g.projects.push(p);
    }
    p.sessions.push(s);
  }
  const rank = (id: string) => order.get(id) ?? Number.MAX_SAFE_INTEGER;
  const list = [...groups.values()].sort((a, b) => rank(a.id) - rank(b.id));
  for (const g of list) {
    for (const p of g.projects) p.sessions.sort((a, b) => b.updatedAt - a.updatedAt);
    // 刚建、还没会话的项目排在最上面（和开着没落盘的会话一个道理）。
    const newest = (p: { sessions: SessionSummary[] }) => p.sessions[0]?.updatedAt ?? Number.MAX_SAFE_INTEGER;
    for (const p of g.projects) p.pinned = pinned.has(p.id);
    // 置顶的在前，各自再按最近活动排。
    g.projects.sort((a, b) => Number(!!b.pinned) - Number(!!a.pinned) || newest(b) - newest(a));
  }
  return list;
}

/** 会话树里项目节点的 id（同 `rebuildSessions` 给会话的 `projectId`）。 */
export function projectId(presetId: string | null, cwd: string): string {
  return `${presetId ?? ""}:${cwd}`;
}

export function basename(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
