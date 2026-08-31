import { css } from 'lit';

export const tokenStyles = css`
  :host {
    --pv-font: Inter, ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    --pv-panel: rgba(9, 18, 31, 0.96);
    --pv-panel-raised: #132238;
    --pv-border: rgba(158, 211, 255, 0.22);
    --pv-text: #f6fbff;
    --pv-muted: #a9bfd0;
    --pv-accent: #73dbff;
    --pv-accent-strong: #2bb7ec;
    --pv-danger: #ff7f8f;
    --pv-warning: #f6c65b;
    --pv-success: #74e6b0;
    --pv-shadow: 0 22px 55px rgba(0, 8, 19, 0.42), 0 4px 14px rgba(0, 8, 19, 0.26);
    --pv-radius: 20px;
    --pv-pet-width: 112px;
    --pv-pet-height: 122px;
    --pv-chat-min-width: 380px;
    --pv-chat-fluid-width: 52vw;
    --pv-chat-max-width: 720px;
  }
`;
