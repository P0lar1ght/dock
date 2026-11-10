// 功能按类型登记到宿主（#181 阶段 4f）：宿主只管布局，不认识具体功能。
// 一个功能放在 features/<name>/，在自己的 index.ts 里往这里登记；main.tsx 引一次。
//
// 类型：
// - 右侧面板：按 kind 前缀认领（`plugin:<id>`），宿主没认出的 kind 来这里找。
// - 分屏菜单分组：「+」菜单里内置面板下面的一组。
// - 工具卡：头部图标、展开时可以换掉默认正文（插件视图）、标题行下面的附件（MCP App 界面）。
// - 设置页插件行：行内操作 + 展开区（插件设置卡）。
// - 输入框工具栏：右组最前面的小部件（状态项）。
// - 轮次脚注：一轮结束后、回复操作行上面的卡片（这一轮改了哪些文件）。
// - 文件预览：文件面板按文件名找预览器（表格、文档）。
// - 文件编辑器：文件面板里改文本文件用的编辑器（只有一个）。
//
// 加新类型：在这里加一张表 + 一个宿主插槽，别让宿主 import 功能目录。

import { createContext, useContext, useSyncExternalStore, type ComponentType, type ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import type { ToolStatus } from "dock-core";

/** 宿主给功能用的能力（App 提供）。 */
export interface FeatureHost {
  /** 在右侧打开一个面板（已开着就不动）。 */
  openPanel: (kind: string) => void;
  /** 关掉右侧某个面板。 */
  closePanel: (kind: string) => void;
  /** 会话区顶部的轻提示（2 秒淡出）。 */
  toast: (text: string) => void;
}

const noHost: FeatureHost = { openPanel() {}, closePanel() {}, toast() {} };
export const FeatureHostContext = createContext<FeatureHost>(noHost);
export const useFeatureHost = () => useContext(FeatureHostContext);

// ---- 右侧面板 ----

export interface PanelProps {
  kind: string;
  sessionId: string | null;
  onClose: () => void;
  /** 头栏最左边（拖手）。 */
  lead: ReactNode;
  /** 关闭按钮左边（分屏菜单、新窗口）。 */
  controls: ReactNode;
  /** 在拖出来的独立窗里（自己一条连接；`perSession` 的面板跟着拖出时的会话，别的不跟）。 */
  detached?: boolean;
}

interface PanelEntry {
  /** 认领 `prefix` 和 `prefix:...`。 */
  prefix: string;
  Panel: ComponentType<PanelProps>;
  /** 能不能拖出 / 开成独立窗口（要 Rust 侧认这种 kind）。 */
  poppable: boolean;
  /** 拖出的独立窗跟着拖出时的会话（URL 带 `session`），面板拿得到 `sessionId`；默认不跟。 */
  perSession?: boolean;
  /** 同前缀只留一页：再开一个同前缀的 kind 就换掉原来那页（面板自己有切换）。 */
  single?: boolean;
  /** 压在下面时页边上的名字和图标（面板叠放，见 components/PanelStack.tsx）。 */
  label?: string | ((kind: string) => string);
  icon?: LucideIcon;
}

const panels: PanelEntry[] = [];

export function registerPanel(entry: PanelEntry) {
  panels.push(entry);
}

export function panelFor(kind: string): PanelEntry | undefined {
  return panels.find((p) => kind === p.prefix || kind.startsWith(`${p.prefix}:`));
}

// ---- 分屏菜单分组 ----

export interface SplitGroupProps {
  /** 已经开着的面板。 */
  open: string[];
  onSplit: (kind: string) => void;
  /** 关掉菜单。 */
  close: () => void;
}

const splitGroups: ComponentType<SplitGroupProps>[] = [];

export function registerSplitGroup(Group: ComponentType<SplitGroupProps>) {
  splitGroups.push(Group);
}

export const useSplitGroups = () => splitGroups;

// ---- 右侧面板菜单项（「再叠一页」里的一行） ----

export interface PanelMenuItem {
  /** 打开这种面板（`前缀:参数`，功能登记的那种）。 */
  kind: `${string}:${string}`;
  /** 同前缀算同一个（single 的面板换了参数也只留一页；判断「已打开」用这个）。 */
  prefix: string;
  label: string;
  icon?: LucideIcon;
}

/** 自己有菜单组的面板（插件面板 / MCP 应用）不在这里登记，它们按实例列。 */
const panelMenuItems: PanelMenuItem[] = [];

export function registerPanelMenuItem(item: PanelMenuItem) {
  panelMenuItems.push(item);
}

export const usePanelMenuItems = () => panelMenuItems;

// ---- 右侧面板入口（工具面板里的行） ----

export interface PanelEntryGroupProps {
  /** 在右侧打开一个面板（宿主给）。 */
  onOpen: (kind: string) => void;
}

/** 一组入口行：插件面板、MCP 应用这些「挂载了才有」的入口。 */
const panelEntryGroups: ComponentType<PanelEntryGroupProps>[] = [];

export function registerPanelEntryGroup(Group: ComponentType<PanelEntryGroupProps>) {
  panelEntryGroups.push(Group);
}

export const usePanelEntryGroups = () => panelEntryGroups;

// ---- 工具卡 ----

export interface ToolBodyProps {
  threadId: string;
  /** 工具调用 id（`tool/view` 的 itemId）。 */
  itemId: string;
  toolName: string;
  args: Record<string, unknown>;
  /** 宿主原本要画的正文（原始输出）。 */
  raw: ReactNode;
}

/** 工具卡标题行下面的附件：不用展开就在，运行中也在。 */
export interface ToolAttachmentProps {
  threadId: string;
  itemId: string;
  toolName: string;
  args: Record<string, unknown>;
  status: ToolStatus;
  /** 开始时间（毫秒）：比页面打开早的是历史里的调用。 */
  startedAt?: number;
}

export interface ToolCardFeature {
  /**
   * 头部图标：回 null = 不管。这是 hook——登记都在启动时完成、顺序固定，
   * 宿主每次按同样顺序调用，符合 hook 规则。
   */
  useIcon?: (toolName: string, args: Record<string, unknown>) => LucideIcon | null;
  /** 包住默认正文：不接管就原样画 `raw`。只在知道线程和调用 id 时用（主会话的工具卡）。 */
  Body?: ComponentType<ToolBodyProps>;
  /** 标题行下面的附件（不接管就回 null）。同样只给主会话的工具卡。 */
  Attachment?: ComponentType<ToolAttachmentProps>;
  /** 这次调用是这一轮的产物：轮次结束后不随过程折起来（同画布卡）。不是 hook，要便宜。 */
  isArtifact?: (toolName: string, args: Record<string, unknown>) => boolean;
  /**
   * 产物跑着的时候也不能挪（里面有 iframe：一轮结束时从折叠区挪出来会换一棵 React 子树，
   * 卸了重挂、状态全丢）：从头就排在折叠区外面。代价是跑着时它排在这一轮过程的后面，
   * 不在调用发生的位置——没有 iframe 的产物别开（工作流卡、草稿卡跑着时就留在原位）。
   */
  keepMounted?: boolean;
}

const toolCards: ToolCardFeature[] = [];

export function registerToolCard(feature: ToolCardFeature) {
  toolCards.push(feature);
}

export const toolCardFeatures = () => toolCards;

// isArtifact 这类判断要等功能自己的数据（MCP App 的工具表）到了才准：数据变了功能调
// bumpToolCards()，用到判断的宿主组件挂 useToolCardRevision() 重渲染。
let toolCardRevision = 0;
const toolCardListeners = new Set<() => void>();

export function bumpToolCards() {
  toolCardRevision += 1;
  toolCardListeners.forEach((fn) => fn());
}

export function useToolCardRevision(): number {
  return useSyncExternalStore(
    (fn) => {
      toolCardListeners.add(fn);
      return () => {
        toolCardListeners.delete(fn);
      };
    },
    () => toolCardRevision,
  );
}

// ---- 设置页插件行 ----

export interface PluginRowProps {
  pluginId: string;
  /** 行内操作区（「配置」按钮）会被画在宿主原有操作左边。 */
  children: (parts: { action: ReactNode; panel: ReactNode }) => ReactNode;
}

const pluginRows: ComponentType<PluginRowProps>[] = [];

export function registerPluginRow(Row: ComponentType<PluginRowProps>) {
  pluginRows.push(Row);
}

export const usePluginRows = () => pluginRows;

// ---- 轮次脚注 ----

export interface TurnFooterProps {
  threadId: string;
  /** 投影里的轮次 id（同 `item/user_message` 的 `turnId`）。 */
  turnId: string;
}

const turnFooters: ComponentType<TurnFooterProps>[] = [];

export function registerTurnFooter(Footer: ComponentType<TurnFooterProps>) {
  turnFooters.push(Footer);
}

export const useTurnFooters = () => turnFooters;

// ---- 输入框工具栏 ----

const composerItems: ComponentType<{ threadId: string }>[] = [];

export function registerComposerItem(Item: ComponentType<{ threadId: string }>) {
  composerItems.push(Item);
}

export const useComposerItems = () => composerItems;

// ---- 文件预览 ----

/** 宿主（文件面板）已经读到的：文本带正文；表格 / 文档这类只读了大小。 */
export interface PreviewFile {
  path: string;
  size: number;
  modifiedMs?: number;
  /** 文本才有：宿主读到的正文（可能被截断）。 */
  text?: string;
  truncated?: boolean;
}

/** 存好的文件（fs/write 的回包）。 */
export interface SavedPreviewFile {
  path: string;
  size: number;
  modifiedMs?: number;
}

/**
 * 宿主让预览器进入编辑（表格）。保存按钮、⌘S、冲突提示、「没保存要不要存」都是宿主的，
 * 预览器只管改数据，把「怎么存」交给宿主。
 */
export interface PreviewEditing {
  /** 宿主打开了编辑。 */
  active: boolean;
  /** 有没有没保存的修改。 */
  onDirty: (dirty: boolean) => void;
  /** 交出保存：`force` = 不比对修改时间（覆盖保存）。期间被改过就抛 conflict（宿主处理）。 */
  bind: (save: ((force: boolean) => Promise<SavedPreviewFile>) | null) => void;
  /** 现在能不能编辑；不能就给原因（还没读完、太大、格式不支持）。 */
  onEditable: (reason: string | null) => void;
}

export interface FilePreviewProps {
  threadId: string;
  file: PreviewFile;
  /** 底栏左边的状态（「12,345 行 · 已读 42%」）；null 用宿主默认的。 */
  onStatus: (text: string | null) => void;
  /** 能编辑的预览器（`editable`）才给。 */
  edit?: PreviewEditing;
}

export interface FilePreviewEntry {
  /** 按文件名（小写）认领。 */
  match: (name: string) => boolean;
  Viewer: ComponentType<FilePreviewProps>;
  /** 二进制格式：宿主只取大小，不读正文，也不给「源码」。 */
  binary: boolean;
  /** 宿主开始读这个文件时调：预览器的代码和读元数据并行加载。 */
  preload?: () => void;
  /** 能在预览里改（表格）：宿主给 `edit`，显示「编辑」。 */
  editable?: boolean;
}

const filePreviews: FilePreviewEntry[] = [];

export function registerFilePreview(entry: FilePreviewEntry) {
  filePreviews.push(entry);
}

export function filePreviewFor(name: string): FilePreviewEntry | undefined {
  const lower = name.toLowerCase();
  return filePreviews.find((p) => p.match(lower));
}

// ---- 文件编辑器 ----

export interface FileEditorProps {
  /** 相对路径：认语言用。 */
  path: string;
  /** 打开时的内容；换了它（或 path）编辑器才重建。 */
  initial: string;
  /** 内容变了：给一个取当前全文的函数（别每敲一个字就拼一遍整份）。 */
  onChange: (read: () => string) => void;
  /** ⌘S。 */
  onSave: () => void;
  /** 打开后跳到这一行（从 1 起）。 */
  line?: number | null;
}

export interface FileEditorEntry {
  Editor: ComponentType<FileEditorProps>;
  /** 预先加载编辑器的代码（打开文本文件时就调，点「编辑」不用等）。 */
  preload: () => void;
}

let fileEditor: FileEditorEntry | null = null;

export function registerFileEditor(entry: FileEditorEntry) {
  fileEditor = entry;
}

export const fileEditorEntry = () => fileEditor;
