import { css } from 'lit';

export const runtimeIssueStyles = css`
  .runtime-issue {
    width: 100%;
    border-left: 2px solid var(--pv-danger);
    border-radius: 0 9px 9px 0;
    background: color-mix(in srgb, var(--pv-danger) 7%, transparent);
    color: var(--pv-muted);
    font-size: 10px;
  }

  .runtime-issue summary {
    display: grid;
    min-height: 32px;
    grid-template-columns: 18px minmax(0, 1fr) auto 12px;
    align-items: center;
    gap: 6px;
    padding: 1px 7px;
    cursor: pointer;
    list-style: none;
  }

  .runtime-issue summary::-webkit-details-marker {
    display: none;
  }

  .runtime-issue summary:focus-visible,
  .runtime-issue-actions button:focus-visible,
  .operation-issue-actions button:focus-visible {
    outline: 2px solid var(--pv-accent);
    outline-offset: 1px;
  }

  .runtime-issue-icon {
    display: grid;
    width: 16px;
    height: 16px;
    flex: 0 0 auto;
    place-items: center;
    border-radius: 50%;
    background: var(--pv-danger);
    color: white;
    font-size: 10px;
    font-weight: 900;
  }

  .runtime-issue-title,
  .runtime-issue-action-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .runtime-issue-title {
    color: var(--pv-text);
    font-weight: 700;
  }

  .runtime-issue-action-label {
    color: var(--pv-danger);
    font-size: 9px;
  }

  .runtime-issue-chevron {
    font-size: 15px;
    transition: transform 120ms ease;
  }

  .runtime-issue[open] .runtime-issue-chevron {
    transform: rotate(90deg);
  }

  .runtime-issue-details {
    display: grid;
    gap: 7px;
    padding: 3px 8px 9px 25px;
  }

  .runtime-issue-details p {
    max-height: 72px;
    margin: 0;
    overflow: auto;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .runtime-issue-retry-error {
    color: var(--pv-danger);
  }

  .runtime-issue-actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
  }

  .runtime-issue-actions button,
  .operation-issue-actions button {
    min-height: 25px;
    padding: 3px 7px;
    border: 1px solid var(--pv-border);
    border-radius: 7px;
    background: var(--pv-panel-raised);
    color: var(--pv-text);
    cursor: pointer;
    font: inherit;
    font-weight: 700;
  }

  .runtime-issue-actions .runtime-issue-primary {
    border-color: color-mix(in srgb, var(--pv-danger) 45%, var(--pv-border));
    color: var(--pv-danger);
  }

  .runtime-issue-actions button:disabled {
    cursor: wait;
    opacity: 0.55;
  }

  .tool-details > .runtime-issue-details,
  .permission-details > .runtime-issue-details {
    padding: 4px 0 0;
    border-top: 1px solid var(--pv-border);
  }

  .operation-issue {
    display: grid;
    min-height: 34px;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 7px;
    padding: 5px 13px 2px;
    border-top: 1px solid var(--pv-border);
    color: var(--pv-danger);
    font-size: 9px;
  }

  .operation-issue-copy {
    display: grid;
    min-width: 0;
  }

  .operation-issue-copy strong,
  .operation-issue-copy span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .operation-issue-copy strong {
    color: var(--pv-text);
  }

  .operation-issue-actions {
    display: flex;
    gap: 4px;
  }

  .operation-issue + .composer-shell {
    padding-top: 6px;
    border-top: 0;
  }

  @media (prefers-reduced-motion: reduce) {
    .runtime-issue-chevron { transition: none; }
  }
`;
