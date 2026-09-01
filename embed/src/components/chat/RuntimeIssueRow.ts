import { html, nothing } from 'lit';
import type { RuntimeIssueInteraction } from '../../controllers/RuntimeIssueController.js';
import type { SessionRuntimeIssue } from '../../session/RuntimeIssueModel.js';
import { TOOL_DIAMOND } from './toolCard.js';

export interface RuntimeIssueActions {
  retryIssue: (issue: SessionRuntimeIssue) => void;
  editIssue: (issue: SessionRuntimeIssue) => void;
  dismissIssue: (issueId: string) => void;
}

export function runtimeIssueRow(
  issue: SessionRuntimeIssue,
  interaction: RuntimeIssueInteraction | undefined,
  actions: RuntimeIssueActions
) {
  return html`
    <details
      class="tool-card runtime-issue"
      data-testid="runtime-issue"
      data-issue-id=${issue.id}
      data-turn-id=${issue.turnId}
      data-kind=${issue.kind}
    >
      <summary aria-label=${`${issue.title}，点击展开恢复操作`}>
        <span class="tool-card-diamond" aria-hidden="true">${TOOL_DIAMOND}</span>
        <span class="tool-card-name">${issue.title}</span>
        <span class="tool-card-summary">${actionLabel(issue)}</span>
      </summary>
      ${runtimeIssueDetails(issue, interaction, actions)}
    </details>
  `;
}

export function runtimeIssueDetails(
  issue: SessionRuntimeIssue,
  interaction: RuntimeIssueInteraction | undefined,
  actions: RuntimeIssueActions
) {
  return html`
    <div class="runtime-issue-details">
      <p>${issue.detail}</p>
      ${interaction?.error ? html`<p class="runtime-issue-retry-error" role="alert">${interaction.error}</p>` : nothing}
      <div class="runtime-issue-actions">
        <button type="button" @click=${() => actions.dismissIssue(issue.id)}>关闭</button>
        ${issue.action === 'dismiss' ? nothing : html`
          <button
            class="runtime-issue-primary"
            type="button"
            ?disabled=${Boolean(interaction?.retrying)}
            @click=${() => issue.action === 'edit' ? actions.editIssue(issue) : actions.retryIssue(issue)}
          >${interaction?.retrying ? '正在重试…' : actionLabel(issue)}</button>
        `}
      </div>
    </div>
  `;
}

function actionLabel(issue: SessionRuntimeIssue) {
  if (issue.action === 'edit') return '修改消息';
  if (issue.action === 'retry') return '重试本轮';
  return '查看详情';
}
