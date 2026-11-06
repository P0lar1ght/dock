/** Strip encrypted thinking signatures from text shown on thinking cards.
 *
 * Mirrors cordis-base `visible_reasoning_text` / `split_reasoning_signature`:
 * - full `<signature>…</signature>` blocks
 * - orphan `AQEA…</signature>\nsummary` (opening tag lost across chunks / old
 *   persisted `reasoning` fields that lack `<signature>`)
 * - bare AQEA / long base64url ciphertext with no readable summary
 */
const SIGNATURE_OPEN = "<signature>";
const SIGNATURE_CLOSE = "</signature>";

function looksLikeEncryptedSignature(text: string): boolean {
  if (text.length < 16) return false;
  if (/\s/.test(text)) return false;
  if (text.startsWith("AQEA")) return true;
  return text.length >= 32 && /^[A-Za-z0-9+/=_-]+$/.test(text);
}

export function visibleReasoningText(text: string): string {
  let display = "";
  let rest = text;

  while (true) {
    const start = rest.indexOf(SIGNATURE_OPEN);
    if (start < 0) break;
    display += rest.slice(0, start);
    const afterOpen = rest.slice(start + SIGNATURE_OPEN.length);
    const end = afterOpen.indexOf(SIGNATURE_CLOSE);
    if (end < 0) {
      rest = "";
      break;
    }
    rest = afterOpen.slice(end + SIGNATURE_CLOSE.length);
  }

  const orphanEnd = rest.indexOf(SIGNATURE_CLOSE);
  if (orphanEnd >= 0) {
    rest = rest.slice(orphanEnd + SIGNATURE_CLOSE.length);
  }
  display += rest;

  const trimmed = display.trim();
  if (!trimmed || trimmed === "[REDACTED]") return "";
  if (looksLikeEncryptedSignature(trimmed)) return "";
  return display;
}
