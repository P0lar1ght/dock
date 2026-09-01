import { animationStyles } from './animations.js';
import { approvalStyles } from './approval.js';
import { chatStyles } from './chat.js';
import { commandOutputStyles } from './commandOutput.js';
import { composerStyles } from './composer.js';
import { layoutStyles } from './layout.js';
import { imageInputStyles } from './imageInputs.js';
import { markdownStyles } from './markdown.js';
import { planActivityStyles } from './planActivity.js';
import { goalActivityStyles } from './goalActivity.js';
import { recoveryStyles } from './recovery.js';
import { resetStyles } from './reset.js';
import { runtimeIssueStyles } from './runtimeIssue.js';
import { slashCommandStyles } from './slashCommands.js';
import { subagentActivityStyles } from './subagentActivity.js';
import { themeStyles } from './theme.js';
import { threadMenuStyles } from './threadMenu.js';
import { tokenStyles } from './tokens.js';
import { toolActivityStyles } from './toolActivity.js';
import { turnControlStyles } from './turnControls.js';
import { turnQueueStyles } from './turnQueue.js';
import { userInputStyles } from './userInput.js';

/** Central style composition for the custom element; feature styles stay in focused modules. */
export const elementStyles = [
  resetStyles,
  tokenStyles,
  themeStyles,
  layoutStyles,
  chatStyles,
  commandOutputStyles,
  composerStyles,
  imageInputStyles,
  slashCommandStyles,
  markdownStyles,
  recoveryStyles,
  planActivityStyles,
  goalActivityStyles,
  subagentActivityStyles,
  turnQueueStyles,
  threadMenuStyles,
  runtimeIssueStyles,
  turnControlStyles,
  toolActivityStyles,
  approvalStyles,
  userInputStyles,
  animationStyles
];
