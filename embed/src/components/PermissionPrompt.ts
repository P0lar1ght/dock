import { html, nothing } from 'lit';
import type { PermissionDecision } from '../protocol/permissions.js';
import type { SessionPermissionRequest } from '../session/PermissionModel.js';
import type { PermissionInteractionState } from '../controllers/PermissionController.js';
import type { SessionRuntimeIssue } from '../session/RuntimeIssueModel.js';
import type { RuntimeIssueInteraction } from '../controllers/RuntimeIssueController.js';
import { runtimeIssueDetails, type RuntimeIssueActions } from './chat/RuntimeIssueRow.js';

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
  return html`
    <details
      class="permission-prompt"
      data-testid="permission-prompt"
      data-permission-id=${permission.id}
      data-turn-id=${permission.turnId}
      data-status=${permission.status}
      data-risk=${permission.risk}
      data-resolving=${String(resolving)}
      ?open=${pending}
    >
      <summary aria-label=${`${permission.title}，${permissionStatus(permission)}`}>
        <span class="permission-icon" aria-hidden="true">${permissionIcon(permission.status)}</span>
        <span class="permission-title">${permission.title}</span>
        <span class="permission-risk">${riskLabel(permission.risk)}</span>
        <span class="permission-state">${permissionStatus(permission)}</span>
        <span class="permission-chevron" aria-hidden="true">›</span>
      </summary>
      <div class="permission-details">
        ${permission.scope ? html`<p class="permission-scope">作用域 · ${permission.scope}</p>` : nothing}
        ${permission.reason ? html`<p class="permission-reason">说明 · ${permission.reason}</p>` : nothing}
        ${permission.bindingSummary ? html`
          <p class="permission-scope">授权绑定 · ${permission.bindingSummary}</p>
        ` : nothing}
        ${permission.expiresAt ? html`
          <p class="permission-scope">授权到期 · ${new Date(permission.expiresAt).toISOString()}</p>
        ` : nothing}
        ${permission.argumentsSummary ? html`
          <section>
            <h4>参数摘要</h4>
            <pre>${permission.argumentsSummary}</pre>
          </section>
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

function permissionIcon(status: SessionPermissionRequest['status']) {
  if (status === 'approved') return '✓';
  if (status === 'denied') return '−';
  if (status === 'failed') return '×';
  return '!';
}

function riskLabel(risk: SessionPermissionRequest['risk']) {
  if (risk === 'high') return '高风险';
  if (risk === 'medium') return '中风险';
  if (risk === 'low') return '低风险';
  return '风险未知';
}
