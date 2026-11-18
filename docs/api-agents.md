# API – Bereich agents

Alle Pfade unter `/api/v1` (ausser `/mcp` und `/attachments/<uuid>`), JSON in camelCase,
Zeiten in Unix-Sekunden. Formen in `shared/src/agents.rs` und `shared/src/common.rs`.
Jeder Thread, jedes Konto und jedes Terminal gehört einem Benutzer – fremde gibt es nicht (404).

## Agent-Arten

| Methode | Pfad | Antwort |
|---|---|---|
| GET | `/agents` | `AgentInfo[]` – Programm, npm-Paket, Version, Modelle, Stufen, Anmeldearten, Login-Befehl, Konten |
| GET | `/agents/<kind>/models?account=` | `ModelList` – Codex (`codex debug models`), OpenCode/Kilo (`… models`) vom CLI, sonst Vorschläge |

## Werkzeuge

| Methode | Pfad | |
|---|---|---|
| GET | `/tools` | `ToolInfo[]` (sechs CLIs und `docker`) |
| GET | `/tools/latest` | `ToolLatest[]` (npm bzw. download.docker.com) |
| POST | `/tools/<key>/install` | nur Admin; installiert/aktualisiert im Hintergrund, `ToolInfo` |
| GET | `/tools/<key>/stream` | SSE: je Zeile ein Ereignis, am Ende `event: done` mit `ok`/`error` |

Programme: `NRZX_BIN_<ART>` → `<data>/tools/npm/bin` → `<data>/tools/bin` → `PATH`.

## Konten

| Methode | Pfad | |
|---|---|---|
| GET | `/accounts?kind=` | `AgentAccount[]` (Schlüssel nur als Hinweis `sk-…cdef`) |
| POST | `/accounts` | `NewAccount` → `AgentAccount` (Home wird angelegt, Status geprüft; erstes Konto je Art = Standard) |
| PATCH | `/accounts/<id>` | `AccountUpdate` |
| DELETE | `/accounts/<id>` | löscht Konto und Home (409, solange ein Lauf es nutzt) |
| POST | `/accounts/<id>/check` | Status prüfen (lokal, ohne Kosten) |
| POST | `/accounts/<id>/login` | `LoginSession`: Terminal im Konto mit dem Login-Befehl |

## KI-Ketten

| Methode | Pfad | |
|---|---|---|
| GET | `/chains` | `ChainSettings`: `chains` und – nur lesen – `blocked` (Konten am Limit, mit `until`) |
| PUT | `/chains` | `ChainSettings` (es zählen nur `chains`) → gespeichert; 400 bei fremden Konten oder doppelten Stufen |

Eine Kette ist eine Reihenfolge von Konten (z. B. Claude Abo 1 → Claude Abo 2 → Codex), je Stufe
optional ein Modell (leer = bisheriges Modell beim gleichen Agenten, sonst Standard des Kontos).
Bricht ein Lauf mit einem Rate-Limit ab, wechselt der Thread sofort auf die nächste Stufe, die nicht
selbst am Limit ist, und macht mit dem bisherigen Verlauf weiter (neue Sitzung – Sitzungen gehören zu
CLI und Konto). Im Verlauf steht eine Systemnachricht mit `data.kind = "chain_switch"` (Inhalt: Name
der Kette, `data.detail`: Agent · Konto). Vor jedem Lauf wird geprüft, ob das Konto bekanntermassen
am Limit ist; dann wird gleich gewechselt. „Am Limit“: zuletzt mit Rate-Limit abgebrochen (bis zum
Zurücksetzen bzw. eine Stunde) oder laut Anbieter ein Limit bei 99 % oder mehr (Claude, Codex). Ohne
passende Kette wartet der Thread wie bisher auf das Zurücksetzen.

## Threads

| Methode | Pfad | |
|---|---|---|
| GET | `/threads?project=&status=a,b&parent=none\|<id>&q=&limit=` | `ThreadSummary[]` |
| POST | `/threads` | `NewThread` → `ThreadSummary` (legt Thread an, startet ersten Lauf; ohne `prompt` mit `tasks` nur Subagenten) |
| GET | `/threads/<id>` | `ThreadDetail` (Eltern, Subagenten, Läufe, Arbeitsordner, Sitzungs-ID) |
| PATCH | `/threads/<id>` | `ThreadPatch`: Titel, Anheften, Ausblenden (`hidden`), Agent/Konto/Modell/Stufe (Wechsel von Agent oder Konto verwirft die CLI-Sitzung) |
| POST | `/threads/order` | `ThreadOrder { ids }` → `ThreadSummary[]` – feste Reihenfolge in der Seitenleiste (oben zuerst, setzt `sortKey`) |
| DELETE | `/threads/<id>` | stoppt Läufe (auch Subagenten) und löscht – Worktree räumt `workspace` auf |
| GET | `/threads/<id>/messages?after=&limit=` | `Message[]` |
| POST | `/threads/<id>/messages` | `PostMessage` – setzt das Gespräch fort (409, solange der Agent arbeitet) |
| POST | `/threads/<id>/stop` | Lauf und Subagenten stoppen |
| POST | `/threads/<id>/status` | `StatusAsk` (Sprache, optional Modell) → `StatusReport`: Stand als kurze Liste und geschätzte Restdauer (`estimate`) aus einem kurzen Nebenlauf über den verdichteten Verlauf, dazu Laufzeit und Todo-Fortschritt als Fakten; unterbricht nicht, speichert nichts (`/status` im Composer) |
| POST | `/threads/<id>/read` | gelesen (done → idle) |
| POST | `/threads/<id>/settle` / `/reopen` | ablegen / zurückholen |
| POST | `/threads/<id>/clear` | Kontext zurücksetzen (nächster Lauf ohne Sitzung und Verlauf) |
| POST | `/threads/<id>/subagents` | `NewSubagents` → `ThreadSummary[]` (nicht bei Nur-Lesen-Threads) |
| POST | `/threads/<id>/messages/<mid>/retract` | Benutzernachricht zurückziehen → `RetractedMessage` (Text, Anhänge): stoppt den Agenten, entfernt die Nachricht und alles danach, nächste Sitzung beginnt neu; Läufe bleiben für die Nutzung |
| POST / DELETE | `/threads/<id>/resume` | nach einem Rate-Limit jetzt weitermachen / geplantes Weitermachen abbrechen |
| GET | `/threads/<id>/files?q=` | `FileEntry[]` (für `@`) |
| GET | `/agents/files?project=&q=` | `FileEntry[]` aus dem Hauptcheckout (neuer Thread) |

Werkzeug-Schritte (`role = tool`, Daten `ToolCall`): Bei Bearbeitungen (`kind` `edit`/`write`) liest der
Server die Datei vor und nach dem Werkzeug und setzt `diffs` auf die geänderten Abschnitte – je Abschnitt
`before`/`after`, `line`/`oldLine` (ab 1), bis zu drei Zeilen `contextBefore`/`contextAfter` und `adds`/`dels`
(`src/agents/edits.rs`, Diff in `shared/src/diff.rs`). Der Chat zeigt sie als Code-Block rot/grün.

Live: `ThreadUpdated` bei jeder Statusänderung (auch des Elternthreads), `Message` für neue und
gestreamte Nachrichten (gleiche ID ersetzt), `MessagesRemoved` (zurückgezogen), `ThreadDeleted`,
`PreviewOpened`, `Notice`.

**Nur lesen** (`NewThread.readOnly`): kein eigener Worktree, keine MCP-Server, keine Subagenten.
Claude Code ohne `bypassPermissions`, nur Lese-/Suchwerkzeuge und lesende git-Befehle; Codex mit
`sandbox_mode="read-only"`; Gemini ohne `--yolo` (nur lesende Werkzeuge); OpenCode/Kilo mit
`permission.edit/bash = deny`; Grok nur über den Systemhinweis.

**Rate-Limit**: Scheitert ein Lauf an einem Rate-/Nutzungslimit, setzt nrzX `resumeAt` (aus der
Meldung, den Limits des Kontos oder 5/10/20/40/60 Minuten) und startet dann einen Lauf mit „continue“
(`UserMessageData.autoResume`). Eine eigene Nachricht oder Stopp hebt das auf.

Status: `working` während des Laufs (auch in der Warteschlange), `waiting` solange der Agent auf
Subagenten wartet, danach `done` (ungelesen) bzw. `error`; gestoppt → `idle`.

## Skills, Anhänge

| Methode | Pfad | |
|---|---|---|
| GET/POST | `/agents/skills` | `Skill[]` / `NewSkill` → `Skill` |
| PATCH/DELETE | `/agents/skills/<id>` | |
| POST | `/attachments` | multipart, Feld `file` → `Attachment` (bzw. Liste) |
| GET | `/attachments/<uuid>` | nur für den Besitzer (ausserhalb `/api/v1`) |

## Nutzung, Notizblock

| Methode | Pfad | |
|---|---|---|
| GET | `/usage?refresh=&tz=` | `UsageOverview`: je Konto Läufe/Tokens/Kosten für 5 h, heute (Zeitzone `tz` wie `Date.getTimezoneOffset()`), 7 und 30 Tage und – Claude-Abo (OAuth-Usage-API), Codex (Sitzungsprotokolle) – die Limits mit Zeitpunkt des Zurücksetzens (2 Minuten zwischengespeichert; antwortet der Anbieter nicht, bleibt der letzte Stand bis 6 Stunden mit `limitsError`). Gezählt wird aus `usage_log`: auch laufende Läufe (Tokens alle 5 s), Nebenläufe (Seiten-Chat, Dieter) und gelöschte Threads |
| GET/POST | `/notes` | `PromptNote[]` / `NoteInput` → `PromptNote` (vorbereitete Prompts, Ideen) |
| PATCH/DELETE | `/notes/<id>` | |

## Terminals

| Methode | Pfad | |
|---|---|---|
| POST | `/terminals` | `TerminalOpen` {threadId \| projectId \| accountId, command?, cols?, rows?} → `TerminalInfo` |
| GET | `/terminals`, `/terminals/<id>` | |
| GET | `/terminals/<id>/stream` | SSE: Daten = Base64-Stücke (zuerst der Puffer), am Ende `event: exit` mit Code |
| POST | `/terminals/<id>/input` | `TerminalInput` {data} |
| POST | `/terminals/<id>/resize` | `TerminalSize` {cols, rows} |
| DELETE | `/terminals/<id>` | |
| POST | `/threads/<id>/cli` | `TerminalSize` {cols, rows} → `TerminalInfo` – das CLI des Agenten selbst (natives CLI statt des Chats) im Arbeitsordner des Threads, mit dessen Konto und Modell, setzt die Sitzung fort (`--resume`, `codex resume`, `-s` …; Grok: neue Sitzung); ohne Rückfragen wie im Chat, bei „Nur lesen“ fragt das CLI. Ein laufendes CLI des Threads kommt zurück (es läuft weiter, während die Oberfläche den Chat zeigt); höchstens 4 je Benutzer, ältere werden geschlossen; `409`, solange ein Lauf arbeitet. Startet ein Lauf, schliesst der Server das CLI mit Exit-Code `-2` (nur ein Programm schreibt in die Sitzung) – die Oberfläche startet es danach neu |

## MCP-Server `POST /mcp`

Streamable HTTP, JSON-RPC 2.0, Protokoll `2025-06-18`, `Authorization: Bearer <Lauf-Token>`
(je Lauf erzeugt, in `run_tokens`, gültig solange der Lauf läuft). Antworten als JSON, Benachrichtigungen
mit 202, `GET /mcp` → 405. Methoden: `initialize`, `ping`, `tools/list`, `tools/call`.

| Werkzeug | Argumente | Ergebnis |
|---|---|---|
| `spawn_subagents` | `{tasks: [{title, prompt, agent?, accountId?, model?, effort?}]}` (≤ 10) | IDs, Titel, Branches |
| `wait_for_subagents` | `{ids?, timeoutSeconds? (600, ≤ 3000)}` | `{allDone, subagents: [{id, title, agent, status, branch, lastAnswer, error, changes}]}` |
| `subagent_status` | `{ids?}` | wie oben, ohne Warten |
| `stop_subagent` | `{id}` | Bericht |
| `open_preview` | `{port, path?}` | sendet `PreviewOpened` |
| `notify_user` | `{text}` | sendet `Notice`, Systemnachricht im Thread |
| `team_status` | `{}` | Nur in Projekten mit Team ([api-team.md](api-team.md)): Mitglieder und ihre Agenten (`name#id`), ungelesene Nachrichten |
| `team_send` | `{to?, text, replyTo?}` | Nachricht an alle, eine Person (`name`) oder einen Agenten (`name#id`) |
| `team_read` | `{waitSeconds?}` | Neue Team-Nachrichten, optional darauf warten |

Jeder Lauf bekommt `nrzx` als `http://127.0.0.1:<Rocket-Port>/mcp` und – falls verbunden –
`nrzgit` (`<nrzgit>/mcp`) mit dem Bearer-Token des Benutzers (CLIs ohne Werkzeugsuche stattdessen
`nrzgit_find`/`nrzgit_call` über `nrzx`):

| CLI | Aufruf | MCP |
|---|---|---|
| Claude Code | `-p --output-format stream-json --verbose --include-partial-messages --permission-mode bypassPermissions [--model] [--effort] [--resume <id>] --append-system-prompt …`, Prompt über stdin (mit Bildern: `--input-format stream-json`) | `--mcp-config <lauf>/mcp.json` |
| Codex | `exec [resume <id>] --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox [-m] [-c model_reasoning_effort=…] [-i bild] -` | `-c mcp_servers.<name>.url/bearer_token_env_var/tool_timeout_sec` |
| Gemini CLI | `--output-format stream-json --yolo --skip-trust [-m] [--resume <id>] -p <prompt>` | `mcpServers` in `<home>/.gemini/settings.json` mit `$NRZX_MCP_URL`/`$NRZX_MCP_TOKEN` |
| OpenCode | `run --format json --auto [-m anbieter/modell] [--variant] [-s <id>] [-f datei] -- <prompt>` | `OPENCODE_CONFIG_CONTENT` (`mcp.<name>` remote) |
| Kilo Code | wie OpenCode (`kilo run …`) | `KILO_CONFIG_CONTENT` |
| Grok CLI | `-p <prompt> -d <ordner> [-m]` (keine Sitzungen: Verlauf im Prompt) | `.grok/settings.json` im Arbeitsordner (nur wenn frei, in `.git/info/exclude`) |
