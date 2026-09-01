import { html } from 'lit';
import { faCheck } from '@fortawesome/free-solid-svg-icons/faCheck';
import { faGaugeHigh } from '@fortawesome/free-solid-svg-icons/faGaugeHigh';
import { faArrowsRotate } from '@fortawesome/free-solid-svg-icons/faArrowsRotate';
import type { SessionEnvironment } from '../../session/ContextUsageModel.js';
import { iconTemplate } from '../icons/Icon.js';

export function modelSelectionPopover(
  environment: SessionEnvironment | undefined,
  activeTurn: boolean,
  changing: boolean,
  select: (modelId: string) => void,
  refresh: () => void,
  openContext: () => void
) {
  const current = environment?.model;
  const options = current?.options || [];
  return html`
    <aside class="context-popover model-popover" data-testid="model-popover" aria-label="选择模型">
      <div class="context-popover-heading">
        <strong>模型</strong>
        <span>${activeTurn ? '下个回复生效' : '当前 Thread'}</span>
      </div>
      <div class="model-options" role="radiogroup" aria-label="可用模型">
        ${options.map((model) => html`
          <button
            class="model-option"
            data-testid=${`model-option-${model.id}`}
            type="button"
            role="radio"
            aria-checked=${String(model.id === current?.id)}
            ?disabled=${changing || model.available === false}
            @click=${() => select(model.id)}
          >
            <span>
              <strong>${model.label}</strong>
              <small>${[
                model.providerLabel,
                model.id,
                model.available === false ? '端点未暴露' : ''
              ].filter(Boolean).join(' · ')}</small>
            </span>
            ${model.id === current?.id ? iconTemplate(faCheck) : ''}
          </button>
        `)}
      </div>
      ${options.length ? '' : html`<p class="context-empty">Gateway 尚未提供可选模型。</p>`}
      ${current?.discovery?.canRefresh ? html`
        <button
          class="composer-menu-row model-refresh"
          data-testid="model-refresh"
          type="button"
          ?disabled=${changing}
          @click=${refresh}
        >
          ${iconTemplate(faArrowsRotate)}
          <span>
            <strong>${changing ? '正在刷新…' : '刷新模型目录'}</strong>
            <small>${discoveryLabel(current.discovery)}</small>
          </span>
        </button>
      ` : ''}
      <button class="composer-menu-row" type="button" @click=${openContext}>
        ${iconTemplate(faGaugeHigh)}
        <span><strong>状态</strong><small>查看上下文用量</small></span>
      </button>
    </aside>
  `;
}

function discoveryLabel(discovery: NonNullable<SessionEnvironment['model']['discovery']>) {
  const total = discovery.totalCount || discovery.configuredCount;
  if (discovery.status === 'failed') return '端点发现失败，保留上次可用目录';
  if (discovery.status === 'partial') return `部分端点可用 · ${discovery.availableCount}/${total}`;
  if (discovery.status === 'ready') return `已发现 · ${discovery.availableCount}/${total}`;
  return `${discovery.availableCount}/${total} 个模型`;
}
