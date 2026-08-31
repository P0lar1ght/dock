export interface ReconnectPolicyOptions {
  maxAttempts: number;
  initialDelayMs: number;
  maxDelayMs: number;
}

const DEFAULTS: ReconnectPolicyOptions = {
  maxAttempts: 8,
  initialDelayMs: 250,
  maxDelayMs: 4_000
};

export class ReconnectPolicy {
  readonly options: ReconnectPolicyOptions;

  constructor(options: Partial<ReconnectPolicyOptions> = {}) {
    this.options = {
      maxAttempts: positiveInteger(options.maxAttempts, DEFAULTS.maxAttempts),
      initialDelayMs: positiveInteger(options.initialDelayMs, DEFAULTS.initialDelayMs),
      maxDelayMs: positiveInteger(options.maxDelayMs, DEFAULTS.maxDelayMs)
    };
  }

  delay(attempt: number) {
    const exponent = Math.max(0, attempt - 1);
    return Math.min(this.options.maxDelayMs, this.options.initialDelayMs * 2 ** exponent);
  }

  shouldRetry(error: unknown) {
    const code = error && typeof error === 'object' && 'code' in error
      ? String((error as { code: unknown }).code)
      : '';
    return ![
      'pairing_required',
      'binding_revoked',
      'origin_not_allowed',
      'protocol_version_mismatch',
      'capability_not_supported'
    ].includes(code);
  }
}

export function reconnectDelay(ms: number) {
  return new Promise<void>((resolve) => setTimeout(resolve, ms));
}

function positiveInteger(value: unknown, fallback: number) {
  const number = Number(value);
  return Number.isInteger(number) && number > 0 ? number : fallback;
}
