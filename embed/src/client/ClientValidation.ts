import { DockClientError } from '../protocol/errors.js';

const REQUIRED_SESSION_CAPABILITIES = [
  'threads',
  'turns',
  'permissions',
  'transcriptEvents',
  'threadSubscriptions'
];

export function assertSessionCapabilities(capabilities: Record<string, boolean>) {
  const missing = REQUIRED_SESSION_CAPABILITIES
    .filter((capability) => !capabilities[capability]);
  if (missing.length) {
    throw new DockClientError(
      'capability_not_supported',
      `Gateway does not support required Session capabilities: ${missing.join(', ')}`
    );
  }
}

export function normalizeApplication(value: string) {
  const application = String(value || '').trim().toLowerCase();
  if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(application)) {
    throw new Error('application must be a stable lowercase identifier');
  }
  return application;
}

export function textValue(value: unknown) {
  return String(value || '').trim();
}
