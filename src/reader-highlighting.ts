import hljs from 'highlight.js/lib/common';

export const MAX_HIGHLIGHT_CHARACTERS = 16_000;
export const MAX_HIGHLIGHT_LINES = 400;
export function highlightLanguage(code: HTMLElement, text: string): string | undefined {
  const language = [...code.classList].find(c => c.startsWith('language-'))?.slice(9);
  return language && hljs.getLanguage(language) && text.length <= MAX_HIGHLIGHT_CHARACTERS && text.split('\n').length <= MAX_HIGHLIGHT_LINES ? language : undefined;
}

// One block is indivisible; thresholds bound it, and slices yield after 4ms.
export function enhanceCode(root: HTMLElement, reader: HTMLElement, report: (error: unknown) => void): () => void {
  const blocks = [...root.querySelectorAll<HTMLElement>('pre code')];
  let index = 0, timer: ReturnType<typeof setTimeout>, cancelled = false, scrollingUntil = 0;
  const buttons: HTMLButtonElement[] = [], resets = new Set<ReturnType<typeof setTimeout>>();
  const onScroll = () => { scrollingUntil = performance.now() + 150; };
  if (blocks.length) reader.addEventListener('scroll', onScroll, { passive: true });
  const run = () => {
    if (cancelled) return;
    if (performance.now() < scrollingUntil) { timer = setTimeout(run, 160); return; }
    const start = performance.now();
    do {
      const code = blocks[index++];
      if (!root.contains(code)) continue;
      const text = code.textContent ?? '', language = highlightLanguage(code, text);
      // Reader mark layers own these text nodes; queued token jobs must preserve them.
      if (language && !code.querySelector('mark.find-match, mark.search-match')) {
        try { code.innerHTML = hljs.highlight(text, { language }).value; } catch { /* Keep readable plaintext. */ }
      }
      code.classList.add('hljs');
      const pre = code.parentElement!;
      if (pre.querySelector('.copy-code')) continue;
      const button = document.createElement('button');
      button.className = 'copy-code'; button.type = 'button'; button.textContent = 'Copy'; button.setAttribute('aria-label', 'Copy code block');
      button.onclick = async () => {
        try {
          await navigator.clipboard.writeText(text);
          if (cancelled) return;
          button.textContent = 'Copied';
          const reset = setTimeout(() => { resets.delete(reset); button.textContent = 'Copy'; }, 1500); resets.add(reset);
        } catch (error) { if (!cancelled) report(error); }
      };
      pre.append(button); buttons.push(button);
    } while (index < blocks.length && performance.now() - start < 4);
    if (index < blocks.length) timer = setTimeout(run, 16);
    else reader.removeEventListener('scroll', onScroll);
  };
  if (blocks.length) timer = setTimeout(run, 16);
  return () => { cancelled = true; clearTimeout(timer); resets.forEach(clearTimeout); reader.removeEventListener('scroll', onScroll); buttons.forEach(button => { button.onclick = null; button.remove(); }); };
}
