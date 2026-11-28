// 行为轨迹（dock.1 `sandbox/status` / `sandbox/trace`，见网关 handlers/sandbox.rs）。
// 开了沙箱的页，Dock 把工具调用、命令、命令和浏览器的出网记进审计日志；这里取汇总的链路图和时间线。
// 开关、导出走 `/sandbox` 斜杠（store.runSlash）。

import { dock } from "../../dock/client";

/** 这一页的工作区：原地（选定的项目目录）或工作树（项目仓库的一棵 git worktree）。 */
export interface Workspace {
  root: string;
  /** 选定的项目目录：会话归在它名下。原地时和 `root` 相同。 */
  home: string;
  mode: "in_place" | "worktree";
  branch?: string;
}

export interface SandboxRule {
  host?: string | null;
  command?: string | null;
  action: "allow" | "ask" | "deny";
}

export interface SandboxStatus {
  enabled: boolean;
  workspace: Workspace;
  cwd: string;
  /** 正在记（沙箱开着）。 */
  auditing: boolean;
  dir: string | null;
  /** 这个会话记过几次（Dock 每起一次、沙箱开着各一份目录）；`records` / `intact` 算的是全部。 */
  runs?: number;
  records: number;
  /** 哈希链和正文块都对得上。 */
  intact: boolean;
  decrypt: boolean;
  egress: "running" | "unavailable" | "idle";
  /** 命令进系统沙箱（`[sandbox] confine`）。 */
  confine: boolean;
  network: "allow" | "proxy" | "off";
  /** 系统沙箱后端（`seatbelt` / `landlock`）；本机用不了是 `null`，原因在 `backendReason`。 */
  backend: string | null;
  backendReason?: string | null;
  /** 这台机器上的后端做不到的保证（老 Dock 没有这两项）。 */
  limits?: string[];
  rules: SandboxRule[];
  passthrough: { host: string; saved: boolean }[];
}

/** 图分四层：会话 → 工具 → 程序（命令里的）→ 主机；`source` 是认不出是谁发的出网，`others` 是并起来的主机。 */
export type NodeKind = "session" | "tool" | "program" | "source" | "host" | "others";

export interface TraceNode {
  id: string;
  kind: NodeKind;
  label: string;
  n: number;
  errors: number;
  bytes: number;
}

export interface TraceEdge {
  from: string;
  to: string;
  n: number;
  errors: number;
  bytes: number;
}

export type RowKind = "tool" | "result" | "command" | "command_end" | "net" | "audit";

export interface TraceRow {
  ts: number;
  kind: RowKind;
  title: string;
  detail: string;
  error: string | null;
  /** 工具调用 id（tool / result / command 有）。 */
  call?: string | null;
  /** 命令的 job（command / command_end 有）。 */
  job?: string | null;
  /** 起这条命令的工具（command 有：bash / monitor）。 */
  tool?: string | null;
  /** 出网是哪条命令（job）或哪个标签页（`browser:<id>`）发的。 */
  tag?: string | null;
  host?: string;
  bytes?: number;
  decrypted?: boolean;
}

export interface TraceTotals {
  toolCalls: number;
  commands: number;
  requests: number;
  decrypted: number;
  bytes: number;
  errors: number;
}

export interface SandboxTrace {
  /** 沙箱现在开着、接着记。关着时照样给之前记下的。 */
  auditing: boolean;
  /** 这个会话记过几次（跨 Dock 重启接起来；之前几次的 job id 后面带 `@<第几次>`）。 */
  runs?: number;
  records?: number;
  graph: { nodes: TraceNode[]; edges: TraceEdge[] };
  totals: TraceTotals | null;
  timeline: TraceRow[];
}

/** 这个 Dock 认不认 `sandbox/*`（老版本没有）。 */
export const canTrace = () => !!dock.capabilities.sandboxTrace;

/** 不给 `threadId` 就是第 1 页（设置页只看后端和配置，不看某一页）。 */
export const sandboxStatus = (threadId?: string) =>
  dock.call<SandboxStatus>("sandbox/status", threadId ? { threadId } : {});

export const sandboxTrace = (threadId: string, limit = 300) =>
  dock.call<SandboxTrace>("sandbox/trace", { threadId, limit });

export function humanBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

// ---- HTTP 历史 ----

/** WebSocket 连接到目前为止的汇总（`sandbox/http` 的行、`sandbox/flow` 都带；老 Dock 没有）。 */
export interface WsSummary {
  /** 还开着：记它的那次运行还在写、还没记到关闭。 */
  open: boolean;
  /** 发出 / 收到的消息条数（含 ping / pong / 关闭帧，含没留下来的）。 */
  sent: number;
  received: number;
  /** 超出一条连接的上限（2000 条）没留下来的条数。 */
  omitted: number;
  /** 关闭帧：谁先发的、关闭码、原因。 */
  close: { by: "client" | "server"; code?: number; reason?: string } | null;
  error: string | null;
  note: string | null;
  bytesUp: number;
  bytesDown: number;
  /** 连上到现在（或到关闭）多久。 */
  durationMs: number | null;
}

/** 一条消息（分片拼好、压缩解开了的）。 */
export interface WsMessage {
  /** `up` 发出，`down` 收到。 */
  dir: "up" | "down";
  /** 连上之后第几毫秒。 */
  at: number;
  op: "text" | "binary" | "close" | "ping" | "pong";
  size: number;
  /** 文本消息给 `text`，其余给 `base64`（只有开头一段，看 `truncated`）。 */
  text?: string;
  base64?: string;
  truncated?: boolean;
  /** 按 permessage-deflate 解压过。 */
  compressed?: boolean;
  /** 内容没法看的原因（扩展不认识、解压失败）。 */
  note?: string;
  code?: number | null;
  reason?: string;
}

export interface WsConversation extends WsSummary {
  messages: WsMessage[];
  /** 正文块不见了、读不回来的条数。 */
  lost: number;
}

/** 一次出网（`sandbox/http` 的一行）：时间线的 `net` 行再加上请求的细节。 */
export interface HttpRow extends TraceRow {
  /** 按它取完整的请求 / 响应包（`sandbox/flow`）。 */
  id: string | null;
  method?: string | null;
  scheme?: string | null;
  port?: number | null;
  path?: string | null;
  status?: number | null;
  ms?: number | null;
  note?: string | null;
  /** WebSocket（握手那一行，之后的消息汇总在这里）。 */
  ws?: WsSummary;
}

export interface SandboxHttp {
  auditing: boolean;
  runs?: number;
  records?: number;
  requests: HttpRow[];
}

export interface PacketBody {
  /** 原始长度（可能比记下来的长）。 */
  size: number;
  /** 记下来的字节数（最多 `body_cap_kb`）。 */
  captured: number;
  truncated: boolean;
  /** 按 Content-Encoding 解开了什么；没解是 `null`。 */
  decoded: "gzip" | "deflate" | "br" | null;
  /** UTF-8 正文；二进制时是 `null`，看 `base64`。 */
  text: string | null;
  base64: string | null;
  shown: number;
}

/** 一边的包：首行（请求行 / 状态行）、头、正文。 */
export interface Packet {
  line: string;
  headers: [string, string][];
  contentType: string | null;
  body: PacketBody;
}

export interface FlowDetail {
  id: string;
  tag: string | null;
  scheme: string | null;
  host: string;
  port: number | null;
  method: string | null;
  path: string | null;
  status: number | null;
  decrypted: boolean;
  note: string | null;
  error: string | null;
  startedMs: number | null;
  durationMs: number | null;
  bytesUp: number | null;
  bytesDown: number | null;
  /** 没解密的连接没有包，只有目的地和字节数。 */
  request: Packet | null;
  response: Packet | null;
  /** WebSocket：这条连接到目前为止的消息（不是 WebSocket 是 `null`，老 Dock 没有这一项）。 */
  ws?: WsConversation | null;
}

export const sandboxHttp = (threadId: string, limit = 1000) =>
  dock.call<SandboxHttp>("sandbox/http", { threadId, limit });

export const sandboxFlow = (threadId: string, id: string) => dock.call<FlowDetail>("sandbox/flow", { threadId, id });
