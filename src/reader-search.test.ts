import { describe, expect, it, vi } from 'vitest';
import { clearMarks, markSearchBlocks, matchedRanges, scrollToSearchMatch, searchBlocks, textSegments } from './reader-search';

describe('safe folded search text', () => {
  it('maps accents, case, prefixes and decomposed Unicode back to exact UTF-16 text', () => {
    const text = '😀 ÜBERprüfung pru\u0308fen Straße İstanbul';
    expect(textSegments(text, ['uber', 'prufen', 'straße', 'istanbul']).filter(s => s.matched).map(s => s.text)).toEqual(['ÜBER', 'pru\u0308fen', 'Straße', 'İstanbul']);
    expect(textSegments(text, ['strasse']).some(s => s.matched)).toBe(false); // Same folding as Rust, not transliteration.
    expect(textSegments('🙂 Ä\u0308rger', ['a']).find(s => s.matched)?.text).toBe('Ä\u0308');
    expect(textSegments('ÜBERprüfung', ['uber', 'uberpruf']).filter(s => s.matched).map(s => s.text)).toEqual(['ÜBERprüf']);
  });
  it('highlights multiple phrase terms/corrected words without interpreting markup or regex', () => {
    const text = '<img onerror="x"> Überprüfung räumlich';
    const segments = textSegments(text, ['uberprufung', 'raumlich']);
    expect(segments.map(s => s.text).join('')).toBe(text);
    expect(segments.filter(s => s.matched).map(s => s.text)).toEqual(['Überprüfung', 'räumlich']);
    expect(matchedRanges('concatenate cat catalog', ['cat'])).toEqual([{ start: 12, end: 15 }, { start: 16, end: 19 }]);
    expect(textSegments('a+b [x]', ['a+b', '[x]']).filter(s => s.matched).map(s => s.text)).toEqual(['a+b', '[x]']);
    expect(textSegments('text', ['', '\u0308'])).toEqual([{ text: 'text', matched: false }]);
  });
});

describe('source-scoped reader marks', () => {
  it('uses exclusive source ranges and a single nearest-block fallback for legacy hits', () => {
    const root = document.createElement('article');
    root.innerHTML = '<div data-source-start="0" data-source-end="10">a</div><div data-source-start="10" data-source-end="30">b</div><div data-source-start="30" data-source-end="50">c</div>';
    expect(searchBlocks(root, 10, 30).map(b => b.textContent)).toEqual(['b']);
    expect(searchBlocks(root, 5, 35).map(b => b.textContent)).toEqual(['a', 'b', 'c']);
    expect(searchBlocks(root, 20, null).map(b => b.textContent)).toEqual(['b']);
    expect(searchBlocks(root, 100, 120)).toEqual([]);
    root.querySelectorAll('[data-source-end]').forEach(b => b.removeAttribute('data-source-end'));
    expect(searchBlocks(root, 10, 30).map(b => b.textContent)).toEqual(['b']);
    expect(searchBlocks(document.createElement('article'), 0)).toEqual([]);
  });
  it('walks only selected blocks, handles split inline text, and preserves anchors/buttons', () => {
    const root = document.createElement('article');
    root.innerHTML = '<div data-source-start="0" data-source-end="10">Überprüfung räumlich</div><div data-source-start="10" data-source-end="30"><p>😀 <a href="guide.md"><em>Üb</em>erprüfung</a> räumlich</p><button>Überprüfung</button><span hidden>räumlich</span></div><div data-source-start="30">Überprüfung</div>';
    const blocks = searchBlocks(root, 10, 30), link = root.querySelector('a'), button = root.querySelector('button');
    const walker = vi.spyOn(document, 'createTreeWalker');
    const marks = markSearchBlocks(blocks, ['uberprufung', 'raumlich']);
    expect(walker.mock.calls.filter(call => call[1] === NodeFilter.SHOW_TEXT)).toEqual([[blocks[0], NodeFilter.SHOW_TEXT]]);
    expect(marks.map(m => m.textContent)).toEqual(['Üb', 'erprüfung', 'räumlich']);
    expect(root.querySelectorAll('button mark, [hidden] mark, [data-source-start="0"] mark, [data-source-start="30"] mark')).toHaveLength(0);
    expect(root.querySelector('a')).toBe(link); expect(root.querySelector('button')).toBe(button);
    clearMarks(marks);
    expect(root.querySelector('mark')).toBeNull(); expect(link?.textContent).toBe('Überprüfung'); expect(link?.getAttribute('href')).toBe('guide.md');
  });
  it('clears either mark layer without destroying nested marks or inline elements', () => {
    const root = document.createElement('div');
    root.innerHTML = '<mark class="find-match"><mark class="search-match">word</mark></mark><mark class="search-match"><mark class="find-match">other</mark></mark>';
    clearMarks(root.querySelectorAll<HTMLElement>('.find-match'));
    expect(root.querySelectorAll('.search-match')).toHaveLength(2);
    const search = root.querySelector<HTMLElement>('.search-match')!;
    search.innerHTML = '<mark class="find-match">word</mark>';
    clearMarks(root.querySelectorAll<HTMLElement>('.search-match'));
    expect(root.querySelector('.find-match')?.textContent).toBe('word');
  });
  it('scrolls the first visible mark or the selected block when none is visible', () => {
    const hidden = document.createElement('mark'), visible = document.createElement('mark'), block = document.createElement('div');
    hidden.hidden = true;
    vi.spyOn(visible, 'getClientRects').mockReturnValue([new DOMRect(0, 0, 10, 10)] as unknown as DOMRectList);
    const first = visible.scrollIntoView = vi.fn(), fallback = block.scrollIntoView = vi.fn();
    scrollToSearchMatch([hidden, visible], block); expect(first).toHaveBeenCalledWith({ block: 'center' }); expect(fallback).not.toHaveBeenCalled();
    scrollToSearchMatch([], block); expect(fallback).toHaveBeenCalledWith({ block: 'center' });
  });
});
