import { css } from 'lit';

export const subagentActivityStyles = css`
  .subagent-activity[data-status='running'] .tool-card-diamond,
  .subagent-activity[data-status='background'] .tool-card-diamond {
    animation: pv-tool-pulse 900ms ease-in-out infinite;
  }

  .subagent-activity[data-status='waiting_permission'] .tool-card-diamond {
    color: var(--pv-warning);
  }

  .subagent-activity[data-status='failed'] .tool-card-diamond {
    color: var(--pv-danger);
  }
`;
