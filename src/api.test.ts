import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Channel, invoke, SERIALIZE_TO_IPC_FN } from '@tauri-apps/api/core';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import type { DeviceAuthorization, GithubStatus } from './types';
import { api } from './api';

vi.mock('@tauri-apps/api/core', async importOriginal => ({
  ...await importOriginal<typeof import('@tauri-apps/api/core')>(),
  invoke: vi.fn(),
}));
beforeEach(() => {
  vi.mocked(invoke).mockReset().mockResolvedValue(undefined);
  mockIPC(() => undefined);
});
afterEach(() => clearMocks());
it('uses exact backend command names and camelCase invoke arguments', async () => {
  await api.githubStatus(); await api.githubConnect(vi.fn()); await api.githubCancel();
  await api.githubDisconnect(); await api.githubRepositories(); await api.githubManage();
  await api.workspaces();
  await api.addLocal('/notes');
  await api.addGithub('https://github.com/owner/repo');
  await api.sync(7); await api.refresh(7); await api.documents(7);
  await api.read(7, 'guide.md');
  await api.save(7, 'guide.md', '# Draft', 'sha256');
  await api.search('guide');
  await api.search('guide', 7);
  await api.resolve(7, 'guide.md', '../intro.md#hello');
  await api.asset(7, 'image.png');
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ['github_status', {}], ['github_connect', { onDevice: expect.any(Channel) }], ['github_cancel', {}],
    ['github_disconnect', {}], ['github_repositories', {}], ['github_manage', {}],
    ['list_workspaces', {}],
    ['add_local', { path: '/notes' }],
    ['add_github', { url: 'https://github.com/owner/repo' }],
    ['sync_workspace', { workspaceId: 7 }],
    ['refresh_workspace', { workspaceId: 7 }],
    ['list_documents', { workspaceId: 7 }],
    ['read_document', { workspaceId: 7, path: 'guide.md' }],
    ['save_document', { workspaceId: 7, path: 'guide.md', content: '# Draft', expectedRevision: 'sha256' }],
    ['search', { query: 'guide', workspaceId: null }],
    ['search', { query: 'guide', workspaceId: 7 }],
    ['resolve_link', { workspaceId: 7, path: 'guide.md', href: '../intro.md#hello' }],
    ['read_asset', { workspaceId: 7, path: 'image.png' }],
  ]);
});

it('requires the native account only when explicitly requested by the picker', async () => {
  const url = 'https://github.com/owner/repo';
  await api.addGithub(url);
  await api.addGithub(url, false);
  await api.addGithub(url, true);
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ['add_github', { url }],
    ['add_github', { url }],
    ['add_github', { url, nativeAccount: true }],
  ]);
});

it('installs the real device channel handler before invoking and returns the final account status', async () => {
  const authorization: DeviceAuthorization = { user_code: 'ABCD-EFGH', verification_uri: 'https://github.com/login/device', expires_in: 900, interval: 5 };
  const status: GithubStatus = { configured: true, connected: true, login: 'reader', avatar_url: null };
  const onDevice = vi.fn();
  let resolve!: (value: GithubStatus) => void;
  const pending = new Promise<GithubStatus>(done => { resolve = done; });
  vi.mocked(invoke).mockImplementationOnce((_command, args) => {
    const channel = (args as { onDevice: Channel<DeviceAuthorization> }).onDevice;
    expect(channel).toBeInstanceOf(Channel);
    expect(channel[SERIALIZE_TO_IPC_FN]()).toMatch(/^__CHANNEL__:\d+$/);
    channel.onmessage(authorization);
    return pending;
  });
  const result = api.githubConnect(onDevice);
  expect(onDevice).toHaveBeenCalledExactlyOnceWith(authorization);
  resolve(status);
  await expect(result).resolves.toEqual(status);
});

it('propagates device authorization errors to the caller', async () => {
  vi.mocked(invoke).mockRejectedValueOnce({ message: 'Authorization expired' });
  await expect(api.githubConnect(vi.fn())).rejects.toEqual({ message: 'Authorization expired' });
});
