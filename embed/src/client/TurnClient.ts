import {
  TURN_CANCEL,
  TURN_ENQUEUE,
  TURN_QUEUE_LIST,
  TURN_QUEUE_REMOVE,
  TURN_START,
  TURN_STEER
} from '../protocol/methods.js';
import type { TurnContextEnvelope } from '../protocol/context.js';
import type { HostToolTurnEnvelope } from '../host-tools/types.js';
import type { PreparedImageInputReference } from '../image-inputs/types.js';
import type { StartTurnInput } from '../image-inputs/types.js';
import type { TurnQueueResult, TurnSubmission } from '../protocol/responses.js';
import type { RpcRequest } from './ThreadClient.js';

export class TurnClient {
  constructor(private readonly request: RpcRequest) {}

  start(
    threadId: string,
    workspaceId: string,
    message: string,
    context?: TurnContextEnvelope,
    hostTools?: HostToolTurnEnvelope,
    imageInputs?: readonly PreparedImageInputReference[],
    intent: NonNullable<StartTurnInput['intent']> = { mode: 'default' }
  ) {
    return this.request<TurnSubmission>(TURN_START, {
      threadId,
      workspaceId,
      message,
      intent,
      ...(context ? {
        contextItems: [...context.contextItems],
        contextSources: [...context.contextSources]
      } : {}),
      ...(hostTools ? {
        hostTools: {
          toolCatalogDigest: hostTools.toolCatalogDigest,
          activeScopes: [...hostTools.activeScopes]
        }
      } : {}),
      ...imageInputParams(imageInputs)
    });
  }

  cancel(threadId: string, workspaceId: string, turnId: string) {
    return this.request<{ cancelled: boolean }>(TURN_CANCEL, { threadId, workspaceId, turnId });
  }

  enqueue(
    threadId: string,
    workspaceId: string,
    message: string,
    hostTools?: HostToolTurnEnvelope,
    imageInputs?: readonly PreparedImageInputReference[]
  ) {
    return this.request<TurnSubmission>(TURN_ENQUEUE, {
      threadId,
      workspaceId,
      message,
      ...hostToolParams(hostTools),
      ...imageInputParams(imageInputs)
    });
  }

  steer(
    threadId: string,
    workspaceId: string,
    message: string,
    hostTools?: HostToolTurnEnvelope,
    imageInputs?: readonly PreparedImageInputReference[]
  ) {
    return this.request<TurnSubmission>(TURN_STEER, {
      threadId,
      workspaceId,
      message,
      ...hostToolParams(hostTools),
      ...imageInputParams(imageInputs)
    });
  }

  listQueue(threadId: string, workspaceId: string) {
    return this.request<TurnQueueResult>(TURN_QUEUE_LIST, { threadId, workspaceId });
  }

  removeQueued(threadId: string, workspaceId: string, queueId: string) {
    return this.request<{ threadId: string; queueId: string; removed: boolean }>(TURN_QUEUE_REMOVE, {
      threadId,
      workspaceId,
      queueId
    });
  }
}

function imageInputParams(imageInputs?: readonly PreparedImageInputReference[]) {
  return imageInputs?.length ? {
    imageInputs: imageInputs.map((image) => 'reuseTurnId' in image
      ? {
          reuseTurnId: image.reuseTurnId,
          detail: image.detail
        }
      : {
          id: image.id,
          digest: image.digest,
          detail: image.detail
        })
  } : {};
}

function hostToolParams(hostTools?: HostToolTurnEnvelope) {
  return hostTools ? {
    hostTools: {
      toolCatalogDigest: hostTools.toolCatalogDigest,
      activeScopes: [...hostTools.activeScopes]
    }
  } : {};
}
