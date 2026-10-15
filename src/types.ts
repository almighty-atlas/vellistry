export interface DeviceAuthorization { user_code: string; verification_uri: string; expires_in: number; interval: number }
export interface GithubStatus { configured: boolean; connected: boolean; login: string | null; avatar_url: string | null }
export interface GithubRepository { id: number; full_name: string; private: boolean; clone_url: string }
export interface Workspace { id: number; kind: 'local' | 'github'; name: string; root: string; url: string | null }
export interface DocumentSummary { workspace_id: number; path: string; title: string; revision: string }
export interface TocEntry { level: number; id: string; text: string; source_start: number }
export interface Document extends DocumentSummary { content: string; html: string; toc: TocEntry[] }
export interface SearchHit { workspace_id: number; path: string; title: string; score: number; source_start: number; source_end: number | null; section_title: string; matched_terms: string[]; snippet: string }
export interface RefreshReport { indexed: number; unchanged: number; removed: number }
export type ResolvedLink = { kind: 'document' | 'asset'; path: string; fragment: string | null } | { kind: 'external'; url: string };
export interface Asset { path: string; mime_type: string; bytes: number[] }
export type ErrorCode = 'io' | 'database' | 'workspace_not_found' | 'unsafe_path' | 'unsupported' | 'conflict' | 'invalid_github_url' | 'git' | 'git_authentication' | 'git_credential_helper_start' | 'git_credential_ipc' | 'git_permission_denied' | 'git_network' | 'git_ssh_host_verification' | 'dirty_checkout' | 'too_large' | 'utf8' | 'invalid_link' | 'lock_poisoned';
export interface ErrorInfo { code: ErrorCode; message: string; expected_revision?: string; actual_revision?: string }
