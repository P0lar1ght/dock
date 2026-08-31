import { html, nothing } from 'lit';
import type { RecoveryView } from '../../controllers/RecoveryController.js';
import type { GatewayConnectionView } from '../../controllers/GatewayConnectionController.js';
import type { PairingView } from '../../controllers/PairingController.js';

export interface ConnectionStatusActions {
  retry: () => void;
  refreshPairing: () => void;
  openGateway: () => void;
  gatewayDraft: (value: string) => void;
  connectGateway: () => void;
  cancelGateway: () => void;
}

export function connectionStatus(
  model: RecoveryView,
  pairing: PairingView | undefined,
  gateway: GatewayConnectionView | undefined,
  actions: ConnectionStatusActions
) {
  if (!model.visible) return nothing;
  const urgent = model.phase === 'error' || model.phase === 'offline';
  return html`
    <div
      class="connection-status"
      data-testid="connection-status"
      data-phase=${model.phase}
      role=${urgent ? 'alert' : 'status'}
      aria-live=${urgent ? 'assertive' : 'polite'}
    >
      <span class="connection-status-dot" aria-hidden="true"></span>
      <span class="connection-status-copy">
        <strong>${model.label}</strong>
        ${model.detail ? html`<span>${model.detail}</span>` : nothing}
      </span>
      ${model.retryable && !gateway?.editing ? html`
        <span class="connection-actions">
          ${gateway?.canChange ? html`
            <button
              class="connection-retry"
              data-testid="gateway-settings-open"
              type="button"
              @click=${actions.openGateway}
            >设置</button>
          ` : nothing}
          <button
            class="connection-retry"
            data-testid="connection-retry"
            type="button"
            @click=${actions.retry}
          >重试</button>
        </span>
      ` : nothing}
    </div>
    ${pairing?.visible ? pairingGuide(pairing, actions) : nothing}
    ${gateway?.editing ? gatewayEditor(gateway, actions) : nothing}
  `;
}

function pairingGuide(model: PairingView, actions: ConnectionStatusActions) {
  return html`
    <div
      class="pairing-guide"
      data-testid="pairing-guide"
      data-phase=${model.phase}
      data-pairing-request-id=${model.pairingRequestId || nothing}
      role="status"
      aria-live="polite"
    >
      <span class="pairing-guide-heading">
        <strong>首次配对</strong>
        <span>${model.phase === 'requesting' ? '正在创建本机请求…' : '在本机终端运行后点击重试'}</span>
      </span>
      ${model.command ? html`
        <code data-testid="pairing-command">${model.command}</code>
      ` : nothing}
      ${model.error ? html`<span class="pairing-guide-error" role="alert">${model.error}</span>` : nothing}
      <button
        class="pairing-refresh"
        data-testid="pairing-refresh"
        type="button"
        ?disabled=${model.phase === 'requesting'}
        @click=${actions.refreshPairing}
      >重新生成</button>
    </div>
  `;
}

function gatewayEditor(model: GatewayConnectionView, actions: ConnectionStatusActions) {
  return html`
    <form
      class="gateway-settings"
      data-testid="gateway-settings"
      @submit=${(event: SubmitEvent) => {
        event.preventDefault();
        actions.connectGateway();
      }}
    >
      <label for="dock-gateway-url">本机 Gateway</label>
      <input
        id="dock-gateway-url"
        data-testid="gateway-url-input"
        type="url"
        inputmode="url"
        autocomplete="off"
        spellcheck="false"
        placeholder="http://127.0.0.1:18990"
        .value=${model.draft}
        ?disabled=${model.connecting}
        @input=${(event: InputEvent) => actions.gatewayDraft(
          (event.currentTarget as HTMLInputElement).value
        )}
        @keydown=${(event: KeyboardEvent) => {
          if (event.key !== 'Escape') return;
          event.preventDefault();
          actions.cancelGateway();
        }}
      />
      <button type="button" ?disabled=${model.connecting} @click=${actions.cancelGateway}>取消</button>
      <button
        class="gateway-connect"
        data-testid="gateway-connect"
        type="submit"
        ?disabled=${model.connecting || !model.draft.trim()}
      >${model.connecting ? '连接中' : '连接'}</button>
      ${model.error ? html`<span class="gateway-settings-error" role="alert">${model.error}</span>` : nothing}
    </form>
  `;
}
