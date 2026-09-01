const MAX_EXTERNAL_URL_LENGTH = 2_048;

/**
 * This module owns the only policy that can turn model text into a navigable
 * URL. Message content itself is never converted to an HTML string: Lit text
 * bindings remain the escaping boundary for every other value.
 */
export function safeExternalUrl(value: string): string | undefined {
  const candidate = value.trim();
  if (!candidate || candidate.length > MAX_EXTERNAL_URL_LENGTH) return undefined;
  if (/[\u0000-\u001f\u007f]/u.test(candidate)) return undefined;

  try {
    const url = new URL(candidate);
    if (!['http:', 'https:'].includes(url.protocol)) return undefined;
    if (url.username || url.password) return undefined;
    return url.href;
  } catch {
    return undefined;
  }
}

export const SAFE_EXTERNAL_LINK_REL = 'noopener noreferrer';
