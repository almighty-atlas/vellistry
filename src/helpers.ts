import type { DocumentSummary } from './types';

export interface TreeNode { name: string; path: string; document?: DocumentSummary; children: TreeNode[] }
export function fileTree(documents: DocumentSummary[]): TreeNode[] {
  const root: TreeNode[] = [];
  for (const document of documents) {
    let nodes = root;
    const parts = document.path.split('/');
    parts.forEach((name, index) => {
      const path = parts.slice(0, index + 1).join('/');
      let node = nodes.find(n => n.path === path);
      if (!node) { node = { name, path, children: [] }; nodes.push(node); }
      if (index === parts.length - 1) node.document = document;
      nodes = node.children;
    });
  }
  const sort = (nodes: TreeNode[]) => { nodes.sort((a, b) => Number(!!a.document) - Number(!!b.document) || a.name.localeCompare(b.name)); nodes.forEach(n => sort(n.children)); };
  sort(root);
  return root;
}

export function sameSummaries(a: DocumentSummary[] | undefined, b: DocumentSummary[]): boolean {
  return !!a && a.length === b.length && a.every((doc, i) => doc.workspace_id === b[i].workspace_id && doc.path === b[i].path && doc.title === b[i].title && doc.revision === b[i].revision);
}

// Rust offsets are UTF-8 bytes; CodeMirror positions are UTF-16 code units.
export function byteToPosition(text: string, offset: number): number {
  let bytes = 0, position = 0;
  for (const character of text) {
    const size = new TextEncoder().encode(character).length;
    if (bytes + size > offset) break;
    bytes += size; position += character.length;
  }
  return position;
}
export function isWebUrl(value: string): boolean {
  try { return ['http:', 'https:'].includes(new URL(value).protocol); } catch { return false; }
}
export const gitGuides = {
  ssh: 'https://docs.github.com/en/authentication/connecting-to-github-with-ssh',
  https: 'https://cli.github.com/manual/gh_auth_setup-git',
  fingerprints: 'https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints',
  gcm: 'https://github.com/git-ecosystem/git-credential-manager',
};
export function errorMessage(error: unknown): string {
  if (typeof error === 'object' && error && 'code' in error) {
    const guidance: Record<string, string> = {
      git_authentication: 'Git authentication failed. For SSH, load your key into ssh-agent and register its public key with GitHub. For HTTPS, check your connected GitHub account or run gh auth login then gh auth setup-git, or configure Git Credential Manager. Account authorization is handled by the native app.',
      git_permission_denied: 'Git access was denied. Check the repository URL and account permissions. For a connected GitHub account, use Manage repositories to install the GitHub App and grant repository access, then retry. Organization approval or SSO authorization may be required.',
      git_network: 'Git could not reach the server. Check your connection, DNS, proxy, or firewall, then retry Import or Sync.',
      git_ssh_host_verification: 'SSH host verification failed. Verify GitHub’s published host-key fingerprints before adding the verified key to known_hosts. Do not disable host verification.',
    };
    if (guidance[String(error.code)]) return guidance[String(error.code)];
  }
  if (typeof error === 'object' && error && 'message' in error) return String(error.message);
  return String(error);
}
export function isConflict(error: unknown): boolean {
  return typeof error === 'object' && error !== null && 'code' in error && error.code === 'conflict';
}
export function readStored<T>(key: string, fallback: T): T {
  try { const value = localStorage.getItem(key); return value === null ? fallback : JSON.parse(value); } catch { return fallback; }
}
export function store(key: string, value: unknown) { try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* Storage may be disabled or full. */ } }
