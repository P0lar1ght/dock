import { html, nothing } from 'lit';
import type { PlanApprovalInteractionState } from '../controllers/PlanApprovalController.js';
import type { PlanDecision, SessionPlanApproval } from '../session/PlanApprovalModel.js';
import { assistantMessageParts } from '../rendering/messageParts.js';
import { TOOL_DIAMOND } from './chat/toolCard.js';

/**
 * 计划模式下模型提交的计划：正文按 markdown 画，等用户「批准并执行 / 要求修改 / 退出计划」。
 * 按钮文案同桌面端的计划卡。批过的收起成一行。
 */
export function planApprovalCard(
  plan: SessionPlanApproval,
  interaction: PlanApprovalInteractionState | undefined,
  resolve: (planId: string, decision: PlanDecision) => void
) {
  const pending = plan.status === 'pending';
  const resolving = Boolean(interaction?.resolving);
  const file = plan.path.split('/').pop() || '';
  return html`
    <details
      class="tool-card plan-approval"
      data-testid="plan-approval"
      data-plan-id=${plan.id}
      data-turn-id=${plan.turnId}
      data-status=${plan.status}
      ?open=${pending}
    >
      <summary aria-label=${`计划，${statusLabel(plan)}`}>
        <span class="tool-card-diamond" aria-hidden="true">${TOOL_DIAMOND}</span>
        <span class="tool-card-name">计划</span>
        <span class="tool-card-summary">${statusLabel(plan)}${file ? ` · ${file}` : ''}</span>
      </summary>
      <div class="tool-card-body plan-approval-body">
        ${plan.empty || !plan.body.trim()
          ? html`<p class="plan-approval-empty">计划文件是空的。</p>`
          : html`<div class="plan-approval-markdown">${assistantMessageParts(plan.body)}</div>`}
        ${interaction?.error ? html`<p class="permission-error" role="alert">${interaction.error}</p>` : nothing}
        ${pending ? html`
          <div class="permission-actions plan-approval-actions" aria-label="计划审批">
            <button
              class="permission-deny"
              type="button"
              data-testid="plan-quit"
              ?disabled=${resolving}
              @click=${() => resolve(plan.id, 'quit')}
            >退出计划</button>
            <button
              class="permission-deny"
              type="button"
              data-testid="plan-revise"
              ?disabled=${resolving}
              @click=${() => resolve(plan.id, 'revise')}
            >要求修改</button>
            <button
              class="permission-approve"
              type="button"
              data-testid="plan-approve"
              ?disabled=${resolving}
              @click=${() => resolve(plan.id, 'approve')}
            >${resolving ? '等待 Gateway…' : '批准并执行'}</button>
          </div>
        ` : nothing}
      </div>
    </details>
  `;
}

function statusLabel(plan: SessionPlanApproval) {
  switch (plan.status) {
    case 'pending':
      return '等你批准';
    case 'approved':
      return '已批准';
    case 'revised':
      return '已要求修改';
    case 'quit':
      return '已退出计划';
    case 'cancelled':
      return '未批准就结束了';
  }
}
