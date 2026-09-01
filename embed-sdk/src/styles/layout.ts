import { css } from 'lit';

export const layoutStyles = css`
  :host {
    all: initial;
    position: fixed;
    inset: 0;
    z-index: 2147483000;
    display: block;
    overflow: visible;
    pointer-events: none;
    color: var(--pv-text);
    font-family: var(--pv-font);
    line-height: 1.4;
    contain: layout style;
  }

  .pet-shell {
    position: fixed;
    width: var(--pv-pet-width);
    height: var(--pv-pet-height);
    pointer-events: auto;
    touch-action: none;
    user-select: none;
  }

  .pet-button {
    position: relative;
    width: 100%;
    height: 100%;
    margin: 0;
    padding: 0;
    border-radius: 28px;
    outline: none;
    background: transparent;
    cursor: grab;
    -webkit-tap-highlight-color: transparent;
  }

  .pet-button::before {
    content: "";
    position: absolute;
    inset: 13px 8px 5px;
    z-index: -1;
    border-radius: 50%;
    background: radial-gradient(circle, rgba(79, 199, 241, 0.2), rgba(79, 199, 241, 0));
    opacity: 0;
    transition: opacity 120ms ease;
  }

  .pet-button:hover::before,
  .pet-button:focus-visible::before {
    opacity: 1;
  }

  .pet-button:focus-visible {
    box-shadow: 0 0 0 3px rgba(115, 219, 255, 0.88), 0 0 0 7px rgba(8, 23, 38, 0.8);
  }

  .pet-sprite {
    width: 100%;
    height: 100%;
    background-repeat: no-repeat;
    image-rendering: auto;
    pointer-events: none;
    transition: filter 160ms ease;
  }

  .status-badge {
    position: absolute;
    right: 2px;
    bottom: 5px;
    display: flex;
    align-items: center;
    max-width: 96px;
    min-height: 24px;
    padding: 4px 7px;
    overflow: hidden;
    border: 1px solid var(--pv-border);
    border-radius: 999px;
    background: var(--pv-panel);
    box-shadow: 0 6px 18px rgba(0, 8, 19, 0.24);
    color: var(--pv-muted);
    font-size: 10px;
    font-weight: 650;
    letter-spacing: 0.01em;
    white-space: nowrap;
  }

  .status-dot {
    width: 7px;
    height: 7px;
    flex: 0 0 auto;
    margin-right: 5px;
    border-radius: 50%;
    background: var(--pv-success);
  }

  .status-label {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .pet-attention-badge {
    position: absolute;
    top: 7px;
    right: 5px;
    display: grid;
    min-width: 19px;
    height: 19px;
    padding: 0 5px;
    place-items: center;
    border: 2px solid var(--pv-panel);
    border-radius: 999px;
    background: var(--pv-danger);
    color: white;
    font-size: 10px;
    font-weight: 800;
    line-height: 1;
  }

`;
