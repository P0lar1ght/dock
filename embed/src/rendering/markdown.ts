import type { Root } from 'mdast';
import remarkGfm from 'remark-gfm';
import remarkParse from 'remark-parse';
import { unified } from 'unified';

import { normalizeInlineRoot, normalizeMarkdownRoot } from './markdownNormalizer.js';

export type {
  MarkdownBlock,
  MarkdownInline,
  MarkdownListItem,
  MarkdownTableAlignment,
  MarkdownTableRow
} from './markdownModel.js';

const processor = unified()
  .use(remarkParse)
  .use(remarkGfm, { singleTilde: false })
  .freeze();

export function parseMarkdown(source: string) {
  const normalized = normalizeSource(source);
  return normalizeMarkdownRoot(processor.parse(normalized) as Root, normalized);
}

export function parseInline(source: string) {
  const normalized = normalizeSource(source);
  return normalizeInlineRoot(processor.parse(normalized) as Root, normalized);
}

function normalizeSource(source: string) {
  return String(source || '').replace(/\r\n?/gu, '\n');
}
