import { html, nothing } from 'lit';
import type { SessionSubagentActivity } from '../../session/SubagentActivityModel.js';
import { TOOL_DIAMOND } from './toolCard.js';

export function subagentActivityRow(subagent: SessionSubagentActivity) {
  const status = statusLabel(subagent.status);
  const preview = subagent.resultSummary || subagent.errorSummary || '';
  return html`
    <details
      class="tool-card subagent-activity"
      data-testid="subagent-activity"
      data-subagent-id=${subagent.id}
      data-turn-id=${subagent.turnId}
      data-status=${subagent.status}
    >
      <summary aria-label=${`${status} ${subagent.subagentType} ${subagent.description}`}>
        <span class="tool-card-diamond" aria-hidden="true">${TOOL_DIAMOND}</span>
        <span class="tool-card-name">${status}</span>
        ${subagent.subagentType
          ? html`<span class="tool-card-summary">${subagent.subagentType}</span>`
          : nothing}
        <span class="tool-card-summary">${`\u201C${subagent.description}\u201D`}</span>
      </summary>
      <div class="tool-card-body">
        ${preview ? html`<pre class="tool-card-pre">${preview}</pre>` : nothing}
        <div class="tool-card-k">${subagent.turns} 轮 · ${subagent.toolCalls} 次工具${
          subagent.durationMs === undefined ? '' : ` · ${formatDuration(subagent.durationMs)}`
        }</div>
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

function formatDuration(durationMs: number) {
  if (durationMs < 1000) return `${Math.round(durationMs)}ms`;
  return `${(durationMs / 1000).toFixed(durationMs < 10_000 ? 1 : 0)}s`;
}
