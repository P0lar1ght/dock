// dock.1 线程事件的类型化契约。来源：cordis-gateway 的 `transcript.rs`（每个
// `push` 的方法名与载荷）。实时推送（`method` + 平铺的 `params`）和
// `thread/history` 的 `events`（`method` + `payload` + `seq`）都先过
// `parseEvent`，之后只和这里的类型打交道。

/** 每条事件共有的字段。`at` 是毫秒时间戳（网关给的是毫秒数字串）。 */
export interface EventBase {
  seq: number;
  threadId: string;
  turnId: string;
  at: number;
}

export type TurnEndStatus = 'completed' | 'cancelled' | 'failed';
/** `denied`：用户在权限门拒绝了这次调用（工具没跑）。 */
export type ToolEndStatus = 'completed' | 'failed' | 'cancelled' | 'denied';

export interface ImageAttachment {
  type: 'image';
  mimeType: string;
  width: number;
  height: number;
  byteLength: number;
}

/** 工具结果里的一张图（截图等）：只有元数据，像素用 `item/image { itemId, index }` 取。 */
export interface ToolImage extends ImageAttachment {
  /** 在这次工具结果里的位置；空图不投影也不占号，所以不能按数组下标算。 */
  index: number;
}

export interface Question {
  id: string;
  header: string;
  question: string;
  options: { label: string; description: string; recommended: boolean }[];
  /** 多选题：可以选多个选项。旧网关不带这个字段，算单选。 */
  multiSelect: boolean;
}

export type CompactionStatus = 'running' | 'completed' | 'failed' | 'cancelled';
/** `memory` 整理记忆 → `summary` 生成摘要 → `apply` 替换历史。 */
export type CompactionPhase = 'memory' | 'summary' | 'apply';

/** 一次压缩的进展：`context/compacted` 推送与 `context.lastCompaction` 同一个外形。 */
export interface CompactionProgress {
  status: CompactionStatus;
  trigger: 'auto' | 'manual';
  phase: CompactionPhase;
  /** 第几次尝试，从 1 起。 */
  attempt: number;
  maxAttempts: number;
  /** 上一次尝试为什么没成（重试时才有）。 */
  retryReason: string | null;
  /** 这次尝试摘要已输出的 token（估算）。 */
  outputTokens: number;
  beforeTokens: number;
  /** 压成了才有。 */
  afterTokens: number | null;
  elapsedMs: number;
  /** 失败原因（`status === 'failed'`）。 */
  error: string | null;
}

/** `context/compacted` 的载荷（或 `context.lastCompaction`）→ 进展。认不出状态返回 `null`。 */
export function parseCompaction(params: Record<string, unknown>): CompactionProgress | null {
  const status = str(params.status);
  if (!['running', 'completed', 'failed', 'cancelled'].includes(status)) return null;
  const after = params.afterTokens;
  return {
    status: status as CompactionStatus,
    trigger: oneOf(params.trigger, ['auto', 'manual'] as const, 'auto'),
    phase: oneOf(params.phase, ['memory', 'summary', 'apply'] as const, 'summary'),
    attempt: num(params.attempt) || 1,
    maxAttempts: num(params.maxAttempts) || 1,
    retryReason: str(params.retryReason) || null,
    outputTokens: num(params.outputTokens),
    beforeTokens: num(params.beforeTokens),
    afterTokens: after == null ? null : num(after),
    elapsedMs: num(params.elapsedMs),
    error: str(params.error) || null,
  };
}

export type SubagentStatus = 'running' | 'idle' | 'completed' | 'failed' | 'cancelled';

/** 在跑的子代理此刻在做什么（快照；实时的从它的对话里算，见 `subagents.ts`）。 */
export type SubagentActivity =
  | { kind: 'tool'; toolName: string; arguments: Record<string, unknown> }
  | { kind: 'replying' }
  | { kind: 'thinking' };

/** 一个子代理：`subagent/updated` 的 `agent`、`subagent/list` 的一项。 */
export interface SubagentInfo {
  agentId: string;
  /** 派它的那次 `task` 调用；workflow 派的是 `null`。 */
  toolCallId: string | null;
  subagentType: string;
  /** 预设里的显示名（如「探索」）。 */
  role: string;
  description: string;
  /** `idle`：这一轮做完了，还能接着聊；`completed`：收掉了。 */
  status: SubagentStatus;
  startedAt: number;
  /** 在跑时是到那次快照为止；停下就定住。 */
  durationMs: number;
  toolCalls: number;
  /** 最近一轮的回复（网关截到 4000 字）。 */
  output: string;
  error: string | null;
  activity: SubagentActivity | null;
}

/** `subagent/updated` 的 `agent`（或 `subagent/list` 的一项）→ 子代理。没有 id 返回 `null`。 */
export function parseSubagent(raw: Record<string, unknown>): SubagentInfo | null {
  const agentId = str(raw.agentId);
  if (!agentId) return null;
  const a = obj(raw.activity);
  const kind = str(a.kind);
  const activity: SubagentActivity | null =
    kind === 'tool'
      ? { kind, toolName: str(a.toolName), arguments: obj(a.arguments) }
      : kind === 'replying' || kind === 'thinking'
        ? { kind }
        : null;
  return {
    agentId,
    toolCallId: str(raw.toolCallId) || null,
    subagentType: str(raw.subagentType),
    role: str(raw.role) || str(raw.subagentType),
    description: str(raw.description),
    status: oneOf(raw.status, ['running', 'idle', 'completed', 'failed', 'cancelled'] as const, 'running'),
    startedAt: num(raw.startedAt),
    durationMs: num(raw.durationMs),
    toolCalls: num(raw.toolCalls),
    output: str(raw.output),
    error: str(raw.error) || null,
    activity,
  };
}

export type DockEvent = EventBase &
  (
    | { method: 'turn/started' }
    | { method: 'turn/completed'; status: TurnEndStatus; error?: string }
    | {
        method: 'item/user_message';
        content: string;
        attachments: ImageAttachment[];
        /** 子代理的对话里：`parent` = 父级在它跑的时候发来的话。其余没有。 */
        origin?: 'parent';
      }
    | { method: 'item/message_delta'; delta: string }
    /** 模型的思考过程（增量）。旧网关不推，就没有。 */
    | { method: 'item/reasoning_delta'; delta: string }
    | { method: 'item/tool_started'; toolCallId: string; toolName: string; arguments: Record<string, unknown> }
    | {
        method: 'item/tool_completed';
        toolCallId: string;
        toolName: string;
        output: string;
        status: ToolEndStatus;
        /** 旧网关不带：空数组。 */
        images: ToolImage[];
      }
    | { method: 'permission/requested'; requestId: string; toolName: string; summary: string }
    | { method: 'permission/resolved'; requestId: string; decision: 'approve' | 'deny'; always: boolean }
    | { method: 'interaction/requested'; interactionId: string; questions: Question[] }
    | { method: 'interaction/resolved'; interactionId: string }
    | { method: 'plan/requested'; planId: string; path: string; body: string; empty: boolean }
    | { method: 'plan/resolved'; planId: string; decision: 'approve' | 'revise' | 'quit' }
    | { method: 'elicit/requested'; elicitId: string; server: string; message: string; heading: string }
    | { method: 'elicit/resolved'; elicitId: string }
    /** 压缩完成的标记（落在历史里；前后占用只在实时的 `context/compacted` 里）。 */
    | { method: 'item/compaction'; itemId: string }
    /** 压缩进展。只推不记（`seq` 为 0），不进 `thread/history`。 */
    | { method: 'context/compacted'; progress: CompactionProgress }
    /** 子代理出现了或状态变了。只推不记（`seq` 为 0）。 */
    | { method: 'subagent/updated'; agent: SubagentInfo }
    /** 子代理对话里的一条事件（包在父线程上，`seq` 为 0；`event.seq` 是子代理自己的）。 */
    | { method: 'subagent/event'; agentId: string; event: DockEvent }
  );

export type DockEventMethod = DockEvent['method'];

type Raw = Record<string, unknown>;

const str = (v: unknown): string => (typeof v === 'string' ? v : v == null ? '' : String(v));
const num = (v: unknown): number => {
  const n = typeof v === 'number' ? v : Number(v);
  return Number.isFinite(n) ? n : 0;
};
const bool = (v: unknown): boolean => v === true;
const obj = (v: unknown): Raw => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Raw) : {});
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

/** 网关的 `timestamp` 是毫秒数字串；也认数字与 ISO 串。认不出返回 0。 */
export function eventTime(v: unknown): number {
  if (typeof v === 'number') return v;
  const s = str(v);
  if (!s) return 0;
  if (/^\d+$/.test(s)) return Number(s);
  const t = Date.parse(s);
  return Number.isNaN(t) ? 0 : t;
}

function oneOf<T extends string>(v: unknown, allowed: readonly T[], fallback: T): T {
  return (allowed as readonly string[]).includes(str(v)) ? (v as T) : fallback;
}

/**
 * 一条原始通知 → 类型化事件。认不出的方法返回 `null`（协议只做加法，旧客户端
 * 遇到新方法应当忽略，而不是报错）。
 */
export function parseEvent(method: string, params: Raw): DockEvent | null {
  const base: EventBase = {
    seq: num(params.seq ?? params.transcriptSeq),
    threadId: str(params.threadId),
    turnId: str(params.turnId),
    at: eventTime(params.timestamp),
  };
  switch (method) {
    case 'turn/started':
      return { ...base, method };
    case 'turn/completed': {
      const status = oneOf(params.status, ['completed', 'cancelled', 'failed'] as const, 'completed');
      const error = str(params.error);
      return error ? { ...base, method, status, error } : { ...base, method, status };
    }
    case 'item/user_message':
      return {
        ...base,
        method,
        ...(params.origin === 'parent' ? { origin: 'parent' as const } : {}),
        content: str(params.content),
        attachments: list(params.attachments)
          .map(obj)
          .filter((a) => a.type === 'image')
          .map((a) => ({
            type: 'image' as const,
            mimeType: str(a.mimeType),
            width: num(a.width),
            height: num(a.height),
            byteLength: num(a.byteLength),
          })),
      };
    case 'item/message_delta':
    case 'item/reasoning_delta':
      return { ...base, method, delta: str(params.delta) };
    case 'item/tool_started':
      return {
        ...base,
        method,
        toolCallId: str(params.toolCallId),
        toolName: str(params.toolName),
        arguments: obj(params.arguments),
      };
    case 'item/tool_completed':
      return {
        ...base,
        method,
        toolCallId: str(params.toolCallId),
        toolName: str(params.toolName),
        output: str(params.output),
        status: oneOf(params.status, ['completed', 'failed', 'cancelled', 'denied'] as const, 'completed'),
        images: list(params.attachments)
          .map(obj)
          .filter((a) => a.type === 'image')
          .map((a) => ({
            type: 'image' as const,
            index: num(a.index),
            mimeType: str(a.mimeType),
            width: num(a.width),
            height: num(a.height),
            byteLength: num(a.byteLength),
          })),
      };
    case 'permission/requested':
      return {
        ...base,
        method,
        requestId: str(params.requestId),
        toolName: str(params.toolName),
        summary: str(params.summary),
      };
    case 'permission/resolved':
      return {
        ...base,
        method,
        requestId: str(params.requestId),
        decision: oneOf(params.decision, ['approve', 'deny'] as const, 'deny'),
        always: bool(params.always),
      };
    case 'interaction/requested':
      return {
        ...base,
        method,
        interactionId: str(params.interactionId),
        questions: list(params.questions)
          .map(obj)
          .map((q) => ({
            id: str(q.id),
            header: str(q.header),
            question: str(q.question),
            options: list(q.options)
              .map(obj)
              .map((o) => ({
                label: str(o.label),
                description: str(o.description),
                recommended: bool(o.recommended),
              })),
            multiSelect: bool(q.multiSelect),
          })),
      };
    case 'interaction/resolved':
      return { ...base, method, interactionId: str(params.interactionId) };
    case 'plan/requested':
      return {
        ...base,
        method,
        planId: str(params.planId),
        path: str(params.path),
        body: str(params.body),
        empty: bool(params.empty),
      };
    case 'plan/resolved':
      return {
        ...base,
        method,
        planId: str(params.planId),
        decision: oneOf(params.decision, ['approve', 'revise', 'quit'] as const, 'quit'),
      };
    case 'elicit/requested':
      return {
        ...base,
        method,
        elicitId: str(params.elicitId),
        server: str(params.server),
        message: str(params.message),
        heading: str(params.heading),
      };
    case 'elicit/resolved':
      return { ...base, method, elicitId: str(params.elicitId) };
    case 'item/compaction':
      return { ...base, method, itemId: str(params.itemId) || `compaction-${base.seq}` };
    case 'context/compacted': {
      const progress = parseCompaction(params);
      return progress ? { ...base, method, progress } : null;
    }
    case 'subagent/updated': {
      const agent = parseSubagent(obj(params.agent));
      return agent ? { ...base, method, agent } : null;
    }
    case 'subagent/event': {
      const agentId = str(params.agentId);
      const event = parseHistoryItem(obj(params.event));
      return agentId && event ? { ...base, method, agentId, event } : null;
    }
    default:
      return null;
  }
}

/** `thread/history` 的一条 `events`（`{method, payload, seq, timestamp}`）。 */
export function parseHistoryItem(item: Raw): DockEvent | null {
  const payload = obj(item.payload);
  return parseEvent(str(item.method), {
    ...payload,
    seq: item.seq ?? payload.seq,
    timestamp: item.timestamp ?? payload.timestamp,
    threadId: payload.threadId ?? item.threadId,
    turnId: payload.turnId ?? item.turnId,
  });
}
