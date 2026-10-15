import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { EditorView } from '@codemirror/view';
import App from './App.svelte';
import { api } from './api';
import { openUrl } from '@tauri-apps/plugin-opener';
import { open as openDirectory } from '@tauri-apps/plugin-dialog';
import type { Document, SearchHit } from './types';

vi.mock('./api', () => ({ api: Object.fromEntries(['workspaces', 'documents', 'read', 'save', 'search', 'resolve', 'asset', 'sync', 'refresh', 'addLocal', 'addGithub', 'githubStatus', 'githubConnect', 'githubCancel', 'githubDisconnect', 'githubRepositories', 'githubManage'].map(name => [name, vi.fn()])) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ onCloseRequested: vi.fn().mockResolvedValue(() => {}) }) }));
const doc: Document = { workspace_id: 1, path: 'notes/start.md', title: 'Start here', revision: 'r1', content: '# Start here\n\nHello reader\n\n```js\nconst x = 1;\n```', html: '<div data-source-start="0"><h1 id="start">Start here</h1></div><div data-source-start="14"><p>Hello reader <a href="https://example.com">Web link</a></p></div><div data-source-start="28"><pre><code class="language-javascript">const x = 1;</code></pre></div>', toc: [{ level: 1, id: 'start', text: 'Start here', source_start: 0 }] };
const other: Document = { ...doc, path: 'other.md', title: 'Other document', content: '# Other document', html: '<h1>Other document</h1>', toc: [] };

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(openUrl).mockResolvedValue(undefined);
  vi.mocked(api.githubStatus).mockResolvedValue({ configured: false, connected: false, login: null, avatar_url: null });
  vi.mocked(api.workspaces).mockResolvedValue([{ id: 1, name: 'Notes', root: '/notes', kind: 'local', url: null }]);
  vi.mocked(api.documents).mockResolvedValue([doc, other]);
  vi.mocked(api.refresh).mockResolvedValue({ indexed: 0, unchanged: 2, removed: 0 });
  vi.mocked(api.read).mockImplementation(async (_, path) => path === doc.path ? { ...doc } : { ...other });
  vi.mocked(api.save).mockResolvedValue({ ...doc, revision: 'r2' });
  vi.mocked(api.search).mockResolvedValue([]);
  vi.mocked(api.resolve).mockResolvedValue({ kind: 'external', url: 'https://example.com' });
});
async function openDocument() {
  render(App);
  await fireEvent.click(await screen.findByRole('button', { name: /start\.md/ }));
  await screen.findByRole('heading', { name: 'Start here' });
}
async function editDraft() {
  await fireEvent.click(screen.getByRole('button', { name: 'Source' }));
  const content = await screen.findByRole('textbox', { name: 'Markdown source' });
  const view = EditorView.findFromDOM(content)!;
  view.dispatch({ changes: { from: view.state.doc.length, insert: '\nMy unsaved draft' } });
  await waitFor(() => expect(screen.getByRole('button', { name: /Save Ctrl S/ }).hasAttribute('disabled')).toBe(false));
  return view.state.doc.toString();
}

async function showHits(hits: SearchHit[], query = 'search') {
  vi.mocked(api.search).mockResolvedValue(hits);
  await fireEvent.keyDown(window, { key: 'f', ctrlKey: true, shiftKey: true });
  await fireEvent.input(await screen.findByRole('textbox', { name: 'Search query' }), { target: { value: query } });
  await waitFor(() => expect(document.querySelectorAll('.search-result')).toHaveLength(hits.length));
}
const searchDoc: Document = { ...doc, html: '<div data-source-start="0" data-source-end="20"><h1 id="start">Start here</h1></div><div data-source-start="20" data-source-end="60"><p>Überprüfung räumlich outside</p></div><div data-source-start="60" data-source-end="160"><p>😀 <a href="https://example.com">Überprüfung</a> räumlich</p></div><div data-source-start="160" data-source-end="220"><pre><code class="language-javascript">const Überprüfung = "räumlich";</code></pre></div>' };
const searchHit: SearchHit = { workspace_id: 1, path: doc.path, title: doc.title, score: -1, source_start: 60, source_end: 160, section_title: 'Start / Überprüfung', matched_terms: ['uberprufung', 'raumlich'], snippet: '😀 Überprüfung räumlich' };

describe('search navigation', () => {
  it('safely shows section context and corrected multiword terms, scopes marks, and preserves links/find layers', async () => {
    vi.mocked(api.read).mockResolvedValue(searchDoc);
    await openDocument();
    await waitFor(() => expect(document.querySelector('.copy-code')).not.toBeNull());
    const link = screen.getByRole('link', { name: 'Überprüfung' });
    vi.spyOn(HTMLElement.prototype, 'getClientRects').mockReturnValue([new DOMRect(0, 0, 10, 10)] as unknown as DOMRectList);
    const scroll = vi.spyOn(HTMLElement.prototype, 'scrollIntoView');
    await showHits([searchHit], '"uberprufung raumlih"');
    expect(document.querySelector('.section-context')?.textContent).toBe('Start / Überprüfung');
    expect([...document.querySelectorAll('.search-result > span mark')].map(m => m.textContent)).toEqual(['Überprüfung', 'räumlich']);
    await fireEvent.click(document.querySelector('.search-result')!);
    await waitFor(() => expect(document.querySelectorAll('article .search-match')).toHaveLength(2));
    const marks = [...document.querySelectorAll<HTMLElement>('article .search-match')];
    expect(scroll.mock.instances).toContain(marks[0]);
    expect(document.querySelector('article [data-source-start="20"] mark')).toBeNull();
    expect(document.querySelector('article pre mark, article button mark')).toBeNull();
    expect(screen.getByRole('link', { name: 'Überprüfung' })).toBe(link);
    await fireEvent.click(marks[0]); await waitFor(() => expect(api.resolve).toHaveBeenCalledWith(1, doc.path, 'https://example.com'));
    await fireEvent.keyDown(window, { key: 'f', ctrlKey: true });
    await fireEvent.input(screen.getByLabelText('Find'), { target: { value: 'Überprüfung' } });
    await waitFor(() => expect(document.querySelectorAll('article .find-match').length).toBeGreaterThan(0));
    await fireEvent.click(screen.getByRole('button', { name: 'Close find' }));
    await waitFor(() => expect(document.querySelector('article .find-match')).toBeNull());
    expect(document.querySelectorAll('article .search-match')).toHaveLength(2);
    await showHits([{ ...searchHit, source_start: 20, source_end: 60, matched_terms: ['uber'], snippet: 'Überprüfung outside' }]);
    await fireEvent.click(document.querySelector('.search-result')!);
    await waitFor(() => expect(document.querySelector('article .search-match')?.textContent).toBe('Über'));
    expect(document.querySelector('article [data-source-start="60"] mark')).toBeNull();
    expect(api.read).toHaveBeenCalledTimes(1);
    await fireEvent.click(screen.getByRole('button', { name: 'Back' }));
    await waitFor(() => expect(document.querySelectorAll('article [data-source-start="60"] .search-match')).toHaveLength(2));
  });
  it('renders HTML-looking context/snippets strictly as escaped text segments', async () => {
    await openDocument();
    const snippet = '<img src="https://tracker.invalid"> Überprüfung';
    await showHits([{ ...searchHit, snippet, section_title: '<script>alert(1)</script>', matched_terms: ['uberprufung', 'alert'] }]);
    expect(document.querySelector('.search-result > span')?.textContent).toBe(snippet);
    expect(document.querySelector('.section-context')?.textContent).toBe('<script>alert(1)</script>');
    expect(document.querySelector('.search-result img, .search-result script')).toBeNull();
  });
  it('marks multiple selected blocks on initial navigation before deferred code jobs and leaves copy controls untouched', async () => {
    vi.mocked(api.read).mockResolvedValue(searchDoc);
    render(App); await screen.findByRole('button', { name: /start\.md/ });
    await showHits([{ ...searchHit, source_end: 220, matched_terms: ['const', 'raumlich'] }]);
    await fireEvent.click(document.querySelector('.search-result')!);
    await waitFor(() => expect(document.querySelector('article code .search-match')).not.toBeNull());
    await waitFor(() => expect(document.querySelector('.copy-code')).not.toBeNull());
    expect(document.querySelectorAll('article .search-match')).toHaveLength(3);
    expect(document.querySelector('article code .hljs-keyword, article button mark, article [data-source-start="20"] mark')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Copy code block' }));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('const Überprüfung = "räumlich";');
  });
  it('falls back to the selected block for a metadata-only/legacy hit', async () => {
    await openDocument();
    const block = document.querySelector<HTMLElement>('[data-source-start="14"]')!;
    const scroll = vi.spyOn(block, 'scrollIntoView');
    await showHits([{ ...searchHit, source_start: 14, source_end: null, matched_terms: ['metadataonly'] }]);
    await fireEvent.click(document.querySelector('.search-result')!);
    await waitFor(() => expect(scroll).toHaveBeenCalledWith({ block: 'center' }));
    expect(document.querySelector('article .search-match')).toBeNull();
  });
  it('keeps Source navigation at the same byte offset without reader marks', async () => {
    await openDocument(); await fireEvent.click(screen.getByRole('button', { name: 'Source' }));
    const host = await screen.findByRole('textbox', { name: 'Markdown source' });
    await showHits([{ ...searchHit, source_start: 14 }]); await fireEvent.click(document.querySelector('.search-result')!);
    await waitFor(() => expect(EditorView.findFromDOM(host)!.state.selection.main.anchor).toBe(14));
    expect(document.querySelector('article .search-match')).toBeNull();
  });
  it('does not apply a late search read over a newer document navigation', async () => {
    render(App); await screen.findByRole('button', { name: /start\.md/ });
    let complete!: (value: Document) => void;
    vi.mocked(api.read).mockImplementationOnce(() => new Promise(resolve => { complete = resolve; }));
    await showHits([searchHit]); await fireEvent.click(document.querySelector('.search-result')!);
    await waitFor(() => expect(api.read).toHaveBeenCalledTimes(1));
    await fireEvent.click(screen.getByRole('button', { name: /other\.md/ }));
    await screen.findByRole('heading', { name: 'Other document' });
    complete(searchDoc); await new Promise(resolve => setTimeout(resolve, 30));
    expect(screen.getByRole('heading', { name: 'Other document' })).toBeTruthy();
    expect(document.querySelector('article .search-match')).toBeNull();
  });
  it('cancels pending find walks when the document changes', async () => {
    await openDocument();
    const article = document.querySelector('article'), walker = vi.spyOn(document, 'createTreeWalker');
    await fireEvent.keyDown(window, { key: 'f', ctrlKey: true });
    await fireEvent.input(screen.getByLabelText('Find'), { target: { value: 'reader' } });
    await fireEvent.click(screen.getByRole('button', { name: /other\.md/ }));
    await screen.findByRole('heading', { name: 'Other document' });
    await new Promise(resolve => setTimeout(resolve, 180));
    expect(walker.mock.calls.filter(call => call[0] === article && call[1] === NodeFilter.SHOW_TEXT)).toHaveLength(0);
    expect(document.querySelector('.find-match')).toBeNull();
  });
  it('flushes pending find input on Enter without a second delayed walk', async () => {
    await openDocument(); await waitFor(() => expect(document.querySelector('.copy-code')).not.toBeNull());
    const article = document.querySelector('article'), walker = vi.spyOn(document, 'createTreeWalker');
    await fireEvent.keyDown(window, { key: 'f', ctrlKey: true });
    const input = screen.getByLabelText('Find');
    await fireEvent.input(input, { target: { value: 'reader' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(document.querySelector('.find-match')?.textContent).toBe('reader');
    await new Promise(resolve => setTimeout(resolve, 180));
    expect(walker.mock.calls.filter(call => call[0] === article && call[1] === NodeFilter.SHOW_TEXT)).toHaveLength(1);
  });
  it('debounces document-find walks to the latest input', async () => {
    await openDocument(); await waitFor(() => expect(document.querySelector('.copy-code')).not.toBeNull());
    const walker = vi.spyOn(document, 'createTreeWalker');
    await fireEvent.keyDown(window, { key: 'f', ctrlKey: true });
    const input = screen.getByLabelText('Find');
    for (const value of ['r', 'rea', 'reader']) await fireEvent.input(input, { target: { value } });
    const textWalks = () => walker.mock.calls.filter(call => call[0] === document.querySelector('article') && call[1] === NodeFilter.SHOW_TEXT);
    expect(textWalks()).toHaveLength(0);
    await waitFor(() => expect(document.querySelector('.find-match')?.textContent).toBe('reader'));
    expect(textWalks()).toHaveLength(1);
  });
});

describe('reading UI', () => {
  it('offers connected accounts in settings and adds a private repository from the GitHub picker', async () => {
    vi.mocked(api.githubStatus).mockResolvedValue({ configured: true, connected: true, login: 'reader', avatar_url: null });
    vi.mocked(api.githubRepositories).mockResolvedValue([{ id: 4, full_name: 'reader/private-notes', private: true, clone_url: 'https://github.com/reader/private-notes.git' }]);
    const workspace = { id: 4, name: 'private-notes', kind: 'github' as const, root: '/repo', url: 'https://github.com/reader/private-notes.git' };
    vi.mocked(api.addGithub).mockImplementation(async () => {
      vi.mocked(api.workspaces).mockResolvedValue([{ id: 1, name: 'Notes', root: '/notes', kind: 'local', url: null }, workspace]);
      return workspace;
    });
    const first = { ...doc, workspace_id: 4, path: 'README.md', title: 'Repository introduction', html: '<h1>Repository introduction</h1>' };
    vi.mocked(api.documents).mockImplementation(async id => id === 4 ? [first, { ...other, workspace_id: 4 }] : [doc, other]);
    vi.mocked(api.read).mockResolvedValue(first);
    render(App);
    await screen.findByRole('button', { name: /start\.md/ });
    await fireEvent.click(screen.getByRole('button', { name: /Reading preferences/ }));
    expect(screen.getByRole('heading', { name: 'Connected accounts' })).toBeTruthy();
    expect(await screen.findByText('Connected as reader')).toBeTruthy();
    expect(screen.getByRole('button', { name: /Repository-Zugriff verwalten/ })).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Add a GitHub repository' }));
    expect(screen.getByText('Erweiterte Optionen').closest('details')!.open).toBe(false);
    await fireEvent.click(await screen.findByRole('button', { name: 'reader/private-notes Private' }));
    await waitFor(() => expect(api.addGithub).toHaveBeenCalledWith('https://github.com/reader/private-notes.git', true));
    await screen.findByRole('heading', { name: 'Repository introduction' });
    expect(api.documents).toHaveBeenCalledWith(4);
    expect(api.read).toHaveBeenCalledWith(4, 'README.md');
    expect(screen.queryByRole('dialog')).toBeNull();
  });
  it('closes the picker without reading another workspace when the added repository has no documents', async () => {
    vi.mocked(api.githubStatus).mockResolvedValue({ configured: true, connected: true, login: 'reader', avatar_url: null });
    vi.mocked(api.githubRepositories).mockResolvedValue([{ id: 4, full_name: 'reader/empty', private: true, clone_url: 'https://github.com/reader/empty.git' }]);
    const workspace = { id: 4, name: 'empty', kind: 'github' as const, root: '/repo', url: 'https://github.com/reader/empty.git' };
    vi.mocked(api.addGithub).mockImplementation(async () => {
      vi.mocked(api.workspaces).mockResolvedValue([workspace]);
      vi.mocked(api.documents).mockResolvedValue([]);
      return workspace;
    });
    render(App);
    await screen.findByRole('button', { name: /start\.md/ });
    await fireEvent.click(screen.getByRole('button', { name: 'Add a GitHub repository' }));
    const advanced = screen.getByText('Erweiterte Optionen').closest('details')!;
    expect(advanced.open).toBe(false);
    await fireEvent.click(await screen.findByRole('button', { name: 'reader/empty Private' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(api.documents).toHaveBeenCalledWith(4);
    expect(api.read).not.toHaveBeenCalled();
  });
  it('keeps unchanged polls from reading/rendering the current document', async () => {
    const interval = vi.spyOn(globalThis, 'setInterval');
    await openDocument();
    await waitFor(() => expect(document.querySelector('code .hljs-keyword')).not.toBeNull());
    const article = document.querySelector('article'), code = article?.querySelector('code');
    const poll = interval.mock.calls.find(call => call[1] === 4000)![0] as () => void;
    poll();
    await waitFor(() => expect(api.documents).toHaveBeenCalledTimes(2));
    expect(api.read).toHaveBeenCalledTimes(1);
    expect(document.querySelector('article')).toBe(article);
    expect(document.querySelector('code')).toBe(code);
    await fireEvent.scroll(document.querySelector('.reader')!);
    expect(api.read).toHaveBeenCalledTimes(1);
  });
  it.each(['git@github.com:owner/repo.git', 'ssh://git@github.com/owner/repo.git'])('submits SSH URL %s without browser URL validation or persisted draft', async url => {
    render(App);
    await screen.findByRole('button', { name: /start\.md/ });
    await fireEvent.click(screen.getByRole('button', { name: 'Add a GitHub repository' }));
    await fireEvent.click(screen.getByText('Erweiterte Optionen'));
    const input = screen.getByRole('textbox', { name: 'Repository URL' }) as HTMLInputElement;
    expect(input.type).toBe('text');
    await fireEvent.input(input, { target: { value: url } });
    expect(input.checkValidity()).toBe(true);
    vi.mocked(api.addGithub).mockResolvedValue({ id: 2, name: 'Repo', kind: 'github', root: '/repo', url });
    await fireEvent.click(screen.getByRole('button', { name: 'Add repository' }));
    await waitFor(() => expect(api.addGithub).toHaveBeenCalledWith(url));
    expect(Object.values(localStorage).join(' ')).not.toContain(url);
  });
  it.each([
    ['git_authentication', 'ssh-agent'], ['git_permission_denied', 'SSO'],
    ['git_network', 'DNS'], ['git_ssh_host_verification', 'known_hosts'],
  ])('shows actionable guidance for %s and opens docs only on request', async (code, guidance) => {
    render(App);
    await screen.findByRole('button', { name: /start\.md/ });
    await fireEvent.click(screen.getByRole('button', { name: 'Add a GitHub repository' }));
    await fireEvent.click(screen.getByText('Erweiterte Optionen'));
    expect(openUrl).not.toHaveBeenCalled();
    vi.mocked(api.addGithub).mockRejectedValue({ code, message: 'sanitized failure' });
    await fireEvent.input(screen.getByRole('textbox', { name: 'Repository URL' }), { target: { value: 'git@github.com:owner/repo.git' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Add repository' }));
    await waitFor(() => expect(screen.getAllByRole('alert').some(node => node.textContent?.includes(guidance))).toBe(true));
    await fireEvent.click(screen.getByRole('button', { name: /SSH setup guide/ }));
    expect(openUrl).toHaveBeenCalledWith('https://docs.github.com/en/authentication/connecting-to-github-with-ssh');
  });
  it('renders real core HTML, highlights code, and opens links only on request', async () => {
    await openDocument();
    expect(api.resolve).not.toHaveBeenCalled(); expect(openUrl).not.toHaveBeenCalled();
    await waitFor(() => expect(document.querySelector('code .hljs-keyword')).not.toBeNull());
    await fireEvent.click(screen.getByRole('button', { name: 'Copy code block' }));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('const x = 1;');
    await fireEvent.click(screen.getByRole('link', { name: 'Web link' }));
    await waitFor(() => expect(openUrl).toHaveBeenCalledWith('https://example.com'));
    expect(api.resolve).toHaveBeenCalledWith(1, doc.path, 'https://example.com');
  });
  it('rejects non-web external targets returned by the backend', async () => {
    await openDocument(); vi.mocked(api.resolve).mockResolvedValue({ kind: 'external', url: 'file:///etc/passwd' });
    await fireEvent.click(screen.getByRole('link', { name: 'Web link' }));
    await screen.findByText('Only explicit HTTP and HTTPS links can be opened.');
    expect(openUrl).not.toHaveBeenCalled();
  });
  it('opens the keyboard palette and searches plain-text results across workspaces', async () => {
    await openDocument();
    await fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    await screen.findByRole('heading', { name: 'Go anywhere' });
    await fireEvent.keyDown(window, { key: 'f', ctrlKey: true, shiftKey: true });
    const input = await screen.findByRole('textbox', { name: 'Search query' });
    vi.mocked(api.search).mockResolvedValue([{ ...doc, score: -1, source_start: 14, source_end: null, section_title: '', matched_terms: [], snippet: '<img src="https://tracker.invalid"> plain text' }]);
    await fireEvent.input(input, { target: { value: 'reader' } });
    await screen.findByText('<img src="https://tracker.invalid"> plain text');
    expect(document.querySelector('img')).toBeNull();
    expect(api.search).toHaveBeenCalledWith('reader', null);
  });
  it('filters search through the backend workspace argument', async () => {
    await openDocument();
    await fireEvent.keyDown(window, { key: 'f', ctrlKey: true, shiftKey: true });
    await fireEvent.input(screen.getByRole('textbox', { name: 'Search query' }), { target: { value: 'reader' } });
    await waitFor(() => expect(api.search).toHaveBeenCalledWith('reader', null));
    await fireEvent.change(screen.getByLabelText('Workspace'), { target: { value: '1' } });
    await waitFor(() => expect(api.search).toHaveBeenCalledWith('reader', 1));
    await fireEvent.change(screen.getByLabelText('Workspace'), { target: { value: '' } });
    await waitFor(() => expect(api.search).toHaveBeenLastCalledWith('reader', null));
  });
  it('loads inert local images and revokes URLs on document switch and destruction', async () => {
    vi.mocked(api.read).mockImplementation(async (_, path) => path === doc.path ? { ...doc, html: `${doc.html}<img data-local-src="../assets/a.png" alt="Diagram">` } : { ...other });
    vi.mocked(api.resolve).mockResolvedValue({ kind: 'asset', path: 'assets/a.png', fragment: null });
    vi.mocked(api.asset).mockResolvedValue({ path: 'assets/a.png', mime_type: 'image/png', bytes: [137, 80, 78, 71] });
    vi.spyOn(URL, 'createObjectURL').mockReturnValueOnce('blob:first').mockReturnValueOnce('blob:second');
    const revoke = vi.spyOn(URL, 'revokeObjectURL');
    const mounted = render(App);
    await fireEvent.click(await screen.findByRole('button', { name: /start\.md/ }));
    await waitFor(() => expect(screen.getByAltText('Diagram').getAttribute('src')).toBe('blob:first'));
    await fireEvent.click(screen.getByRole('button', { name: /other\.md/ }));
    await screen.findByRole('heading', { name: 'Other document' });
    expect(revoke).toHaveBeenCalledWith('blob:first');
    await fireEvent.click(screen.getByRole('button', { name: 'Back' }));
    await waitFor(() => expect(screen.getByAltText('Diagram').getAttribute('src')).toBe('blob:second'));
    mounted.unmount();
    expect(revoke).toHaveBeenCalledWith('blob:second');
    expect(openUrl).not.toHaveBeenCalled();
  });
  it('does not render delayed image bytes into a different document with the same HTML', async () => {
    const html = '<h1>Start here</h1><img data-local-src="../assets/a.png" alt="Diagram">';
    vi.mocked(api.read).mockImplementation(async (_, path) => ({ ...(path === doc.path ? doc : other), html }));
    vi.mocked(api.resolve).mockResolvedValue({ kind: 'asset', path: 'assets/a.png', fragment: null });
    let complete!: (asset: { path: string; mime_type: string; bytes: number[] }) => void;
    vi.mocked(api.asset).mockImplementationOnce(() => new Promise(resolve => { complete = resolve; })).mockResolvedValue({ path: 'assets/a.png', mime_type: 'image/png', bytes: [1] });
    const create = vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:second-document');
    await openDocument();
    await waitFor(() => expect(api.asset).toHaveBeenCalledTimes(1));
    await fireEvent.click(screen.getByRole('button', { name: /other\.md/ }));
    await waitFor(() => expect(api.asset).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByAltText('Diagram').getAttribute('src')).toBe('blob:second-document'));
    complete({ path: 'assets/a.png', mime_type: 'image/png', bytes: [2] });
    await Promise.resolve();
    expect(create).toHaveBeenCalledTimes(1);
    expect(api.resolve).toHaveBeenLastCalledWith(1, other.path, '../assets/a.png');
  });
  it('does not open an asset-link preview after switching documents', async () => {
    await openDocument();
    vi.mocked(api.resolve).mockResolvedValue({ kind: 'asset', path: 'assets/a.png', fragment: null });
    let complete!: (asset: { path: string; mime_type: string; bytes: number[] }) => void;
    vi.mocked(api.asset).mockImplementationOnce(() => new Promise(resolve => { complete = resolve; }));
    const create = vi.spyOn(URL, 'createObjectURL');
    await fireEvent.click(screen.getByRole('link', { name: 'Web link' }));
    await waitFor(() => expect(api.asset).toHaveBeenCalledWith(1, 'assets/a.png'));
    await fireEvent.click(screen.getByRole('button', { name: /other\.md/ }));
    await screen.findByRole('heading', { name: 'Other document' });
    complete({ path: 'assets/a.png', mime_type: 'image/png', bytes: [1] });
    await Promise.resolve();
    expect(create).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog')).toBeNull();
  });
  it('imports a real directory selection and GitHub URL through the backend', async () => {
    render(App);
    await screen.findByRole('button', { name: /start\.md/ });
    const workspace = { id: 1, name: 'Notes', root: '/notes', kind: 'local' as const, url: null };
    vi.mocked(openDirectory).mockResolvedValue('/chosen/notes');
    vi.mocked(api.addLocal).mockResolvedValue(workspace);
    await fireEvent.click(screen.getByRole('button', { name: /Open a local folder/ }));
    await waitFor(() => expect(api.addLocal).toHaveBeenCalledWith('/chosen/notes'));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Add a GitHub repository' }).hasAttribute('disabled')).toBe(false));
    vi.mocked(api.addGithub).mockResolvedValue({ ...workspace, kind: 'github', url: 'https://github.com/owner/repo' });
    await fireEvent.click(screen.getByRole('button', { name: 'Add a GitHub repository' }));
    await fireEvent.click(screen.getByText('Erweiterte Optionen'));
    await fireEvent.input(screen.getByRole('textbox', { name: 'Repository URL' }), { target: { value: 'https://github.com/owner/repo' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Add repository' }));
    await waitFor(() => expect(api.addGithub).toHaveBeenCalledWith('https://github.com/owner/repo'));
  });
  it('persists reading preferences and restores the last document', async () => {
    localStorage.setItem('mdwell.last', JSON.stringify({ workspace_id: 1, path: doc.path }));
    render(App);
    await screen.findByRole('heading', { name: 'Start here' });
    await fireEvent.click(screen.getByRole('button', { name: /Reading preferences/ }));
    await fireEvent.change(screen.getByLabelText('Appearance'), { target: { value: 'dark' } });
    await waitFor(() => expect(JSON.parse(localStorage.getItem('mdwell.preferences')!).theme).toBe('dark'));
    expect(document.documentElement.dataset.theme).toBe('dark');
  });
  it('refreshes the index before listing documents and reading clean files without remote sync', async () => {
    const interval = vi.spyOn(globalThis, 'setInterval');
    await openDocument();
    const poll = interval.mock.calls.find(call => call[1] === 4000)![0] as () => void;
    vi.mocked(api.documents).mockResolvedValue([{ ...doc, revision: 'r2' }, other]);
    vi.mocked(api.read).mockResolvedValue({ ...doc, revision: 'r2', content: '# Updated from disk', html: '<h1>Updated from disk</h1>' });
    poll();
    await screen.findByRole('heading', { name: 'Updated from disk' });
    expect(api.refresh).toHaveBeenCalledWith(1);
    expect(vi.mocked(api.refresh).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(api.documents).mock.invocationCallOrder[1]);
    expect(vi.mocked(api.documents).mock.invocationCallOrder[1]).toBeLessThan(vi.mocked(api.read).mock.invocationCallOrder[1]);
    expect(api.sync).not.toHaveBeenCalled();
  });
  it('refreshes every workspace and does not overlap polls when one scan fails and another is pending', async () => {
    vi.mocked(api.workspaces).mockResolvedValue([
      { id: 1, name: 'Notes', root: '/notes', kind: 'local', url: null },
      { id: 2, name: 'Repository', root: '/checkout', kind: 'github', url: 'https://github.com/owner/repo' },
    ]);
    vi.mocked(api.documents).mockImplementation(async id => id === 1 ? [doc, other] : []);
    const interval = vi.spyOn(globalThis, 'setInterval');
    await openDocument();
    const poll = interval.mock.calls.find(call => call[1] === 4000)![0] as () => void;
    let finish!: () => void;
    vi.mocked(api.refresh).mockImplementation(id => id === 1
      ? Promise.reject({ message: 'Folder unavailable' })
      : new Promise(resolve => { finish = () => resolve({ indexed: 1, unchanged: 0, removed: 0 }); }));
    poll();
    await waitFor(() => expect(api.refresh).toHaveBeenCalledTimes(2));
    poll();
    expect(api.refresh).toHaveBeenCalledTimes(2);
    expect(api.documents).toHaveBeenCalledTimes(2);
    finish();
    await screen.findByText('Notes: Folder unavailable');
    expect(api.refresh).toHaveBeenCalledWith(1);
    expect(api.refresh).toHaveBeenCalledWith(2);
    expect(api.documents).toHaveBeenLastCalledWith(2);
    expect(api.sync).not.toHaveBeenCalled();
    expect(screen.getByRole('status').textContent).toBe('');
    vi.mocked(api.refresh).mockResolvedValue({ indexed: 0, unchanged: 2, removed: 0 });
    poll();
    await waitFor(() => expect(api.documents).toHaveBeenCalledTimes(5));
    expect(api.refresh).toHaveBeenCalledTimes(4);
  });
  it('navigates history back and forward', async () => {
    await openDocument();
    await fireEvent.click(screen.getByRole('button', { name: /other\.md/ }));
    await screen.findByRole('heading', { name: 'Other document' });
    await fireEvent.click(screen.getByRole('button', { name: 'Back' }));
    await screen.findByRole('heading', { name: 'Start here' });
    await fireEvent.click(screen.getByRole('button', { name: 'Forward' }));
    await screen.findByRole('heading', { name: 'Other document' });
  });
});

describe('revision-safe editing', () => {
  it('marks GitHub edits as local and unpublished, including after a successful save', async () => {
    vi.mocked(api.workspaces).mockResolvedValue([{ id: 1, name: 'Repository', root: '/checkout', kind: 'github', url: 'https://github.com/owner/repo' }]);
    await openDocument();
    expect(screen.queryByText('Local · unpublished')).toBeNull();
    const content = await editDraft();
    await screen.findByText('Local · unpublished');
    vi.mocked(api.save).mockResolvedValue({ ...doc, content, revision: 'r2' });
    await fireEvent.keyDown(window, { key: 's', ctrlKey: true });
    await screen.findByText('Saved locally · not published to GitHub');
    expect(screen.getByText('Local · unpublished')).not.toBeNull();
    expect(JSON.parse(localStorage.getItem('mdwell.githubLocalEdits')!)['1']).toBe(true);
    expect(api.sync).not.toHaveBeenCalled();
  });
  it('keeps a conflicting draft and guards navigation without a force save', async () => {
    await openDocument(); const content = await editDraft();
    vi.mocked(api.save).mockRejectedValue({ code: 'conflict', message: 'revision conflict', expected_revision: 'r1', actual_revision: 'r2' });
    await fireEvent.keyDown(window, { key: 's', ctrlKey: true });
    await screen.findByText('This file changed on disk.');
    expect(api.save).toHaveBeenCalledWith(1, doc.path, content, 'r1');
    await fireEvent.click(screen.getByRole('button', { name: /other\.md/ }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Keep editing' }));
    expect(api.read).toHaveBeenCalledTimes(1);
    expect(document.querySelector('.cm-content')?.textContent).toContain('My unsaved draft');
  });
  it('polls document lists but never overwrites a dirty draft', async () => {
    const interval = vi.spyOn(globalThis, 'setInterval');
    await openDocument(); await editDraft();
    const poll = interval.mock.calls.find(call => call[1] === 4000)![0] as () => void;
    poll();
    await waitFor(() => expect(api.documents).toHaveBeenCalledTimes(2));
    expect(api.refresh).toHaveBeenCalledWith(1);
    expect(api.read).toHaveBeenCalledTimes(1);
    expect(document.querySelector('.cm-content')?.textContent).toContain('My unsaved draft');
  });
  it('saves with the expected revision and preserves edits typed during save', async () => {
    await openDocument(); const content = await editDraft();
    let resolve!: (value: Document) => void;
    vi.mocked(api.save).mockImplementation(() => new Promise(done => { resolve = done; }));
    await fireEvent.keyDown(window, { key: 's', ctrlKey: true });
    const view = EditorView.findFromDOM(screen.getByRole('textbox', { name: 'Markdown source' }))!;
    view.dispatch({ changes: { from: view.state.doc.length, insert: '\nNewer edit' } });
    resolve({ ...doc, content, revision: 'r2' });
    await waitFor(() => expect(document.querySelector('.cm-content')?.textContent).toContain('Newer edit'));
    await waitFor(() => expect(screen.getByRole('button', { name: /Save Ctrl S/ }).hasAttribute('disabled')).toBe(false));
    expect(api.save).toHaveBeenCalledWith(1, doc.path, content, 'r1');
  });
});
