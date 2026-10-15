<script lang="ts">
  import { onMount } from 'svelte';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api } from './api';
  import { errorMessage } from './helpers';
  import type { GithubRepository, GithubStatus, Workspace } from './types';

  let { picker = false, onadded }: { picker?: boolean; onadded?: (workspace: Workspace) => void | Promise<void> } = $props();
  let status = $state<GithubStatus | null>(null);
  let repositories = $state<GithubRepository[]>([]);
  let query = $state('');
  let visibility = $state('all');
  let loading = $state(true);
  let connecting = $state(false);
  let cancelling = $state(false);
  let working = $state(false);
  let loadingRepositories = $state(false);
  let repositoriesLoaded = $state(false);
  let error = $state('');
  let notice = $state('');
  let alive = true;
  let generation = 0;
  const disabled = $derived(loading || connecting || working);
  const filtered = $derived(repositories.filter(repo => repo.full_name.toLowerCase().includes(query.trim().toLowerCase()) && (visibility === 'all' || repo.private === (visibility === 'private'))));
  // Rust downloads and validates avatars; the webview never requests remote images.
  const avatar = $derived(status?.avatar_url && /^data:image\/(png|jpeg|gif|webp);base64,/i.test(status.avatar_url) ? status.avatar_url : null);

  onMount(() => {
    void loadStatus();
    return () => {
      alive = false; generation++;
      if (connecting) void api.githubCancel().catch(() => {});
    };
  });

  async function loadRepositories() {
    const request = generation;
    loadingRepositories = true; repositoriesLoaded = false; error = '';
    try {
      const result = await api.githubRepositories();
      if (alive && request === generation) { repositories = result; repositoriesLoaded = true; }
    } catch (e) { if (alive && request === generation) error = errorMessage(e); }
    finally { if (alive && request === generation) loadingRepositories = false; }
  }
  async function loadStatus() {
    loading = true; error = '';
    try {
      const result = await api.githubStatus();
      if (!alive) return;
      status = result;
      if (result.connected && picker) await loadRepositories();
    } catch (e) { if (alive) error = errorMessage(e); }
    finally { if (alive) loading = false; }
  }
  async function connect() {
    if (disabled || !status?.configured) return;
    const request = ++generation;
    connecting = true; error = ''; notice = '';
    try {
      // This command stays pending until the native browser callback completes.
      const result = await api.githubConnect();
      if (!alive || request !== generation) return;
      status = result; connecting = false;
      if (result.connected && picker) await loadRepositories();
    } catch (e) { if (alive && request === generation) error = errorMessage(e); }
    finally { if (alive && request === generation) connecting = false; }
  }
  async function cancel() {
    if (cancelling) return;
    ++generation; cancelling = true; error = '';
    try {
      await api.githubCancel();
      if (alive) { connecting = false; loadingRepositories = false; notice = 'Connection cancelled.'; }
    } catch (e) { if (alive) error = errorMessage(e); }
    finally { if (alive) cancelling = false; }
  }
  async function disconnect() {
    if (disabled) return;
    working = true; error = ''; notice = '';
    try {
      await api.githubDisconnect();
      if (!alive) return;
      ++generation;
      status = status && { ...status, connected: false, login: null, avatar_url: null };
      repositories = []; query = ''; repositoriesLoaded = false; loadingRepositories = false;
      notice = 'GitHub disconnected. Existing local workspaces are unchanged.';
    } catch (e) { if (alive) error = errorMessage(e); }
    finally { if (alive) working = false; }
  }
  async function manage() {
    if (disabled) return;
    working = true; error = '';
    try { await api.githubManage(); if (alive) notice = 'Repository access opened in your browser. Refresh repositories after making changes.'; }
    catch (e) { if (alive) error = errorMessage(e); }
    finally { if (alive) working = false; }
  }
  async function add(repo: GithubRepository) {
    if (disabled) return;
    working = true; error = ''; notice = 'Cloning repository…';
    try {
      const workspace = await api.addGithub(repo.clone_url);
      if (alive) { notice = `Added ${workspace.name}`; await onadded?.(workspace); }
    } catch (e) { if (alive) { error = errorMessage(e); notice = ''; } }
    finally { if (alive) working = false; }
  }
  async function setupGuide() {
    try { await openUrl('https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/registering-a-github-app'); }
    catch (e) { if (alive) error = errorMessage(e); }
  }
</script>

<section class="github-account" aria-label="GitHub account">
  <div class="account-heading">
    {#if avatar}<img src={avatar} alt="GitHub avatar" width="36" height="36" referrerpolicy="no-referrer" />{:else}<svg aria-hidden="true" width="28" height="28" viewBox="0 0 24 24" fill="currentColor"><path d="M12 .5a12 12 0 0 0-3.8 23.4c.6.1.8-.3.8-.6v-2.2c-3.3.7-4-1.4-4-1.4-.5-1.4-1.3-1.8-1.3-1.8-1.1-.7.1-.7.1-.7 1.2.1 1.9 1.3 1.9 1.3 1.1 1.9 2.8 1.4 3.4 1 .1-.8.4-1.4.8-1.7-2.7-.3-5.5-1.4-5.5-6a4.7 4.7 0 0 1 1.2-3.2c-.1-.3-.5-1.5.1-3.1 0 0 1-.3 3.3 1.2a11.5 11.5 0 0 1 6 0C17.3 5.2 18.3 5.5 18.3 5.5c.6 1.6.2 2.8.1 3.1a4.7 4.7 0 0 1 1.2 3.2c0 4.6-2.8 5.7-5.5 6 .4.4.8 1.1.8 2.2v3.3c0 .3.2.7.8.6A12 12 0 0 0 12 .5Z" /></svg>{/if}
    <div><strong>GitHub</strong><p class="muted">{loading ? 'Checking account…' : status?.connected ? `Connected${status.login ? ` as ${status.login}` : ''}` : 'Not connected'}</p></div>
  </div>
  {#if status && !status.configured}
    <p role="note">GitHub account sign-in is not configured in this build. Ask the app maintainer to configure the native GitHub App and browser callback, then restart mdwell. You can still add a repository by URL using your existing Git credentials.</p>
    <button onclick={setupGuide}>GitHub App setup documentation ↗</button>
  {/if}
  {#if connecting}
    <p role="status">Waiting for GitHub authorization in your browser…</p>
    <button onclick={cancel} disabled={cancelling}>{cancelling ? 'Cancelling…' : 'Cancel connection'}</button>
  {:else if status?.connected}
    <div class="account-actions"><button onclick={manage} disabled={disabled}>Manage repositories ↗</button><button onclick={disconnect} disabled={disabled}>Disconnect GitHub</button></div>
  {:else if status}
    <button class="primary" onclick={connect} disabled={disabled || !status.configured}>Connect GitHub</button>
  {/if}
  {#if !status && !loading}<button onclick={loadStatus}>Retry account status</button>{/if}
  {#if picker && status?.connected}
    <div class="repositories">
      <div class="account-actions"><strong>Repositories</strong><button onclick={loadRepositories} disabled={disabled || loadingRepositories}>Refresh repositories</button></div>
      <input aria-label="Search repositories" placeholder="Search repositories…" bind:value={query} />
      <select aria-label="Repository visibility" bind:value={visibility}><option value="all">Public and private</option><option value="public">Public</option><option value="private">Private</option></select>
      {#if loadingRepositories}<p role="status">Loading repositories…</p>
      {:else if repositoriesLoaded && !repositories.length}<p>No repositories available. Install the GitHub App for your account or organization using Manage repositories, select the repositories Mdwell may read, then refresh. Organization approval or an active SSO session may be required.</p>
      {:else if repositoriesLoaded && !filtered.length}<p>No matching repositories.</p>{/if}
      <ul>{#each filtered as repo (repo.id)}<li><button aria-label={`${repo.full_name} ${repo.private ? 'Private' : 'Public'}`} onclick={() => add(repo)} disabled={disabled}><span>{repo.full_name}</span><small>{repo.private ? 'Private' : 'Public'}</small></button></li>{/each}</ul>
    </div>
  {/if}
  {#if error}<p role="alert" class="error-text">{error}</p>{/if}
  {#if notice}<p role="status">{notice}</p>{/if}
</section>

<style>
  .github-account { padding: 18px 0; border-bottom: 1px solid var(--line); font-size: 13px; }
  .account-heading, .account-actions { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .account-heading img { border-radius: 50%; }
  p { line-height: 1.6; }
  .account-heading p { margin: 4px 0; }
  .repositories { margin-top: 18px; }
  .repositories input { width: 100%; margin: 10px 0; }
  ul { list-style: none; padding: 0; max-height: 230px; overflow: auto; }
  li button { width: 100%; display: flex; justify-content: space-between; gap: 12px; text-align: left; }
  li span { overflow-wrap: anywhere; }
  small { color: var(--muted); }
</style>
