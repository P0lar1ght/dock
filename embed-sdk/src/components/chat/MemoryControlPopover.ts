import { html } from 'lit';
import { faBookOpen } from '@fortawesome/free-solid-svg-icons/faBookOpen';
import { faCheck } from '@fortawesome/free-solid-svg-icons/faCheck';
import { faPenToSquare } from '@fortawesome/free-solid-svg-icons/faPenToSquare';

import type { ThreadMemorySelection } from '../../controllers/ThreadExecutionControlController.js';
import type { SessionEnvironment } from '../../session/ContextUsageModel.js';
import { iconTemplate } from '../icons/Icon.js';

export function memoryControlPopover(
  environment: SessionEnvironment | undefined,
  activeTurn: boolean,
  changing: boolean,
  select: (selection: ThreadMemorySelection) => void
) {
  const memory = environment?.memory;
  const options = [
    {
      key: 'read',
      label: '读取记忆',
      detail: '在回复前加载当前应用和项目的已保存记忆',
      enabled: Boolean(memory?.read),
      available: Boolean(memory?.canRead),
      icon: faBookOpen
    },
    {
      key: 'write',
      label: '生成记忆',
      detail: '允许 Agent 通过 File/apply_patch 提议写入当前范围',
      enabled: Boolean(memory?.write),
      available: Boolean(memory?.canWrite),
      icon: faPenToSquare
    }
  ] as const;
  return html`
    <aside class="context-popover policy-popover" data-testid="memory-control-popover" aria-label="记忆">
      <div class="context-popover-heading">
        <strong>记忆</strong>
        <span>${activeTurn ? '下个回复生效' : changing ? '正在同步' : '当前 Thread'}</span>
      </div>
      <div class="model-options" role="group" aria-label="Thread 记忆权限">
        ${options.map((option) => html`
          <button
            class="model-option approval-option"
            data-testid=${`memory-${option.key}`}
            type="button"
            role="checkbox"
            aria-checked=${String(option.enabled)}
            ?disabled=${changing || !option.available}
            @click=${() => select({ [option.key]: !option.enabled })}
          >
            ${iconTemplate(option.icon)}
            <span><strong>${option.label}</strong><small>${option.detail}</small></span>
            ${option.enabled ? iconTemplate(faCheck) : ''}
          </button>
        `)}
      </div>
      <p class="context-empty">生成仍受当前审批模式约束；不同应用和项目之间不会共享记忆。</p>
    </aside>
  `;
}
