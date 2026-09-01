import { css } from 'lit';

export const themeStyles = css`
  :host([theme="light"]) {
    --pv-panel: rgba(246, 251, 255, 0.97);
    --pv-panel-raised: #e9f4fb;
    --pv-border: rgba(19, 73, 105, 0.18);
    --pv-text: #10263a;
    --pv-muted: #567086;
    --pv-shadow: 0 22px 55px rgba(20, 68, 96, 0.22), 0 4px 14px rgba(20, 68, 96, 0.16);
  }

  @media (prefers-color-scheme: light) {
    :host(:not([theme="dark"])) {
      --pv-panel: rgba(246, 251, 255, 0.97);
      --pv-panel-raised: #e9f4fb;
      --pv-border: rgba(19, 73, 105, 0.18);
      --pv-text: #10263a;
      --pv-muted: #567086;
      --pv-shadow: 0 22px 55px rgba(20, 68, 96, 0.22), 0 4px 14px rgba(20, 68, 96, 0.16);
    }
  }
`;
