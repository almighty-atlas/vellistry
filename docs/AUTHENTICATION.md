# GitHub authentication

Vellistry can clone and sync public or private GitHub repositories. The preferred integrated account connection uses a GitHub App, Device Flow in the system browser, direct GitHub endpoints, secretless token refresh, and native credential storage. See [GITHUB_INTEGRATION.md](GITHUB_INTEGRATION.md) for public identity, App settings, selected-repository installation, keyring/opener troubleshooting, and manual integration tests. No broker, tunnel, callback, or auth build configuration is needed. No client secret or App private key belongs in the desktop app.

This document describes the **alternative existing Git/SSH authentication paths**. When no native account is connected (for example after local disconnect), HTTPS uses your trusted credential helpers; SSH always uses your existing agent/configuration. Configure those tools before adding or syncing a workspace. Git itself never prompts interactively. A connected native account takes precedence for HTTPS; an OAuth failure does not silently switch identities to an external helper. Disconnect the account to use the external HTTPS helper again.

For the native HTTPS credential path, reproduced clone/fetch defects, sanitized diagnostics, onboarding, and offline real-Git regressions, see [GIT_DIAGNOSTICS.md](GIT_DIAGNOSTICS.md). Native picker imports require the connected account; they never silently fall back to an external helper if the account disappears.

## Accepted URLs

- `https://github.com/OWNER/REPO` (optional `.git` suffix)
- `git@github.com:OWNER/REPO.git`
- `ssh://git@github.com/OWNER/REPO.git`

A trailing slash is accepted. HTTPS is saved as `https://github.com/OWNER/REPO.git`; either SSH spelling is saved as `git@github.com:OWNER/REPO.git`. HTTPS and SSH remain distinct workspace identities. Sync uses the saved URL, not a possibly edited `origin`. There is no automatic transport fallback or credential transfer between transports.

Only the exact `github.com` host and, for SSH, the `git` user are accepted. Explicit ports (including default ports), passwords, HTTPS usernames/tokens, query strings, fragments, URL escapes, extra path segments, and other protocols/hosts are rejected. Never paste tokens into a repository URL. Rejected input is not echoed in errors.

## HTTPS: GitHub CLI

Install Git and [GitHub CLI](https://cli.github.com/), then run these yourself in a terminal, outside the checkout:

```sh
gh auth login --hostname github.com --git-protocol https
gh auth setup-git --hostname github.com
gh auth status --hostname github.com
```

Complete the interactive login in the terminal/browser. `setup-git` installs a trusted global credential-helper definition, which Vellistry reuses, including GitHub-specific helper settings and empty-helper resets. Use an account with access to the repository and authorize organization SSO when required. Launch Vellistry with `git` and `gh` available on its `PATH`.

## HTTPS: Git Credential Manager

Alternatively install [Git Credential Manager](https://github.com/git-ecosystem/git-credential-manager) and follow its platform-specific secure-storage instructions. Typical setup is:

```sh
git-credential-manager configure
git ls-remote https://github.com/OWNER/REPO.git
```

Run the second command yourself to finish any browser/device login and populate the credential manager before using Vellistry. On installations where GCM is exposed as a Git subcommand, use `git credential-manager configure`. Vellistry reuses the configured system/global helper and its validated `credential.credentialStore` selection; it does not install GCM, choose a credential store, or save a PAT. Avoid inline tokens in helper definitions, plaintext credential files, or shell command arguments; prefer your credential manager's OS-backed store.

On Linux, GCM may need an explicit store selection in your trusted global configuration, for example:

```sh
git config --global credential.credentialStore secretservice
```

Use the store you have already configured according to GCM's platform-specific instructions. Vellistry supports `secretservice`, `gpg`, `wincred`, `dpapi`, `keychain`, and the memory-only `cache` store (names are case-insensitive). It preserves your selection in the in-memory child configuration, including when a helper invokes `git config --get credential.credentialStore`. Vellistry rejects `plaintext`, empty, and unknown store selections with a sanitized `git` error rather than silently discarding the setting and falling back. Store selection must be unscoped system/global configuration, not repository-local or URL-scoped configuration. Unlock the selected store outside Vellistry when necessary. GCM environment overrides remain user-controlled external policy; Vellistry does not configure or select them.

## SSH: agent and verified host key

1. Create/register an SSH key with GitHub following [GitHub's SSH instructions](https://docs.github.com/en/authentication/connecting-to-github-with-ssh). Have an SSH agent running in your login session.
2. Load the private key interactively, for example:

   ```sh
   ssh-add ~/.ssh/id_ed25519
   ssh-add -l
   ```

3. Verify the connection **yourself**, interactively:

   ```sh
   ssh -T git@github.com
   ```

   Before accepting a new host key, compare its fingerprint against [GitHub's published fingerprints](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints). A successful GitHub greeting normally exits with status 1 because GitHub does not offer shell access. If a key has changed, investigate and verify it; do not blindly remove the old entry or accept a replacement.
4. Launch Vellistry in a session that can reach the agent (`SSH_AUTH_SOCK` on Unix). Add an SSH URL from the list above.

Vellistry invokes OpenSSH with `BatchMode=yes` and `StrictHostKeyChecking=yes`. It uses your trusted SSH configuration, known-hosts files, and agent, but cannot ask for a passphrase or enroll an unknown host. It never uses `accept-new`, disables host validation, or downloads keys automatically. Trusted SSH configuration can intentionally specify identities, a proxy/jump host, or a host mapping; review it as executable user policy. Git `core.sshCommand` settings are not reused.

## Configuration trust and isolation

Before each Git command, Vellistry reads an allowlisted snapshot of system and global Git configuration with `git config --system/--global --includes` from a temporary directory outside the checkout. This preserves Git's scope ordering, trusted includes, XDG global configuration, and explicit `GIT_CONFIG_SYSTEM`/`GIT_CONFIG_GLOBAL` overrides. `GIT_CONFIG_NOSYSTEM` is respected. Repository-dependent conditional includes are deliberately not evaluated in the checkout; put Vellistry's helper in unconditional global configuration or an unconditional trusted include.

Only these settings are reused:

- `credential.helper`, `credential.username`, `credential.useHttpPath`, and their URL-scoped forms; helper order and empty resets are preserved.
- Unscoped `credential.credentialStore`, validated against the supported GCM store names above.
- `http.proxy`, `proxyAuthMethod`, `noProxy`, `sslCAInfo`, `sslCAPath`, `sslBackend`, `schannelUseSSLCAInfo`, `sslVersion`, and their URL-scoped forms.

Snapshots are held in memory and passed through child-process environment configuration, not command-line arguments or temporary config files. Extra authorization headers, cookies, credential password/token settings, URL rewrites, SSH commands, and arbitrary global settings are not copied. HTTPS certificate verification remains enabled and redirects are disabled. Custom client-certificate authentication and helper-specific settings other than the validated GCM `credential.credentialStore` selection are not currently supported. System/global helpers (including shell helpers) are trusted executable user policy: review your configuration and its includes.

Git provides no general switch to disable repository-local configuration. Vellistry therefore inspects it with includes disabled and **refuses unsafe/custom local settings before executing Git operations**, rather than attempting to merge untrusted helpers into the trusted list. Supported local settings are standard clone metadata (`core.repositoryFormatVersion`, `fileMode`, `bare`, `logAllRefUpdates`, `ignoreCase`, `precomposeUnicode`), `remote.origin.url`/`fetch`, the exact Git-generated `remote.origin.tagopt=--no-tags` setting, branch remote/merge tracking, user name/email, and hooks/fsmonitor paths (which are always overridden). Other local settings, including scoped helpers, includes, HTTP settings, URL rewrites, filters, extensions, and SSH commands, cause a sanitized `git` error. Linked/worktree Git directories are unsupported. Remove custom settings from the Vellistry-managed checkout after reviewing them; put intended authentication configuration in trusted global/system config instead. Vellistry never edits your existing configuration.

Hooks and fsmonitor stay disabled, clone templates are empty, submodules are not recursed, LFS smudging is skipped, and only HTTPS/SSH transports are enabled. File transport is enabled only inside local tests. Git-specific injected environment config is cleared; SSH agent/session environment is retained. Authentication commands have a 120-second timeout; configuration reads have a 10-second timeout. Errors identify the operation without echoing arguments or raw stderr. A missing helper/key does not trigger another transport or an interactive prompt.

## Errors and frontend mapping

`Error::info()`/serialized errors add these stable `ErrorCode` values:

| Code | Meaning / next action |
| --- | --- |
| `git_authentication` | Authentication failed. For native HTTPS this classification follows recorded credential delivery; reconnect and retry. External helpers/SSH remain advanced alternatives. |
| `git_credential_helper_start` | The native credential helper executable/runtime could not start. |
| `git_credential_ipc` | Credential protocol/local IPC failed, or native authentication failed without recorded token delivery. |
| `git_permission_denied` | Repository unavailable, forbidden, or not found. Check spelling, account/repository permissions and SSO authorization. GitHub intentionally hides private repositories; this cannot reliably distinguish a missing repository from missing permission. |
| `git_network` | DNS, connection, proxy, or TLS failure. Check connectivity and trusted CA configuration. |
| `git_ssh_host_verification` | Unknown/changed SSH host key. Verify interactively against GitHub's fingerprints. |
| `git` | General Git failure, unsafe local config, start failure, or timeout. |

Classification is best-effort using Git's English diagnostics, not a server-side guarantee. Helper failures such as `unable to get password from user` and `could not read from user` are classified as `git_authentication` without returning their raw diagnostics. Host-verification, unavailable-repository, and authentication messages take precedence over generic connection errors. Raw stderr is never returned, including for unknown failures: helpers and Git may print secrets. The repository dialog and sync error banner use these codes to show setup guidance and user-clicked documentation links.

## Offline verification

Core tests use real Git, local repositories, dummy credential helpers, isolated temporary system/global config files (including an include, URL-scoped reset, and layered credential-store selections), and a mock SSH executable. A dummy helper checks the selected store using `git config --get credential.credentialStore` during real `git credential fill`. They do not contact GitHub, require real credentials, mutate the machine's system/global configuration, or log helper credential output.

```sh
cargo test -p mdwell-core
cargo clippy -p mdwell-core --all-targets -- -D warnings
```
