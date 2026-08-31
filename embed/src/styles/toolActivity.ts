import { css } from 'lit';

export const toolActivityStyles = css`
  @keyframes pv-tool-pulse {
    0%, 100% { opacity: 0.4; transform: scale(0.78); }
    50% { opacity: 1; transform: scale(1); }
  }

  .tool-activity {
    width: 100%;
    color: var(--pv-muted);
    font-size: 11px;
  }

  .tool-activity > summary {
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

  .tool-activity > summary::-webkit-details-marker,
  .tool-output-more > summary::-webkit-details-marker {
    display: none;
  }

  .tool-activity > summary:hover,
  .tool-activity > summary:focus-visible,
  .tool-activity[open] > summary {
    outline: none;
    background: color-mix(in srgb, var(--pv-panel-raised) 72%, transparent);
    color: var(--pv-text);
  }

  .tool-status-icon {
    display: grid;
    width: 14px;
    height: 14px;
    place-items: center;
    border-radius: 50%;
    color: var(--pv-success);
    font-size: 12px;
    font-weight: 850;
  }

  .tool-activity[data-status="running"] .tool-status-icon {
    color: var(--pv-accent);
    animation: pv-tool-pulse 900ms ease-in-out infinite;
  }

  .tool-activity[data-status="failed"] .tool-status-icon {
    color: var(--pv-danger);
  }

  .tool-row-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tool-duration {
    color: var(--pv-muted);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
  }

  .tool-chevron {
    color: var(--pv-muted);
    font-size: 16px;
    transform: rotate(0deg);
    transition: transform 120ms ease;
  }

  .tool-activity[open] .tool-chevron {
    transform: rotate(90deg);
  }

  .tool-details {
    max-height: min(42vh, 320px);
    margin: 2px 4px 5px 20px;
    padding: 7px 9px;
    overflow: auto;
    border-left: 1px solid var(--pv-border);
    color: var(--pv-muted);
    scrollbar-width: thin;
  }

  .tool-detail-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin-bottom: 6px;
  }

  .tool-detail-meta span {
    padding: 2px 5px;
    border-radius: 5px;
    background: var(--pv-panel-raised);
    font-size: 9px;
  }

  .tool-details section + section {
    margin-top: 8px;
  }

  .tool-details h4,
  .tool-details p,
  .tool-details pre {
    margin: 0;
  }

  .tool-details h4 {
    margin-bottom: 3px;
    color: var(--pv-text);
    font-size: 10px;
    font-weight: 650;
  }

  .tool-details pre,
  .tool-details p {
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    font: 10px/1.45 var(--pv-font);
  }

  .tool-output-section > pre {
    max-height: 110px;
    overflow: auto;
    scrollbar-width: thin;
  }

  .tool-output-more {
    margin-top: 6px;
    border-top: 1px solid var(--pv-border);
  }

  .tool-output-more > summary {
    display: flex;
    min-height: 26px;
    align-items: center;
    justify-content: space-between;
    padding: 2px 0;
    color: var(--pv-accent);
    cursor: pointer;
    list-style: none;
    font: 10px/1.4 var(--pv-font);
  }

  .tool-output-more > pre {
    max-height: 180px;
    padding-top: 5px;
    overflow: auto;
    border-top: 1px dashed var(--pv-border);
    scrollbar-width: thin;
  }

  @media (prefers-reduced-motion: reduce) {
    .tool-status-icon,
    .tool-chevron {
      animation: none !important;
      transition: none !important;
    }
  }
`;
