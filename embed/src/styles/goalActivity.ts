import { css } from 'lit';

export const goalActivityStyles = css`
  .goal-progress-row {
    margin: 8px 36px;
    padding: 10px 12px;
    border: 1px solid color-mix(in srgb, var(--pv-accent) 26%, var(--pv-border));
    border-radius: 12px;
    background: color-mix(in srgb, var(--pv-accent) 5%, var(--pv-surface));
  }

  .goal-progress-row header {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 11px;
    color: var(--pv-muted);
  }

  .goal-progress-row header strong { color: var(--pv-text); }
  .goal-progress-objective,
  .goal-progress-copy,
  .goal-progress-blocked { margin: 7px 0 0; font-size: 12px; line-height: 1.45; }
  .goal-progress-copy { color: var(--pv-muted); }
  .goal-progress-blocked { color: var(--pv-danger); }

  .goal-progress-meter {
    height: 4px;
    margin-top: 9px;
    overflow: hidden;
    border-radius: 99px;
    background: color-mix(in srgb, var(--pv-text) 10%, transparent);
  }

  .goal-progress-meter span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--pv-accent);
  }

  .goal-progress-row small {
    display: block;
    margin-top: 5px;
    color: var(--pv-muted);
    font-size: 10px;
  }

  .goal-progress-row[data-status="completed"] {
    border-color: color-mix(in srgb, #45a56a 40%, var(--pv-border));
  }

  .goal-progress-row[data-status="blocked"] {
    border-color: color-mix(in srgb, var(--pv-danger) 35%, var(--pv-border));
  }
`;
