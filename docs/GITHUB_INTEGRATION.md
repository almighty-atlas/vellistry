# GitHub App integration and auth broker

Mdwell's optional GitHub account integration uses a GitHub **App**, browser authorization with state and S256 PKCE, a desktop loopback callback, and a small confidential token broker. The broker holds the App client secret; the desktop never does. Existing Git/SSH agents and HTTPS credential helpers remain alternatives: see [AUTHENTICATION.md](AUTHENTICATION.md) for that separate workflow. This integration does not install or replace those tools.

## 1. Register the GitHub App (external operator step)

In GitHub **Settings → Developer settings → GitHub Apps → New GitHub App**:

1. Choose a real name/homepage and record the resulting **App slug** and **Client ID** (not the numeric App ID). There are deliberately no dummy production IDs in this repository.
2. Choose one fixed desktop callback port in **1024–65535** (nonzero u16, greater than 1023) for this deployment. **43827 is an illustrative port choice only**, not an automatically selected/default port or a dummy client identifier. Review expected local port conflicts before choosing the real deployment value. Use the same decimal port in the desktop compile-time `MDWELL_GITHUB_CALLBACK_PORT`, broker server environment `GITHUB_CALLBACK_PORT`, and GitHub callback registration. The broker's own listen port (8787 by default) is separate and need not match.
3. Register exactly `http://127.0.0.1:PORT/callback`, replacing `PORT` with that chosen decimal value. For the illustrative choice only, this is `http://127.0.0.1:43827/callback`. Disable callback wildcard matching. Do not substitute `localhost`, HTTPS, the broker URL, or a portless callback. This exact fixed-port registration follows the [GitHub App callback policy](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/about-the-user-authorization-callback-url) and [user-token flow](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app). The required desktop flow binds **that exact `127.0.0.1` port before opening the browser**, supplies the identical redirect URI to authorization and exchange, and reports an explicit port-busy error instead of falling back to another port. The broker rejects any different callback port and does not rewrite or omit `redirect_uri`.
4. Disable **Request user authorization (OAuth) during installation**. Automatic installation authorization lacks the desktop-controlled PKCE challenge, state, and chosen callback port. Users install the App, then initiate authorization explicitly from Mdwell. Leave setup URL unset unless separately needed; it is not the authorization callback.
5. Repository permissions: **Contents: Read-only** and mandatory **Metadata: Read-only** only. No repository write, organization, or account permissions; no additional event subscriptions. Disable **Active** under Webhook; no webhook endpoint/secret is required. No App private key is needed: this broker exchanges user tokens, not JWTs or installation access tokens. Leave device flow disabled for this implementation.
6. Allow installation on **Any account** (public App) if distributing Mdwell to other users. On the public installation page, choose the intended personal/organization account and **Only select repositories**, not all repositories. Organization approval/SSO may be required. Authorization grants only the intersection of user access and App installation access.
7. Keep expiring user access tokens enabled. GitHub currently documents access tokens lasting eight hours and refresh tokens six months; rely on returned expiry values, not hardcoded lifetimes. Generate a client secret and provision it **only on the broker host**.

## 2. Broker contract and local run

Requires Python 3.11+ with stdlib only. No pip packages, database, token files, App key, or desktop secret. The only application environment variables are:

- `GITHUB_CLIENT_ID`: exact registered GitHub App Client ID.
- `GITHUB_CLIENT_SECRET`: registered App client secret.
- `GITHUB_CALLBACK_PORT`: **required**, canonical decimal port in 1024–65535, matching `MDWELL_GITHUB_CALLBACK_PORT` and the registered callback exactly. Missing, invalid, whitespace-padded, or leading-zero values prevent startup; there is no default.

Provision these using your process manager's protected environment/secret injection. Never put secrets in source control, command arguments, a desktop build, support tickets, or a shell history. For an interactive local run, use `getpass` prompts (nothing is sent to GitHub until a valid request arrives):

```sh
python3 -B -c 'import getpass, os, runpy; os.environ["GITHUB_CLIENT_ID"] = input("Real GitHub App Client ID: "); os.environ["GITHUB_CLIENT_SECRET"] = getpass.getpass("App client secret: "); os.environ["GITHUB_CALLBACK_PORT"] = input("Registered desktop callback port (1024-65535): "); runpy.run_path("scripts/github-auth-broker/broker.py", run_name="__main__")'
```

The listener is always `127.0.0.1:8787`; `--port` changes the nonzero port. Local HTTP is **only** for testing or a same-host TLS reverse proxy. Desktop production configuration requires HTTPS. There is no remotely accessible bind option or environment-selectable upstream.

`POST /token`, `Content-Type: application/json` (optionally `; charset=utf-8`), with exactly one of:

```text
authorization_code: grant_type, client_id, code, code_verifier, redirect_uri
refresh_token:     grant_type, client_id, refresh_token
```

All fields are strings. Exact client ID equality; no unknown or duplicate fields. Authorization verifier is RFC 7636 unreserved ASCII, 43–128 characters (desktop generates 43). Redirect must be exactly `http://127.0.0.1:<GITHUB_CALLBACK_PORT>/callback`, without credentials, query, fragment, or leading-zero port; other otherwise-valid ports are also rejected. Codes/refresh tokens are 1–2048 printable non-space ASCII characters; control characters are rejected. Body limit is 8192 bytes. Content-Length is required; duplicate lengths, transfer encoding, and content encoding are rejected. Connections are closed after a response.

The broker sends a fixed, certificate-verified HTTPS POST to `https://github.com/login/oauth/access_token`, URL-encoded form body, `Accept: application/json`. It injects the secret server-side. **Authorization-code exchange omits `grant_type`**, as required by GitHub's web flow; refresh sends `grant_type=refresh_token`. It never follows redirects, honors environment HTTP proxies, retries exchanges, or echoes upstream error bodies/details. Success is filtered to `access_token`, `token_type`, optional `refresh_token`, `expires_in`, `refresh_token_expires_in`; upstream OAuth rejection maps to a generic 401, other upstream problems to generic 502. All HTTP responses carry `Cache-Control: no-store`. There are no CORS headers and no browser preflight support. CORS is not an authentication mechanism.

Socket timeouts are 10 seconds; request-body and upstream-body reading also have deadlines. Upstream response limit is 16384 bytes. These are **not a universal end-to-end deadline**: OS DNS resolution and stdlib header parsing can exceed socket deadlines (e.g. slow-drip headers). The reverse proxy, process resource limits, and bounded workers are required in production. The desktop itself has a 15-second broker-request timeout; slow exchanges can finish after the desktop has stopped waiting.

## 3. Same-host TLS deployment (systemd + nginx)

These instructions target a Linux host with Python 3.11+, systemd, nginx, a real domain pointing at it, and an already provisioned trusted TLS certificate. Obtain/renew certificates using your normal ACME tooling; certificate issuance is an external deployment step, not something the broker performs. Substitute your **real domain and certificate paths** in the nginx template; do not ship the explanatory placeholders.

From the checkout, install only the runtime script:

```sh
sudo install -d -m 0755 /opt/mdwell-github-auth-broker
sudo install -m 0644 scripts/github-auth-broker/broker.py /opt/mdwell-github-auth-broker/broker.py
sudo install -m 0600 /dev/null /etc/mdwell-github-auth-broker.env
sudoedit /etc/mdwell-github-auth-broker.env
```

In that root-owned file, enter `GITHUB_CLIENT_ID=`, `GITHUB_CLIENT_SECRET=`, and `GITHUB_CALLBACK_PORT=` with the real values (systemd EnvironmentFile syntax, not `export`). The callback port must match the GitHub registration and desktop build; the App secret must never be included in the desktop environment. Use a secret manager instead if available. Keep the file root-readable only. Create `/etc/systemd/system/mdwell-github-auth-broker.service`:

```ini
[Unit]
Description=Mdwell GitHub App auth broker
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
DynamicUser=yes
EnvironmentFile=/etc/mdwell-github-auth-broker.env
ExecStart=/usr/bin/python3 -B /opt/mdwell-github-auth-broker/broker.py --port 8787
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
LimitCORE=0
TasksMax=32
MemoryMax=128M

[Install]
WantedBy=multi-user.target
```

This assumes `/usr/bin/python3` is your suitable interpreter. Disable swap or use encrypted swap on the host; memory is sensitive. Start and inspect without printing credentials:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now mdwell-github-auth-broker
sudo systemctl status --no-pager mdwell-github-auth-broker
```

Place this template in an nginx config included **inside `http {}`** (e.g. an enabled site). `YOUR_REAL_DOMAIN` and the certificate paths below are mandatory operator substitutions, not runnable production defaults:

```nginx
limit_req_zone $binary_remote_addr zone=mdwell_auth_ip:10m rate=12r/m;
limit_conn_zone $binary_remote_addr zone=mdwell_auth_connections:10m;
# Deliberately excludes URI/query, IP, headers, bodies, tokens and upstream details.
log_format mdwell_auth_safe '$status $request_time';

server {
    listen 443 ssl;
    server_name YOUR_REAL_DOMAIN;
    ssl_certificate /PATH/TO/REAL/fullchain.pem;
    ssl_certificate_key /PATH/TO/REAL/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    client_max_body_size 8k;
    client_header_timeout 10s;
    client_body_timeout 10s;
    send_timeout 10s;
    access_log /var/log/nginx/mdwell-auth-status.log mdwell_auth_safe;
    # Raw proxy error messages can include attacker-controlled request URIs.
    error_log /dev/null;
    add_header Cache-Control no-store always;

    location = /token {
        limit_req zone=mdwell_auth_ip burst=20 nodelay;
        limit_req_status 429;
        limit_conn mdwell_auth_connections 4;
        limit_conn_status 429;
        proxy_pass http://127.0.0.1:8787;
        proxy_http_version 1.1;
        proxy_request_buffering on;
        proxy_connect_timeout 2s;
        proxy_send_timeout 10s;
        proxy_read_timeout 12s;
        proxy_set_header Host localhost;
        proxy_set_header Connection close;
        proxy_set_header X-Forwarded-For "";
        proxy_set_header Forwarded "";
        proxy_set_header X-Real-IP "";
        proxy_hide_header Cache-Control;
    }
    location / { return 404; }
}
```

Do not add a plaintext redirect listener for token POSTs. Expose only TLS port 443 to clients, keep 8787 inaccessible outside the host, and allow broker outbound DNS and TCP 443 to GitHub. The proxy must share the host/network namespace for this loopback setup. Validate and reload:

```sh
sudo nginx -t
sudo systemctl reload nginx
```

Use an existing dedicated/default-deny virtual-host configuration so unknown Host/SNI values do not unexpectedly route to the broker. nginx body/header timeout settings are inactivity limits, not absolute request-duration limits; add edge slow-client/DDoS protection for a public service.

**IP trust and capacity:** the Python broker always limits the TCP peer, never `Forwarded`, `X-Forwarded-For`, or `X-Real-IP`. Its aggregate token bucket allows a burst of 200 and replenishes 120/minute per peer; behind this proxy all users share the loopback bucket. nginx separately limits each direct client IP to a burst of 20 and 12/minute, so one edge client cannot consume the entire aggregate budget at its allowed rate. Multiple attacking clients can still exhaust shared capacity: size and load-test these limits for the deployment, with reviewed tuning of `RateLimit` defaults and edge abuse protection. If nginx is itself behind a CDN/load balancer, configure nginx real-IP handling **only for explicitly allowlisted trusted proxy CIDRs**, block direct bypass, and ensure that upstream overwrites attacker-provided forwarding headers. Do not enable unconditional forwarded-IP trust. For multiple broker replicas, enforce shared edge limits; per-process buckets are not distributed limits.

Python caps concurrent accepted workers at 16, bounds its peer table to 4096 entries, and closes excess connections without spawning workers. Saturation can therefore produce a connection error instead of HTTP 429. Reverse-proxy limits are part of deployment, not optional decorations. Monitor sanitized status/latency counts, 401/429/502 trends, connection resets, process restarts, memory/CPU, DNS/TLS failures, and certificate expiry. Keep log retention limited. Never enable debug/request-body logging or APM token/header capture. Alerting/load testing and host hardening remain operator responsibilities. Secret rotation: provision the new secret, restart the broker, verify authorization/refresh, then retire the previous secret in GitHub; no desktop change is required if the Client ID is unchanged.

## 4. Desktop build configuration

Set these **compile-time** variables to real public configuration before building Rust/Tauri:

- `MDWELL_GITHUB_CLIENT_ID`: exactly the broker's GitHub App Client ID.
- `MDWELL_GITHUB_BROKER_URL`: HTTPS **base URL**, without credentials/query/fragment. The desktop appends `/token`; use `https://your-real-broker-domain`, **not** `https://your-real-broker-domain/token`. A base path such as `/github` needs an explicit proxy route mapping `/github/token` to broker `/token`; the nginx template above supports the root base only.
- `MDWELL_GITHUB_APP_SLUG`: real public App slug used for installation.
- `MDWELL_GITHUB_CALLBACK_PORT`: **required**, fixed decimal port in **1024–65535**; exactly the value in broker `GITHUB_CALLBACK_PORT` and the GitHub registration. There is no random-port fallback. Changing it requires coordinated registration, broker configuration/restart, and desktop rebuild/distribution; retain a matching registration/deployment for older supported builds or retire them explicitly.

Prompt for real values rather than building with examples:

```sh
read -r -p 'GitHub App Client ID: ' MDWELL_GITHUB_CLIENT_ID
read -r -p 'HTTPS broker base URL: ' MDWELL_GITHUB_BROKER_URL
read -r -p 'GitHub App slug: ' MDWELL_GITHUB_APP_SLUG
read -r -p 'Registered desktop callback port (1024-65535): ' MDWELL_GITHUB_CALLBACK_PORT
export MDWELL_GITHUB_CLIENT_ID MDWELL_GITHUB_BROKER_URL MDWELL_GITHUB_APP_SLUG MDWELL_GITHUB_CALLBACK_PORT
npm ci
npm run tauri -- build
```

The prompt commands require Bash. Client ID, broker URL, slug, and callback port are public, not authentication secrets. Do not set `GITHUB_CLIENT_SECRET` in desktop CI. Configure release CI with real public values and verify that a changed compile-time configuration triggers a fresh Rust build. Missing variables disable this integration with configuration guidance, rather than selecting a demo App. Release signing/distribution and installed-artifact validation are external steps.

## 5. Native credentials, offline use, and revocation

The desktop uses the OS-native keyring with service **`mdwell.github`** and account **the Client ID**. Linux uses synchronous, encrypted Secret Service D-Bus access with prompts disabled; macOS uses Keychain and Windows uses the native credential store. Linux requires an existing, unlocked default collection and fails with external-unlock guidance rather than creating a collection or waiting for an unlock prompt. D-Bus methods use two-second timeouts; connection setup has a three-second caller wait and at most one unfinished connection worker. No plaintext fallback is allowed. A locked/unavailable store is an actionable error; do not work around it with a token file.

On CachyOS/Arch with Hyprland, install a Secret Service provider such as `gnome-keyring` plus an unlock UI such as `seahorse` (`sudo pacman -S gnome-keyring seahorse`), or a KWallet version configured to expose **the Secret Service API**. KWallet being installed alone is not proof that this API works. The graphical user session needs a working D-Bus session bus and a reachable `org.freedesktop.secrets` service. Run Mdwell as the logged-in graphical user, not root or a system service. For diagnostics, run `busctl --user status org.freedesktop.secrets` in the same session; do not dump secret contents.

Prefer distro-supported PAM/session integration to start/unlock the login keyring. If not activated automatically, start `gnome-keyring-daemon --start --components=secrets` **inside the existing graphical D-Bus session**, once, and unlock/create the default login keyring interactively in Seahorse. Hyprland can use `exec-once = gnome-keyring-daemon --start --components=secrets` when PAM/service activation is absent. Do not start competing providers or a separate D-Bus session for Mdwell. Autologin often cannot unlock an encrypted keyring automatically; unlock it interactively. Login-keyring and login-password synchronization/PAM details depend on the actual display/login manager, not Hyprland alone. Verify read/write/delete in the built app and persistence after logout/reboot before release.

Account status, saved avatars, and previously synchronized workspace content can be shown offline (the repository picker itself requires an online API request); “connected” reflects locally stored authorization, not live GitHub validity. Refresh requires the network, broker, and unlocked keyring. An expired/revoked refresh token requires signing in again. Refresh tokens rotate: if the broker exchanges one but the reply is lost, or the desktop crashes/fails keyring storage before saving the replacement, the old token may no longer work. The broker deliberately cannot recover that rotation; reauthorization is the safe recovery. Do not automatically retry a possibly consumed refresh/code exchange.

**Disconnect is local:** it deletes this App's stored credentials, not GitHub authorization/installation or cached workspace content. For server-side revocation, use GitHub **Settings → Applications → Authorized GitHub Apps → revoke**, and optionally uninstall under **Installed GitHub Apps** / the organization installation settings. Remove local caches/workspaces separately if desired. Revocation also affects other desktop instances using that authorization. No webhook is needed for correctness: subsequent API/refresh failures surface rejection.

## 6. Security boundary

The broker is trusted with the App secret **and** transient user authorization codes, verifiers, access tokens, and refresh tokens. TLS protects transport but does not hide these from the broker operator, compromised host, memory dumps, or instrumentation. It is stateless: tokens are not persisted, logged, or cached, and GitHub calls receive only the allowlisted fields. Python cannot promise reliable in-memory secret zeroization; disable core dumps and unneeded tracing.

The fixed-port local callback must bind before browser launch and validate PKCE/state. Once the desktop owns the listener, a second process cannot win the pre-launch callback-listener race. A local process can occupy the configured port first and deny sign-in; the desktop must fail with a clear port-busy error without opening the browser, switching ports, or exposing credentials. Release testing must verify this ordering and the busy-port failure. PKCE/state protect the authorization flow against callback interception/forgery; these protections do not defend against a compromised desktop or other privileged local attacker.

The endpoint is public. A client ID is public and **does not authenticate the Mdwell binary**; there is no meaningful embedded desktop shared secret. PKCE/state protect the user authorization path, not broker capacity or all endpoint abuse. Rate/concurrency limits, bounded parsing, fixed upstream, trusted deployment, monitoring, and rotation are required. Do not mistake no-CORS for protection against non-browser attackers. A malicious party can exercise the App's token endpoint through this broker with their own valid grants; GitHub still enforces user/App installation permissions.

## 7. Test matrix and release gates

Run the offline checks from the repository root:

```sh
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
npm run check
npm test
python3 -B -m unittest discover -s scripts/github-auth-broker -v
npm run tauri -- build --no-bundle -- --offline
```

Rust dependencies must already be cached for `--offline`. The final command builds the Linux desktop release binary, not installers. Desktop unit tests exercise PKCE/state and loopback callbacks, denial/cancellation/timeouts, token-exchange and storage failures, native-store error classification, cancellation during credential writes, refresh rotation/expiry/serialization, bounded avatars, and paginated private repository discovery. Core fixtures exercise the real credential-helper protocol and clone/fetch orchestration against local mocks, preserve SSH/HTTPS-helper paths, and read synchronized content offline. Svelte tests mock IPC for account lifecycle and private repository filtering/selection. These are not live GitHub or real OS keychain integration tests.

The tests use only local loopback mock servers and patched HTTPS connections; no real GitHub request, DNS lookup, client secret, or native keyring is involved.

| Check | Coverage / required verification |
| --- | --- |
| Code grant / PKCE | Local mock verifies form encoding, secret injection, exact verifier/redirect, **no grant_type** upstream, filtered response. Desktop end-to-end state/PKCE remains a release check. |
| Refresh | Local mock verifies exact refresh form/whitelist. GitHub rotation/expiry and crash recovery require real-account validation. |
| Input / HTTP | Local tests cover wrong client, unknown/duplicate fields, malformed JSON/UTF-8, verifier bounds/alphabet, code controls/size, redirect host/scheme/port/path/query/fragment/credentials, body cap, media type, length ambiguity, unsupported method/path, no-store/no CORS. |
| Upstream safety | Local tests cover redacted OAuth/HTTP/invalid/oversized responses, no redirect follow, fixed host/timeouts, allowlisted output, silent logs. Real trusted TLS and DNS need deployment verification. |
| Resource safety | Local tests cover body timeout, simulated upstream timeout, spoofed forwarding headers, bucket refill/table cap and concurrency saturation. Public edge slow-client/load/DDoS testing remains external. |
| Callback registration / fixed port | Local broker tests verify exact configured-port matching, rejection of other ports, 1024/65535 bounds, and startup failure for missing/invalid configuration. Release checks: real GitHub authorization/exchange with the registered fixed port, desktop binds before browser, explicit busy-port error with no browser launch/random fallback, and state/PKCE rejection. |
| App installation | Real public personal + organization installs with selected repositories; verify private/public listing/content and denied unselected repositories, SSO/approval. |
| Native storage | Test installed builds on macOS, Windows and Linux/Hyprland: unlocked, locked, unavailable, logout/reboot, store failure, disconnect. No plaintext fallback. |
| Lifecycle | Real expiry/refresh, revoked/expired tokens, offline cached use, interrupted exchange, rotation-before-persist crash, reconnect, local disconnect versus GitHub revoke. |
| Release deployment | Real IDs, signed installed build, HTTPS chain/hostname, proxy routing, body/rate limits, no secret in bundle/logs, service restart/secret rotation, alerts/certificate renewal. |

Passing the local broker suite is not evidence that the real GitHub fixed-port authorization flow has run, that the installed desktop binds before browser launch or handles port conflicts correctly, that nginx/systemd have been deployed, that a keyring session is correctly configured, or that an installed release has passed the external matrix. Do not advertise those checks as completed until they have actually run.
