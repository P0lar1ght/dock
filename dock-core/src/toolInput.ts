// 模型还在写的工具参数：dock.1 推送 `item/tool_input_delta`（能力 `toolInputStream`）。
//
// - 只推不记：同一个 `toolCallId` 的 `delta` 按 `chunk`（从 0 起）依次拼起来，是参数 JSON 的一个前缀；
// - 随后的 `item/tool_started`（同一个 `toolCallId`）才算数——推过的调用不一定会开始
//   （`llm/stream` 可能改写，采样也可能中途失败）；
// - 中途接入会漏掉前面的段：漏了就整条作废（`broken`），等 `item/tool_started` 的完整参数。

import type { DockEvent } from './events.ts';

export type ToolInputDelta = Extract<DockEvent, { method: 'item/tool_input_delta' }>;

/** 一次还在写的工具调用。 */
export interface ToolInputStream {
  threadId: string;
  turnId: string;
  toolCallId: string;
  toolName: string;
  /** 到目前为止拼起来的原文（参数 JSON 的前缀）。 */
  raw: string;
  /** 下一段该是第几段。 */
  next: number;
  /** 漏了段（中途接入）：`raw` 不是前缀，别用。 */
  broken: boolean;
}

/** 收一段。重复的旧段忽略；跳过了段就作废。 */
export function feedToolInput(prev: ToolInputStream | undefined, event: ToolInputDelta): ToolInputStream {
  const stream: ToolInputStream = prev ?? {
    threadId: event.threadId,
    turnId: event.turnId,
    toolCallId: event.toolCallId,
    toolName: event.toolName,
    raw: '',
    next: 0,
    broken: false,
  };
  if (stream.broken || event.chunk < stream.next) return stream;
  if (event.chunk > stream.next) return { ...stream, broken: true };
  return { ...stream, toolName: event.toolName || stream.toolName, raw: stream.raw + event.delta, next: stream.next + 1 };
}

const MISSING = Symbol('missing');

export interface PartialJsonOptions {
  /** 没写完的字符串也收（截到已写的部分）。默认 `true`；`false` 时当它还没写到。 */
  partialStrings?: boolean;
}

/**
 * 残缺的 JSON 前缀 → 尽量补全的值（MCP Apps `tool-input-partial` 说的「best-effort recovery of
 * incomplete JSON」）：
 * - 没写完的字符串照收（截到已写的部分；末尾半个转义丢掉），没写完的数字收已经成形的部分，
 *   没写完的 `true` / `false` / `null` 丢掉；
 * - 对象里还没写到值的键丢掉；没写完的对象 / 数组照收已经写好的成员；
 * - 一个值都认不出来、或者根本不是 JSON，回 `undefined`。
 *
 * 写完整的 JSON 与 `JSON.parse` 结果相同。
 */
export function partialJson(raw: string, options: PartialJsonOptions = {}): unknown {
  const partialStrings = options.partialStrings !== false;
  const n = raw.length;
  let i = 0;

  const ws = () => {
    while (i < n) {
      const c = raw.charCodeAt(i);
      if (c !== 0x20 && c !== 0x0a && c !== 0x0d && c !== 0x09) break;
      i++;
    }
  };

  const value = (): unknown => {
    ws();
    if (i >= n) return MISSING;
    const c = raw[i];
    if (c === '{') return object();
    if (c === '[') return array();
    if (c === '"') return string();
    if (c === '-' || (c >= '0' && c <= '9')) return number();
    return literal();
  };

  const object = (): unknown => {
    i++;
    const out: Record<string, unknown> = {};
    for (;;) {
      ws();
      if (i >= n) return out;
      const c = raw[i];
      if (c === '}') {
        i++;
        return out;
      }
      if (c === ',') {
        i++;
        continue;
      }
      if (c !== '"') throw new SyntaxError('key');
      const key = stringToken();
      // 键还没写完：这一项丢掉。
      if (key === MISSING || !key.closed) return out;
      ws();
      if (i >= n) return out;
      if (raw[i] !== ':') throw new SyntaxError(':');
      i++;
      const v = value();
      if (v === MISSING) return out;
      out[key.text] = v;
    }
  };

  const array = (): unknown => {
    i++;
    const out: unknown[] = [];
    for (;;) {
      ws();
      if (i >= n) return out;
      const c = raw[i];
      if (c === ']') {
        i++;
        return out;
      }
      if (c === ',') {
        i++;
        continue;
      }
      const v = value();
      if (v === MISSING) return out;
      out.push(v);
    }
  };

  const string = (): unknown => {
    const token = stringToken();
    if (token === MISSING) return MISSING;
    if (!token.closed && !partialStrings) return MISSING;
    return token.text;
  };

  /** 从 `"` 起读一个字符串：写完了 `closed`；没写完给已写的部分（末尾半个转义丢掉）。 */
  const stringToken = (): { text: string; closed: boolean } | typeof MISSING => {
    const start = i;
    i++;
    while (i < n) {
      const c = raw[i];
      if (c === '\\') {
        i += 2;
        continue;
      }
      if (c === '"') {
        i++;
        return { text: decode(raw.slice(start, i)), closed: true };
      }
      i++;
    }
    i = n;
    let body = raw.slice(start + 1);
    // 末尾半个转义：`\`、`\u`、`\u12`……
    const escape = /\\(u[0-9a-fA-F]{0,3})?$/.exec(body);
    if (escape && countTrailingBackslashes(body.slice(0, escape.index + 1)) % 2 === 1) body = body.slice(0, escape.index);
    return { text: decode(`"${body}"`), closed: false };
  };

  const number = (): unknown => {
    const start = i;
    while (i < n && /[-+0-9.eE]/.test(raw[i])) i++;
    let text = raw.slice(start, i);
    if (i < n) {
      const v = Number(text);
      if (!/^-?\d/.test(text) || Number.isNaN(v)) throw new SyntaxError('number');
      return v;
    }
    // 写到末尾的数字可能还没写完：收已经成形的部分（`1.` → 1，`-` → 还没有）。
    while (text && !/^-?\d+(\.\d+)?([eE][-+]?\d+)?$/.test(text)) text = text.slice(0, -1);
    return text ? Number(text) : MISSING;
  };

  const literal = (): unknown => {
    for (const [word, v] of [
      ['true', true],
      ['false', false],
      ['null', null],
    ] as const) {
      if (raw.startsWith(word, i)) {
        i += word.length;
        return v;
      }
      const rest = raw.slice(i);
      if (word.startsWith(rest)) {
        i = n;
        return MISSING;
      }
    }
    throw new SyntaxError('literal');
  };

  try {
    const v = value();
    return v === MISSING ? undefined : v;
  } catch {
    return undefined;
  }
}

function countTrailingBackslashes(s: string): number {
  let k = 0;
  while (k < s.length && s[s.length - 1 - k] === '\\') k++;
  return k;
}

/** 带引号的 JSON 字符串 → 文本。模型偶尔在字符串里直接写换行（不合法）：退回去掉引号的原文。 */
function decode(quoted: string): string {
  try {
    return JSON.parse(quoted) as string;
  } catch {
    try {
      return JSON.parse(quoted.replace(/[\u0000-\u001f]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`)) as string;
    } catch {
      return quoted.slice(1, -1);
    }
  }
}

function obj(v: unknown): Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
}

/**
 * 还在写的这次调用调的是哪颗工具、参数（补全过的）是什么。
 * - 经 `use_tool` 调的按需工具：`tool_name` 写完了才认（写到一半的名字可能正好是另一颗的前缀），
 *   参数是 `tool_input`（对象，或 JSON 字符串——也按残缺 JSON 补全）；
 * - 作废了（漏了段）或者还认不出是哪颗：回 `null`。
 */
export function liveToolCall(stream: ToolInputStream): { name: string; args: Record<string, unknown> } | null {
  if (stream.broken) return null;
  const parsed = obj(partialJson(stream.raw));
  if (stream.toolName !== 'use_tool') return { name: stream.toolName, args: parsed };
  const name = obj(partialJson(stream.raw, { partialStrings: false })).tool_name;
  if (typeof name !== 'string' || !name) return null;
  const input = parsed.tool_input;
  return { name, args: typeof input === 'string' ? obj(partialJson(input)) : obj(input) };
}
