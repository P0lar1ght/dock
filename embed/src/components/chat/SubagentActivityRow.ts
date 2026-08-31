import { html, nothing } from 'lit';
import type { SessionSubagentActivity } from '../../session/SubagentActivityModel.js';

export function subagentActivityRow(subagent: SessionSubagentActivity) {
  const status = statusLabel(subagent.status);
  return html`
    <details
      class="subagent-activity"
      data-testid="subagent-activity"
      data-subagent-id=${subagent.id}
      data-turn-id=${subagent.turnId}
      data-status=${subagent.status}
    >
      <summary aria-label=${`${subagent.description}，${status}，点击展开详情`}>
        <span class="subagent-status-icon" aria-hidden="true">${statusIcon(subagent.status)}</span>
        <span class="subagent-row-copy">
          <span class="subagent-row-title">${subagent.description}</span>
          <span class="subagent-row-type">${subagent.subagentType}</span>
        </span>
        <span class="subagent-row-status">${status}</span>
        <span class="subagent-chevron" aria-hidden="true">›</span>
      </summary>
      <div class="subagent-details">
        <div class="subagent-detail-meta">
          <span>${status}</span>
          <span>${subagent.turns} 轮</span>
          <span>${subagent.toolCalls} 次工具</span>
          ${subagent.durationMs === undefined ? nothing : html`
            <span>${formatDuration(subagent.durationMs)}</span>
          `}
          ${subagent.resultTruncated ? html`<span>结果已截断</span>` : nothing}
        </div>
        ${subagent.resultSummary ? html`
          <section>
            <h4>结果摘要</h4>
            <p>${subagent.resultSummary}</p>
          </section>
        ` : nothing}
        ${subagent.errorSummary ? html`
          <section>
            <h4>失败摘要</h4>
            <p>${subagent.errorSummary}</p>
          </section>
        ` : nothing}
      </div>
    </details>
  `;
}

function statusLabel(status: SessionSubagentActivity['status']) {
  if (status === 'running') return '协作中';
  if (status === 'background') return '后台运行';
  if (status === 'waiting_permission') return '等待授权';
  if (status === 'failed') return '执行失败';
  if (status === 'cancelled') return '已停止';
  return '已完成';
}

function statusIcon(status: SessionSubagentActivity['status']) {
  if (status === 'running' || status === 'background') return '•';
  if (status === 'waiting_permission') return '!';
  if (status === 'failed') return '×';
  if (status === 'cancelled') return '−';
  return '✓';
}

function formatDuration(durationMs: number) {
  if (durationMs < 1000) return `${Math.round(durationMs)}ms`;
  return `${(durationMs / 1000).toFixed(durationMs < 10_000 ? 1 : 0)}s`;
}
