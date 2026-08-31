export interface GatewayErrorBody {
  code?: string;
  message?: string;
}

export class DockClientError extends Error {
  constructor(
    public readonly code: string,
    message: string,
    public readonly status?: number
  ) {
    super(message);
    this.name = 'DockClientError';
  }
}

export function clientError(value: unknown, fallbackCode: string, status?: number) {
  const body = object(value);
  const nested = object(body.error);
  const code = text(nested.code ?? body.code) || fallbackCode;
  const message = text(nested.message ?? body.message) || 'Dock Gateway request failed';
  return new DockClientError(code, message, status);
}

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function text(value: unknown) {
  return String(value || '').trim();
}
