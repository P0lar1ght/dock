import { DockClientError } from '../protocol/errors.js';

export const DEFAULT_GATEWAY_URL = 'http://127.0.0.1:18990';

const LOOPBACK_HOSTS = new Set(['127.0.0.1', 'localhost', '[::1]']);

export function normalizeGatewayUrl(value: unknown) {
  const input = String(value || '').trim() || DEFAULT_GATEWAY_URL;
  let parsed: URL;
  try {
    parsed = new URL(input);
  } catch {
    throw invalidGatewayUrl('Gateway URL must be a valid loopback HTTP address');
  }
  if (parsed.protocol !== 'http:') {
    throw invalidGatewayUrl('Gateway URL must use loopback HTTP');
  }
  if (!LOOPBACK_HOSTS.has(parsed.hostname.toLowerCase())) {
    throw invalidGatewayUrl('Gateway URL must target 127.0.0.1, localhost, or [::1]');
  }
  if (parsed.username || parsed.password) {
    throw invalidGatewayUrl('Gateway URL cannot contain credentials');
  }
  if ((parsed.pathname && parsed.pathname !== '/') || parsed.search || parsed.hash) {
    throw invalidGatewayUrl('Gateway URL cannot contain a path, query, or fragment');
  }
  return parsed.origin;
}

function invalidGatewayUrl(message: string) {
  return new DockClientError('invalid_gateway_url', message);
}
