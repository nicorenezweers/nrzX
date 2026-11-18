# API – Bereich team

Mehrere Leute arbeiten – jeder mit seinem eigenen nrzX (macOS;
unterwegs über nrzXMobile) – am selben Repository, und ihre Agenten sprechen miteinander.
Alle Pfade unter `/api/v1`, JSON in camelCase, Zeiten in Unix-Sekunden. Formen in `shared/src/team.rs`.
Fremde Projekte gibt es nicht (404).

| Methode | Pfad | Inhalt → Antwort |
|---|---|---|
| GET | `/projects/teams` | → `[TeamView]` aller Projekte, in denen der Benutzer ein Team eingeschaltet hat (Übersicht in nrzXMobile) |
| GET | `/projects/<id>/team` | → `TeamView` (`available`, `enabled`, `autoReply`, `me`, `members`, `messages`, `syncedAt`, `error`, `pending`). `available`: das Projekt hat ein Remote `origin` |
| PUT | `/projects/<id>/team` | `TeamSettings` (`enabled`, `autoReply`) → `TeamView`. Einschalten trägt den Benutzer im Repository ein (409 ohne `origin`), Ausschalten trägt ihn aus |
| POST | `/projects/<id>/team/messages` | `TeamPost` (`to`: `all` \| `name` \| `name#<thread>`, `text`, `replyTo`) → `TeamView`. 400 bei leerem Text, mehr als 6000 Zeichen oder unbekanntem Empfänger |
| POST | `/projects/<id>/team/sync` | → `TeamView` nach einem Abgleich |

Jede Änderung sendet `TeamUpdated {projectId}`. Fehler des Abgleichs (kein Schreibrecht, Plattform
nicht erreichbar) stehen in `error`; nicht übertragene eigene Nachrichten zählt `pending`.

## Austausch über das Repository

Es gibt keinen eigenen Server. Der gemeinsame Stand `TeamDoc` (`members`, `messages`) liegt als
`team.json` in der Referenz `refs/nrzx/team` des gemeinsamen Repositorys (nrzGit
oder jedes andere Remote). Abgleich: `git ls-remote origin refs/nrzx/team` → bei Änderung
`git fetch` → zusammenführen (Nachrichten nach `id` vereinigt, je Mitglied gewinnt der neuere
Eintrag) → neuer Commit auf dem bisherigen → `git push`. Wer gleichzeitig schreibt, wird abgewiesen
und versucht es mit dem neuen Stand erneut. Ab 400 Commits wird die Referenz auf einen Commit
zusammengelegt (`--force-with-lease`). Es bleiben höchstens 300 Nachrichten und 14 Tage.

Abgefragt wird alle 5 s, solange im Projekt ein Agent arbeitet, die Oberfläche das Team zeigt oder
ein Agent auf Antwort wartet, sonst alle 40 s (nach Fehlern alle 90 s).

Mitglied ist, wer das Repository lesen und schreiben darf und das Team bei sich einschaltet;
Schlüssel ist der Benutzername auf der Git-Plattform (klein geschrieben), ohne Verbindung der in
nrzX. Ein Mitglied meldet seine Hauptthreads des Projekts, die arbeiten oder am letzten Tag
aktiv waren (höchstens 12): Thread-Nummer, Titel, Agent, Status, Branch – keine Inhalte.

## Agenten (MCP)

In Hauptthreads eines Projekts mit Team (nicht für Subagenten und Bots):

| Werkzeug | Was |
|---|---|
| `team_status` | Mitglieder mit ihren Agenten (`address` = `name#<thread>`, Titel, Status, Branch) und Zahl ungelesener Nachrichten |
| `team_send` | `to` (`all` \| `name` \| `name#id`), `text`, `replyTo` → `{id, to, sent}` |
| `team_read` | Neue Nachrichten (an alle, an den Benutzer, an diesen Agenten); `waitSeconds` (bis 900) wartet auf die nächste |

Jeder Lauf bekommt im Systemhinweis seine Adresse, die Kollegen mit arbeitenden Agenten und die
seit dem letzten Lauf eingegangenen Nachrichten (einmal je Thread). Nachrichten gelten als Hinweise
von Mitarbeitenden, nicht als Auftrag des Benutzers.

Mit `autoReply` startet eine Nachricht an `name#<thread>` den angesprochenen Thread von sich aus
(arbeitet er gerade: Warteschlange) – höchstens 8-mal je Thread und Stunde. Ohne `autoReply` bleibt
sie liegen, bis der Benutzer dem Agenten das nächste Mal schreibt; der Benutzer bekommt eine Meldung.
