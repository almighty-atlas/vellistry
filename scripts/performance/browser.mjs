export function install(fixture, summaries, imageBytes) {
  localStorage.clear(); localStorage.setItem('mdwell.last', JSON.stringify({ workspace_id: 1, path: fixture.path }));
  const m = window.__metrics = { calls: {}, scroll: [], rafWork: [], layouts: { scroll: 0, raf: 0, other: 0 }, queries: { scroll: 0, raf: 0, other: 0 }, highlights: [], longtasks: [], frames: [], shifts: [], textWalks: [] };
  let phase = 'other';
  window.__measureHighlight = job => { const t = performance.now(); try { return job(); } finally { m.highlights.push(performance.now() - t); } };
  const rect = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function (...args) { m.layouts[phase]++; return rect.apply(this, args); };
  const query = Element.prototype.querySelectorAll;
  Element.prototype.querySelectorAll = function (...args) { m.queries[phase]++; return query.apply(this, args); };
  const walk = document.createTreeWalker.bind(document);
  document.createTreeWalker = (root, whatToShow, ...args) => {
    if (whatToShow === NodeFilter.SHOW_TEXT) m.textWalks.push({ tag: root.nodeName, sourceStart: root.dataset?.sourceStart ?? null });
    return walk(root, whatToShow, ...args);
  };
  const target = fixture.toc[Math.floor(fixture.toc.length / 2)];
  const hit = { workspace_id: fixture.workspace_id, path: fixture.path, title: fixture.title, score: -1, source_start: target.source_start, source_end: target.source_start + 1, section_title: target.text, matched_terms: ['section'], snippet: target.text };
  const add = EventTarget.prototype.addEventListener;
  EventTarget.prototype.addEventListener = function (type, fn, options) {
    if (type !== 'scroll' || typeof fn !== 'function') return add.call(this, type, fn, options);
    // Preserve listener identity for removeEventListener, including effect teardown.
    const wrapped = function (...args) { const old = phase, t = performance.now(); phase = 'scroll'; try { return fn.apply(this, args); } finally { phase = old; m.scroll.push(performance.now() - t); } };
    (this.__scrollWrappers ??= new Map()).set(fn, wrapped);
    return add.call(this, type, wrapped, options);
  };
  const remove = EventTarget.prototype.removeEventListener;
  EventTarget.prototype.removeEventListener = function (type, fn, options) { return remove.call(this, type, type === 'scroll' ? this.__scrollWrappers?.get(fn) ?? fn : fn, options); };
  const raf = window.requestAnimationFrame.bind(window);
  window.requestAnimationFrame = fn => raf(t => { const old = phase, start = performance.now(); phase = 'raf'; try { fn(t); } finally { phase = old; m.rafWork.push(performance.now() - start); } });
  new PerformanceObserver(list => m.longtasks.push(...list.getEntries().map(e => ({ start: e.startTime, duration: e.duration })))).observe({ type: 'longtask', buffered: true });
  new PerformanceObserver(list => m.shifts.push(...list.getEntries().filter(e => !e.hadRecentInput).map(e => e.value))).observe({ type: 'layout-shift', buffered: true });
  window.__TAURI_INTERNALS__ = { invoke: async (cmd, args) => {
    m.calls[cmd] = (m.calls[cmd] ?? 0) + 1;
    if (cmd === 'list_workspaces') return [{ id: 1, kind: 'local', name: 'Performance corpus', root: '/mock', url: null }];
    if (cmd === 'list_documents') return summaries;
    if (cmd === 'read_document') return fixture;
    if (cmd === 'search') return [hit];
    if (cmd === 'refresh_workspace') return { indexed: 0, unchanged: summaries.length, removed: 0 };
    if (cmd === 'resolve_link') return { kind: 'asset', path: 'diagram.png', fragment: null };
    if (cmd === 'read_asset') return { path: 'diagram.png', mime_type: 'image/png', bytes: imageBytes };
    throw new Error(`Harness does not implement ${cmd}`);
  } };
  window.__verifyHeadings = async () => {
    const reader = document.querySelector('.reader');
    const wait = () => new Promise(resolve => raf(() => raf(() => raf(resolve))));
    const checks = [];
    const verify = async fraction => {
      reader.scrollTop = (reader.scrollHeight - reader.clientHeight) * fraction; await wait();
      const top = reader.getBoundingClientRect().top + 80;
      const headings = [...document.querySelectorAll('article h2[id]')];
      const expected = (headings.filter(h => h.getBoundingClientRect().top <= top).at(-1) ?? headings[0]).id;
      const text = document.querySelector('.current-heading')?.textContent;
      if (text !== headings.find(h => h.id === expected).textContent) throw new Error(`Wrong active heading: ${text}, expected ${expected}`);
      checks.push({ fraction, expected });
    };
    for (const fraction of [0, 1, .25, .8, .1]) await verify(fraction);
    // Real settings UI changes layout; cache must update before another jump.
    [...document.querySelectorAll('button')].find(b => b.textContent.includes('Reading preferences')).click(); await wait();
    const input = document.querySelector('#font-size');
    input.value = '24'; input.dispatchEvent(new Event('input', { bubbles: true })); await wait();
    await verify(.6);
    input.value = '18'; input.dispatchEvent(new Event('input', { bubbles: true })); await wait();
    document.querySelector('[aria-label="Close dialog"]').click(); await wait(); await verify(.4);
    return checks;
  };
  window.__verifySearch = async () => {
    const wait = () => new Promise(resolve => raf(() => raf(() => raf(resolve))));
    const walks = m.textWalks.length;
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, shiftKey: true })); await wait();
    const input = document.querySelector('[aria-label="Search query"]');
    input.value = 'section'; input.dispatchEvent(new Event('input', { bubbles: true }));
    for (let i = 0; i < 40 && !document.querySelector('.search-result'); i++) await new Promise(resolve => setTimeout(resolve, 50));
    const result = document.querySelector('.search-result');
    if (!result) throw new Error('Search result did not render');
    result.click(); await wait();
    const marks = [...document.querySelectorAll('article .search-match')];
    if (marks.length !== 1 || marks[0].closest('[data-source-start]')?.dataset.sourceStart !== String(hit.source_start)) throw new Error('Search marks escaped selected block');
    const textWalks = m.textWalks.slice(walks);
    if (textWalks.length !== 1 || textWalks[0].sourceStart !== String(hit.source_start)) throw new Error('Search walked outside selected block');
    return { marks: marks.length, textWalks, readCalls: m.calls.read_document };
  };
  window.__runScroll = async () => {
    const reader = document.querySelector('.reader');
    const before = { layouts: structuredClone(m.layouts), queries: structuredClone(m.queries), handlers: m.scroll.length, raf: m.rafWork.length, frames: m.frames.length, time: performance.now() };
    let last;
    await new Promise(resolve => {
      let frame = 0;
      const step = t => {
        if (last !== undefined) m.frames.push(t - last); last = t;
        // Sweep down/up plus deterministic jumps; never read layout inside this loop.
        reader.scrollTop = (frame < 120 ? frame / 119 : (239 - frame) / 119) * window.__maxScroll;
        if (++frame < 240) raf(step); else resolve();
      }; raf(step);
    });
    await new Promise(resolve => setTimeout(resolve, 100));
    return { before, after: { layouts: structuredClone(m.layouts), queries: structuredClone(m.queries), time: performance.now() } };
  };
}
