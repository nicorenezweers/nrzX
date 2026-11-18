# API – Bereich workspace

Alle Pfade unter `/api/v1`, JSON in camelCase, Zeiten als Unix-Sekunden. Formen in
`shared/src/workspace.rs` (und `AuthStatus`, `Me`, `ProjectSummary`, `Diff` aus `common.rs`).
Fehler: `{ "error": { "code", "message" } }`. Ohne Anmeldung 401; fremde Projekte und Threads 404.

## Anmeldung und Konto

| Methode | Pfad | Inhalt → Antwort |
|---|---|---|
| GET | `/auth/status` | → `AuthStatus` (ohne Anmeldung) |
| POST | `/auth/setup` | `SetupRequest` → `Me` – erstes Konto, wird Administrator, meldet an |
| POST | `/auth/register` | `RegisterRequest` → 201 `Me` – nur bei offener Registrierung |
| POST | `/auth/login` | `LoginRequest` (Name oder E-Mail) → `Me`, setzt das Sitzungs-Cookie |
| POST | `/auth/logout` | → 204 |
| GET | `/me` | → `Me` (mit `nrzgitConnected`, `nrzgitUrl`) |
| PUT | `/me/profile` | `ProfileUpdate` → `Me` |
| PUT | `/me/password` | `PasswordChange` → 204; andere Sitzungen enden, neues Cookie |
| PUT | `/me/appearance` | `Appearance` (`dark` \| `light` \| `system`, Sprache) → `Appearance` |
| GET | `/me/stats?tz=` | → `Stats` (Projekte, Threads, Läufe, Tokens, Kosten, je Agent; „heute“ in der Zeitzone `tz`, Verbrauch auch gelöschter Threads) |
| PUT | `/me/avatar` | `AvatarUpload` (`image`: Data-URL oder Base64; PNG, JPEG, WebP oder GIF, höchstens 2 MB – die Oberfläche schickt ein quadratisches PNG mit 256 px) → `Me` (`customAvatar`, `avatarUrl` mit `?v=<Version>`); abgelegt unter `<data>/avatars/<id>` |
| DELETE | `/me/avatar` | eigenes Profilbild entfernen → `Me` (wieder das Identicon) |
| GET | `/users/<name>/avatar?v=` | → eigenes Profilbild oder Identicon (SVG); mit passender Version `immutable`, sonst `no-cache` mit ETag |

## Verwaltung (Administratoren)

| Methode | Pfad | Inhalt → Antwort |
|---|---|---|
| GET | `/users` | → `[UserInfo]` |
| POST | `/users` | `CreateUser` → 201 `UserInfo` |
| PATCH | `/users/<id>` | `EditUser` (Administrator, gesperrt, Passwort, Name, E-Mail) → `UserInfo`; Sperren beendet Sitzungen und hält Agenten an |
| DELETE | `/users/<id>` | → 204; Projekte, Threads, Arbeitsordner weg (nicht das eigene Konto, nicht der letzte Administrator) |
| GET/PUT | `/settings/instance` | `InstanceSettings` – Registrierung, Vorgabe-Adresse von nrzGit; `maxParallelRuns`, `previewPort`, `dataDir`, `version`, `users` nur Anzeige |

## nrzGit

| Methode | Pfad | Inhalt → Antwort |
|---|---|---|
| GET | `/nrzgit` | → `NrzGitStatus` (Token nur als Hinweis, `tokenUrl` = `<url>/user/settings/mcp`) |
| GET | `/nrzgit/connections` | → `[NrzGitStatus]` – die Verbindung zu nrzGit als Liste |
| PUT | `/nrzgit` | `NrzGitConnect` → `NrzGitStatus`; prüft sofort `GET <url>/api/v1/me` mit `Authorization: token <t>` und speichert nur bei Erfolg |
| POST | `/nrzgit/verify` | → `NrzGitStatus` (erneut prüfen) |
| DELETE | `/nrzgit` | → 204 (Token löschen) |
| GET | `/nrzgit/repos?q=` | → `[NrzGitRepo]` aus `GET <url>/api/v1/me/repos` (eigene, Organisationen, Mitarbeit), mit `projectId`, wenn schon übernommen |
| GET | `/nrzgit/owners` | → `[NrzGitOwner]` (ich und meine Organisationen) |
| GET | `/nrzgit/candidates?url=` | → `[NrzGitCandidate]` (übliche Adressen von nrzGit, vom Server aus geprüft) |

nrzX verbindet sich nur mit nrzGit; `?kind=` ist für ältere Oberflächen noch erlaubt, aber nur `nrzgit`.

## Projekte

| Methode | Pfad | Inhalt → Antwort |
|---|---|---|
| GET | `/projects` | → `[ProjectSummary]` (zuletzt aktive zuerst) |
| GET | `/projects/<id>` | → `ProjectDetail` (Adresse in nrzGit, Ordner, Branch, letzte Commits) |
| POST | `/projects/import` | `ImportProject` (`owner/name`) → 201 `ProjectSummary` (`cloning`); klont im Hintergrund nach `workspaces/<id>/repo`, dann `ready`/`error` per `HubEvent::ProjectUpdated` |
| POST | `/projects/create` | `CreateRepoProject` → 201; legt das Repository in nrzGit an (`POST <url>/api/v1/repos`) und übernimmt es |
| POST | `/projects/local` | `CreateLocalProject` → 201; `git init` mit README und erstem Commit |
| PATCH | `/projects/<id>` | `EditProject` → `ProjectSummary` (nur der Name in nrzX) |
| POST | `/projects/<id>/refresh` | → `GitResult`; `fetch` + Vorspulen des sauberen Hauptcheckouts, nach Fehlern neu klonen |
| DELETE | `/projects/<id>` | → 204; hält alle Threads an (`agents::stop_thread`), löscht Threads und Ordner (nicht das Repository in nrzGit) |

Git-Anmeldung bei nrzGit immer über `http.<url>/.extraHeader: Authorization: Basic base64(name:token)`
in der Umgebung von `git` (`GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n`) – das Token
steht nie in der Remote-Adresse oder der Prozessliste.

## Git eines Threads

Pfade `/workspace/<thread>/…`. Der Arbeitsordner ist der eigene Worktree
(`workspaces/<projekt>/worktrees/<thread>`, Branch `nrz/<kurzname>-<id>`), der Hauptcheckout
oder ohne Projekt `scratch/<thread>`. „Änderungen“ = gegen den Abzweig vom Standard-Branch
(`origin/<standard>`, sonst lokal), inklusive unversionierter Dateien (über einen
vorübergehenden Index; der Index der Agenten bleibt unberührt).

| Methode | Pfad | Inhalt → Antwort |
|---|---|---|
| GET | `status` | → `WorkspaceStatus` (Branch, Upstream, ahead/behind, Änderungen, nicht committete Dateien, letzter Pull Request); legt den Worktree bei Bedarf an |
| GET | `diff?path=` | → `Diff` mit Syntaxfarben je Zeile (`html`) |
| POST | `commit` | `CommitRequest` (alle oder ausgewählte Dateien, optional Push) → `CommitResult` |
| POST | `push` | → `GitResult` (`push --set-upstream origin <branch>`) |
| POST | `pull` | → `GitResult` (fetch, vorspulen, sonst Merge; bei Konflikt abgebrochen) |
| GET | `branches` | → `BranchList` (lokal und nur auf origin, in anderem Worktree ausgecheckt) |
| POST | `branch` | `BranchRequest` → `WorkspaceStatus`; im Hauptcheckout gilt der Wechsel für alle Threads dort |
| POST | `worktree` | → `WorkspaceStatus` – eigenen Worktree einrichten („Setup Worktree“) |
| DELETE | `worktree` | → `WorkspaceStatus` – Worktree entfernen, Branch bleibt |
| POST | `pull-request` | `PullRequestRequest` → `PullRequestRef`; pusht und ruft `POST <url>/api/v1/repos/<o>/<r>/pulls` auf |
| GET | `files?path=` | → `[FileEntry]` (ein Ordner, ohne `.git`) |
| GET | `files/all` | → `[String]` (alle Dateien, für die Schnellsuche) |
| GET | `file?path=` | → `FileContent` (Text bis 2 MB; Binärdateien ohne Inhalt) |
| PUT | `file` | `FileWrite` → 204 |

Während ein Agent arbeitet (`working`/`waiting`), sind Branch-Wechsel, Pull und Worktree-Änderungen gesperrt (409).

## Vorschau

| Methode | Pfad | Inhalt → Antwort |
|---|---|---|
| GET | `/previews?threadId=` bzw. `?projectId=` | → `[PreviewPort]` – lauschende Ports (Linux `/proc/net/tcp{,6}`, macOS `lsof`), eigene und solche ohne bekannten Prozess; nie die anderer Benutzer, nie 9300/9301 bzw. die eigenen Ports |
| POST | `/previews/token` | `PreviewTokenRequest` → `PreviewToken` (`http://<host>:<preview_port>/__nrzx/open?token=…`, 2 Minuten gültig, einmal einlösbar); `threadId: 0` = ohne Thread (Browser-Fenster der Canvas, der Port muss trotzdem diesem Benutzer gehören) |

Der Proxy auf `preview_port` setzt beim Einlösen das Cookie `nrzx_preview` (Sitzung für genau
diesen Port, 12 Stunden) und leitet auf den gewünschten Pfad. Danach reicht er jede Anfrage an
`127.0.0.1:<port>` (bzw. `[::1]`) weiter – mit `Host`/`Origin` = `localhost:<port>`, ohne Cookies
`nrzx_*`, mit WebSocket-Upgrade. Weiterleitungen auf `localhost:<port>` werden relativ,
`X-Frame-Options` und `frame-ancestors` entfallen (Einbetten im Paneel), `Domain=` an Cookies wird
entfernt.

## Verträge für den Bereich agents

* `workspace::thread_workdir(state, thread)` – Arbeitsordner (siehe oben), legt Worktree/Ordner an
  und meldet einen neuen Branch mit `agents::publish_thread`.
* `workspace::agent_env(state, user)` – `GIT_CONFIG_*` (Anmeldung bei nrzGit), `GIT_AUTHOR_*`,
  `GIT_COMMITTER_*`, `GIT_TERMINAL_PROMPT=0`, `NRZGIT_URL`, `NRZGIT_USERNAME`, `NRZGIT_TOKEN`,
  `NRZGIT_REGISTRY` und `DOCKER_CONFIG` (`<data>/docker/<benutzer>/config.json` mit Anmeldung an der
  Registry unter dem Host von nrzGit).
* `workspace::after_run(state, thread)` – zählt Änderungen → `HubEvent::ChangesUpdated`.
* `workspace::nrzgit_access(state, user)` – nur bei geprüfter Verbindung.
