import { DockClientError } from '../protocol/errors.js';
import type {
  ContextJsonValue,
  HostContextItem,
  NormalizedContextItem
} from '../protocol/context.js';

export const HOST_CONTEXT_LIMITS = Object.freeze({
  maxItems: 16,
  maxItemBytes: 8 * 1024,
  maxDepth: 6,
  maxStringLength: 4096,
  maxTitleLength: 512,
  maxSummaryLength: 2048
});

const SOURCE_PATTERN = /^[a-zA-Z0-9][a-zA-Z0-9._:-]*$/;
const SENSITIVE_KEY = /^(authorization|cookie|set-cookie|password|passwd|secret|token|access[_-]?token|refresh[_-]?token|api[_-]?key|provider[_-]?key|credential|credentials)$/i;

export function normalizeContextItems(input: readonly HostContextItem[]) {
  if (!Array.isArray(input)) invalid('Context Provider must return an array');
  if (input.length > HOST_CONTEXT_LIMITS.maxItems) limit('Too many host Context items');
  const seen = new Set<string>();
  return input.map((item, index) => {
    if (!item || typeof item !== 'object' || Array.isArray(item)) invalid('Context item must be an object');
    const source = normalizeContextSource(item.source);
    const type = requiredString(item.type, 'type', 128);
    const id = optionalString(item.id, 128) || `auto-${type}-${index}`;
    const identity = `${source}\u0000${id}`;
    if (seen.has(identity)) invalid('Context item ids must be unique within a source');
    seen.add(identity);
    let truncated = Boolean(item.truncated);
    const title = truncate(optionalString(item.title), HOST_CONTEXT_LIMITS.maxTitleLength);
    const summary = truncate(optionalString(item.summary), HOST_CONTEXT_LIMITS.maxSummaryLength);
    truncated ||= title.truncated || summary.truncated;
    const normalized: NormalizedContextItem = {
      id,
      type,
      title: title.value,
      summary: summary.value,
      source,
      entityRef: normalizeObject(item.entityRef, 'entityRef'),
      data: normalizeObject(item.data, 'data'),
      priority: integer(item.priority, 50, 0, 100, 'priority'),
      ttlMs: integer(item.ttlMs, 0, 0, 86_400_000, 'ttlMs') || undefined,
      timestamp: Number.isSafeInteger(item.timestamp) ? Number(item.timestamp) : Date.now(),
      truncated,
      originalSize: integer(item.originalSize, 0, 0, 1_000_000, 'originalSize') || undefined
    };
    if (jsonBytes(normalized) > HOST_CONTEXT_LIMITS.maxItemBytes) {
      limit('Host Context item exceeds the byte limit');
    }
    return normalized;
  });
}

export function normalizeContextSource(value: unknown) {
  const source = requiredString(value, 'source', 128);
  if (!SOURCE_PATTERN.test(source)) invalid('Context source contains unsupported characters');
  return source;
}

function normalizeObject(
  value: Record<string, ContextJsonValue> | undefined,
  name: string
): Record<string, ContextJsonValue> {
  if (value === undefined) return {};
  if (!value || typeof value !== 'object' || Array.isArray(value)) invalid(`${name} must be an object`);
  return normalizeJson(value, 0, name) as Record<string, ContextJsonValue>;
}

function normalizeJson(value: ContextJsonValue, depth: number, path: string): ContextJsonValue {
  if (depth > HOST_CONTEXT_LIMITS.maxDepth) limit('Host Context exceeds the JSON depth limit');
  if (value === null || typeof value === 'boolean') return value;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) invalid('Context numbers must be finite');
    return value;
  }
  if (typeof value === 'string') return value.slice(0, HOST_CONTEXT_LIMITS.maxStringLength);
  if (Array.isArray(value)) return value.map((item, index) => normalizeJson(item, depth + 1, `${path}[${index}]`));
  const output: Record<string, ContextJsonValue> = {};
  for (const [key, item] of Object.entries(value)) {
    if (key.length > 256) limit('Context object key exceeds the length limit');
    if (SENSITIVE_KEY.test(key)) {
      throw new DockClientError('sensitive_host_context', `Sensitive field is not allowed in host Context: ${path}.${key}`);
    }
    output[key] = normalizeJson(item, depth + 1, `${path}.${key}`);
  }
  return output;
}

function requiredString(value: unknown, name: string, max: number) {
  const normalized = typeof value === 'string' ? value.trim() : '';
  if (!normalized) invalid(`${name} is required`);
  if (normalized.length > max) limit(`${name} exceeds the length limit`);
  return normalized;
}

function optionalString(value: unknown, max = Number.MAX_SAFE_INTEGER) {
  if (value === undefined || value === null) return '';
  if (typeof value !== 'string') invalid('Context text fields must be strings');
  const normalized = value.trim();
  if (normalized.length > max) limit('Context text field exceeds the length limit');
  return normalized;
}

function truncate(value: string, max: number) {
  return { value: value.slice(0, max), truncated: value.length > max };
}

function integer(value: unknown, fallback: number, min: number, max: number, name: string) {
  if (value === undefined || value === null || value === '') return fallback;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < min || parsed > max) invalid(`${name} is out of range`);
  return parsed;
}

function jsonBytes(value: unknown) {
  return new TextEncoder().encode(JSON.stringify(value)).byteLength;
}

function invalid(message: string): never {
  throw new DockClientError('invalid_host_context', message);
}

function limit(message: string): never {
  throw new DockClientError('host_context_limit_exceeded', message);
}
