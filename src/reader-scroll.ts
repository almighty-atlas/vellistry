export interface HeadingOffset { id: string; top: number }

export function headingAt(offsets: HeadingOffset[], top: number): string {
  let low = 0, high = offsets.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (offsets[middle].top <= top) low = middle + 1;
    else high = middle;
  }
  return offsets[Math.max(0, low - 1)]?.id ?? '';
}

// Geometry is read only on invalidation, never by the scroll listener or its RAF.
export function trackHeadings(reader: HTMLElement, article: HTMLElement, select: (id: string) => void) {
  const headings = [...article.querySelectorAll<HTMLElement>('h1[id],h2[id],h3[id],h4[id],h5[id],h6[id]')];
  let offsets: HeadingOffset[] = [], frame = 0, rebuild = 0, disposed = false, selected: string | undefined;
  const update = () => {
    frame = 0;
    const id = headingAt(offsets, reader.scrollTop + 80);
    if (id !== selected) { selected = id; select(id); }
  };
  const onScroll = () => { if (!frame) frame = requestAnimationFrame(update); };
  const invalidate = () => {
    if (disposed || rebuild) return;
    rebuild = requestAnimationFrame(() => {
      rebuild = 0;
      const origin = reader.getBoundingClientRect().top + reader.clientTop - reader.scrollTop;
      offsets = headings.map(heading => ({ id: heading.id, top: heading.getBoundingClientRect().top - origin }));
      onScroll();
    });
  };
  const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(invalidate);
  observer?.observe(article); observer?.observe(reader);
  reader.addEventListener('scroll', onScroll, { passive: true });
  article.addEventListener('load', invalidate, true);
  article.addEventListener('error', invalidate, true);
  window.addEventListener('resize', invalidate);
  document.fonts?.addEventListener('loadingdone', invalidate);
  void document.fonts?.ready.then(invalidate);
  invalidate();
  return { invalidate, destroy() {
    disposed = true; cancelAnimationFrame(frame); cancelAnimationFrame(rebuild); observer?.disconnect();
    reader.removeEventListener('scroll', onScroll); article.removeEventListener('load', invalidate, true); article.removeEventListener('error', invalidate, true);
    window.removeEventListener('resize', invalidate); document.fonts?.removeEventListener('loadingdone', invalidate);
  } };
}
