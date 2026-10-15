import { afterEach, describe, expect, it, vi } from 'vitest';
import hljs from 'highlight.js/lib/common';
import { enhanceCode, highlightLanguage, MAX_HIGHLIGHT_CHARACTERS, MAX_HIGHLIGHT_LINES } from './reader-highlighting';

afterEach(() => vi.useRealTimers());
describe('scheduled code enhancement', () => {
  it('highlights only known, bounded explicit languages', () => {
    const code = document.createElement('code'); code.className = 'language-js';
    expect(highlightLanguage(code, 'const a = 1;')).toBe('js');
    expect(highlightLanguage(code, 'x'.repeat(MAX_HIGHLIGHT_CHARACTERS + 1))).toBeUndefined();
    expect(highlightLanguage(code, '\n'.repeat(MAX_HIGHLIGHT_LINES))).toBeUndefined();
    code.className = 'language-not-real'; expect(highlightLanguage(code, 'x')).toBeUndefined();
    code.className = ''; expect(highlightLanguage(code, 'x')).toBeUndefined();
  });
  it('yields first, never auto-detects, defers during scrolling and removes jobs/buttons/listeners', async () => {
    vi.useFakeTimers();
    const auto = vi.spyOn(hljs, 'highlightAuto');
    const reader = document.createElement('div'), root = document.createElement('article');
    root.innerHTML = '<pre><code class="language-js">const x = 1;</code></pre><pre><code>unknown</code></pre>';
    reader.append(root);
    const stop = enhanceCode(root, reader, vi.fn());
    expect(root.querySelector('.hljs-keyword')).toBeNull();
    reader.dispatchEvent(new Event('scroll'));
    await vi.advanceTimersByTimeAsync(16);
    expect(root.querySelector('.copy-code')).toBeNull();
    await vi.advanceTimersByTimeAsync(160);
    expect(root.querySelector('.hljs-keyword')).not.toBeNull();
    expect(auto).not.toHaveBeenCalled(); expect(root.querySelectorAll('.copy-code')).toHaveLength(2);
    (root.querySelector('button') as HTMLButtonElement).click(); await Promise.resolve();
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('const x = 1;');
    stop(); expect(root.querySelector('.copy-code')).toBeNull(); expect(vi.getTimerCount()).toBe(0);
    reader.dispatchEvent(new Event('scroll')); expect(vi.getTimerCount()).toBe(0);
  });
  it.each(['find-match', 'search-match'])('cancels before execution and preserves existing %s marks', async className => {
    vi.useFakeTimers();
    const root = document.createElement('article'), reader = document.createElement('div');
    root.innerHTML = `<pre><code class="language-js"><mark class="${className}">const</mark> x = 1;</code></pre>`;
    const stop = enhanceCode(root, reader, vi.fn()); stop(); await vi.runAllTimersAsync();
    expect(root.querySelector('button')).toBeNull();
    const cleanup = enhanceCode(root, reader, vi.fn()); await vi.runAllTimersAsync();
    expect(root.querySelector('mark')).not.toBeNull(); expect(root.querySelector('.hljs-keyword')).toBeNull(); cleanup();
  });
});
