import { INTERACTION_RESPOND } from '../protocol/methods.js';
import type {
  RuntimeInteractionResolutionResult,
  RuntimeUserInputAnswer
} from '../protocol/interactions.js';

type Request = <T>(method: string, params?: Record<string, unknown>) => Promise<T>;

export class InteractionClient {
  constructor(private readonly request: Request) {}

  respond(input: {
    interactionId: string;
    threadId: string;
    turnId: string;
    workspaceId: string;
    answers: RuntimeUserInputAnswer[];
  }) {
    return this.request<RuntimeInteractionResolutionResult>(INTERACTION_RESPOND, {
      interactionId: input.interactionId,
      threadId: input.threadId,
      turnId: input.turnId,
      workspaceId: input.workspaceId,
      answers: input.answers
    });
  }
}
