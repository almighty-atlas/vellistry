# Vellistry GitHub integration: Device Flow

Vellistry connects a GitHub **App** using GitHub's OAuth Device Flow. The native desktop talks directly to GitHub; the system browser is used only for the user's approval. No client secret, App private key, broker, Docker container, tunnel, homelab repository, callback listener, custom domain, or per-build auth environment configuration is required. Existing HTTPS credential helpers and SSH remain alternatives: [AUTHENTICATION.md](AUTHENTICATION.md).

The user has confirmed live Device Flow, private repository listing, authenticated clone/open of `almighty-atlas/notes`, improved search, and simplified onboarding. See [VALIDATION.md](VALIDATION.md) for the scope and attribution. This is not proof of the full installed-release/platform matrix; the checklist below remains the complete release gate.

## 1. Public identity and endpoints

| Purpose | Fixed public value |
| --- | --- |
| Product / GitHub App | Vellistry |
| Client ID (public, not a password) | `Iv23liS0HYbIuk9WY8B0` |
| App slug | `vellistry` |
| App page | <https://github.com/apps/vellistry> |
| Install / select repositories | <https://github.com/apps/vellistry/installations/new> |
| Request device authorization (native POST) | `https://github.com/login/device/code` |
| User verification (system browser) | <https://github.com/login/device> |
| Poll tokens and refresh (native POST) | `https://github.com/login/oauth/access_token` |
| Authenticated REST API | `https://api.github.com` |
| Manage authorizations / installations | <https://github.com/settings/apps> |

These values are compiled into the native host. Ordinary development and CI builds need no auth secrets or auth configuration. The former `MDWELL_GITHUB_*` broker/callback build settings are obsolete; remove them from local build environments rather than carrying forward deployment instructions. The Device Flow does not use a redirect URI, loopback OAuth port, PKCE verifier, or callback state. The separate short-lived native Git credential-helper loopback channel is **not** an OAuth callback and remains part of the existing Git architecture.

## 2. GitHub App owner settings (external prerequisite)

The App owner must review the actual GitHub settings; this repository does not change them:

1. Use the existing **Vellistry** App matching the client ID and slug above, not a new OAuth App or a dummy client. Enable **Device Flow** in its settings.
2. Keep **expiring user access tokens** enabled. GitHub documents an eight-hour access-token lifetime and six-month refresh-token lifetime; the desktop uses returned expiry values, not those examples as constants. Do not disable expiry to avoid implementing/testing refresh.
3. Repository permissions: **Contents: Read-only**, plus mandatory **Metadata: Read-only**. No write, organization, or additional account permissions. No event subscriptions are needed. Disable **Active** under Webhook; no webhook URL or webhook secret is required.
4. Allow installation on **Any account** for public distribution. Disable **Request user authorization (OAuth) during installation** so installation and the explicit desktop Device Flow remain separate. No setup URL is needed for this workflow. Callback fields, if present in GitHub's registration UI, do not participate in Device Flow and must not be treated as a deployment requirement.
5. Do not generate or distribute a client secret or App private key for this desktop integration. Device authorization, token polling, and refresh use the public client ID without `client_secret`. No operator-hosted token endpoint is involved.

Reference: GitHub's [generating a user access token (Device Flow)](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app) and [refreshing user access tokens](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/refreshing-user-access-tokens).

## 3. Install, authorize, and choose repositories

Installation and user authorization are different grants; either one alone is insufficient for private repository access.

1. Open <https://github.com/apps/vellistry/installations/new>, select the intended personal or organization account, and choose **Only select repositories**. Select the minimum repositories you want to read, not all repositories.
2. Organization owners may need to approve the installation, and organization policy/SSO may impose additional requirements. A pending approval is not successful repository access.
3. In the desktop account settings, start GitHub sign-in. Vellistry requests a device authorization and displays the **user code**, verification URL, and expiry information. The private `device_code` stays native; it is never sent to the WebView.
4. Open the displayed verification page in your system browser, or manually navigate to **https://github.com/login/device** if the opener fails. Enter only the code from the sign-in you just started in your trusted local Vellistry instance. Confirm the GitHub account and **Vellistry** App before approving.
5. Leave the desktop sign-in pending while approving. Vellistry polls GitHub, reads the profile, and saves credentials in the native keychain. Closing the browser alone does not cancel polling; use the desktop cancellation action. After expiry/denial, start a fresh sign-in and use the new code.
6. Verify the connected login, then open the repository picker. It enumerates `/user/installations` and `/user/installations/{id}/repositories`, paginates both, and deduplicates by repository ID. Its list is the intersection of the user's access and the App's installation permissions, not every repository on the account.
7. Select a repository to clone over HTTPS. Sync is an explicit fetch/fast-forward operation, not a background push, commit, or cloud-sync service. Unselected repositories must remain unavailable even if the user can access them outside Vellistry.

To add/remove repository access later, manage the relevant installation in GitHub settings (personal or organization), then reload the repository list. An empty list can mean no installation, no selected repositories, insufficient user access, pending approval, or organization policy; repeated sign-in does not grant missing installation permissions.

## 4. Native protocol, polling, and token lifecycle

All authorization forms go directly to the fixed GitHub HTTPS endpoints:

- Device request: `client_id`.
- Token poll: `client_id`, private `device_code`, and `grant_type=urn:ietf:params:oauth:grant-type:device_code`.
- Refresh: `client_id`, `refresh_token`, and `grant_type=refresh_token`. **No client secret**, including for refresh.

The native host validates the response and accepts only the exact verification URI `https://github.com/login/device`. It waits the server's interval before the first and subsequent polls, continues on `authorization_pending`, increases the interval by at least five seconds on `slow_down`, and backs off transient transport/server failures within the device deadline. Denied, expired, invalid, or malformed grants stop with sanitized errors. Cancellation stops pending work and prevents a late success from reconnecting a disconnected account. A request already in flight may finish; cancellation is not server-side revocation.

Before an authenticated operation, access tokens expiring within 60 seconds are refreshed under a credential lock. Refresh tokens rotate: save the replacement access token, refresh token, and expiry metadata in the native store before continuing. An expired/revoked refresh token requires a fresh Device Flow. An ambiguous refresh network failure or failed storage after rotation may mean GitHub consumed the old refresh token; the implementation deletes stale credentials or blocks their reuse until cleanup succeeds. Do not retry a possibly consumed refresh token or restore an old token backup as recovery. Unlock the keychain, disconnect if required, and sign in again.

Account status, cached avatars, and synchronized files can be shown offline. **Connected** means locally saved authorization, not a live GitHub validity check. Listing repositories, cloning, syncing, and refreshing need network access. Local reading/searching must not depend on GitHub or refresh succeeding. Optional avatar failures do not block sign-in; only bounded, validated raster avatars from the approved GitHub avatar host are cached and displayed as local data URLs.

## 5. Native storage and persistent compatibility

- macOS: Keychain. Windows: native credential store. Linux: encrypted Secret Service D-Bus access to an **existing, unlocked default collection**.
- The persistent service remains **`mdwell.github`**, with account/username **`Iv23liS0HYbIuk9WY8B0`** (the client ID). Linux matches the existing keyring-v3 attributes `service=mdwell.github`, `username=<client ID>`, `target=default`. Preserve these identities so existing credentials remain discoverable after the visible rename.
- Vellistry is the visible product name; legacy `mdwell` Cargo/package/executable names, `org.mdwell.desktop` application identity, app-data paths, SQLite/workspace caches, and frontend preference keys are compatibility identifiers, not auth configuration. Do not rename/delete them as part of login troubleshooting. See [architecture](ARCHITECTURE.md).
- Tokens and refresh metadata stay in the native credential store, not SQLite, plaintext JSON files, browser storage, repository URLs, command arguments, or ordinary child-process environment variables. A serialized credential payload inside the encrypted keychain is not a plaintext-file fallback.
- Linux never creates/unlocks a collection, runs a Secret Service prompt, or falls back to a session-only collection. Locked/unavailable storage fails with guidance to unlock externally. D-Bus method calls have two-second timeouts; connection setup bounds the caller's wait to three seconds with at most one unfinished connection worker.

**Disconnect is local:** it deletes this client's saved credentials, not the GitHub authorization, App installation, or cached repository content. For server-side revocation use GitHub **Settings → Applications → Authorized GitHub Apps → Vellistry → Revoke**; optionally uninstall under **Installed GitHub Apps** / organization installation settings. Revocation can affect other desktop instances sharing the authorization. Remove cached files separately if needed; revocation cannot erase already cloned content. No webhook is needed for correctness: subsequent online operations surface authorization failure. A connected native account takes precedence over external HTTPS helpers; auth errors do not silently switch identities. Disconnect to intentionally use the existing helper path again. SSH remains independent.

## 6. Phishing and trust boundaries

A public client ID identifies the App, **not a genuine Vellistry binary**. There is no embedded desktop shared secret. An attacker can initiate Device Flow for the same public client and ask a victim to approve the attacker's code on the real GitHub site. Seeing `github.com` and the correct App name alone does not prove that the code came from your desktop.

- Start sign-in yourself in a trusted local build/installed app. Approve only its freshly displayed code. Never enter a code received through chat, email, support, an issue, a QR advertisement, or someone else's screen, even at the legitimate GitHub verification page.
- Check the exact HTTPS hostname `github.com`, the account, App identity, and requested permissions. If unexpected, deny/cancel and investigate. Never send user codes, device codes, tokens, keychain dumps, or approval screenshots to support.
- Device Flow is not callback PKCE/state; it does not provide those interception protections or binary attestation. Short expiry, careful code provenance, minimal installation access, and user review matter. A compromised browser, desktop, keychain, privileged local process, or malicious app build remains outside the protection offered by this flow.
- Only safe display fields cross IPC. Auth responses/errors are bounded and sanitized; raw token bodies and Git/helper stderr must not be logged. Do not enable HTTP body tracing, process dumps, or debug telemetry containing credentials. TLS verification stays enabled and authorization redirects are not followed.
- GitHub receives authorization/API requests directly; no Vellistry-operated broker or Cloudflare tunnel processes tokens. This removes that intermediary, not GitHub's or the local machine's trust boundary. External Git helpers/SSH configuration remain executable user policy as described in [AUTHENTICATION.md](AUTHENTICATION.md).

## 7. Linux / Hyprland troubleshooting

Run Vellistry as the logged-in graphical user, not root, a system service, or inside a new `dbus-run-session` disconnected from the desktop. Keyring access and browser opening are separate dependencies.

### Keyring unavailable, locked, or requires a prompt

On Arch/CachyOS, one option is:

```sh
sudo pacman -S --needed gnome-keyring seahorse
busctl --user status org.freedesktop.secrets
```

Use a single Secret Service provider. A KWallet installation works only if its version/configuration actually exposes `org.freedesktop.secrets`; having KWallet installed is not proof. Inspect service status, not secret contents.

Prefer distro-supported PAM/session integration to start and unlock the login keyring. If activation is absent, start once **inside the existing graphical D-Bus session**:

```sh
gnome-keyring-daemon --start --components=secrets
```

Hyprland can use `exec-once = gnome-keyring-daemon --start --components=secrets` when PAM/service activation is not already doing this. Do not start competing daemons. Use Seahorse (or the provider's supported UI) to create/select a default persistent login collection and unlock it **outside Vellistry**, then retry. Autologin often cannot unlock an encrypted keyring; password/PAM synchronization depends on the actual login manager. Never remove keyring encryption, dump credentials, or create a plaintext token file to bypass this error. Verify persistence after logout and reboot.

### Browser does not open

The browser may not share the desktop's environment when launched from a terminal, launcher, or user service. From the same graphical session, test a harmless URL:

```sh
xdg-settings get default-web-browser
xdg-open https://github.com/login/device
systemctl --user status xdg-desktop-portal.service xdg-desktop-portal-hyprland.service
```

Ensure a default browser and `xdg-utils` are installed. On Hyprland, use the distro-supported `xdg-desktop-portal` / `xdg-desktop-portal-hyprland` setup and appropriate desktop environment propagation for the user service manager. Do not blindly restart session services or introduce a second D-Bus session. An absent portal service is diagnostic context, not proof of the cause: `xdg-open` can also use a browser directly. If manual opening works, navigate to the exact verification URL and enter your own current desktop code; no callback handler or local port needs repair. Cancel/restart if the code expires while troubleshooting.

For network errors, check DNS, proxy, system time, and trusted CA configuration for GitHub. Do not disable TLS verification. For a successful login with no repositories, review installation selection, organization approval/SSO, and account identity before changing the keyring.

## 8. Automated checks and full manual integration checklist

See [the migration validation report](VALIDATION.md) for commands actually run, observed results, generated Linux packages, and outstanding manual gates. [Native Git diagnosis](GIT_DIAGNOSTICS.md) explains the reproduced credential capability/configuration bugs, real smart HTTPS regressions, safe error classification, and simplified repository onboarding.

From the repository root, with Rust/npm dependencies and native build prerequisites already available:

```sh
cargo fmt --all --check
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
npm run check
npm test
npm run build
cargo build -p mdwell-desktop --offline
cargo build -p mdwell-desktop --release --offline
python3 scripts/test-git-http.py
python3 scripts/test-git-helper.py target/debug/mdwell-desktop target/release/mdwell-desktop
```

`--offline` requires cached Rust dependencies. These commands need no client secret, broker, Docker, tunnel, or real GitHub authorization. Native tests use mock stores and local HTTP fixtures for Device Flow forms, response validation, pending/slow-down/transient retries, deadlines, cancellation races, refresh rotation/storage failures, avatar validation, and paginated repository discovery. Core fixtures cover the real Git credential-helper protocol and Git orchestration; frontend tests mock IPC. They are not evidence of live GitHub settings, real OS keychains, browser integration, signing, or installer behavior. There is no remaining Python broker test job.

### Manual release gates — not yet verified by this document

The user-confirmed happy path is recorded in [VALIDATION.md](VALIDATION.md#user-confirmed-manual-mvp-validation). Unchecked items below denote the complete release scenarios, not a claim that no manual MVP testing has occurred.

Use a dedicated test account and disposable selected repositories; never attach credentials to the results. Record OS/session, build/commit, installed versus development launch, steps, observed outcome, and pass/fail/not-run. Repeat relevant gates on Linux (including Hyprland), macOS, and Windows with a real installed build.

- [ ] **App configuration:** confirm public identity/client ID, enabled Device Flow, expiring tokens, read-only Contents/Metadata, inactive webhook, public installation policy, and no auth secret/config injected into build or runtime.
- [ ] **Clean install/build:** launch without former broker environment variables or services. Verify the app does not contact a former custom auth domain, bind an OAuth callback port, or depend on Docker/tunnels. Inspect distribution/config/logs for secrets without printing token data.
- [ ] **Personal installation:** install on one account with **Only select repositories**, including a disposable private and public repository. Keep a second private repository unselected as a negative control.
- [ ] **Organization installation:** exercise owner approval, pending/denied approval, selected repositories, SSO/policy restrictions, and a user lacking repository access. Confirm the intersection of user and installation access.
- [ ] **Happy-path Device Flow:** initiate locally, verify code/URL/expiry are visible, open the system browser, confirm the correct GitHub account and Vellistry App, approve, and observe a saved connected profile. No token/device code should appear in the UI, logs, URLs, or clipboard operations intended for the user code.
- [ ] **Browser failure/manual route:** simulate unavailable opener/default browser; manually use the exact verification page with the current locally initiated code, and complete sign-in without a callback. Test launcher and terminal starts on Hyprland.
- [ ] **Denial and expiry:** deny in GitHub; separately let a code expire. Observe actionable terminal state, no newly saved credentials, and successful fresh-code retry. Closing only the browser must not imply cancellation.
- [ ] **Cancellation/races:** cancel during pending polling, retry promptly, and disconnect while approval/credential saving is in flight. A late response must not resurrect the account or overwrite newer credentials; double initiation must not create concurrent sign-ins.
- [ ] **Polling/network:** exercise a temporary outage and recovery within the device deadline; verify bounded backoff, no rapid polling, and stop at expiry/cancellation. Confirm `slow_down` timing in controlled fixtures if it cannot safely be induced live; do not load-test GitHub.
- [ ] **Picker:** verify selected private/public repositories, multiple installations/pages, stable deduplication, search/selection, empty installation/list, and an unavailable repository. Add/remove installation selections and reload. Unselected repositories must not become accessible by manually entering their URL.
- [ ] **Clone/read/sync:** import the selected private repository over HTTPS, read/search it, make an upstream commit, and explicitly fast-forward sync. Confirm dirty local work is protected and no push/write capability is used. Repository URLs/config must contain no tokens.
- [ ] **Native persistence:** on each OS, sign in, close/reopen, log out/in, and reboot; login/cache should persist using the existing `mdwell.github` identity. Confirm existing app-data/workspaces/preferences remain discoverable under legacy compatibility IDs.
- [ ] **Locked/unavailable storage:** lock the keyring, stop/unavailable-provider test safely, and exercise read/save/delete failures. Linux must fail promptly with external-unlock guidance, not prompt/create a collection or fall back to plaintext/session-only storage. Unlock externally and recover; failed disconnect must not be reported as successful deletion.
- [ ] **Real expiry/refresh:** allow access-token expiry (or use an isolated controlled test harness without editing production credentials), perform an authenticated operation, and verify direct secretless refresh plus persisted rotation. Restart and verify the replacement credentials work. Do not disable expiring tokens to pass this gate.
- [ ] **Refresh failure:** test expired/revoked refresh authorization and interrupted refresh/failed persistence in controlled fixtures. No reuse of a possibly consumed token, silent helper identity fallback, or infinite retry. Cleanup/disconnect then fresh Device Flow must recover.
- [ ] **Offline:** after successful sync, disconnect the network and restart; cached login/avatar/files/search remain usable. Picker/sync/refresh fail clearly without deleting local content or falsely claiming online validity.
- [ ] **Disconnect/revoke/uninstall:** local disconnect removes credentials but preserves cloned content and GitHub authorization/installation. Reconnect, revoke on GitHub, and confirm the next online operation fails safely; uninstall or remove repository selection and verify access is lost. Account for other desktop instances and still-cached files.
- [ ] **Alternative auth regression:** with native account disconnected, verify existing HTTPS helper; verify SSH agent/strict host-key behavior independently. With native account connected but rejected, ensure HTTPS does not silently switch to the helper account.
- [ ] **Phishing review:** unexpected or externally supplied codes are rejected by the tester; exact domain/App/account and local code provenance are explained. Testers understand that correct client ID/App name is not binary attestation.
- [ ] **Installed-release UX/security:** verify focus, accessible code/expiry/errors/cancel controls, browser launch, keychain permissions, upgrade/cache compatibility, and absence of token leakage on all target platforms. Signing/notarization/bundle/installer validation remains separate from an unsigned CI build.

Only mark a gate passed after observing it. A local test suite or successful build does not prove a real GitHub Device Flow, secretless refresh, native keyring, or installed desktop workflow has run.
