//! Bausteine, die beide Bereiche brauchen.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub status: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paged<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRef {
    pub id: i64,
    pub username: String,
    pub full_name: String,
    /// `/api/v1/users/<name>/avatar` (erzeugtes Identicon)
    pub avatar_url: String,
    pub is_org: bool,
}

impl UserRef {
    /// Anzeigename: voller Name, sonst Benutzername.
    pub fn display(&self) -> &str {
        if self.full_name.trim().is_empty() {
            &self.username
        } else {
            &self.full_name
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    pub name: String,
    pub email: String,
    pub when: i64,
    pub user: Option<UserRef>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub sha: String,
    pub short_sha: String,
    /// Erste Zeile der Nachricht
    pub summary: String,
    /// Ganze Nachricht
    pub message: String,
    pub author: Signature,
    pub committer: Signature,
    pub parents: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineKind {
    #[default]
    Context,
    Add,
    Del,
    /// `\ No newline at end of file`
    Meta,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Zeilennummer in der alten Datei (nicht bei hinzugefügten Zeilen)
    pub old_line: Option<i64>,
    /// Zeilennummer in der neuen Datei (nicht bei entfernten Zeilen)
    pub new_line: Option<i64>,
    /// Text ohne führendes `+`/`-`/` `
    pub content: String,
    /// Optional hervorgehobener Text als HTML (Klassen `hl-…`)
    pub html: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    /// `@@ -1,4 +1,6 @@ fn main()`
    pub header: String,
    pub old_start: i64,
    pub old_lines: i64,
    pub new_start: i64,
    pub new_lines: i64,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub old_path: String,
    pub new_path: String,
    /// "added" | "deleted" | "modified" | "renamed" | "copied"
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
    pub binary: bool,
    /// Zu gross zum Anzeigen – nur Zahlen, keine Zeilen
    pub too_large: bool,
    pub old_mode: String,
    pub new_mode: String,
    pub hunks: Vec<DiffHunk>,
}

impl DiffFile {
    /// Pfad zum Anzeigen (neuer Pfad, ausser bei gelöschten Dateien).
    pub fn path(&self) -> &str {
        if self.status == "deleted" {
            &self.old_path
        } else {
            &self.new_path
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub files: Vec<DiffFile>,
    pub additions: i64,
    pub deletions: i64,
    /// Es gab mehr Dateien als angezeigt werden
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownRequest {
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownResponse {
    pub html: String,
}

/// Antwort von `GET /api/v1/auth/status` (ohne Anmeldung).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    /// Noch kein Konto – das erste wird Administrator
    pub needs_setup: bool,
    pub registration_open: bool,
    pub version: String,
}

/// Der angemeldete Benutzer (`GET /api/v1/me`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub id: i64,
    pub username: String,
    pub full_name: String,
    pub email: String,
    pub avatar_url: String,
    /// Eigenes Profilbild hochgeladen (sonst Identicon)
    #[serde(default)]
    pub custom_avatar: bool,
    pub is_admin: bool,
    pub language: String,
    /// "dark" | "light" | "system"
    pub theme: String,
    /// Mit nrzGit verbunden (URL und Token hinterlegt und geprüft)
    pub nrzgit_connected: bool,
    /// Adresse der nrzGit-Instanz (leer, wenn nicht verbunden)
    pub nrzgit_url: String,
    /// Alle verbundenen Git-Plattformen (nrzX: nur nrzGit); `nrzgit_connected` und
    /// `nrzgit_url` beschreiben die erste davon
    #[serde(default)]
    pub forges: Vec<ForgeRef>,
}

/// Verbundene Git-Plattform eines Benutzers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeRef {
    /// immer "nrzgit"
    pub kind: String,
    /// Adresse für Links im Browser
    pub url: String,
}

// ---------------------------------------------------------------------------------- Agenten

/// Ein Coding-Agent (Kommandozeilenprogramm), den nrzX startet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    /// Claude Code (Anthropic), Programm `claude`
    #[default]
    Claude,
    /// Codex CLI (OpenAI), Programm `codex`
    Codex,
    /// Gemini CLI (Google), Programm `gemini`
    Gemini,
    /// OpenCode, Programm `opencode`
    Opencode,
    /// Kilo Code CLI, Programm `kilocode`
    Kilo,
    /// Grok CLI (xAI), Programm `grok`
    Grok,
}

impl AgentKind {
    pub const ALL: [AgentKind; 6] = [
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Gemini,
        AgentKind::Opencode,
        AgentKind::Kilo,
        AgentKind::Grok,
    ];

    /// Anzeigename, z. B. "Claude Code".
    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::Gemini => "Gemini CLI",
            AgentKind::Opencode => "OpenCode",
            AgentKind::Kilo => "Kilo Code",
            AgentKind::Grok => "Grok CLI",
        }
    }

    /// Schlüssel wie in JSON ("claude", "codex" …).
    pub fn key(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Gemini => "gemini",
            AgentKind::Opencode => "opencode",
            AgentKind::Kilo => "kilo",
            AgentKind::Grok => "grok",
        }
    }

    pub fn parse(s: &str) -> Option<AgentKind> {
        AgentKind::ALL.into_iter().find(|k| k.key() == s)
    }
}

/// Zustand eines Threads (wie in der Seitenleiste gruppiert).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadStatus {
    /// Noch nichts gestartet oder fertig gelesen
    #[default]
    Idle,
    /// Agent läuft
    Working,
    /// Wartet auf Subagenten oder eine Antwort des Benutzers
    Waiting,
    /// Fertig, noch nicht angesehen
    Done,
    /// Mit Fehler beendet
    Error,
    /// Abgelegt („Settled“)
    Settled,
}

impl ThreadStatus {
    pub fn key(self) -> &'static str {
        match self {
            ThreadStatus::Idle => "idle",
            ThreadStatus::Working => "working",
            ThreadStatus::Waiting => "waiting",
            ThreadStatus::Done => "done",
            ThreadStatus::Error => "error",
            ThreadStatus::Settled => "settled",
        }
    }

    pub fn parse(s: &str) -> ThreadStatus {
        match s {
            "working" => ThreadStatus::Working,
            "waiting" => ThreadStatus::Waiting,
            "done" => ThreadStatus::Done,
            "error" => ThreadStatus::Error,
            "settled" => ThreadStatus::Settled,
            _ => ThreadStatus::Idle,
        }
    }

    /// Läuft gerade etwas (Agent oder Subagenten)?
    pub fn is_active(self) -> bool {
        matches!(self, ThreadStatus::Working | ThreadStatus::Waiting)
    }
}

/// Projekt = ein nrzGit-Repository, das nrzX ausgecheckt hat.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: i64,
    pub name: String,
    /// `owner/name` in nrzGit (leer bei lokalen Projekten)
    pub repo: String,
    /// Plattform des Repositorys: "nrzgit" (leer bei lokalen Projekten)
    #[serde(default)]
    pub forge: String,
    pub default_branch: String,
    /// cloning | ready | error
    pub status: String,
    pub status_message: String,
    pub threads: i64,
    pub last_activity_at: i64,
}

/// Thread (Unterhaltung mit einem Agenten) für Listen und die Seitenleiste.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub id: i64,
    pub project_id: Option<i64>,
    pub project_name: Option<String>,
    /// Elternthread, wenn dies ein Subagent ist
    pub parent_id: Option<i64>,
    pub title: String,
    pub status: ThreadStatus,
    pub agent: AgentKind,
    pub account_id: Option<i64>,
    /// Name des Agent-Kontos, z. B. "Claude Max (privat)"
    pub account_label: String,
    pub model: String,
    /// "low" | "medium" | "high" | "xhigh" | "max" (leer = Vorgabe)
    pub effort: String,
    /// Branch des Worktrees (oder der Hauptbranch, ohne Worktree)
    pub branch: String,
    pub unread: bool,
    pub pinned: bool,
    /// Nur lesen: Fragen zur Codebasis, der Agent ändert nichts
    #[serde(default)]
    pub read_only: bool,
    /// FreeStyle: der Agent entscheidet alles selbst und stellt keine Rückfragen
    #[serde(default)]
    pub freestyle: bool,
    /// Nach einem Rate-Limit automatisch weitermachen, sobald es zurückgesetzt ist
    #[serde(default)]
    pub resume_auto: bool,
    /// Während des Laufs gesendete Nachrichten (gehen nach dem Lauf raus)
    #[serde(default)]
    pub queued: Vec<QueuedMessage>,
    /// Rate-Limit erreicht: der Agent macht um diese Zeit automatisch weiter (Unix-Sekunden)
    #[serde(default)]
    pub resume_at: Option<i64>,
    /// In der Seitenleiste ausgeblendet (Gruppe „Ausgeblendet“)
    #[serde(default)]
    pub hidden: bool,
    /// Feste Stelle nach Ziehen und Ablegen (sonst nach `last_activity_at` sortiert)
    #[serde(default)]
    pub sort_key: Option<i64>,
    /// Gespräch eines Bots (Bot-Modus) – solche Threads stehen nicht in der Code-Seitenleiste
    #[serde(default)]
    pub bot_id: Option<i64>,
    /// Laufende Subagenten
    pub children_running: i64,
    /// Beginn des aktuellen Laufs (für „Working 25s“)
    pub run_started_at: Option<i64>,
    /// Zuletzt gezählte Änderungen (Zeilen hinzugefügt/entfernt)
    #[serde(default)]
    pub additions: i64,
    #[serde(default)]
    pub deletions: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_activity_at: i64,
    /// Anfang der letzten Antwort des Agenten (eine Zeile, für die Aufgabenliste)
    #[serde(default)]
    pub snippet: String,
    /// Ausgang des letzten Laufs: "running" | "done" | "error" | "stopped" (leer = noch keiner) –
    /// färbt die Aufgabe in der Seitenleiste auch nach dem Lesen rot (Fehler/Abbruch); Grün für
    /// „fertig“ gilt nur bis zum Ansehen
    #[serde(default)]
    pub last_run: String,
    /// Eine Rückfrage des Agenten wartet auf Antwort (unbeantwortet, nicht abgelaufen, danach keine
    /// eigene Nachricht)
    #[serde(default)]
    pub asking: bool,
    /// Deep Research: der Thread recherchiert und schreibt ein Research Paper (eigener Bereich, nicht
    /// in der Aufgabenliste)
    #[serde(default)]
    pub research: bool,
    /// Gespräch über diese Recherche (sieht ihr Paper)
    #[serde(default)]
    pub discuss_of: Option<i64>,
}

/// Nachricht in der Warteschlange eines Threads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueuedMessage {
    pub id: String,
    pub text: String,
    /// UUIDs der Anhänge
    pub attachments: Vec<String>,
    pub created_at: i64,
    /// Zwischenruf: geht im nächsten passenden Moment raus (der Agent wird dafür unterbrochen)
    pub interrupt: bool,
}

/// Eine Nachricht im Thread. `role`: user | assistant | tool | system | subagents | error.
/// `data` enthält je nach Rolle Einzelheiten (Werkzeug-Name, Eingabe, Ergebnis, IDs der
/// Subagenten, Anhänge …) – Aufbau siehe `shared/src/agents.rs`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: i64,
    pub thread_id: i64,
    pub run_id: Option<i64>,
    pub role: String,
    /// Text (Markdown bei user/assistant)
    pub content: String,
    /// Gerendertes Markdown (assistant/user), sonst leer
    pub html: String,
    pub data: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Ereignisse an die Oberfläche (`GET /api/v1/events`, Server-Sent Events, je Benutzer).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HubEvent {
    /// Thread neu oder geändert (Status, Titel, Branch …)
    #[serde(rename_all = "camelCase")]
    ThreadUpdated { thread: ThreadSummary },
    #[serde(rename_all = "camelCase")]
    ThreadDeleted { id: i64 },
    /// Neue oder (beim Streamen) fortgeschriebene Nachricht – gleiche `id` ersetzt die alte
    #[serde(rename_all = "camelCase")]
    Message { message: Message },
    #[serde(rename_all = "camelCase")]
    ProjectUpdated { project: ProjectSummary },
    #[serde(rename_all = "camelCase")]
    ProjectDeleted { id: i64 },
    /// Ein Agent hat einen Server gestartet und möchte die Vorschau zeigen
    #[serde(rename_all = "camelCase")]
    PreviewOpened { thread_id: i64, port: u16, path: String },
    /// Änderungen im Arbeitsverzeichnis eines Threads (für „Changes +12 −3“)
    #[serde(rename_all = "camelCase")]
    ChangesUpdated { thread_id: i64, additions: i64, deletions: i64, files: i64 },
    /// Nachrichten ab `from_id` wurden entfernt (Nachricht zurückgezogen)
    #[serde(rename_all = "camelCase")]
    MessagesRemoved { thread_id: i64, from_id: i64 },
    /// Kurze Meldung (Toast)
    #[serde(rename_all = "camelCase")]
    Notice { text: String, error: bool },
    /// Team eines Projekts geändert (Mitglieder, Agenten, Nachrichten, Abgleich)
    #[serde(rename_all = "camelCase")]
    TeamUpdated { project_id: i64 },
}

/// Hochgeladene Datei (z. B. Bild im Composer), liegt unter `<data>/attachments/<uuid>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub uuid: String,
    pub name: String,
    pub size: i64,
    pub content_type: String,
    pub download_count: i64,
    /// `/attachments/<uuid>`
    pub url: String,
    pub created_at: i64,
}
