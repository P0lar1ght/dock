import { css } from 'lit';

/** Styles for the coding-agent style slash-command completion surface. */
export const slashCommandStyles = css`
  .slash-command-menu {
    position: absolute;
    right: 10px;
    bottom: calc(100% - 2px);
    left: 10px;
    z-index: 9;
    padding: 7px;
    border: 1px solid var(--pv-border);
    border-radius: 14px;
    background: color-mix(in srgb, var(--pv-panel) 97%, transparent);
    box-shadow: var(--pv-shadow);
  }

  .slash-command-menu button {
    display: grid;
    width: 100%;
    grid-template-columns: auto 1fr;
    gap: 12px;
    align-items: center;
    padding: 9px 10px;
    border: 0;
    border-radius: 9px;
    background: transparent;
    color: var(--pv-text);
    cursor: pointer;
    font: inherit;
    text-align: left;
  }

  .slash-command-menu button[aria-selected="true"] {
    background: color-mix(in srgb, var(--pv-accent) 14%, transparent);
  }

  .slash-command-menu button:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .slash-command-menu span { display: grid; gap: 2px; }
  .slash-command-menu strong { color: var(--pv-accent); font-size: 11px; }
  .slash-command-menu small,
  .slash-command-menu em,
  .slash-command-menu p { color: var(--pv-muted); font-size: 9px; }
  .slash-command-menu em {
    overflow: hidden;
    font-style: normal;
    text-align: right;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .slash-command-menu p { margin: 5px 7px 1px; }
`;
