import { html } from 'lit';
import { faCheck } from '@fortawesome/free-solid-svg-icons/faCheck';
import { faHand } from '@fortawesome/free-solid-svg-icons/faHand';
import { faShieldHalved } from '@fortawesome/free-solid-svg-icons/faShieldHalved';
import { faTerminal } from '@fortawesome/free-solid-svg-icons/faTerminal';
import type { ApprovalMode } from '../../protocol/responses.js';
import type { SessionEnvironment } from '../../session/ContextUsageModel.js';
import { iconTemplate } from '../icons/Icon.js';

const MODES: Array<{
  id: ApprovalMode;
  label: string;
  detail: string;
  icon: typeof faHand;
}> = [
  { id: 'ask', label: '请求审批', detail: '编辑文件和使用互联网时始终询问', icon: faHand },
  { id: 'auto', label: '按风险审批', detail: '仅对检测到的风险操作请求审批', icon: faTerminal },
  { id: 'full_access', label: '完全访问', detail: '跳过工具审批；需要本机单次确认', icon: faShieldHalved }
];

export function approvalModePopover(
  environment: SessionEnvironment | undefined,
  activeTurn: boolean,
  changing: boolean,
  confirmationPending: boolean,
  select: (mode: ApprovalMode) => void
) {
  const approval = environment?.approval;
  return html`
    <aside class="context-popover policy-popover" data-testid="approval-popover" aria-label="审批模式">
      <div class="context-popover-heading">
        <strong>审批模式</strong>
        <span>${confirmationPending ? '等待本机确认' : activeTurn ? '下个回复生效' : '当前 Thread'}</span>
      </div>
      <div class="model-options" role="radiogroup" aria-label="审批模式">
        ${MODES.map((mode) => {
          const allowed = Boolean(approval?.options.includes(mode.id));
          return html`
            <button
              class="model-option approval-option"
              data-mode=${mode.id}
              data-testid=${`approval-option-${mode.id}`}
              type="button"
              role="radio"
              aria-checked=${String(mode.id === approval?.mode)}
              ?disabled=${changing || !allowed}
              @click=${() => select(mode.id)}
            >
              ${iconTemplate(mode.icon)}
              <span>
                <strong>${mode.label}</strong>
                <small>${allowed ? mode.detail : '当前工作区未授予本机完全访问资格'}</small>
              </span>
              ${mode.id === approval?.mode ? iconTemplate(faCheck) : ''}
            </button>
          `;
        })}
      </div>
    </aside>
  `;
}
