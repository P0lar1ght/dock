import { css } from 'lit';

export const runtimeIssueStyles = css`
  .runtime-issue .tool-card-diamond {
    color: var(--pv-danger);
  }

  .runtime-issue-details {
    display: grid;
    gap: 4px;
    margin-top: 2px;
  }

  .runtime-issue-details p {
    max-height: 4.5em;
    margin: 0;
    overflow: auto;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    font-size: 11px;
    line-height: 1.4;
  }

  .runtime-issue-retry-error {
    color: var(--pv-danger);
  }

  .runtime-issue-actions,
  .operation-issue-actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
  }

  .runtime-issue-actions button,
  .operation-issue-actions button {
    min-height: 22px;
    padding: 1px 8px;
    border: 1px solid var(--pv-border);
    border-radius: 4px;
    background: transparent;
    color: var(--pv-text);
    cursor: pointer;
    font: 11px/1.3 var(--pv-font);
  }

  .runtime-issue-actions .runtime-issue-primary {
    border-color: color-mix(in srgb, var(--pv-danger) 45%, var(--pv-border));
    color: var(--pv-danger);
  }

  .runtime-issue-actions button:disabled {
    cursor: wait;
    opacity: 0.55;
  }

  .runtime-issue-actions button:focus-visible,
  .operation-issue-actions button:focus-visible {
    outline: 1px solid var(--pv-accent);
    outline-offset: 1px;
  }

  .tool-card-body > .runtime-issue-details,
  .permission-details > .runtime-issue-details {
    padding-top: 4px;
  }

  .operation-issue {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 6px;
    padding: 4px 13px 2px;
    border-top: 1px solid var(--pv-border);
    color: var(--pv-danger);
    font-size: 11px;
  }

  .operation-issue-copy {
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

  .operation-issue + .composer-shell {
    padding-top: 6px;
    border-top: 0;
  }
`;
