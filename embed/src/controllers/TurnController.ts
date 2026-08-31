import type { AgentSession } from '../session/AgentSession.js';
import type { SessionState, SessionTurn } from '../session/SessionState.js';

const CANCELLABLE_STATUSES = new Set(['running', 'waiting_permission', 'waiting_input']);

export interface TurnControlView {
  turnId?: string;
  cancelling: boolean;
  error?: string;
}

export class TurnController {
  private sessionValue?: AgentSession;
  private stateValue?: Readonly<SessionState>;
  private requestedTurnId = '';
  private errorTurnId = '';
  private errorValue = '';

  constructor(private readonly onChange: () => void) {}

  get view(): TurnControlView {
    const active = cancellableTurn(this.stateValue?.turns);
    return {
      turnId: active?.id,
      cancelling: Boolean(active && this.requestedTurnId === active.id),
      error: active?.id === this.errorTurnId ? this.errorValue || undefined : undefined
    };
  }

  bind(session: AgentSession | undefined) {
    if (this.sessionValue === session) return;
    this.sessionValue = session;
    this.stateValue = session?.state;
    this.requestedTurnId = '';
    this.errorTurnId = '';
    this.errorValue = '';
  }

  update(state: Readonly<SessionState> | undefined) {
    this.stateValue = state;
    const active = cancellableTurn(state?.turns);
    if (this.requestedTurnId && active?.id !== this.requestedTurnId) {
      this.requestedTurnId = '';
    }
    if (this.errorTurnId && active?.id !== this.errorTurnId) {
      this.errorTurnId = '';
      this.errorValue = '';
    }
  }

  async cancelActiveTurn() {
    const session = this.sessionValue;
    const turn = cancellableTurn(this.stateValue?.turns);
    if (!session || !turn || this.requestedTurnId) return false;

    this.requestedTurnId = turn.id;
    this.errorTurnId = '';
    this.errorValue = '';
    this.onChange();
    try {
      const result = await session.cancelTurn(turn.id);
      if (!result.cancelled) throw new Error('Gateway 未接受停止请求');
      return true;
    } catch (error) {
      if (this.sessionValue === session && cancellableTurn(this.stateValue?.turns)?.id === turn.id) {
        this.requestedTurnId = '';
        this.errorTurnId = turn.id;
        this.errorValue = error instanceof Error ? error.message : String(error);
        this.onChange();
      }
      return false;
    }
  }
}

export function cancellableTurn(
  turns: Readonly<Record<string, SessionTurn>> | undefined
): SessionTurn | undefined {
  let selected: SessionTurn | undefined;
  for (const turn of Object.values(turns || {})) {
    if (CANCELLABLE_STATUSES.has(turn.status)) selected = turn;
  }
  return selected;
}
