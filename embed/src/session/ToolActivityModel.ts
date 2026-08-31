export type SessionToolStatus = 'running' | 'completed' | 'failed' | 'cancelled';

export interface SessionToolActivity {
  id: string;
  turnId: string;
  toolName: string;
  title: string;
  status: SessionToolStatus;
  resultType: string;
  startedSeq: number;
  updatedSeq: number;
  durationMs?: number;
  truncated: boolean;
  inputSummary?: string;
  outputSummary?: string;
  outputPreview?: string;
  outputDetails?: string;
  outputPreviewTruncated?: boolean;
}

const SENSITIVE_KEY = /(?:api[_-]?key|authorization|cookie|credential|password|secret|session[_-]?key|token)/i;
const EMBEDDED_SECRET = /(?:\bBearer\s+\S+|\b(?:sk|pk)-[A-Za-z0-9_-]{8,}|\b(?:api[_-]?key|authorization|cookie|credential|password|secret|session[_-]?key|token)\b\s*[:=]\s*(?:"[^"]*"|'[^']*'|\S+)|\b[A-Z][A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD)\s*=\s*\S+)/gi;
const MAX_SUMMARY_CHARS = 720;
const MAX_OUTPUT_PREVIEW_CHARS = 900;
const MAX_OUTPUT_DETAILS_CHARS = 12_000;

export function toolInputSummary(value: unknown) {
  if (value === undefined || value === null) return undefined;
  try {
    const rendered = JSON.stringify(sanitize(value, 0));
    return bounded(rendered || String(value), MAX_SUMMARY_CHARS);
  } catch {
    return '[unavailable]';
  }
}

export function toolOutputSummary(value: unknown, resultTypeValue: unknown) {
  const resultType = boundedText(resultTypeValue, 40) || 'result';
  if (value === undefined || value === null) return `${resultType} · no details`;
  if (Array.isArray(value)) return `${resultType} · ${value.length} items`;
  if (typeof value === 'object') {
    const record = value as Record<string, unknown>;
    const keys = Object.keys(record).filter((key) => !SENSITIVE_KEY.test(key)).slice(0, 8);
    const success = typeof record.success === 'boolean' ? (record.success ? 'success' : 'failed') : '';
    const fields = keys.length ? `fields: ${keys.join(', ')}` : 'structured result';
    return bounded([resultType, success, fields].filter(Boolean).join(' · '), MAX_SUMMARY_CHARS);
  }
  if (typeof value === 'string') return `${resultType} · text result · ${value.length} chars`;
  return `${resultType} · ${typeof value}`;
}

export function toolOutputPresentation(value: unknown, resultTypeValue: unknown) {
  const outputSummary = toolOutputSummary(value, resultTypeValue);
  const rendered = renderOutput(value);
  if (!rendered.text) return { outputSummary };

  const outputPreviewTruncated = rendered.text.length > MAX_OUTPUT_DETAILS_CHARS;
  const boundedDetails = bounded(rendered.text, MAX_OUTPUT_DETAILS_CHARS);
  const outputPreview = bounded(boundedDetails, MAX_OUTPUT_PREVIEW_CHARS);
  const hasMore = boundedDetails !== outputPreview;
  return {
    outputSummary,
    outputPreview,
    outputDetails: hasMore ? boundedDetails : undefined,
    outputPreviewTruncated: outputPreviewTruncated || undefined
  };
}

export function boundedToolLabel(value: unknown, fallback: string) {
  return boundedText(value, 80) || fallback;
}

export function finiteDuration(value: unknown) {
  const duration = Number(value);
  return Number.isFinite(duration) && duration >= 0 ? duration : undefined;
}

function sanitize(value: unknown, depth: number): unknown {
  if (depth >= 3) return summaryOf(value);
  if (typeof value === 'string') return bounded(redact(value), 180);
  if (typeof value === 'number' || typeof value === 'boolean' || value === null) return value;
  if (Array.isArray(value)) return value.slice(0, 6).map((item) => sanitize(item, depth + 1));
  if (!value || typeof value !== 'object') return String(value || '');
  const output: Record<string, unknown> = {};
  for (const [key, item] of Object.entries(value as Record<string, unknown>).slice(0, 10)) {
    output[key] = SENSITIVE_KEY.test(key) ? '[redacted]' : sanitize(item, depth + 1);
  }
  return output;
}

function renderOutput(value: unknown) {
  if (value === undefined) return { text: undefined };
  if (typeof value === 'string') return { text: value || '(empty response)' };
  if (typeof value === 'number' || typeof value === 'boolean' || value === null) {
    return { text: String(value) };
  }
  try {
    return { text: JSON.stringify(value, null, 2) || String(value) };
  } catch {
    return { text: '[unavailable]' };
  }
}

function summaryOf(value: unknown) {
  if (Array.isArray(value)) return `[${value.length} items]`;
  if (value && typeof value === 'object') return `{${Object.keys(value as object).length} fields}`;
  return sanitize(value, 0);
}

function redact(value: string) {
  return value.replace(EMBEDDED_SECRET, '[redacted]');
}

function bounded(value: string, limit: number) {
  return value.length <= limit ? value : `${value.slice(0, limit - 1)}…`;
}

function boundedText(value: unknown, limit: number) {
  const text = String(value || '').trim();
  return text ? bounded(redact(text), limit) : '';
}
