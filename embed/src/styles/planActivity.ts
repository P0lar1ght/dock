import { css } from 'lit';

export const planActivityStyles = css`
  .plan-activity {
    margin: 1px 4px;
    color: var(--pv-text-soft);
    font-size: 11px;
  }

  .plan-activity > summary {
    display: flex;
    min-height: 25px;
    align-items: center;
    gap: 7px;
    padding: 2px 5px;
    border-radius: 7px;
    cursor: pointer;
    list-style: none;
    transition: background 120ms ease;
  }

  .plan-activity > summary::-webkit-details-marker {
    display: none;
  }

  .plan-activity > summary:hover,
  .plan-activity > summary:focus-visible {
    background: rgba(255, 255, 255, 0.045);
    outline: none;
  }

  .plan-status-icon {
    display: grid;
    width: 15px;
    height: 15px;
    flex: 0 0 auto;
    place-items: center;
    border: 1px solid rgba(143, 209, 255, 0.34);
    border-radius: 50%;
    color: var(--pv-accent);
    font-size: 10px;
    font-weight: 800;
  }

  .plan-activity[data-status='running'] .plan-status-icon {
    animation: pv-plan-pulse 1.4s ease-in-out infinite;
  }

  .plan-activity[data-status='failed'] .plan-status-icon {
    border-color: rgba(255, 132, 132, 0.42);
    color: #ff9c9c;
  }

  .plan-activity[data-status='cancelled'] .plan-status-icon {
    color: var(--pv-muted);
  }

  .plan-row-title {
    min-width: 0;
    overflow: hidden;
    color: var(--pv-text-soft);
    font-weight: 650;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .plan-progress {
    margin-left: auto;
    color: var(--pv-muted);
    font-variant-numeric: tabular-nums;
  }

  .plan-chevron {
    color: var(--pv-muted);
    font-size: 16px;
    line-height: 1;
    transition: transform 120ms ease;
  }

  .plan-activity[open] .plan-chevron {
    transform: rotate(90deg);
  }

  .plan-details {
    margin: 1px 4px 5px 12px;
    padding: 3px 0 2px 10px;
    border-left: 1px solid rgba(143, 209, 255, 0.18);
  }

  .todo-list {
    display: grid;
    gap: 5px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .todo-step {
    display: grid;
    grid-template-columns: 15px minmax(0, 1fr) auto;
    align-items: start;
    gap: 6px;
    color: var(--pv-text-soft);
  }

  .todo-step-icon {
    color: var(--pv-muted);
    font-size: 11px;
    line-height: 16px;
    text-align: center;
  }

  .todo-step[data-status='in_progress'] .todo-step-icon,
  .todo-step[data-status='in_progress'] .todo-step-content {
    color: var(--pv-accent);
  }

  .todo-step[data-status='completed'] .todo-step-content {
    color: var(--pv-muted);
    text-decoration: line-through;
    text-decoration-color: rgba(255, 255, 255, 0.2);
  }

  .todo-step-copy {
    display: grid;
    min-width: 0;
    gap: 1px;
  }

  .todo-step-content,
  .todo-step-active {
    overflow-wrap: anywhere;
    line-height: 1.45;
  }

  .todo-step-active {
    color: var(--pv-muted);
    font-size: 10px;
  }

  .todo-step-status {
    padding-top: 1px;
    color: var(--pv-muted);
    font-size: 9px;
    white-space: nowrap;
  }

  .plan-empty,
  .plan-degraded {
    margin: 0;
    color: var(--pv-muted);
    line-height: 1.45;
  }

  .plan-degraded {
    margin-top: 6px;
    color: #e6ba7a;
  }

  @keyframes pv-plan-pulse {
    0%, 100% { opacity: 0.55; }
    50% { opacity: 1; }
  }

  @media (prefers-reduced-motion: reduce) {
    .plan-activity[data-status='running'] .plan-status-icon {
      animation: none;
    }
  }
`;
