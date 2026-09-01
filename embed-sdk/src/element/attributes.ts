export interface DockElementOptions {
  application: string;
  gatewayUrl?: string;
  skin?: string;
  skinUrl?: string;
  theme?: 'auto' | 'light' | 'dark';
  autoConnect?: boolean;
}

export const AGENT_ATTRIBUTES = [
  'application',
  'gateway-url',
  'skin',
  'skin-url',
  'theme',
  'auto-connect'
] as const;

export function readAgentAttributes(element: Element): DockElementOptions {
  const application = text(element.getAttribute('application')).toLowerCase();
  if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(application)) {
    throw new Error('dock-agent requires a stable lowercase application attribute');
  }
  const themeValue = text(element.getAttribute('theme'));
  const theme = themeValue === 'light' || themeValue === 'dark' ? themeValue : 'auto';
  return {
    application,
    gatewayUrl: optional(element.getAttribute('gateway-url')),
    skin: optional(element.getAttribute('skin')),
    skinUrl: optional(element.getAttribute('skin-url')),
    theme,
    autoConnect: element.getAttribute('auto-connect') !== 'false'
  };
}

export function applyAgentOptions(element: Element, options: DockElementOptions) {
  element.setAttribute('application', options.application);
  setOptional(element, 'gateway-url', options.gatewayUrl);
  setOptional(element, 'skin', options.skin);
  setOptional(element, 'skin-url', options.skinUrl);
  element.setAttribute('theme', options.theme || 'auto');
  element.setAttribute('auto-connect', options.autoConnect === false ? 'false' : 'true');
}

function setOptional(element: Element, name: string, value?: string) {
  if (value) element.setAttribute(name, value);
  else element.removeAttribute(name);
}

function optional(value: unknown) {
  const valueText = text(value);
  return valueText || undefined;
}

function text(value: unknown) {
  return String(value || '').trim();
}
