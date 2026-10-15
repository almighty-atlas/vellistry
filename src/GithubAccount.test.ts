import { beforeEach, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { openUrl } from '@tauri-apps/plugin-opener';
import { getCurrentWindow } from '@tauri-apps/api/window';
import GithubAccount from './GithubAccount.svelte';
import { api } from './api';
import type { DeviceAuthorization, GithubStatus } from './types';

vi.mock('./api', () => ({ api: Object.fromEntries(['githubStatus', 'githubConnect', 'githubCancel', 'githubDisconnect', 'githubRepositories', 'githubManage', 'addGithub'].map(name => [name, vi.fn()])) }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: vi.fn() }));
const disconnected: GithubStatus = { configured: true, connected: false, login: null, avatar_url: null };
const connected: GithubStatus = { ...disconnected, connected: true, login: 'reader', avatar_url: 'data:image/png;base64,aGVsbG8=' };
const cleanupFailure = 'GitHub sign-in cancelled, but secure credential cleanup failed. Keychain unavailable';
const authorization: DeviceAuthorization = { user_code: 'WDJT-QMNP', verification_uri: 'https://github.com/login/device', expires_in: 900, interval: 5 };
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
  vi.mocked(getCurrentWindow).mockReturnValue({ onFocusChanged: vi.fn().mockResolvedValue(() => {}) } as unknown as ReturnType<typeof getCurrentWindow>);
  vi.mocked(api.githubStatus).mockResolvedValue(disconnected);
  vi.mocked(api.githubConnect).mockResolvedValue(connected);
  vi.mocked(api.githubRepositories).mockResolvedValue(repositories);
  vi.mocked(api.githubCancel).mockResolvedValue(undefined);
  vi.mocked(api.githubDisconnect).mockResolvedValue(undefined);
  vi.mocked(api.githubManage).mockResolvedValue(undefined);
  vi.mocked(openUrl).mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: vi.fn().mockResolvedValue(undefined) } });
});

it('shows the notified user code while pending, opens and copies only explicitly, then loads the picker', async () => {
  const pending = deferred<GithubStatus>();
  vi.mocked(api.githubConnect).mockImplementation(onDevice => {
    onDevice(authorization);
    return pending.promise;
  });
  render(GithubAccount, { picker: true });
  await fireEvent.click(await screen.findByRole('button', { name: 'Mit GitHub verbinden' }));
  expect(screen.getByText(/Waiting for GitHub authorization/)).toBeTruthy();
  expect(screen.getByRole('progressbar', { name: 'GitHub authorization' }).hasAttribute('value')).toBe(false);
  expect(screen.getByRole('region', { name: 'Vellistry mit GitHub verbinden' })).toBeTruthy();
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(screen.getByText(authorization.user_code)).toBeTruthy();
  expect(screen.getByText(/900 Sekunden/)).toBeTruthy();
  expect(screen.getByText(/Offizielle GitHub-Seite.*Phishing/)).toBeTruthy();
  expect(api.githubRepositories).not.toHaveBeenCalled();
  expect(openUrl).not.toHaveBeenCalled();
  expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
  await fireEvent.click(screen.getByRole('button', { name: 'Code kopieren' }));
  expect(navigator.clipboard.writeText).toHaveBeenCalledExactlyOnceWith(authorization.user_code);
  expect(await screen.findByText('Code copied.')).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'GitHub öffnen' }));
  expect(openUrl).toHaveBeenCalledExactlyOnceWith('https://github.com/login/device');
  pending.resolve(connected);
  expect(await screen.findByText('Connected as reader')).toBeTruthy();
  expect(screen.queryByText(authorization.user_code)).toBeNull();
  expect(screen.queryByRole('progressbar')).toBeNull();
  expect(await screen.findByRole('button', { name: 'reader/private-notes Private' })).toBeTruthy();
  expect(screen.getByAltText('GitHub avatar').getAttribute('src')).toBe(connected.avatar_url);
  vi.mocked(api.githubConnect).mock.calls[0][0]({ ...authorization, user_code: 'LATE-CODE' });
  await waitFor(() => expect(screen.queryByText('LATE-CODE')).toBeNull());
});

it('cancels before notification and ignores old notifications and completion during a retry', async () => {
  const pending = deferred<GithubStatus>();
  const retry = deferred<GithubStatus>();
  vi.mocked(api.githubConnect).mockReturnValueOnce(pending.promise).mockReturnValueOnce(retry.promise);
  render(GithubAccount, { picker: true });
  await fireEvent.click(await screen.findByRole('button', { name: 'Mit GitHub verbinden' }));
  expect(screen.getByText('Requesting GitHub device code…')).toBeTruthy();
  expect(screen.getByRole('progressbar', { name: 'GitHub authorization' }).hasAttribute('value')).toBe(false);
  expect(screen.queryByRole('button', { name: 'Code kopieren' })).toBeNull();
  const oldNotification = vi.mocked(api.githubConnect).mock.calls[0][0];
  await fireEvent.click(screen.getByRole('button', { name: 'Abbrechen' }));
  expect(api.githubCancel).toHaveBeenCalledOnce();
  expect(await screen.findByText('Connection cancelled.')).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'Mit GitHub verbinden' }));
  const notification = vi.mocked(api.githubConnect).mock.calls[1][0];
  notification(authorization);
  expect(await screen.findByText(authorization.user_code)).toBeTruthy();
  oldNotification({ ...authorization, user_code: 'STALE-CODE' });
  pending.resolve(connected);
  await waitFor(() => expect(screen.queryByText('Connected as reader')).toBeNull());
  expect(screen.queryByText('STALE-CODE')).toBeNull();
  expect(screen.getByText(authorization.user_code)).toBeTruthy();
  expect(api.githubRepositories).not.toHaveBeenCalled();
  await fireEvent.click(screen.getByRole('button', { name: 'Abbrechen' }));
  expect(await screen.findByText('Connection cancelled.')).toBeTruthy();
  expect(screen.queryByText(authorization.user_code)).toBeNull();
  retry.reject({ message: 'Stale failure' });
  await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
});

it.each(['before', 'after'] as const)('preserves cleanup failures %s cancellation resolves and lets disconnected users retry secure cleanup', async order => {
  const pending = deferred<GithubStatus>();
  const cancellation = deferred<void>();
  vi.mocked(api.githubConnect).mockReturnValue(pending.promise);
  vi.mocked(api.githubCancel).mockReturnValue(cancellation.promise);
  render(GithubAccount);
  await fireEvent.click(await screen.findByRole('button', { name: 'Mit GitHub verbinden' }));
  await fireEvent.click(screen.getByRole('button', { name: 'Abbrechen' }));
  if (order === 'after') {
    cancellation.resolve();
    await screen.findByText('Connection cancelled.');
  }
  pending.reject({ message: cleanupFailure });
  expect((await screen.findByRole('alert')).textContent).toBe(cleanupFailure);
  if (order === 'before') cancellation.resolve();
  const cleanup = await screen.findByRole('button', { name: 'Disconnect GitHub and clean up credentials' });
  await waitFor(() => expect((cleanup as HTMLButtonElement).disabled).toBe(false));
  expect(screen.getByText('Not connected')).toBeTruthy();
  expect(screen.queryByText('Connection cancelled.')).toBeNull();
  expect(screen.queryByRole('progressbar')).toBeNull();
  vi.mocked(api.githubDisconnect).mockRejectedValueOnce({ message: 'Credential cleanup still unavailable' });
  await fireEvent.click(cleanup);
  expect((await screen.findByRole('alert')).textContent).toContain('Credential cleanup still unavailable');
  await fireEvent.click(screen.getByRole('button', { name: 'Disconnect GitHub and clean up credentials' }));
  expect(api.githubDisconnect).toHaveBeenCalledTimes(2);
  expect(await screen.findByText(/GitHub disconnected/)).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.queryByRole('button', { name: /clean up credentials/ })).toBeNull();
});

it.each(['pending', 'connected', 'cancelled', 'disconnected'] as const)('does not let old cleanup failure overwrite a newer %s sign-in generation', async state => {
  const pending = deferred<GithubStatus>();
  const retry = deferred<GithubStatus>();
  vi.mocked(api.githubConnect).mockReturnValueOnce(pending.promise).mockImplementationOnce(onDevice => {
    onDevice(authorization);
    return retry.promise;
  });
  render(GithubAccount);
  await fireEvent.click(await screen.findByRole('button', { name: 'Mit GitHub verbinden' }));
  await fireEvent.click(screen.getByRole('button', { name: 'Abbrechen' }));
  await screen.findByText('Connection cancelled.');
  await fireEvent.click(screen.getByRole('button', { name: 'Mit GitHub verbinden' }));
  if (state === 'connected' || state === 'disconnected') {
    retry.resolve(connected);
    await screen.findByText('Connected as reader');
    if (state === 'disconnected') {
      await fireEvent.click(screen.getByRole('button', { name: 'Disconnect GitHub' }));
      await screen.findByText(/GitHub disconnected/);
    }
  } else if (state === 'cancelled') {
    await fireEvent.click(screen.getByRole('button', { name: 'Abbrechen' }));
    await screen.findByText('Connection cancelled.');
  }
  pending.reject({ message: cleanupFailure });
  await waitFor(() => {
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByRole('button', { name: /clean up credentials/ })).toBeNull();
  });
  if (state === 'pending') expect(screen.getByText(authorization.user_code)).toBeTruthy();
  if (state === 'connected') expect(screen.getByText('Connected as reader')).toBeTruthy();
  if (state === 'cancelled') expect(screen.getByText('Connection cancelled.')).toBeTruthy();
  if (state === 'disconnected') expect(screen.getByText(/GitHub disconnected/)).toBeTruthy();
});

it.each(['Authorization denied', 'Authorization expired', 'Network unavailable'])('reports %s, clears the code and allows retry', async message => {
  vi.mocked(api.githubConnect).mockImplementationOnce(onDevice => {
    onDevice(authorization);
    return Promise.reject({ message });
  });
  render(GithubAccount);
  await fireEvent.click(await screen.findByRole('button', { name: 'Mit GitHub verbinden' }));
  expect((await screen.findByRole('alert')).textContent).toContain(message);
  expect(screen.queryByText(authorization.user_code)).toBeNull();
  await fireEvent.click(screen.getByRole('button', { name: 'Mit GitHub verbinden' }));
  expect(await screen.findByText('Connected as reader')).toBeTruthy();
  expect(api.githubRepositories).not.toHaveBeenCalled();
});

it('keeps unavailable builds explicit without obsolete broker setup instructions', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue({ ...disconnected, configured: false });
  render(GithubAccount);
  expect((await screen.findByRole('note')).textContent).toContain('Vellistry');
  expect((screen.getByRole('button', { name: 'Mit GitHub verbinden' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: /setup documentation/ })).toBeNull();
  expect(openUrl).not.toHaveBeenCalled();
  expect(api.githubConnect).not.toHaveBeenCalled();
});

it('reports clipboard and browser failures, supports retry, and never opens the notified URI', async () => {
  vi.mocked(api.githubConnect).mockImplementation(onDevice => {
    onDevice({ ...authorization, verification_uri: 'https://phishing.invalid/device' });
    return new Promise(() => {});
  });
  render(GithubAccount);
  await fireEvent.click(await screen.findByRole('button', { name: 'Mit GitHub verbinden' }));
  vi.mocked(navigator.clipboard.writeText).mockRejectedValueOnce(new Error('Clipboard denied'));
  await fireEvent.click(screen.getByRole('button', { name: 'Code kopieren' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Clipboard denied');
  await fireEvent.click(screen.getByRole('button', { name: 'Code kopieren' }));
  expect(await screen.findByText('Code copied.')).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
  vi.mocked(openUrl).mockRejectedValueOnce(new Error('Browser unavailable'));
  await fireEvent.click(screen.getByRole('button', { name: 'GitHub öffnen' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Browser unavailable');
  await fireEvent.click(screen.getByRole('button', { name: 'GitHub öffnen' }));
  expect(screen.queryByRole('alert')).toBeNull();
  expect(openUrl).toHaveBeenNthCalledWith(1, 'https://github.com/login/device');
  expect(openUrl).toHaveBeenNthCalledWith(2, 'https://github.com/login/device');
  expect(screen.getByText(authorization.user_code)).toBeTruthy();
});

it('handles unavailable clipboard access without abandoning authorization', async () => {
  vi.mocked(api.githubConnect).mockImplementation(onDevice => {
    onDevice(authorization);
    return new Promise(() => {});
  });
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined });
  render(GithubAccount);
  await fireEvent.click(await screen.findByRole('button', { name: 'Mit GitHub verbinden' }));
  await fireEvent.click(screen.getByRole('button', { name: 'Code kopieren' }));
  expect(await screen.findByRole('alert')).toBeTruthy();
  expect(screen.getByText(authorization.user_code)).toBeTruthy();
});

it.each(['copy', 'open'] as const)('ignores a late %s result after cancellation and retry', async action => {
  const pendingAction = deferred<void>();
  vi.mocked(api.githubConnect).mockImplementation(onDevice => {
    onDevice(authorization);
    return new Promise(() => {});
  });
  if (action === 'copy') vi.mocked(navigator.clipboard.writeText).mockReturnValueOnce(pendingAction.promise);
  else vi.mocked(openUrl).mockReturnValueOnce(pendingAction.promise);
  render(GithubAccount);
  await fireEvent.click(await screen.findByRole('button', { name: 'Mit GitHub verbinden' }));
  await fireEvent.click(screen.getByRole('button', { name: action === 'copy' ? 'Code kopieren' : 'GitHub öffnen' }));
  await fireEvent.click(screen.getByRole('button', { name: 'Abbrechen' }));
  await screen.findByText('Connection cancelled.');
  await fireEvent.click(screen.getByRole('button', { name: 'Mit GitHub verbinden' }));
  if (action === 'copy') pendingAction.resolve();
  else pendingAction.reject(new Error('Stale browser error'));
  await waitFor(() => {
    expect(screen.queryByText('Code copied.')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
  });
});

it('ignores notifications and completion after unmount', async () => {
  const pending = deferred<GithubStatus>();
  vi.mocked(api.githubConnect).mockReturnValue(pending.promise);
  const mounted = render(GithubAccount, { picker: true });
  await fireEvent.click(await screen.findByRole('button', { name: 'Mit GitHub verbinden' }));
  const notification = vi.mocked(api.githubConnect).mock.calls[0][0];
  mounted.unmount();
  expect(api.githubCancel).toHaveBeenCalledOnce();
  notification(authorization);
  pending.resolve(connected);
  await waitFor(() => expect(api.githubRepositories).not.toHaveBeenCalled());
  expect(openUrl).not.toHaveBeenCalled();
  expect(screen.queryByText(authorization.user_code)).toBeNull();
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
  expect(api.addGithub).toHaveBeenCalledWith(repositories[1].clone_url, true);
});

it('opens native repository management on request, refreshes access, and disconnects without deleting workspaces', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  render(GithubAccount, { picker: true });
  await screen.findByRole('button', { name: 'reader/private-notes Private' });
  expect(api.githubManage).not.toHaveBeenCalled();
  await fireEvent.click(screen.getByRole('button', { name: /Repository-Zugriff verwalten/ }));
  expect(api.githubManage).toHaveBeenCalledOnce();
  expect(openUrl).not.toHaveBeenCalled();
  vi.mocked(api.githubRepositories).mockResolvedValue([]);
  await fireEvent.click(screen.getByRole('button', { name: 'Refresh repositories' }));
  expect(await screen.findByRole("heading", { name: "Repository-Zugriff einrichten" })).toBeTruthy();
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
  await fireEvent.click(screen.getByRole('button', { name: /Repository-Zugriff verwalten/ }));
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
  await fireEvent.click(await screen.findByRole('button', { name: 'Mit GitHub verbinden' }));
  await fireEvent.click(screen.getByRole('button', { name: 'Abbrechen' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Cancel failed');
  await fireEvent.click(screen.getByRole('button', { name: 'Abbrechen' }));
  expect(await screen.findByText('Connection cancelled.')).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'Mit GitHub verbinden' }));
  mounted.unmount();
  expect(api.githubCancel).toHaveBeenCalledTimes(3);
});

it('explains separate repository grants only after a successful empty response', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  vi.mocked(api.githubRepositories).mockResolvedValue([]);
  render(GithubAccount, { picker: true });
  expect(await screen.findByRole('heading', { name: 'Repository-Zugriff einrichten' })).toBeTruthy();
  expect(screen.getByText(/Falls die App noch nicht installiert ist/)).toBeTruthy();
  vi.mocked(api.githubRepositories).mockRejectedValueOnce(new Error('Access unavailable'));
  await fireEvent.click(screen.getByRole('button', { name: 'Refresh repositories' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Access unavailable');
  expect(screen.queryByRole('heading', { name: 'Repository-Zugriff einrichten' })).toBeNull();
  expect(screen.queryByText('No matching repositories.')).toBeNull();
});

it('refreshes once on return even before browser launch resolves, not on ordinary focus', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  const launch = deferred<void>();
  const refresh = deferred<typeof repositories>();
  vi.mocked(api.githubManage).mockReturnValue(launch.promise);
  render(GithubAccount, { picker: true });
  await screen.findByRole('button', { name: 'reader/private-notes Private' });
  await fireEvent.focus(window);
  expect(api.githubRepositories).toHaveBeenCalledTimes(1);
  vi.mocked(api.githubRepositories).mockReturnValueOnce(refresh.promise);
  await fireEvent.click(screen.getByRole('button', { name: /Repository-Zugriff verwalten/ }));
  await fireEvent.focus(window);
  await fireEvent(document, new Event('visibilitychange'));
  await fireEvent.focus(window);
  expect(api.githubRepositories).toHaveBeenCalledTimes(2);
  refresh.resolve([]);
  launch.resolve();
  await screen.findByRole('heading', { name: 'Repository-Zugriff einrichten' });
  await fireEvent.focus(window);
  expect(api.githubRepositories).toHaveBeenCalledTimes(2);
});

it('refreshes on visibility return and preserves a manual retry after auto-refresh fails', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  render(GithubAccount, { picker: true });
  await screen.findByRole('button', { name: 'reader/private-notes Private' });
  await fireEvent.click(screen.getByRole('button', { name: /Repository-Zugriff verwalten/ }));
  vi.mocked(api.githubRepositories).mockRejectedValueOnce(new Error('Refresh failed'));
  await fireEvent(document, new Event('visibilitychange'));
  expect((await screen.findByRole('alert')).textContent).toContain('Refresh failed');
  await fireEvent.focus(window);
  expect(api.githubRepositories).toHaveBeenCalledTimes(2);
  await fireEvent.click(screen.getByRole('button', { name: 'Refresh repositories' }));
  await waitFor(() => expect(api.githubRepositories).toHaveBeenCalledTimes(3));
  expect(screen.queryByRole('alert')).toBeNull();
});

it('manual refresh consumes the pending return refresh and removes native listeners on unmount', async () => {
  const unlisten = vi.fn();
  const onFocusChanged = vi.fn().mockResolvedValue(unlisten);
  vi.mocked(getCurrentWindow).mockReturnValue({ onFocusChanged } as unknown as ReturnType<typeof getCurrentWindow>);
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  const mounted = render(GithubAccount, { picker: true });
  await screen.findByRole('button', { name: 'reader/private-notes Private' });
  await waitFor(() => expect(onFocusChanged).toHaveBeenCalledOnce());
  await fireEvent.click(screen.getByRole('button', { name: /Repository-Zugriff verwalten/ }));
  onFocusChanged.mock.calls[0][0]({ payload: false });
  expect(api.githubRepositories).toHaveBeenCalledTimes(1);
  await fireEvent.click(screen.getByRole('button', { name: 'Refresh repositories' }));
  await waitFor(() => expect(api.githubRepositories).toHaveBeenCalledTimes(2));
  onFocusChanged.mock.calls[0][0]({ payload: true });
  await fireEvent.focus(window);
  expect(api.githubRepositories).toHaveBeenCalledTimes(2);
  mounted.unmount();
  expect(unlisten).toHaveBeenCalledOnce();
});

it.each(['resolve', 'reject'])('ignores a stale repository %s after unmount', async outcome => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  const pending = deferred<typeof repositories>();
  vi.mocked(api.githubRepositories).mockReturnValue(pending.promise);
  const mounted = render(GithubAccount, { picker: true });
  await waitFor(() => expect(api.githubRepositories).toHaveBeenCalledOnce());
  mounted.unmount();
  if (outcome === 'resolve') pending.resolve([]);
  else pending.reject(new Error('Late repository failure'));
  await pending.promise.catch(() => {});
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.queryByRole('heading', { name: 'Repository-Zugriff einrichten' })).toBeNull();
});

it('does not arm return refresh when opening management fails', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  vi.mocked(api.githubManage).mockRejectedValue(new Error('Browser failed'));
  render(GithubAccount, { picker: true });
  await screen.findByRole('button', { name: 'reader/private-notes Private' });
  await fireEvent.click(screen.getByRole('button', { name: /Repository-Zugriff verwalten/ }));
  await screen.findByRole('alert');
  await fireEvent.focus(window);
  expect(api.githubRepositories).toHaveBeenCalledTimes(1);
});

it('shows a disappeared native account error without retrying through manual credentials', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  const message = 'No GitHub account is connected. Reconnect GitHub before adding this repository.';
  vi.mocked(api.addGithub).mockRejectedValue({ code: 'git', message });
  const onadded = vi.fn();
  render(GithubAccount, { picker: true, onadded });
  await fireEvent.click(await screen.findByRole('button', { name: 'reader/private-notes Private' }));
  expect((await screen.findByRole('alert')).textContent).toContain(message);
  expect(vi.mocked(api.addGithub).mock.calls).toEqual([[repositories[1].clone_url, true]]);
  expect(onadded).not.toHaveBeenCalled();
  expect(screen.queryByRole('heading', { name: 'Repository-Zugriff einrichten' })).toBeNull();
});

it('retries the failed selected repository with subtle clone/index progress', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  const clone = deferred<Awaited<ReturnType<typeof api.addGithub>>>();
  const workspace = { id: 7, name: 'private-notes', kind: 'github' as const, root: '/repo', url: repositories[1].clone_url };
  vi.mocked(api.addGithub).mockReturnValueOnce(clone.promise).mockResolvedValueOnce(workspace);
  const onadded = vi.fn();
  render(GithubAccount, { picker: true, onadded });
  await fireEvent.click(await screen.findByRole('button', { name: 'reader/private-notes Private' }));
  expect(screen.getByText('Repository wird geklont und indexiert…')).toBeTruthy();
  clone.reject(new Error('Clone failed'));
  await fireEvent.click(await screen.findByRole('button', { name: 'reader/private-notes erneut versuchen' }));
  await waitFor(() => expect(onadded).toHaveBeenCalledWith(workspace));
  expect(vi.mocked(api.addGithub).mock.calls).toEqual([[repositories[1].clone_url, true], [repositories[1].clone_url, true]]);
});

it('cleans up native focus registration even when it completes after unmount', async () => {
  const registration = deferred<() => void>();
  const unlisten = vi.fn();
  const onFocusChanged = vi.fn().mockReturnValue(registration.promise);
  vi.mocked(getCurrentWindow).mockReturnValue({ onFocusChanged } as unknown as ReturnType<typeof getCurrentWindow>);
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  const mounted = render(GithubAccount, { picker: true });
  await screen.findByRole('button', { name: 'reader/private-notes Private' });
  await waitFor(() => expect(onFocusChanged).toHaveBeenCalledOnce());
  await fireEvent.click(screen.getByRole('button', { name: /Repository-Zugriff verwalten/ }));
  const late = deferred<typeof repositories>();
  vi.mocked(api.githubRepositories).mockReturnValueOnce(late.promise);
  onFocusChanged.mock.calls[0][0]({ payload: true });
  await waitFor(() => expect(api.githubRepositories).toHaveBeenCalledTimes(2));
  mounted.unmount();
  registration.resolve(unlisten);
  late.resolve([]);
  await waitFor(() => expect(unlisten).toHaveBeenCalledOnce());
  onFocusChanged.mock.calls[0][0]({ payload: true });
  await fireEvent.focus(window);
  await fireEvent(document, new Event('visibilitychange'));
  expect(api.githubRepositories).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole('heading', { name: 'Repository-Zugriff einrichten' })).toBeNull();
});

it('ignores a late clone completion after unmount', async () => {
  vi.mocked(api.githubStatus).mockResolvedValue(connected);
  const clone = deferred<Awaited<ReturnType<typeof api.addGithub>>>();
  vi.mocked(api.addGithub).mockReturnValueOnce(clone.promise);
  const onadded = vi.fn();
  const mounted = render(GithubAccount, { picker: true, onadded });
  await fireEvent.click(await screen.findByRole('button', { name: 'reader/private-notes Private' }));
  mounted.unmount();
  clone.resolve({ id: 7, name: 'private-notes', kind: 'github', root: '/repo', url: repositories[1].clone_url });
  await clone.promise;
  expect(onadded).not.toHaveBeenCalled();
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
