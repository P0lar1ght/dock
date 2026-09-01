import { css } from 'lit';

export const goalActivityStyles = css`
  .goal-progress-row[data-status="blocked"] .tool-card-diamond {
    color: var(--pv-danger);
  }

  .goal-progress-row[data-status="completed"] .tool-card-diamond {
    color: var(--pv-success);
  }

  .goal-progress-blocked {
    margin: 2px 0 0;
    color: var(--pv-danger);
  }
`;
