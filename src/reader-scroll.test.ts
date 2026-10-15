import { afterEach, describe, expect, it, vi } from 'vitest';
import { headingAt, trackHeadings } from './reader-scroll';

afterEach(() => vi.unstubAllGlobals());
describe('heading cache', () => {
  it('handles empty, initial, exact, between, end and jumped positions', () => {
    const offsets = [{ id: 'a', top: 100 }, { id: 'b', top: 200 }, { id: 'c', top: 500 }];
    expect(headingAt([], 100)).toBe('');
    expect([-100, 100, 199, 200, 499, 500, 10000].map(top => headingAt(offsets, top))).toEqual(['a', 'a', 'a', 'b', 'b', 'c', 'c']);
  });
  it('coalesces scrolls without geometry/queries; rebuilds on resize/load and cleans up', () => {
    const callbacks = new Map<number, FrameRequestCallback>(); let sequence = 0;
    vi.stubGlobal('requestAnimationFrame', (fn: FrameRequestCallback) => { callbacks.set(++sequence, fn); return sequence; });
    vi.stubGlobal('cancelAnimationFrame', (id: number) => callbacks.delete(id));
    const flush = () => { const jobs = [...callbacks.values()]; callbacks.clear(); jobs.forEach(fn => fn(0)); };
    let resize!: ResizeObserverCallback;
    const disconnect = vi.fn();
    vi.stubGlobal('ResizeObserver', class { constructor(fn: ResizeObserverCallback) { resize = fn; } observe() {} disconnect = disconnect; });
    const reader = document.createElement('div'), article = document.createElement('article');
    article.innerHTML = '<h1 id="a">A</h1><h2 id="b">B</h2>'; reader.append(article);
    const headings = [...article.querySelectorAll<HTMLElement>('h1,h2')]; let secondTop = 250;
    vi.spyOn(reader, 'getBoundingClientRect').mockImplementation(() => ({ top: 10 } as DOMRect));
    vi.spyOn(headings[0], 'getBoundingClientRect').mockImplementation(() => ({ top: 100 - reader.scrollTop } as DOMRect));
    vi.spyOn(headings[1], 'getBoundingClientRect').mockImplementation(() => ({ top: secondTop - reader.scrollTop } as DOMRect));
    const query = vi.spyOn(article, 'querySelectorAll'), select = vi.fn();
    const tracker = trackHeadings(reader, article, select); flush(); flush();
    expect(select).toHaveBeenLastCalledWith('a');
    const geometryCalls = headings.map(h => vi.mocked(h.getBoundingClientRect).mock.calls.length);
    reader.scrollTop = 1000;
    reader.dispatchEvent(new Event('scroll')); reader.dispatchEvent(new Event('scroll'));
    expect(callbacks.size).toBe(1); flush();
    expect(select).toHaveBeenLastCalledWith('b');
    expect(headings.map(h => vi.mocked(h.getBoundingClientRect).mock.calls.length)).toEqual(geometryCalls);
    expect(query).toHaveBeenCalledTimes(1);
    reader.dispatchEvent(new Event('scroll')); flush(); expect(select).toHaveBeenCalledTimes(2);
    reader.scrollTop = 200; secondTop = 1000;
    resize([], {} as ResizeObserver); flush(); flush();
    expect(select).toHaveBeenLastCalledWith('a');
    article.dispatchEvent(new Event('load')); expect(callbacks.size).toBe(1);
    tracker.destroy(); expect(callbacks.size).toBe(0); expect(disconnect).toHaveBeenCalled();
    reader.dispatchEvent(new Event('scroll')); tracker.invalidate(); expect(callbacks.size).toBe(0);
  });
});
