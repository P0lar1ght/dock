import { css } from 'lit';

export const commandOutputStyles = css`
  .command-output {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    min-height: 0;
    max-height: min(46vh, 380px);
    margin: 8px 10px 0;
    overflow: hidden;
    border: 1px solid color-mix(in srgb, var(--pv-accent) 28%, var(--pv-border));
    border-radius: 14px;
    background:
      linear-gradient(180deg, rgba(115, 219, 255, 0.07), transparent 42px),
      color-mix(in srgb, var(--pv-panel-raised) 92%, #071018);
    color: var(--pv-text);
    box-shadow: inset 0 1px rgba(255, 255, 255, 0.04);
  }

  .command-output[data-kind="applied"] {
    max-height: none;
  }

  .command-output-header {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 8px;
    padding: 8px 8px 8px 12px;
    border-bottom: 1px solid var(--pv-border);
  }

  .command-output[data-kind="applied"] .command-output-header {
    border-bottom: 0;
    padding-bottom: 2px;
  }

  .command-output-diamond {
    flex: 0 0 auto;
    color: var(--pv-accent);
    font-size: 11px;
    line-height: 1;
  }

  .command-output-kicker {
    flex: 0 0 auto;
    color: var(--pv-accent);
    font-size: 10px;
    font-weight: 760;
    letter-spacing: 0.06em;
  }

  .command-output-title {
    min-width: 0;
    flex: 1;
    margin: 0;
    overflow: hidden;
    color: var(--pv-text);
    font-size: 13px;
    font-weight: 760;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .command-output-close {
    display: grid;
    width: 28px;
    height: 28px;
    flex: 0 0 28px;
    margin-left: auto;
    place-items: center;
    padding: 0;
    border: 1px solid transparent;
    border-radius: 8px;
    background: transparent;
    color: var(--pv-muted);
    cursor: pointer;
    font: 18px/1 var(--pv-font);
  }

  .command-output-close:hover,
  .command-output-close:focus-visible {
    border-color: var(--pv-border);
    color: var(--pv-text);
  }

  .command-output-close:focus-visible {
    outline: 2px solid var(--pv-accent);
    outline-offset: 1px;
  }

  .command-output-body {
    min-height: 0;
    overflow: auto;
    padding: 4px 14px 14px;
    color: var(--pv-text);
    font-size: 13px;
    line-height: 1.55;
    overscroll-behavior: contain;
    scrollbar-width: thin;
  }

  .command-output[data-kind="applied"] .command-output-body {
    padding-top: 0;
    padding-bottom: 12px;
  }

  .command-output-body .markdown-message {
    color: var(--pv-text);
  }

  .command-output-body .markdown-message h1,
  .command-output-body .markdown-message h2 {
    padding-bottom: 4px;
    border-bottom: 1px solid var(--pv-border);
  }

  .command-output-body .markdown-message h2,
  .command-output-body .markdown-message h3 {
    margin-top: 4px;
  }

  .command-output-body .markdown-code-block {
    max-height: 260px;
    color: var(--pv-text);
  }

  .command-output-plain {
    margin: 0;
    color: var(--pv-text);
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    font-size: 12px;
    line-height: 1.55;
    tab-size: 4;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .command-output + .connection-status,
  .command-output + .turn-controls,
  .command-output + .turn-queue,
  .command-output + .operation-issue,
  .command-output + .composer-shell {
    border-top: 0;
  }
`;
