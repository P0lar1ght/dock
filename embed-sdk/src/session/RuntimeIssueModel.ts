import type { SessionState } from './SessionState.js';

export type RuntimeIssueKind = 'model' | 'guardrail' | 'tooling' | 'approval' | 'runtime';
export type RuntimeIssueAction = 'retry' | 'edit' | 'dismiss';

export interface SessionRuntimeIssue {
  id: string;
  turnId: string;
  seq: number;
  code: string;
  kind: RuntimeIssueKind;
  title: string;
  detail: string;
  action: RuntimeIssueAction;
  relatedToolId?: string;
  relatedPermissionId?: string;
}

const EMBEDDED_SECRET = /(?:\bBearer\s+\S+|\b(?:sk|pk)-[A-Za-z0-9_-]{8,}|\b(?:api[_-]?key|authorization|cookie|credential|password|secret|session[_-]?key|token)\b\s*[:=]\s*(?:"[^"]*"|'[^']*'|\S+)|\b[A-Z][A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD)\s*=\s*\S+)/gi;
const UNIX_LOCAL_PATH = /(^|[\s("'`])((?:\/(?!\/)[^\s"'`<>]+){2,})/g;
const WINDOWS_LOCAL_PATH = /\b[A-Za-z]:\\[^\s"'`<>]+/g;
const MAX_DETAIL_CHARS = 420;

export function projectRuntimeIssue(
  state: SessionState,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
) {
  if (!turnId) return state;
  const code = safeCode(params.code || params.kind || params.type);
  const classification = classifyIssue(code);
  const issue: SessionRuntimeIssue = {
    id: `issue:${turnId}:${seq || state.lastSeq + 1}`,
    turnId,
    seq,
    code,
    kind: classification.kind,
    title: classification.title,
    detail: classification.fallback,
    action: classification.action,
    relatedToolId: safeInline(params.toolCallId || params.itemId, 160) || undefined,
    relatedPermissionId: safeInline(params.requestId, 160) || undefined
  };
  return upsertIssue(state, issue);
}

export function projectTurnFailureIssue(
  state: SessionState,
  turnId: string,
  status: string,
  seq: number
) {
  if (!turnId || !isFailureStatus(status) || state.runtimeIssues.some((issue) => issue.turnId === turnId)) {
    return state;
  }
  return upsertIssue(state, {
    id: `issue:${turnId}:${seq || state.lastSeq + 1}`,
    turnId,
    seq,
    code: status || 'runtime_failed',
    kind: 'model',
    title: status === 'guardrail_tripped' ? '请求被安全策略阻止' : '本轮运行失败',
    detail: status === 'guardrail_tripped'
      ? '请修改请求内容后重新发送。'
      : 'Gateway 没有返回可用的完成结果。',
    action: status === 'guardrail_tripped' ? 'edit' : 'retry'
  });
}

export function safeRuntimeIssueText(value: unknown, maxLength = MAX_DETAIL_CHARS) {
  return safeDetail(value, maxLength);
}

function upsertIssue(state: SessionState, issue: SessionRuntimeIssue) {
  const duplicate = state.runtimeIssues.some((current) =>
    current.id === issue.id
    || (current.turnId === issue.turnId && current.code === issue.code && current.seq === issue.seq)
  );
  return duplicate ? state : { ...state, runtimeIssues: [...state.runtimeIssues, issue] };
}

function classifyIssue(codeValue: string): {
  kind: RuntimeIssueKind;
  title: string;
  fallback: string;
  action: RuntimeIssueAction;
} {
  const code = codeValue.toLowerCase();
  if (code.includes('guardrail')) {
    return {
      kind: 'guardrail',
      title: '请求被安全策略阻止',
      fallback: '请修改请求内容后重新发送。',
      action: 'edit'
    };
  }
  if (code.includes('permission') || code.includes('approval')) {
    return {
      kind: 'approval',
      title: '审批状态无法恢复',
      fallback: '原审批已失效，可以把原消息作为新一轮重试。',
      action: 'retry'
    };
  }
  if (code.includes('mcp') || code.includes('tool')) {
    return {
      kind: 'tooling',
      title: '所需本地能力不可用',
      fallback: '请检查本机工具配置后重试本轮。',
      action: 'retry'
    };
  }
  if (code.includes('model') || code.includes('maxturn') || code.includes('provider')) {
    return {
      kind: 'model',
      title: '模型运行未完成',
      fallback: '可以重新提交本轮消息。',
      action: 'retry'
    };
  }
  return {
    kind: 'runtime',
    title: '本轮运行失败',
    fallback: 'Gateway 已保留本轮历史，可以安全重试。',
    action: 'retry'
  };
}

function isFailureStatus(status: string) {
  return status === 'failed' || status === 'guardrail_tripped';
}

function safeInline(value: unknown, limit: number) {
  if (typeof value !== 'string') return '';
  return safeDetail(value.replace(/\s+/gu, ' '), limit);
}

function safeCode(value: unknown) {
  if (typeof value !== 'string') return 'runtime_failed';
  const code = value.trim();
  return /^[A-Za-z0-9_.:-]{1,96}$/u.test(code) ? code : 'runtime_failed';
}

function safeDetail(value: unknown, limit = MAX_DETAIL_CHARS) {
  if (typeof value !== 'string') return '';
  const redacted = value
    .replace(EMBEDDED_SECRET, '[redacted]')
    .replace(UNIX_LOCAL_PATH, '$1[local path]')
    .replace(WINDOWS_LOCAL_PATH, '[local path]')
    .replace(/\r\n?/gu, '\n')
    .replace(/[ \t]+/gu, ' ')
    .replace(/\n{3,}/gu, '\n\n')
    .trim();
  return redacted.length <= limit ? redacted : `${redacted.slice(0, limit - 1)}…`;
}
