# Changelog

## Unreleased — Vellistry MVP checkpoint (2026-10-15)

### Changed

- Replace broker-backed browser OAuth with native GitHub Device Flow and direct, secretless token refresh; retain encrypted native credential storage and legacy application/data identifiers.
- Remove the obsolete Python auth broker and its runtime/build configuration requirements.
- Simplify GitHub onboarding, selected-repository access setup, repository refresh, private selection, and clone retry feedback.
- Use Vellistry as the visible product name while preserving existing `mdwell` compatibility identifiers.

### Fixed

- Accept Git credential-helper capability announcements without weakening host/protocol checks.
- Allow the exact Git-generated `remote.origin.tagopt=--no-tags` setting so cloned repositories can be reopened and fetched.
- Check native repository grants and Contents permissions before authenticated Git operations; report sanitized transport/credential errors without switching identities silently.

### Validation

- Add real local smart HTTPS clone/fetch and desktop credential-helper regressions, including CI coverage.
- User-confirmed manual MVP: Device Flow, private repository listing, authenticated clone/open of `almighty-atlas/notes`, improved search, and simplified onboarding.
- The improved section search is included in this checkpoint; its implementation was already present in the base commit, not newly introduced by this delta.
- See [validation results](docs/VALIDATION.md) for automated checks and remaining manual platform/release gates.
