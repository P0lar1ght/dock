export interface RuntimeQuestionOption {
  label: string;
  description: string;
  recommended: boolean;
}

export interface RuntimeQuestion {
  id: string;
  header: string;
  question: string;
  options: RuntimeQuestionOption[];
}

export interface RuntimeUserInputAnswer {
  questionId: string;
  value: string;
  kind: 'option' | 'other';
}

export interface RuntimeInteractionResolutionResult {
  ok: boolean;
  resumed: boolean;
  code?: string;
  message?: string;
}
