import { html } from 'lit';
import type { SessionPlanActivity } from '../session/PlanActivityModel.js';
import { todoList } from './TodoList.js';

export function planCard(plan: SessionPlanActivity) {
  const completed = plan.steps.filter((step) => step.status === 'completed').length;
  const summary = plan.steps.length ? `${completed}/${plan.steps.length}` : '无步骤';
  return html`
    <details
      class="plan-activity"
      data-testid="plan-activity"
      data-plan-id=${plan.id}
      data-turn-id=${plan.turnId}
      data-status=${plan.status}
      data-degraded=${String(plan.degraded)}
      ?open=${plan.status === 'running'}
    >
      <summary aria-label=${`执行计划，${planStatus(plan.status)}，${summary}，点击展开详情`}>
        <span class="plan-status-icon" aria-hidden="true">${statusIcon(plan.status)}</span>
        <span class="plan-row-title">执行计划</span>
        <span class="plan-progress">${summary}</span>
        <span class="plan-chevron" aria-hidden="true">›</span>
      </summary>
      <div class="plan-details">
        ${plan.steps.length ? todoList(plan.steps) : html`
          <p class="plan-empty">${plan.degraded ? '计划状态暂不可用' : '计划已清空'}</p>
        `}
        ${plan.degraded && plan.steps.length ? html`
          <p class="plan-degraded">部分计划状态无法识别，已按待处理显示。</p>
        ` : ''}
      </div>
    </details>
  `;
}

function planStatus(status: SessionPlanActivity['status']) {
  if (status === 'running') return '执行中';
  if (status === 'failed') return '执行失败';
  if (status === 'cancelled') return '已停止';
  return '已完成';
}

function statusIcon(status: SessionPlanActivity['status']) {
  if (status === 'running') return '•';
  if (status === 'failed') return '×';
  if (status === 'cancelled') return '−';
  return '✓';
}
