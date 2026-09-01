import { css } from 'lit';

export const recoveryStyles = css`
  .connection-status {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 7px;
    min-height: 30px;
    padding: 6px 13px 2px;
    border-top: 1px solid var(--pv-border);
    color: var(--pv-muted);
    font-size: 10px;
  }

  .connection-status[data-phase='error'],
  .connection-status[data-phase='offline'] {
    color: var(--pv-danger);
  }

  .connection-status-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
  }

  .connection-status[data-phase='connecting'] .connection-status-dot,
  .connection-status[data-phase='reconnecting'] .connection-status-dot,
  .connection-status[data-phase='recovering'] .connection-status-dot {
    background: var(--pv-accent);
    animation: pv-tool-pulse 900ms ease-in-out infinite;
  }

  .connection-status-copy {
    display: flex;
    min-width: 0;
    flex-direction: column;
    line-height: 1.35;
  }

  .connection-status-copy strong,
  .connection-status-copy span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .connection-status-copy strong {
    color: var(--pv-text);
    font-weight: 700;
  }

  .connection-status[data-phase='error'] .connection-status-copy strong,
  .connection-status[data-phase='offline'] .connection-status-copy strong {
    color: currentColor;
  }

  .connection-retry {
    min-height: 26px;
    padding: 3px 8px;
    border: 1px solid currentColor;
    border-radius: 8px;
    background: transparent;
    color: inherit;
    cursor: pointer;
    font: inherit;
    font-weight: 750;
  }

  .connection-actions {
    display: inline-flex;
    gap: 5px;
  }

  .pairing-guide {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 6px 8px;
    padding: 8px 13px;
    border-top: 1px solid var(--pv-border);
    background: color-mix(in srgb, var(--pv-accent) 7%, var(--pv-panel));
    color: var(--pv-muted);
    font-size: 10px;
  }

  .pairing-guide-heading {
    display: flex;
    min-width: 0;
    flex-direction: column;
    line-height: 1.35;
  }

  .pairing-guide-heading strong {
    color: var(--pv-text);
  }

  .pairing-guide code,
  .pairing-guide-error {
    grid-column: 1 / -1;
  }

  .pairing-guide code {
    overflow-wrap: anywhere;
    padding: 6px 8px;
    border: 1px solid var(--pv-border);
    border-radius: 7px;
    background: var(--pv-panel-raised);
    color: var(--pv-text);
    font: 600 9px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    user-select: all;
  }

  .pairing-guide-error {
    color: var(--pv-danger);
    line-height: 1.35;
  }

  .pairing-refresh {
    align-self: center;
    min-height: 26px;
    padding: 3px 8px;
    border: 1px solid var(--pv-border);
    border-radius: 8px;
    background: transparent;
    color: var(--pv-text);
    cursor: pointer;
    font: inherit;
    font-weight: 750;
  }

  .pairing-refresh:disabled {
    cursor: default;
    opacity: 0.55;
  }

  .gateway-settings {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    gap: 6px;
    padding: 7px 13px 8px;
    border-top: 1px solid var(--pv-border);
    background: var(--pv-panel);
    font-size: 10px;
  }

  .gateway-settings label {
    grid-column: 1 / -1;
    color: var(--pv-muted);
    font-weight: 700;
  }

  .gateway-settings input {
    min-width: 0;
    height: 28px;
    padding: 0 8px;
    border: 1px solid var(--pv-border);
    border-radius: 8px;
    outline: 0;
    background: var(--pv-panel-raised);
    color: var(--pv-text);
    font: inherit;
  }

  .gateway-settings input:focus {
    border-color: var(--pv-accent);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--pv-accent) 20%, transparent);
  }

  .gateway-settings button {
    min-height: 28px;
    padding: 3px 8px;
    border: 1px solid var(--pv-border);
    border-radius: 8px;
    background: transparent;
    color: var(--pv-text);
    cursor: pointer;
    font: inherit;
    font-weight: 750;
  }

  .gateway-settings .gateway-connect {
    border-color: var(--pv-accent);
    background: var(--pv-accent);
    color: var(--pv-panel);
  }

  .gateway-settings button:disabled,
  .gateway-settings input:disabled {
    cursor: default;
    opacity: 0.55;
  }

  .gateway-settings-error {
    grid-column: 1 / -1;
    color: var(--pv-danger);
    line-height: 1.35;
  }

  .connection-retry:focus-visible {
    outline: 2px solid var(--pv-accent);
    outline-offset: 2px;
  }

  .connection-status + .turn-controls {
    border-top: 0;
  }

  .connection-status + .composer-shell {
    padding-top: 6px;
    border-top: 0;
  }

  @media (prefers-reduced-motion: reduce) {
    .connection-status-dot { animation: none !important; }
  }
`;
