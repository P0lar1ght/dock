import { html, nothing } from 'lit';

export interface OperationIssueView {
  source: 'message' | 'thread';
  title: string;
  detail: string;
  retryable: boolean;
}

export function operationIssueNotice(
  model: OperationIssueView | undefined,
  retry: () => void,
  dismiss: () => void
) {
  if (!model) return nothing;
  return html`
    <div class="operation-issue" data-testid="operation-issue" data-source=${model.source} role="alert">
      <span class="runtime-issue-icon" aria-hidden="true">!</span>
      <span class="operation-issue-copy">
        <strong>${model.title}</strong>
        <span>${model.detail}</span>
      </span>
      <span class="operation-issue-actions">
        <button type="button" @click=${dismiss}>关闭</button>
        ${model.retryable ? html`<button type="button" @click=${retry}>重试</button>` : nothing}
      </span>
    </div>
  `;
}
