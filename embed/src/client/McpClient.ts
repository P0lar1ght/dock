import { MCP_RELOAD } from '../protocol/methods.js';
import type { McpReloadResult } from '../protocol/responses.js';

type Request = <T>(method: string, params?: Record<string, unknown>) => Promise<T>;

export class McpClient {
  constructor(private readonly request: Request) {}

  reload() {
    return this.request<McpReloadResult>(MCP_RELOAD);
  }
}
