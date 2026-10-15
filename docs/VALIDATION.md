# Device Flow and native Git validation

Validation performed on Linux in the development workspace on 2026-10-15. No real GitHub tokens or accounts were used by automated tests.

| Check | Observed result |
| --- | --- |
| `cargo test --workspace --offline` | Passed, including 45 desktop tests (native GitHub authentication and import-boundary checks), core tests, integration fixtures, and doctest. |
| `cargo clippy --workspace --all-targets --offline -- -D warnings` | Passed. |
| `cargo fmt --all -- --check` | Passed. |
| `npm run check` | Passed: zero errors and warnings. |
| `npm test` | Passed: 110 tests across eight files. |
| `npm run build` | Passed. Vite reports the existing large editor chunk advisory (>500 kB); this is not a build failure. |
| `npm run tauri -- build -- --offline` | Passed, including production frontend, release Rust executable, DEB, RPM, and AppImage packaging. |
| `python3 scripts/test-git-http.py` | Passed: real authenticated smart HTTPS clone, Core reopen, fetch of a new commit, reading/search, and HTTP 401/403 classification through a loopback-only TLS fixture. |
| `python3 scripts/test-git-helper.py target/debug/mdwell-desktop target/release/mdwell-desktop` | Passed: 14/14 checks for each real desktop executable. Before the capability parser fix both passed only 12/14; see the diagnosis report. |
| `git diff --check` | Passed. |
| Obsolete auth infrastructure/config reference inspection | Removed Python broker, broker CI job, Docker/tunnel templates, callback listener and PKCE code, compile-time auth environment requirements, and unused direct Rust auth dependencies. Legacy credential-helper IPC is intentionally retained. |

The final Linux artifacts are under `target/release/bundle/{deb,rpm,appimage}/` with Vellistry product names. The Rust executable/package name and application identifier remain legacy compatibility identifiers. These packages were built, not installed, launched, signed, deployed, or uploaded.

Native local HTTP fixtures verify direct device/token form requests, actual user-code fields, interval enforcement, persistent slow-down, transient retries, cancellation/expiry/denial/invalid codes, and sanitized failures. Mock-store tests cover refresh without a client secret, serialized rotation, failed persistence/cleanup, offline status, avatar caching, and installation repository pagination. Frontend tests cover the IPC channel, code display/copy, explicit browser opening, stale events, cancellation cleanup errors, repository filtering and private selection. Core fixtures cover credential-helper IPC, real smart HTTPS clone/fetch, offline content, and helper/SSH policy; these are not live private GitHub clone tests. All listed checks were rerun successfully after the interrupted workstation session. See [GIT_DIAGNOSTICS.md](GIT_DIAGNOSTICS.md) for the reproduced capability-field and clone-generated-config defects, before/after evidence, safe diagnosis, test commands, and the focused live end-to-end checklist.

## User-confirmed manual MVP validation

Before this repository checkpoint, the user reported successful manual tests of:

- GitHub Device Flow sign-in.
- Display of private GitHub repositories.
- Authenticated clone and opening of `almighty-atlas/notes`.
- Improved search.
- Simplified GitHub onboarding.

These observations were supplied by the user, not repeated by the coding agent. Exact OS/session and installed-versus-development launch were not supplied, so they do not establish the complete platform/release matrix below.

For this checkpoint on 2026-10-15, formatting, workspace tests, Clippy, frontend checks (zero errors/warnings), all 110 frontend tests, the frontend build, the smart HTTPS fixture, debug/release desktop builds, and all 14 helper checks per executable were rerun successfully. The full Tauri packaging result in the table above is from the preceding development validation, not a new packaging run for this checkpoint.

## Remaining manual release gates

Follow the complete checklist in [GITHUB_INTEGRATION.md](GITHUB_INTEGRATION.md#manual-release-gates--not-yet-verified-by-this-document), for the full release matrix beyond the confirmed happy path, particularly:

1. Broader authorization scenarios, including denial, expiry, cancellation, and browser failure.
2. Installation with selected private/public repositories and an unselected negative-control repository; organization approval/SSO where applicable.
3. Live fetch/sync, dirty-checkout protection, and offline restart beyond the confirmed private clone/open/search path.
4. Actual native keychain save/load/delete and restart persistence on Linux/Hyprland, macOS, and Windows, including locked/unavailable stores and existing `mdwell.github` entries.
5. Real expiring-token refresh without a secret, persisted rotation, revoked authorization, and recovery.
6. Closing the application during authorization and installed-package browser/clipboard/accessibility behavior.
7. Installed-package upgrades, signing/notarization, and macOS/Windows builds.

No automated result establishes these live/platform gates. This report contains no credentials. The checkpoint operation only commits/pushes this Vellistry repository; no merge, deployment, or modification to another repository is part of it.
