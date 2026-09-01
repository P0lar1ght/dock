import { boundedToolLabel, toolInputSummary } from './ToolActivityModel.js';

export type SessionPermissionStatus = 'pending' | 'approved' | 'denied' | 'failed';
export type SessionPermissionRisk = 'low' | 'medium' | 'high' | 'unknown';
export type SessionPermissionDecisionSource = 'deterministic' | 'assistant' | 'policy';

const EMBEDDED_SECRET = /(?:\bBearer\s+\S+|\b(?:sk|pk)-[A-Za-z0-9_-]{8,}|\b(?:api[_-]?key|authorization|cookie|credential|password|secret|session[_-]?key|token)\b\s*[:=]\s*(?:"[^"]*"|'[^']*'|\S+)|\b[A-Z][A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD)\s*=\s*\S+)/gi;

export interface SessionPermissionRequest {
  id: string;
  turnId: string;
  toolCallId?: string;
  toolName: string;
  title: string;
  status: SessionPermissionStatus;
  risk: SessionPermissionRisk;
  scope?: string;
  reason?: string;
  argumentsSummary?: string;
  automatic?: boolean;
  decisionSource?: SessionPermissionDecisionSource;
  expiresAt?: number;
  bindingSummary?: string;
  requestedSeq: number;
  updatedSeq: number;
}

export function permissionRisk(value: unknown): SessionPermissionRisk {
  const risk = String(value || '').trim().toLowerCase();
  return risk === 'low' || risk === 'medium' || risk === 'high' ? risk : 'unknown';
}

export function permissionText(value: unknown, fallback = '', maxLength = 240) {
  const text = String(value || fallback).trim()
    .replace(EMBEDDED_SECRET, '[redacted]')
    .replace(/[\u0000-\u001f\u007f\u202a-\u202e\u2066-\u2069]/gu, ' ')
    .replace(/\s+/gu, ' ');
  return text.length <= maxLength ? text : `${text.slice(0, maxLength - 1)}…`;
}

export function permissionArgumentsSummary(params: Record<string, unknown>) {
  const summarized = object(params.argumentsSummary);
  const input = Object.keys(summarized).length ? summarized : object(params.arguments);
  return toolInputSummary(input);
}

export function permissionDecisionSource(value: unknown): SessionPermissionDecisionSource | undefined {
  const source = String(value || '').trim().toLowerCase();
  return source === 'deterministic' || source === 'assistant' || source === 'policy'
    ? source
    : undefined;
}

export function permissionExpiresAt(value: unknown) {
  const expiresAt = Number(value);
  return Number.isFinite(expiresAt) && expiresAt > 0 && expiresAt <= 8.64e15
    ? Math.trunc(expiresAt)
    : undefined;
}

export function permissionBindingSummary(value: unknown) {
  const binding = object(value);
  const fields: Array<[string, keyof typeof binding, number]> = [
    ['应用', 'application', 80],
    ['来源', 'origin', 160],
    ['工作区', 'workspaceId', 96],
    ['线程', 'threadId', 96],
    ['回合', 'turnId', 96],
    ['工具', 'toolName', 96]
  ];
  const parts = fields.flatMap(([label, key, maxLength]) => {
    const projected = permissionText(binding[key], '', maxLength);
    return projected ? [`${label} ${projected}`] : [];
  });
  const parameterDigest = String(binding.parameterDigest || '').trim().toLowerCase();
  if (/^[a-f0-9]{64}$/.test(parameterDigest)) {
    parts.push(`参数指纹 ${parameterDigest.slice(0, 12)}`);
  }
  return permissionText(parts.join(' · '), '', 640) || undefined;
}

export function permissionTitle(params: Record<string, unknown>, fallback = 'tool') {
  return permissionText(boundedToolLabel(params.title, boundedToolLabel(params.toolName, fallback)), fallback, 96);
}

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}
