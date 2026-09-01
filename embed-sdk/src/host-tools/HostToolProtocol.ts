import { DockClientError } from '../protocol/errors.js';
import type {
  HostToolAnnotations,
  HostToolDescriptor,
  HostToolJson
} from './types.js';

export const HOST_TOOL_LIMITS = Object.freeze({
  toolsPerConnection: 32,
  titleCharacters: 128,
  descriptionCharacters: 2_048,
  schemaBytes: 32 * 1024,
  catalogBytes: 256 * 1024,
  argumentsBytes: 64 * 1024,
  resultBytes: 256 * 1024,
  jsonDepth: 16,
  completedInvocations: 128,
  handlerTimeoutMs: 30_000,
  scopesPerTool: 8,
  activeScopes: 16,
  scopeCharacters: 64
});

const NAME_PATTERN = /^[a-z][a-z0-9_]{0,47}$/;
const SCOPE_PATTERN = /^[a-z][a-z0-9._:-]{0,63}$/;
const FORBIDDEN_KEYS = new Set(['__proto__', 'constructor', 'prototype']);
const SCHEMA_KEYS = new Set([
  'type', 'properties', 'required', 'additionalProperties', 'items',
  'enum', 'const', 'title', 'description', 'default', 'examples',
  'minLength', 'maxLength', 'pattern', 'minimum', 'maximum',
  'minItems', 'maxItems'
]);
const SCHEMA_TYPES = new Set(['object', 'array', 'string', 'number', 'integer', 'boolean', 'null']);

export function normalizeHostToolDescriptor(value: HostToolDescriptor): HostToolDescriptor {
  const raw = objectValue(value, 'host_tool_descriptor_invalid');
  const name = boundedText(raw.name, 48, 'host_tool_name_invalid');
  if (!NAME_PATTERN.test(name)) clientError('host_tool_name_invalid', 'Host Tool name is invalid');
  const inputSchema = normalizeBoundedJson(
    raw.inputSchema,
    HOST_TOOL_LIMITS.schemaBytes,
    'host_tool_schema_invalid'
  );
  if (!inputSchema || typeof inputSchema !== 'object' || Array.isArray(inputSchema)) {
    clientError('host_tool_schema_invalid', 'Host Tool inputSchema must be an object');
  }
  validateSchemaNode(inputSchema as Record<string, HostToolJson>, 0);
  if ((inputSchema as Record<string, HostToolJson>).type !== 'object') {
    clientError('host_tool_schema_invalid', 'Host Tool inputSchema root type must be object');
  }
  return {
    name,
    title: boundedText(raw.title, HOST_TOOL_LIMITS.titleCharacters, 'host_tool_title_invalid'),
    description: boundedText(
      raw.description,
      HOST_TOOL_LIMITS.descriptionCharacters,
      'host_tool_description_invalid'
    ),
    inputSchema: inputSchema as Record<string, HostToolJson>,
    annotations: normalizeAnnotations(raw.annotations),
    scopes: normalizeHostToolScopes(raw.scopes, HOST_TOOL_LIMITS.scopesPerTool)
  };
}

export async function digestHostToolDescriptor(descriptor: HostToolDescriptor) {
  return sha256(canonicalHostToolJson({
    contractVersion: 2,
    name: descriptor.name,
    title: descriptor.title,
    description: descriptor.description,
    inputSchema: descriptor.inputSchema,
    annotations: {
      readOnly: descriptor.annotations.readOnly,
      ...(descriptor.annotations.destructive === undefined ? {} : {
        destructive: descriptor.annotations.destructive
      }),
      ...(descriptor.annotations.risk ? { risk: descriptor.annotations.risk } : {})
    },
    scopes: [...normalizeHostToolScopes(descriptor.scopes, HOST_TOOL_LIMITS.scopesPerTool)]
  }));
}

export function normalizeHostToolScopes(value: unknown, limit: number = HOST_TOOL_LIMITS.activeScopes) {
  if (value === undefined) return Object.freeze([]) as readonly string[];
  if (!Array.isArray(value) || value.length > limit) {
    clientError('host_tool_scopes_invalid', 'Host Tool scopes exceed the configured limit');
  }
  const normalized = value.map((item) => boundedText(
    item,
    HOST_TOOL_LIMITS.scopeCharacters,
    'host_tool_scopes_invalid'
  ));
  if (normalized.some((scope) => !SCOPE_PATTERN.test(scope)) || new Set(normalized).size !== normalized.length) {
    clientError('host_tool_scopes_invalid', 'Host Tool scopes are invalid or duplicated');
  }
  return Object.freeze([...normalized].sort());
}

export async function digestVisibleHostTools(
  toolCatalogDigest: string,
  activeScopesValue: unknown,
  tools: Array<{ name: string; descriptorDigest: string; registrationEpoch: string }>
) {
  const activeScopes = normalizeHostToolScopes(activeScopesValue);
  return sha256(canonicalHostToolJson({
    contractVersion: 1,
    toolCatalogDigest,
    activeScopes: [...activeScopes],
    tools: [...tools]
      .sort((left, right) => left.name.localeCompare(right.name))
      .map(({ name, descriptorDigest, registrationEpoch }) => ({
        name,
        descriptorDigest,
        registrationEpoch
      }))
  }));
}

export async function digestHostToolCatalog(tools: Array<{
  name: string;
  descriptorDigest: string;
  registrationEpoch: string;
}>) {
  return sha256(canonicalHostToolJson(
    [...tools]
      .sort((left, right) => left.name.localeCompare(right.name))
      .map(({ name, descriptorDigest, registrationEpoch }) => ({
        name,
        descriptorDigest,
        registrationEpoch
      }))
  ));
}

export function normalizeHostToolArguments(
  value: unknown,
  schema: Record<string, HostToolJson>
): Record<string, HostToolJson> {
  const normalized = normalizeBoundedJson(
    value,
    HOST_TOOL_LIMITS.argumentsBytes,
    'host_tool_arguments_invalid'
  );
  validateSchemaValue(normalized, schema, '$');
  if (!normalized || typeof normalized !== 'object' || Array.isArray(normalized)) {
    clientError('host_tool_arguments_invalid', 'Host Tool arguments must be an object');
  }
  return normalized as Record<string, HostToolJson>;
}

export function normalizeHostToolResult(value: unknown): HostToolJson {
  return normalizeBoundedJson(value, HOST_TOOL_LIMITS.resultBytes, 'host_tool_result_invalid');
}

export function randomHostToolId() {
  const cryptoValue = globalThis.crypto;
  if (!cryptoValue?.randomUUID) {
    clientError('secure_random_unavailable', 'Secure random UUID generation is required for Host Tools');
  }
  return cryptoValue.randomUUID();
}

function normalizeAnnotations(value: unknown): HostToolAnnotations {
  const raw = objectValue(value, 'host_tool_annotations_invalid');
  const risk = raw.risk === undefined ? undefined : String(raw.risk);
  if (risk && risk !== 'low' && risk !== 'medium' && risk !== 'high') {
    clientError('host_tool_annotations_invalid', 'Host Tool risk annotation is invalid');
  }
  return {
    readOnly: raw.readOnly === true,
    ...(raw.destructive === undefined ? {} : { destructive: raw.destructive === true }),
    ...(risk ? { risk: risk as HostToolAnnotations['risk'] } : {})
  };
}

function normalizeBoundedJson(value: unknown, maxBytes: number, code: string): HostToolJson {
  const normalized = normalizeJson(value, new WeakSet<object>(), 0, code);
  if (utf8Bytes(canonicalHostToolJson(normalized)) > maxBytes) {
    clientError(code, 'Host Tool JSON payload exceeds the configured limit');
  }
  return normalized;
}

function canonicalHostToolJson(value: HostToolJson): string {
  if (value === null || typeof value === 'boolean' || typeof value === 'string') return JSON.stringify(value);
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) clientError('host_tool_json_invalid', 'JSON number must be finite');
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return '[' + value.map(canonicalHostToolJson).join(',') + ']';
  return '{' + Object.keys(value)
    .sort()
    .map((key) => JSON.stringify(key) + ':' + canonicalHostToolJson(value[key]))
    .join(',') + '}';
}

function validateSchemaNode(schema: Record<string, HostToolJson>, depth: number): void {
  if (depth > HOST_TOOL_LIMITS.jsonDepth) clientError('host_tool_schema_invalid', 'Host Tool schema exceeds maximum depth');
  for (const key of Object.keys(schema)) {
    if (!SCHEMA_KEYS.has(key)) clientError('host_tool_schema_invalid', 'Host Tool schema contains an unsupported keyword');
  }
  const type = String(schema.type || '');
  if (!SCHEMA_TYPES.has(type)) clientError('host_tool_schema_invalid', 'Host Tool schema type is invalid');
  if (schema.properties !== undefined) {
    for (const [key, child] of Object.entries(schemaObject(schema.properties))) {
      assertSafeKey(key, 'host_tool_schema_invalid');
      validateSchemaNode(schemaObject(child), depth + 1);
    }
  }
  if (
    schema.required !== undefined
    && (!Array.isArray(schema.required) || schema.required.some((item) => typeof item !== 'string'))
  ) {
    clientError('host_tool_schema_invalid', 'Host Tool schema required must be a string array');
  }
  if (schema.items !== undefined) validateSchemaNode(schemaObject(schema.items), depth + 1);
  if (
    schema.additionalProperties !== undefined
    && typeof schema.additionalProperties !== 'boolean'
    && (!schema.additionalProperties || typeof schema.additionalProperties !== 'object' || Array.isArray(schema.additionalProperties))
  ) {
    clientError('host_tool_schema_invalid', 'Host Tool additionalProperties is invalid');
  }
  if (schema.additionalProperties && typeof schema.additionalProperties === 'object') {
    validateSchemaNode(schemaObject(schema.additionalProperties), depth + 1);
  }
}

function validateSchemaValue(value: HostToolJson, schema: Record<string, HostToolJson>, path: string): void {
  const type = String(schema.type);
  const matches = type === 'null' ? value === null
    : type === 'array' ? Array.isArray(value)
      : type === 'object' ? Boolean(value && typeof value === 'object' && !Array.isArray(value))
        : type === 'integer' ? typeof value === 'number' && Number.isInteger(value)
          : typeof value === type;
  if (!matches) invalidArguments(path + ' does not match schema type ' + type);
  if (schema.const !== undefined && canonicalHostToolJson(value) !== canonicalHostToolJson(schema.const)) {
    invalidArguments(path + ' does not match schema const');
  }
  if (Array.isArray(schema.enum) && !schema.enum.some((item) => canonicalHostToolJson(item) === canonicalHostToolJson(value))) {
    invalidArguments(path + ' is not an allowed enum value');
  }
  if (type === 'object') validateObject(value as Record<string, HostToolJson>, schema, path);
  if (type === 'array') validateArray(value as HostToolJson[], schema, path);
  if (type === 'string') validateString(value as string, schema, path);
  if (type === 'number' || type === 'integer') validateNumber(value as number, schema, path);
}

function validateObject(value: Record<string, HostToolJson>, schema: Record<string, HostToolJson>, path: string) {
  const properties = schema.properties === undefined ? {} : schemaObject(schema.properties);
  const required = Array.isArray(schema.required) ? schema.required as string[] : [];
  for (const key of required) if (!(key in value)) invalidArguments(path + '.' + key + ' is required');
  for (const [key, item] of Object.entries(value)) {
    assertSafeKey(key, 'host_tool_arguments_invalid');
    const child = properties[key];
    if (child) validateSchemaValue(item, schemaObject(child), path + '.' + key);
    else if (schema.additionalProperties === false) invalidArguments(path + '.' + key + ' is not allowed');
    else if (schema.additionalProperties && typeof schema.additionalProperties === 'object') {
      validateSchemaValue(item, schemaObject(schema.additionalProperties), path + '.' + key);
    }
  }
}

function validateArray(value: HostToolJson[], schema: Record<string, HostToolJson>, path: string) {
  const min = numberKeyword(schema.minItems);
  const max = numberKeyword(schema.maxItems);
  if (min !== undefined && value.length < min) invalidArguments(path + ' has too few items');
  if (max !== undefined && value.length > max) invalidArguments(path + ' has too many items');
  if (schema.items) value.forEach((item, index) => validateSchemaValue(item, schemaObject(schema.items), path + '[' + index + ']'));
}

function validateString(value: string, schema: Record<string, HostToolJson>, path: string) {
  const min = numberKeyword(schema.minLength);
  const max = numberKeyword(schema.maxLength);
  if (min !== undefined && value.length < min) invalidArguments(path + ' is too short');
  if (max !== undefined && value.length > max) invalidArguments(path + ' is too long');
  if (typeof schema.pattern === 'string') {
    let expression: RegExp;
    try {
      expression = new RegExp(schema.pattern, 'u');
    } catch {
      clientError('host_tool_schema_invalid', 'Host Tool schema pattern is invalid');
    }
    if (!expression!.test(value)) invalidArguments(path + ' does not match pattern');
  }
}

function validateNumber(value: number, schema: Record<string, HostToolJson>, path: string) {
  const minimum = numberKeyword(schema.minimum);
  const maximum = numberKeyword(schema.maximum);
  if (minimum !== undefined && value < minimum) invalidArguments(path + ' is below minimum');
  if (maximum !== undefined && value > maximum) invalidArguments(path + ' is above maximum');
}

function normalizeJson(value: unknown, seen: WeakSet<object>, depth: number, code: string): HostToolJson {
  if (depth > HOST_TOOL_LIMITS.jsonDepth) clientError(code, 'Host Tool JSON exceeds maximum depth');
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return value;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) clientError(code, 'Host Tool JSON contains a non-finite number');
    return value;
  }
  if (!value || typeof value !== 'object') clientError(code, 'Host Tool value is not JSON serializable');
  if (seen.has(value as object)) clientError(code, 'Host Tool JSON contains a cycle');
  seen.add(value as object);
  if (Array.isArray(value)) return value.map((item) => normalizeJson(item, seen, depth + 1, code));
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    clientError(code, 'Host Tool JSON must contain only plain objects');
  }
  const output: Record<string, HostToolJson> = Object.create(null);
  for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
    assertSafeKey(key, code);
    output[key] = normalizeJson(item, seen, depth + 1, code);
  }
  return output;
}

async function sha256(value: string) {
  const digest = await globalThis.crypto.subtle.digest('SHA-256', new TextEncoder().encode(value));
  const bytes = new Uint8Array(digest);
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/gu, '-').replace(/\//gu, '_').replace(/=+$/gu, '');
}

function utf8Bytes(value: string) {
  return new TextEncoder().encode(value).byteLength;
}

function objectValue(value: unknown, code: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) clientError(code, 'Host Tool value must be an object');
  return value as Record<string, unknown>;
}

function schemaObject(value: HostToolJson): Record<string, HostToolJson> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    clientError('host_tool_schema_invalid', 'Host Tool schema node must be an object');
  }
  return value as Record<string, HostToolJson>;
}

function boundedText(value: unknown, maximum: number, code: string) {
  const result = String(value || '').trim();
  if (!result || [...result].length > maximum) clientError(code, 'Host Tool text field is empty or exceeds its limit');
  return result;
}

function assertSafeKey(key: string, code: string) {
  if (FORBIDDEN_KEYS.has(key)) clientError(code, 'Host Tool JSON contains a forbidden key');
}

function numberKeyword(value: HostToolJson | undefined) {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function invalidArguments(message: string): never {
  return clientError('host_tool_arguments_invalid', message);
}

function clientError(code: string, message: string): never {
  throw new DockClientError(code, message);
}
