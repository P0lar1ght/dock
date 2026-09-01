import { css } from 'lit';

export const toolActivityStyles = css`
  @keyframes pv-tool-pulse {
    0%, 100% { opacity: 0.45; }
    50% { opacity: 1; }
  }

  .tool-card {
    width: 100%;
    color: var(--pv-muted);
    font: 12px/1.35 var(--pv-font);
  }

  .tool-card > summary {
    display: flex;
    min-height: 0;
    align-items: baseline;
    gap: 6px;
    padding: 0;
    cursor: pointer;
    list-style: none;
  }

  .tool-card > summary::-webkit-details-marker,
  .tool-output-more > summary::-webkit-details-marker {
    display: none;
  }

  .tool-card > summary:hover,
  .tool-card > summary:focus-visible {
    outline: none;
    color: var(--pv-text);
  }

  .tool-card-diamond,
  .tool-card-prompt {
    flex: 0 0 auto;
    color: var(--pv-accent);
    font-size: 11px;
    line-height: 1.35;
  }

  .tool-card-prompt {
    font-weight: 700;
  }

  .tool-card[data-status="running"] .tool-card-diamond,
  .tool-card[data-status="running"] .tool-card-prompt {
    animation: pv-tool-pulse 900ms ease-in-out infinite;
  }

  .tool-card[data-status="failed"] .tool-card-diamond,
  .tool-card[data-status="failed"] .tool-card-prompt {
    color: var(--pv-danger);
  }

  .tool-card[data-status="cancelled"] .tool-card-diamond,
  .tool-card[data-status="completed"]:not([open]) .tool-card-name,
  .tool-card[data-status="cancelled"]:not([open]) .tool-card-name {
    color: var(--pv-muted);
  }

  .tool-card-name {
    flex: 0 1 auto;
    overflow: hidden;
    color: var(--pv-text);
    font-weight: 700;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tool-card-summary,
  .tool-card-live {
    min-width: 0;
    overflow: hidden;
    color: var(--pv-muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tool-card-live {
    flex: 0 0 auto;
    font-size: 11px;
  }

  .tool-card-body {
    margin: 2px 0 4px 17px;
    color: var(--pv-muted);
  }

  .tool-card-k {
    margin: 4px 0 1px;
    color: var(--pv-muted);
    font-size: 11px;
    opacity: 0.72;
  }

  .tool-card-pre,
  .tool-card-body p {
    margin: 0;
    overflow: auto;
    overflow-wrap: anywhere;
    max-height: 9.5em;
    white-space: pre-wrap;
    font: 11px/1.4 var(--pv-font);
    scrollbar-width: thin;
  }

  .tool-card-live-body {
    margin-top: 2px;
    color: var(--pv-muted);
  }

  .tool-output-more {
    margin-top: 2px;
  }

  .tool-output-more > summary {
    padding: 0;
    color: var(--pv-muted);
    cursor: pointer;
    list-style: none;
    font-size: 11px;
  }

  .tool-output-more > pre {
    max-height: 14em;
    margin: 2px 0 0;
    overflow: auto;
    white-space: pre-wrap;
    font: 11px/1.4 var(--pv-font);
    scrollbar-width: thin;
  }

  @media (prefers-reduced-motion: reduce) {
    .tool-card-diamond,
    .tool-card-prompt {
      animation: none !important;
    }
  }
`;
