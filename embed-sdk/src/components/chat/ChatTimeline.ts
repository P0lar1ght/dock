import type { SessionMessage } from '../../session/MessageModel.js';
import type { SessionToolActivity } from '../../session/ToolActivityModel.js';
import type { SessionPermissionRequest } from '../../session/PermissionModel.js';
import type { SessionPlanActivity } from '../../session/PlanActivityModel.js';
import type { SessionSubagentActivity } from '../../session/SubagentActivityModel.js';
import type { SessionRuntimeIssue } from '../../session/RuntimeIssueModel.js';
import type { SessionUserInputRequest } from '../../session/UserInputModel.js';
import type { SessionGoalActivity } from '../../session/GoalActivityModel.js';

export type ChatTimelineItem =
  | { kind: 'message'; message: SessionMessage }
  | { kind: 'tool'; tool: SessionToolActivity }
  | { kind: 'permission'; permission: SessionPermissionRequest }
  | { kind: 'user_input'; request: SessionUserInputRequest }
  | { kind: 'plan'; plan: SessionPlanActivity }
  | { kind: 'goal'; goal: SessionGoalActivity }
  | { kind: 'subagent'; subagent: SessionSubagentActivity }
  | { kind: 'issue'; issue: SessionRuntimeIssue };

interface SequencedTimelineItem {
  seq: number;
  order: number;
  item: ChatTimelineItem;
}

export function buildChatTimeline(
  messages: readonly SessionMessage[],
  tools: readonly SessionToolActivity[],
  permissions: readonly SessionPermissionRequest[] = [],
  plans: readonly SessionPlanActivity[] = [],
  subagents: readonly SessionSubagentActivity[] = [],
  issues: readonly SessionRuntimeIssue[] = [],
  userInputs: readonly SessionUserInputRequest[] = [],
  goals: readonly SessionGoalActivity[] = []
): ChatTimelineItem[] {
  const subagentTurns = new Set(subagents.map((subagent) => subagent.turnId));
  let order = 0;
  const legacy: ChatTimelineItem[] = [];
  const sequenced: SequencedTimelineItem[] = [];
  const append = (seq: number | undefined, item: ChatTimelineItem) => {
    if (!seq || seq < 1) legacy.push(item);
    else sequenced.push({ seq, order: order++, item });
  };

  for (const message of messages) append(message.startedSeq, { kind: 'message', message });
  for (const tool of tools) {
    if (tool.toolName === 'spawn_subagent' && subagentTurns.has(tool.turnId)) continue;
    append(tool.startedSeq, { kind: 'tool', tool });
  }
  for (const permission of permissions) {
    append(permission.requestedSeq, { kind: 'permission', permission });
  }
  for (const request of userInputs) append(request.seq, { kind: 'user_input', request });
  for (const plan of plans) append(plan.startedSeq, { kind: 'plan', plan });
  for (const goal of goals) append(goal.startedSeq, { kind: 'goal', goal });
  for (const subagent of subagents) append(subagent.startedSeq, { kind: 'subagent', subagent });
  for (const issue of issues) {
    if (issue.relatedToolId || issue.relatedPermissionId) continue;
    append(issue.seq, { kind: 'issue', issue });
  }

  sequenced.sort((left, right) => left.seq - right.seq || left.order - right.order);
  return [...legacy, ...sequenced.map((entry) => entry.item)];
}
