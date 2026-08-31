import { css } from 'lit';

export const imageInputStyles = css`
  .pending-image-strip {
    display: flex;
    gap: 8px;
    padding: 1px 2px 6px;
    overflow-x: auto;
    scrollbar-width: thin;
  }

  .pending-image {
    position: relative;
    display: grid;
    width: 92px;
    min-width: 92px;
    gap: 4px;
    margin: 0;
    padding: 5px;
    border: 1px solid var(--pv-border);
    border-radius: 11px;
    background: color-mix(in srgb, var(--pv-panel) 82%, transparent);
  }

  .pending-image img {
    width: 100%;
    height: 58px;
    object-fit: cover;
    border-radius: 7px;
    background: color-mix(in srgb, var(--pv-text) 5%, transparent);
  }

  .pending-image figcaption {
    overflow: hidden;
    color: var(--pv-muted);
    font-size: 9px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pending-image > div {
    display: flex;
    justify-content: center;
    gap: 2px;
  }

  .pending-image button {
    display: grid;
    width: 24px;
    height: 22px;
    padding: 0;
    place-items: center;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--pv-muted);
    cursor: pointer;
  }

  .pending-image button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--pv-text) 9%, transparent);
    color: var(--pv-text);
  }

  .pending-image button:disabled {
    cursor: default;
    opacity: 0.3;
  }

  .pending-image .control-icon {
    width: 10px;
    height: 10px;
  }

  .attachment-control {
    width: 32px;
    padding: 0;
  }

  .composer:has(.pending-image-strip) {
    border-color: color-mix(in srgb, var(--pv-accent) 35%, var(--pv-border));
  }
`;
