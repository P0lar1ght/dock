import type { DockElementOptions } from '../element/attributes.js';

export function scriptAgentOptions(script: HTMLScriptElement): DockElementOptions {
  const application = text(script.dataset.application).toLowerCase();
  if (!application) throw new Error('data-application is required for Dock auto mount');
  return {
    application,
    gatewayUrl: optional(script.dataset.gatewayUrl),
    skin: optional(script.dataset.skin),
    skinUrl: optional(script.dataset.skinUrl),
    theme: theme(script.dataset.theme),
    autoConnect: script.dataset.autoConnect !== 'false'
  };
}

export function findAutoMountScript(documentValue: Document = document) {
  const current = documentValue.currentScript;
  if (current instanceof HTMLScriptElement && current.hasAttribute('data-dock-auto')) return current;
  const scripts = [...documentValue.querySelectorAll<HTMLScriptElement>('script[data-dock-auto]')];
  return scripts.find((script) => script.dataset.dockMounted !== 'true');
}

function theme(value: unknown): DockElementOptions['theme'] {
  return value === 'light' || value === 'dark' ? value : 'auto';
}

function optional(value: unknown) {
  const valueText = text(value);
  return valueText || undefined;
}

function text(value: unknown) {
  return String(value || '').trim();
}
