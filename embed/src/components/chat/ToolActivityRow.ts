import { html, nothing } from 'lit';
import type { SessionToolActivity } from '../../session/ToolActivityModel.js';
import type { SessionRuntimeIssue } from '../../session/RuntimeIssueModel.js';
import type { RuntimeIssueInteraction } from '../../controllers/RuntimeIssueController.js';
import { runtimeIssueDetails, type RuntimeIssueActions } from './RuntimeIssueRow.js';

export function toolActivityRow(
  tool: SessionToolActivity,
  issue?: SessionRuntimeIssue,
  interactions: Readonly<Record<string, RuntimeIssueInteraction>> = {},
  issueActions?: RuntimeIssueActions
) {
  const label = toolLabel(tool);
  return html`
    <details
      class="tool-activity"
      data-testid="tool-activity"
      data-tool-id=${tool.id}
      data-turn-id=${tool.turnId}
      data-status=${tool.status}
    >
      <summary aria-label=${`${label}，${statusLabel(tool.status)}，点击展开详情`}>
        <span class="tool-status-icon" aria-hidden="true">${statusIcon(tool.status)}</span>
        <span class="tool-row-title">${label}</span>
        ${tool.durationMs === undefined ? nothing : html`
          <span class="tool-duration">${formatDuration(tool.durationMs)}</span>
        `}
        <span class="tool-chevron" aria-hidden="true">›</span>
      </summary>
      <div class="tool-details">
        <div class="tool-detail-meta">
          <span>${statusLabel(tool.status)}</span>
          ${tool.resultType ? html`<span>${tool.resultType}</span>` : nothing}
          ${tool.truncated ? html`<span>服务端内容已截断</span>` : nothing}
          ${tool.outputPreviewTruncated ? html`<span>响应预览已限长</span>` : nothing}
        </div>
        ${tool.inputSummary ? html`
          <section>
            <h4>输入摘要</h4>
            <pre>${tool.inputSummary}</pre>
          </section>
        ` : nothing}
        ${tool.outputSummary ? html`
          <section>
            <h4>结果摘要</h4>
            <p>${tool.outputSummary}</p>
          </section>
        ` : nothing}
        ${tool.outputPreview ? html`
          <section class="tool-output-section">
            <h4>响应预览</h4>
            <pre data-testid="tool-output-preview">${tool.outputPreview}</pre>
            ${tool.outputDetails ? html`
              <details class="tool-output-more">
                <summary aria-label="查看更多工具响应">
                  <span>查看更多响应</span>
                  <span aria-hidden="true">›</span>
                </summary>
                <pre data-testid="tool-output-details">${tool.outputDetails}</pre>
              </details>
            ` : nothing}
          </section>
        ` : nothing}
        ${issue && issueActions ? runtimeIssueDetails(issue, interactions[issue.id], issueActions) : nothing}
      </div>
    </details>
  `;
}

function toolLabel(tool: SessionToolActivity) {
  const name = tool.title || tool.toolName || 'tool';
  if (/shell|exec|command/i.test(tool.toolName)) return `运行 ${name}`;
  if (/read|file|context/i.test(tool.toolName)) return `读取 ${name}`;
  if (/search|find/i.test(tool.toolName)) return `搜索 ${name}`;
  return `调用 ${name}`;
}

function statusLabel(status: SessionToolActivity['status']) {
  if (status === 'running') return '执行中';
  if (status === 'failed') return '执行失败';
  if (status === 'cancelled') return '已停止';
  return '已完成';
}

function statusIcon(status: SessionToolActivity['status']) {
  if (status === 'running') return '•';
  if (status === 'failed') return '×';
  if (status === 'cancelled') return '−';
  return '✓';
}

function formatDuration(durationMs: number) {
  if (durationMs < 1000) return `${Math.round(durationMs)}ms`;
  return `${(durationMs / 1000).toFixed(durationMs < 10_000 ? 1 : 0)}s`;
}
