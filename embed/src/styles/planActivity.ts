import { css } from 'lit';

export const planActivityStyles = css`
  .plan-details {
    margin-top: 2px;
  }

  .todo-list {
    display: grid;
    gap: 1px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .todo-step {
    display: grid;
    grid-template-columns: 12px minmax(0, 1fr);
    align-items: baseline;
    gap: 6px;
    color: var(--pv-muted);
    font-size: 12px;
    line-height: 1.35;
  }

  .todo-step-icon {
    color: var(--pv-muted);
    font-size: 11px;
    text-align: center;
  }

  .todo-step[data-status='in_progress'] .todo-step-icon,
  .todo-step[data-status='in_progress'] .todo-step-content {
    color: var(--pv-accent);
  }

  .todo-step[data-status='completed'] .todo-step-content {
    color: var(--pv-muted);
    text-decoration: line-through;
    text-decoration-color: color-mix(in srgb, var(--pv-muted) 45%, transparent);
  }

  .todo-step-copy {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .todo-step-active {
    display: block;
    color: var(--pv-muted);
    font-size: 11px;
  }

  .todo-step-status {
    display: none;
  }

  .plan-empty,
  .plan-degraded {
    margin: 0;
    color: var(--pv-muted);
    font-size: 11px;
    line-height: 1.4;
  }

  .plan-degraded {
    margin-top: 4px;
    color: var(--pv-warning);
  }
`;
