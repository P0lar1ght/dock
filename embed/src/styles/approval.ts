import { css } from 'lit';

export const approvalStyles = css`
  .permission-prompt {
    width: 100%;
    border-left: 2px solid var(--pv-accent);
    border-radius: 0 9px 9px 0;
    background: color-mix(in srgb, var(--pv-accent) 7%, transparent);
    color: var(--pv-muted);
    font-size: 11px;
  }

  .permission-prompt[data-risk="high"] {
    border-left-color: var(--pv-danger);
    background: color-mix(in srgb, var(--pv-danger) 8%, transparent);
  }

  .permission-prompt summary {
    display: grid;
    min-height: 34px;
    grid-template-columns: 18px minmax(0, 1fr) auto auto 12px;
    align-items: center;
    gap: 6px;
    padding: 2px 7px;
    cursor: pointer;
    list-style: none;
  }

  .permission-prompt summary::-webkit-details-marker {
    display: none;
  }

  .permission-prompt summary:focus-visible {
    outline: 2px solid var(--pv-accent);
    outline-offset: 1px;
  }

  .permission-icon {
    display: grid;
    width: 16px;
    height: 16px;
    place-items: center;
    border-radius: 50%;
    background: var(--pv-accent);
    color: #052033;
    font-size: 10px;
    font-weight: 900;
  }

  .permission-prompt[data-risk="high"] .permission-icon,
  .permission-prompt[data-status="failed"] .permission-icon {
    background: var(--pv-danger);
    color: #fff;
  }

  .permission-prompt[data-status="approved"] .permission-icon {
    background: var(--pv-success);
  }

  .permission-title {
    overflow: hidden;
    color: var(--pv-text);
    font-weight: 650;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .permission-risk {
    padding: 2px 5px;
    border-radius: 5px;
    background: var(--pv-panel-raised);
    font-size: 9px;
  }

  .permission-state {
    color: var(--pv-text);
    font-size: 9px;
  }

  .permission-chevron {
    font-size: 15px;
    transition: transform 120ms ease;
  }

  .permission-prompt[open] .permission-chevron {
    transform: rotate(90deg);
  }

  .permission-details {
    display: grid;
    max-height: 190px;
    gap: 7px;
    padding: 3px 9px 9px 25px;
    overflow: auto;
    scrollbar-width: thin;
  }

  .permission-details p,
  .permission-details h4,
  .permission-details pre {
    margin: 0;
  }

  .permission-scope {
    color: var(--pv-text);
    font-size: 10px;
  }

  .permission-reason {
    line-height: 1.4;
  }

  .permission-details h4 {
    margin-bottom: 3px;
    color: var(--pv-text);
    font-size: 10px;
    font-weight: 650;
  }

  .permission-details pre {
    max-height: 60px;
    overflow: auto;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    font: 10px/1.4 var(--pv-font);
  }

  .permission-error {
    color: var(--pv-danger);
    font-size: 10px;
  }

  .permission-actions {
    display: flex;
    justify-content: flex-end;
    gap: 7px;
  }

  .permission-actions button {
    min-width: 58px;
    min-height: 29px;
    padding: 4px 9px;
    border: 1px solid var(--pv-border);
    border-radius: 8px;
    background: var(--pv-panel-raised);
    color: var(--pv-text);
    cursor: pointer;
    font: inherit;
    font-weight: 700;
  }

  .permission-actions .permission-approve {
    border-color: transparent;
    background: var(--pv-accent-strong);
    color: #052033;
  }

  .permission-actions button:disabled {
    cursor: wait;
    opacity: 0.55;
  }

  .permission-actions button:focus-visible {
    outline: 2px solid var(--pv-accent);
    outline-offset: 2px;
  }

  @media (prefers-reduced-motion: reduce) {
    .permission-chevron {
      transition: none;
    }
  }
`;
