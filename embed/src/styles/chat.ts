import { css } from 'lit';

export const chatStyles = css`
  .chat-panel {
    position: fixed;
    width: clamp(
      var(--pv-chat-min-width),
      var(--pv-chat-fluid-width),
      var(--pv-chat-max-width)
    );
    max-width: calc(100vw - var(--pv-pet-width) - 38px);
    height: min(640px, calc(100vh - 24px));
    min-width: 210px;
    min-height: 360px;
    pointer-events: auto;
    border: 1px solid var(--pv-border);
    border-radius: var(--pv-radius);
    background: var(--pv-panel);
    box-shadow: var(--pv-shadow);
    color: var(--pv-text);
    backdrop-filter: blur(20px) saturate(1.2);
  }

  .chat-panel::after {
    content: "";
    position: absolute;
    top: var(--pv-panel-tail-y, 50%);
    width: 18px;
    height: 18px;
    background: var(--pv-panel);
    transform: translateY(-50%) rotate(45deg);
  }

  .chat-panel[data-side="left"]::after {
    right: -9px;
    border-right: 1px solid var(--pv-border);
    border-top: 1px solid var(--pv-border);
  }

  .chat-panel[data-side="right"]::after {
    left: -9px;
    border-bottom: 1px solid var(--pv-border);
    border-left: 1px solid var(--pv-border);
  }

  .panel-content {
    position: relative;
    z-index: 1;
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    height: 100%;
    overflow: hidden;
    border-radius: inherit;
    background: var(--pv-panel);
  }

  .panel-header {
    position: relative;
    display: flex;
    width: 100%;
    min-width: 0;
    align-items: center;
    gap: 10px;
    padding: 13px 14px 12px;
    border-bottom: 1px solid var(--pv-border);
  }

  .panel-avatar {
    display: grid;
    width: 38px;
    height: 38px;
    flex: 0 0 auto;
    place-items: center;
    border: 1px solid rgba(255, 189, 85, 0.5);
    border-radius: 14px;
    background: linear-gradient(145deg, #ffd65c, #e99524);
    color: #5b3205;
    font-size: 15px;
    font-weight: 850;
    box-shadow: inset 0 1px rgba(255, 255, 255, 0.55);
  }

  .panel-heading {
    min-width: 0;
    flex: 1;
  }

  .panel-title {
    margin: 0;
    overflow: hidden;
    font-size: 14px;
    font-weight: 760;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .panel-subtitle {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 2px 0 0;
    overflow: hidden;
    color: var(--pv-muted);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .header-status-dot {
    width: 6px;
    height: 6px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: var(--pv-danger);
  }

  .header-status-dot[data-live="true"] {
    background: var(--pv-success);
  }

  .icon-button,
  .action-button {
    border: 1px solid var(--pv-border);
    border-radius: 10px;
    background: var(--pv-panel-raised);
    color: var(--pv-text);
    cursor: pointer;
  }

  .icon-button {
    display: grid;
    width: 32px;
    height: 32px;
    flex: 0 0 32px;
    padding: 0;
    place-items: center;
    font-size: 18px;
    line-height: 1;
  }

  .icon-button:focus-visible,
  .action-button:focus-visible,
  .send-button:focus-visible,
  textarea:focus-visible,
  select:focus-visible {
    outline: 2px solid var(--pv-accent);
    outline-offset: 2px;
  }

  .skin-menu {
    position: relative;
    flex: 0 0 auto;
  }

  .skin-menu summary {
    list-style: none;
  }

  .skin-menu summary::-webkit-details-marker {
    display: none;
  }

  .skin-menu-popover {
    position: absolute;
    top: 40px;
    right: 0;
    z-index: 4;
    display: grid;
    width: 230px;
    gap: 8px;
    padding: 12px;
    border: 1px solid var(--pv-border);
    border-radius: 13px;
    background: var(--pv-panel);
    box-shadow: var(--pv-shadow);
  }

  .skin-menu-popover label {
    color: var(--pv-muted);
    font-size: 11px;
  }

  select,
  .action-button {
    min-width: 0;
    min-height: 36px;
    padding: 7px 9px;
  }

  select {
    border: 1px solid var(--pv-border);
    border-radius: 9px;
    outline: none;
    background: var(--pv-panel-raised);
    color: var(--pv-text);
  }

  .action-button {
    font-size: 12px;
    font-weight: 700;
  }

  .message-list {
    display: flex;
    min-height: 0;
    flex-direction: column;
    gap: 13px;
    overflow-x: hidden;
    overflow-y: auto;
    padding: 18px 16px 16px;
    overscroll-behavior: contain;
    scrollbar-width: thin;
  }

  .empty-chat {
    max-width: 240px;
    margin: auto;
    color: var(--pv-muted);
    text-align: center;
  }

  .empty-chat-mark {
    display: grid;
    width: 48px;
    height: 48px;
    margin: 0 auto 12px;
    place-items: center;
    border: 1px solid var(--pv-border);
    border-radius: 17px;
    background: linear-gradient(145deg, rgba(115, 219, 255, 0.18), rgba(43, 183, 236, 0.04));
    color: var(--pv-accent);
    font-size: 23px;
  }

  .empty-chat h3 {
    margin: 0 0 7px;
    color: var(--pv-text);
    font-size: 16px;
  }

  .empty-chat p {
    margin: 0;
    font-size: 12px;
  }

  .message-row {
    display: flex;
    max-width: 88%;
    flex-direction: column;
    gap: 4px;
  }

  .message-row.user {
    align-self: flex-end;
    align-items: flex-end;
  }

  .message-row.assistant {
    align-self: flex-start;
    align-items: flex-start;
  }

  .message-meta {
    padding: 0 4px;
    color: var(--pv-muted);
    font-size: 10px;
  }

  .message-bubble {
    padding: 10px 12px;
    border: 1px solid var(--pv-border);
    border-radius: 15px 15px 15px 5px;
    background: var(--pv-panel-raised);
    font-size: 13px;
    line-height: 1.55;
    overflow-wrap: anywhere;
    white-space: normal;
  }

  .user .message-copy {
    white-space: pre-wrap;
  }

  .message-attachment {
    display: block;
    width: fit-content;
    margin-top: 7px;
    padding: 3px 7px;
    border: 1px solid color-mix(in srgb, var(--pv-accent) 42%, var(--pv-border));
    border-radius: 999px;
    color: var(--pv-muted);
    font-size: 10px;
  }

  .message-image-preview {
    display: grid;
    width: min(260px, 100%);
    gap: 5px;
    margin: 8px 0 0;
  }

  .message-image-preview img {
    display: block;
    width: 100%;
    max-height: 180px;
    border: 1px solid color-mix(in srgb, var(--pv-accent) 35%, var(--pv-border));
    border-radius: 9px;
    background: #111;
    object-fit: contain;
  }

  .message-image-preview figcaption {
    color: var(--pv-muted);
    font-size: 10px;
    line-height: 1.35;
  }

  .user .message-bubble {
    border-color: rgba(115, 219, 255, 0.4);
    border-radius: 15px 15px 5px 15px;
    background: linear-gradient(145deg, rgba(43, 183, 236, 0.3), rgba(43, 183, 236, 0.17));
  }

  .message-row[data-status="failed"] .message-bubble {
    border-color: rgba(255, 127, 143, 0.55);
  }

  .stream-caret {
    display: inline-block;
    width: 2px;
    height: 1em;
    margin-left: 3px;
    vertical-align: -0.15em;
    background: var(--pv-accent);
    animation: pv-caret-blink 760ms steps(1) infinite;
  }

  .message-failed {
    display: block;
    margin-top: 5px;
    color: var(--pv-danger);
    font-size: 10px;
  }

  .agent-activity {
    display: flex;
    align-items: center;
    gap: 4px;
    align-self: flex-start;
    color: var(--pv-muted);
    font-size: 11px;
  }

  .agent-activity span {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--pv-accent);
    animation: pv-typing 900ms ease-in-out infinite;
  }

  .agent-activity span:nth-child(2) { animation-delay: 120ms; }
  .agent-activity span:nth-child(3) { animation-delay: 240ms; }
  .agent-activity em { margin-left: 3px; font-style: normal; }

  .reconnect-button { width: 100%; }

  @media (max-height: 520px) {
    .chat-panel {
      height: calc(100vh - 16px);
      min-height: 0;
      border-radius: 16px;
    }

    .panel-header { padding-block: 9px; }
  }
`;
