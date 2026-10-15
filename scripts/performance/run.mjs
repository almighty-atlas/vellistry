import { createServer } from 'vite';
import { spawn } from 'node:child_process';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fixture, imageBytes } from './fixtures.mjs';
import { install } from './browser.mjs';

// Vite runs in a bounded, owned subprocess, never as a tool-launched server.
if (process.argv[2] === '--vite-worker') {
  const vite = await createServer({ server: { port: 1420, strictPort: true }, plugins: [{ name: 'performance-probes', enforce: 'pre', transform(code, id) {
    if (!id.endsWith('/App.svelte') && !id.endsWith('/reader-highlighting.ts')) return;
    return code.replace(/hljs\.highlight\(text, \{ language \}\)\.value/g, 'window.__measureHighlight(() => hljs.highlight(text, { language }).value)').replace(/hljs\.highlightAuto\(text\)\.value/g, 'window.__measureHighlight(() => hljs.highlightAuto(text).value)');
  } }] });
  await vite.listen(); process.send?.('ready');
  let closing = false;
  const close = async () => { if (closing) return; closing = true; await vite.close(); process.exit(0); };
  process.on('SIGTERM', close); process.on('disconnect', close);
} else {
const label = process.argv[2] ?? 'measurement';
const output = process.argv[3];
const temporary = await mkdtemp(join(tmpdir(), 'mdwell-performance-'));
let server, chrome, socket;
const watchdog = setTimeout(() => { console.error('Performance harness exceeded 120s'); chrome?.kill('SIGKILL'); server?.kill('SIGKILL'); process.exitCode = 1; socket?.close(); }, 120000);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const percentile = (values, p) => [...values].sort((a,b) => a-b)[Math.max(0, Math.ceil(values.length * p) - 1)] ?? 0;
function summarizeSweep(sweep, metrics) {
  const delta = field => Object.fromEntries(Object.keys(sweep.after[field]).map(key => [key, sweep.after[field][key] - sweep.before[field][key]]));
  return { listenerInvocations: metrics.scroll.length - sweep.before.handlers, layoutReads: delta('layouts'), queries: delta('queries'), handlerP95Ms: percentile(metrics.scroll.slice(sweep.before.handlers), .95), rafWorkP95Ms: percentile(metrics.rafWork.slice(sweep.before.raf), .95), frameP95Ms: percentile(metrics.frames.slice(sweep.before.frames), .95), longtasks: metrics.longtasks.filter(t => t.start >= sweep.before.time), calls: metrics.calls };
}
try {
  server = spawn(process.execPath, [import.meta.filename, '--vite-worker'], { stdio: ['ignore', 'inherit', 'inherit', 'ipc'] });
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('Vite did not start within 15s')), 15000);
    server.once('message', () => { clearTimeout(timeout); resolve(); });
    server.once('error', error => { clearTimeout(timeout); reject(error); });
    server.once('exit', code => { clearTimeout(timeout); reject(new Error(`Vite exited: ${code}`)); });
  });
  chrome = spawn('/usr/bin/chromium', ['--headless', '--no-sandbox', '--disable-dev-shm-usage', '--disable-background-timer-throttling', '--disable-renderer-backgrounding', '--remote-debugging-port=0', `--user-data-dir=${temporary}/chrome`, '--window-size=1440,1000', 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] });
  let endpoint = '';
  chrome.stderr.on('data', data => { const match = data.toString().match(/DevTools listening on (ws:\/\/\S+)/); if (match) endpoint = match[1]; });
  for (let i = 0; !endpoint && i < 100; i++) await delay(100);
  if (!endpoint) throw new Error('Chromium CDP endpoint unavailable');
  const port = new URL(endpoint).port;
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  socket = new WebSocket(targets.find(t => t.type === 'page').webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let sequence = 0; const pending = new Map();
  socket.onmessage = event => { const message = JSON.parse(event.data); const job = pending.get(message.id); if (job) { pending.delete(message.id); message.error ? job.reject(message.error) : job.resolve(message.result); } };
  socket.onclose = () => { for (const job of pending.values()) job.reject(new Error('CDP connection closed')); pending.clear(); };
  function send(method, params = {}) { return new Promise((resolve, reject) => {
    const id = ++sequence, deadline = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 30000);
    pending.set(id, { resolve: value => { clearTimeout(deadline); resolve(value); }, reject: error => { clearTimeout(deadline); reject(error); } });
    socket.send(JSON.stringify({ id, method, params }));
  }); }
  async function evaluate(expression) { const result = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }); if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails)); return result.result.value; }
  async function memorySnapshot() {
    // Adjacent target snapshots, not process RSS; leave GC on its normal schedule.
    const sampledAtMs = await evaluate('performance.now()');
    const heap = await send('Runtime.getHeapUsage');
    const dom = await send('Memory.getDOMCounters');
    return { sampledAtMs, heap, dom };
  }
  await send('Page.enable');
  const report = { label, chromium: (await send('Browser.getVersion')).product, node: process.version, viewport: '1440x1000', memoryMethodology: { heap: 'Runtime.getHeapUsage', dom: 'Memory.getDOMCounters', forcedGC: false }, scenarios: [] };
  for (const sections of [4, 300]) {
    const doc = fixture(sections);
    await writeFile(join(temporary, doc.path), doc.content);
    await writeFile(join(temporary, doc.path + '.html'), doc.html);
    const summaries = [doc, ...Array.from({ length: 1000 }, (_, i) => ({ workspace_id: 1, path: `library/folder-${i % 20}/note-${i}.md`, title: `Note ${i}`, revision: 'fixed-1' }))];
    const script = await send('Page.addScriptToEvaluateOnNewDocument', { source: `(${install.toString()})(${JSON.stringify(doc)},${JSON.stringify(summaries)},${JSON.stringify(imageBytes)})` });
    await send('Page.navigate', { url: 'http://localhost:1420/' });
    let ready = false;
    for (let i = 0; i < 300; i++) { await delay(100); ready = await evaluate('!!document.querySelector(".reader code")'); if (ready) break; }
    if (!ready) throw new Error('Reader failed to mount');
    await delay(6000); // includes at least one real 4s unchanged poll and delayed highlighting
    const initialMemory = await memorySnapshot();
    const load = await evaluate('({ highlights: __metrics.highlights, longtasks: __metrics.longtasks, calls: __metrics.calls, shifts: __metrics.shifts, blocks: document.querySelectorAll("pre code").length, highlighted: document.querySelectorAll("code .hljs-keyword").length, height: document.querySelector("article").getBoundingClientRect().height })');
    await evaluate('window.__maxScroll = document.querySelector(".reader").scrollHeight - document.querySelector(".reader").clientHeight');
    const sweep = await evaluate('__runScroll()');
    const metrics = await evaluate('__metrics');
    const headingChecks = await evaluate('__verifyHeadings()');
    const search = await evaluate('__verifySearch()');
    search.memory = await memorySnapshot();
    const searchSweep = await evaluate('__runScroll()');
    search.scroll = summarizeSweep(searchSweep, await evaluate('__metrics'));
    report.scenarios.push({ path: doc.path, lines: doc.content.split('\n').length, headings: sections, blocks: load.blocks, headingChecks, load: { memory: initialMemory, highlightJobs: load.highlights.length, highlightTotalMs: load.highlights.reduce((a,b)=>a+b,0), highlightP95Ms: percentile(load.highlights,.95), longtasks: load.longtasks, calls: load.calls, layoutShift: load.shifts.reduce((a,b)=>a+b,0), articleHeight: load.height }, scroll: summarizeSweep(sweep, metrics), search });
    await send('Page.removeScriptToEvaluateOnNewDocument', { identifier: script.identifier });
  }
  const json = JSON.stringify(report, null, 2); console.log(json); if (output) await writeFile(output, json + '\n');
} finally {
  clearTimeout(watchdog); socket?.close();
  if (chrome && chrome.exitCode === null) { chrome.kill('SIGTERM'); await Promise.race([new Promise(resolve => chrome.once('exit', resolve)), delay(2000)]); if (chrome.exitCode === null) chrome.kill('SIGKILL'); }
  if (server && server.exitCode === null) { server.kill('SIGTERM'); await Promise.race([new Promise(resolve => server.once('exit', resolve)), delay(2000)]); if (server.exitCode === null) server.kill('SIGKILL'); }
  await rm(temporary, { recursive: true, force: true });
}
}
