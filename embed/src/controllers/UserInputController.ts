import type { AgentSession } from '../session/AgentSession.js';
import type { RuntimeUserInputAnswer } from '../protocol/interactions.js';
import type { SessionUserInputRequest } from '../session/UserInputModel.js';

export interface UserInputInteractionState {
  selections: Readonly<Record<string, string>>;
  otherValues: Readonly<Record<string, string>>;
  submitting: boolean;
  error?: string;
}

const OTHER = '__other__';

export class UserInputController {
  private session?: AgentSession;
  private readonly states = new Map<string, UserInputInteractionState>();

  constructor(private readonly onChange: () => void) {}

  bind(session: AgentSession | undefined) {
    if (this.session === session) return;
    this.session = session;
    this.states.clear();
  }

  interactions(requests: readonly SessionUserInputRequest[]) {
    return Object.fromEntries(requests.map((request) => [
      request.id,
      this.states.get(request.id) || emptyState()
    ]));
  }

  select(interactionId: string, questionId: string, value: string) {
    const current = this.states.get(interactionId) || emptyState();
    this.states.set(interactionId, {
      ...current,
      selections: { ...current.selections, [questionId]: value },
      error: undefined
    });
    this.onChange();
  }

  setOther(interactionId: string, questionId: string, value: string) {
    const current = this.states.get(interactionId) || emptyState();
    this.states.set(interactionId, {
      ...current,
      selections: { ...current.selections, [questionId]: OTHER },
      otherValues: { ...current.otherValues, [questionId]: value.slice(0, 1_000) },
      error: undefined
    });
    this.onChange();
  }

  async submit(request: SessionUserInputRequest) {
    const session = this.session;
    if (!session || request.status !== 'pending') return false;
    const current = this.states.get(request.id) || emptyState();
    let answers: RuntimeUserInputAnswer[];
    try {
      answers = request.questions.map((question) => {
        const selected = current.selections[question.id];
        if (!selected) throw new Error(`请选择“${question.header}”`);
        if (selected === OTHER) {
          const value = String(current.otherValues[question.id] || '').trim();
          if (!value) throw new Error(`请填写“${question.header}”的其他方向`);
          return { questionId: question.id, value, kind: 'other' as const };
        }
        return { questionId: question.id, value: selected, kind: 'option' as const };
      });
    } catch (error) {
      this.states.set(request.id, {
        ...current,
        error: error instanceof Error ? error.message : '请选择所有方向'
      });
      this.onChange();
      return false;
    }
    this.states.set(request.id, { ...current, submitting: true, error: undefined });
    this.onChange();
    try {
      const result = await session.respondToInteraction(request.id, answers);
      if (!result.ok) throw new Error(result.message || '交互已结束');
      return true;
    } catch (error) {
      this.states.set(request.id, {
        ...current,
        submitting: false,
        error: error instanceof Error ? error.message : '方向未能提交'
      });
      return false;
    } finally {
      this.onChange();
    }
  }
}

export { OTHER as OTHER_USER_INPUT_VALUE };

function emptyState(): UserInputInteractionState {
  return { selections: {}, otherValues: {}, submitting: false };
}
