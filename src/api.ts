import { Channel, invoke } from '@tauri-apps/api/core';
import type { Workspace, DocumentSummary, Document, SearchHit, RefreshReport, ResolvedLink, Asset, DeviceAuthorization, GithubStatus, GithubRepository } from './types';

export const api = {
  workspaces: () => invoke<Workspace[]>('list_workspaces', {}),
  addLocal: (path: string) => invoke<Workspace>('add_local', { path }),
  addGithub: (url: string, nativeAccount = false) => invoke<Workspace>('add_github', nativeAccount ? { url, nativeAccount: true } : { url }),
  githubStatus: () => invoke<GithubStatus>('github_status', {}),
  githubConnect: (onDevice: (authorization: DeviceAuthorization) => void) => {
    const channel = new Channel<DeviceAuthorization>();
    channel.onmessage = onDevice;
    return invoke<GithubStatus>('github_connect', { onDevice: channel });
  },
  githubCancel: () => invoke<void>('github_cancel', {}),
  githubDisconnect: () => invoke<void>('github_disconnect', {}),
  githubRepositories: () => invoke<GithubRepository[]>('github_repositories', {}),
  githubManage: () => invoke<void>('github_manage', {}),
  sync: (workspaceId: number) => invoke<RefreshReport>('sync_workspace', { workspaceId }),
  refresh: (workspaceId: number) => invoke<RefreshReport>('refresh_workspace', { workspaceId }),
  documents: (workspaceId: number) => invoke<DocumentSummary[]>('list_documents', { workspaceId }),
  read: (workspaceId: number, path: string) => invoke<Document>('read_document', { workspaceId, path }),
  save: (workspaceId: number, path: string, content: string, expectedRevision: string) => invoke<Document>('save_document', { workspaceId, path, content, expectedRevision }),
  search: (query: string, workspaceId: number | null = null) => invoke<SearchHit[]>('search', { query, workspaceId }),
  resolve: (workspaceId: number, path: string, href: string) => invoke<ResolvedLink>('resolve_link', { workspaceId, path, href }),
  asset: (workspaceId: number, path: string) => invoke<Asset>('read_asset', { workspaceId, path }),
};
