import { css } from 'lit';

export const resetStyles = css`
  :host,
  :host *,
  :host *::before,
  :host *::after {
    box-sizing: border-box;
  }

  :host * {
    font-family: var(--pv-font, ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif);
  }

  button,
  input,
  select {
    color: inherit;
    font: inherit;
  }

  button {
    border: 0;
  }

  [hidden] {
    display: none !important;
  }
`;
