//! Schema der Datenbank als Migrationen (in dieser Reihenfolge, jede genau einmal; sqlx merkt sich
//! in `_sqlx_migrations`, welche schon gelaufen sind, und prüft ihre Prüfsumme – eine
//! angewandte Migration darf sich darum nie mehr ändern, Neues kommt als neue Migration dazu).
//! Version 14 (Canvas) gibt es in nrzX nicht.

/// (Version, Beschreibung, SQL)
pub const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "init", r###"-- nrzX – Grundschema. Zeitangaben als Unix-Sekunden, Wahrheitswerte als 0/1.
-- Weitere Migrationen je Bereich: 0002_agents.sql, 0003_workspace.sql.

CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL,
    lower_name TEXT NOT NULL UNIQUE,
    full_name TEXT NOT NULL DEFAULT '',
    email TEXT NOT NULL DEFAULT '',
    password_hash TEXT NOT NULL,
    is_admin INTEGER NOT NULL DEFAULT 0,
    is_active INTEGER NOT NULL DEFAULT 1,
    language TEXT NOT NULL DEFAULT '',
    theme TEXT NOT NULL DEFAULT 'dark',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_login_at INTEGER
);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    user_agent TEXT NOT NULL DEFAULT ''
);
CREATE INDEX sessions_user ON sessions (user_id);

-- Einstellungen der Instanz (Schlüssel → Wert)
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Verbindung zu nrzGit je Benutzer: Adresse und Zugriffstoken (wird nie ausgeliefert)
CREATE TABLE nrzgit_connections (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    token TEXT NOT NULL,
    -- Benutzername in nrzGit (aus /api/v1/me)
    username TEXT NOT NULL DEFAULT '',
    verified_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Agent-Konten: ein Abo oder Schlüssel eines Anbieters. Jedes Konto hat ein eigenes
-- Home-Verzeichnis (<data>/accounts/<id>/home), damit mehrere Abos desselben Anbieters
-- (z. B. zwei Claude-Code-Konten) getrennt bleiben.
CREATE TABLE agent_accounts (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- claude | codex | gemini | opencode | kilo | grok
    kind TEXT NOT NULL,
    label TEXT NOT NULL,
    -- login (Anmeldung im CLI, im Home gespeichert) | api_key | oauth_token
    auth_kind TEXT NOT NULL DEFAULT 'login',
    -- API-Schlüssel oder OAuth-Token (nie ausgeliefert, nur Hinweis)
    secret TEXT NOT NULL DEFAULT '',
    default_model TEXT NOT NULL DEFAULT '',
    -- unknown | ok | login_required | error
    status TEXT NOT NULL DEFAULT 'unknown',
    status_message TEXT NOT NULL DEFAULT '',
    is_default INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_used_at INTEGER
);

-- Projekte: nrzGit-Repositories, ausgecheckt unter <data>/workspaces/<id>/repo
CREATE TABLE projects (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    -- owner/name in nrzGit (leer bei lokalen Projekten)
    repo TEXT NOT NULL DEFAULT '',
    clone_url TEXT NOT NULL DEFAULT '',
    default_branch TEXT NOT NULL DEFAULT 'main',
    -- cloning | ready | error
    status TEXT NOT NULL DEFAULT 'cloning',
    status_message TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_activity_at INTEGER NOT NULL
);

-- Threads: Unterhaltung mit einem Agenten in einem Projekt (oder ohne Projekt).
-- Subagenten sind Threads mit parent_id.
CREATE TABLE threads (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    project_id INTEGER REFERENCES projects (id) ON DELETE CASCADE,
    parent_id INTEGER REFERENCES threads (id) ON DELETE CASCADE,
    title TEXT NOT NULL DEFAULT '',
    -- idle | working | waiting | done | error | settled
    status TEXT NOT NULL DEFAULT 'idle',
    agent_kind TEXT NOT NULL DEFAULT 'claude',
    account_id INTEGER REFERENCES agent_accounts (id) ON DELETE SET NULL,
    model TEXT NOT NULL DEFAULT '',
    effort TEXT NOT NULL DEFAULT '',
    -- Branch des Worktrees bzw. Hauptbranch
    branch TEXT NOT NULL DEFAULT '',
    -- Eigener Worktree (sonst direkt im Hauptcheckout des Projekts)
    use_worktree INTEGER NOT NULL DEFAULT 1,
    worktree_path TEXT NOT NULL DEFAULT '',
    -- Sitzungs-ID des Agenten zum Fortsetzen (claude --resume, codex resume …)
    provider_session_id TEXT NOT NULL DEFAULT '',
    unread INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_activity_at INTEGER NOT NULL
);
CREATE INDEX threads_user ON threads (user_id, last_activity_at);
CREATE INDEX threads_parent ON threads (parent_id);

-- Läufe: ein Aufruf des Agenten (eine Benutzernachricht → Antwort)
CREATE TABLE runs (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    agent_kind TEXT NOT NULL,
    account_id INTEGER REFERENCES agent_accounts (id) ON DELETE SET NULL,
    model TEXT NOT NULL DEFAULT '',
    -- running | done | error | stopped
    status TEXT NOT NULL DEFAULT 'running',
    exit_code INTEGER,
    error TEXT NOT NULL DEFAULT '',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX runs_thread ON runs (thread_id, started_at);

-- Nachrichten im Thread. role: user | assistant | tool | system | subagents | error
CREATE TABLE messages (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    run_id INTEGER REFERENCES runs (id) ON DELETE SET NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    data TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX messages_thread ON messages (thread_id, id);

-- Zugriffstokens für den nrzX-MCP-Server (je Lauf erzeugt, mit dem Lauf ungültig)
CREATE TABLE run_tokens (
    token_hash TEXT PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    thread_id INTEGER NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);
"###),
    (2, "agents", r###"-- nrzX – Bereich agents: Anhänge, Skills, Ergänzungen an Konten, Läufen und Nachrichten.

-- Umgebungsvariable für den Schlüssel eines Kontos (OpenCode/Kilo: Anbieter wählbar,
-- z. B. ANTHROPIC_API_KEY oder OPENROUTER_API_KEY; leer = Vorgabe der Agent-Art)
ALTER TABLE agent_accounts ADD COLUMN secret_env TEXT NOT NULL DEFAULT '';
-- Vorgabe für die Denk-Stufe (low | medium | high | max, leer = Vorgabe des CLIs)
ALTER TABLE agent_accounts ADD COLUMN default_effort TEXT NOT NULL DEFAULT '';
CREATE INDEX agent_accounts_user ON agent_accounts (user_id, kind);

-- Gerendertes Markdown (user/assistant), beim Schreiben erzeugt
ALTER TABLE messages ADD COLUMN html TEXT NOT NULL DEFAULT '';

-- Denk-Stufe und Sitzungs-ID des Laufs (Läufe: queued | running | done | error | stopped)
ALTER TABLE runs ADD COLUMN effort TEXT NOT NULL DEFAULT '';
ALTER TABLE runs ADD COLUMN session_id TEXT NOT NULL DEFAULT '';
CREATE INDEX runs_status ON runs (status);

-- Angehängte Dateien (Bilder im Composer), Datei unter <data>/attachments/<uuid>
CREATE TABLE attachments (
    uuid TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    size INTEGER NOT NULL DEFAULT 0,
    content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
    download_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE INDEX attachments_user ON attachments (user_id, created_at);

-- Skills: gespeicherte Prompt-Bausteine je Benutzer (im Composer mit `$`)
CREATE TABLE agent_skills (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    prompt TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX agent_skills_name ON agent_skills (user_id, name);
"###),
    (3, "workspace", r###"-- Bereich workspace: Pull Requests der Threads und Indizes für Projekte.

-- Pull Requests, die nrzX für einen Thread in nrzGit angelegt hat (neuester zuerst angezeigt)
CREATE TABLE workspace_pull_requests (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    -- owner/name in nrzGit
    repo TEXT NOT NULL,
    number INTEGER NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    base TEXT NOT NULL DEFAULT '',
    head TEXT NOT NULL DEFAULT '',
    url TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
CREATE INDEX workspace_pull_requests_thread ON workspace_pull_requests (thread_id, id);

-- Letzte Prüfung der nrzGit-Verbindung (Fehlermeldung, leer = in Ordnung)
ALTER TABLE nrzgit_connections ADD COLUMN last_error TEXT NOT NULL DEFAULT '';

CREATE INDEX projects_user ON projects (user_id, last_activity_at);
CREATE INDEX threads_project ON threads (project_id);
"###),
    (4, "nrzgit public url", r###"-- Adresse von nrzGit für Links im Browser. Die Verbindungsadresse (url) ist oft intern,
-- z. B. http://nrz-git_app_1:8380 auf Umbrel – die kennt der Browser nicht.
ALTER TABLE nrzgit_connections ADD COLUMN public_url TEXT NOT NULL DEFAULT '';
"###),
    (5, "thread sidebar", r###"-- Seitenleiste: Threads ausblenden und von Hand sortieren (Ziehen und Ablegen).
-- sort_key leer = nach letzter Aktivität; gesetzt = feste Stelle (gleiche Skala wie die Zeit).
ALTER TABLE threads ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
ALTER TABLE threads ADD COLUMN sort_key INTEGER;
"###),
    (6, "read only resume notes", r###"-- Nur-Lesen-Threads: Fragen zur Codebasis, der Agent darf nichts ändern (kein eigener Worktree,
-- keine Schreibwerkzeuge, keine Befehle ausser lesenden git-Befehlen, keine MCP-Server)
ALTER TABLE threads ADD COLUMN read_only INTEGER NOT NULL DEFAULT 0;

-- Automatisch weitermachen nach einem Rate-Limit: Zeitpunkt (Unix-Sekunden, NULL = nichts geplant)
-- und Zahl der Versuche hintereinander (für längere Pausen, wenn das Limit noch gilt)
ALTER TABLE threads ADD COLUMN resume_at INTEGER;
ALTER TABLE threads ADD COLUMN resume_attempts INTEGER NOT NULL DEFAULT 0;
CREATE INDEX threads_resume ON threads (resume_at) WHERE resume_at IS NOT NULL;

-- Notizblock: vorbereitete Prompts und Ideen, die man später sendet (je Benutzer, optional
-- einem Projekt zugeordnet)
CREATE TABLE prompt_notes (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    project_id INTEGER REFERENCES projects (id) ON DELETE SET NULL,
    text TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX prompt_notes_user ON prompt_notes (user_id, updated_at);
"###),
    (7, "questions freestyle", r###"-- FreeStyle: der Agent entscheidet alles selbst, ohne Rückfragen (je Thread)
ALTER TABLE threads ADD COLUMN freestyle INTEGER NOT NULL DEFAULT 0;
-- Nach einem Rate-Limit automatisch weitermachen (je Thread); Vorgabe je Benutzer
ALTER TABLE threads ADD COLUMN resume_auto INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN auto_resume INTEGER NOT NULL DEFAULT 1;
"###),
    (8, "btw", r###"-- /btw: Seiten-Chat zum Zwischenstand eines Threads (eigener, kurzer Lauf eines Modells, der
-- den Agenten nicht unterbricht und nicht in den Verlauf des Threads kommt)
CREATE TABLE btw_entries (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    question TEXT NOT NULL,
    answer TEXT NOT NULL DEFAULT '',
    -- pending | done | error
    status TEXT NOT NULL DEFAULT 'pending',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX btw_entries_thread ON btw_entries (thread_id, id);
"###),
    (9, "queue", r###"-- Nachrichten, die während eines Laufs gesendet werden, warten hier (JSON-Liste) und gehen nach
-- dem Lauf zusammen als nächste Nachricht an den Agenten
ALTER TABLE threads ADD COLUMN queued TEXT NOT NULL DEFAULT '[]';
"###),
    (10, "note attachments", r###"-- Bilder in Notizen: UUIDs der Anhänge (JSON-Liste, Tabelle attachments) in ihrer Reihenfolge
ALTER TABLE prompt_notes ADD COLUMN attachments TEXT NOT NULL DEFAULT '[]';
"###),
    (11, "run token details", r###"-- Genaue Statistik: Tokens aus dem Cache (gelesen/geschrieben) und Denk-Tokens je Lauf.
-- input_tokens enthält wie bisher alle Eingabe-Tokens (auch die aus dem Cache),
-- output_tokens alle Ausgabe-Tokens (auch die Denk-Tokens).
ALTER TABLE runs ADD COLUMN cache_read_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE runs ADD COLUMN cache_write_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE runs ADD COLUMN reasoning_tokens INTEGER NOT NULL DEFAULT 0;
"###),
    (12, "thread changes", r###"-- Zuletzt gezählte Änderungen je Thread (Zeilen hinzugefügt/entfernt) für die Anzeige +N −N
-- in der Seitenleiste und bei den Subagenten.
ALTER TABLE threads ADD COLUMN additions INTEGER NOT NULL DEFAULT 0;
ALTER TABLE threads ADD COLUMN deletions INTEGER NOT NULL DEFAULT 0;
"###),
    (13, "usage log", r###"-- Nutzung je Lauf, unabhängig von Threads und Projekten: Läufe werden mit ihrem Thread gelöscht,
-- ihr Verbrauch bleibt hier für die Nutzung (Limits, Kosten, Tokens) erhalten. Trigger halten die
-- Tabelle mit `runs` gleich; gelöschte Läufe verlieren nur den Bezug `run_id`. IDs von Threads und
-- Läufen können nach dem Löschen wieder vergeben werden – Sitzungen zählen darum über
-- `thread_key` („s<ID der ersten Zeile des Threads>“, AUTOINCREMENT vergibt keine ID doppelt).
-- Nebenläufe ohne eigenen Lauf (Seiten-Chat, Dieter) schreibt der Server direkt mit `run_id` NULL.
CREATE TABLE usage_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id INTEGER,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    thread_id INTEGER,
    thread_key TEXT,
    agent_kind TEXT NOT NULL,
    account_id INTEGER,
    model TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'running',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX usage_log_run ON usage_log (run_id);
CREATE INDEX usage_log_thread ON usage_log (thread_id);
CREATE INDEX usage_log_user ON usage_log (user_id, started_at);
CREATE INDEX usage_log_account ON usage_log (account_id, started_at);

INSERT INTO usage_log (run_id, user_id, thread_id, agent_kind, account_id, model, status, input_tokens, output_tokens,
                       cache_read_tokens, cache_write_tokens, reasoning_tokens, cost_usd, started_at, finished_at)
SELECT r.id, t.user_id, r.thread_id, r.agent_kind, r.account_id, r.model, r.status, r.input_tokens, r.output_tokens,
       r.cache_read_tokens, r.cache_write_tokens, r.reasoning_tokens, r.cost_usd, r.started_at, r.finished_at
FROM runs r JOIN threads t ON t.id = r.thread_id
ORDER BY r.id;
UPDATE usage_log SET thread_key = 's' || (SELECT MIN(u.id) FROM usage_log u WHERE u.thread_id = usage_log.thread_id);

-- Neuer Lauf: Schlüssel des Threads übernehmen (noch vorhandene Läufe), sonst einen neuen vergeben
CREATE TRIGGER usage_log_insert AFTER INSERT ON runs
BEGIN
    INSERT INTO usage_log (run_id, user_id, thread_id, thread_key, agent_kind, account_id, model, status, input_tokens,
                           output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, cost_usd, started_at, finished_at)
    SELECT NEW.id, t.user_id, NEW.thread_id,
           (SELECT u.thread_key FROM usage_log u WHERE u.thread_id = NEW.thread_id AND u.run_id IS NOT NULL LIMIT 1),
           NEW.agent_kind, NEW.account_id, NEW.model, NEW.status, NEW.input_tokens, NEW.output_tokens,
           NEW.cache_read_tokens, NEW.cache_write_tokens, NEW.reasoning_tokens, NEW.cost_usd, NEW.started_at, NEW.finished_at
    FROM threads t WHERE t.id = NEW.thread_id;
    UPDATE usage_log SET thread_key = 's' || id WHERE id = last_insert_rowid() AND thread_key IS NULL;
END;

CREATE TRIGGER usage_log_update AFTER UPDATE ON runs
BEGIN
    UPDATE usage_log SET
        agent_kind = NEW.agent_kind,
        account_id = NEW.account_id,
        model = NEW.model,
        status = NEW.status,
        input_tokens = NEW.input_tokens,
        output_tokens = NEW.output_tokens,
        cache_read_tokens = NEW.cache_read_tokens,
        cache_write_tokens = NEW.cache_write_tokens,
        reasoning_tokens = NEW.reasoning_tokens,
        cost_usd = NEW.cost_usd,
        started_at = NEW.started_at,
        finished_at = NEW.finished_at
    WHERE run_id = NEW.id;
END;

CREATE TRIGGER usage_log_delete AFTER DELETE ON runs
BEGIN
    UPDATE usage_log SET run_id = NULL WHERE run_id = OLD.id;
END;
"###),
    (15, "avatar", r###"-- Eigenes Profilbild: Datei unter <data>/avatars/<user_id>; avatar_at = Zeitpunkt (Version der
-- Adresse `…/avatar?v=`), 0 = kein eigenes Bild (Identicon).
ALTER TABLE users ADD COLUMN avatar_at INTEGER NOT NULL DEFAULT 0;
"###),
    (16, "forge kind", r###"-- Art der verbundenen Git-Plattform: 'nrzgit' (Vorgabe) oder 'lugit' (luGit: gleiche API für
-- Repositories, Pull Requests und Git über HTTP, aber ohne MCP-Server und mit eigenen Routen zum
-- Anlegen von Repositories). Wird beim Verbinden und Prüfen erkannt.
ALTER TABLE nrzgit_connections ADD COLUMN kind TEXT NOT NULL DEFAULT 'nrzgit';
"###),
    (17, "two forges", r###"-- nrzGit und luGit gleichzeitig: je Benutzer höchstens eine Verbindung je Plattform (bisher genau
-- eine). `has_mcp`: hat die Plattform einen MCP-Server (nrzGit immer, luGit ab 1.1.0 – wird beim
-- Verbinden und Prüfen erkannt). Projekte merken sich, von welcher Plattform sie stammen.
ALTER TABLE projects ADD COLUMN forge TEXT NOT NULL DEFAULT '';
UPDATE projects
SET forge = COALESCE((SELECT c.kind FROM nrzgit_connections c WHERE c.user_id = projects.user_id), 'nrzgit')
WHERE repo <> '';

CREATE TABLE nrzgit_connections_new (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 'nrzgit' oder 'lugit'
    kind TEXT NOT NULL DEFAULT 'nrzgit',
    url TEXT NOT NULL,
    token TEXT NOT NULL,
    username TEXT NOT NULL DEFAULT '',
    verified_at INTEGER,
    last_error TEXT NOT NULL DEFAULT '',
    public_url TEXT NOT NULL DEFAULT '',
    has_mcp INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, kind)
);
INSERT INTO nrzgit_connections_new (user_id, kind, url, token, username, verified_at, last_error, public_url, has_mcp, created_at, updated_at)
SELECT user_id, kind, url, token, username, verified_at, last_error, public_url, CASE WHEN kind = 'lugit' THEN 0 ELSE 1 END, created_at, updated_at
FROM nrzgit_connections;
DROP TABLE nrzgit_connections;
ALTER TABLE nrzgit_connections_new RENAME TO nrzgit_connections;
"###),
    (18, "bots", r###"-- Bot-Modus: dauerhafte KI-Teamkollegen mit eigenem Gedächtnis, eigenem Arbeitsordner
-- (<data>/bots/<id>) und Routinen. Ein Gespräch mit einem Bot ist ein Thread mit bot_id;
-- thread_id zeigt auf das aktuelle Gespräch.
CREATE TABLE bots (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    emoji TEXT NOT NULL DEFAULT '',
    -- Kurzbeschreibung unter dem Namen, z. B. „Recherche“
    role TEXT NOT NULL DEFAULT '',
    instructions TEXT NOT NULL DEFAULT '',
    -- Gedächtnis (Markdown), schreibt der Bot selbst über nrzx remember/rewrite_memory
    memory TEXT NOT NULL DEFAULT '',
    agent_kind TEXT NOT NULL DEFAULT 'claude',
    account_id INTEGER REFERENCES agent_accounts (id) ON DELETE SET NULL,
    model TEXT NOT NULL DEFAULT '',
    effort TEXT NOT NULL DEFAULT '',
    thread_id INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX bots_user ON bots (user_id);

ALTER TABLE threads ADD COLUMN bot_id INTEGER;
CREATE INDEX threads_bot ON threads (bot_id);

-- Routinen: zu einer Uhrzeit an bestimmten Wochentagen einen Auftrag ins aktuelle Gespräch
-- schicken. time_min = Minuten nach Mitternacht in der Ortszeit des Benutzers,
-- tz_offset = Minuten östlich von UTC beim Speichern, days = Bitmaske Mo=1 … So=64.
CREATE TABLE bot_routines (
    id INTEGER PRIMARY KEY,
    bot_id INTEGER NOT NULL REFERENCES bots (id) ON DELETE CASCADE,
    title TEXT NOT NULL DEFAULT '',
    prompt TEXT NOT NULL,
    time_min INTEGER NOT NULL DEFAULT 480,
    days INTEGER NOT NULL DEFAULT 127,
    tz_offset INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_run_at INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE INDEX bot_routines_bot ON bot_routines (bot_id);
"###),
    (19, "team", r###"-- Team: mehrere Leute (jeder mit eigenem nrzX) arbeiten am selben Repository, ihre Agenten
-- sprechen miteinander. Der gemeinsame Stand liegt als team.json in refs/nrzx/team des
-- Repositorys; hier steht je Projekt die letzte bekannte Fassung und was noch zu senden ist.
CREATE TABLE team_projects (
    project_id INTEGER PRIMARY KEY REFERENCES projects (id) ON DELETE CASCADE,
    enabled INTEGER NOT NULL DEFAULT 0,
    -- Agenten antworten selbstständig auf Nachrichten, die direkt an sie gehen
    auto_reply INTEGER NOT NULL DEFAULT 0,
    -- Commit der Referenz beim letzten Abgleich (leer = es gab sie nicht)
    remote_sha TEXT NOT NULL DEFAULT '',
    -- Letzter zusammengeführter Stand (TeamDoc als JSON)
    doc TEXT NOT NULL DEFAULT '{}',
    -- Wann die eigenen Agenten zuletzt gemeldet wurden
    presence_at INTEGER NOT NULL DEFAULT 0,
    synced_at INTEGER NOT NULL DEFAULT 0,
    error TEXT NOT NULL DEFAULT '',
    -- Bis dahin häufig abgleichen (Oberfläche offen, ein Agent wartet auf Antwort)
    hot_until INTEGER NOT NULL DEFAULT 0
);

-- Eigene Nachrichten, die noch nicht im Repository stehen (TeamMessage als JSON)
CREATE TABLE team_outbox (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES team_projects (project_id) ON DELETE CASCADE,
    message TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX team_outbox_project ON team_outbox (project_id);

-- Was ein Thread (Agent) schon bekommen hat; thread_id 0 = der Mensch (Benachrichtigung gezeigt)
CREATE TABLE team_seen (
    project_id INTEGER NOT NULL REFERENCES team_projects (project_id) ON DELETE CASCADE,
    thread_id INTEGER NOT NULL,
    message_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, thread_id, message_id)
);
"###),
    (20, "bot mcp", r###"-- Eigene MCP-Server je Bot (JSON-Liste von BotMcp): nur dieser Bot nutzt sie
ALTER TABLE bots ADD COLUMN mcp TEXT NOT NULL DEFAULT '[]';
"###),
    (21, "linked logins", r###"-- Aus nrzCode übernommene Konten: Home des Kontos in nrzCode. Die Anmeldedateien beider Seiten
-- werden abgeglichen (die neuere gewinnt) – erneuert eine App ihr Token, bekommt es die andere.
ALTER TABLE agent_accounts ADD COLUMN linked_home TEXT NOT NULL DEFAULT '';
"###),
    (22, "deep research", r###"-- Deep Research: der Thread recherchiert im Netz und schreibt ein Research Paper (paper.md in
-- seinem Ordner research/<id>)
ALTER TABLE threads ADD COLUMN research INTEGER NOT NULL DEFAULT 0;
-- Gespräch über eine Recherche: sieht deren Paper und arbeitet in deren Ordner
ALTER TABLE threads ADD COLUMN discuss_of INTEGER REFERENCES threads (id) ON DELETE CASCADE;
"###),
];
