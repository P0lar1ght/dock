// 界面层的数据形状，由 dock.1 事件投影过来（见 dock/store.ts）。

export interface SessionSummary {
  /** dock.1 的 threadId = 落盘会话 id。 */
  id: string;
  projectId: string;
  title: string;
  /** agent 预设的显示名，会话头的 chip 显示它（如「编码」）。创建时定下，不中途换。 */
  preset: string;
  presetId: string | null;
  cwd: string;
  /** 在 Dock 里开着（占一页，能直接发消息）。 */
  open: boolean;
  updatedAt: number;
  /** 派过的子代理数（内存里的 + 落盘的；老 Dock 没有就是 0）。 */
  subagentCount: number;
  /** 正在跑的子代理数（只有开着的页才可能不为 0）。 */
  subagentsRunning: number;
}

export interface Project {
  /** `${presetId}:${cwd}`：同一目录在不同预设下各算一个项目节点。 */
  id: string;
  name: string;
  /** 工作目录；每个会话跟随自己所属项目的 cwd。 */
  path: string;
  sessions: SessionSummary[];
  /** 置顶（GUI 本地记的）：排在同组最上面。 */
  pinned?: boolean;
}

/** 侧栏最外层：预设 → 项目 → 会话。 */
export interface PresetGroup {
  id: string;
  label: string;
  projects: Project[];
}

export interface PresetOption {
  id: string;
  label: string;
  description: string;
  origin: "shipped" | "user" | "project";
  /** agent.yml 里记的图标名（自定义预设）；内置的没有，按 id 用语义图标。 */
  icon?: string | null;
  /** 内置预设的 id；`origin` 不是 shipped 时是用户改过的覆盖层，删掉 = 恢复内置。 */
  builtin?: boolean;
  available: boolean;
  error?: string | null;
}

/** 预设编辑器里的一个子代理（`agents/<id>.yml`）。`tools`：`null` = 全部工具，`[]` = 不用。 */
export interface SubagentDraft {
  id: string;
  name: string;
  description: string;
  persona: string;
  tools: string[] | null;
  replacePrompt: boolean;
  listings: boolean;
  /** 只读角色（explore / plan）：bash 只放行只读命令，别的问用户。编辑器不改它，原样写回。 */
  readOnly?: boolean;
  /** 内置预设自带的角色：删不掉、id 不能改。 */
  builtin: boolean;
  /** 这次编辑里新建的（id 还能改）。 */
  fresh?: boolean;
}

/** `preset/get` 的整份定义（编辑器的草稿同形）。 */
export interface PresetDraft {
  name: string;
  description: string;
  icon: string | null;
  order: number | null;
  persona: string;
  replacePrompt: boolean;
  tools: string[] | null;
  agents: SubagentDraft[];
}

export interface PresetDetail extends PresetDraft {
  id: string;
  origin: "shipped" | "user" | "project";
  builtin: boolean;
  available: boolean;
  error: string | null;
  /** 落盘的 agent.yml；内置没改过为 null。 */
  path: string | null;
}

/** `preset/draft` 的草稿：同 PresetDraft，另带挑工具的理由。不写盘。 */
export interface AiPresetDraft extends Omit<PresetDraft, "order" | "agents"> {
  toolReasons: { name: string; reason: string }[];
  agents: Omit<SubagentDraft, "builtin" | "fresh">[];
}

/** `tool/catalog` 的一项：预设能选的工具（不含 MCP）。 */
export interface CatalogTool {
  name: string;
  summary: string;
  kind: "resident" | "deferred" | "dynamic";
}

export interface ComposerSettings {
  /** 审批模式：`ask` 请求批准 / `auto` 自动批准。 */
  approval: string;
  model: string;
  modelId: string;
  /** 可选模型（`thread/environment/get` 的 `model.options`），按会话切换。 */
  models: { id: string; label: string }[];
  /** 推理强度 id（none / low / medium / high）。 */
  reasoning: string;
  reasoningOptions: string[];
  /** 计划模式开着（`thread/plan/set`）。 */
  plan: boolean;
  /** 这一页的目标（`environment.goal`）；没有为 `null`。 */
  goal: GoalInfo | null;
  /** 下一次请求的上下文占用（`environment.context`）；还没拿到为 `null`。 */
  context: ContextUsage | null;
}

export interface GoalInfo {
  /** `completed`：模型报了完成，标题和最后一条进展还留着。 */
  status: "active" | "paused" | "completed";
  summary: string;
  /** 模型最近一次 `update_goal` 报的进展。 */
  progress: string;
  /** 拿到时已经跑了多久（暂停不算）。 */
  elapsedMs: number;
  /** 这个目标用掉的请求数。 */
  requests: number;
  /** 用户用 `/goal budget` 设的上限；不限为 `null`。 */
  budget: { requests?: number; ms?: number } | null;
  /** 拿到的时刻（`Date.now()`）：`active` 时界面从这里往上走表。 */
  at: number;
}

export interface ContextUsage {
  used: number;
  total: number;
  percent: number;
  /** 到这个百分比自动压缩。 */
  triggerPercent: number;
}

/** `thread/context/get` 的一组明细。 */
export interface ContextDetailGroup {
  heading: string;
  rows: { label: string; tokens: number | null; note: string | null }[];
}

/** 上下文窗口的一片（互不重叠，加起来是整个窗口）。 */
export interface ContextSlice {
  id: "system" | "tools" | "messages" | "overhead" | "free";
  label: string;
  /** `prefix` 每轮原样重发；`session` 随对话增长。 */
  group: "prefix" | "session" | "free";
  tokens: number;
  note: string | null;
  /** 已算在这一片里的子项（技能在系统提示里、规约在消息里）。 */
  includes: { label: string; tokens: number; note: string | null }[];
  detail: ContextDetailGroup[];
}

export interface ContextBreakdown {
  model: string;
  used: number;
  total: number;
  percent: number;
  triggerPercent: number;
  triggerTokens: number;
  compactions: number;
  slices: ContextSlice[];
  /** 按需加载、不占窗口的工具（MCP / 本地按需）。 */
  onDemand: { id: string; label: string; note: string | null; detail: ContextDetailGroup[] }[];
}

/**
 * 输入框里待发的一张图：已经规范成网关收的样子（PNG / JPEG / WebP、≤ 2MB、边长 ≤ 4096），
 * `digest` 是字节的 SHA-256 十六进制。`url` 是本地预览（object URL），不发给 Dock。
 */
export interface ImageInput {
  url: string;
  mimeType: string;
  width: number;
  height: number;
  byteLength: number;
  digest: string;
  base64: string;
}

/** `slash/list` 的一条。`surface`：`gateway` 这里能跑；`terminal` 只能在 Dock 终端；`embed` 是网页宿主截图。 */
export interface SlashCommand {
  name: string;
  display: string;
  description: string;
  takesArgs: boolean;
  surface: "gateway" | "terminal" | "embed";
  kind: string;
}

/** `slash/execute` 的结果，GUI 按 `kind` 处理。 */
export type SlashResult =
  | { kind: "submitted" }
  | { kind: "filled"; fill: string }
  | { kind: "notice"; title: string; body: string }
  | { kind: "passthrough" };
