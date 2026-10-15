# Roadmap

Status: initiales Entwicklungs-MVP, nicht produktionsgehärtet. Die Liste trennt überprüften Code von Integrationsarbeit und Zukunftsideen; sie enthält keine Terminversprechen.

## Implementiert

- [x] Unabhängiger Rust-Core für lokale Ordner und öffentliche/private GitHub-Repositories über HTTPS/SSH.
- [x] Systemauthentifizierung: vertrauenswürdige System-/Global-Helper (GitHub CLI/GCM) oder bestehende SSH-Konfiguration, Agent und strikte `known_hosts`-Prüfung. Zusätzlich integrierte [GitHub-App-Anmeldung](GITHUB_INTEGRATION.md) mit PKCE, Browser, Auth-Broker, System-Keychain und Repository-Auswahl (externe Registrierung/Bereitstellung und reale Ende-zu-Ende-Prüfung erforderlich). Kein Klartext-Credential-Speicher oder Token-URL; benutzerdefinierte lokale Git-Konfiguration absichtlich nicht unterstützt. [Einrichtung und Grenzen](AUTHENTICATION.md).
- [x] SQLite-Metadaten/FTS5, SHA-256-basierter inkrementeller Abschnittsindex mit Überschriftenhierarchie; exakter Dateinamen-Rang, Überschriften-/Titelstärke, BM25, Phrasen, implizites letztes Präfix und begrenzte unabhängige Mehrwort-Tippfehlerkorrektur. [Query-Grenzen und Messungen](SEARCH.md).
- [x] Transaktionale Offline-Migration des Suchschemas auf v1 aus Legacy-Caches, Revisionserhalt, Rebuild-Marker, Rollback und Ablehnung neuerer Versionen; kein allgemeines App-Migrationssystem.
- [x] Suchkontext/-markierungen mit `section_title`, `source_end`, `matched_terms`, blockbegrenzten Textwalks, Unicode-/Präfixabbildung, History und erhaltenen Links/Copy/Find-Layern; Source-Offsets bleiben Blockoffsets.
- [x] GFM inklusive Admonitions, ausgeblendetes Frontmatter mit erhaltenen Originaloffsets, HTML-Bereinigung, TOC und UTF-8-Blockoffsets; keine exakten Substring-Offsets.
- [x] Revisionsabhängiges Speichern bestehender Dateien, atomarer Austausch und Locks für kooperierende Instanzen.
- [x] Expliziter GitHub-Fetch/Fast-Forward mit Dirty-Checkout-Schutz; keine Commit-/Push-Funktion.
- [x] Svelte-Leser, Dateibaum, Navigation, globale Suche, CodeMirror-Quellmodus, Find, Code-Copy und persistente Lesepräferenzen.
- [x] Entwurfsschutz bei Navigation, Polling, Konflikten und während laufender Saves.
- [x] Vier-Sekunden-Polling mit lokalem `api.refresh` je Workspace vor den Listen: inkrementelle Indexupdates, gecachte unveränderte Summaries/Dateibäume und Read nur bei geänderter aktueller Revision; **kein Remote-Sync im Polling**.
- [x] Reader-Optimierung ohne UI-Redesign: gecachte Überschriftenoffsets, passiver Scroll-Listener/RAF/Binärsuche, Resize-/Bild-/Font-Invalidierung und bekannte Bildgrößen aus sicheren PNG/GIF/JPEG-Headern. Verzögertes Highlighting bekannter expliziter Sprachen; unbekannte/unmarkierte oder über 16.000 Zeichen/400 Zeilen große Blöcke bleiben bewusst Klartext. [Chromium-Messungen](PERFORMANCE.md), kein nativer FPS-Nachweis.
- [x] Feste Ausschlüsse `.git`, `node_modules`, `target`; **keine konfigurierbaren Regeln und keine `.gitignore`-Auswertung**.
- [x] Begrenzte lokale Raster-Asset-API, Anzeige verlinkter Assets und eingebettete lokale Markdown-Bilder über inerte Referenzen und Blob-URLs mit Cleanup-/Race-Tests.
- [x] Tauri-2-Host mit asynchronen Commands/`spawn_blocking`, App-Datenverzeichnis, restriktiver CSP und Hauptfenster-Capability; Desktop-`cargo check` und Binary-Build unter Linux bestanden.
- [x] Root-Cargo-Workspace mit `default-members = ["crates/core"]` und installierte npm-Tauri-CLI.
- [x] Aktueller Linux-Prüfstand: `cargo fmt --all --check`, `cargo test --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build -p mdwell-desktop`, npm-Check, 71 Frontendtests, Frontend-Build und Audit (0 Schwachstellen) bestanden. Separater Core-Lauf: 32 Tests plus ein Doc-Test. Editor-Chunk-Warnung bleibt bestehen.
- [x] Windows-Pfadkomponenten-Normalisierung korrigiert; portable Git-Konfigurationsisolation mit vertrauenswürdiger System-/Global-Allowlist und leerem Hooks-Verzeichnis ersetzt Unix-Spezialpfade. Windows-Runtime noch nicht getestet.
- [x] Direkter nativer Linux-Starttest: Prozess blieb fünf Sekunden ohne Fehlermeldungen aktiv und wurde danach beendet. Ein erster Xvfb-Harness-Lauf hatte ein Timeout-/Cleanup-Problem, keinen nachgewiesenen App-Fehler.
- [x] Core-/Git-Fixture-Tests, gemockte Frontend-/IPC-Tests und synthetisches Release-Benchmark-Beispiel.

## Unmittelbare Validierung und Medien

- [ ] Native Interaktion, echte authentifizierte GitHub-Verbindungen und Ende-zu-Ende-Abläufe auf echten Desktop-Plattformen vollständig prüfen; bisherige Auth-Tests laufen offline mit lokalen Fixtures/Dummy-Helpern/Mock-SSH. Linux-Check, Binary-Build und begrenzter Starttest sind bestanden; Release-Builds, Tauri-Bundles und Installer sind noch nicht getestet.
- [ ] CI für Core-Tests/Formatting, Frontend-Checks/Tests/Build und Desktop-Check/Build auf Linux, macOS und Windows erfolgreich durchlaufen lassen. Keine Signing-Secrets voraussetzen.

- [ ] Raster-Allowlisten zwischen Core und Frontend vereinheitlichen; Inhaltsvalidierung und Decoder-Grenzen prüfen. Remote-Bilder/SVG nicht nebenbei freischalten.
- [ ] Windows-Runtime mit portabler Pfadnormalisierung und Git-Isolation tatsächlich testen; Cross-OS-CI und Linux-Tests allein ersetzen keine Interaktionstests.

## Zuverlässigkeit vor Funktionsbreite

- [ ] Dateisystem-Watcher mit Debounce, gezielter Indexaktualisierung, Rescan-Fallback und Fehlerzuständen. Polling aktualisiert bereits den lokalen Suchindex inklusive neuer/gelöschter Dateien, liest/hasht dabei aber wiederholt den Bestand.
- [ ] Workspace-Entfernung in Core und UI. Registrierung/Index entfernen und Checkout-Löschung getrennt bestätigen; lokale Originaldateien erhalten. Derzeit fehlt eine Remove-API vollständig.
- [ ] Konfigurierbare Ausschlüsse mit eindeutiger Semantik; feste Regeln bleiben die heutige Implementierung.
- [ ] Allgemeines versioniertes App-/DB-Schema sowie Backup-/Recovery- und explizite Rebuild-Verfahren definieren. Suchschema-v1-Migration einschließlich offline fehlender Roots und Rebuild beim nächsten erfolgreichen Refresh ist bereits implementiert.
- [ ] Crashfeste Entwurfswiederherstellung und optionaler manueller Konfliktvergleich; keine stille Force-Save-Funktion.
- [ ] Fehler nach Datei-Rename vor DB-Commit, externe Writer-Races und adversariale Symlink-/Directory-Swaps gezielt testen/härten. Keine atomare CAS-Garantie gegenüber fremden Editoren behaupten.
- [ ] Echte Desktop-Ende-zu-Ende-Tests, Screenreader-/Fokus-/Kontrastprüfung und Close-Request-Tests auf allen Plattformen.
- [ ] Reale Suchkorpora, Hardware-/Cache-Baseline und Prozessspeicher profilieren; native GTK/WebKit-, macOS-/Windows-Reader-Performance messen. [SEARCH.md](SEARCH.md) enthält aktuelle p50/p95-, Qualitäts- und DB-Größenmessungen samt Tradeoff (mehr Indexzeit, ~3,1× DB-Speicher, höhere High-Fanout-Latenz); der frühere MVP-Benchmark ist historisch und abgelöst. [PERFORMANCE.md](PERFORMANCE.md) misst Chromium mit gemocktem IPC, keinen nativen WebView-/FPS-Gewinn. Ziele unter 2 s/1000 Dokumente und p95-Suche unter 50 ms bleiben workloadabhängige Ziele, keine Garantie.

## Später / bewusst zurückgestellt

- [ ] Integrierte OAuth-Anmeldung samt Scopes, Widerruf/Abmeldung und sicherer OS-Credential-Übergabe evaluieren; ausdrücklich zurückgestellt. Private Repositories mit vorhandenen HTTPS-Helpern/SSH-Agent sind bereits implementiert.
- [ ] Embeddings/semantische Suche höchstens nach nachgewiesenem Bedarf evaluieren; nicht implementiert oder zugesagt.
- [ ] Mermaid-Rendering erst nach isoliertem Rendering-/Sanitization-Konzept und Ressourcenlimits; derzeit normale Codeblöcke.
- [ ] Signierte/notarisierte Releases, Installer-/Upgrade-Tests und Release-Prozess. Der normale CI-Build benötigt keine Secrets und liefert keine signierten Releases.
- [ ] Größere Korpora/Streaming-Indexierung profilieren; Tantivy nur bei nachgewiesenem Bedarf evaluieren, nicht aufgrund erfundener Vergleichsmetriken.

Nicht Teil des aktuellen MVP: WYSIWYG, Zusammenarbeit/Cloud-Sync, Git-Push, Submodule/LFS-Verwaltung, allgemeines Plugin-System oder unbeschränkter Remote-HTML-/Bildzugriff.
