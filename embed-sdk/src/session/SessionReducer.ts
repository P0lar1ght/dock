import type { RuntimeNotification } from '../protocol/notifications.js';
import type { ThreadHistoryResult, TranscriptEvent } from '../protocol/responses.js';
import type { SessionMessage } from './MessageModel.js';
import {
  appendAssistantDelta,
  closeAssistantSegment,
  hasTranscriptMessage,
  projectUserMessage,
  settleAssistantSegments,
  storedMessages
} from './MessageProjector.js';
import {
  permissionArgumentsSummary,
  permissionBindingSummary,
  permissionDecisionSource,
  permissionExpiresAt,
  permissionRisk,
  permissionText,
  permissionTitle,
  type SessionPermissionRequest
} from './PermissionModel.js';
import { initialSessionState, type SessionState, type SessionTurnStatus } from './SessionState.js';
import {
  isPlanTaskTool,
  planActivityCompleted,
  planActivityStarted,
  settlePlanActivity,
  type SessionPlanActivity
} from './PlanActivityModel.js';
import { projectGoalActivity, type SessionGoalActivity } from './GoalActivityModel.js';
import {
  isActiveSubagent,
  projectSubagentActivity,
  type SessionSubagentActivity
} from './SubagentActivityModel.js';
import {
  boundedToolLabel,
  finiteDuration,
  toolInputSummary,
  toolOutputPresentation,
  type SessionToolActivity
} from './ToolActivityModel.js';
import {
  projectDequeuedTurn,
  projectQueuedTurn,
  removeQueueItem
} from './TurnQueueModel.js';
import { projectRuntimeIssue, projectTurnFailureIssue } from './RuntimeIssueModel.js';
import {
  reduceApprovalSelection,
  reduceContextUsage,
  reduceGoalSelection,
  reduceMemorySelection,
  reducePlanSelection,
  reduceModelSelection,
  reduceReasoningSelection
} from './ContextUsageModel.js';
import { reduceThreadLifecycle } from './ThreadLifecycleModel.js';
import {
  settleUserInputForTurn,
  withUserInputRequested,
  withUserInputResolved
} from './UserInputModel.js';

export function restoreSession(history: ThreadHistoryResult): SessionState {
  let state = initialSessionState(history.thread);
  const ordered = [...history.events].sort((left, right) => left.seq - right.seq);
  for (const event of ordered) state = reduceSession(state, transcriptNotification(event));
  if (!ordered.some(hasTranscriptMessage)) {
    state = { ...state, messages: storedMessages(history.messages) };
  }
  return state;
}

export function reduceSession(state: SessionState, notification: RuntimeNotification): SessionState {
  const seq = sequence(notification.params);
  if (seq && seq <= state.lastSeq) return state;
  const params = notification.params;
  const turnId = text(params.turnId);
  let next = state;
  switch (notification.method) {
    case 'item/user_message':
      if (turnId) next = withActivity(projectUserMessage(state, turnId, text(params.content), seq, params.attachments), 'thinking');
      break;
    case 'item/message_delta':
      if (turnId) next = withActivity(appendAssistantDelta(state, turnId, messageDelta(params), seq), 'streaming');
      break;
    case 'turn/started':
      if (turnId) next = withActivity(withTurn(state, turnId, 'running'), 'thinking');
      break;
    case 'turn/queued':
      next = projectQueuedTurn(state, params, seq);
      break;
    case 'turn/dequeued':
      next = projectDequeuedTurn(state, params);
      break;
    case 'turn/queue/removed':
      next = removeQueueItem(state, text(params.queueId));
      break;
    case 'permission/requested':
      if (turnId) {
        const segmented = closeAssistantSegment(state, turnId);
        next = withActivity(
          withTurn(withPermissionRequested(segmented, turnId, params, seq), turnId, 'waiting_permission'),
          'approval'
        );
      }
      break;
    case 'permission/resolved':
      next = withActivity(
        withPermissionResolved(state, params, seq),
        text(params.decision) === 'approve' ? 'working' : 'thinking'
      );
      break;
    case 'permission/automatic':
      if (turnId) {
        next = withPermissionResolved(
          withPermissionRequested(state, turnId, params, seq, true),
          { ...params, decision: 'approve' },
          seq
        );
      }
      break;
    case 'interaction/requested':
      if (turnId && params.kind === 'user_input') {
        const segmented = closeAssistantSegment(state, turnId);
        next = withActivity(
          withTurn(withUserInputRequested(segmented, turnId, params, seq), turnId, 'waiting_input'),
          'input'
        );
      }
      break;
    case 'interaction/resolved':
      next = withActivity(withUserInputResolved(state, params), 'working');
      break;
    case 'subagent/waiting_permission':
      if (turnId) {
        const segmented = closeAssistantSegment(state, turnId);
        next = withActivity(
          withTurn(withSubagentEvent(segmented, turnId, notification.method, params, seq), turnId, 'waiting_permission'),
          'approval'
        );
      }
      break;
    case 'item/tool_started': {
      const segmented = turnId ? closeAssistantSegment(state, turnId) : state;
      next = withActivity(
        isPlanTaskTool(params.toolName)
          ? withPlanStarted(segmented, turnId, params, seq)
          : withToolStarted(segmented, turnId, params, seq),
        'working'
      );
      break;
    }
    case 'item/tool_progress':
      next = withActivity(withToolProgress(state, params, seq), 'working');
      break;
    case 'item/tool_completed':
      next = withActivity(
        isPlanTaskTool(params.toolName)
          ? withPlanCompleted(state, turnId, params, seq)
          : withToolCompleted(state, turnId, params, seq),
        'thinking'
      );
      break;
    case 'subagent/started':
    case 'subagent/backgrounded': {
      const segmented = turnId ? closeAssistantSegment(state, turnId) : state;
      next = withActivity(withSubagentEvent(segmented, turnId, notification.method, params, seq), 'subagent');
      break;
    }
    case 'subagent/completed':
    case 'subagent/cancelled':
    case 'subagent/failed': {
      const projected = withSubagentEvent(state, turnId, notification.method, params, seq);
      next = withActivity(projected, activityAfterSubagent(projected, turnId));
      break;
    }
    case 'turn/completed':
      if (turnId) {
        const status = turnStatus(params.status);
        const completed = status === 'waiting_permission'
          ? withActivity(withTurn(state, turnId, status), 'approval')
          : status === 'waiting_input'
            ? withActivity(withTurn(state, turnId, status), 'input')
          : activityAfterTurn(completeTurn(state, turnId, status, seq), status);
        next = projectTurnFailureIssue(
          settleUserInputForTurn(completed, turnId, status),
          turnId,
          text(params.status),
          seq
        );
      }
      break;
    case 'error':
      next = withPermissionFailed(state, params, seq);
      next = projectRuntimeIssue(next, turnId, params, seq);
      next = {
        ...next,
        activity: 'error',
        error: next.runtimeIssues.at(-1)?.title || 'Agent turn failed'
      };
      break;
    case 'context/projected':
    case 'context/compacted':
      next = {
        ...state,
        environment: reduceContextUsage(state.environment, notification.method, params, seq)
      };
      break;
    case 'thread/title_changed': {
      const title = text(params.title).slice(0, 120);
      if (title) {
        const updatedAt = Number(params.updatedAt);
        next = {
          ...state,
          thread: {
            ...state.thread,
            title,
            updatedAt: Number.isFinite(updatedAt) ? updatedAt : state.thread.updatedAt
          }
        };
      }
      break;
    }
    case 'thread/renamed':
    case 'thread/archived':
    case 'thread/restored':
    case 'thread/deleted':
      next = reduceThreadLifecycle(state, notification);
      break;
    case 'thread/model/changed':
      next = {
        ...state,
        environment: reduceModelSelection(state.environment, params)
      };
      break;
    case 'thread/reasoning/changed':
      next = { ...state, environment: reduceReasoningSelection(state.environment, params) };
      break;
    case 'thread/approval/changed':
      next = { ...state, environment: reduceApprovalSelection(state.environment, params) };
      break;
    case 'thread/goal/changed':
    case 'goal/progress':
    case 'goal/completed':
    case 'goal/blocked':
    case 'goal/paused':
    case 'goal/continuation_scheduled':
    case 'goal/continuation_started':
      next = withGoalActivity(
        { ...state, environment: reduceGoalSelection(state.environment, params) },
        turnId,
        params,
        seq
      );
      break;
    case 'thread/plan/changed':
      next = { ...state, environment: reducePlanSelection(state.environment, params) };
      break;
    case 'thread/memory/changed':
      next = { ...state, environment: reduceMemorySelection(state.environment, params) };
      break;
    default:
      break;
  }

  return {
    ...next,
    lastSeq: Math.max(next.lastSeq, seq)
  };
}

export function transcriptNotification(event: TranscriptEvent): RuntimeNotification {
  return {
    method: event.method,
    params: {
      ...event.payload,
      threadId: text(event.payload.threadId) || event.threadId,
      turnId: text(event.payload.turnId) || event.turnId,
      seq: event.seq,
      transcriptSeq: event.seq
    }
  };
}

function withTurn(state: SessionState, turnId: string, status: SessionTurnStatus) {
  return {
    ...state,
    turns: { ...state.turns, [turnId]: { id: turnId, status } }
  };
}

function withPlanStarted(
  state: SessionState,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
) {
  const current = state.planActivities.find((plan) => plan.turnId === turnId);
  const activity = planActivityStarted(current, turnId, params, seq);
  return activity ? upsertPlan(state, activity) : state;
}

function withPlanCompleted(
  state: SessionState,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
) {
  const current = state.planActivities.find((plan) => plan.turnId === turnId);
  const activity = planActivityCompleted(current, turnId, params, seq);
  return activity ? upsertPlan(state, activity) : state;
}

function withToolStarted(
  state: SessionState,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
) {
  const id = toolId(params);
  if (!id || !turnId) return state;
  const current = state.toolActivities.find((tool) => tool.id === id);
  const toolName = boundedToolLabel(params.toolName, current?.toolName || 'tool');
  return upsertTool(state, {
    id,
    turnId,
    toolName,
    title: boundedToolLabel(params.title, toolName),
    status: 'running',
    resultType: boundedToolLabel(params.resultType, 'tool_call'),
    startedSeq: current?.startedSeq || seq,
    updatedSeq: seq,
    truncated: false,
    inputSummary: toolInputSummary(params.arguments) || current?.inputSummary
  });
}

function withSubagentEvent(
  state: SessionState,
  turnId: string,
  method: string,
  params: Record<string, unknown>,
  seq: number
) {
  const payload = object(params.subagent);
  const id = text(payload.id);
  const current = state.subagentActivities.find((subagent) => subagent.id === id);
  const activity = projectSubagentActivity(current, turnId, method, params, seq);
  return activity ? upsertSubagent(state, activity) : state;
}

function withToolProgress(state: SessionState, params: Record<string, unknown>, seq: number) {
  const id = toolId(params);
  const current = state.toolActivities.find((tool) => tool.id === id);
  if (!current) return state;
  return upsertTool(state, {
    ...current,
    status: 'running',
    updatedSeq: seq,
    resultType: boundedToolLabel(params.resultType, current.resultType)
  });
}

function withToolCompleted(
  state: SessionState,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
) {
  const id = toolId(params);
  if (!id || !turnId) return state;
  const current = state.toolActivities.find((tool) => tool.id === id);
  const toolName = boundedToolLabel(params.toolName, current?.toolName || 'tool');
  const resultType = boundedToolLabel(params.resultType, current?.resultType || 'result');
  const output = toolOutputPresentation(params.output, resultType);
  return upsertTool(state, {
    id,
    turnId,
    toolName,
    title: boundedToolLabel(params.title, current?.title || toolName),
    status: toolTerminalStatus(params.status),
    resultType,
    startedSeq: current?.startedSeq || seq,
    updatedSeq: seq,
    durationMs: finiteDuration(params.durationMs),
    truncated: Boolean(params.truncated),
    inputSummary: current?.inputSummary,
    ...output
  });
}

function withPermissionRequested(
  state: SessionState,
  turnId: string,
  params: Record<string, unknown>,
  seq: number,
  automatic = false
) {
  const id = text(params.requestId);
  if (!id) return state;
  const current = state.permissionRequests.find((request) => request.id === id);
  const toolName = permissionText(params.toolName, current?.toolName || 'tool', 96);
  return upsertPermission(state, {
    id,
    turnId,
    toolCallId: permissionText(params.toolCallId || params.itemId, current?.toolCallId || '', 128) || undefined,
    toolName,
    title: permissionTitle(params, current?.title || toolName),
    status: 'pending',
    risk: permissionRisk(params.risk),
    scope: permissionText(params.scope || params.source, current?.scope || '', 160) || undefined,
    reason: permissionText(params.reason || params.summary, current?.reason || '', 320) || undefined,
    argumentsSummary: permissionArgumentsSummary(params) || current?.argumentsSummary,
    automatic: automatic || current?.automatic || undefined,
    decisionSource: automatic
      ? permissionDecisionSource(params.decisionSource)
      : current?.decisionSource,
    expiresAt: automatic ? permissionExpiresAt(params.expiresAt) : current?.expiresAt,
    bindingSummary: automatic
      ? permissionBindingSummary(params.binding)
      : current?.bindingSummary,
    requestedSeq: current?.requestedSeq || seq,
    updatedSeq: seq
  });
}

function withPermissionResolved(state: SessionState, params: Record<string, unknown>, seq: number) {
  const id = text(params.requestId);
  const current = state.permissionRequests.find((request) => request.id === id);
  if (!current) return state;
  return upsertPermission(state, {
    ...current,
    status: text(params.decision) === 'approve' ? 'approved' : 'denied',
    updatedSeq: seq
  });
}

function withPermissionFailed(state: SessionState, params: Record<string, unknown>, seq: number) {
  const id = text(params.requestId);
  const current = state.permissionRequests.find((request) => request.id === id);
  if (!current) return state;
  return upsertPermission(state, { ...current, status: 'failed', updatedSeq: seq });
}

function upsertPermission(state: SessionState, request: SessionPermissionRequest) {
  const permissionRequests = state.permissionRequests.some((item) => item.id === request.id)
    ? state.permissionRequests.map((item) => item.id === request.id ? request : item)
    : [...state.permissionRequests, request];
  return { ...state, permissionRequests };
}

function upsertTool(state: SessionState, activity: SessionToolActivity) {
  const toolActivities = state.toolActivities.some((tool) => tool.id === activity.id)
    ? state.toolActivities.map((tool) => tool.id === activity.id ? activity : tool)
    : [...state.toolActivities, activity];
  return { ...state, toolActivities };
}

function upsertPlan(state: SessionState, activity: SessionPlanActivity) {
  const planActivities = state.planActivities.some((plan) => plan.id === activity.id)
    ? state.planActivities.map((plan) => plan.id === activity.id ? activity : plan)
    : [...state.planActivities, activity];
  return { ...state, planActivities };
}

function withGoalActivity(
  state: SessionState,
  turnId: string,
  params: Record<string, unknown>,
  seq: number
) {
  const revision = Math.floor(Number(object(params.goal).revision));
  const current = state.goalActivities.find((goal) => goal.revision === revision);
  const activity = projectGoalActivity(current, turnId, params, seq);
  return activity ? upsertGoal(state, activity) : state;
}

function upsertGoal(state: SessionState, activity: SessionGoalActivity) {
  const goalActivities = state.goalActivities.some((goal) => goal.id === activity.id)
    ? state.goalActivities.map((goal) => goal.id === activity.id ? activity : goal)
    : [...state.goalActivities, activity];
  return { ...state, goalActivities };
}

function upsertSubagent(state: SessionState, activity: SessionSubagentActivity) {
  const subagentActivities = state.subagentActivities.some((subagent) => subagent.id === activity.id)
    ? state.subagentActivities.map((subagent) => subagent.id === activity.id ? activity : subagent)
    : [...state.subagentActivities, activity];
  return { ...state, subagentActivities };
}

function withActivity(state: SessionState, activity: SessionState['activity']) {
  return state.activity === activity ? state : { ...state, activity, error: activity === 'error' ? state.error : undefined };
}

function activityAfterTurn(state: SessionState, status: SessionTurnStatus) {
  const active = activeSubagentActivity(state.subagentActivities);
  if (active) return withActivity(state, active);
  return withActivity(
    state,
    status === 'completed'
      ? 'success'
      : status === 'denied' || status === 'cancelled' ? 'idle' : 'error'
  );
}

function activityAfterSubagent(state: SessionState, turnId: string): SessionState['activity'] {
  const active = activeSubagentActivity(state.subagentActivities);
  if (active) return active;
  const turn = state.turns[turnId];
  if (!turn || turn.status === 'running' || turn.status === 'queued') return 'thinking';
  if (turn.status === 'waiting_permission') return 'approval';
  if (turn.status === 'waiting_input') return 'input';
  if (turn.status === 'completed') return 'success';
  if (turn.status === 'failed') return 'error';
  return 'idle';
}

function activeSubagentActivity(subagents: readonly SessionSubagentActivity[]) {
  if (subagents.some((subagent) => subagent.status === 'waiting_permission')) return 'approval' as const;
  return subagents.some((subagent) => isActiveSubagent(subagent.status)) ? 'subagent' as const : undefined;
}

function completeTurn(state: SessionState, turnId: string, status: SessionTurnStatus, seq: number) {
  const next = withTurn(state, turnId, status);
  const messageStatus: SessionMessage['status'] = status === 'completed' || status === 'denied'
    ? 'completed'
    : status === 'cancelled' ? 'cancelled' : 'failed';
  const toolStatus: SessionToolActivity['status'] = status === 'completed'
    ? 'completed'
    : status === 'cancelled' ? 'cancelled' : 'failed';
  return {
    ...next,
    messages: settleAssistantSegments(next.messages, turnId, messageStatus),
    toolActivities: next.toolActivities.map((tool) => tool.turnId === turnId && tool.status === 'running'
      ? { ...tool, status: toolStatus }
      : tool),
    planActivities: next.planActivities.map((plan) => settlePlanActivity(plan, turnId, status, seq))
  };
}

function sequence(params: Record<string, unknown>) {
  return Math.max(0, Number(params.seq ?? params.transcriptSeq) || 0);
}

function toolId(params: Record<string, unknown>) {
  return text(params.toolCallId) || text(params.itemId);
}

function toolTerminalStatus(value: unknown): SessionToolActivity['status'] {
  const status = text(value);
  if (status === 'failed' || status === 'cancelled') return status;
  // 权限门拒绝（工具没跑）：这里没有单独的样式，按失败显示，别落到「完成」。
  if (status === 'denied') return 'failed';
  return 'completed';
}

function turnStatus(value: unknown): SessionTurnStatus {
  const status = text(value);
  return status === 'completed'
    || status === 'cancelled'
    || status === 'denied'
    || status === 'waiting_permission'
    || status === 'waiting_input'
    ? status
    : 'failed';
}

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function text(value: unknown) {
  return String(value || '').trim();
}

function messageDelta(params: Record<string, unknown>) {
  const nested = object(params.delta);
  if (typeof params.delta === 'string') return params.delta;
  if (typeof nested.text === 'string') return nested.text;
  if (typeof nested.content === 'string') return nested.content;
  if (typeof nested.delta === 'string') return nested.delta;
  if (typeof params.text === 'string') return params.text;
  if (typeof params.content === 'string') return params.content;
  return stringValue(params.delta);
}

function stringValue(value: unknown) {
  return value === undefined || value === null ? '' : String(value);
}
