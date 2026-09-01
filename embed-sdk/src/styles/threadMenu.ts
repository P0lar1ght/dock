import { css } from 'lit';

export const threadMenuStyles = css`
  .thread-menu {
    position: relative;
    flex: 0 0 auto;
  }

  .thread-menu summary {
    list-style: none;
  }

  .thread-menu summary::-webkit-details-marker {
    display: none;
  }

  .thread-menu-toggle {
    position: relative;
    font-size: 14px;
    font-weight: 800;
  }

  .thread-menu-count {
    position: absolute;
    top: -5px;
    right: -5px;
    display: grid;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    place-items: center;
    border: 2px solid var(--pv-panel);
    border-radius: 999px;
    background: var(--pv-danger);
    color: white;
    font-size: 9px;
    line-height: 1;
  }

  .thread-menu-popover {
    position: absolute;
    top: 40px;
    right: -74px;
    z-index: 5;
    width: min(318px, calc(100vw - 42px));
    overflow: hidden;
    border: 1px solid var(--pv-border);
    border-radius: 13px;
    background: var(--pv-panel);
    box-shadow: var(--pv-shadow);
  }

  .thread-menu-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 9px 10px;
    border-bottom: 1px solid var(--pv-border);
    font-size: 12px;
  }

  .thread-new-button,
  .thread-switch,
  .thread-rename,
  .thread-archive,
  .thread-action {
    border: 0;
    background: transparent;
    color: var(--pv-text);
    cursor: pointer;
    font: inherit;
  }

  .thread-new-button {
    padding: 5px 7px;
    border-radius: 7px;
    background: color-mix(in srgb, var(--pv-accent) 14%, transparent);
    color: var(--pv-accent);
    font-size: 11px;
    font-weight: 750;
  }

  .thread-list {
    max-height: 260px;
    overflow-y: auto;
    padding: 4px;
  }

  .thread-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    min-height: 43px;
    border-radius: 9px;
  }

  .thread-row:hover,
  .thread-row[data-active="true"] {
    background: var(--pv-panel-raised);
  }

  .thread-switch {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 8px;
    padding: 7px 5px 7px 8px;
    text-align: left;
  }

  .thread-state {
    width: 7px;
    height: 7px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: var(--pv-muted);
  }

  .thread-state[data-activity="thinking"],
  .thread-state[data-activity="streaming"],
  .thread-state[data-activity="working"],
  .thread-state[data-activity="subagent"] {
    background: var(--pv-accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--pv-accent) 16%, transparent);
  }

  .thread-state[data-attention="approval"] {
    background: var(--pv-warning);
  }

  .thread-state[data-attention="error"] {
    background: var(--pv-danger);
  }

  .thread-state[data-attention="completed"] {
    background: var(--pv-success);
  }

  .thread-copy {
    display: grid;
    min-width: 0;
  }

  .thread-archived-section {
    margin-top: 4px;
    border-top: 1px solid var(--pv-border);
  }

  .thread-archived-section > summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px;
    color: var(--pv-muted);
    cursor: pointer;
    font-size: 10px;
    font-weight: 700;
  }

  .thread-archived-section > summary::marker {
    color: var(--pv-muted);
  }

  .thread-row-archived {
    min-height: 39px;
  }

  .thread-archived-copy {
    display: grid;
    min-width: 0;
    padding: 6px 5px 6px 8px;
    opacity: 0.76;
  }

  .thread-row-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    padding-right: 4px;
  }

  .thread-rename {
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 6px;
    color: var(--pv-muted);
    font-size: 13px;
  }

  .thread-rename:hover:not(:disabled) {
    background: var(--pv-panel-raised);
    color: var(--pv-accent);
  }

  .thread-rename-editor {
    display: grid;
    grid-column: 1 / -1;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 3px;
    padding: 5px;
  }

  .thread-rename-editor input {
    width: 100%;
    min-width: 0;
    height: 29px;
    padding: 5px 7px;
    border: 1px solid var(--pv-accent);
    border-radius: 7px;
    outline: 0;
    background: var(--pv-panel);
    color: var(--pv-text);
    font: inherit;
    font-size: 10px;
  }

  .thread-rename-confirm {
    background: color-mix(in srgb, var(--pv-accent) 12%, transparent);
    color: var(--pv-accent);
    font-weight: 750;
  }

  .thread-action {
    padding: 5px 4px;
    border-radius: 6px;
    color: var(--pv-muted);
    font-size: 8px;
    white-space: nowrap;
  }

  .thread-action:hover:not(:disabled) {
    background: var(--pv-panel-raised);
    color: var(--pv-text);
  }

  .thread-delete:hover:not(:disabled),
  .thread-delete-confirm {
    color: var(--pv-danger);
  }

  .thread-delete-confirm {
    background: color-mix(in srgb, var(--pv-danger) 12%, transparent);
    font-weight: 750;
  }

  .thread-title,
  .thread-meta {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .thread-title {
    font-size: 11px;
    font-weight: 700;
  }

  .thread-meta {
    color: var(--pv-muted);
    font-size: 9px;
  }

  .thread-archive {
    margin-right: 5px;
    padding: 5px;
    border-radius: 6px;
    color: var(--pv-muted);
    font-size: 9px;
  }

  .thread-archive:hover:not(:disabled) {
    background: color-mix(in srgb, var(--pv-danger) 12%, transparent);
    color: var(--pv-danger);
  }

  .thread-new-button:disabled,
  .thread-rename:disabled,
  .thread-archive:disabled,
  .thread-action:disabled {
    cursor: not-allowed;
    opacity: 0.42;
  }

  .thread-empty,
  .thread-menu-error {
    margin: 0;
    padding: 11px;
    color: var(--pv-muted);
    font-size: 10px;
  }

  .thread-menu-error {
    border-top: 1px solid var(--pv-border);
    color: var(--pv-danger);
  }
`;
