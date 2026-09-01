import { css } from 'lit';

export const approvalStyles = css`
  .permission-prompt[data-risk="high"] .tool-card-diamond,
  .permission-prompt[data-status="failed"] .tool-card-diamond {
    color: var(--pv-danger);
  }

  .permission-prompt[data-status="approved"] .tool-card-diamond {
    color: var(--pv-success);
  }

  .permission-prompt[data-status="pending"] .tool-card-diamond {
    color: var(--pv-warning);
  }

  .permission-reason,
  .permission-error {
    margin: 0;
    font-size: 11px;
    line-height: 1.4;
  }

  .permission-error {
    color: var(--pv-danger);
  }

  .permission-actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    margin-top: 4px;
  }

  .permission-actions button {
    min-height: 22px;
    padding: 1px 8px;
    border: 1px solid var(--pv-border);
    border-radius: 4px;
    background: transparent;
    color: var(--pv-text);
    cursor: pointer;
    font: 11px/1.3 var(--pv-font);
  }

  .permission-actions .permission-approve {
    border-color: color-mix(in srgb, var(--pv-accent) 55%, var(--pv-border));
    color: var(--pv-accent);
  }

  .permission-actions button:disabled {
    cursor: wait;
    opacity: 0.55;
  }

  .permission-actions button:focus-visible {
    outline: 1px solid var(--pv-accent);
    outline-offset: 1px;
  }
`;
