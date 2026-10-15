---
tags: [homelab, home-assistant, automations, monitoring, systemd, cron, lidarr]
updated: 2026-10-15
status: aktiv
---

# Homelab – Zentrale Automatisierungsübersicht in Home Assistant

> **Stand 15.10.2026:** Die erste Ausbaustufe ist produktiv und im Home-Assistant-Dashboard geprüft. Es handelt sich um **lesendes Monitoring**, nicht um eine zentrale Steuerung. n8n wird vorerst **nicht** integriert; der nächste Arbeitsschwerpunkt ist wieder Lidarr/Mixarr.

## 1. Ziel und Architekturentscheidung

Home Assistant dient als zentrale Übersicht über geplante Aufgaben im Homelab. Die eigentliche Ausführung verbleibt bei den ursprünglichen Systemen (systemd, Cron, Home Assistant, später ggf. n8n). So werden bestehende produktive Jobs nicht an ein zusätzliches Steuerungssystem gekoppelt.

```text
                    Home Assistant OS (Raspberry Pi, 10.0.1.10)
                               |
                     REST-Sensoren (alle 300 s)
                       /                     \
    GET http://10.0.1.1:8765/api/automations  GET http://10.0.1.2:8765/api/automations
                tiny01                                  tiny02
    homelab-automation-exporter.service      homelab-automation-exporter.service
     systemd-System-/User-Timer + Cron       systemd-System-/User-Timer + Cron

    Home-Assistant-Automationen: direkt aus states.automation im Dashboard
```

- Der Exporter ist ein kleiner Python-HTTP-Dienst ohne zusätzliche Python-Abhängigkeiten, gestartet als unprivilegierter `atlas`-Benutzerdienst.
- Kein Docker-Container, keine Cloud-Verbindung, kein Traefik-Router, keine zusätzliche Download-/Steuerungs-API.
- Nur lesende Statusabfragen; HTTP `POST`, `PUT`, `PATCH` und `DELETE` werden mit `405` abgelehnt. Cron-Kommandos und Secrets werden nicht ausgegeben.
- **Sicherheit:** HTTP bindet nur an die jeweilige Host-LAN-IP. Der Exporter erlaubt die Quell-IP `10.0.1.10` (Home Assistant) und Loopback; sonst `403`. **Das ist eine Anwendungssperre, keine Firewall-Regel und keine Verschlüsselung oder Authentifizierung.** Nicht öffentlich exponieren; bei höherem Schutzbedarf Netzwerk-ACL/Firewall ergänzen.

## 2. Implementierter Stand

| Bereich | tiny01 | tiny02 |
|---|---|---|
| Debian-Host | `10.0.1.1` | `10.0.1.2` |
| Exporter-URL | `http://10.0.1.1:8765/api/automations` | `http://10.0.1.2:8765/api/automations` |
| Exporter-Systemd-Unit | `homelab-automation-exporter.service` (User) | dito |
| Timer laut Statusprüfung | 10 aktiv | 9 aktiv |
| Letzte gemeldete systemd-Dienstfehler | 0 | 0 |
| Cron-Tabellenzeilen | 6 | 6 |
| Dateien in periodischen Cron-Ordnern | 5 | 3 |

**Zusammen:** 19 aktive systemd-Timer sowie 20 Cron-Tabellenzeilen bzw. periodische Skriptdateien (nicht notwendigerweise 20 unabhängige Jobs). Die Zahlen sind eine Momentaufnahme vom **15.10.2026**, keine Live-Garantie. Die Dashboard-Screenshots bestätigten die vollständige Darstellung beider Hosts.

Besonders relevante eigene Timer:

- `tiny01`: `observability-backup.timer` (02:40), `pg-dump.timer` (02:50).
- `tiny02`: `pg-dump.timer` (02:50), `lidarr-neuerscheinungen.timer` (12:15, `Europe/Berlin`).
- Weitere Timer betreffen die Debian-Standardwartung (APT, Trim, Logrotate, etc.).

Home Assistant meldete außerdem **vier aktivierte und null deaktivierte Automationen**. Auf der HA-Ansicht werden Name, Aktivierung und letzte Auslösung angezeigt, z. B. `Abfall`, `Benachrichtigung bei Login Problemen`, `Lampe Küche an, wenn besetzt` und `Lampe Küche aus, wenn frei`. Die letzte Auslösung von `Abfall` lag laut Bildschirmansicht am 30.12.2025; bei Bedarf fachlich prüfen, nicht pauschal als Fehler werten.

## 3. Laufzeitinstallation auf beiden Debian-Hosts

Jeweils unter dem unprivilegierten Benutzer `atlas`:

```text
/home/atlas/.local/share/homelab-automation-exporter/exporter.py
/home/atlas/.config/systemd/user/homelab-automation-exporter.service
```

Die systemd-Unit startet (auf jedem Host mit dessen IP):

```text
/usr/bin/python3 /home/atlas/.local/share/homelab-automation-exporter/exporter.py serve --bind <HOST-IP> --port 8765 --ha-ip 10.0.1.10
```

Systemd ist mit `Restart=on-failure`, `NoNewPrivileges=yes`, `UMask=0077` und weiteren einfachen Härtungen konfiguriert. `loginctl show-user atlas -p Linger` liefert auf **beiden Hosts** `Linger=yes`, sodass der User-Dienst unabhängig von einer offenen SSH-Sitzung läuft. Die Installer meldeten jeweils `active=active enabled=enabled`.

**Betrieb/Diagnose vom Fish-Desktop:**

```fish
ssh tiny01 'systemctl --user status homelab-automation-exporter.service --no-pager'
ssh tiny02 'systemctl --user status homelab-automation-exporter.service --no-pager'
ssh tiny01 'journalctl --user -u homelab-automation-exporter.service -n 40 --no-pager'
ssh tiny02 'journalctl --user -u homelab-automation-exporter.service -n 40 --no-pager'
```

Der Exporter bietet `GET /healthz` und `GET /api/automations`. Letzterer liefert JSON mit u. a. `host`, `observed_at`, `summary`, `timers`, `cron`, `warnings`, `coverage_gaps`. Die Timer enthalten Name, Scope, Aktivierungsstatus, letzten bekannten Dienstlauf/Ergebnis und nächsten Lauf. Die Cron-Einträge enthalten Quelle, Zeitplan, Benutzer und ausdrücklich **keinen behaupteten Ausführungserfolg**.

### HTTP-Test

**Im Terminal von Home Assistant** (nicht im CachyOS-Desktop-Terminal) ausführen:

```sh
for ip in 10.0.1.1 10.0.1.2; do
  curl --max-time 5 -sS -o /dev/null -w "$ip: HTTP %{http_code}\n" \
    "http://$ip:8765/api/automations"
done
```

Erwartung: zweimal `HTTP 200`. Vom normalen Desktop-Client ist `HTTP 403` bei erfolgreicher Quell-IP-Prüfung **beabsichtigt**. Bei `000`/Timeout Netzwerk oder Dienst prüfen, bei `503` die Datenerfassung und das User-Journal.

## 4. Home-Assistant-Konfiguration

HAOS läuft auf `10.0.1.10` (Raspberry Pi). Die bereits vorhandene `renpho_observability`-Konfiguration bleibt unverändert.

In `/config/configuration.yaml` wurde die Package-Einbindung ergänzt:

```yaml
homeassistant:
  packages: !include_dir_named packages
```

Vor der Änderung wurde `/config/configuration.yaml.bak-20261015` angelegt. Das Package liegt unter:

```text
/config/packages/homelab_automationen.yaml
```

Darin sind zwei REST-Sensoren auf die obigen URLs definiert:

| Sensor | Zweck |
|---|---|
| `sensor.homelab_automationen_tiny01` | Anzahl aktiver systemd-Timer, Statusdetails als Attribute |
| `sensor.homelab_automationen_tiny02` | dito für tiny02 |

Einstellungen: `scan_interval: 300` Sekunden, `timeout: 25` Sekunden, `method: GET`. Die Attribute `host`, `summary`, `timers`, `cron`, `warnings`, `coverage_gaps` werden übernommen; `observed_at` wurde bewusst **nicht** als Sensorattribut aufgenommen, um unnötige Recorder-Änderungen zu reduzieren. Bei Erweiterung der Datenmenge den Einfluss großer Sensorattribute auf den Recorder kontrollieren.

Installation wurde mit `ha core check` geprüft; danach wurde Home Assistant einmal neu gestartet. Beide Sensoren sind in **Entwicklerwerkzeuge → Zustände** vorhanden und liefern `10` beziehungsweise `9`.

## 5. Dashboard

Angelegt als separates, über die Home-Assistant-Seitenleiste erreichbares Dashboard **„Homelab · Automatisierungen“** (Admin-Sicht empfohlen), über **Dashboard bearbeiten → ⋮ → Raw-Konfigurationseditor** verwaltet.

**Aktuelle, visuell bestätigte Fassung: v3 vom 15.10.2026**, mit vier Ansichten:

1. **Übersicht:** 19 aktive Timer, zwei Serverkacheln, Auffälligkeiten, wichtige Musik-/Backupaufgaben sowie Zusammenfassung der HA-Automationen.
2. **systemd:** Zwei Tabellen mit Timer, Status, letztem Dienstlauf, Ergebnis und nächstem Lauf.
3. **Cron:** Getrennte Inventartabellen pro Host; nur Zeitpläne, keine erfundenen Erfolgsangaben.
4. **Home Assistant:** Alle vier nativen Automationen mit Aktivierungsstatus und letztem `last_triggered`.

Die Karten verwenden Home-Assistant-Bordmittel (Markdown, Tile, Grid, Vertical Stack); keine HACS-Karten erforderlich. Die zunächst fehlerhaft dargestellten Markdown-Tabellen wurden in v2 durch das Zusammenfügen aller Tabellenzeilen ohne Jinja-Leerzeilen korrigiert. In v3 kam die HA-Ansicht dazu. **Alle vier Ansichten wurden anhand von Screenshots im Browser geprüft.**

**Hinweis zur Wiederherstellung:** Dashboard-Definition und Package sind aktuell als HA-Laufzeitkonfiguration angelegt. Vor einem Restore die aktuelle Raw-YAML aus Home Assistant exportieren und ebenso wie das Package und den Exporter in Gitea versionieren (siehe offene Arbeiten). Ein Dokument allein ersetzt diese Konfigurationsdateien nicht.

## 6. Abdeckungs- und Interpretationsgrenzen

- `systemd Result=success` bedeutet: Der systemd-Dienst wurde technisch erfolgreich beendet; **kein Nachweis**, dass ein Datenbankdump brauchbar ist oder ein Download fachlich erfolgreich war.
- Fehlender letzter Lauf (`-`) und fehlender nächster Lauf (z. B. monotone Timer) bedeuten nicht automatisch einen Fehler.
- Cron: Root-Benutzer-Crontab wird ohne `sudo` nicht inventarisiert. Cron-Läufe/Exit-Codes sind ohne Zusatzinstrumentierung unbekannt. Skripte unter `/etc/cron.daily` etc. können parallel zu systemd existieren und müssen nicht ausgeführt worden sein.
- Bislang keine Erfassung von n8n-Workflows, internen Docker-/App-Schedulern oder tatsächlichen fachlichen Erfolgskennzahlen. n8n-Datenbankzugriffe/API-Einbindung wurden **nicht** umgesetzt und sollen vorerst nicht weiterverfolgt werden.
- Home Assistant nutzt `states.automation`; `last_triggered` zeigt lediglich die letzte Auslösung, nicht Erfolg oder Fehler.
- Kein Starten/Stoppen über das Dashboard. Insbesondere keine API-Endpoints für Änderungen an Timern, Cron oder Musikdiensten.

## 7. Bezug zu Lidarr/Mixarr

Das Dashboard zeigt auf `tiny02` bereits `lidarr-neuerscheinungen.timer` als aktiven User-Timer an. Der bestehende kontrollierte **Mental-Cruelty-Pilot** ist weiterhin auf das Album **„Echoes of a Forlorn Land“** (Album-ID `1041`, Release laut Lidarr 23.10.2026) beschränkt. Der Timer prüft täglich um 12:15 Uhr; die erste mögliche Albumsuche ist am 23.10.2026. Die bisherigen Testläufe meldeten vor dem Release `WAIT`.

Die Kette **Lidarr → slskd → Import** wurde mit dem Album **„Zwielicht“** (21/21 erfasste Tracks, FLAC) erfolgreich getestet. Mixarr ist mit Lidarr und Jellyfin verbunden. Im Delay-Profil `slskd-auto` ist das Slskd-Protokoll für entsprechend getaggte Künstler freigegeben.

Der allgemeine Mehrkünstler-Scanner ist **noch nicht produktiv geschaltet**: Ein Read-only-Test meldete 132 Künstler, einen mit `slskd-auto` (Mental Cruelty), keine fälligen Kandidaten und keine Queue-Einträge. Zwei Fehler (API-Pfadprüfung und Verwechslung von Trackzählungen) wurden in Pilot-/Multi-Skripten behoben und erneut rein lesend getestet. Das bestehende Pilot-Systemd-Timer bleibt weiterhin für Album `1041` zuständig; keine zweite parallele Suchautomatik aktivieren.

Weiterführend: `docs/homelab-lidarr-musikabgleich.md` sowie `stacks/tiny02/arr/README.md` im Homelab-Repository.

## 8. Offene Arbeiten / Wiederanlauf

- [ ] **GitOps-Lücke schließen:** Aktuelle Laufzeitversionen von `exporter.py`, systemd-User-Unit, `/config/packages/homelab_automationen.yaml` und der Dashboard-Raw-YAML separat und ohne Secrets in Gitea sichern. Reproduzierbaren Deployment-/Restore-Pfad dokumentieren; keine produktiven Units ohne Review ersetzen.
- [ ] HA-Sensoren auf Staleness/Freshness überprüfen: ein grüner systemd-Dienststatus ist keine Garantie für aktuelle Daten. Recorder-Größe beobachten.
- [ ] Optional hostseitige Firewall-/ACL-Regel für Port `8765` zusätzlich zur Python-IP-Prüfung setzen; keine öffentliche Route anlegen.
- [ ] Anzeige kompakter machen (lokalisierte Datumsangaben, Priorisierung eigener Jobs, aussagekräftige Fehlerzustände).
- [ ] Fachliche Erfolgsnachweise für `pg-dump`, Observability-Backup und Lidarr ggf. ergänzen, bevor Benachrichtigungen ausgelöst werden.
- [ ] Cron nur bei Bedarf instrumentieren; Dopplungen mit systemd vermeiden.
- [ ] **n8n-Integration vorerst zurückstellen**, ebenso interne Container-Scheduler. Keine n8n-Datenbank-/API-Abfragen ohne neue Entscheidung.
- [ ] Musikprojekt fortsetzen: Mixarr-Empfehlungen und kontrolliertes Onboarding weiterer Künstler; danach Mehrkünstler-Suche sicher freigeben.

## 9. Änderungen und Prüfungen am 15.10.2026

- User Linger für `atlas` auf beiden Hosts aktiviert.
- Read-only-Exporter auf beiden Hosts installiert; `active=active enabled=enabled` und Home-Assistant-HTTP-Abrufe mit zweimal `200` bestätigt.
- Home-Assistant-Package aktiviert, `ha core check` erfolgreich, Sensoren nach Neustart sichtbar.
- Dashboard v1 erstellt, v2-Tabellen korrigiert, v3-Home-Assistant-Reiter ergänzt; Screenshots aller vier Reiter bestätigen Darstellung.
- Keine produktiven Timer, Cronjobs, Backups, Lidarr-Downloads oder n8n-Workflows durch die Dashboard-Einrichtung ausgelöst oder verändert.
