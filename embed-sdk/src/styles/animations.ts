import { css } from 'lit';

export const animationStyles = css`
  @keyframes pv-status-pulse {
    0%, 100% { opacity: 0.55; transform: scale(0.86); }
    50% { opacity: 1; transform: scale(1); }
  }

  @keyframes pv-panel-in-left {
    from { opacity: 0; }
    to { opacity: 1; }
  }

  @keyframes pv-panel-in-right {
    from { opacity: 0; }
    to { opacity: 1; }
  }

  @keyframes pv-caret-blink {
    0%, 45% { opacity: 1; }
    46%, 100% { opacity: 0; }
  }

  @keyframes pv-typing {
    0%, 100% { opacity: 0.35; transform: translateY(0); }
    50% { opacity: 1; transform: translateY(-2px); }
  }

  .pet-button[data-live="false"] .pet-sprite {
    filter: saturate(0.42) brightness(0.82);
  }

  .pet-button[data-dragging="true"] {
    cursor: grabbing;
  }

  .status-dot[data-live="false"] {
    background: var(--pv-danger);
    animation: pv-status-pulse 1.4s ease-in-out infinite;
  }

  .chat-panel[data-side="left"] {
    animation: pv-panel-in-left 180ms ease-out;
  }

  .chat-panel[data-side="right"] {
    animation: pv-panel-in-right 180ms ease-out;
  }

  @media (prefers-reduced-motion: reduce) {
    .chat-panel,
    .agent-activity span,
    .stream-caret,
    .status-dot {
      animation: none !important;
    }
  }
`;
