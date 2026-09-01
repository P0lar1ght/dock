import { css } from 'lit';

export const turnControlStyles = css`
  .chat-footer {
    min-width: 0;
  }

  .turn-controls {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 5px 10px;
    padding: 8px 13px 2px;
    border-top: 1px solid var(--pv-border);
    color: var(--pv-muted);
    font-size: 10px;
  }

  .turn-control-state {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 6px;
  }

  .turn-control-state > span:last-child,
  .turn-cancel-error {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .turn-control-dot {
    width: 6px;
    height: 6px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: var(--pv-accent);
    animation: pv-tool-pulse 900ms ease-in-out infinite;
  }

  .turn-cancel-error {
    grid-column: 1;
    color: var(--pv-danger);
  }

  .turn-cancel-button {
    display: inline-flex;
    min-height: 28px;
    grid-column: 2;
    grid-row: 1 / span 2;
    align-items: center;
    gap: 5px;
    padding: 4px 8px;
    border: 1px solid color-mix(in srgb, var(--pv-danger) 45%, var(--pv-border));
    border-radius: 8px;
    background: color-mix(in srgb, var(--pv-danger) 10%, var(--pv-panel-raised));
    color: var(--pv-danger);
    cursor: pointer;
    font: inherit;
    font-weight: 700;
  }

  .turn-cancel-button:disabled {
    cursor: wait;
    opacity: 0.58;
  }

  .turn-cancel-button:focus-visible {
    outline: 2px solid var(--pv-accent);
    outline-offset: 2px;
  }

  .turn-controls + .composer-shell {
    padding-top: 6px;
    border-top: 0;
  }

  .message-row[data-status="cancelled"] .message-bubble {
    border-color: color-mix(in srgb, var(--pv-muted) 58%, var(--pv-border));
  }

  .message-cancelled {
    display: block;
    margin-top: 5px;
    color: var(--pv-muted);
    font-size: 10px;
  }

  .tool-activity[data-status="cancelled"] .tool-status-icon {
    color: var(--pv-muted);
  }

  @media (prefers-reduced-motion: reduce) {
    .turn-control-dot { animation: none !important; }
  }
`;
