import { html } from 'lit';
import { faBrain } from '@fortawesome/free-solid-svg-icons/faBrain';
import { faCheck } from '@fortawesome/free-solid-svg-icons/faCheck';
import { faGaugeHigh } from '@fortawesome/free-solid-svg-icons/faGaugeHigh';
import type { ReasoningEffort } from '../../protocol/responses.js';
import type { SessionEnvironment } from '../../session/ContextUsageModel.js';
import { iconTemplate } from '../icons/Icon.js';

const LABELS: Record<ReasoningEffort, string> = {
  none: '无',
  minimal: '最小',
  low: '低',
  medium: '中',
  high: '高',
  xhigh: '极高',
  max: '最大'
};

export function reasoningSelectionPopover(
  environment: SessionEnvironment | undefined,
  activeTurn: boolean,
  changing: boolean,
  select: (effort: ReasoningEffort) => void,
  openContext: () => void
) {
  const reasoning = environment?.reasoning;
  return html`
    <aside class="context-popover policy-popover" data-testid="reasoning-popover" aria-label="推理强度">
      <div class="context-popover-heading">
        <strong>推理强度</strong>
        <span>${activeTurn ? '下个回复生效' : environment?.model.label || '当前模型'}</span>
      </div>
      <div class="model-options" role="radiogroup" aria-label="可用推理强度">
        ${(reasoning?.options || []).map((effort) => html`
          <button
            class="model-option"
            data-testid=${`reasoning-option-${effort}`}
            type="button"
            role="radio"
            aria-checked=${String(effort === reasoning?.effort)}
            ?disabled=${changing}
            @click=${() => select(effort)}
          >
            ${iconTemplate(faBrain)}
            <span><strong>${LABELS[effort]}</strong><small>${effort}</small></span>
            ${effort === reasoning?.effort ? iconTemplate(faCheck) : ''}
          </button>
        `)}
      </div>
      ${reasoning?.options.length ? '' : html`<p class="context-empty">当前模型未声明可切换的推理强度。</p>`}
      <button class="composer-menu-row" type="button" @click=${openContext}>
        ${iconTemplate(faGaugeHigh)}
        <span><strong>状态</strong><small>返回上下文用量</small></span>
      </button>
    </aside>
  `;
}

export function reasoningLabel(effort: ReasoningEffort | undefined) {
  return effort ? LABELS[effort] : '模型默认';
}
