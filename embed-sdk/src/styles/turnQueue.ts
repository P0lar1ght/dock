import { css } from 'lit';

export const turnQueueStyles = css`
  .turn-queue {
    display: grid;
    max-height: 112px;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 7px;
    padding: 7px 13px 2px;
    border-top: 1px solid var(--pv-border);
    color: var(--pv-muted);
    font-size: 10px;
  }

  .turn-queue-heading {
    display: flex;
    align-items: center;
    align-self: start;
    gap: 4px;
    padding-top: 4px;
    white-space: nowrap;
  }

  .turn-queue-heading strong {
    display: grid;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    place-items: center;
    border-radius: 8px;
    background: color-mix(in srgb, var(--pv-accent) 18%, transparent);
    color: var(--pv-accent);
    font-size: 9px;
  }

  .turn-queue-items {
    display: grid;
    min-width: 0;
    gap: 3px;
    overflow-y: auto;
  }

  .turn-queue-item {
    display: grid;
    min-width: 0;
    min-height: 25px;
    grid-template-columns: auto minmax(0, 1fr) 22px;
    align-items: center;
    gap: 6px;
    padding: 2px 3px 2px 7px;
    border-radius: 7px;
    background: color-mix(in srgb, var(--pv-panel-raised) 78%, transparent);
  }

  .turn-queue-kind {
    color: var(--pv-accent);
    font-weight: 750;
  }

  .turn-queue-item[data-kind="steer"] .turn-queue-kind {
    color: color-mix(in srgb, var(--pv-accent) 55%, #ffac58);
  }

  .turn-queue-message {
    overflow: hidden;
    color: var(--pv-text);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .turn-queue-item button {
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--pv-muted);
    cursor: pointer;
    font: inherit;
    font-size: 15px;
  }

  .turn-queue-item button:hover {
    background: color-mix(in srgb, var(--pv-danger) 13%, transparent);
    color: var(--pv-danger);
  }

  .turn-queue-item button:focus-visible {
    outline: 2px solid var(--pv-accent);
    outline-offset: 1px;
  }

  .turn-queue + .composer-shell {
    padding-top: 6px;
    border-top: 0;
  }
`;
