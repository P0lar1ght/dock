export type SessionSubagentStatus =
  | 'running'
  | 'background'
  | 'waiting_permission'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface SessionSubagentActivity {
  id: string;
  turnId: string;
  subagentType: string;
  description: string;
  status: SessionSubagentStatus;
  startedSeq: number;
  updatedSeq: number;
  toolCalls: number;
  turns: number;
  durationMs?: number;
  resultSummary?: string;
  resultTruncated: boolean;
  errorSummary?: string;
}

const EMBEDDED_SECRET = /(?:\bBearer\s+\S+|\b(?:sk|pk)-[A-Za-z0-9_-]{8,}|\b(?:api[_-]?key|authorization|cookie|credential|password|secret|session[_-]?key|token)\b\s*[:=]\s*(?:"[^"]*"|'[^']*'|\S+)|\b[A-Z][A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD)\s*=\s*\S+)/gi;
const UNIX_LOCAL_PATH = /(^|[\s("'`])((?:\/(?!\/)[^\s"'`<>]+){2,})/g;
const WINDOWS_LOCAL_PATH = /\b[A-Za-z]:\\[^\s"'`<>]+/g;
const MAX_ID_CHARS = 160;
const MAX_TYPE_CHARS = 80;
const MAX_DESCRIPTION_CHARS = 220;
const MAX_RESULT_CHARS = 900;
const MAX_ERROR_CHARS = 360;

export function projectSubagentActivity(
  current: SessionSubagentActivity | undefined,
  turnId: string,
  method: string,
  params: Record<string, unknown>,
  seq: number
): SessionSubagentActivity | undefined {
  const payload = object(params.subagent);
  const id = safeInline(payload.id, MAX_ID_CHARS);
  if (!id || !turnId) return undefined;

  const status = statusFor(method, payload.status);
  if (!status) return undefined;
  if (current && isTerminalSubagent(current.status)) return current;

  const result = safeBlock(payload.finalOutput, MAX_RESULT_CHARS);
  const error = safeBlock(payload.error, MAX_ERROR_CHARS);
  const durationMs = safeDuration(payload.durationMs) ?? current?.durationMs;
  return {
    id,
    turnId,
    subagentType: safeInline(payload.subagentType, MAX_TYPE_CHARS)
      || current?.subagentType
      || 'SubAgent',
    description: safeInline(payload.description, MAX_DESCRIPTION_CHARS)
      || current?.description
      || '协作任务',
    status,
    startedSeq: current?.startedSeq || seq,
    updatedSeq: seq,
    toolCalls: safeCount(payload.toolCalls, current?.toolCalls),
    turns: safeCount(payload.turns, current?.turns),
    ...(durationMs === undefined ? {} : { durationMs }),
    resultSummary: result.text || current?.resultSummary,
    resultTruncated: result.present ? result.truncated : current?.resultTruncated || false,
    errorSummary: error.text || current?.errorSummary
  };
}

export function isActiveSubagent(status: SessionSubagentStatus) {
  return status === 'running' || status === 'background' || status === 'waiting_permission';
}

export function isTerminalSubagent(status: SessionSubagentStatus) {
  return status === 'completed' || status === 'failed' || status === 'cancelled';
}

function statusFor(method: string, value: unknown): SessionSubagentStatus | undefined {
  if (method === 'subagent/started') return 'running';
  if (method === 'subagent/backgrounded') return 'background';
  if (method === 'subagent/waiting_permission') return 'waiting_permission';
  if (method === 'subagent/completed') return 'completed';
  if (method === 'subagent/failed') return 'failed';
  if (method === 'subagent/cancelled') return 'cancelled';
  const status = String(value || '').trim();
  return ['running', 'background', 'waiting_permission', 'completed', 'failed', 'cancelled'].includes(status)
    ? status as SessionSubagentStatus
    : undefined;
}

function safeCount(value: unknown, fallback = 0) {
  const number = Number(value);
  return Number.isFinite(number) && number >= 0
    ? Math.min(1_000_000, Math.floor(number))
    : fallback;
}

function safeDuration(value: unknown) {
  const number = Number(value);
  return Number.isFinite(number) && number >= 0 ? number : undefined;
}

function safeInline(value: unknown, limit: number) {
  if (typeof value !== 'string') return '';
  return bounded(redact(value).replace(/\s+/gu, ' ').trim(), limit).text;
}

function safeBlock(value: unknown, limit: number) {
  if (typeof value !== 'string') return { present: false, text: '', truncated: false };
  const normalized = redact(value)
    .replace(/\r\n?/gu, '\n')
    .replace(/[ \t]+/gu, ' ')
    .replace(/\n{3,}/gu, '\n\n')
    .trim();
  return { present: true, ...bounded(normalized, limit) };
}

function bounded(value: string, limit: number) {
  return value.length <= limit
    ? { text: value, truncated: false }
    : { text: `${value.slice(0, limit - 1)}…`, truncated: true };
}

function redact(value: string) {
  return value
    .replace(EMBEDDED_SECRET, '[redacted]')
    .replace(UNIX_LOCAL_PATH, '$1[local path]')
    .replace(WINDOWS_LOCAL_PATH, '[local path]');
}

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}
