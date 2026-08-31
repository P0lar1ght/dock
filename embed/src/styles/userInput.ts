import { css } from 'lit';

export const userInputStyles = css`
  .user-input-card {
    display: grid;
    gap: 10px;
    padding: 12px;
    border: 1px solid color-mix(in srgb, var(--pv-accent) 38%, var(--pv-border));
    border-radius: 14px;
    background: color-mix(in srgb, var(--pv-surface) 94%, var(--pv-accent));
  }
  .user-input-card > header {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    align-items: baseline;
  }
  .user-input-card > header strong { font-size: 12px; }
  .user-input-card > header span,
  .user-input-card > p { color: var(--pv-muted); font-size: 9px; }
  .user-input-card fieldset {
    display: grid;
    gap: 6px;
    padding: 0;
    border: 0;
  }
  .user-input-card legend {
    display: grid;
    gap: 2px;
    width: 100%;
    margin-bottom: 5px;
    color: var(--pv-text);
    font-size: 11px;
  }
  .user-input-card legend small {
    color: var(--pv-accent);
    font-size: 9px;
    font-weight: 700;
  }
  .user-input-card label {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 8px;
    border: 1px solid var(--pv-border);
    border-radius: 10px;
    cursor: pointer;
  }
  .user-input-card label > span { display: grid; gap: 2px; }
  .user-input-card label strong { font-size: 10px; }
  .user-input-card label small { color: var(--pv-muted); font-size: 9px; }
  .user-input-card label em {
    color: var(--pv-accent);
    font-size: 8px;
    font-style: normal;
  }
  .user-input-other {
    box-sizing: border-box;
    width: 100%;
    padding: 8px 10px;
    border: 1px solid var(--pv-border);
    border-radius: 9px;
    color: var(--pv-text);
    background: var(--pv-surface);
  }
  .user-input-card > button {
    justify-self: end;
    padding: 7px 11px;
    border: 0;
    border-radius: 9px;
    color: white;
    background: var(--pv-accent);
    cursor: pointer;
  }
  .user-input-card > button:disabled { opacity: .55; cursor: default; }
  .user-input-error { color: var(--pv-danger, #c33) !important; }
  .user-input-card[data-status='resolved'] { opacity: .72; }
`;
