# Design

## Produktidee und Prioritäten

Mdwell ist ein ruhiger, lokaler Leseplatz für Markdown aus eigenen Ordnern und öffentlichen/privaten GitHub-Repositories. Das initiale Entwicklungs-MVP priorisiert:

1. **Daten erhalten:** Quelldateien bleiben maßgeblich; explizites Speichern mit Revision, Entwurfsschutz und verständliche Konflikte vor automatischer Bequemlichkeit.
2. **Gut lesen:** begrenzte Zeilenlänge, klare Typografie, Inhaltsverzeichnis, Code-Highlighting und wenig visuelle Ablenkung.
3. **Schnell orientieren:** Workspace-Dateibaum, globale Suche, History, interne Links und Tastaturpalette.
4. **Vertrauensgrenzen sichtbar halten:** kein stiller Remote-Sync, keine entfernten Bilder, keine ungeprüften HTML-Snippets.
5. **Einfacher Aufbau:** UI-unabhängiger Rust-Core; vorhandene Komponenten statt eines eigenen Editors oder einer zweiten Suchdatenbank.

Keine Kollaborationssuite, kein WYSIWYG-Editor, keine Git-IDE und noch keine produktionsgehärtete Anwendung.

## Aktuelle Oberfläche

- Sidebar mit aufklappbaren Workspaces und verschachtelten Dateipfaden; eine ausblendbare Sidebar ist vorhanden, Workspace-Entfernung nicht.
- Toolbar mit Zurück/Vorwärts, Pfad, Refresh, optionalem GitHub-Sync und Read/Source-Wechsel.
- Leser mit Tabellen/GFM-Admonitions, lokalen eingebetteten Rasterbildern, TOC, highlight.js und Copy-Code-Aktion. Geschlossenes Frontmatter ist im Leser ausgeblendet, bleibt im Quellmodus erhalten; Source-Offsets beziehen sich weiter auf die Originalquelle. Überschrift-IDs sind eindeutig, aber nicht als vollständige GitHub-Anchor-Kompatibilität spezifiziert.
- System-/Hell-/Dunkel-Theme, Serif-/Sans-Leseschrift und Größe von 14–24 px. Präferenzen, letzter Dokumentpfad und Lesescrollpositionen liegen in `localStorage`; History und Entwürfe sind nicht dauerhaft gesichert.
- CodeMirror 6 wird erst beim Quellmodus geladen. Kein Live-Preview des Entwurfs: Read zeigt die zuletzt gelesene/gespeicherte Version.
- Suchdialog mit 180-ms-Debounce, Abschnittskontext, sicher als Text gerenderten Snippets/Begriffsmarkierungen und Pfeiltasten-/Enter-Auswahl. Veraltete Suchantworten werden ignoriert. Abschnittssuche priorisiert exakte Dateinamen und Überschriften, ergänzt das letzte Wort als Präfix und korrigiert begrenzt Mehrwort-Tippfehler; Phrasen bleiben exakt. Regeln und Grenzen: [Suche](SEARCH.md).
- Repository-Eingabe akzeptiert HTTPS und beide dokumentierten SSH-Formen; keine Token-Felder oder persistierten URL-Entwürfe. Git-Fehler bieten Hinweise zu Helper, Agent, geprüften Hostkeys, Berechtigungen/SSO und Netzwerk. Dokumentationslinks öffnen erst nach Klick; kein UI-Redesign.
- Dialoge stellen den Fokus wieder her; Schließen und Dokumentwechsel fragen bei ungespeicherten Änderungen nach. Native Close-Requests werden über die Tauri-Window-API abgefangen, müssen aber im echten Host geprüft werden.

## Navigation und Bearbeitung

`Ctrl`/`Cmd` + `K` öffnet die Palette, + `F` die Dokument-Suche, + `Shift` + `F` die Workspace-Suche, + `S` speichert. Source-Toolbar und TOC sind Alternativen zu Double-Click-/Tastaturaktionen. Eine umfassende Screenreader-, Kontrast- und plattformübergreifende Tastaturprüfung steht aus.

Suchtreffer und Doppelklicks navigieren zu gemappten Markdown-Blöcken. Suchmarkierungen nutzen `matched_terms` nur in source-überlappenden Blöcken, mit Unicode-/Akzent-/Präfixabbildung; sichtbare Markierungen werden beim Scrollziel bevorzugt. Links, Copy und die separate Find-Markierung bleiben erhalten, History bewahrt Suchbereiche/-begriffe. Core-Offsets sind UTF-8-Bytes und bezeichnen **Blöcke, keine exakten Teilzeichenfolgen**; CodeMirror erhält weiterhin den nach UTF-16 konvertierten Blockanfang. Legacy-Treffer ohne Endoffset nutzen einen nächsten Block als Fallback. Nach Entwurfsänderungen können gespeicherte Offsets veraltet sein. Dokument-Find wartet 150 ms, verwirft veraltete Arbeit und führt Enter/Vor/Zurück sofort aus.

Speichern übermittelt die erwartete SHA-256-Revision. Bei Konflikt bleibt der Entwurf erhalten; Copy Draft und bestätigtes Reload ermöglichen manuelles Zusammenführen. Es gibt kein Force-Overwrite und noch keinen Merge-Editor. Während Save getippte Änderungen bleiben ungespeichert im Editor. Ein In-Memory-Entwurf ist kein Backup gegen Crash oder Stromausfall.

## Aktualisierung und Medien

Polling alle vier Sekunden ruft bei sichtbarer App für jeden Workspace zuerst `api.refresh`, dann die Dokumentliste ab. Damit werden externe Änderungen sowie neue/gelöschte Dateien im Suchindex berücksichtigt; nur geänderte Dateien werden neu gerendert/indexiert, der Bestand wird trotzdem gelesen/gehasht. Unveränderte Summaries/Dateibäume bleiben gecacht; das aktuelle saubere Dokument wird nur bei geänderter indexierter Revision neu gelesen. Dirty Entwürfe sowie Navigation-/Save-Guards bleiben vor und nach dem Read geschützt. Fehler werden angezeigt, erfolgreiche Workspace-Listen bleiben erhalten und Scans überlappen nicht zwischen Polling-Runden. Ein Watcher fehlt weiter. Sync greift bei GitHub ausdrücklich auf das Netzwerk zu und wird nie vom Polling ausgelöst. Dirty Source verhindert Remote-Sync; ein schmutziger Git-Checkout wird auch vom Core abgelehnt.

Lokale Markdown-Bilder werden jetzt eingebettet: der Core liefert inerte Bildreferenzen ohne ladendes `src`; das Frontend löst sie über die workspacegebundene Asset-API auf und setzt Blob-URLs. Navigation, Moduswechsel und Fehler räumen URLs auf; veraltete Antworten werden ignoriert, fehlende/ungültige Bilder erhalten einen Text-Fallback. Normale Asset-Links öffnen weiterhin einen Dialog. AVIF/BMP-Allowlisten zwischen Core und Frontend sind noch nicht konsistent. Remote-Bilder und SVG bleiben aus Sicherheits-/Datenschutzgründen ausgeschlossen. Mermaid-Rendering ist zurückgestellt; Diagramm-Code ist weiterhin lesbar.

## Authentifizierung und Reader-Performance

Private Repositories sind über vorhandene vertrauenswürdige System-/Global-Credential-Helper (GitHub CLI/GCM) oder SSH-Konfiguration mit Agent und strikt geprüften `known_hosts` implementiert. Login/Schlüsselladen/Hostprüfung erfolgen vorher außerhalb Mdwell. Diese alternativen Wege bleiben neben der integrierten [GitHub-App-Anmeldung mit PKCE, Auth-Broker und System-Keychain](GITHUB_INTEGRATION.md) bestehen. Kein Klartext-Credential-Speicher oder Token in URLs; benutzerdefinierte lokale Git-Konfiguration wird absichtlich abgelehnt. Einrichtung und Grenzen: [Authentifizierung](AUTHENTICATION.md).

Überschriftenpositionen werden gecacht; passives Scrollen bündelt Auswahl per RAF/Binärsuche ohne laufende DOM-Geometriemessung. Resize, Bilder, Fonts und Leseeinstellungen invalidieren den Cache. Sichere PNG/GIF/JPEG-Header reservieren bekannte Bildgrößen; späte/andere Bilder können weiterhin Layout verschieben. Highlighting startet verzögert und pausiert beim Scrollen: nur bekannte explizite Sprachen; unbekannte/unmarkierte oder über 16.000 Zeichen bzw. 400 Zeilen große Blöcke bleiben **bewusst Klartext**. Copy nutzt weiterhin den Originaltext.

Das sind gezielte Arbeitsreduktionen, kein Reader-/Editor-Redesign. [Performance](PERFORMANCE.md) dokumentiert Chromium-Messungen mit gemocktem IPC, keine native WebView-Messung und keinen garantierten FPS-Gewinn. Abschnittsranking verbessert die Suchqualität im kleinen Testkorpus, kostet aber Indexzeit, DB-Größe (~3,1×) und High-Fanout-Latenz; [Suche](SEARCH.md) enthält aktuelle Messungen statt allgemeiner Geschwindigkeitsversprechen. Embeddings/semantische Suche sind nicht implementiert und höchstens eine Zukunftsidee.

## Nächste Designentscheidungen

- Bild-Allowlisten vereinheitlichen, Inhaltsvalidierung und Decoder-Grenzen prüfen; die begrenzte workspacegebundene Auflösung ohne stille Netzwerkzugriffe beibehalten.
- Watcher mit Debounce, Rescan und Fehleranzeige evaluieren, um die wiederholten I/O-Kosten der derzeitigen Polling-Scans zu reduzieren.
- Workspace-Entfernung klar von Datei-/Checkout-Löschung unterscheiden; nie lokale Originale implizit löschen.
- Integrierte OAuth-Anmeldung/Abmeldung bleibt zurückgestellt; bestehende Systemauthentifizierung beibehalten, ohne Secrets in URLs, SQLite, Logs oder WebView-Speicher.
- Messbare Performance und Accessibility vor zusätzlichen Diagramm-, Plugin- oder Sync-Funktionen. Keine RAM-/Geschwindigkeitsversprechen ohne dokumentierte Baseline.

Details zu Schemas, Suche, Dateisystem-Races und Sicherheitsgrenzen: [Architektur](ARCHITECTURE.md). Geplante Arbeit: [Roadmap](ROADMAP.md).
