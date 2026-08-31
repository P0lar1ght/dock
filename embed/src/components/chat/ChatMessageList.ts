import { html, nothing } from 'lit';
import type { SessionMessage } from '../../session/MessageModel.js';
import type { SessionActivityState } from '../../session/SessionState.js';
import type { SessionToolActivity } from '../../session/ToolActivityModel.js';
import type { SessionPermissionRequest } from '../../session/PermissionModel.js';
import type { SessionPlanActivity } from '../../session/PlanActivityModel.js';
import { isActiveSubagent, type SessionSubagentActivity } from '../../session/SubagentActivityModel.js';
import type { PermissionInteractionState } from '../../controllers/PermissionController.js';
import type { PermissionDecision } from '../../protocol/permissions.js';
import { buildChatTimeline } from './ChatTimeline.js';
import { toolActivityRow } from './ToolActivityRow.js';
import { permissionPrompt } from '../PermissionPrompt.js';
import { assistantMessageParts } from '../../rendering/messageParts.js';
import { planCard } from '../PlanCard.js';
import { subagentActivityRow } from './SubagentActivityRow.js';
import type { SessionRuntimeIssue } from '../../session/RuntimeIssueModel.js';
import type { RuntimeIssueInteraction } from '../../controllers/RuntimeIssueController.js';
import { runtimeIssueRow, type RuntimeIssueActions } from './RuntimeIssueRow.js';
import type { SessionUserInputRequest } from '../../session/UserInputModel.js';
import type { UserInputInteractionState } from '../../controllers/UserInputController.js';
import { userInputPrompt, type UserInputPromptActions } from '../UserInputPrompt.js';
import type { SessionGoalActivity } from '../../session/GoalActivityModel.js';
import { goalProgressRow } from './GoalProgressRow.js';

export interface ChatMessageListModel {
  messages: readonly SessionMessage[];
  toolActivities: readonly SessionToolActivity[];
  permissionRequests: readonly SessionPermissionRequest[];
  userInputRequests: readonly SessionUserInputRequest[];
  planActivities: readonly SessionPlanActivity[];
  goalActivities: readonly SessionGoalActivity[];
  subagentActivities: readonly SessionSubagentActivity[];
  runtimeIssues: readonly SessionRuntimeIssue[];
  runtimeIssueInteractions: Readonly<Record<string, RuntimeIssueInteraction>>;
  permissionInteractions: Readonly<Record<string, PermissionInteractionState>>;
  userInputInteractions: Readonly<Record<string, UserInputInteractionState>>;
  activity: SessionActivityState;
  sessionReady: boolean;
}

export function chatMessageList(
  model: ChatMessageListModel,
  resolvePermission: (requestId: string, decision: PermissionDecision) => void,
  userInputActions: UserInputPromptActions,
  issueActions: RuntimeIssueActions
) {
  const timeline = buildChatTimeline(
    model.messages,
    model.toolActivities,
    model.permissionRequests,
    model.planActivities,
    model.subagentActivities,
    model.runtimeIssues,
    model.userInputRequests,
    model.goalActivities
  );
  const empty = timeline.length === 0;
  const hasRunningTool = model.toolActivities.some((tool) => tool.status === 'running');
  const hasActiveSubagent = model.subagentActivities.some((subagent) => isActiveSubagent(subagent.status));
  return html`
    <main
      class="message-list"
      data-testid="message-list"
      role="log"
      aria-live="polite"
      aria-label="Agent messages"
    >
      ${empty ? html`
        <div class="empty-chat" data-testid="empty-chat">
          <span class="empty-chat-mark" aria-hidden="true">⌁</span>
          <h3>和嘟嘟聊聊吧</h3>
          <p>${model.sessionReady ? '输入一条消息，本地 Agent 会在这里实时回复。' : '正在等待本地 Agent 连接。'}</p>
        </div>
      ` : timeline.map((item) => item.kind === 'message'
        ? messageBubble(item.message)
        : item.kind === 'tool'
          ? toolActivityRow(
            item.tool,
            model.runtimeIssues.find((issue) => issue.relatedToolId === item.tool.id),
            model.runtimeIssueInteractions,
            issueActions
          )
          : item.kind === 'plan'
            ? planCard(item.plan)
            : item.kind === 'goal'
              ? goalProgressRow(item.goal)
            : item.kind === 'user_input'
              ? userInputPrompt(
                item.request,
                model.userInputInteractions[item.request.id],
                userInputActions
              )
            : item.kind === 'subagent'
              ? subagentActivityRow(item.subagent)
              : item.kind === 'issue'
                ? runtimeIssueRow(item.issue, model.runtimeIssueInteractions[item.issue.id], issueActions)
                : permissionPrompt(
                  item.permission,
                  model.permissionInteractions[item.permission.id],
                  resolvePermission,
                  model.runtimeIssues.find((issue) => issue.relatedPermissionId === item.permission.id),
                  model.runtimeIssueInteractions,
                  issueActions
                ))}
      ${activityIndicator(model.activity, hasRunningTool, hasActiveSubagent)}
    </main>
  `;
}

function messageBubble(message: SessionMessage) {
  const assistant = message.role === 'assistant';
  const label = assistant ? 'Agent' : 'You';
  return html`
    <article
      class="message-row ${message.role}"
      data-testid="message-row"
      data-turn-id=${message.turnId}
      data-role=${message.role}
      data-status=${message.status}
    >
      <div class="message-meta">${label}</div>
      <div class="message-bubble">
        ${assistant
          ? assistantMessageParts(message.content || (message.status === 'streaming' ? '…' : ''))
          : html`<span class="message-copy">${message.content}</span>`}
        ${message.attachments?.length
          ? message.attachments.map((attachment, index) => imageAttachment(attachment, index))
          : nothing}
        ${message.status === 'streaming' ? html`<span class="stream-caret" aria-label="回复中"></span>` : nothing}
        ${message.status === 'failed' ? html`<span class="message-failed">回复中断</span>` : nothing}
        ${message.status === 'cancelled' ? html`<span class="message-cancelled">已停止</span>` : nothing}
      </div>
    </article>
  `;
}

function imageAttachment(
  attachment: NonNullable<SessionMessage['attachments']>[number],
  index: number
) {
  const label = attachment.type === 'screenshot'
    ? '当前界面截图'
    : attachment.name || `图片 ${index + 1}`;
  if (!attachment.previewUrl) {
    return html`<span class="message-attachment">▧ ${label}</span>`;
  }
  const sizeKb = Math.max(1, Math.ceil(attachment.byteLength / 1024));
  return html`
    <figure class="message-image-preview">
      <img
        src=${attachment.previewUrl}
        alt=${`本轮发送给模型的${label}`}
        loading="lazy"
      />
      <figcaption>
        ${label} · ${attachment.width}×${attachment.height} · ${sizeKb} KiB
      </figcaption>
    </figure>
  `;
}

function activityIndicator(
  activity: SessionActivityState,
  hasRunningTool: boolean,
  hasActiveSubagent: boolean
) {
  if (activity === 'working' && hasRunningTool) return nothing;
  if (activity === 'subagent' && hasActiveSubagent) return nothing;
  if (!['thinking', 'working', 'approval', 'input', 'subagent'].includes(activity)) return nothing;
  const labels: Partial<Record<SessionActivityState, string>> = {
    thinking: '嘟嘟正在思考',
    working: '嘟嘟正在使用本地能力',
    approval: '等待本地授权确认',
    input: '等待你选择规划方向',
    subagent: '协作 Agent 正在处理'
  };
  return html`
    <div class="agent-activity" data-testid="agent-activity" role="status">
      <span></span><span></span><span></span>
      <em>${labels[activity]}</em>
    </div>
  `;
}
