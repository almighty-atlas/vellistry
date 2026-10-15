# Native GitHub clone/fetch: diagnosis and regression tests

## Reproduced causes

The GitHub API repository picker and Git's HTTPS transport are separate paths. A successful picker response proves account/API access, not that Git received a credential.

A local test with **real Git smart HTTPS**, rather than only `git credential fill`, reproduced two bugs:

1. Git's HTTPS transport sends repeated `capability[]` announcements, including `authtype` and `state`, to the credential helper. The strict helper request parser rejected these otherwise valid requests. It exited without returning credentials, so Git reported a missing password/authentication failure even though a native access token was available. Both the host helper and the authenticated IPC broker use this parser.
2. `git clone --no-tags` writes `remote.origin.tagopt=--no-tags` into the checkout. The repository-local configuration allowlist rejected this Git-generated setting before a later fetch could run.

The first failure was reproduced directly in both existing desktop executables: each passed the original 12 helper tests but failed the two capability-bearing requests. Those requests exited successfully with **zero IPC requests and zero credential output**. After rebuilding with the parser fix, both debug and release passed **14/14**, with one authenticated IPC request and the expected fake credential response for each valid request. Host mismatches still produced no credential output.

This establishes a credential-delivery defect independent of repository permissions. The fixture itself does not establish live permissions or token validity. Subsequently, the user confirmed successful authenticated clone and opening of `almighty-atlas/notes`; see [VALIDATION.md](VALIDATION.md#user-confirmed-manual-mvp-validation). The broader manual checklist below remains relevant.

## Targeted fixes and security boundaries

- Accept informational `capability[]` fields alongside `wwwauth[]`; do not negotiate capabilities or add new response fields.
- Continue requiring exact `protocol=https`, exact `host=github.com`, the expected username, valid bounded framing, and the private per-operation IPC capability. Duplicate routing fields, ports, other hosts, control characters, and unsupported request fields remain rejected.
- Allow only the exact data-only local setting `remote.origin.tagopt=--no-tags`. Other values and unsafe local executable configuration remain rejected.
- Keep the broker alive until Git exits. Join it before inspecting whether the credential response was delivered.
- Distinguish helper execution failure, protocol/IPC failure, and an authentication error after credential delivery. No tokens, headers, raw frames, raw stderr, or credential environment values are exposed in application errors.
- Reuse the securely stored Device Flow token, refreshing it before Git when necessary. Verify installation/repository grants with that same token before native clone/fetch. Explicit installation permissions must include Contents read (or write); metadata visibility alone is insufficient. Missing Contents permission gets a separate permission-update explanation.
- Picker imports explicitly require the native account. A disappeared connection must not silently invoke another helper identity. Manual advanced imports without a native account and SSH keep their existing alternative authentication behavior.

Tokens remain out of URLs, arguments, logs, persistent Git configuration, and plaintext files. No production commit/push behavior was added. The local HTTP fixture prepares and advances only disposable repositories inside its temporary directory.

## Safe diagnosis

The UI displays sanitized, actionable errors and a retry for the failed selected repository. Native access preflight distinguishes missing account, token/refresh or secure-store failure, inaccessible installation discovery, missing repository grant, and missing Contents permission.

Core transport codes include:

| Code | Interpretation |
| --- | --- |
| `git_credential_helper_start` | Git could not execute the helper host/runtime. |
| `git_credential_ipc` | Helper protocol/IPC failed, or a native authentication failure occurred without a recorded token delivery. |
| `git_authentication` | Authentication failure; for native HTTPS, credentials were delivered before this classification. This is not proof of missing repository permission. |
| `git_permission_denied` | Repository unavailable/forbidden/not found. GitHub can conceal private repositories with 404, so existence and permissions cannot always be separated. |
| `git_network` | DNS, connectivity, proxy, or TLS failure. |
| `git_ssh_host_verification` | SSH host-key verification failure. |

No diagnostic mode enables Git tracing or prints underlying credential data. Never request that a user paste their token, Authorization header, credential frames, or full Git environment.

## Reproducible offline tests

With cached Rust dependencies and Git, Python 3, OpenSSL, and the usual native build prerequisites:

```sh
cargo test --workspace --offline
python3 scripts/test-git-http.py
cargo build -p mdwell-desktop --offline
cargo build -p mdwell-desktop --release --offline
python3 scripts/test-git-helper.py target/debug/mdwell-desktop target/release/mdwell-desktop
```

`test-git-http.py` runs real Git through a **loopback-only CONNECT/TLS fixture** and `git http-backend`. It accepts only the `github.com:443` CONNECT target and never forwards traffic externally. Its temporary SAN certificate is explicitly trusted through isolated `http.sslCAInfo`; certificate verification remains enabled. A fixed fake token is held in memory. The test covers challenged authentication, real clone, closing/reopening the Core, fetching a new commit, updated reading/search, and actual HTTP 401/403 classification. It checks that credentials are absent from persisted application data. Temporary certificates and keys belong only to the mock TLS server, not to GitHub authentication.

`test-git-helper.py` calls the real desktop executables before GUI startup, both directly and through real `git credential fill`. It covers EOF/blank-line input, repeated challenge/capability fields, exact IPC framing, host mismatch, unreachable IPC, and no-op store/erase. It prints only fixed test labels, executable fingerprints, counts, exit codes, and marker presence—not captured credential output. It does not launch the development webview or installed AppImage.

Both scripts are test infrastructure, not an authentication broker or a runtime dependency. CI runs the smart HTTPS fixture and Linux debug/release helper checks. UI tests separately verify picker/API wiring, native-account-required imports, setup/return refresh, clone retries, and opening the imported document. Mock native tests verify token refresh and installation grants; the HTTPS fixture does not use a real keychain or Device Flow token.

## Onboarding and live end-to-end test

The standard picker separates account connection from repository access. An empty successful list offers **Repository-Zugriff einrichten** and explains installation versus selected-repository access without guessing which is missing. Returning from the management page triggers a one-shot refresh via native focus, browser focus, or visibility change. Manual refresh remains available. API errors are not shown as an empty installation state. Manual URLs and optional SSH/helper instructions are under **Erweiterte Optionen**.

For a real test, use the newly rebuilt application (restart an already running development process; old processes retain old code):

1. Launch the debug application or install/launch a newly built Linux package. Do not configure extra Git credentials for this test.
2. Keep the existing connected account if valid, or complete Device Flow once using the exact displayed code.
3. If needed, choose **Repository-Zugriff einrichten**, install the official Vellistry App, and select `almighty-atlas/notes`. Confirm Contents Read-only is approved. Do not grant write access.
4. Return to Vellistry. Verify automatic list refresh and the connected username independently of repository access. A repository deliberately not selected is a negative control.
5. Click the private repository. Verify clone/index progress, automatic opening, reading, and search without a second login or CLI.
6. Close and restart Vellistry. Verify cached content and account persistence, then explicitly sync after an upstream change. Check dirty local files remain protected.
7. Test offline reading/search, repository access removal, and an expired-token refresh. Do not edit or print production credentials to force expiry.
8. If any step fails, record the sanitized error/code, phase, OS/session, Git version, and dev/release package. Do not attach HTTP tracing, tokens, or credential data.

The live GitHub happy path is user-confirmed. Broader keychain lifecycle behavior, native browser-return focus, installed AppImage execution, macOS, and Windows remain manual release validation gates. See [VALIDATION.md](VALIDATION.md) for executed checks.
