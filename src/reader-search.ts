export interface TextRange { start: number; end: number }
export interface TextSegment { text: string; matched: boolean }

// Match the core's NFD/remove-combining-marks/lowercase folding, retaining UTF-16
// source boundaries (including astral characters and decomposed accents).
function foldedText(text: string) {
  let folded = '', offset = 0;
  const starts: number[] = [], ends: number[] = [];
  for (const character of text) {
    const end = offset + character.length;
    const value = character.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();
    if (!value) {
      for (let i = ends.length - 1; i >= 0 && ends[i] === offset; i--) ends[i] = end;
    }
    for (let i = 0; i < value.length; i++) { starts.push(offset); ends.push(end); }
    folded += value; offset = end;
  }
  return { folded, starts, ends };
}

export function matchedRanges(text: string, terms: readonly string[]): TextRange[] {
  if (!text || !terms.length) return [];
  const { folded, starts, ends } = foldedText(text);
  const needles = [...new Set(terms.map(term => foldedText(term).folded).filter(Boolean))];
  const ranges: TextRange[] = [];
  for (const needle of needles) {
    let from = 0, index: number;
    while ((index = folded.indexOf(needle, from)) !== -1) {
      // Core terms can be prefixes, but should not paint the middle of a word.
      const before = [...folded.slice(Math.max(0, index - 2), index)].at(-1) ?? '';
      if (!/[\p{L}\p{N}]/u.test(before)) ranges.push({ start: starts[index], end: ends[index + needle.length - 1] });
      from = index + Math.max(1, needle.length);
    }
  }
  ranges.sort((a, b) => a.start - b.start || b.end - a.end);
  const merged: TextRange[] = [];
  for (const range of ranges) {
    const previous = merged.at(-1);
    if (previous && range.start <= previous.end) previous.end = Math.max(previous.end, range.end);
    else merged.push({ ...range });
  }
  return merged;
}

export function textSegments(text: string, terms: readonly string[]): TextSegment[] {
  const segments: TextSegment[] = []; let offset = 0;
  for (const range of matchedRanges(text, terms)) {
    if (range.start > offset) segments.push({ text: text.slice(offset, range.start), matched: false });
    segments.push({ text: text.slice(range.start, range.end), matched: true }); offset = range.end;
  }
  if (offset < text.length) segments.push({ text: text.slice(offset), matched: false });
  return segments;
}

export function searchBlocks(root: HTMLElement, start: number, end?: number | null): HTMLElement[] {
  const blocks = [...root.querySelectorAll<HTMLElement>('[data-source-start]')];
  const entries = blocks.map((block, i) => ({ block, start: Number(block.dataset.sourceStart), end: block.dataset.sourceEnd === undefined ? Number(blocks[i + 1]?.dataset.sourceStart ?? Infinity) : Number(block.dataset.sourceEnd) }));
  if (end !== undefined && end !== null && end > start) {
    return entries.filter(entry => entry.start < end && entry.end > start).map(entry => entry.block);
  }
  const nearest = entries.filter(entry => entry.start <= start).at(-1)?.block ?? blocks[0];
  return nearest ? [nearest] : [];
}

// Unwrap, don't replace textContent: another mark layer may be nested inside.
export function clearMarks(marks: Iterable<HTMLElement>): void {
  const parents = new Set<HTMLElement>();
  for (const mark of marks) {
    if (mark.parentElement) parents.add(mark.parentElement);
    mark.replaceWith(...mark.childNodes);
  }
  parents.forEach(parent => parent.normalize());
}

export function markSearchBlocks(blocks: HTMLElement[], terms: readonly string[]): HTMLElement[] {
  const marks: HTMLElement[] = [];
  if (!terms.length) return marks;
  for (const block of blocks) {
    const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT);
    const nodes: { node: Text; start: number; end: number }[] = [];
    let text = '';
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      if (node.parentElement?.closest('button, script, style, [hidden], [aria-hidden="true"]')) { text += '\0'; continue; }
      const start = text.length; text += node.data; nodes.push({ node, start, end: text.length });
    }
    const ranges = matchedRanges(text, terms);
    // Ranges operate across inline elements/token spans; only text nodes change.
    let rangeIndex = 0;
    for (const { node, start, end } of nodes) {
      while (rangeIndex < ranges.length && ranges[rangeIndex].end <= start) rangeIndex++;
      if (!ranges[rangeIndex] || ranges[rangeIndex].start >= end) continue;
      const fragment = document.createDocumentFragment(); let offset = 0;
      for (let i = rangeIndex; i < ranges.length && ranges[i].start < end; i++) {
        const from = Math.max(start, ranges[i].start) - start, to = Math.min(end, ranges[i].end) - start;
        fragment.append(node.data.slice(offset, from));
        const mark = document.createElement('mark'); mark.className = 'search-match'; mark.textContent = node.data.slice(from, to);
        fragment.append(mark); marks.push(mark); offset = to;
      }
      fragment.append(node.data.slice(offset)); node.replaceWith(fragment);
    }
  }
  return marks;
}

export function scrollToSearchMatch(marks: HTMLElement[], fallback?: HTMLElement): void {
  const visible = marks.find(mark => !mark.closest('[hidden], [aria-hidden="true"]') && mark.getClientRects().length > 0 && getComputedStyle(mark).visibility !== 'hidden');
  (visible ?? fallback)?.scrollIntoView({ block: 'center' });
}
