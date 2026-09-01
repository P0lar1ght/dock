import { html } from 'lit';
import { faCheck } from '@fortawesome/free-solid-svg-icons/faCheck';
import { faHand } from '@fortawesome/free-solid-svg-icons/faHand';
import { faTerminal } from '@fortawesome/free-solid-svg-icons/faTerminal';
import type { ApprovalMode } from '../../protocol/responses.js';
import type { SessionEnvironment } from '../../session/ContextUsageModel.js';
import { iconTemplate } from '../icons/Icon.js';

/** Mirrors Dock `PermissionMode::{Ask, Allow}` and TUI chrome (`询问` / `始终允许`). */
const MODES: Array<{
  id: Exclude<ApprovalMode, 'full_access'>;
  label: string;
  detail: string;
  icon: typeof faHand;
}> = [
  { id: 'ask', label: '询问', detail: '每次工具调用都请求确认', icon: faHand },
  { id: 'auto', label: '始终允许', detail: '跳过工具审批，不再询问', icon: faTerminal }
];

export function approvalModePopover(
  environment: SessionEnvironment | undefined,
  activeTurn: boolean,
  changing: boolean,
  confirmationPending: boolean,
  select: (mode: ApprovalMode) => void
) {
  const approval = environment?.approval;
  const offered = new Set(approval?.options.length ? approval.options : ['ask', 'auto']);
  const modes = MODES.filter((mode) => offered.has(mode.id));
  return html`
    <aside class="context-popover policy-popover" data-testid="approval-popover" aria-label="审批模式">
      <div class="context-popover-heading">
        <strong>审批模式</strong>
        <span>${confirmationPending ? '等待本机确认' : activeTurn ? '下个回复生效' : '当前 Thread'}</span>
      </div>
      <div class="model-options" role="radiogroup" aria-label="审批模式">
        ${modes.map((mode) => html`
          <button
            class="model-option approval-option"
            data-mode=${mode.id}
            data-testid=${`approval-option-${mode.id}`}
            type="button"
            role="radio"
            aria-checked=${String(mode.id === approval?.mode)}
            ?disabled=${changing}
            @click=${() => select(mode.id)}
          >
            ${iconTemplate(mode.icon)}
            <span>
              <strong>${mode.label}</strong>
              <small>${mode.detail}</small>
            </span>
            ${mode.id === approval?.mode ? iconTemplate(faCheck) : ''}
          </button>
        `)}
      </div>
    </aside>
  `;
}
