# Architektur

## Status und Grenzen

Mdwell ist ein initiales Entwicklungs-MVP, keine produktionsgehärtete Anwendung. Der integrierte Stand enthält einen unabhängigen Rust-Core, ein Svelte-Frontend und den Tauri-2-Desktop-Host. Der Root-Cargo-Workspace umfasst Core und Desktop mit `default-members = ["crates/core"]`; die npm-Tauri-CLI ist installiert. Desktop-`cargo check` und Binary-Build sind unter Linux bestanden; ein begrenzter direkter nativer Starttest lief fünf Sekunden ohne Fehlermeldungen und wurde anschließend beendet. Native Ende-zu-Ende-Interaktion, Release-/Bundle-/Installer-Abläufe und vollständige Cross-OS-Parität sind damit nicht nachgewiesen.

## Schichten und Datenfluss

- `crates/core`: synchrone Rust-API, Dateizugriff, Git, Markdown-Verarbeitung und persistente Suche; keine Abhängigkeit von einem UI-Framework.
- `src/api.ts` und `src/types.ts`: IPC-Vertrag. Commands verwenden Namen wie `read_document`; Argumente werden als `workspaceId` und `expectedRevision` übertragen, Ergebnisfelder als `workspace_id` und `source_start`.
- `src/App.svelte`: Navigation, Lesen, Suche, Einstellungen, Konflikt- und Entwurfszustand.
- `src/Editor.svelte`: dynamisch geladener CodeMirror-6-Quelleditor.
- `src-tauri/src/main.rs`: initialisiert `Arc<Core>` im Tauri-App-Datenverzeichnis, registriert asynchrone Commands und führt synchrone Core-Operationen mit `spawn_blocking` außerhalb des UI-Threads aus; Fehler werden als `ErrorInfo` übertragen.
- `src-tauri/tauri.conf.json`: CSP und inline definierte Capability für das Hauptfenster. Erlaubt sind `core:default`, der Öffnen-Dialog und der URL-Opener nur für HTTP(S); keine allgemeine Dateisystem-Plugin-Freigabe. Die CSP begrenzt Scripts auf `self`, Verbindungen auf Tauri-IPC und Bilder auf `self`/`blob:`/`data:` (validierte Konto-Avatare); Frames und Objects sind gesperrt. Inline-Styles sind für die UI erlaubt. Diese Konfiguration ist keine umfassend auditierte Sandbox.

Markdown-Dateien bleiben die Quelle der Wahrheit. Lesen rendert die aktuelle Datei; Suche und Dokumentliste lesen einen SQLite-Snapshot. Speichern aktualisiert Datei und Index. `refresh_workspace` durchsucht den Workspace und repariert diesen Snapshot nach externen Änderungen. GitHub-Sync ist eine ausdrückliche Netzwerkaktion, kein Hintergrunddienst.

## UI-Technologie: Tauri gegenüber iced und egui

| Ansatz | Passung zum Produkt | Kosten und offene Fragen |
| --- | --- | --- |
| Tauri 2 + Svelte 5 | DOM/CSS für lange Dokumente, Textauswahl, Tabellen, Links; CodeMirror und highlight.js können direkt genutzt werden. Rust-Core bleibt unabhängig. | System-WebView, IPC und plattformspezifische Abhängigkeiten; WebView-Prozesse und JS/DOM verbrauchen Speicher. Sanitization, CSP und Capabilities sind zwingend. |
| iced | Rust-zentrierte, deklarative native UI; mögliche Alternative für einen späteren Host. | Markdown-Lesefläche, Quelleditor und Barrierefreiheit müssten gesondert aufgebaut und geprüft werden; vorhandene Web-Komponenten sind nicht direkt wiederverwendbar. |
| egui | Immediate-Mode-UI, geeignet für Werkzeuge und Prototypen; Core ebenfalls wiederverwendbar. | Dokumentlayout, Textselektion, Navigation und Editor-Integration benötigen eigene Lösungen; Rendering- und Aktualisierungskosten müssen gemessen werden. |

Tauri ist die gewählte Integrationsrichtung wegen der vorhandenen dokumentorientierten Web-Komponenten, nicht aufgrund eines behaupteten RAM-Vorteils. Es gibt keine gemessenen Vergleichswerte für RSS, Startzeit oder Idle-Verbrauch. Native Toolkits sind nicht automatisch sparsamer; auch Tauri ist nicht automatisch leichtgewichtig. Ein fairer Vergleich benötigt dasselbe Dokumentkorpus, dieselben Funktionen, OS/Hardware-Angaben sowie den Speicher aller beteiligten Prozesse. Bundle-Größe ist kein RAM-Messwert.

## Persistenz und Schemas

`Core::open(data_dir)` erstellt `mutation.lock` und `index.sqlite3`; SQLite läuft mit WAL, Foreign Keys und fünf Sekunden Busy-Timeout. GitHub-Checkouts liegen unter `repositories/<SHA-256 der normalisierten URL>`.

| Tabelle | Inhalt / Schlüssel |
| --- | --- |
| `workspaces` | `id` als Primärschlüssel, `kind`, `name`, eindeutiger kanonischer `root`, optionale eindeutige `url` |
| `documents` | `(workspace_id, path)` als Primärschlüssel, `title`, SHA-256-`revision`; Workspace-FK mit Cascade |
| `document_blocks` | `(workspace_id, path, source_start)` als Primärschlüssel, extrahierter Klartext `content`; Dokument-FK mit Cascade |
| `search_schema` | Suchschema-Version, aktuell `version = 1` |
| `search_documents` | Suchmetadaten und Rebuild-Marker pro Dokument |
| `section_fts` | FTS5 pro Präambel/Überschriftenabschnitt mit geerbtem Überschriftenpfad |
| `search_blocks` und Block-FTS | Abschnittsgebundener Klartext, originale UTF-8-Start-/Endoffsets und indexierte Blocknavigation |

Hinzu kommen persistierte Vokabularzuordnung und indizierte Ein-Zeichen-Löschsignaturen für Tippfehlerkandidaten.

Suchschema v1 migriert transaktional aus gecachten `document_fts`-/`document_blocks`-Daten, ohne Dateisystem-/Netzwerkzugriff; erst danach wird `document_fts` entfernt. Metadaten und Revisionen bleiben erhalten, Rebuild-Marker erzwingen beim nächsten erfolgreichen Refresh den vollständigen Neuaufbau. Legacy-Überschriften bleiben zunächst ein Fallback-Abschnitt mit unbekanntem Endoffset; Fehler rollen zurück, neuere Versionen werden abgelehnt. Das ist kein allgemeines versioniertes App-Schema. Die DB enthält Text/Pfade unverschlüsselt; Backup/Recovery und Workspace-Entfernung bleiben offen. Details: [Suche](SEARCH.md).

Transportmodelle sind `Workspace`, `DocumentSummary`, `Document` (inklusive Quelle, bereinigtem HTML und TOC), `SearchHit`, `RefreshReport`, `ResolvedLink` und `Asset`. `ErrorInfo` transportiert einen stabilen `code`, eine Meldung und bei Konflikten erwartete/tatsächliche Revision. OS-/DB-Fehlerobjekte bleiben in Rust; Meldungen können trotzdem lokale Pfade enthalten.

## Volltextsuche: SQLite FTS5 statt Tantivy

SQLite FTS5 ist implementiert (`rusqlite` mit gebündeltem SQLite). Metadaten und Suchindex teilen sich eine Datenbank und Transaktionen; das reduziert für das MVP die Integrationskomplexität gegenüber separater SQLite-/Tantivy-Persistenz. Tantivy bleibt eine mögliche Alternative, falls belastbare Profile oder benötigte Suchfunktionen den Wechsel rechtfertigen. Es existiert kein gemessener FTS5/Tantivy-Vergleich.

- `unicode61 remove_diacritics 2`, Präfixindex für Längen 2, 3 und 4.
- Abschnittsindex mit geerbten Überschriften, nicht geerbtem Eltern-Inhalt; AND gilt innerhalb eines Abschnitts, nicht über Geschwisterabschnitte hinweg.
- Unicode-Wörter werden AND-verknüpft; letzter unzitierter Term ist implizit ein Präfix, explizites `wort*` bleibt möglich. Zitierte Phrasen bleiben exakt. Über 1024 Query-Bytes oder 16 Wort-/Phrasenslots: keine Treffer; keine freie Operatorsprache.
- Lexikografisches Ranking: exakter normalisierter Dateiname/-stamm vor Überschriften-/Titelstärke, dann BM25 (Überschriften 12, Dateiname 10, Titel 8, Pfad 2, Inhalt 1), stabile Tie-Breaks. Bester Abschnitt je Dokument wird vor dem 50-Dokument-Limit gewählt. `score` ist nur rohes BM25; Backend-Reihenfolge beibehalten.
- Nur ohne initiale Treffer: unabhängige Korrektur unzitierter Wörter (3–64 Zeichen), bis acht Alternativen pro AND-Slot, höchstens 128 Distanzprüfungen pro Wort über persistierte Löschsignaturen. Erlaubt sind ein Edit oder eine benachbarte Vertauschung, keine beliebige Distanz zwei; Phrasen bleiben unverändert.
- Indexierte Blocknavigation ohne vollständigen Blockscan je Treffer. `SearchHit` enthält `section_title`, optionales exklusives `source_end` und `matched_terms`; Snippets sind Klartext. Offsets bezeichnen **Blöcke**, keine exakten Tokenbereiche; Legacy-/Metadatentreffer nutzen Fallbacks.
- Keine Wortzerlegung, Stemming, Substring- oder semantische Suche. Embeddings sind nicht implementiert und höchstens eine Zukunftsidee. Regeln und Grenzen: [Suche](SEARCH.md).

Refresh liest und hasht auch unveränderte Dateien, überspringt aber deren Rendering und Neuindexierung. Geänderte Render-Ergebnisse werden bis zur Transaktion gesammelt; große Workspaces können daher Speicher und Laufzeit beanspruchen. Es gibt kein dokumentiertes Gesamt-Korpuslimit und keine Streaming-Indexpipeline.

## Markdown, Links und Assets

`pulldown-cmark` verarbeitet Tabellen, Durchstreichungen, Tasklisten, Fußnoten und GFM-Admonitions. Überschriften erhalten eindeutige IDs; Block-Wrapper tragen `data-source-start`/`data-source-end`. Geschlossenes Frontmatter am Dokumentanfang (`---` bis `---` oder `...`, auch mit BOM/CRLF) wird für Rendering und Textindex ausgeblendet, bleibt aber unverändert in der Quelle. Parser-Ranges werden um dessen Bytelänge versetzt: TOC und Blockoffsets beziehen sich weiterhin auf die Originalquelle. Es ist kein YAML-Metadatenparser; ungeschlossenes Frontmatter bleibt normales Markdown.

Raw-HTML-Events werden verworfen, anschließend bereinigt `ammonia` das erzeugte HTML. Lokale Markdown-Bilder erzeugen ausschließlich inerte `<img data-local-src="…" alt="…">` ohne `src`/`srcset`. Remote-/unsichere Bildziele werden als Alt-Text ausgegeben. `src/images.ts` löst relative Referenzen über `resolve_link` auf, akzeptiert nur Asset-Ergebnisse, liest begrenzte Workspace-Bytes und setzt erst dann eine Blob-URL. Es weist direkte Remote-/Schema-/absolute Referenzen erneut zurück. Veraltete Antworten werden ignoriert; Cleanup bzw. Decoderfehler entfernen `src` und widerrufen Blob-URLs. Fehler liefern einen zugänglichen Text-Fallback. Keine allgemeine Dateisystemfreigabe oder Remote-Bildanfrage. Tasklisten sind nicht interaktiv. highlight.js verarbeitet im Frontend den Text von Codeblöcken.

Relative Dokumentlinks werden innerhalb des Workspaces aufgelöst; Fragmentnavigation ist vorhanden. Der Core akzeptiert externe HTTP(S)- und mailto-Links ohne Zugangsdaten, das Frontend öffnet jedoch ausschließlich explizite HTTP(S)-URLs und nur nach Benutzeraktion. Lokale Raster-Assets können als Markdown-Bilder eingebettet und per Link in einem Dialog angezeigt werden. Der Core erlaubt PNG/JPEG/GIF/WebP/AVIF; die UI-Allowlist erlaubt PNG/JPEG/GIF/WebP/BMP. AVIF/BMP-Unterstützung ist deshalb noch nicht Ende-zu-Ende konsistent. SVG ist absichtlich ausgeschlossen. MIME-Werte beruhen auf Dateiendungen, nicht auf einer Inhaltsvalidierung. Mermaid-Rendering fehlt; solche Blöcke bleiben Code.

## Dateisicherheit, Speichern und verbleibende Races

- Dokumente: UTF-8, `.md`/`.markdown` ohne Beachtung der Groß-/Kleinschreibung, maximal 16 MiB. Assets: maximal 8 MiB.
- Relative Pfade müssen normalisiert sein; absolute Pfade, Backslashes, NUL, leere Komponenten, `.` und `..` werden abgelehnt. Kanonische Ziele müssen Dateien innerhalb des kanonischen Workspace-Roots sein.
- Feste Ausschlüsse: `.git`, `node_modules`, `target`; keine konfigurierbaren Regeln, kein `.gitignore`-Parser. Verzeichnis-Symlinks werden nicht verfolgt; Datei-Symlinks dürfen nicht aus dem Workspace oder in ausgeschlossene Verzeichnisse führen. Alias-Dateien können separat im Index erscheinen.
- Revisionen sind SHA-256 über exakte Dateibytes, keine Zeitstempel. Speichern existierender Dateien prüft die erwartete Revision vor dem Schreiben und erneut vor dem Ersetzen.
- Temporäre Datei im Zielverzeichnis, Erhalt der Permissions, `sync_all`, anschließend atomarer Austausch; unter Unix zusätzlich Verzeichnis-Sync. Keine Force-Save-API.
- Mutex plus OS-Dateilock serialisieren Mutationen kooperierender Core-Instanzen mit demselben Datenverzeichnis. Andere Datenverzeichnisse bzw. externe Editoren nehmen diesen Lock nicht.
- Externe Writer können zwischen finaler Revisionsprüfung und Rename schreiben. Kanonisierung verhindert keine adversarialen Verzeichnis-/Symlink-Austauschraces. Keine Sandbox für einen bösartigen lokalen Prozess mit denselben Rechten.
- Dateiaustausch und SQLite-Commit sind keine gemeinsame Transaktion: ein Fehler nach erfolgreichem Dateischreiben kann einen veralteten Index hinterlassen. Refresh stellt den Index aus Dateien wieder her. Ein fehlgeschlagener Scan aktualisiert den bisherigen Index nicht teilweise.
- Native relative Pfade werden aus `Component::Normal`-Komponenten zu slash-getrennten API-Pfaden zusammengesetzt; Windows-Trennzeichen werden damit strukturell normalisiert. Literal-Backslashes in API-Eingaben bleiben verboten. Die frühere Windows-Normalisierungslücke ist behoben, Windows-Laufzeitverhalten aber noch nicht getestet; erfolgreiche Linux-Tests beweisen keine Windows-Parität.

Der Browserzustand schützt Entwürfe vor veralteten Navigationsergebnissen und Polling. Während eines Saves getippte Änderungen bleiben erhalten. Das ersetzt keine Dateisystem-CAS-Garantie und keine crashfeste Entwurfswiederherstellung.

## Git und Netzwerkgrenze

Import akzeptiert öffentliche/private Repositories über `https://github.com/owner/repo` (optionales `.git`), `git@github.com:owner/repo.git` oder `ssh://git@github.com/owner/repo.git`. Nur exakt `github.com`, bei SSH nur Benutzer `git`; keine Ports, HTTPS-Zugangsdaten/Tokens, Queryparameter oder Fragmente. HTTPS-/SSH-Identitäten bleiben getrennt, ohne automatischen Transport-Fallback. Clone ist shallow, single-branch, ohne Tags. Sync prüft einen sauberen Checkout vor und nach Fetch, verwendet die gespeicherte validierte URL statt eines veränderten `origin` und erlaubt nur Fast-Forward. Ignorierte lokale Dateien dürfen nicht überschrieben werden. Kein Commit, Push, Merge-UI, Submodul- oder LFS-Download.

Git wird ohne Shell gestartet. Vor jedem Aufruf wird außerhalb des Checkouts eine Allowlist vertrauenswürdiger System-/Global-Konfiguration inklusive Includes gelesen: Credential-Helper/-Benutzer/-Pfadoptionen und begrenzte Proxy-/TLS-Einstellungen. HTTPS nutzt bei verbundenem Konto die native GitHub-App-Autorisierung, ansonsten vorhandene GitHub-CLI-/GCM-Helper. Der eigene Credential-Helper läuft als früh abgefangener Modus desselben Executables und erhält Tokens nur über einen kurzlebigen, capability-geschützten Loopback-Kanal; Tokens stehen nicht in Argumenten, Umgebung, URLs oder Dateien. OAuth-Fehler wechseln nicht still auf andere Credential-Helper; SSH nutzt bestehende vertrauenswürdige SSH-Konfiguration, Agent und `known_hosts` mit `BatchMode=yes`/`StrictHostKeyChecking=yes`. Unbekannte/geänderte Hostkeys werden nicht automatisch akzeptiert. Helper und SSH-Konfiguration sind ausführbare Benutzer-Policy, keine Sandbox.

Snapshots bleiben im Speicher und werden per Prozessumgebung übergeben. Unsichere/benutzerdefinierte lokale Git-Einstellungen werden vor Operationen abgelehnt; nur dokumentierte Standardmetadaten sind erlaubt. Linked/Worktree-Git-Verzeichnisse sind nicht unterstützt. Hooks/fsmonitor, Submodulrekursion, LFS-Smudge, Redirects und injizierte Git-Konfiguration bleiben deaktiviert, TLS-Prüfung aktiv. Commands: 120 s Timeout, Konfigurationslesen: 10 s. Git selbst fragt niemals interaktiv nach Credentials. Die integrierte Anmeldung läuft im Systembrowser mit PKCE/State, exakt registriertem Loopback-Callback und vertraulichem HTTPS-Broker; Zugangsdaten verbleiben im nativen Secret Store, nicht in SQLite/JSON. Details und erforderliche externe Einrichtung: [GitHub-Integration](GITHUB_INTEGRATION.md). Fehler liefern bereinigte Codes ohne rohe Git-/Helper-Ausgabe. Einrichtung und genaue Trust-Grenzen: [Authentifizierung](AUTHENTICATION.md).

## Polling, Tests und Performance

Das Frontend pollt alle vier Sekunden bei sichtbarer App. Für jeden Workspace ruft es zuerst `api.refresh` und anschließend `api.documents` auf: neue, geänderte und gelöschte Dateien werden damit im Suchindex berücksichtigt. Refresh liest/hasht den Bestand, rendert und indexiert aber nur Änderungen. Listen werden vor Zuweisung verglichen, der Dateibaum bleibt bei unveränderten Summaries gecacht. Das aktuelle saubere Dokument wird nur bei geänderter indexierter Revision neu gelesen; Navigations-/Save-/Entwurfsschutz gilt vor und nach dem Read. `Promise.allSettled` wartet auch auf fehlgeschlagene Scans, verhindert überlappende Polling-Runden und erhält erfolgreiche Listenupdates; Fehler werden angezeigt. Bei Fehlern kann die Aktualisierung des aktuellen Dokuments ausbleiben. Es gibt **keinen automatischen Remote-Sync** und noch keinen Dateisystem-Watcher; wiederholte Vollscans haben auch bei unverändertem Bestand I/O-Kosten.

Tests prüfen Persistenz, Migration/Rollback, Ranking/Query-Grenzen, Sanitization/Offsets, Pfad-/Asset-Grenzen, revisionsabhängige Saves und Entwurfsschutz. Auth-Tests nutzen reale lokale Git-Fixtures, Dummy-Helper und Mock-SSH ohne GitHub-Netzwerkzugriff; Frontendtests mocken IPC/OS-APIs. Aktueller Linux-Prüfstand: Workspace-Formatting, `cargo test --workspace --all-targets`, Clippy mit `--all-targets -- -D warnings`, Desktop-Binary-Build, npm-Check, 82 Frontendtests, Frontend-Build und Audit (0 Schwachstellen) bestanden. Separater Core-Lauf: 32 Tests plus ein Doc-Test. Kein Nachweis echter authentifizierter GitHub-Verbindungen, nativer Ende-zu-Ende-Interaktion oder macOS-/Windows-Runtime; Release/Bundles/Installer bleiben ungeprüft. Die Editor-Chunk-Warnung bleibt bestehen.

**Historischer MVP-Benchmark**, nicht der heutige Abschnittsindex; abgelöst durch [aktuelle Suchmessungen](SEARCH.md). Reproduktion des alten Beispielablaufs:

```sh
cargo run -p mdwell-core --release --example benchmark
```

Das Beispiel erzeugt 1000 kleine synthetische Dokumente, misst initiale Indexierung, unveränderten Refresh, die Gesamtdauer von 100 gleichen Phrasensuchen und Save inklusive Indexupdate. Aufgezeichneter Release-Lauf in dieser Entwicklungsumgebung: **Linux, Rust 1.99, Node 26** (Node wird vom Core-Benchmark nicht verwendet).

| Operation | Aufgezeichnete Dauer |
| --- | ---: |
| Initiale Indexierung, 1000 Dokumente | 181.680746 ms |
| Unveränderter Refresh | 10.943851 ms |
| 100 gleiche Phrasensuchen insgesamt | 658.685348 ms |
| Rechnerischer Durchschnitt je Suche | 6.58685348 ms |
| Save inklusive Indexupdate | 4.187487 ms |

Die aktuellen Suchmessungen dokumentieren auch p50/p95 und DB-Größe: bessere Relevanz kostet Indexzeit, DB-Speicher (~3,1×) und High-Fanout-Latenz. Historische Durchschnittswerte belegen kein aktuelles p95-Ziel; reale Korpora und Prozessspeicher bleiben zu profilieren.

Reader-Optimierungen ohne UI-Redesign: gecachte Überschriftenoffsets, passiver Scroll-Listener mit einem RAF und Binärsuche; Invalidierung außerhalb des normalen Scrollpfads bei Resize, Bildern, Fonts und Leseeinstellungen. Sichere PNG/GIF/JPEG-Header reservieren bekannte Bildgrößen vor Blob-Zuweisung, ohne allgemeine Layout-Shift-Garantie. Highlighting startet verzögert, nur für explizite registrierte Sprachen, mit etwa 4-ms-Slices und Pause beim Scrollen. Unbekannte/unmarkierte Blöcke und Blöcke über 16.000 Zeichen oder 400 Zeilen bleiben absichtlich Klartext: eine Verhaltensänderung, nicht nur schnellerer identischer Output.

Such-/Find-Markierungen erhalten Links, Copy und Highlight-Spans; Such-Textwalks bleiben auf source-überlappende Blöcke begrenzt. [Performance](PERFORMANCE.md) beschreibt die Chromium-Harness-Messungen mit gemocktem IPC. Diese messen weder Rust-Rendering noch native WebView-Performance und belegen keinen FPS-Gewinn; macOS-/Windows-Performance ist nicht geprüft.
