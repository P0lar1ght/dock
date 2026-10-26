import { html, nothing } from 'lit';
import type { ElicitationInteractionState } from '../controllers/ElicitationController.js';
import type { ElicitAction, SessionElicitation } from '../session/ElicitationModel.js';
import { TOOL_DIAMOND } from './chat/toolCard.js';

/** MCP 服务要用户确认的一件事：说明文字 + 接受 / 拒绝 / 取消（填不了表单，网关没给 schema）。 */
export function elicitationCard(
  request: SessionElicitation,
  interaction: ElicitationInteractionState | undefined,
  resolve: (elicitId: string, action: ElicitAction) => void
) {
  const pending = request.status === 'pending';
  const resolving = Boolean(interaction?.resolving);
  const status = pending ? '等你确认' : request.status === 'resolved' ? '已回应' : '没回应就结束了';
  return html`
    <details
      class="tool-card elicitation-card"
      data-testid="elicitation"
      data-elicit-id=${request.id}
      data-status=${request.status}
      ?open=${pending}
    >
      <summary aria-label=${`${request.server || 'MCP'} 请求确认，${status}`}>
        <span class="tool-card-diamond" aria-hidden="true">${TOOL_DIAMOND}</span>
        <span class="tool-card-name">${request.server || 'MCP'}</span>
        <span class="tool-card-summary">${status}</span>
      </summary>
      <div class="tool-card-body permission-details">
        ${request.heading ? html`<strong class="elicitation-heading">${request.heading}</strong>` : nothing}
        ${request.message ? html`<p class="permission-reason elicitation-message">${request.message}</p>` : nothing}
        ${interaction?.error ? html`<p class="permission-error" role="alert">${interaction.error}</p>` : nothing}
        ${pending ? html`
          <div class="permission-actions" aria-label="回应 MCP 请求">
            <button
              class="permission-deny"
              type="button"
              data-testid="elicit-cancel"
              ?disabled=${resolving}
              @click=${() => resolve(request.id, 'cancel')}
            >取消</button>
            <button
              class="permission-deny"
              type="button"
              data-testid="elicit-decline"
              ?disabled=${resolving}
              @click=${() => resolve(request.id, 'decline')}
            >拒绝</button>
            <button
              class="permission-approve"
              type="button"
              data-testid="elicit-accept"
              ?disabled=${resolving}
              @click=${() => resolve(request.id, 'accept')}
            >${resolving ? '等待 Gateway…' : '接受'}</button>
          </div>
        ` : nothing}
      </div>
    </details>
  `;
}
