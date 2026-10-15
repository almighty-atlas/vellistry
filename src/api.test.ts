import { expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { api } from './api';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
it('uses exact backend command names and camelCase invoke arguments', async () => {
  await api.githubStatus(); await api.githubConnect(); await api.githubCancel();
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
    ['github_status', {}], ['github_connect', {}], ['github_cancel', {}],
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
