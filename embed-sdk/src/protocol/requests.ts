export interface JsonRpcRequest {
  id: string;
  method: string;
  params: Record<string, unknown>;
}

export interface JsonRpcError {
  code: number;
  message: string;
  details?: Record<string, unknown>;
}

export interface JsonRpcResponse<T = unknown> {
  id: string;
  result?: T;
  error?: JsonRpcError;
}

export interface ThreadListParams {
  workspaceId: string;
}

export interface ThreadStartParams {
  workspaceId: string;
  title?: string;
}

export interface ThreadAccessParams {
  threadId: string;
  workspaceId: string;
}

export interface ThreadSubscribeParams extends ThreadAccessParams {
  sinceSeq: number;
}

export interface TurnStartParams extends ThreadAccessParams {
  message: string;
}
