import { html } from 'lit';
import type { SessionPlanActivity } from '../session/PlanActivityModel.js';
import { todoList } from './TodoList.js';
import { TOOL_DIAMOND } from './chat/toolCard.js';

export function planCard(plan: SessionPlanActivity) {
  const completed = plan.steps.filter((step) => step.status === 'completed').length;
  const summary = plan.steps.length ? `${completed}/${plan.steps.length}` : '无步骤';
  return html`
    <details
      class="tool-card plan-activity"
      data-testid="plan-activity"
      data-plan-id=${plan.id}
      data-turn-id=${plan.turnId}
      data-status=${plan.status}
      data-degraded=${String(plan.degraded)}
    >
      <summary aria-label=${`执行计划，${planStatus(plan.status)}，${summary}`}>
        <span class="tool-card-diamond" aria-hidden="true">${TOOL_DIAMOND}</span>
        <span class="tool-card-name">执行计划</span>
        <span class="tool-card-summary">${summary}</span>
      </summary>
      <div class="tool-card-body plan-details">
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
