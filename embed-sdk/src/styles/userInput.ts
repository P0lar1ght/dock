import { css } from 'lit';

export const userInputStyles = css`
  .user-input-card fieldset {
    display: grid;
    gap: 2px;
    margin: 0 0 6px;
    padding: 0;
    border: 0;
  }

  .user-input-card legend {
    display: grid;
    gap: 1px;
    width: 100%;
    margin-bottom: 3px;
    padding: 0;
    color: var(--pv-text);
    font-size: 12px;
  }

  .user-input-card legend small {
    color: var(--pv-muted);
    font-size: 11px;
    font-weight: 600;
  }

  .user-input-card label {
    display: flex;
    gap: 6px;
    align-items: baseline;
    padding: 1px 0;
    cursor: pointer;
    font-size: 12px;
    line-height: 1.35;
  }

  .user-input-card label > span {
    display: grid;
    min-width: 0;
    gap: 0;
  }

  .user-input-card label strong {
    font-size: 12px;
    font-weight: 650;
  }

  .user-input-card label small {
    color: var(--pv-muted);
    font-size: 11px;
  }

  .user-input-card label em {
    color: var(--pv-accent);
    font-size: 11px;
    font-style: normal;
  }

  .user-input-other {
    box-sizing: border-box;
    width: 100%;
    margin-top: 2px;
    padding: 3px 6px;
    border: 1px solid var(--pv-border);
    border-radius: 4px;
    color: var(--pv-text);
    background: transparent;
    font: 12px/1.35 var(--pv-font);
  }

  .user-input-card button {
    margin-top: 4px;
    min-height: 22px;
    padding: 1px 8px;
    border: 1px solid color-mix(in srgb, var(--pv-accent) 55%, var(--pv-border));
    border-radius: 4px;
    color: var(--pv-accent);
    background: transparent;
    cursor: pointer;
    font: 11px/1.3 var(--pv-font);
  }

  .user-input-card button:disabled {
    cursor: wait;
    opacity: 0.55;
  }

  .user-input-error {
    margin: 0;
    color: var(--pv-danger);
    font-size: 11px;
  }
`;
