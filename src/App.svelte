<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { open as openDirectory } from '@tauri-apps/plugin-dialog';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { enhanceCode } from './reader-highlighting';
  import { trackHeadings } from './reader-scroll';
  import { clearMarks, markSearchBlocks, scrollToSearchMatch, searchBlocks, textSegments } from './reader-search';
  import type Editor from './Editor.svelte';
  import FileTree from './FileTree.svelte';
  import GithubAccount from './GithubAccount.svelte';
  import { api } from './api';
  import { assetObjectUrl, enhanceLocalImages } from './images';
  import { errorMessage, fileTree, gitGuides, isConflict, isWebUrl, readStored, sameSummaries, store } from './helpers';
  import type { Document, DocumentSummary, SearchHit, Workspace } from './types';

  type Location = { workspace_id: number; path: string; offset?: number; fragment?: string; source_end?: number | null; matched_terms?: string[] };
  type Preferences = { theme: 'system' | 'light' | 'dark'; font: 'serif' | 'sans'; size: number; sidebar: boolean };
  const saved = readStored<Partial<Preferences>>('mdwell.preferences', {});
  let preferences = $state<Preferences>({ theme: ['system', 'light', 'dark'].includes(saved?.theme ?? '') ? saved.theme! : 'system', font: saved?.font === 'sans' ? 'sans' : 'serif', size: typeof saved?.size === 'number' ? Math.max(14, Math.min(24, saved.size)) : 18, sidebar: saved?.sidebar !== false });
  let workspaces = $state<Workspace[]>([]);
  let documents = $state<Record<number, DocumentSummary[]>>({});
  let trees = $state<Record<number, ReturnType<typeof fileTree>>>({});
  let headingTracker: ReturnType<typeof trackHeadings> | undefined;
  let current = $state<Document | null>(null);
  let draft = $state('');
  let source = $state(false);
  let sourceOffset = $state(0);
  let editor = $state<Editor>();
  let SourceEditor = $state<typeof Editor>();
  let reader = $state<HTMLDivElement>();
  let article = $state<HTMLElement>();
  let dirty = $derived(current !== null && draft !== current.content);
  let saving = $state(false);
  let loading = $state(false);
  let busy = $state(false);
  let initialized = $state(false);
  let error = $state('');
  let notice = $state('');
  let conflict = $state(false);
  let history = $state<Location[]>([]);
  let historyIndex = $state(-1);
  let navigation = 0;
  let searchMarks: HTMLElement[] = [];
  let polling = false;
  let scrolls = readStored<Record<string, number>>('mdwell.scrolls', {});
  if (!scrolls || typeof scrolls !== 'object' || Array.isArray(scrolls)) scrolls = {};
  let modal = $state<'commands' | 'search' | 'github' | 'settings' | 'toc' | 'asset' | null>(null);
  let assetUrl = $state('');
  let assetPath = $state('');
  let dialog = $state<HTMLDialogElement>();
  let previousFocus: HTMLElement | null = null;
  let guard = $state<{ message: string; resolve: (discard: boolean) => void } | null>(null);
  let guardDialog = $state<HTMLDialogElement>();
  let guardFocus: HTMLElement | null = null;
  let query = $state('');
  let searchWorkspace = $state<number | null>(null);
  const storedLocalEdits = readStored<Record<number, boolean>>('mdwell.githubLocalEdits', {});
  let githubLocalEdits = $state<Record<number, boolean>>(storedLocalEdits && typeof storedLocalEdits === 'object' && !Array.isArray(storedLocalEdits) ? storedLocalEdits : {});
  let githubUrl = $state('');
  let hits = $state<SearchHit[]>([]);
  let searching = $state(false);
  let searchError = $state('');
  let activeResult = $state(0);
  let findQuery = $state('');
  let finding = $state(false);
  let findIndex = $state(0);
  let findCount = $state(0);
  let findInput = $state<HTMLInputElement>();
  let findTimer: ReturnType<typeof setTimeout> | undefined;
  let activeHeading = $state('');
  const workspace = $derived(workspaces.find(w => w.id === current?.workspace_id));
  const localUnpublished = $derived(workspace?.kind === 'github' && (dirty || githubLocalEdits[workspace.id] === true));
  const commands = $derived([
    { name: 'Search all workspaces', shortcut: 'Ctrl ⇧ F', run: () => showModal('search') },
    { name: 'Open local folder', shortcut: '', run: () => { closeModal(); void addLocal(); } },
    { name: 'Add GitHub repository', shortcut: '', run: () => showModal('github') },
    { name: source ? 'Switch to reading' : 'Edit Markdown source', shortcut: '', run: () => { closeModal(); if (current) switchMode(); } },
    { name: 'Find in document', shortcut: 'Ctrl F', run: () => { closeModal(); void find(); } },
    { name: 'Refresh workspace', shortcut: '', run: () => { closeModal(); void refresh(false); } },
    { name: 'Sync GitHub workspace', shortcut: '', run: () => { closeModal(); void refresh(true); } },
    { name: 'Reading preferences', shortcut: '', run: () => showModal('settings') },
    { name: preferences.sidebar ? 'Hide sidebar' : 'Show sidebar', shortcut: '', run: () => { closeModal(); preferences.sidebar = !preferences.sidebar; } },
  ].filter(c => c.name.toLowerCase().includes(query.toLowerCase())));

  $effect(() => { store('mdwell.preferences', preferences); document.documentElement.dataset.theme = preferences.theme; });
  $effect(() => {
    if (source && !SourceEditor) void import('./Editor.svelte').then(module => { SourceEditor = module.default; }).catch(e => { error = errorMessage(e); });
  });
  $effect(() => {
    if (!dialog) return;
    if (modal && !dialog.open) dialog.showModal();
    if (modal) void tick().then(() => dialog?.querySelector<HTMLInputElement | HTMLSelectElement>('input, select')?.focus());
    if (!modal && dialog.open) { dialog.close(); previousFocus?.focus(); }
  });
  $effect(() => {
    if (!guardDialog) return;
    if (guard && !guardDialog.open) {
      guardDialog.showModal();
      void tick().then(() => guardDialog?.querySelector<HTMLButtonElement>('button')?.focus());
    } else if (!guard && guardDialog.open) { guardDialog.close(); guardFocus?.focus(); }
  });
  $effect(() => {
    const value = query.trim();
    const scope = searchWorkspace;
    if (modal !== 'search') return;
    let cancelled = false;
    hits = []; activeResult = 0; searchError = ''; searching = !!value;
    const timer = setTimeout(async () => {
      if (!value) return;
      try { const results = await api.search(value, scope); if (!cancelled) hits = results; }
      catch (e) { if (!cancelled) searchError = errorMessage(e); }
      finally { if (!cancelled) searching = false; }
    }, 180);
    return () => { cancelled = true; clearTimeout(timer); };
  });
  $effect(() => {
    const doc = current, root = article, scroller = reader;
    if (source || !doc || !root || !scroller) return;
    // Read identity/html explicitly; helper callbacks must not become reactive dependencies.
    void doc.html;
    return untrack(() => enhanceCode(root, scroller, e => { error = `Could not copy: ${errorMessage(e)}`; }));
  });
  $effect(() => {
    const doc = current, root = article, scroller = reader;
    if (source || !doc || !root || !scroller) return;
    void doc.html;
    const tracker = untrack(() => trackHeadings(scroller, root, id => { activeHeading = id; }));
    headingTracker = tracker;
    return () => { tracker.destroy(); if (headingTracker === tracker) headingTracker = undefined; };
  });
  $effect(() => {
    void preferences.size; void preferences.font; void preferences.sidebar;
    void tick().then(() => headingTracker?.invalidate());
  });
  $effect(() => {
    const doc = current;
    const root = article;
    if (source || !doc || !root) return;
    return untrack(() => enhanceLocalImages(root, { workspace_id: doc.workspace_id, path: doc.path }, () => current === doc && article === root && !source));
  });
  $effect(() => {
    const q = findQuery, doc = current, root = article, mode = source;
    const enabled = finding && !mode; void doc?.html;
    if (!root) return;
    let cancelled = false;
    const timer = setTimeout(async () => {
      await tick();
      if (!cancelled && findTimer === timer && current === doc && article === root && source === mode) {
        findTimer = undefined; markMatches(enabled ? q : '');
      }
    }, enabled && q.trim() ? 150 : 0);
    findTimer = timer;
    return () => { cancelled = true; clearTimeout(timer); if (findTimer === timer) findTimer = undefined; };
  });

  function key(location: Location) { return `${location.workspace_id}:${location.path}`; }
  function rememberScroll() {
    if (current && reader && !source) { scrolls[key(current)] = reader.scrollTop; store('mdwell.scrolls', scrolls); }
  }
  function askDiscard(message: string): Promise<boolean> {
    if (guard) return Promise.resolve(false);
    guardFocus = document.activeElement as HTMLElement;
    return new Promise(resolve => { guard = { message, resolve }; });
  }
  function finishGuard(discard: boolean) { const pending = guard; guard = null; pending?.resolve(discard); }
  async function canLeave() { return !saving && (!dirty || await askDiscard('Discard your unsaved Markdown changes?')); }
  function showModal(value: typeof modal) {
    if (!modal) previousFocus = document.activeElement as HTMLElement;
    if (value !== 'asset' && assetUrl) { URL.revokeObjectURL(assetUrl); assetUrl = ''; }
    query = ''; activeResult = 0; modal = value;
  }
  function closeModal() { modal = null; if (assetUrl) { URL.revokeObjectURL(assetUrl); assetUrl = ''; } }
  function notify(message: string) { notice = message; }
  function updateDocuments(id: number, list: DocumentSummary[]) {
    if (sameSummaries(documents[id], list)) return;
    documents[id] = list;
    trees[id] = fileTree(list);
  }
  async function loadWorkspaces() {
    const list = await api.workspaces();
    const entries = await Promise.all(list.map(async w => [w.id, await api.documents(w.id)] as const));
    workspaces = list;
    for (const [id, summaries] of entries) updateDocuments(id, summaries);
  }
  async function navigate(location: Location, historyTarget?: number, bypassGuard = false) {
    const same = current && key(current) === key(location);
    if (!same && !bypassGuard && !await canLeave()) return false;
    const acceptedDraft = draft;
    if (same) {
      const token = ++navigation; loading = false;
      if (historyTarget !== undefined) historyIndex = historyTarget;
      else if (location.offset !== undefined || location.fragment !== undefined) recordHistory(location);
      await tick(); if (token !== navigation) return false; jump(location); return true;
    }
    rememberScroll();
    const token = ++navigation;
    loading = true; error = '';
    try {
      const doc = await api.read(location.workspace_id, location.path);
      if (token !== navigation) return false;
      // A user can type while a backend read is pending. Never silently discard that edit.
      if (saving || (!bypassGuard && dirty && draft !== acceptedDraft && !await canLeave())) return false;
      if (modal === 'asset') closeModal();
      current = doc; draft = doc.content; conflict = false; sourceOffset = location.offset ?? 0; activeHeading = ''; finding = false;
      if (historyTarget !== undefined) historyIndex = historyTarget;
      else recordHistory(location);
      store('mdwell.last', { workspace_id: doc.workspace_id, path: doc.path });
      await tick();
      if (token !== navigation) return false;
      if (reader) reader.scrollTop = typeof scrolls[key(doc)] === 'number' ? scrolls[key(doc)] : 0;
      jump(location);
      return true;
    } catch (e) { if (token === navigation) error = errorMessage(e); return false; }
    finally { if (token === navigation) loading = false; }
  }
  function recordHistory(location: Location) {
    history = [...history.slice(0, historyIndex + 1), location]; historyIndex = history.length - 1;
  }
  function jump(location: Location) {
    clearMarks(searchMarks); searchMarks = [];
    if (source) {
      const offset = location.offset ?? current?.toc.find(h => h.id === location.fragment)?.source_start;
      if (offset !== undefined) { sourceOffset = offset; editor?.goTo(offset); }
      return;
    }
    if (!article) return;
    if (location.matched_terms !== undefined && location.offset !== undefined) {
      const blocks = searchBlocks(article, location.offset, location.source_end);
      searchMarks = markSearchBlocks(blocks, location.matched_terms);
      scrollToSearchMatch(searchMarks, blocks[0] ?? searchBlocks(article, location.offset)[0]);
      return;
    }
    if (location.fragment) {
      let fragment = location.fragment;
      try { fragment = decodeURIComponent(fragment); } catch { /* Use literal fragment. */ }
      const heading = [...article.querySelectorAll<HTMLElement>('[id]')].find(e => e.id === fragment);
      heading?.scrollIntoView({ block: 'start' });
    } else if (location.offset !== undefined) {
      const blocks = [...article.querySelectorAll<HTMLElement>('[data-source-start]')];
      const block = blocks.filter(b => Number(b.dataset.sourceStart) <= location.offset!).at(-1) ?? blocks[0];
      block?.scrollIntoView({ block: 'center' });
    }
  }
  async function back(direction: number) {
    const index = historyIndex + direction;
    if (history[index]) await navigate(history[index], index);
  }
  async function addLocal() {
    if (busy) return;
    busy = true; error = '';
    try {
      const path = await openDirectory({ directory: true, multiple: false, title: 'Open a Markdown workspace' });
      if (typeof path === 'string') { const w = await api.addLocal(path); await loadWorkspaces(); notify(`Opened ${w.name}`); }
    } catch (e) { error = errorMessage(e); } finally { busy = false; }
  }
  async function githubAdded(w: Workspace) {
    await loadWorkspaces(); closeModal(); githubUrl = ''; notify(`Added ${w.name}`);
    const first = documents[w.id]?.[0];
    if (first) await navigate({ workspace_id: w.id, path: first.path });
  }
  async function addGithub(event: SubmitEvent) {
    event.preventDefault(); if (busy) return;
    busy = true; error = '';
    try { await githubAdded(await api.addGithub(githubUrl.trim())); }
    catch (e) { error = errorMessage(e); } finally { busy = false; }
  }
  async function refresh(sync: boolean) {
    if (!workspace || busy) return;
    if (sync && workspace.kind !== 'github') { notify('This is a local workspace; use Refresh instead.'); return; }
    if (sync && dirty) { error = 'Save or discard your source changes before syncing.'; return; }
    busy = true; error = '';
    try {
      const report = await (sync ? api.sync(workspace.id) : api.refresh(workspace.id));
      await poll(); notify(`${sync ? 'Synced' : 'Refreshed'} · ${report.indexed} indexed, ${report.removed} removed`);
    } catch (e) { error = errorMessage(e); } finally { busy = false; }
  }
  async function save() {
    if (!current || !dirty || saving) return;
    saving = true; error = ''; conflict = false;
    const doc = current, content = draft;
    try {
      const updated = await api.save(doc.workspace_id, doc.path, content, doc.revision);
      current = updated;
      if (workspaces.find(w => w.id === doc.workspace_id)?.kind === 'github') {
        githubLocalEdits[doc.workspace_id] = true;
        store('mdwell.githubLocalEdits', githubLocalEdits);
      }
      // Keep any edits typed while the save was in flight.
      if (draft === content) draft = updated.content;
      updateDocuments(doc.workspace_id, await api.documents(doc.workspace_id));
      notify(githubLocalEdits[doc.workspace_id] ? 'Saved locally · not published to GitHub' : 'Saved to workspace');
    } catch (e) { conflict = isConflict(e); error = errorMessage(e); }
    finally { saving = false; }
  }
  async function reloadConflict() {
    if (!current || saving || !await askDiscard('Reload the file from disk and discard your unsaved changes?')) return;
    const doc = current, acceptedDraft = draft; const token = ++navigation; loading = true;
    try {
      const updated = await api.read(doc.workspace_id, doc.path);
      if (token !== navigation || saving || (draft !== acceptedDraft && !await canLeave())) return;
      current = updated; draft = updated.content; conflict = false; error = ''; sourceOffset = 0;
    } catch (e) { error = errorMessage(e); } finally { if (token === navigation) loading = false; }
  }
  async function copyDraft() { try { await navigator.clipboard.writeText(draft); notify('Your draft is on the clipboard'); } catch (e) { error = errorMessage(e); } }
  async function poll() {
    if (polling || loading || saving || !initialized) return;
    polling = true;
    const doc = current, token = navigation;
    try {
      // Wait for every workspace even on failure, so the next tick cannot overlap a scan.
      const results = await Promise.allSettled(workspaces.map(async w => {
        try {
          await api.refresh(w.id);
          return [w.id, await api.documents(w.id)] as const;
        } catch (e) { throw new Error(`${w.name}: ${errorMessage(e)}`); }
      }));
      const entries = results.flatMap(result => result.status === 'fulfilled' ? [result.value] : []);
      for (const [id, summaries] of entries) updateDocuments(id, summaries);
      const failure = results.find(result => result.status === 'rejected');
      if (failure?.status === 'rejected') throw failure.reason;
      const indexed = doc && entries.find(([id]) => id === doc.workspace_id)?.[1].find(summary => summary.path === doc.path);
      if (doc && token === navigation && current === doc && !dirty && !saving && indexed && indexed.revision !== doc.revision) {
        const updated = await api.read(doc.workspace_id, doc.path);
        if (token === navigation && current === doc && !dirty && !saving && updated.revision !== doc.revision) {
          rememberScroll(); current = updated; draft = updated.content;
          await tick(); if (reader) reader.scrollTop = scrolls[key(doc)] ?? 0;
          notify('Updated from disk');
        }
      }
    } catch (e) { error = errorMessage(e); } finally { polling = false; }
  }
  async function followLink(event: MouseEvent) {
    if (event.type === 'auxclick' && event.button !== 1) return;
    const anchor = (event.target as Element).closest('a');
    if (!anchor) return;
    event.preventDefault();
    if (!current) return;
    const href = anchor.getAttribute('href'); if (!href) return;
    const doc = current, token = navigation;
    const active = () => current === doc && token === navigation;
    try {
      const result = await api.resolve(doc.workspace_id, doc.path, href);
      if (!active()) return;
      if (result.kind === 'external') {
        if (!isWebUrl(result.url)) throw new Error('Only explicit HTTP and HTTPS links can be opened.');
        await openUrl(result.url);
      } else if (result.kind === 'document') await navigate({ workspace_id: doc.workspace_id, path: result.path, fragment: result.fragment ?? undefined });
      else {
        const asset = await api.asset(doc.workspace_id, result.path);
        if (!active()) return;
        const url = assetObjectUrl(asset);
        closeModal();
        assetUrl = url;
        assetPath = asset.path; showModal('asset');
      }
    } catch (e) { if (active()) error = errorMessage(e); }
  }
  function inspectSource(event: MouseEvent) {
    if (!current || (event.target as Element).closest('a, button')) return;
    const block = (event.target as Element).closest<HTMLElement>('[data-source-start]');
    sourceOffset = Number(block?.dataset.sourceStart ?? 0); source = true;
  }
  function switchMode() {
    if (!source && article && reader) {
      const top = reader.getBoundingClientRect().top;
      const block = [...article.querySelectorAll<HTMLElement>('[data-source-start]')].find(b => b.getBoundingClientRect().bottom >= top);
      sourceOffset = Number(block?.dataset.sourceStart ?? 0);
      rememberScroll();
    }
    source = !source;
    if (!source) void tick().then(() => { if (reader && current) reader.scrollTop = scrolls[key(current)] ?? 0; });
  }
  async function find() {
    if (!current) return;
    if (source) { editor?.find(); return; }
    finding = true; await tick(); findInput?.focus(); findInput?.select();
  }
  function markMatches(value: string) {
    if (!article) return;
    clearMarks(article.querySelectorAll<HTMLElement>('mark.find-match'));
    findCount = 0; findIndex = 0;
    if (!value.trim()) return;
    const walker = document.createTreeWalker(article, NodeFilter.SHOW_TEXT);
    const nodes: Text[] = [];
    while (walker.nextNode()) { const node = walker.currentNode as Text; if (!node.parentElement?.closest('button')) nodes.push(node); }
    for (const node of nodes) {
      const text = node.data; const lower = text.toLocaleLowerCase(), needle = value.toLocaleLowerCase();
      let start = 0, index = lower.indexOf(needle);
      if (index < 0) continue;
      const fragment = document.createDocumentFragment();
      while (index >= 0) {
        fragment.append(text.slice(start, index)); const mark = document.createElement('mark'); mark.className = 'find-match'; mark.textContent = text.slice(index, index + value.length); fragment.append(mark); findCount++;
        start = index + value.length; index = lower.indexOf(needle, start);
      }
      fragment.append(text.slice(start)); node.replaceWith(fragment);
    }
    focusMatch(0);
  }
  function nextFind(direction: number) {
    if (findTimer !== undefined) {
      clearTimeout(findTimer); findTimer = undefined; markMatches(findQuery);
      if (direction < 0) focusMatch(-1);
    } else focusMatch(findIndex + direction);
  }
  function focusMatch(index: number) {
    if (!article || !findCount) return;
    findIndex = (index + findCount) % findCount;
    const marks = article.querySelectorAll<HTMLElement>('mark.find-match'); marks.forEach((m, i) => m.classList.toggle('active-match', i === findIndex)); marks[findIndex]?.scrollIntoView({ block: 'center' });
  }

  function keydown(event: KeyboardEvent) {
    if (guard) return;
    const mod = event.ctrlKey || event.metaKey;
    if (mod && event.key.toLowerCase() === 'k') { event.preventDefault(); modal ? closeModal() : showModal('commands'); }
    else if (mod && event.key.toLowerCase() === 'f') { event.preventDefault(); if (event.shiftKey) showModal('search'); else void find(); }
    else if (mod && event.key.toLowerCase() === 's') { event.preventDefault(); void save(); }
    else if (event.key === 'Escape' && finding) finding = false;
  }
  function resultKey(event: KeyboardEvent) {
    const length = modal === 'search' ? hits.length : commands.length;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); activeResult = length ? (activeResult + (event.key === 'ArrowDown' ? 1 : -1) + length) % length : 0; }
    else if (event.key === 'Enter') {
      event.preventDefault(); if (modal === 'commands') commands[activeResult]?.run(); else if (hits[activeResult]) chooseHit(hits[activeResult]);
    }
  }
  function chooseHit(hit: SearchHit) {
    closeModal(); finding = false;
    void navigate({ workspace_id: hit.workspace_id, path: hit.path, offset: hit.source_start, source_end: hit.source_end, matched_terms: [...(hit.matched_terms ?? [])] });
  }
  onMount(() => {
    let alive = true;
    void (async () => {
      try {
        await loadWorkspaces(); if (!alive) return;
        const last = readStored<Location | null>('mdwell.last', null);
        if (last && workspaces.some(w => w.id === last.workspace_id) && documents[last.workspace_id]?.some(d => d.path === last.path)) await navigate(last);
      } catch (e) { error = `Could not connect to the desktop backend: ${errorMessage(e)}`; }
      finally { initialized = true; }
    })();
    const timer = setInterval(() => { if (!document.hidden) void poll(); }, 4000);
    const unload = (e: BeforeUnloadEvent) => { rememberScroll(); if (dirty || saving) { e.preventDefault(); e.returnValue = ''; } };
    window.addEventListener('beforeunload', unload);
    // Native close requests do not consistently dispatch beforeunload in every webview.
    let unlisten: (() => void) | undefined;
    void import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
      const nativeWindow = getCurrentWindow();
      const stop = await nativeWindow.onCloseRequested(e => {
        rememberScroll();
        if (dirty || saving) {
          e.preventDefault();
          if (!saving) void canLeave().then(discard => { if (discard) return nativeWindow.destroy(); }).catch(e => { error = errorMessage(e); });
        }
      });
      if (!alive) stop(); else unlisten = stop;
    }).catch(() => { /* Browser preview has no native window. */ });
    return () => { alive = false; clearMarks(searchMarks); searchMarks = []; clearInterval(timer); window.removeEventListener('beforeunload', unload); unlisten?.(); rememberScroll(); if (assetUrl) URL.revokeObjectURL(assetUrl); finishGuard(false); };
  });
</script>

<svelte:window onkeydown={keydown} />
<div class="app" class:sidebar-hidden={!preferences.sidebar} style:--reading-size={`${preferences.size}px`} class:sans={preferences.font === 'sans'}>
  {#if preferences.sidebar}
    <aside class="sidebar" aria-label="Workspaces">
      <div class="brand"><span class="brand-mark" aria-hidden="true">v.</span><strong>Vellistry</strong><button class="icon-button" aria-label="Hide sidebar" onclick={() => preferences.sidebar = false}>⇤</button></div>
      <button class="search-launch" onclick={() => showModal('search')}><span>⌕ &nbsp; Search your library</span><kbd>Ctrl ⇧ F</kbd></button>
      <div class="sidebar-label">WORKSPACES <button class="icon-button" aria-label="Add workspace" onclick={() => showModal('commands')}>＋</button></div>
      <nav class="workspace-list" aria-label="Document library">
        {#each workspaces as w (w.id)}
          <details class="workspace" open>
            <summary><span aria-hidden="true">{w.kind === 'github' ? '◈' : '▱'}</span><strong>{w.name}</strong><span class="count">{documents[w.id]?.length ?? 0}</span></summary>
            <FileTree nodes={trees[w.id] ?? []} selected={current?.workspace_id === w.id ? current.path : undefined} open={d => { void navigate(d); }} />
            {#if !documents[w.id]?.length}<p class="empty-tree">No Markdown documents</p>{/if}
          </details>
        {/each}
        {#if initialized && !workspaces.length}<p class="empty-tree">Your folders and repositories will live here.</p>{/if}
      </nav>
      <div class="sidebar-bottom"><button onclick={() => { void addLocal(); }} disabled={busy}>＋ Open folder</button><button onclick={() => showModal('github')} disabled={busy}>◈ Add GitHub repository</button><button onclick={() => showModal('settings')}>⚙ Reading preferences</button><p>Local first. A little more quiet.</p></div>
    </aside>
  {/if}
  <main>
    <header class="toolbar">
      <div class="toolbar-left">
        {#if !preferences.sidebar}<button class="icon-button" aria-label="Show sidebar" onclick={() => preferences.sidebar = true}>☰</button>{/if}
        <button class="icon-button" aria-label="Back" disabled={historyIndex <= 0 || saving} onclick={() => { void back(-1); }}>←</button>
        <button class="icon-button" aria-label="Forward" disabled={historyIndex >= history.length - 1 || saving} onclick={() => { void back(1); }}>→</button>
        <div class="breadcrumb">{#if current}<span>{workspace?.name}</span><span class="slash">/</span><span title={current.path}>{current.path}</span>{:else}<span>Your reading space</span>{/if}</div>
      </div>
      <div class="toolbar-actions">
        {#if current}<button onclick={() => { void refresh(false); }} disabled={busy} title="Refresh from disk">↻</button>{#if workspace?.kind === 'github'}<button onclick={() => { void refresh(true); }} disabled={busy || dirty}>Sync</button>{/if}<button class:active={source} onclick={switchMode}>{source ? 'Read' : 'Source'}</button>{#if current.toc.length}<button class="contents-toggle" aria-label="Table of contents" onclick={() => showModal('toc')}>☷</button>{/if}{/if}
        <button aria-label="Command palette" title="Command palette (Ctrl K)" onclick={() => showModal('commands')}>⌘</button>
      </div>
    </header>
    {#if error}<div class="message error" role="alert"><span>{error}</span><button aria-label="Dismiss error" onclick={() => error = ''}>×</button></div>{/if}
    {#if localUnpublished}<div class="message unpublished-message" role="status"><span class="unpublished-badge">Local · unpublished</span><span>GitHub edits stay in this local checkout. Save does not commit or push them.</span></div>{/if}
    {#if conflict}<div class="conflict" role="alert"><strong>This file changed on disk.</strong><span>Your draft is safe. Copy it before reloading and merging; saves never force-overwrite another revision.</span><button onclick={() => { void copyDraft(); }}>Copy draft</button><button onclick={() => { void reloadConflict(); }}>Reload disk version</button></div>{/if}
    {#if current}
      {#if source}
        <div class="source-bar"><span>MARKDOWN SOURCE <span class="muted">· {dirty ? 'Unsaved changes' : 'All changes saved'}</span></span><div><button onclick={() => { void copyDraft(); }}>Copy draft</button><button class="primary" disabled={!dirty || saving || loading} onclick={() => { void save(); }}>{saving ? 'Saving…' : 'Save'} <kbd>Ctrl S</kbd></button></div></div>
        {#if SourceEditor}{#key key(current)}<SourceEditor bind:this={editor} content={draft} offset={sourceOffset} onchange={value => draft = value} onsave={() => { void save(); }} />{/key}{:else}<p class="empty-tree" role="status">Loading source editor…</p>{/if}
      {:else}
        {#if dirty}<div class="message" role="status"><span>Showing the last saved version. Your unsaved draft is preserved in Source.</span><button onclick={switchMode}>Return to draft</button></div>{/if}
                {#if finding}<div class="find-bar"><label for="find">Find</label><input id="find" bind:this={findInput} bind:value={findQuery} onkeydown={e => { if (e.key === 'Enter') { e.preventDefault(); nextFind(e.shiftKey ? -1 : 1); } }} /><span>{findCount ? findIndex + 1 : 0} / {findCount}</span><button aria-label="Previous match" onclick={() => nextFind(-1)}>↑</button><button aria-label="Next match" onclick={() => nextFind(1)}>↓</button><button aria-label="Close find" onclick={() => finding = false}>×</button></div>{/if}
        <div class="reading-layout">
          <div class="reader" bind:this={reader}>
            <div class="document-meta"><span>{workspace?.kind === 'github' ? 'GITHUB WORKSPACE' : 'LOCAL WORKSPACE'}</span><span>{current.content.trim() ? Math.max(1, Math.ceil(current.content.trim().split(/\s+/).length / 220)) : 0} MIN READ</span></div>
            <!-- Delegated click handles native anchor activation, including Enter. Source editing also has a toolbar button. -->
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
            <article class="markdown" bind:this={article} onclick={followLink} onauxclick={followLink} ondblclick={inspectSource}>{@html current.html}</article>
            <footer class="document-footer"><span>End of document</span><span>Double-click a block to edit its source</span></footer>
          </div>
          {#if current.toc.length}<aside class="toc" aria-label="Table of contents"><h2>ON THIS PAGE</h2><nav>{#each current.toc as heading}<button class:current-heading={activeHeading === heading.id} style:padding-left={`${Math.max(0, heading.level - 1) * 12}px`} onclick={() => jump({ ...current!, fragment: heading.id })}>{heading.text}</button>{/each}</nav><p>Relative images load from workspace bytes. Remote images are never fetched.</p></aside>{/if}
        </div>
      {/if}
    {:else}
      <section class="welcome"><div class="eyebrow">A QUIET PLACE FOR MARKDOWN</div><h1>Less noise.<br />More reading.</h1><p>Your notes, documentation, and ideas.<br />Beautifully readable. Right where they are.</p><div class="welcome-actions"><button class="primary" disabled={busy} onclick={() => { void addLocal(); }}>Open a local folder <span>↗</span></button><button disabled={busy} onclick={() => showModal('github')}>Add a GitHub repository</button></div><div class="welcome-note"><span>▱ Files stay yours</span><span>⌕ Search across workspaces</span><span>◌ No remote image requests</span></div><small>{initialized ? 'Select a document in the sidebar to begin.' : 'Connecting to your library…'}</small></section>
    {/if}
    <footer class="statusbar"><span>{loading ? 'Opening document…' : busy ? 'Working…' : source && dirty ? '● Unsaved changes' : '● Local first'}{current ? ` · ${current.path}` : ''}</span><span role="status" aria-live="polite">{notice}</span><button onclick={() => showModal('commands')}>Ctrl K <span>commands</span></button></footer>
  </main>
</div>

<dialog bind:this={dialog} oncancel={() => closeModal()} onclose={() => { if (modal) closeModal(); }} aria-labelledby="dialog-title">
  <div class="dialog-heading"><h2 id="dialog-title">{modal === 'commands' ? 'Go anywhere' : modal === 'search' ? 'Search your library' : modal === 'github' ? 'Add a GitHub repository' : modal === 'toc' ? 'On this page' : modal === 'asset' ? assetPath : 'Reading preferences'}</h2><button aria-label="Close dialog" onclick={closeModal}>×</button></div>
  {#if modal === 'commands' || modal === 'search'}
    <input class="palette-input" aria-label={modal === 'search' ? 'Search query' : 'Filter commands'} placeholder={modal === 'search' ? 'Search notes, headings, and ideas…' : 'What would you like to do?'} bind:value={query} oninput={() => activeResult = 0} onkeydown={resultKey} aria-controls="palette-results" aria-activedescendant={`result-${activeResult}`} />
    {#if modal === 'search'}<div class="search-scope"><label for="search-workspace">Workspace</label><select id="search-workspace" bind:value={searchWorkspace}><option value={null}>All workspaces</option>{#each workspaces as w (w.id)}<option value={w.id}>{w.name}</option>{/each}</select></div>{/if}
    <div class="palette-results" id="palette-results">
      {#if modal === 'commands'}{#each commands as command, i}<button id={`result-${i}`} class:highlighted={activeResult === i} onclick={command.run}><span>{command.name}</span><kbd>{command.shortcut}</kbd></button>{/each}{#if !commands.length}<p>No matching commands.</p>{/if}
      {:else}{#if searching}<p role="status">Searching…</p>{:else if searchError}<p role="alert">{searchError}</p>{:else if !query.trim()}<p>Search your selected workspace or the entire library. Results stay on your device.</p>{:else if !hits.length}<p>No results. Try another word or refresh your workspace.</p>{/if}{#each hits as hit, i}<button id={`result-${i}`} class:highlighted={activeResult === i} class="search-result" onclick={() => chooseHit(hit)}><strong>{hit.title}</strong><small>{workspaces.find(w => w.id === hit.workspace_id)?.name} / {hit.path}</small>{#if hit.section_title}<small class="section-context">{#each textSegments(hit.section_title, hit.matched_terms ?? []) as segment}{#if segment.matched}<mark class="result-match">{segment.text}</mark>{:else}{segment.text}{/if}{/each}</small>{/if}<span>{#each textSegments(hit.snippet, hit.matched_terms ?? []) as segment}{#if segment.matched}<mark class="result-match">{segment.text}</mark>{:else}{segment.text}{/if}{/each}</span></button>{/each}{/if}
    </div><div class="palette-footer">↑ ↓ navigate <span>↵ select · Esc close</span></div>
  {:else if modal === 'github'}
    <div class="settings"><GithubAccount picker onadded={githubAdded} /></div>
    <details><summary>Erweiterte Optionen</summary>
    <form onsubmit={addGithub}><p>Clone a public or private repository using HTTPS or SSH. Import and Sync contact GitHub only when you request them.</p><label for="github-url">Repository URL</label><input id="github-url" type="text" required placeholder="https://github.com/owner/repository or git@github.com:owner/repository.git" bind:value={githubUrl} /><p class="muted">SSH uses your ssh-agent and verified known_hosts. HTTPS uses your Git credential helper: gh auth login then gh auth setup-git, or Git Credential Manager. Account authorization is handled by the native app, not this form. Never paste tokens into a URL; this URL draft is not saved.</p><button type="button" onclick={() => { void openUrl(gitGuides.ssh).catch(e => { error = errorMessage(e); }); }}>SSH setup guide ↗</button><button type="button" onclick={() => { void openUrl(gitGuides.https).catch(e => { error = errorMessage(e); }); }}>HTTPS setup guide ↗</button><button type="button" onclick={() => { void openUrl(gitGuides.fingerprints).catch(e => { error = errorMessage(e); }); }}>Verify SSH fingerprints ↗</button><button type="button" onclick={() => { void openUrl(gitGuides.gcm).catch(e => { error = errorMessage(e); }); }}>Git Credential Manager ↗</button>{#if error}<p role="alert" class="error-text">{error}</p>{/if}<button class="primary" disabled={busy}>{busy ? 'Cloning and indexing repository…' : 'Add repository'}</button></form>
        </details>
  {:else if modal === 'toc' && current}
    <nav class="palette-results" aria-label="Document headings">{#each current.toc as heading}<button style:padding-left={`${14 + Math.max(0, heading.level - 1) * 12}px`} onclick={() => { closeModal(); jump({ ...current!, fragment: heading.id }); }}>{heading.text}</button>{/each}</nav>
  {:else if modal === 'asset'}
    <div class="asset-preview"><img src={assetUrl} alt={assetPath} /><p>Loaded from workspace bytes. No remote image request.</p></div>
  {:else if modal === 'settings'}
    <div class="settings"><h3>Connected accounts</h3><GithubAccount picker onadded={githubAdded} /><label for="theme">Appearance</label><select id="theme" bind:value={preferences.theme}><option value="system">Follow system</option><option value="light">Light</option><option value="dark">Dark</option></select><label for="font">Reading typeface</label><select id="font" bind:value={preferences.font}><option value="serif">Serif — a book-like feel</option><option value="sans">Sans — clear and simple</option></select><label for="font-size">Text size · {preferences.size}px</label><input id="font-size" type="range" min="14" max="24" step="1" bind:value={preferences.size} /><p>Preferences and reading positions are saved on this device. Relative images load securely from workspace bytes; remote images are never fetched.</p></div>
  {/if}
</dialog>

<dialog bind:this={guardDialog} aria-labelledby="guard-title" aria-describedby="guard-description" oncancel={() => finishGuard(false)} onclose={() => { if (guard) finishGuard(false); }}>
  <div class="dialog-heading"><h2 id="guard-title">Unsaved changes</h2></div>
  <div class="settings"><p id="guard-description">{guard?.message}</p><div class="guard-actions"><button onclick={() => finishGuard(false)}>Keep editing</button><button class="primary" onclick={() => finishGuard(true)}>Discard changes</button></div></div>
</dialog>
