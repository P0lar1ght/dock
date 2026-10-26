import type { ThreadSummary } from '../protocol/responses.js';
import type { SessionMessage } from './MessageModel.js';
import type { SessionPermissionRequest } from './PermissionModel.js';
import type { SessionPlanActivity } from './PlanActivityModel.js';
import type { SessionGoalActivity } from './GoalActivityModel.js';
import type { SessionToolActivity } from './ToolActivityModel.js';
import type { SessionSubagentActivity } from './SubagentActivityModel.js';
import type { SessionTurnQueueItem } from './TurnQueueModel.js';
import type { SessionRuntimeIssue } from './RuntimeIssueModel.js';
import type { SessionEnvironment } from './ContextUsageModel.js';
import type { SessionUserInputRequest } from './UserInputModel.js';
import type { SessionPlanApproval } from './PlanApprovalModel.js';
import type { SessionElicitation } from './ElicitationModel.js';

export type SessionConnectionState = 'recovering' | 'live' | 'disconnected';
export type SessionTurnStatus =
  | 'running'
  | 'queued'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'denied'
  | 'waiting_permission'
  | 'waiting_input';
export type SessionActivityState =
  | 'idle'
  | 'thinking'
  | 'streaming'
  | 'working'
  | 'approval'
  | 'input'
  | 'subagent'
  | 'success'
  | 'error';

export interface SessionTurn {
  id: string;
  status: SessionTurnStatus;
}

export interface SessionState {
  thread: ThreadSummary;
  messages: SessionMessage[];
  toolActivities: SessionToolActivity[];
  planActivities: SessionPlanActivity[];
  goalActivities: SessionGoalActivity[];
  subagentActivities: SessionSubagentActivity[];
  permissionRequests: SessionPermissionRequest[];
  userInputRequests: SessionUserInputRequest[];
  /** 计划模式里等批准的计划（`plan/requested`）。 */
  planApprovals: SessionPlanApproval[];
  /** MCP 服务要用户确认的事（`elicit/requested`）。 */
  elicitations: SessionElicitation[];
  turnQueue: SessionTurnQueueItem[];
  runtimeIssues: SessionRuntimeIssue[];
  turns: Record<string, SessionTurn>;
  lastSeq: number;
  connection: SessionConnectionState;
  activity: SessionActivityState;
  environment?: SessionEnvironment;
  error?: string;
}

export function initialSessionState(thread: ThreadSummary): SessionState {
  return {
    thread,
    messages: [],
    toolActivities: [],
    planActivities: [],
    goalActivities: [],
    subagentActivities: [],
    permissionRequests: [],
    userInputRequests: [],
    planApprovals: [],
    elicitations: [],
    turnQueue: [],
    runtimeIssues: [],
    turns: {},
    lastSeq: 0,
    connection: 'recovering',
    activity: 'idle'
  };
}
