import { html, nothing } from 'lit';
import type { TurnControlView } from '../../controllers/TurnController.js';

export function turnControls(model: TurnControlView, cancel: () => void) {
  if (!model.turnId) return nothing;
  const label = model.cancelling ? '正在停止当前回复…' : '嘟嘟正在回复';
  return html`
    <div
      class="turn-controls"
      data-testid="turn-controls"
      data-turn-id=${model.turnId}
      data-cancelling=${String(model.cancelling)}
      role="status"
      aria-live="polite"
    >
      <span class="turn-control-state">
        <span class="turn-control-dot" aria-hidden="true"></span>
        <span>${label}</span>
      </span>
      ${model.error ? html`<span class="turn-cancel-error" role="alert">${model.error}</span>` : nothing}
      <button
        class="turn-cancel-button"
        data-testid="turn-cancel"
        type="button"
        aria-label=${model.error ? '重试停止当前回复' : '停止当前回复'}
        ?disabled=${model.cancelling}
        @click=${cancel}
      >
        <span aria-hidden="true">■</span>${model.error ? '重试停止' : '停止生成'}
      </button>
    </div>
  `;
}
