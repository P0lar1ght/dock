import { css } from 'lit';

export const composerStyles = css`
  .composer-shell {
    position: relative;
    padding: 8px 10px 10px;
    border-top: 1px solid var(--pv-border);
  }

  .active-send-modes {
    display: flex;
    align-items: center;
    gap: 4px;
    margin: 0 4px 6px;
    color: var(--pv-muted);
    font-size: 10px;
  }

  .active-send-modes > span { margin-right: auto; }

  .active-send-modes button {
    min-height: 24px;
    padding: 2px 8px;
    border: 1px solid var(--pv-border);
    border-radius: 7px;
    background: transparent;
    color: var(--pv-muted);
    cursor: pointer;
    font: inherit;
    font-weight: 700;
  }

  .active-send-modes button[aria-pressed="true"] {
    border-color: color-mix(in srgb, var(--pv-accent) 52%, var(--pv-border));
    background: color-mix(in srgb, var(--pv-accent) 14%, transparent);
    color: var(--pv-accent);
  }

  .composer {
    position: relative;
    z-index: 7;
    display: grid;
    gap: 5px;
    padding: 11px 10px 8px;
    border: 1px solid var(--pv-border);
    border-radius: 22px;
    background: var(--pv-panel-raised);
    box-shadow: inset 0 1px rgba(255, 255, 255, 0.025);
  }

  .composer:focus-within {
    border-color: color-mix(in srgb, var(--pv-accent) 58%, var(--pv-border));
  }

  textarea {
    box-sizing: border-box;
    width: 100%;
    max-height: 112px;
    min-height: 42px;
    padding: 0 4px;
    resize: none;
    overflow-y: auto;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--pv-text);
    font: inherit;
    font-size: 13px;
    line-height: 1.5;
  }

  textarea::placeholder { color: var(--pv-muted); }

  .composer-toolbar,
  .composer-toolbar-start,
  .composer-toolbar-end {
    display: flex;
    align-items: center;
  }

  .composer-toolbar {
    min-width: 0;
    justify-content: space-between;
    gap: 8px;
  }

  .composer-toolbar-start,
  .composer-toolbar-end { gap: 3px; }
  .composer-toolbar-start { min-width: 0; }

  .composer-control,
  .send-button {
    display: inline-flex;
    min-width: 32px;
    height: 32px;
    align-items: center;
    justify-content: center;
    padding: 0 8px;
    border: 0;
    border-radius: 999px;
    background: transparent;
    color: var(--pv-text);
    cursor: pointer;
    font: inherit;
    font-size: 11px;
    white-space: nowrap;
  }

  .composer-control:hover { background: color-mix(in srgb, var(--pv-text) 8%, transparent); }

  .composer-control[aria-disabled="true"] {
    color: var(--pv-muted);
    cursor: default;
  }

  .composer-control:disabled {
    color: var(--pv-muted);
    cursor: not-allowed;
    opacity: 0.55;
  }

  .approval-control { width: 32px; padding: 0; }
  .model-control { max-width: 150px; gap: 5px; }
  .model-control span {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .control-icon {
    width: 14px;
    height: 14px;
    flex: 0 0 auto;
  }

  .control-chevron { width: 9px; height: 9px; }

  .send-button {
    width: 34px;
    height: 34px;
    padding: 0;
    background: var(--pv-text);
    color: var(--pv-panel);
  }

  .send-button .control-icon { width: 15px; height: 15px; }

  .send-button:disabled,
  textarea:disabled {
    cursor: not-allowed;
    opacity: 0.42;
  }

  .composer-control:focus-visible,
  .send-button:focus-visible,
  .active-send-modes button:focus-visible {
    outline: 2px solid var(--pv-accent);
    outline-offset: 2px;
  }

  .composer-hint,
  .composer-error {
    margin: 5px 5px 0;
    font-size: 10px;
  }

  .composer-hint { color: var(--pv-muted); }
  .composer-error {
    max-height: 4.8em;
    overflow: auto;
    color: var(--pv-danger);
    white-space: pre-wrap;
  }

  .composer-popover-scrim {
    position: fixed;
    inset: 0;
    z-index: 5;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: default;
  }

  .context-popover {
    position: absolute;
    right: 10px;
    bottom: calc(100% - 2px);
    left: 10px;
    z-index: 8;
    padding: 14px;
    border: 1px solid var(--pv-border);
    border-radius: 17px;
    background: color-mix(in srgb, var(--pv-panel) 97%, transparent);
    box-shadow: var(--pv-shadow);
    max-height: min(480px, calc(100dvh - 180px));
    overflow-x: hidden;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-width: thin;
  }

  .context-popover-heading {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 11px;
  }

  .context-popover-heading strong { font-size: 13px; }
  .context-popover-heading span { color: var(--pv-muted); font-size: 11px; }

  .context-meter {
    position: relative;
    height: 18px;
    overflow: hidden;
    border: 1px solid var(--pv-border);
    border-radius: 999px;
    background: color-mix(in srgb, var(--pv-panel-raised) 74%, black);
  }

  .context-meter-fill {
    position: absolute;
    inset: 0 auto 0 0;
    border-radius: inherit;
    background: linear-gradient(90deg, var(--pv-accent-strong), var(--pv-accent));
  }

  .context-meter-trigger {
    position: absolute;
    top: -1px;
    bottom: -1px;
    width: 2px;
    background: var(--pv-warning);
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--pv-panel) 55%, transparent);
  }

  .context-meter-value {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--pv-text);
    font-size: 9px;
    font-weight: 800;
    text-shadow: 0 1px 2px rgba(0, 0, 0, 0.6);
  }

  .context-meter-labels {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    gap: 6px;
    margin-top: 6px;
    color: var(--pv-muted);
    font-size: 9px;
  }

  .context-meter-labels span:nth-child(2) { color: var(--pv-warning); text-align: center; }
  .context-meter-labels span:last-child { text-align: right; }

  .context-facts {
    display: grid;
    gap: 6px;
    margin: 12px 0 0;
    padding-top: 10px;
    border-top: 1px solid var(--pv-border);
  }

  .context-facts div { display: flex; justify-content: space-between; gap: 12px; }
  .context-facts dt,
  .context-facts dd { margin: 0; font-size: 10px; }
  .context-facts dt { color: var(--pv-muted); }

  .context-compaction,
  .context-empty {
    margin: 10px 0 0;
    color: var(--pv-muted);
    font-size: 10px;
  }

  .context-compaction[data-status="running"] { color: var(--pv-accent); }
  .context-compaction[data-status="completed"] { color: var(--pv-success); }
  .context-compaction[data-status="failed"] { color: var(--pv-danger); }

  .composer-menu-row,
  .model-option {
    display: flex;
    width: 100%;
    align-items: center;
    gap: 10px;
    padding: 9px 8px;
    border: 0;
    border-radius: 10px;
    background: transparent;
    color: var(--pv-text);
    cursor: pointer;
    font: inherit;
    text-align: left;
  }

  .composer-menu-row {
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid var(--pv-border);
    border-radius: 0;
  }

  .composer-menu-row:hover,
  .model-option:hover { background: color-mix(in srgb, var(--pv-text) 7%, transparent); }

  .composer-menu-row:disabled { opacity: 0.5; cursor: not-allowed; }
  .composer-menu-row:disabled:hover { background: transparent; }

  .composer-menu-row .control-icon,
  .model-option .control-icon { width: 13px; height: 13px; flex: 0 0 auto; }

  .composer-menu-row > span,
  .model-option > span { display: grid; min-width: 0; gap: 2px; flex: 1; }

  .composer-menu-row strong,
  .model-option strong { font-size: 11px; }

  .composer-menu-row small,
  .model-option small {
    overflow: hidden;
    color: var(--pv-muted);
    font-size: 9px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .environment-detail > summary { list-style: none; }
  .environment-detail > summary::-webkit-details-marker { display: none; }
  .environment-detail[open] .detail-chevron { transform: rotate(90deg); }
  .detail-chevron { transition: transform 120ms ease; }

  .environment-detail-list {
    display: grid;
    gap: 3px;
    margin: 0 5px 2px 28px;
    padding: 2px 0 4px 9px;
    border-left: 1px solid var(--pv-border);
  }

  .environment-detail-item {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 8px;
    padding: 5px 3px;
  }

  .environment-detail-item > span { display: grid; min-width: 0; gap: 1px; }
  .environment-detail-item strong { overflow: hidden; font-size: 10px; text-overflow: ellipsis; white-space: nowrap; }
  .environment-detail-item small { color: var(--pv-muted); font-size: 8px; }
  .environment-status { width: 7px; height: 7px; border-radius: 50%; background: var(--pv-muted); flex: 0 0 auto; }
  .environment-status[data-status="connected"] { background: var(--pv-success); }
  .environment-status[data-status="unavailable"] { background: var(--pv-danger); }
  .environment-order {
    display: grid;
    width: 17px;
    height: 17px;
    place-items: center;
    border: 1px solid var(--pv-border);
    border-radius: 50%;
    color: var(--pv-muted);
    font-size: 8px;
    flex: 0 0 auto;
  }
  .environment-detail-note { margin: 3px 0; color: var(--pv-muted); font-size: 8px; }

  .model-options { display: grid; gap: 2px; }

  .model-popover {
    display: grid;
    max-height: min(440px, calc(100dvh - 150px));
    grid-template-rows: auto minmax(0, 1fr) auto auto;
    overflow: hidden;
  }

  .model-popover .model-options {
    min-height: 0;
    overflow-x: hidden;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding-right: 3px;
    scrollbar-gutter: stable;
    scrollbar-width: thin;
  }

  .model-popover .model-refresh { margin-top: 8px; }

  .model-option[aria-checked="true"] {
    background: color-mix(in srgb, var(--pv-accent) 12%, transparent);
  }

  .model-option:disabled { cursor: wait; opacity: 0.62; }

  .policy-popover { padding: 11px; }
  .policy-popover .model-option { min-height: 46px; }
  .approval-option small {
    overflow: visible;
    line-height: 1.35;
    text-overflow: clip;
    white-space: normal;
  }
  .approval-option:disabled { cursor: not-allowed; }

  .goal-popover { padding: 12px; }

  .goal-summary {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin: 0 0 11px;
    padding-bottom: 10px;
    border-bottom: 1px solid var(--pv-border);
    color: var(--pv-text);
    font-size: 10px;
    line-height: 1.45;
  }

  .goal-summary .control-icon {
    width: 13px;
    height: 13px;
    margin-top: 1px;
    color: var(--pv-accent);
    flex: 0 0 auto;
  }

  .goal-summary[data-status="completed"] { color: var(--pv-muted); }

  .goal-label {
    display: block;
    margin-bottom: 6px;
    color: var(--pv-muted);
    font-size: 9px;
  }

  .goal-input {
    width: 100%;
    min-height: 66px;
    max-height: 116px;
    padding: 8px 9px;
    resize: vertical;
    border: 1px solid var(--pv-border);
    border-radius: 10px;
    outline: 0;
    background: var(--pv-panel-raised);
    color: var(--pv-text);
    font-size: 11px;
    line-height: 1.45;
  }

  .goal-input:focus { border-color: color-mix(in srgb, var(--pv-accent) 58%, var(--pv-border)); }

  .goal-actions {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-top: 8px;
  }

  .goal-action {
    display: inline-flex;
    height: 29px;
    align-items: center;
    justify-content: center;
    gap: 5px;
    padding: 0 9px;
    border: 1px solid var(--pv-border);
    border-radius: 999px;
    background: transparent;
    color: var(--pv-muted);
    cursor: pointer;
    font: inherit;
    font-size: 9px;
    font-weight: 700;
  }

  .goal-action .control-icon { width: 10px; height: 10px; }
  .goal-action:hover { background: color-mix(in srgb, var(--pv-text) 7%, transparent); }
  .goal-action:disabled { cursor: wait; opacity: 0.5; }
  .goal-save { border-color: color-mix(in srgb, var(--pv-accent) 45%, var(--pv-border)); color: var(--pv-accent); }
  .goal-clear { margin-left: auto; color: var(--pv-danger); }

  .goal-note {
    margin: 9px 0 0;
    color: var(--pv-muted);
    font-size: 8px;
    line-height: 1.35;
  }

  @media (max-height: 520px) {
    .composer-hint { display: none; }
    .context-popover { padding: 11px; }
  }
`;
