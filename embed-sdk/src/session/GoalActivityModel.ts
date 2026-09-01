import type { ThreadGoalStatus } from '../protocol/responses.js';

export interface SessionGoalActivity {
  id: string;
  turnId: string;
  revision: number;
  status: Exclude<ThreadGoalStatus, 'none'>;
  summary: string;
  progressSummary?: string;
  completedSteps: number;
  totalSteps: number;
  continuationCount: number;
  maxContinuations: number;
  blockedReason?: string;
  startedSeq: number;
  updatedSeq: number;
}

export function projectGoalActivity(
  current: SessionGoalActivity | undefined,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
): SessionGoalActivity | undefined {
  const goal = object(params.goal);
  const revision = positive(goal.revision);
  const status = goalStatus(goal.status);
  if (!revision || !status) return undefined;
  return {
    id: `goal:${revision}`,
    turnId: turnId || current?.turnId || '',
    revision,
    status,
    summary: safeText(goal.summary, 220) || current?.summary || '',
    progressSummary: safeText(goal.progressSummary, 500) || current?.progressSummary,
    completedSteps: nonNegative(goal.completedSteps, current?.completedSteps || 0),
    totalSteps: nonNegative(goal.totalSteps, current?.totalSteps || 0),
    continuationCount: nonNegative(
      goal.continuationCount,
      current?.continuationCount || 0
    ),
    maxContinuations: nonNegative(goal.maxContinuations, current?.maxContinuations || 0),
    blockedReason: safeText(goal.blockedReason, 500) || current?.blockedReason,
    startedSeq: current?.startedSeq || seq,
    updatedSeq: Math.max(seq, current?.updatedSeq || 0)
  };
}

function goalStatus(value: unknown): SessionGoalActivity['status'] | undefined {
  return value === 'active'
    || value === 'paused'
    || value === 'blocked'
    || value === 'completed'
    ? value
    : undefined;
}

function safeText(value: unknown, max: number) {
  return String(value || '')
    .replace(
      /(?:\bBearer\s+\S+|\b(?:sk|pk)-[A-Za-z0-9_-]{8,}|\b[A-Z][A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD)\s*=\s*\S+)/gi,
      '[redacted]'
    )
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, max);
}

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function positive(value: unknown) {
  const parsed = Math.floor(Number(value));
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 0;
}

function nonNegative(value: unknown, fallback: number) {
  const parsed = Math.floor(Number(value));
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback;
}
