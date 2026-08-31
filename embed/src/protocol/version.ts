export const SUPPORTED_PROTOCOL_VERSION = '2026-07-24.1';

export function assertProtocolVersion(actual: string) {
  if (actual !== SUPPORTED_PROTOCOL_VERSION) {
    throw new Error(`Unsupported Dock protocol ${actual}; expected ${SUPPORTED_PROTOCOL_VERSION}`);
  }
}
