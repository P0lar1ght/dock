import { css } from 'lit';

export const subagentActivityStyles = css`
  @keyframes pv-subagent-pulse {
    0%, 100% { opacity: 0.45; transform: scale(0.8); }
    50% { opacity: 1; transform: scale(1); }
  }

  .subagent-activity {
    width: 100%;
    color: var(--pv-muted);
    font-size: 11px;
  }

  .subagent-activity > summary {
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr) auto 14px;
    min-height: 30px;
    align-items: center;
    gap: 6px;
    padding: 1px 4px;
    border-radius: 8px;
    cursor: pointer;
    list-style: none;
    transition: background 120ms ease, color 120ms ease;
  }

  .subagent-activity > summary::-webkit-details-marker {
    display: none;
  }

  .subagent-activity > summary:hover,
  .subagent-activity > summary:focus-visible,
  .subagent-activity[open] > summary {
    outline: none;
    background: color-mix(in srgb, var(--pv-panel-raised) 72%, transparent);
    color: var(--pv-text);
  }

  .subagent-status-icon {
    display: grid;
    width: 14px;
    height: 14px;
    place-items: center;
    border: 1px solid rgba(115, 219, 255, 0.34);
    border-radius: 50%;
    color: var(--pv-success);
    font-size: 10px;
    font-weight: 850;
  }

  .subagent-activity[data-status='running'] .subagent-status-icon,
  .subagent-activity[data-status='background'] .subagent-status-icon {
    color: var(--pv-accent);
    animation: pv-subagent-pulse 1s ease-in-out infinite;
  }

  .subagent-activity[data-status='waiting_permission'] .subagent-status-icon {
    border-color: rgba(255, 189, 85, 0.5);
    color: #f3bd69;
  }

  .subagent-activity[data-status='failed'] .subagent-status-icon {
    border-color: rgba(255, 127, 143, 0.5);
    color: var(--pv-danger);
  }

  .subagent-activity[data-status='cancelled'] .subagent-status-icon {
    color: var(--pv-muted);
  }

  .subagent-row-copy {
    display: flex;
    min-width: 0;
    align-items: baseline;
    gap: 6px;
  }

  .subagent-row-title {
    min-width: 0;
    overflow: hidden;
    color: var(--pv-text-soft);
    font-weight: 650;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .subagent-row-type,
  .subagent-row-status {
    color: var(--pv-muted);
    font-size: 9px;
    white-space: nowrap;
  }

  .subagent-row-type::before {
    content: '· ';
  }

  .subagent-chevron {
    color: var(--pv-muted);
    font-size: 16px;
    line-height: 1;
    transition: transform 120ms ease;
  }

  .subagent-activity[open] .subagent-chevron {
    transform: rotate(90deg);
  }

  .subagent-details {
    max-height: 170px;
    margin: 2px 4px 5px 20px;
    padding: 7px 9px;
    overflow: auto;
    border-left: 1px solid var(--pv-border);
    color: var(--pv-muted);
    scrollbar-width: thin;
  }

  .subagent-detail-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin-bottom: 6px;
  }

  .subagent-detail-meta span {
    padding: 2px 5px;
    border-radius: 5px;
    background: var(--pv-panel-raised);
    font-size: 9px;
  }

  .subagent-details section + section {
    margin-top: 8px;
  }

  .subagent-details h4,
  .subagent-details p {
    margin: 0;
  }

  .subagent-details h4 {
    margin-bottom: 3px;
    color: var(--pv-text);
    font-size: 10px;
  }

  .subagent-details p {
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    font: 10px/1.45 var(--pv-font);
  }

  @media (prefers-reduced-motion: reduce) {
    .subagent-status-icon,
    .subagent-chevron {
      animation: none !important;
      transition: none !important;
    }
  }
`;
