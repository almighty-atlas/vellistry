<script lang="ts">
  import { onMount } from 'svelte';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api } from './api';
  import { errorMessage } from './helpers';
  import type { DeviceAuthorization, GithubRepository, GithubStatus, Workspace } from './types';

  let { picker = false, onadded }: { picker?: boolean; onadded?: (workspace: Workspace) => void | Promise<void> } = $props();
  let status = $state<GithubStatus | null>(null);
  let repositories = $state<GithubRepository[]>([]);
  let query = $state('');
  let visibility = $state('all');
  let loading = $state(true);
  let connecting = $state(false);
  let device = $state<DeviceAuthorization | null>(null);
  let cancelling = $state(false);
  let working = $state(false);
  let loadingRepositories = $state(false);
  let repositoriesLoaded = $state(false);
  let error = $state('');
  let notice = $state('');
  let cleanupRequired = $state(false);
  let selected = $state<GithubRepository | null>(null);
  let retryRepo = $state<GithubRepository | null>(null);
  let refreshOnReturn = false;
  let alive = true;
  let generation = 0;
  let cancelledRequest: number | null = null;
  const cleanupFailurePrefix = 'GitHub sign-in cancelled, but secure credential cleanup failed.';
  const disabled = $derived(loading || connecting || cancelling || working || loadingRepositories);
  const filtered = $derived(repositories.filter(repo => repo.full_name.toLowerCase().includes(query.trim().toLowerCase()) && (visibility === 'all' || repo.private === (visibility === 'private'))));
  // Rust downloads and validates avatars; the webview never requests remote images.
  const avatar = $derived(status?.avatar_url && /^data:image\/(png|jpeg|gif|webp);base64,/i.test(status.avatar_url) ? status.avatar_url : null);

  onMount(() => {
    void loadStatus();
    const focus = () => {
      if (!alive || !refreshOnReturn || !picker || !status?.connected || loadingRepositories) return;
      refreshOnReturn = false;
      void loadRepositories();
    };
    const visible = () => { if (document.visibilityState === 'visible') focus(); };
    window.addEventListener('focus', focus);
    document.addEventListener('visibilitychange', visible);
    let unlisten: (() => void) | undefined;
    void import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
      const stop = await getCurrentWindow().onFocusChanged?.(event => { if (event.payload) focus(); });
      if (!alive) stop?.(); else unlisten = stop;
    }).catch(() => {});
    return () => {
      alive = false; generation++; refreshOnReturn = false;
      window.removeEventListener('focus', focus);
      document.removeEventListener('visibilitychange', visible);
      unlisten?.();
      if (connecting) void api.githubCancel().catch(() => {});
    };
  });

  async function loadRepositories() {
    if (!alive || loadingRepositories) return;
    refreshOnReturn = false;
    const request = generation;
    loadingRepositories = true; repositoriesLoaded = false; error = '';
    try {
      const result = await api.githubRepositories();
      if (alive && request === generation) { repositories = result; repositoriesLoaded = true; }
    } catch (e) { if (alive && request === generation) error = errorMessage(e); }
    finally { if (alive && request === generation) loadingRepositories = false; }
  }
  async function loadStatus() {
    const request = generation;
    loading = true; error = '';
    try {
      const result = await api.githubStatus();
      if (!alive || request !== generation) return;
      status = result;
      if (result.connected && picker) await loadRepositories();
    } catch (e) { if (alive && request === generation) error = errorMessage(e); }
    finally { if (alive && request === generation) loading = false; }
  }
  async function connect() {
    if (disabled || !status?.configured) return;
    const request = ++generation;
    cancelledRequest = null; cleanupRequired = false;
    connecting = true; device = null; error = ''; notice = '';
    try {
      // The invoke stays pending while Rust polls; the channel delivers the public user code.
      const result = await api.githubConnect(authorization => {
        if (alive && request === generation && connecting) device = authorization;
      });
      if (!alive || request !== generation) return;
      status = result; connecting = false; device = null;
      if (result.connected && picker) await loadRepositories();
    } catch (e) {
      const message = errorMessage(e);
      const cleanupFailed = message.startsWith(cleanupFailurePrefix);
      // Cancellation invalidates ordinary results, but rollback failures still need user action.
      if (alive && (request === generation || (request === cancelledRequest && cleanupFailed))) {
        error = message;
        if (cleanupFailed) { cleanupRequired = true; notice = ''; }
      }
    }
    finally { if (alive && request === generation) { connecting = false; device = null; } }
  }
  async function cancel() {
    if (!connecting || cancelling) return;
    cancelledRequest ??= generation;
    const request = ++generation;
    cancelling = true; device = null; error = ''; notice = '';
    try {
      await api.githubCancel();
      if (alive && request === generation) {
        connecting = false; loadingRepositories = false;
        if (!cleanupRequired) notice = 'Connection cancelled.';
      }
    } catch (e) { if (alive && request === generation) error = errorMessage(e); }
    finally { if (alive && request === generation) cancelling = false; }
  }
  async function disconnect() {
    if (disabled) return;
    ++generation; cancelledRequest = null; refreshOnReturn = false;
    working = true; error = ''; notice = '';
    try {
      await api.githubDisconnect();
      if (!alive) return;
      cleanupRequired = false;
      status = status && { ...status, connected: false, login: null, avatar_url: null };
      repositories = []; query = ''; repositoriesLoaded = false; loadingRepositories = false; retryRepo = null;
      notice = 'GitHub disconnected. Existing local workspaces are unchanged.';
    } catch (e) { if (alive) error = errorMessage(e); }
    finally { if (alive) working = false; }
  }
  async function manage() {
    if (disabled) return;
    working = true; error = ''; notice = '';
    refreshOnReturn = true;
    try { await api.githubManage(); if (alive && refreshOnReturn) notice = 'Repository-Zugriff im Browser geöffnet. Bei der Rückkehr wird die Liste aktualisiert; alternativ manuell aktualisieren.'; }
    catch (e) { if (alive) { refreshOnReturn = false; error = errorMessage(e); } }
    finally { if (alive) working = false; }
  }
  async function add(repo: GithubRepository) {
    if (disabled) return;
    selected = repo; retryRepo = null;
    working = true; error = ''; notice = 'Repository wird geklont und indexiert…';
    try {
      const workspace = await api.addGithub(repo.clone_url, true);
      if (alive) { notice = `Added ${workspace.name}`; await onadded?.(workspace); }
    } catch (e) { if (alive) { error = errorMessage(e); notice = ''; retryRepo = repo; } }
    finally { if (alive) { working = false; selected = null; } }
  }
  async function deviceAction(action: 'copy' | 'open') {
    if (!connecting || cancelling || !device) return;
    const request = generation;
    error = ''; notice = '';
    try {
      if (action === 'copy') await navigator.clipboard.writeText(device.user_code);
      else await openUrl('https://github.com/login/device');
      if (alive && request === generation && connecting && action === 'copy') notice = 'Code copied.';
    } catch (e) {
      if (alive && request === generation && connecting) error = errorMessage(e);
    }
  }
</script>

<section class="github-account" aria-label="GitHub account">
  <div class="account-heading">
    {#if avatar}<img src={avatar} alt="GitHub avatar" width="36" height="36" referrerpolicy="no-referrer" />{:else}<svg aria-hidden="true" width="28" height="28" viewBox="0 0 24 24" fill="currentColor"><path d="M12 .5a12 12 0 0 0-3.8 23.4c.6.1.8-.3.8-.6v-2.2c-3.3.7-4-1.4-4-1.4-.5-1.4-1.3-1.8-1.3-1.8-1.1-.7.1-.7.1-.7 1.2.1 1.9 1.3 1.9 1.3 1.1 1.9 2.8 1.4 3.4 1 .1-.8.4-1.4.8-1.7-2.7-.3-5.5-1.4-5.5-6a4.7 4.7 0 0 1 1.2-3.2c-.1-.3-.5-1.5.1-3.1 0 0 1-.3 3.3 1.2a11.5 11.5 0 0 1 6 0C17.3 5.2 18.3 5.5 18.3 5.5c.6 1.6.2 2.8.1 3.1a4.7 4.7 0 0 1 1.2 3.2c0 4.6-2.8 5.7-5.5 6 .4.4.8 1.1.8 2.2v3.3c0 .3.2.7.8.6A12 12 0 0 0 12 .5Z" /></svg>{/if}
    <div><strong>GitHub</strong><p class="muted">{loading ? 'Checking account…' : status?.connected ? `Connected${status.login ? ` as ${status.login}` : ''}` : 'Not connected'}</p></div>
  </div>
  {#if status && !status.configured}
    <p role="note">GitHub sign-in is unavailable in this build of Vellistry. You can still add a repository by URL using your existing Git credentials.</p>
  {/if}
  {#if connecting}
    <section class="device-authorization" aria-labelledby="github-device-heading" aria-describedby="github-device-hint">
      <h3 id="github-device-heading">Vellistry mit GitHub verbinden</h3>
      {#if device}
        <p>Gib diesen Code auf GitHub ein:</p>
        <p class="device-code" aria-label="GitHub Gerätecode"><code>{device.user_code}</code></p>
        <div class="account-actions">
          <button onclick={() => deviceAction('copy')} disabled={cancelling}>Code kopieren</button>
          <button onclick={() => deviceAction('open')} disabled={cancelling}>GitHub öffnen</button>
        </div>
        <p>Der Code ist {device.expires_in} Sekunden gültig. Vellistry prüft die Freigabe automatisch.</p>
      {/if}
      <p id="github-device-hint">Offizielle GitHub-Seite: https://github.com/login/device. Prüfe die Adresse im Browser und gib nur den hier angezeigten Code ein. Teile den Code nicht mit anderen und verwende keine fremden Codes (Phishing).</p>
      <progress aria-label="GitHub authorization"></progress>
      <p role="status">{cancelling ? 'Cancelling…' : device ? 'Waiting for GitHub authorization…' : 'Requesting GitHub device code…'}</p>
      <button onclick={cancel} disabled={cancelling}>Abbrechen</button>
    </section>
  {:else if status?.connected}
    <div class="account-actions"><button class="muted" onclick={manage} disabled={disabled}>Repository-Zugriff verwalten ↗</button><button onclick={disconnect} disabled={disabled}>Disconnect GitHub</button></div>
  {:else if status}
    <button class="primary" onclick={connect} disabled={disabled || !status.configured}>Mit GitHub verbinden</button>
  {/if}
  {#if !status && !loading}<button onclick={loadStatus}>Retry account status</button>{/if}
  {#if picker && status?.connected}
    <div class="repositories">
      <div class="account-actions"><strong>Repositories</strong><button onclick={loadRepositories} disabled={disabled || loadingRepositories}>Refresh repositories</button></div>
      <input aria-label="Search repositories" placeholder="Search repositories…" bind:value={query} />
      <select aria-label="Repository visibility" bind:value={visibility}><option value="all">Public and private</option><option value="public">Public</option><option value="private">Private</option></select>
      {#if loadingRepositories}<p role="status">Loading repositories…</p>
      {:else if repositoriesLoaded && !error && !repositories.length}
              <section class="repository-setup" aria-labelledby="repository-setup-heading">
                <h3 id="repository-setup-heading">Repository-Zugriff einrichten</h3>
                <p>Dein GitHub-Konto ist verbunden. Der Zugriff auf Repositories wird separat über die GitHub App freigegeben.</p>
                <p>Falls die App noch nicht installiert ist, installiere sie für dein Konto oder deine Organisation. Falls sie bereits installiert ist, wähle die Repositories aus, die Vellistry lesen darf. Eine Organisationsfreigabe oder eine aktive SSO-Sitzung kann erforderlich sein.</p>
                <button class="primary" onclick={manage} disabled={disabled}>Repository-Zugriff einrichten ↗</button>
                <p class="muted">Nach der Rückkehr wird die Liste automatisch aktualisiert. Du kannst sie auch manuell aktualisieren.</p>
              </section>
      {:else if repositoriesLoaded && !error && repositories.length > 0 && !filtered.length}<p>No matching repositories.</p>{/if}
      <ul>{#each filtered as repo (repo.id)}<li><button aria-label={`${repo.full_name} ${repo.private ? 'Private' : 'Public'}`} onclick={() => add(repo)} disabled={disabled}><span>{repo.full_name}{#if selected?.id === repo.id}<span class="muted"> · Klonen und Indexieren…</span>{/if}</span><small>{repo.private ? 'Private' : 'Public'}</small></button></li>{/each}</ul>
    </div>
  {/if}
  {#if error}<p role="alert" class="error-text">{error}</p>{/if}
  {#if retryRepo}<button onclick={() => retryRepo && add(retryRepo)} disabled={disabled}>{retryRepo.full_name} erneut versuchen</button>{/if}
  {#if cleanupRequired && !status?.connected}
    <button onclick={disconnect} disabled={disabled}>Disconnect GitHub and clean up credentials</button>
  {/if}
  {#if notice}<p role="status" class="muted">{notice}</p>{/if}
</section>

<style>
  .github-account { padding: 18px 0; border-bottom: 1px solid var(--line); font-size: 13px; }
  .account-heading, .account-actions { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .account-heading img { border-radius: 50%; }
  p { line-height: 1.6; }
  .account-heading p { margin: 4px 0; }
  .device-authorization { margin-top: 16px; padding: 16px; border: 1px solid var(--line); border-radius: 8px; }
  .device-code { font-size: 24px; font-weight: 700; letter-spacing: .12em; overflow-wrap: anywhere; user-select: all; }
  .repository-setup { margin-top: 14px; padding: 16px; border: 1px solid var(--line); border-radius: 8px; }
  .repository-setup h3 { margin-top: 0; }
  .repositories { margin-top: 18px; }
  .repositories input { width: 100%; margin: 10px 0; }
  ul { list-style: none; padding: 0; max-height: 230px; overflow: auto; }
  li button { width: 100%; display: flex; justify-content: space-between; gap: 12px; text-align: left; }
  li span { overflow-wrap: anywhere; }
  small { color: var(--muted); }
</style>
