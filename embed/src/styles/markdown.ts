import { css } from 'lit';

export const markdownStyles = css`
  .markdown-message {
    display: grid;
    min-width: 0;
    gap: 8px;
    white-space: normal;
  }

  .markdown-message > * {
    min-width: 0;
    margin: 0;
  }

  .markdown-message h1,
  .markdown-message h2,
  .markdown-message h3,
  .markdown-message h4,
  .markdown-message h5,
  .markdown-message h6 {
    color: var(--pv-text);
    font-weight: 780;
    line-height: 1.3;
  }

  .markdown-message h1 { font-size: 17px; }
  .markdown-message h2 { font-size: 16px; }
  .markdown-message h3 { font-size: 15px; }
  .markdown-message h4,
  .markdown-message h5,
  .markdown-message h6 { font-size: 14px; }

  .markdown-message p,
  .markdown-message li,
  .markdown-message blockquote {
    overflow-wrap: anywhere;
  }

  .markdown-message ul,
  .markdown-message ol {
    display: grid;
    gap: 3px;
    padding-left: 20px;
  }

  .markdown-message li > .markdown-list-content,
  .markdown-message blockquote {
    display: grid;
    min-width: 0;
    gap: 4px;
  }

  .markdown-message li > .markdown-list-content > *,
  .markdown-message blockquote > * {
    margin: 0;
  }

  .markdown-message .markdown-task-item {
    display: grid;
    grid-template-columns: 15px minmax(0, 1fr);
    gap: 6px;
    margin-left: -20px;
    list-style: none;
  }

  .markdown-task-checkbox {
    width: 13px;
    height: 13px;
    margin: 3px 0 0;
    accent-color: var(--pv-accent);
    opacity: 1;
  }

  .markdown-message blockquote {
    padding: 2px 0 2px 10px;
    border-left: 3px solid rgba(115, 219, 255, 0.52);
    color: var(--pv-muted);
  }

  .markdown-message strong { font-weight: 780; }

  .markdown-message del {
    color: var(--pv-muted);
    text-decoration-thickness: 1px;
  }

  .markdown-message hr {
    width: 100%;
    border: 0;
    border-top: 1px solid var(--pv-border);
  }

  .markdown-message a {
    color: var(--pv-accent);
    text-decoration: underline;
    text-decoration-thickness: 1px;
    text-underline-offset: 2px;
  }

  .markdown-message a:focus-visible {
    border-radius: 3px;
    outline: 2px solid var(--pv-accent);
    outline-offset: 2px;
  }

  .markdown-table-scroll {
    max-width: 100%;
    overflow-x: auto;
    border: 1px solid var(--pv-border);
    border-radius: 8px;
    overscroll-behavior: contain;
  }

  .markdown-table-scroll:focus-visible {
    outline: 2px solid var(--pv-accent);
    outline-offset: 2px;
  }

  .markdown-message table {
    width: 100%;
    min-width: max-content;
    border-collapse: collapse;
    font-size: 0.94em;
  }

  .markdown-message th,
  .markdown-message td {
    max-width: 280px;
    padding: 6px 8px;
    border-right: 1px solid var(--pv-border);
    border-bottom: 1px solid var(--pv-border);
    overflow-wrap: anywhere;
    text-align: left;
    vertical-align: top;
  }

  .markdown-message th {
    background: rgba(115, 219, 255, 0.08);
    font-weight: 760;
  }

  .markdown-message tr:last-child td { border-bottom: 0; }
  .markdown-message th:last-child,
  .markdown-message td:last-child { border-right: 0; }
  .markdown-message [data-align="center"] { text-align: center; }
  .markdown-message [data-align="right"] { text-align: right; }

  .markdown-inline-code,
  .markdown-code-block {
    border: 1px solid var(--pv-border);
    background: rgba(4, 13, 22, 0.48);
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    font-size: 0.9em;
  }

  .markdown-inline-code {
    padding: 1px 4px;
    border-radius: 5px;
    white-space: break-spaces;
  }

  .markdown-code-block {
    max-width: 100%;
    max-height: 220px;
    overflow: auto;
    padding: 10px;
    border-radius: 9px;
    line-height: 1.45;
    overscroll-behavior: contain;
    white-space: pre;
  }

  .markdown-code-block code {
    display: block;
    min-width: max-content;
    color: inherit;
    font: inherit;
  }
`;
