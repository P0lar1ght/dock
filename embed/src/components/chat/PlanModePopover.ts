import { html } from 'lit';
import { faCheck } from '@fortawesome/free-solid-svg-icons/faCheck';
import { faListCheck } from '@fortawesome/free-solid-svg-icons/faListCheck';
import { faPenToSquare } from '@fortawesome/free-solid-svg-icons/faPenToSquare';

import type { SessionEnvironment } from '../../session/ContextUsageModel.js';
import { iconTemplate } from '../icons/Icon.js';

const OPTIONS = [
  {
    enabled: true,
    label: '计划模式',
    detail: '只分析、读取和规划；禁止修改工作区',
    icon: faListCheck
  },
  {
    enabled: false,
    label: '执行模式',
    detail: '按当前审批模式使用写入和执行能力',
    icon: faPenToSquare
  }
] as const;

export function planModePopover(
  environment: SessionEnvironment | undefined,
  activeTurn: boolean,
  changing: boolean,
  turnIntent: 'default' | 'plan',
  select: (enabled: boolean) => void
) {
  const plan = environment?.plan;
  return html`
    <aside class="context-popover policy-popover" data-testid="plan-mode-popover" aria-label="计划模式">
      <div class="context-popover-heading">
        <strong>计划模式</strong>
        <span>${activeTurn ? '进行中不可切换' : changing ? '正在同步' : '仅下一次发送'}</span>
      </div>
      <div class="model-options" role="radiogroup" aria-label="Thread 工作模式">
        ${OPTIONS.map((option) => html`
          <button
            class="model-option approval-option"
            data-testid=${`plan-mode-${option.enabled ? 'on' : 'off'}`}
            type="button"
            role="radio"
            aria-checked=${String(option.enabled === (turnIntent === 'plan'))}
            ?disabled=${activeTurn || changing || !plan?.canChange}
            @click=${() => select(option.enabled)}
          >
            ${iconTemplate(option.icon)}
            <span><strong>${option.label}</strong><small>${option.detail}</small></span>
            ${option.enabled === (turnIntent === 'plan') ? iconTemplate(faCheck) : ''}
          </button>
        `)}
      </div>
    </aside>
  `;
}
