import type { ContextCompactionStatus, ThreadEnvironmentResult } from '../protocol/responses.js';

export type SessionEnvironment = ThreadEnvironmentResult;

export function normalizeEnvironment(value: ThreadEnvironmentResult): SessionEnvironment {
  const maxContextTokens = positive(value.context.maxContextTokens, 1);
  const reservedOutputTokens = Math.min(
    positive(value.context.reservedOutputTokens, 0),
    maxContextTokens
  );
  const estimatedTokens = positive(value.context.estimatedTokens, 0);
  const triggerPercent = bounded(value.context.compactionTriggerPercent, 1, 100);
  return {
    threadId: safeText(value.threadId, ''),
    model: {
      id: safeText(value.model?.id, 'model'),
      label: safeText(value.model?.label, 'Model'),
      inputModalities: normalizeInputModalities(value.model?.inputModalities),
      canChange: Boolean(value.model?.canChange),
      options: normalizeModelOptions(value.model?.options),
      discovery: normalizeModelDiscovery(value.model?.discovery, value.model?.options)
    },
    approval: {
      mode: approvalMode(value.approval?.mode),
      canChange: Boolean(value.approval?.canChange),
      options: normalizeApprovalOptions(value.approval?.options)
    },
    reasoning: {
      supported: Boolean(value.reasoning?.supported),
      effort: reasoningEffort(value.reasoning?.effort),
      canChange: Boolean(value.reasoning?.canChange),
      options: normalizeReasoningOptions(value.reasoning?.options)
    },
    goal: normalizeGoal(value.goal),
    plan: {
      enabled: Boolean(value.plan?.enabled),
      canChange: Boolean(value.plan?.canChange)
    },
    memory: {
      read: Boolean(value.memory?.read),
      write: Boolean(value.memory?.write),
      canRead: Boolean(value.memory?.canRead),
      canWrite: Boolean(value.memory?.canWrite)
    },
    mcp: normalizeMcp(value.mcp),
    rules: normalizeRules(value.rules),
    context: {
      estimatedTokens,
      maxContextTokens,
      inputBudgetTokens: Math.min(
        positive(value.context.inputBudgetTokens, maxContextTokens - reservedOutputTokens),
        maxContextTokens
      ),
      reservedOutputTokens,
      usagePercent: percent(estimatedTokens, maxContextTokens),
      compactionEnabled: Boolean(value.context.compactionEnabled),
      canCompact: Boolean(value.context.canCompact),
      compactionTriggerPercent: triggerPercent,
      compactionTriggerTokens: Math.min(
        positive(value.context.compactionTriggerTokens, Math.round(maxContextTokens * triggerPercent / 100)),
        maxContextTokens
      ),
      revisionSeq: positive(value.context.revisionSeq, 0),
      lastCompaction: normalizeCompaction(value.context.lastCompaction)
    }
  };
}

export function mergeEnvironment(
  current: SessionEnvironment | undefined,
  incoming: ThreadEnvironmentResult
) {
  const normalized = normalizeEnvironment(incoming);
  if (!current || normalized.context.revisionSeq >= current.context.revisionSeq) return normalized;
  return current;
}

export function reduceContextUsage(
  environment: SessionEnvironment | undefined,
  method: string,
  params: Record<string, unknown>,
  seq: number
): SessionEnvironment | undefined {
  if (!environment || seq <= environment.context.revisionSeq) return environment;
  if (method === 'context/projected') return projectUsage(environment, params, seq);
  if (method === 'context/compacted') return projectCompaction(environment, params, seq);
  return environment;
}

export function reduceModelSelection(
  environment: SessionEnvironment | undefined,
  params: Record<string, unknown>
): SessionEnvironment | undefined {
  if (!environment) return environment;
  const raw = objectValue(params.model);
  const options = normalizeModelOptions(raw.options || environment.model.options);
  const id = safeText(raw.id, '');
  if (!id || !options.some((model) => model.id === id)) return environment;
  return {
    ...environment,
    model: {
      id,
      label: safeText(raw.label, options.find((model) => model.id === id)?.label || 'Model'),
      inputModalities: normalizeInputModalities(
        raw.inputModalities
        || options.find((model) => model.id === id)?.inputModalities
      ),
      canChange: Boolean(raw.canChange ?? environment.model.canChange),
      options,
      discovery: normalizeModelDiscovery(raw.discovery, options)
    }
  };
}

export function reduceReasoningSelection(
  environment: SessionEnvironment | undefined,
  params: Record<string, unknown>
): SessionEnvironment | undefined {
  if (!environment) return environment;
  const raw = objectValue(params.reasoning);
  const options = normalizeReasoningOptions(raw.options || environment.reasoning.options);
  const effort = reasoningEffort(raw.effort);
  if (effort && !options.includes(effort)) return environment;
  return {
    ...environment,
    reasoning: {
      supported: Boolean(raw.supported ?? options.length),
      effort,
      canChange: Boolean(raw.canChange ?? environment.reasoning.canChange),
      options
    }
  };
}

export function reduceApprovalSelection(
  environment: SessionEnvironment | undefined,
  params: Record<string, unknown>
): SessionEnvironment | undefined {
  if (!environment) return environment;
  const raw = objectValue(params.approval);
  const options = normalizeApprovalOptions(raw.options || environment.approval.options);
  const mode = approvalMode(raw.mode);
  if (!options.includes(mode)) return environment;
  return {
    ...environment,
    approval: {
      mode,
      canChange: Boolean(raw.canChange ?? environment.approval.canChange),
      options
    }
  };
}

export function reduceGoalSelection(
  environment: SessionEnvironment | undefined,
  params: Record<string, unknown>
): SessionEnvironment | undefined {
  if (!environment) return environment;
  return { ...environment, goal: normalizeGoal(objectValue(params.goal)) };
}

export function reducePlanSelection(
  environment: SessionEnvironment | undefined,
  params: Record<string, unknown>
): SessionEnvironment | undefined {
  if (!environment) return environment;
  const raw = objectValue(params.plan);
  return {
    ...environment,
    plan: {
      enabled: Boolean(raw.enabled),
      canChange: Boolean(raw.canChange ?? environment.plan.canChange)
    }
  };
}

export function reduceMemorySelection(
  environment: SessionEnvironment | undefined,
  params: Record<string, unknown>
): SessionEnvironment | undefined {
  if (!environment) return environment;
  const raw = objectValue(params.memory);
  return {
    ...environment,
    memory: {
      read: Boolean(raw.read),
      write: Boolean(raw.write),
      canRead: Boolean(raw.canRead ?? environment.memory.canRead),
      canWrite: Boolean(raw.canWrite ?? environment.memory.canWrite)
    }
  };
}

function projectUsage(
  environment: SessionEnvironment,
  params: Record<string, unknown>,
  seq: number
) {
  if (safeText(params.stage, '') !== 'model_input') return environment;
  const estimatedTokens = positive(params.afterTokens, environment.context.estimatedTokens);
  return withContext(environment, {
    ...environment.context,
    estimatedTokens,
    usagePercent: percent(estimatedTokens, environment.context.maxContextTokens),
    revisionSeq: seq
  });
}

function projectCompaction(
  environment: SessionEnvironment,
  params: Record<string, unknown>,
  seq: number
) {
  const status = compactionStatus(params.status);
  if (!status) return environment;
  const beforeTokens = positive(params.beforeTokens, environment.context.estimatedTokens);
  const afterTokens = positive(
    params.afterTokens,
    status === 'completed' || status === 'failed' ? environment.context.estimatedTokens : beforeTokens
  );
  const estimatedTokens = status === 'completed' || status === 'failed' ? afterTokens : beforeTokens;
  return withContext(environment, {
    ...environment.context,
    estimatedTokens,
    usagePercent: percent(estimatedTokens, environment.context.maxContextTokens),
    revisionSeq: seq,
    lastCompaction: {
      status,
      beforeTokens,
      afterTokens,
      usagePercent: finite(params.usagePercent, percent(beforeTokens, environment.context.maxContextTokens)),
      seq,
      failureCategory: safeText(params.failureCategory ?? params.category, '') || undefined
    }
  });
}

function withContext(environment: SessionEnvironment, context: SessionEnvironment['context']) {
  return { ...environment, context };
}

function normalizeCompaction(value: SessionEnvironment['context']['lastCompaction']) {
  if (!value) return undefined;
  const status = compactionStatus(value.status);
  if (!status) return undefined;
  return {
    status,
    beforeTokens: positive(value.beforeTokens, 0),
    afterTokens: positive(value.afterTokens, 0),
    usagePercent: finite(value.usagePercent, 0),
    seq: positive(value.seq, 0),
    failureCategory: safeText(value.failureCategory, '') || undefined
  };
}

function compactionStatus(value: unknown): ContextCompactionStatus | undefined {
  const status = safeText(value, '');
  return status === 'running' || status === 'completed' || status === 'failed' || status === 'suppressed'
    ? status
    : undefined;
}

function approvalMode(value: unknown): SessionEnvironment['approval']['mode'] {
  return value === 'ask' || value === 'full_access' ? value : 'auto';
}

function reasoningEffort(value: unknown): SessionEnvironment['reasoning']['effort'] {
  const effort = safeText(value, '');
  return ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].includes(effort)
    ? effort as SessionEnvironment['reasoning']['effort']
    : undefined;
}

function normalizeReasoningOptions(value: unknown) {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.map(reasoningEffort).filter(Boolean))]
    .slice(0, 7) as SessionEnvironment['reasoning']['options'];
}

function normalizeApprovalOptions(value: unknown) {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.map((item) => approvalMode(item)))]
    .filter((item) => value.includes(item))
    .slice(0, 3) as SessionEnvironment['approval']['options'];
}

function normalizeModelOptions(value: unknown) {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  return value.flatMap((item) => {
    const raw = objectValue(item);
    const id = safeText(raw.id, '');
    if (!id || seen.has(id)) return [];
    seen.add(id);
    return [{
      id,
      label: safeText(raw.label, 'Model'),
      providerLabel: safeText(raw.providerLabel, '') || undefined,
      available: raw.available !== false,
      inputModalities: normalizeInputModalities(raw.inputModalities)
    }];
  }).slice(0, 64);
}

function normalizeInputModalities(value: unknown): Array<'text' | 'image'> {
  return Array.isArray(value) && value.includes('image') ? ['text', 'image'] : ['text'];
}

function normalizeModelDiscovery(
  value: unknown,
  optionsValue: unknown
): NonNullable<SessionEnvironment['model']['discovery']> {
  const raw = objectValue(value);
  const options = normalizeModelOptions(optionsValue);
  const status = ['idle', 'refreshing', 'ready', 'partial', 'failed'].includes(String(raw.status))
    ? raw.status as NonNullable<SessionEnvironment['model']['discovery']>['status']
    : 'idle';
  return {
    status,
    canRefresh: Boolean(raw.canRefresh),
    configuredCount: positive(raw.configuredCount, options.length),
    totalCount: positive(raw.totalCount, options.length),
    availableCount: positive(
      raw.availableCount,
      options.filter((model) => model.available !== false).length
    ),
    refreshedAt: positiveTimestamp(raw.refreshedAt)
  };
}

function normalizeGoal(value: unknown): SessionEnvironment['goal'] {
  const raw = objectValue(value);
  const status = ['active', 'paused', 'blocked', 'completed'].includes(String(raw.status))
    ? raw.status as SessionEnvironment['goal']['status']
    : 'none';
  return {
    status,
    summary: boundedText(raw.summary, 220),
    truncated: Boolean(raw.truncated),
    revision: positive(raw.revision, 0) || undefined,
    progressSummary: boundedText(raw.progressSummary, 500) || undefined,
    completedSteps: positive(raw.completedSteps, 0),
    totalSteps: positive(raw.totalSteps, 0),
    continuationCount: positive(raw.continuationCount, 0),
    maxContinuations: positive(raw.maxContinuations, 0),
    blockedReason: boundedText(raw.blockedReason, 500) || undefined,
    updatedAt: positiveTimestamp(raw.updatedAt),
    completedAt: positiveTimestamp(raw.completedAt)
  };
}

function normalizeMcp(value: unknown): SessionEnvironment['mcp'] {
  const raw = objectValue(value);
  const servers = Array.isArray(raw.servers)
    ? raw.servers.flatMap((item, index) => {
      const server = objectValue(item);
      const status = mcpStatus(server.status);
      return [{
        id: `mcp-${index + 1}`,
        label: safeMetadataLabel(server.label, `MCP Server ${index + 1}`),
        status,
        toolCount: status === 'connected' ? positive(server.toolCount, 0) : 0
      }];
    }).slice(0, 24)
    : [];
  return {
    enabled: Boolean(raw.enabled),
    connectedServers: positive(raw.connectedServers, servers.filter((server) => server.status === 'connected').length),
    totalServers: positive(raw.totalServers, servers.length),
    toolCount: positive(raw.toolCount, servers.reduce((total, server) => total + server.toolCount, 0)),
    refreshedAt: positiveTimestamp(raw.refreshedAt),
    servers
  };
}

function normalizeRules(value: unknown): SessionEnvironment['rules'] {
  const raw = objectValue(value);
  const order = Array.isArray(raw.order)
    ? raw.order.flatMap((item, index) => {
      const rule = objectValue(item);
      const scope: SessionEnvironment['rules']['order'][number]['scope'] = rule.scope === 'global'
        ? 'global'
        : 'workspace';
      return [{
        order: index + 1,
        scope,
        label: safeMetadataLabel(
          rule.label,
          scope === 'global' ? '全局 AGENTS.md' : `Workspace AGENTS.md ${index + 1}`
        )
      }];
    }).slice(0, 16)
    : [];
  return { appliedCount: positive(raw.appliedCount, order.length), order };
}

function mcpStatus(value: unknown): SessionEnvironment['mcp']['servers'][number]['status'] {
  return value === 'connected' || value === 'disabled' ? value : 'unavailable';
}

function safeMetadataLabel(value: unknown, fallback: string) {
  const label = boundedText(value, 80).replace(/\s+/g, ' ');
  if (
    !label
    || /:\/\/|[/\\@=$]|\[REDACTED\]|\b(?:Bearer|Basic|token|secret|password|api[-_ ]?key)\b/iu.test(label)
  ) return fallback;
  return label;
}

function objectValue(value: unknown) {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function percent(value: number, total: number) {
  return total > 0 ? Math.max(0, value / total * 100) : 0;
}

function positive(value: unknown, fallback: number) {
  return Math.max(0, Math.floor(finite(value, fallback)));
}

function bounded(value: unknown, minimum: number, maximum: number) {
  return Math.min(maximum, Math.max(minimum, finite(value, minimum)));
}

function finite(value: unknown, fallback: number) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

function safeText(value: unknown, fallback: string) {
  const text = String(value ?? '').trim();
  return text ? text.slice(0, 160) : fallback;
}

function boundedText(value: unknown, limit: number) {
  return String(value ?? '').trim().slice(0, limit);
}

function positiveTimestamp(value: unknown) {
  const timestamp = Math.floor(Number(value));
  return Number.isFinite(timestamp) && timestamp > 0 ? timestamp : undefined;
}
