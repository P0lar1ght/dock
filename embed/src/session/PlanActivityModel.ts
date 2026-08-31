export type SessionPlanStepStatus = 'pending' | 'in_progress' | 'completed';
export type SessionPlanStatus = 'running' | 'completed' | 'failed' | 'cancelled';

export interface SessionPlanStep {
  id: string;
  content: string;
  activeForm: string;
  status: SessionPlanStepStatus;
}

export interface SessionPlanActivity {
  id: string;
  turnId: string;
  sourceTool: 'TodoWrite' | 'TaskUpdate';
  status: SessionPlanStatus;
  steps: SessionPlanStep[];
  startedSeq: number;
  updatedSeq: number;
  updateCount: number;
  degraded: boolean;
}

const PLAN_TASK_TOOLS = new Set(['TodoWrite', 'TaskUpdate']);
const SECRET_TEXT = /(?:\bBearer\s+\S+|\b(?:sk|pk)-[A-Za-z0-9_-]{8,}|\b[A-Z][A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD)\s*=\s*\S+)/gi;
const MAX_STEPS = 40;
const MAX_STEP_CHARS = 240;

export function isPlanTaskTool(value: unknown): value is SessionPlanActivity['sourceTool'] {
  return PLAN_TASK_TOOLS.has(text(value, 64));
}

export function planActivityStarted(
  current: SessionPlanActivity | undefined,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
): SessionPlanActivity | undefined {
  const sourceTool = text(params.toolName, 64);
  if (!turnId || !isPlanTaskTool(sourceTool)) return undefined;

  const parsed = parseSteps(params.arguments, turnId);
  const steps = parsed.present ? parsed.steps : current?.steps || [];
  return {
    id: `plan:${turnId}`,
    turnId,
    sourceTool,
    status: steps.length === 0 || steps.every((step) => step.status === 'completed') ? 'completed' : 'running',
    steps,
    startedSeq: current?.startedSeq || seq,
    updatedSeq: seq,
    updateCount: (current?.updateCount || 0) + 1,
    degraded: parsed.degraded
  };
}

export function planActivityCompleted(
  current: SessionPlanActivity | undefined,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
): SessionPlanActivity | undefined {
  const sourceTool = text(params.toolName, 64);
  if (!turnId || !isPlanTaskTool(sourceTool)) return undefined;
  return current
    ? { ...current, updatedSeq: seq }
    : {
      id: `plan:${turnId}`,
      turnId,
      sourceTool,
      status: 'completed',
      steps: [],
      startedSeq: seq,
      updatedSeq: seq,
      updateCount: 1,
      degraded: true
    };
}

export function settlePlanActivity(
  activity: SessionPlanActivity,
  turnId: string,
  turnStatus: string,
  seq: number
) {
  if (activity.turnId !== turnId || turnStatus === 'waiting_permission') return activity;
  const status: SessionPlanStatus = turnStatus === 'cancelled'
    ? 'cancelled'
    : turnStatus === 'completed' || turnStatus === 'denied' ? 'completed' : 'failed';
  return { ...activity, status, updatedSeq: Math.max(activity.updatedSeq, seq) };
}

function parseSteps(value: unknown, turnId: string) {
  const args = object(value);
  const rawSteps = Array.isArray(args.tasks) ? args.tasks : Array.isArray(args.todos) ? args.todos : undefined;
  if (!rawSteps) return { present: false, degraded: true, steps: [] as SessionPlanStep[] };

  let degraded = rawSteps.length > MAX_STEPS;
  const steps = rawSteps.slice(0, MAX_STEPS).flatMap((value, index) => {
    const task = object(value);
    const content = text(task.content, MAX_STEP_CHARS);
    const activeForm = text(task.activeForm, MAX_STEP_CHARS);
    if (!content) {
      degraded = true;
      return [];
    }
    const normalized = normalizeStepStatus(task.status);
    if (normalized.degraded) degraded = true;
    return [{
      id: `plan:${turnId}:step:${index + 1}`,
      content,
      activeForm: activeForm || content,
      status: normalized.status
    }];
  });
  if (steps.length !== Math.min(rawSteps.length, MAX_STEPS)) degraded = true;
  return { present: true, degraded, steps };
}

function normalizeStepStatus(value: unknown): { status: SessionPlanStepStatus; degraded: boolean } {
  if (value === 'in_progress' || value === 'completed') return { status: value, degraded: false };
  return { status: 'pending', degraded: value !== 'pending' };
}

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function text(value: unknown, max: number) {
  if (typeof value !== 'string') return '';
  return value.replace(SECRET_TEXT, '[redacted]').replace(/\s+/g, ' ').trim().slice(0, max);
}
