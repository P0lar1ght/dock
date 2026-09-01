import { html, nothing } from 'lit';

import type { SessionGoalActivity } from '../../session/GoalActivityModel.js';
import { TOOL_DIAMOND } from './toolCard.js';

export function goalProgressRow(goal: SessionGoalActivity) {
  const steps = goal.totalSteps > 0 ? `${goal.completedSteps}/${goal.totalSteps}` : '';
  return html`
    <details
      class="tool-card goal-progress-row"
      data-testid="goal-progress-row"
      data-status=${goal.status}
      data-revision=${goal.revision}
    >
      <summary aria-label=${`Goal，${statusLabel(goal.status)}，${goal.summary}`}>
        <span class="tool-card-diamond" aria-hidden="true">${TOOL_DIAMOND}</span>
        <span class="tool-card-name">Goal</span>
        <span class="tool-card-summary">${goal.summary}${steps ? `  ${steps}` : ''}</span>
      </summary>
      <div class="tool-card-body">
        ${goal.progressSummary
          ? html`<p>${goal.progressSummary}</p>`
          : nothing}
        <div class="tool-card-k">${statusLabel(goal.status)} · continuation ${goal.continuationCount}/${goal.maxContinuations}</div>
        ${goal.blockedReason
          ? html`<p class="goal-progress-blocked">${goal.blockedReason}</p>`
          : nothing}
      </div>
    </details>
  `;
}

function statusLabel(status: SessionGoalActivity['status']) {
  if (status === 'active') return '推进中';
  if (status === 'paused') return '已暂停';
  if (status === 'blocked') return '需要处理';
  return '已完成';
}
