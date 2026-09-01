import { html, nothing } from 'lit';
import type { PermissionDecision } from '../protocol/permissions.js';
import type { SessionPermissionRequest } from '../session/PermissionModel.js';
import type { PermissionInteractionState } from '../controllers/PermissionController.js';
import type { SessionRuntimeIssue } from '../session/RuntimeIssueModel.js';
import type { RuntimeIssueInteraction } from '../controllers/RuntimeIssueController.js';
import { runtimeIssueDetails, type RuntimeIssueActions } from './chat/RuntimeIssueRow.js';
import { TOOL_DIAMOND, prettyArgs } from './chat/toolCard.js';

export function permissionPrompt(
  permission: SessionPermissionRequest,
  interaction: PermissionInteractionState | undefined,
  resolve: (requestId: string, decision: PermissionDecision) => void,
  issue?: SessionRuntimeIssue,
  issueInteractions: Readonly<Record<string, RuntimeIssueInteraction>> = {},
  issueActions?: RuntimeIssueActions
) {
  const pending = permission.status === 'pending';
  const resolving = Boolean(interaction?.resolving);
  const args = prettyArgs(permission.argumentsSummary);
  return html`
    <details
      class="tool-card permission-prompt"
      data-testid="permission-prompt"
      data-permission-id=${permission.id}
      data-turn-id=${permission.turnId}
      data-status=${permission.status}
      data-risk=${permission.risk}
      data-resolving=${String(resolving)}
      ?open=${pending}
    >
      <summary aria-label=${`${permission.title}，${permissionStatus(permission)}`}>
        <span class="tool-card-diamond" aria-hidden="true">${TOOL_DIAMOND}</span>
        <span class="tool-card-name">${permission.title}</span>
        <span class="tool-card-summary">${permissionStatus(permission)}${permission.risk === 'high' ? ' · 高风险' : ''}</span>
      </summary>
      <div class="tool-card-body permission-details">
        ${permission.reason ? html`<p class="permission-reason">${permission.reason}</p>` : nothing}
        ${permission.scope ? html`<div class="tool-card-k">${permission.scope}</div>` : nothing}
        ${args ? html`
          <div class="tool-card-k">输入</div>
          <pre class="tool-card-pre">${args}</pre>
        ` : nothing}
        ${interaction?.error ? html`<p class="permission-error" role="alert">${interaction.error}</p>` : nothing}
        ${issue && issueActions ? runtimeIssueDetails(issue, issueInteractions[issue.id], issueActions) : nothing}
        ${pending ? html`
          <div class="permission-actions" aria-label="审批操作">
            <button
              class="permission-deny"
              type="button"
              ?disabled=${resolving}
              @click=${() => resolve(permission.id, 'deny')}
            >拒绝</button>
            <button
              class="permission-approve"
              type="button"
              ?disabled=${resolving}
              @click=${() => resolve(permission.id, 'approve')}
            >${resolving ? '等待 Gateway…' : '允许'}</button>
          </div>
        ` : nothing}
      </div>
    </details>
  `;
}

function permissionStatus(permission: SessionPermissionRequest) {
  if (permission.status === 'approved' && permission.automatic) {
    if (permission.decisionSource === 'deterministic') return '自动允许（确定性策略）';
    if (permission.decisionSource === 'assistant') return '自动允许（辅助分类器）';
    if (permission.decisionSource === 'policy') return '自动允许（安全策略）';
    return '自动允许（来源未标注）';
  }
  if (permission.status === 'approved') return '已允许';
  if (permission.status === 'denied') return '已拒绝';
  if (permission.status === 'failed') return '处理失败';
  return '需要确认';
}

