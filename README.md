# Vellistry

Ein lokaler Markdown-Leseplatz für eigene Ordner und öffentliche/private GitHub-Repositories: Rust-Core, SQLite FTS5 und Svelte-Oberfläche mit Tauri-2-Integration.

**Status: initiales Entwicklungs-MVP, nicht produktionsgehärtet.** Core, Frontend, asynchroner Tauri-Desktop-Host, Root-Cargo-Workspace und npm-Tauri-CLI sind integriert. Linux-Debug-/Release-Builds und automatisierte Tests sind bestanden; Linux-Pakete wurden gebaut. Device Flow, private Repository-Anzeige, authentifiziertes Klonen/Öffnen von `almighty-atlas/notes`, verbesserte Suche und vereinfachtes Onboarding wurden vom Nutzer manuell erfolgreich getestet. Installierte Pakete, vollständige Release-Abläufe und Cross-OS-Parität sind damit nicht verifiziert. Details: [Prüfergebnisse](docs/VALIDATION.md).

## Funktionen und Grenzen

- Lokale `.md`-/`.markdown`-Dateien lesen, durchsuchen und im CodeMirror-Quellmodus bearbeiten.
- Öffentliche/private GitHub-Repositories über HTTPS oder SSH importieren und ausdrücklich per Fast-Forward synchronisieren; kein Commit/Push. Die integrierte [Vellistry-GitHub-App-Anmeldung](docs/GITHUB_INTEGRATION.md) nutzt Device Flow im Systembrowser, direkte GitHub-Endpunkte und native System-Keychains; inklusive Kontoanzeige und Auswahl installierter, freigegebener Repositories. Öffentlicher Client `Iv23liS0HYbIuk9WY8B0`, App-Slug `vellistry`: kein Client-Secret, Broker, Docker/Tunnel oder Auth-Build-Konfiguration nötig. Die App muss Device Flow und ablaufende Tokens aktiviert haben; Installation mit **Only select repositories** und ausschließlich lesenden Rechten. Die echte Anmeldung wurde manuell bestätigt; realer Token-Refresh und die vollständige Plattformmatrix bleiben zu prüfen. Alternativ bleiben bestehende [HTTPS-Credential-Helper und SSH](docs/AUTHENTICATION.md) unterstützt. Keine Klartext-Tokenablage oder Tokens in URLs; benutzerdefinierte lokale Git-Konfiguration ist absichtlich nicht unterstützt.
- GFM inklusive Tabellen und Admonitions, TOC, Code-Highlighting/Copy, History, abschnittsbasierte Suche über alle oder einen ausgewählten Workspace mit Überschriftenhierarchie, exaktem Dateinamen-Rang, Phrasen, implizitem letztem Präfix und begrenzter Mehrwort-Tippfehlerkorrektur. Geschlossenes Frontmatter wird ausgeblendet, bleibt aber samt Originaloffsets in der Quelle erhalten.
- Speichern mit SHA-256-Revision und Konfliktanzeige; Entwürfe werden nicht still durch Polling ersetzt. Externe Writer können trotzdem den letzten Check/Rename racen; Entwürfe sind nicht crashfest gespeichert.
- Suchtreffer zeigen Abschnittskontext und markierte Begriffe; Reader-Markierungen bleiben auf passende Blöcke begrenzt. Core-Offsets bezeichnen **Markdown-Blöcke**, keine exakten Teilzeichenfolgen; Source springt weiter zum Blockanfang.
- Lokale Rasterbilder werden sicher eingebettet: inerte Core-Bildreferenzen werden über workspacegebundene Assets zu Blob-URLs, die beim Cleanup widerrufen werden. Asset-Links öffnen einen Dialog; kein Remote-Bildabruf, kein SVG und kein Mermaid-Rendering. AVIF/BMP-Allowlisten sind noch nicht Ende-zu-Ende konsistent.
- Vier-Sekunden-Polling ruft lokal Refresh für jeden Workspace vor dessen Dokumentliste auf und aktualisiert damit auch den Suchindex nach externen Änderungen. Nur Änderungen werden neu indexiert; unveränderte Dateien werden trotzdem gelesen/gehasht. Das aktuelle saubere Dokument wird nur bei geänderter indexierter Revision neu gelesen; unveränderte Listen/Dateibäume bleiben gecacht, dirty Entwürfe erhalten. Sync bleibt eine separate, ausdrückliche Netzwerkaktion.
- Feste Ausschlüsse `.git`, `node_modules`, `target`; keine konfigurierbaren Regeln/`.gitignore`-Auswertung und noch keine Workspace-Entfernung.

Der sichtbare Produktname ist **Vellistry**. Bestehende `mdwell`-Paket-/Executable-Namen, `org.mdwell.desktop`, App-Datenpfade, Preference-Keys und der Keychain-Service `mdwell.github` bleiben aus Kompatibilitätsgründen erhalten; die sichtbare Umbenennung migriert oder löscht keine Nutzerdaten.

## Voraussetzungen

- **Node.js 22.12+** für die hier verwendete Vite-8-Toolchain (CI verwendet Node 22); npm.
- **Rust 1.89+**, Cargo und rustfmt; aktuelle stabile Toolchain empfohlen.
- Git im `PATH` für Repository-Import, Sync und lokale Git-Testfixtures.
- OS-Abhängigkeiten für Tauri 2:

### Arch Linux

```sh
sudo pacman -S --needed gtk3 webkit2gtk-4.1 libsoup3 openssl base-devel librsvg git
```

Node/npm und Rust separat installieren. WebKitGTK 4.1 verwenden, nicht das alte 4.0-Paket.

### Ubuntu / Debian

Ubuntu 24.04 ist die Linux-CI-Baseline:

```sh
sudo apt-get update
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev librsvg2-dev libayatana-appindicator3-dev git
```

Auf anderen Distributionen können Paketnamen bzw. Verfügbarkeit abweichen.

### macOS

Xcode Command Line Tools (`xcode-select --install`), Rust, Node/npm und Git installieren. Tauri nutzt das System-WKWebView. Signierung/Notarisierung für Distribution ist ein späterer Release-Schritt, keine Voraussetzung für die normale Entwicklungs-CI.

### Windows

Microsoft C++ Build Tools mit „Desktop development with C++“ und Windows SDK, Microsoft Edge WebView2 Runtime, Rust mit MSVC-Toolchain, Node/npm sowie Git installieren. Native Pfadkomponenten werden inzwischen portabel zu slash-getrennten relativen API-Pfaden normalisiert; Git verwendet portable Konfigurationsisolation mit einer Allowlist vertrauenswürdiger System-/Global-Einstellungen und einem leeren Hooks-Verzeichnis statt Unix-Spezialpfaden. Windows-Laufzeitverhalten ist trotzdem noch nicht getestet; ein erfolgreicher Linux-Build ist kein Nachweis vollständiger Windows-Unterstützung.

## Starten und bauen

Vom Repository-Root:

```sh
npm install
npm run tauri dev
```

Desktop-Build:

```sh
npm run tauri build
```

Das baut den Desktop-Host und die für die lokale Plattform konfigurierten Bundles; es ist kein signierter Release-Prozess. Der Linux-Binary-Build mit Cargo ist bestanden; Release-Builds, Tauri-Bundles und Installer sind noch nicht getestet. Nur das Frontend starten:

```sh
npm run dev
```

Die Browseransicht hat **keinen nativen Backendzugriff**; Ordnerdialog, Persistenz und IPC-Funktionen benötigen den Tauri-Host. `npm run build` baut nur die Web-Assets.

## Prüfen

Der Root-Workspace hat `default-members = ["crates/core"]`; `cargo test` ohne Paketauswahl prüft daher standardmäßig den Core:

```sh
cargo fmt --all --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p mdwell-desktop
npm run check
npm test
npm run build
npm audit
```

Desktop-Check ohne Installer-Bundling:

```sh
cargo check --manifest-path src-tauri/Cargo.toml
```

Vorhandene Tests decken Core-Persistenz, Suche, Sanitization, Pfad-/Größenlimits, Konflikte und kooperierende Saves ab; Git-Sync wird mit lokalen Fixtures geprüft. Symlink-/Permissions-Prüfungen sind teilweise Unix-spezifisch. Die 71 vorhandenen Frontendtests mocken IPC/OS-APIs und prüfen u. a. Navigation, Suche, Preferences, Konfliktentwürfe, lokalen Polling-Refresh sowie Bildauflösung, Blob-Cleanup und veraltete Antworten. Das sind keine nativen Desktop-Ende-zu-Ende-Tests.

Im aktuellen Linux-Prüfstand bestanden `cargo fmt --all --check`, `cargo test --workspace --all-targets`, Workspace-Clippy mit `--all-targets -- -D warnings`, `cargo build -p mdwell-desktop`, `npm run check`, alle 71 Frontendtests und `npm run build`; `npm audit` meldete 0 Schwachstellen. Separat erneut geprüft: 32 Core-Tests plus ein Doc-Test. Auth-Tests nutzen lokale Git-Fixtures, Dummy-Helper und Mock-SSH, keine echte authentifizierte GitHub-Verbindung. Der native Desktop-Prozess blieb bei einem begrenzten Fünf-Sekunden-Starttest ohne Fehlermeldung aktiv; anschließend wurde er beendet. Das ist kein interaktiver End-to-End-Test. Auch der Frontend-Build ist bestanden, mit Warnung zum großen dynamischen Editor-Chunk; eine allgemeine Startup-/RAM-Aussage folgt daraus nicht. Ein erster Xvfb-Harness-Lauf hatte ein Timeout-/Cleanup-Problem, keinen nachgewiesenen App-Fehler. Release-Builds, Bundles und Installer sind nicht getestet.

CI: Core-Tests und Rust-Formatting, Frontend-Check/Tests/Build sowie Desktop-`cargo check`/`cargo build` auf GitHub-Linux-/macOS-/Windows-Runnern. Keine Signing- oder Auth-Secrets erforderlich; keine automatische Veröffentlichung signierter Installer.

## Suche und Reader-Performance

Aktuelle Suchqualität, Migration, Query-Grenzen und reproduzierbare Messungen: [Suche](docs/SEARCH.md).

```sh
cargo run -p mdwell-core --release --example search_quality
```

**Historischer MVP-Benchmark, durch die Messungen in [SEARCH.md](docs/SEARCH.md) abgelöst:** Linux, Rust 1.99, Node 26 (Node ist am Core-Benchmark nicht beteiligt). Der damalige Lauf von `--example benchmark` verwendete 1000 kleine synthetische Dokumente:

| Operation | Dauer |
| --- | ---: |
| Initiale Indexierung | 181.680746 ms |
| Unveränderter Refresh | 10.943851 ms |
| 100 gleiche Phrasensuchen insgesamt | 658.685348 ms |
| Rechnerischer Durchschnitt je Suche | 6.58685348 ms |
| Save inklusive Indexupdate | 4.187487 ms |

Die alten Werte sind kein Nachweis für den heutigen Abschnittsindex. Aktuell misst der lokale Suchkorpus 956,5 ms für 1000 zusätzliche Dokumente, Docker-Suche p50/p95 6,514/7,303 ms und 4.399.104 SQLite-Bytes. Bessere Relevanz kostet Indexzeit, DB-Speicher (~3,1×) und bei vielen Treffern Latenz; keine allgemeine Garantie, keine Embeddings/semantische Suche implementiert.

Der Reader nutzt gecachte Überschriftenpositionen, passives Scrollen mit RAF/Binärsuche, Bildgrößen aus sicheren PNG/GIF/JPEG-Headern und verzögertes Highlighting bekannter Sprachen. Unbekannte/unmarkierte Blöcke sowie Blöcke über 16.000 Zeichen oder 400 Zeilen bleiben **bewusst Klartext**. [Performance](docs/PERFORMANCE.md) dokumentiert Chromium-Messungen und Grenzen: kein UI-Redesign, kein garantierter FPS-Gewinn, native WebView-Performance noch nicht gemessen.

## Dokumentation und Lizenz

- [Architektur](docs/ARCHITECTURE.md): Technologieentscheidung Tauri/iced/egui, FTS5/Tantivy, Schemas und Sicherheits-/Race-Grenzen.
- [Design](docs/DESIGN.md): Prioritäten, Interaktionen und aktuelles Verhalten.
- [GitHub-Kontoverbindung](docs/GITHUB_INTEGRATION.md): Vellistry Device Flow, öffentliche Identität/URLs, secretloser Refresh, System-Keychains, Phishing-Grenzen, Linux/Hyprland-Hilfe und vollständige manuelle Integrationstest-Checkliste.
- [Alternative Authentifizierung](docs/AUTHENTICATION.md): HTTPS-Helper, SSH-Agent/Hostprüfung und absichtliche Konfigurationsgrenzen.
- [Git-Klondiagnose und Regressionstests](docs/GIT_DIAGNOSTICS.md): reproduzierte Helper-Ursache, sichere Fehlerdiagnose, echte lokale HTTPS-Clone-/Fetch-Tests und GitHub-End-to-End-Abnahme.
- [Changelog](CHANGELOG.md): gesicherter MVP-Stand und Änderungen.
- [Prüfergebnisse](docs/VALIDATION.md): tatsächlich ausgeführte Checks, Linux-Pakete und offene manuelle Prüfungen.
- [Suche](docs/SEARCH.md): Abschnittsindex, Migration, Ranking, Query-Regeln und aktuelle Benchmarks.
- [Performance](docs/PERFORMANCE.md): Reader-Optimierungen, Verhaltensänderungen und Chromium-/Native-Messgrenzen.
- [Roadmap](docs/ROADMAP.md): Bildhärtung, Watcher, Workspace-Entfernung, Device-Flow-Integrationsprüfung und zurückgestelltes Mermaid-Rendering.

MIT, © 2026 Vellistry contributors. Siehe [LICENSE](LICENSE).
