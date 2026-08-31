import { html, nothing } from 'lit';
import { faBullseye } from '@fortawesome/free-solid-svg-icons/faBullseye';
import { faPause } from '@fortawesome/free-solid-svg-icons/faPause';
import { faPlay } from '@fortawesome/free-solid-svg-icons/faPlay';
import { faTrashCan } from '@fortawesome/free-solid-svg-icons/faTrashCan';

import type { SessionEnvironment } from '../../session/ContextUsageModel.js';
import { iconTemplate } from '../icons/Icon.js';

export function goalPopover(
  environment: SessionEnvironment | undefined,
  draft: string,
  changing: boolean,
  setDraft: (value: string) => void,
  save: () => void,
  pause: () => void,
  resume: () => void,
  clear: () => void
) {
  const goal = environment?.goal;
  const active = goal?.status === 'active';
  const resumable = goal?.status === 'paused' || goal?.status === 'blocked';
  const completed = goal?.status === 'completed';
  return html`
    <aside class="context-popover goal-popover" data-testid="goal-popover" aria-label="持续目标">
      <div class="context-popover-heading">
        <strong>持续目标</strong>
        <span>${changing ? '正在同步' : active ? '正在推进' : resumable ? '已停止' : completed ? '已完成' : '当前 Thread'}</span>
      </div>
      ${goal?.summary ? html`
        <p class="goal-summary" data-status=${goal.status}>
          ${iconTemplate(faBullseye)}
          <span>${goal.summary}${goal.truncated ? '…' : ''}</span>
        </p>
      ` : html`<p class="context-empty">尚未设置持续目标。</p>`}
      <label class="goal-label" for="dock-goal-input">
        ${active ? '输入新的目标以替换当前目标' : '设置要持续追求的目标'}
      </label>
      <textarea
        id="dock-goal-input"
        class="goal-input"
        data-testid="goal-input"
        rows="3"
        maxlength="2000"
        .value=${draft}
        placeholder="例如：完成可独立验收的浏览器 SDK 闭环"
        ?disabled=${changing || !environment}
        @input=${(event: InputEvent) => setDraft((event.currentTarget as HTMLTextAreaElement).value)}
      ></textarea>
      <div class="goal-actions">
        <button
          class="goal-action goal-save"
          data-testid="goal-save"
          type="button"
          ?disabled=${changing || !draft.trim()}
          @click=${save}
        >${iconTemplate(faBullseye)}<span>${active ? '替换目标' : '保存目标'}</span></button>
        ${active ? html`
          <button class="goal-action" data-testid="goal-pause" type="button" ?disabled=${changing} @click=${pause}>
            ${iconTemplate(faPause)}<span>暂停</span>
          </button>
        ` : nothing}
        ${resumable ? html`
          <button class="goal-action" data-testid="goal-resume" type="button" ?disabled=${changing} @click=${resume}>
            ${iconTemplate(faPlay)}<span>恢复</span>
          </button>
        ` : nothing}
        ${goal && goal.status !== 'none' ? html`
          <button class="goal-action goal-clear" data-testid="goal-clear" type="button" ?disabled=${changing} @click=${clear}>
            ${iconTemplate(faTrashCan)}<span>清除</span>
          </button>
        ` : nothing}
      </div>
      ${goal?.progressSummary ? html`
        <p class="goal-note" data-testid="goal-progress-summary">
          进度：${goal.progressSummary}
          ${goal.totalSteps ? ` · ${goal.completedSteps || 0}/${goal.totalSteps}` : ''}
        </p>
      ` : nothing}
      <p class="goal-note">完整 Goal 由 Runtime 保存；界面只恢复有界安全摘要和进度。</p>
    </aside>
  `;
}
