import { html, nothing } from 'lit';
import type { SessionToolActivity } from '../../session/ToolActivityModel.js';
import type { SessionRuntimeIssue } from '../../session/RuntimeIssueModel.js';
import type { RuntimeIssueInteraction } from '../../controllers/RuntimeIssueController.js';
import { runtimeIssueDetails, type RuntimeIssueActions } from './RuntimeIssueRow.js';
import {
  TOOL_DIAMOND,
  argumentSummary,
  isShellTool,
  prettyArgs,
  truncatedOutput
} from './toolCard.js';

export function toolActivityRow(
  tool: SessionToolActivity,
  issue?: SessionRuntimeIssue,
  interactions: Readonly<Record<string, RuntimeIssueInteraction>> = {},
  issueActions?: RuntimeIssueActions
) {
  const name = tool.toolName || tool.title || 'tool';
  const summary = argumentSummary(name, tool.inputSummary);
  const shell = isShellTool(name);
  const input = prettyArgs(tool.inputSummary);
  const outputSource = tool.outputDetails || tool.outputPreview || '';
  const output = outputSource ? truncatedOutput(outputSource) : undefined;
  const label = shell ? `$ ${summary || '\u{2026}'}` : `${name}${summary ? `  ${summary}` : ''}`;
  return html`
    <details
      class="tool-card tool-activity"
      data-testid="tool-activity"
      data-tool-id=${tool.id}
      data-turn-id=${tool.turnId}
      data-status=${tool.status}
    >
      <summary aria-label=${`${label}，${statusLabel(tool.status)}`}>
        ${shell
          ? html`<span class="tool-card-prompt" aria-hidden="true">$</span>`
          : html`<span class="tool-card-diamond" aria-hidden="true">${TOOL_DIAMOND}</span>`}
        ${shell
          ? html`<span class="tool-card-summary">${summary || '\u{2026}'}</span>`
          : html`
              <span class="tool-card-name">${name}</span>
              ${summary ? html`<span class="tool-card-summary">${summary}</span>` : nothing}
            `}
        ${tool.status === 'running' ? html`<span class="tool-card-live">运行中</span>` : nothing}
      </summary>
      <div class="tool-card-body">
        ${!shell && input ? html`
          <div class="tool-card-k">输入</div>
          <pre class="tool-card-pre">${input}</pre>
        ` : nothing}
        ${tool.status === 'running' && !outputSource ? html`
          <div class="tool-card-live-body">运行中</div>
        ` : nothing}
        ${outputSource ? html`
          <div class="tool-card-k">输出</div>
          <pre class="tool-card-pre" data-testid="tool-output-preview">${output?.text}</pre>
          ${tool.outputDetails && output?.hidden ? html`
            <details class="tool-output-more">
              <summary>查看完整输出</summary>
              <pre data-testid="tool-output-details">${tool.outputDetails}</pre>
            </details>
          ` : nothing}
        ` : nothing}
        ${issue && issueActions ? runtimeIssueDetails(issue, interactions[issue.id], issueActions) : nothing}
      </div>
    </details>
  `;
}

function statusLabel(status: SessionToolActivity['status']) {
  if (status === 'running') return '执行中';
  if (status === 'failed') return '执行失败';
  if (status === 'cancelled') return '已停止';
  return '已完成';
}
