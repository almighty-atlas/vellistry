import { beforeEach, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { openUrl } from '@tauri-apps/plugin-opener';
import GithubAccount from './GithubAccount.svelte';
import { api } from './api';
import type { GithubStatus } from './types';

vi.mock('./api', () => ({ api: Object.fromEntries(['githubStatus', 'githubConnect', 'githubCancel', 'githubDisconnect', 'githubRepositories', 'githubManage', 'addGithub'].map(name => [name, vi.fn()])) }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }));
const disconnected: GithubStatus = { configured: true, connected: false, login: null, avatar_url: null };
const connected: GithubStatus = { ...disconnected, connected: true, login: 'reader', avatar_url: 'data:image/png;base64,aGVsbG8=' };
const repositories = [
  { id: 1, full_name: 'reader/public-notes', private: false, clone_url: 'https://github.com/reader/public-notes.git' },
  { id: 2, full_name: 'reader/private-notes', private: true, clone_url: 'https://github.com/reader/private-notes.git' },
];
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.githubStatus).mockResolvedValue(disconnected);
  vi.mocked(api.githubConnect).mockResolvedValue(connected);
  vi.mocked(api.githubRepositories).mockResolvedValue(repositories);
  vi.mocked(api.githubCancel).mockResolvedValue(undefined);
  vi.mocked(api.githubDisconnect).mockResolvedValue(undefined);
  vi.mocked(api.githubManage).mockResolvedValue(undefined);
  vi.mocked(openUrl).mockResolvedValue(undefined);
});

it('waits for browser authorization, then shows the avatar and loads the picker', async () => {
  const pending = deferred<GithubStatus>();
  vi.mocked(api.githubConnect).mockReturnValue(pending.promise);
  render(GithubAccount, { picker: true });
  await fireEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));
  expect(screen.getByText(/Waiting for GitHub authorization/)).toBeTruthy();
  expect(api.githubRepositories).not.toHaveBeenCalled();
  pending.resolve(connected);
  expect(await screen.findByText('Connected as reader')).toBeTruthy();
  expect(await screen.findByRole('button', { name: 'reader/private-notes Private' })).toBeTruthy();
  expect(screen.getByAltText('GitHub avatar').getAttribute('src')).toBe(connected.avatar_url);
});

it('cancels a pending connection and ignores a late successful callback', async () => {
  const pending = deferred<GithubStatus>();
  vi.mocked(api.githubConnect).mockReturnValue(pending.promise);
  render(GithubAccount, { picker: true });
  await fireEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));
  await fireEvent.click(screen.getByRole('button', { name: 'Cancel connection' }));
  expect(api.githubCancel).toHaveBeenCalledOnce();
  expect(await screen.findByText('Connection cancelled.')).toBeTruthy();
  pending.resolve(connected);
  await waitFor(() => expect(screen.queryByText('Connected as reader')).toBeNull());
  expect(api.githubRepositories).not.toHaveBeenCalled();
});

it('reports connect failure and allows retry', async () => {
  vi.mocked(api.githubConnect).mockRejectedValueOnce({ message: 'Authorization denied' });
  render(GithubAccount);
  await fireEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Authorization denied');
  await fireEvent.click(screen.getByRole('button', { name: 'Connect GitHub' }));
  expect(await screen.findByText('Connected as reader')).toBeTruthy();
  expect(api.githubRepositories).not.toHaveBeenCalled();
});

it('makes missing configuration explicit, disables connect, and opens docs only on request', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue({ ...disconnected, configured: false });
  render(GithubAccount);
  expect(await screen.findByRole('note')).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Connect GitHub' }) as HTMLButtonElement).disabled).toBe(true);
  expect(openUrl).not.toHaveBeenCalled();
  await fireEvent.click(screen.getByRole('button', { name: /setup documentation/ }));
  expect(openUrl).toHaveBeenCalledWith(expect.stringContaining('docs.github.com/en/apps/'));
  expect(api.githubConnect).not.toHaveBeenCalled();
});

it('searches repositories, filters visibility and adds a private repository through the existing API', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  const workspace = { id: 7, name: 'private-notes', kind: 'github' as const, root: '/repo', url: repositories[1].clone_url };
  vi.mocked(api.addGithub).mockResolvedValue(workspace);
  const onadded = vi.fn();
  render(GithubAccount, { picker: true, onadded });
  await screen.findByRole('button', { name: 'reader/public-notes Public' });
  await fireEvent.input(screen.getByRole('textbox', { name: 'Search repositories' }), { target: { value: 'PRIVATE-NOTES' } });
  expect(screen.queryByRole('button', { name: 'reader/public-notes Public' })).toBeNull();
  await fireEvent.change(screen.getByRole('combobox', { name: 'Repository visibility' }), { target: { value: 'public' } });
  expect(screen.getByText('No matching repositories.')).toBeTruthy();
  await fireEvent.change(screen.getByRole('combobox', { name: 'Repository visibility' }), { target: { value: 'private' } });
  await fireEvent.click(screen.getByRole('button', { name: 'reader/private-notes Private' }));
  await waitFor(() => expect(onadded).toHaveBeenCalledWith(workspace));
  expect(api.addGithub).toHaveBeenCalledWith(repositories[1].clone_url);
});

it('opens native repository management on request, refreshes access, and disconnects without deleting workspaces', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  render(GithubAccount, { picker: true });
  await screen.findByRole('button', { name: 'reader/private-notes Private' });
  expect(api.githubManage).not.toHaveBeenCalled();
  await fireEvent.click(screen.getByRole('button', { name: /Manage repositories/ }));
  expect(api.githubManage).toHaveBeenCalledOnce();
  expect(openUrl).not.toHaveBeenCalled();
  vi.mocked(api.githubRepositories).mockResolvedValue([]);
  await fireEvent.click(screen.getByRole('button', { name: 'Refresh repositories' }));
  expect(await screen.findByText(/No repositories available/)).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'Disconnect GitHub' }));
  expect(api.githubDisconnect).toHaveBeenCalledOnce();
  expect(await screen.findByText('Not connected')).toBeTruthy();
  expect(screen.queryByAltText('GitHub avatar')).toBeNull();
  expect(screen.queryByRole('textbox', { name: 'Search repositories' })).toBeNull();
  expect(screen.getByText(/Existing local workspaces are unchanged/)).toBeTruthy();
});

it('reports repository loading, cloning, management and disconnect errors without losing the account', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  vi.mocked(api.githubRepositories).mockRejectedValueOnce({ message: 'Cannot list repositories' });
  render(GithubAccount, { picker: true });
  expect((await screen.findByRole('alert')).textContent).toContain('Cannot list repositories');
  await fireEvent.click(screen.getByRole('button', { name: 'Refresh repositories' }));
  vi.mocked(api.addGithub).mockRejectedValue({ message: 'Clone failed' });
  await fireEvent.click(await screen.findByRole('button', { name: 'reader/private-notes Private' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Clone failed');
  vi.mocked(api.githubManage).mockRejectedValue({ message: 'Browser unavailable' });
  await fireEvent.click(screen.getByRole('button', { name: /Manage repositories/ }));
  expect((await screen.findByRole('alert')).textContent).toContain('Browser unavailable');
  vi.mocked(api.githubDisconnect).mockRejectedValue({ message: 'Disconnect failed' });
  await fireEvent.click(screen.getByRole('button', { name: 'Disconnect GitHub' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Disconnect failed');
  expect(screen.getByText('Connected as reader')).toBeTruthy();
});

it('can retry cancellation failures and cancels on unmount', async () => {
  vi.mocked(api.githubConnect).mockReturnValue(new Promise(() => {}));
  vi.mocked(api.githubCancel).mockRejectedValueOnce({ message: 'Cancel failed' });
  const mounted = render(GithubAccount);
  await fireEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));
  await fireEvent.click(screen.getByRole('button', { name: 'Cancel connection' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Cancel failed');
  await fireEvent.click(screen.getByRole('button', { name: 'Cancel connection' }));
  expect(await screen.findByText('Connection cancelled.')).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'Connect GitHub' }));
  mounted.unmount();
  expect(api.githubCancel).toHaveBeenCalledTimes(3);
});

it('never loads a remote avatar URL in the webview', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue({ ...connected, avatar_url: 'https://avatars.githubusercontent.com/u/1' });
  render(GithubAccount);
  expect(await screen.findByText('Connected as reader')).toBeTruthy();
  expect(screen.queryByAltText('GitHub avatar')).toBeNull();
});

it('retries failed status checks and works without an avatar', async () => {
  vi.mocked(api.githubStatus).mockRejectedValueOnce({ message: 'Account status unavailable' }).mockResolvedValueOnce({ ...connected, avatar_url: null });
  render(GithubAccount);
  expect((await screen.findByRole('alert')).textContent).toContain('Account status unavailable');
  await fireEvent.click(screen.getByRole('button', { name: 'Retry account status' }));
  expect(await screen.findByText('Connected as reader')).toBeTruthy();
  expect(screen.queryByAltText('GitHub avatar')).toBeNull();
});
