import { html, nothing } from 'lit';

import type { SessionGoalActivity } from '../../session/GoalActivityModel.js';

export function goalProgressRow(goal: SessionGoalActivity) {
  const percent = goal.totalSteps > 0
    ? Math.min(100, Math.round((goal.completedSteps / goal.totalSteps) * 100))
    : 0;
  return html`
    <article
      class="goal-progress-row"
      data-testid="goal-progress-row"
      data-status=${goal.status}
      data-revision=${goal.revision}
    >
      <header>
        <strong>Goal · revision ${goal.revision}</strong>
        <span>${statusLabel(goal.status)}</span>
      </header>
      <p class="goal-progress-objective">${goal.summary}</p>
      ${goal.progressSummary
        ? html`<p class="goal-progress-copy">${goal.progressSummary}</p>`
        : nothing}
      ${goal.totalSteps > 0 ? html`
        <div
          class="goal-progress-meter"
          role="progressbar"
          aria-valuemin="0"
          aria-valuemax=${goal.totalSteps}
          aria-valuenow=${goal.completedSteps}
        ><span style=${`width:${percent}%`}></span></div>
        <small>${goal.completedSteps}/${goal.totalSteps} · continuation ${goal.continuationCount}/${goal.maxContinuations}</small>
      ` : html`
        <small>continuation ${goal.continuationCount}/${goal.maxContinuations}</small>
      `}
      ${goal.blockedReason
        ? html`<p class="goal-progress-blocked">${goal.blockedReason}</p>`
        : nothing}
    </article>
  `;
}

function statusLabel(status: SessionGoalActivity['status']) {
  if (status === 'active') return '推进中';
  if (status === 'paused') return '已暂停';
  if (status === 'blocked') return '需要处理';
  return '已完成';
}
