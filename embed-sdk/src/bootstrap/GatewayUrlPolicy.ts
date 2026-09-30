import { DockClientError } from '../protocol/errors.js';

export const DEFAULT_GATEWAY_URL = 'http://127.0.0.1:18991';

const LOOPBACK_HOSTS = new Set(['127.0.0.1', 'localhost', '[::1]', '::1']);

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

/**
 * Gateway URL for device-token connections (`dock serve --remote` behind a TLS
 * reverse proxy or Tailscale). Remote hosts must use `https:` so the token
 * never crosses the network in clear text; loopback `http:` stays allowed for
 * SSH tunnels and local testing.
 */
export function normalizeRemoteGatewayUrl(value: unknown) {
  const input = String(value || '').trim();
  if (!input) throw invalidGatewayUrl('A device token needs an explicit gatewayUrl');
  let parsed: URL;
  try {
    parsed = new URL(input);
  } catch {
    throw invalidGatewayUrl('Gateway URL must be a valid https:// address');
  }
  const loopback = LOOPBACK_HOSTS.has(parsed.hostname.toLowerCase());
  if (parsed.protocol !== 'https:' && !(parsed.protocol === 'http:' && loopback)) {
    throw invalidGatewayUrl('Remote gateway URL must use https:// (http:// only for loopback)');
  }
  if (parsed.username || parsed.password) {
    throw invalidGatewayUrl('Gateway URL cannot contain credentials');
  }
  if ((parsed.pathname && parsed.pathname !== '/') || parsed.search || parsed.hash) {
    throw invalidGatewayUrl('Gateway URL cannot contain a path, query, or fragment');
  }
  return parsed.origin;
}
