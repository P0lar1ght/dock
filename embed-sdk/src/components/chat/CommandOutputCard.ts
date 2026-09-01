import { html, nothing } from 'lit';
import {
  commandOutputLooksLikeMarkdown,
  type CommandOutputView
} from '../../controllers/CommandOutput.js';
import { assistantMessageParts } from '../../rendering/messageParts.js';
import { TOOL_DIAMOND } from './toolCard.js';

export function commandOutputCard(model: CommandOutputView | undefined, dismiss: () => void) {
  if (!model) return nothing;
  const compact = model.kind === 'applied';
  const kicker = compact ? '已完成' : '命令输出';
  const showTitle = Boolean(model.title) && model.title !== kicker;
  return html`
    <section
      class="command-output"
      data-testid="command-output"
      data-kind=${model.kind}
      role="region"
      aria-label=${showTitle ? `${kicker}：${model.title}` : kicker}
    >
      <header class="command-output-header">
        <span class="command-output-diamond" aria-hidden="true">${TOOL_DIAMOND}</span>
        <span class="command-output-kicker">${kicker}</span>
        ${showTitle ? html`<h3 class="command-output-title">${model.title}</h3>` : nothing}
        <button
          class="command-output-close"
          type="button"
          aria-label="关闭命令输出"
          @click=${dismiss}
        >×</button>
      </header>
      <div class="command-output-body">
        ${commandOutputLooksLikeMarkdown(model.body)
          ? assistantMessageParts(model.body)
          : html`<pre class="command-output-plain">${model.body}</pre>`}
      </div>
    </section>
  `;
}
