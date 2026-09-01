export type HostToolJson =
  | null
  | boolean
  | number
  | string
  | HostToolJson[]
  | { [key: string]: HostToolJson };

export interface HostToolAnnotations {
  readOnly: boolean;
  destructive?: boolean;
  risk?: 'low' | 'medium' | 'high';
}

export interface HostToolDescriptor {
  name: string;
  title: string;
  description: string;
  inputSchema: Record<string, HostToolJson>;
  annotations: HostToolAnnotations;
  scopes?: readonly string[];
}

export interface HostToolTurnEnvelope {
  toolCatalogDigest: string;
  activeScopes: readonly string[];
}

export interface HostToolInvocationContext {
  invocationId: string;
  workspaceId: string;
  signal: AbortSignal;
}

export type HostToolHandler = (
  argumentsValue: Record<string, HostToolJson>,
  invocation: HostToolInvocationContext
) => HostToolJson | Promise<HostToolJson>;

export interface HostToolRegistration {
  readonly name: string;
  readonly registrationEpoch: string;
  readonly synchronized: Promise<void>;
  dispose(): Promise<void>;
}
