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

export interface Question {
  id: string;
  header: string;
  question: string;
  options: { label: string; description: string; recommended: boolean }[];
  /** 多选题：可以选多个选项。旧网关不带这个字段，算单选。 */
  multiSelect: boolean;
}

export type DockEvent = EventBase &
  (
    | { method: 'turn/started' }
    | { method: 'turn/completed'; status: TurnEndStatus; error?: string }
    | { method: 'item/user_message'; content: string; attachments: ImageAttachment[] }
    | { method: 'item/message_delta'; delta: string }
    | { method: 'item/tool_started'; toolCallId: string; toolName: string; arguments: Record<string, unknown> }
    | { method: 'item/tool_completed'; toolCallId: string; toolName: string; output: string; status: ToolEndStatus }
    | { method: 'permission/requested'; requestId: string; toolName: string; summary: string }
    | { method: 'permission/resolved'; requestId: string; decision: 'approve' | 'deny'; always: boolean }
    | { method: 'interaction/requested'; interactionId: string; questions: Question[] }
    | { method: 'interaction/resolved'; interactionId: string }
    | { method: 'plan/requested'; planId: string; path: string; body: string; empty: boolean }
    | { method: 'plan/resolved'; planId: string; decision: 'approve' | 'revise' | 'quit' }
    | { method: 'elicit/requested'; elicitId: string; server: string; message: string; heading: string }
    | { method: 'elicit/resolved'; elicitId: string }
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
