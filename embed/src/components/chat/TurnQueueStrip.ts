import { html, nothing } from 'lit';
import type { SessionTurnQueueItem } from '../../session/TurnQueueModel.js';

export function turnQueueStrip(
  items: readonly SessionTurnQueueItem[],
  remove: (queueId: string) => void
) {
  if (!items.length) return nothing;
  return html`
    <section class="turn-queue" data-testid="turn-queue" aria-label="排队消息">
      <div class="turn-queue-heading">
        <span>待处理</span>
        <strong>${items.length}</strong>
      </div>
      <div class="turn-queue-items">
        ${items.map((item) => html`
          <div class="turn-queue-item" data-queue-id=${item.id} data-kind=${item.kind}>
            <span class="turn-queue-kind">${item.kind === 'steer' ? '引导' : '排队'}</span>
            <span class="turn-queue-message" title=${boundedMessage(item.message)}>
              ${boundedMessage(item.message) || '等待开始的消息'}
            </span>
            <button
              type="button"
              aria-label="移除排队消息"
              @click=${() => remove(item.id)}
            >×</button>
          </div>
        `)}
      </div>
    </section>
  `;
}

function boundedMessage(value: string | undefined) {
  const message = String(value || '').replace(/\s+/g, ' ').trim();
  return message.length > 160 ? `${message.slice(0, 157)}…` : message;
}
