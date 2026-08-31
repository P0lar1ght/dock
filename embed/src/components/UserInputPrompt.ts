import { html, nothing } from 'lit';

import type { UserInputInteractionState } from '../controllers/UserInputController.js';
import { OTHER_USER_INPUT_VALUE } from '../controllers/UserInputController.js';
import type { SessionUserInputRequest } from '../session/UserInputModel.js';

export interface UserInputPromptActions {
  select: (interactionId: string, questionId: string, value: string) => void;
  other: (interactionId: string, questionId: string, value: string) => void;
  submit: (request: SessionUserInputRequest) => void;
}

export function userInputPrompt(
  request: SessionUserInputRequest,
  interaction: UserInputInteractionState | undefined,
  actions: UserInputPromptActions
) {
  const state = interaction || { selections: {}, otherValues: {}, submitting: false };
  const pending = request.status === 'pending';
  return html`
    <article
      class="user-input-card"
      data-testid="user-input-prompt"
      data-status=${request.status}
    >
      <header>
        <strong>需要你选择规划方向</strong>
        <span>${pending ? '回答后继续同一个 Planning Run' : request.status === 'cancelled' ? '已取消' : '已回答'}</span>
      </header>
      ${request.questions.map((question) => html`
        <fieldset ?disabled=${!pending || state.submitting}>
          <legend><small>${question.header}</small>${question.question}</legend>
          ${question.options.map((option) => html`
            <label>
              <input
                type="radio"
                name=${`${request.id}:${question.id}`}
                .checked=${state.selections[question.id] === option.label}
                @change=${() => actions.select(request.id, question.id, option.label)}
              />
              <span>
                <strong>${option.label}${option.recommended ? html` <em>推荐</em>` : nothing}</strong>
                <small>${option.description}</small>
              </span>
            </label>
          `)}
          <label>
            <input
              type="radio"
              name=${`${request.id}:${question.id}`}
              .checked=${state.selections[question.id] === OTHER_USER_INPUT_VALUE}
              @change=${() => actions.select(request.id, question.id, OTHER_USER_INPUT_VALUE)}
            />
            <span><strong>其他</strong><small>输入一个不在候选项中的方向</small></span>
          </label>
          ${state.selections[question.id] === OTHER_USER_INPUT_VALUE ? html`
            <input
              class="user-input-other"
              type="text"
              maxlength="1000"
              placeholder="描述你的选择…"
              .value=${state.otherValues[question.id] || ''}
              @input=${(event: InputEvent) => actions.other(
                request.id,
                question.id,
                (event.currentTarget as HTMLInputElement).value
              )}
            />
          ` : nothing}
        </fieldset>
      `)}
      ${request.autoResolutionMs && pending ? html`
        <p>若暂不回答，约 ${Math.round(request.autoResolutionMs / 1000)} 秒后采用推荐项。</p>
      ` : nothing}
      ${state.error ? html`<p class="user-input-error" role="alert">${state.error}</p>` : nothing}
      ${pending ? html`
        <button
          type="button"
          ?disabled=${state.submitting}
          @click=${() => actions.submit(request)}
        >${state.submitting ? '正在提交…' : '确认并继续规划'}</button>
      ` : nothing}
    </article>
  `;
}
