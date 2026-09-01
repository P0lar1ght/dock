import { html } from 'lit';
import type { SessionPlanStep } from '../session/PlanActivityModel.js';

export function todoList(steps: readonly SessionPlanStep[]) {
  return html`
    <ol class="todo-list" aria-label="计划步骤">
      ${steps.map((step) => html`
        <li
          class="todo-step"
          data-testid="plan-step"
          data-step-id=${step.id}
          data-status=${step.status}
        >
          <span class="todo-step-icon" aria-hidden="true">${stepIcon(step.status)}</span>
          <span class="todo-step-copy">
            <span class="todo-step-content">${step.content}</span>
            ${step.status === 'in_progress' && step.activeForm !== step.content
              ? html`<span class="todo-step-active">${step.activeForm}</span>`
              : ''}
          </span>
          <span class="todo-step-status">${stepStatus(step.status)}</span>
        </li>
      `)}
    </ol>
  `;
}

function stepIcon(status: SessionPlanStep['status']) {
  if (status === 'completed') return '✓';
  if (status === 'in_progress') return '•';
  return '○';
}

function stepStatus(status: SessionPlanStep['status']) {
  if (status === 'completed') return '已完成';
  if (status === 'in_progress') return '进行中';
  return '待处理';
}
