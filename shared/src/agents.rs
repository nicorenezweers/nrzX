//! Agenten: Agent-Konten (mehrere Abos je Anbieter mit eigenem Home-Verzeichnis), Werkzeuge (CLIs installieren), Läufe, Nachrichten, Subagenten, Terminals.
//!
//! Aufbau von `Message::data` je Rolle:
//! * `user` – [`UserMessageData`] (Anhänge)
//! * `assistant` – `{}` (Text in `content`, gerendert in `html`)
//! * `tool` – [`ToolCall`] (Werkzeugaufruf, auch Denkschritte mit `kind = "think"`)
//! * `subagents` – [`SubagentsData`] (IDs der gestarteten Subagenten)
//! * `system` – [`SystemData`] (z. B. Kontext zurückgesetzt, Agent gewechselt)
//! * `error` – `{}` (Fehlertext in `content`)

#[allow(unused_imports)]
use crate::common::*;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------- Agent-Arten

/// Modell-Vorschlag (`id` wird an das CLI übergeben, leer = Standard des CLIs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub id: String,
    pub label: String,
}

/// Anmeldeart einer Agent-Art.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthKindInfo {
    /// "login" (Anmeldung im CLI, Abo/OAuth) | "api_key" | "oauth_token"
    pub key: String,
    pub label: String,
    /// Mögliche Umgebungsvariablen für den Schlüssel (die erste ist die Vorgabe)
    pub env_vars: Vec<String>,
    pub hint: String,
}

/// Eine Agent-Art mit Programm, Modellen, Stufen, Anmeldearten und den Konten des Benutzers
/// (`GET /api/v1/agents`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub kind: AgentKind,
    pub name: String,
    /// Programmname, z. B. "claude"
    pub program: String,
    /// npm-Paket, z. B. "@anthropic-ai/claude-code"
    pub npm_package: String,
    pub installed: bool,
    pub version: String,
    /// Gefundenes Programm (leer = nicht gefunden)
    pub bin_path: String,
    /// Vorschläge; freie Eingabe ist immer möglich, leer = Standard des CLIs
    pub models: Vec<ModelOption>,
    /// Unterstützte Denk-Stufen ("low" | "medium" | "high" | "xhigh" | "max"), leer = keine Auswahl
    pub efforts: Vec<String>,
    pub auth_kinds: Vec<AuthKindInfo>,
    /// Befehl für die Anmeldung im Terminal (leer = nur Schlüssel)
    pub login_command: String,
    /// Was bei der Anmeldung zu tun ist
    pub login_help: String,
    /// Gespräche lassen sich mit der Sitzungs-ID fortsetzen
    pub supports_resume: bool,
    /// Konten dieser Art (Standardkonto zuerst)
    pub accounts: Vec<AgentAccount>,
}

/// Modelle einer Agent-Art, ggf. vom CLI selbst gelistet (`GET /api/v1/agents/<kind>/models`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelList {
    pub models: Vec<ModelOption>,
    /// "cli" (vom Programm gelistet) | "builtin" (Vorschläge von nrzX)
    pub source: String,
}

// ---------------------------------------------------------------------------------- Konten

/// Agent-Konto (ein Abo oder Schlüssel) – Geheimnisse nur als Hinweis.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAccount {
    pub id: i64,
    pub kind: AgentKind,
    pub label: String,
    /// "login" | "api_key" | "oauth_token"
    pub auth_kind: String,
    /// Umgebungsvariable des Schlüssels (z. B. "ANTHROPIC_API_KEY")
    pub secret_env: String,
    /// "sk-…cdef" (leer = kein Schlüssel hinterlegt)
    pub secret_hint: String,
    pub default_model: String,
    pub default_effort: String,
    /// "unknown" | "ok" | "login_required" | "error"
    pub status: String,
    pub status_message: String,
    pub is_default: bool,
    /// Home-Verzeichnis des Kontos auf dem Server
    pub home: String,
    pub threads: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: Option<i64>,
}

/// `POST /api/v1/accounts`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewAccount {
    pub kind: AgentKind,
    pub label: String,
    pub auth_kind: String,
    pub secret: String,
    pub secret_env: String,
    pub default_model: String,
    pub default_effort: String,
    pub is_default: bool,
}

/// `PATCH /api/v1/accounts/<id>` – nur gesetzte Felder ändern.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountUpdate {
    pub label: Option<String>,
    pub auth_kind: Option<String>,
    /// Neuer Schlüssel (leer = Schlüssel entfernen)
    pub secret: Option<String>,
    pub secret_env: Option<String>,
    pub default_model: Option<String>,
    pub default_effort: Option<String>,
    pub is_default: Option<bool>,
}

/// `POST /api/v1/accounts/<id>/login` – Terminal mit dem Anmeldebefehl im Konto.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginSession {
    pub terminal: TerminalInfo,
    pub command: String,
    pub help: String,
}

// ---------------------------------------------------------------------------------- Werkzeuge

/// Ein Werkzeug (Agent-CLI oder Docker CLI) mit Installationsstand (`GET /api/v1/tools`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    /// "claude" … "grok" oder "docker"
    pub key: String,
    pub name: String,
    pub program: String,
    /// npm-Paket bzw. Herkunft ("download.docker.com")
    pub package: String,
    pub installed: bool,
    pub version: String,
    pub path: String,
    /// "nrzx" (in tools/ installiert) | "path" | "env" (NRZX_BIN_…) | ""
    pub source: String,
    /// Läuft gerade eine Installation?
    pub installing: bool,
    /// Letzte bekannte neueste Version (nach `GET /tools/latest`)
    pub latest: String,
    /// Meldung der letzten Installation (Fehler)
    pub error: String,
}

/// Neueste Version eines Werkzeugs (`GET /api/v1/tools/latest`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolLatest {
    pub key: String,
    pub latest: String,
}

// ---------------------------------------------------------------------------------- Threads

/// `POST /api/v1/threads` – neuer Thread mit erstem Lauf. Ohne `prompt`, aber mit `tasks`
/// entsteht ein Elternthread, der nur die Subagenten startet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewThread {
    pub project_id: Option<i64>,
    pub prompt: String,
    pub agent: AgentKind,
    pub account_id: Option<i64>,
    pub model: String,
    pub effort: String,
    pub use_worktree: bool,
    /// Nur lesen: Fragen zur Codebasis stellen, der Agent ändert nichts (kein Worktree)
    pub read_only: bool,
    /// FreeStyle: keine Rückfragen, der Agent entscheidet selbst
    pub freestyle: bool,
    pub title: Option<String>,
    /// UUIDs aus `POST /attachments`
    pub attachments: Vec<String>,
    pub tasks: Vec<SubagentTask>,
    /// Deep Research: recherchieren und ein Research Paper schreiben (ohne Projekt, ohne Rückfragen)
    pub research: bool,
}

impl Default for NewThread {
    fn default() -> Self {
        NewThread {
            project_id: None,
            prompt: String::new(),
            agent: AgentKind::Claude,
            account_id: None,
            model: String::new(),
            effort: String::new(),
            // Standard: direkt im Hauptcheckout (Standard-Branch) arbeiten
            use_worktree: false,
            read_only: false,
            freestyle: false,
            title: None,
            attachments: Vec::new(),
            tasks: Vec::new(),
            research: false,
        }
    }
}

/// `GET /api/v1/threads/<id>/paper` – Research Paper einer Recherche (auch über ihr Gespräch).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResearchPaper {
    /// Thread der Recherche
    pub research_id: i64,
    /// Inhalt von `paper.md` (leer, solange es noch keines gibt)
    pub markdown: String,
    /// Als sicheres HTML
    pub html: String,
    /// Zuletzt geändert (Unix-Sekunden, 0 = noch keines)
    pub updated_at: i64,
}

/// `POST /api/v1/threads/<id>/messages` – Gespräch fortsetzen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PostMessage {
    pub text: String,
    pub attachments: Vec<String>,
    /// Arbeitet der Agent noch: unterbrechen und sofort senden (sonst kommt die Nachricht in die
    /// Warteschlange und geht nach dem Lauf raus)
    pub interrupt: bool,
}

/// `PATCH /api/v1/threads/<id>` – umbenennen, anheften, Agent/Konto/Modell/Stufe wechseln.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadPatch {
    pub title: Option<String>,
    pub pinned: Option<bool>,
    /// In der Seitenleiste aus- bzw. einblenden
    pub hidden: Option<bool>,
    pub agent: Option<AgentKind>,
    pub account_id: Option<i64>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// FreeStyle ein/aus
    pub freestyle: Option<bool>,
    /// Nur lesen ein/aus (an schaltet FreeStyle aus); gilt ab dem nächsten Lauf
    pub read_only: Option<bool>,
    /// Nach einem Rate-Limit automatisch weitermachen (gilt auch als Vorgabe für neue Limits)
    pub resume_auto: Option<bool>,
}

/// Antwort von `POST /api/v1/threads/<id>/messages/<mid>/retract`: Text und Anhänge der
/// zurückgezogenen Nachricht (zum Bearbeiten im Composer).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RetractedMessage {
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub thread: ThreadSummary,
}

/// `POST /api/v1/threads/order` – Threads in dieser Reihenfolge (oben zuerst) festlegen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadOrder {
    pub ids: Vec<i64>,
}

/// Ein Lauf (ein Aufruf des Agenten).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInfo {
    pub id: i64,
    pub thread_id: i64,
    pub agent: AgentKind,
    pub account_id: Option<i64>,
    pub model: String,
    pub effort: String,
    /// "queued" | "running" | "done" | "error" | "stopped"
    pub status: String,
    pub exit_code: Option<i64>,
    pub error: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// `GET /api/v1/threads/<id>`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetail {
    pub thread: ThreadSummary,
    pub parent: Option<ThreadSummary>,
    pub children: Vec<ThreadSummary>,
    pub runs: Vec<RunInfo>,
    /// Arbeitsordner auf dem Server
    pub workdir: String,
    /// Sitzungs-ID des CLIs (zum Fortsetzen)
    pub session_id: String,
}

/// Datei im Arbeitsordner (für `@` im Composer).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub dir: bool,
}

// ---------------------------------------------------------------------------------- Nachrichten

/// `data` einer Benutzernachricht.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserMessageData {
    pub attachments: Vec<Attachment>,
    /// Von nrzX nach einem Rate-Limit automatisch gesendet
    pub auto_resume: bool,
    /// Zwischenruf (der Agent wurde dafür unterbrochen); der Hinweis an den Agenten steht nicht im Text
    pub interjection: bool,
}

/// Bearbeitung einer Datei (kurz: alter und neuer Text, gekürzt).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EditDiff {
    pub path: String,
    pub before: String,
    pub after: String,
    /// Zeilen hinzugefügt / entfernt, aus dem ungekürzten Text gezählt (0/0 bei älteren
    /// Nachrichten – dann zählt [`EditDiff::counts`] aus `before`/`after`)
    pub adds: i64,
    pub dels: i64,
    /// Zeile (ab 1), an der `after` in der neuen Datei beginnt; 0 = unbekannt (ältere Nachrichten,
    /// Datei nicht lesbar). Der Server liest die Datei vor und nach der Bearbeitung.
    #[serde(skip_serializing_if = "is_zero")]
    pub line: i64,
    /// Zeile (ab 1), an der `before` in der alten Datei begann
    #[serde(skip_serializing_if = "is_zero")]
    pub old_line: i64,
    /// Unveränderte Zeilen davor und danach (höchstens drei)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context_before: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context_after: Vec<String>,
    /// `before`/`after` gekürzt (sehr grosse Abschnitte, neue grosse Dateien)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl EditDiff {
    /// Zeilen hinzugefügt und entfernt.
    pub fn counts(&self) -> (i64, i64) {
        if self.adds > 0 || self.dels > 0 {
            (self.adds, self.dels)
        } else {
            line_changes(&self.before, &self.after)
        }
    }
}

/// Geänderte Zeilen zwischen zwei Texten (gleiche Zeilen am Anfang und Ende zählen nicht):
/// (hinzugefügt, entfernt).
pub fn line_changes(before: &str, after: &str) -> (i64, i64) {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end = 0;
    while end < a.len() - start && end < b.len() - start && a[a.len() - 1 - end] == b[b.len() - 1 - end] {
        end += 1;
    }
    ((b.len() - start - end) as i64, (a.len() - start - end) as i64)
}

/// `data` einer Werkzeug-Nachricht (vereinheitlicht über alle CLIs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCall {
    /// ID des Aufrufs beim CLI
    pub tool_id: String,
    /// Name beim CLI, z. B. "Edit", "command_execution", "mcp__nrzgit__create_repo"
    pub name: String,
    /// "read" | "edit" | "write" | "command" | "search" | "web" | "mcp" | "task" | "todo" | "think" | "other"
    pub kind: String,
    /// Ziel: Pfad, Befehl, Suchmuster, Adresse, MCP-Werkzeug …
    pub target: String,
    /// Kurz, z. B. "Edit src/main.rs"
    pub summary: String,
    pub input: serde_json::Value,
    /// Ergebnis (gekürzt)
    pub output: String,
    /// "running" | "ok" | "error"
    pub status: String,
    pub diffs: Vec<EditDiff>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// `data` einer Subagenten-Nachricht („4 subagents · 4 working“).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubagentsData {
    pub children: Vec<i64>,
    pub titles: Vec<String>,
    /// Vom Benutzer (Composer) statt vom Agenten gestartet
    pub by_user: bool,
}

/// `data` einer Systemnachricht.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemData {
    /// "context_reset" | "agent_switch" | "notice" | "stopped" | "raw" | "rate_limit" | ""
    pub kind: String,
    /// rate_limit: Zeitpunkt, an dem das Limit zurückgesetzt wird (Unix-Sekunden)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// rate_limit: Originalmeldung des CLIs
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl SystemData {
    pub fn of(kind: &str) -> Self {
        SystemData { kind: kind.into(), ..Default::default() }
    }
}

/// Eine Antwortmöglichkeit einer Rückfrage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

/// `data` einer Rückfrage des Agenten (Rolle `question`, Text = Frage).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QuestionData {
    pub options: Vec<QuestionOption>,
    /// Mehrere Optionen wählbar
    pub multiple: bool,
    /// Antwort des Benutzers (None = offen)
    pub answer: Option<String>,
    /// Der Benutzer überlässt die Entscheidung dem Agenten
    pub delegated: bool,
    /// Nicht mehr beantwortbar (Lauf beendet oder Zeit abgelaufen)
    pub expired: bool,
}

/// Eintrag im Seiten-Chat `/btw` (Frage zum Zwischenstand und Antwort eines Modells).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BtwEntry {
    pub id: i64,
    pub thread_id: i64,
    pub question: String,
    /// Antwort als Markdown (leer, solange `status` = "pending")
    pub answer: String,
    /// Gerendertes Markdown der Antwort
    pub html: String,
    /// pending | done | error
    pub status: String,
    pub created_at: i64,
}

/// `POST /api/v1/threads/<id>/btw`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BtwAsk {
    pub question: String,
    /// Eigenes Modell für den Zwischenstand (Einstellung im Seiten-Chat). Ohne Angabe: Agent
    /// und Konto des Threads mit einem kleinen Modell (Claude: Haiku) und niedriger Stufe.
    pub agent: Option<crate::AgentKind>,
    pub account_id: Option<i64>,
    pub model: String,
    pub effort: String,
}

/// `POST /api/v1/threads/<id>/status` – Stand mit Zeitschätzung (`/status` im Composer). Wie
/// `/btw` ein kurzer Nebenlauf, aber nichts wird gespeichert.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusAsk {
    /// Sprache der Antwort ("de", "en", …)
    pub lang: String,
    /// Modell wie im Seiten-Chat gewählt (ohne Angabe: kleines Modell des Thread-Agenten)
    pub agent: Option<crate::AgentKind>,
    pub account_id: Option<i64>,
    pub model: String,
    pub effort: String,
}

/// Antwort auf [`StatusAsk`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusReport {
    /// Geschätzte Restdauer, kurz („ca. 10 Min.“, „fertig“); leer = keine Schätzung möglich
    pub estimate: String,
    /// Stand als Markdown (kurze Liste) und gerendert
    pub summary: String,
    pub html: String,
    /// Agent arbeitet gerade
    pub active: bool,
    /// Beginn des aktuellen Laufs (Unix-Sekunden)
    pub running_since: Option<i64>,
    /// Todo-Liste des Agenten: erledigt / gesamt (0/0 = keine Liste)
    pub todos_done: u32,
    pub todos_total: u32,
    /// Eintrag, an dem der Agent gerade ist
    pub todo_current: String,
    /// Nebenlauf fehlgeschlagen (die Fakten oben stimmen trotzdem)
    pub error: String,
}

/// `ESTIMATE:`-Zeile aus der Antwort des Nebenlaufs lösen: (Schätzung, Rest).
pub fn split_estimate(answer: &str) -> (String, String) {
    let mut estimate = String::new();
    let mut rest = Vec::new();
    for line in answer.lines() {
        let t = line.trim().trim_start_matches(['*', '#', '-', ' ']).trim();
        let lower = t.to_lowercase();
        let hit = ["estimate:", "schätzung:", "schaetzung:", "restdauer:"].iter().find(|k| lower.starts_with(*k));
        match hit {
            Some(k) if estimate.is_empty() => {
                estimate = t[k.len()..].trim_matches(['*', '`', '"', ' ', '\t']).chars().take(60).collect();
            }
            _ => rest.push(line),
        }
    }
    (estimate, rest.join("\n").trim().to_string())
}

/// `POST /api/v1/threads/<id>/questions/<mid>` – Rückfrage beantworten.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnswerQuestion {
    pub answer: String,
    /// Statt einer Antwort: „entscheide selbst“
    pub delegate: bool,
}

// ---------------------------------------------------------------------------------- Subagenten

/// Eine Aufgabe für einen Subagenten.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubagentTask {
    pub title: String,
    pub prompt: String,
    pub agent: Option<AgentKind>,
    pub account_id: Option<i64>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// `POST /api/v1/threads/<id>/subagents`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewSubagents {
    pub tasks: Vec<SubagentTask>,
}

// ---------------------------------------------------------------------------------- Terminals

/// `POST /api/v1/terminals`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TerminalOpen {
    pub thread_id: Option<i64>,
    pub project_id: Option<i64>,
    pub account_id: Option<i64>,
    /// Befehl statt einer Shell (z. B. der Anmeldebefehl eines CLIs)
    pub command: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub thread_id: Option<i64>,
    pub account_id: Option<i64>,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub cols: u16,
    pub rows: u16,
    pub created_at: i64,
}

/// `POST /api/v1/terminals/<id>/input` – Text (UTF-8, Steuerzeichen wie "\r", "\u{3}").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TerminalInput {
    pub data: String,
}

/// `POST /api/v1/terminals/<id>/resize`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

// ---------------------------------------------------------------------------------- Skills

/// Gespeicherter Prompt-Baustein (im Composer mit `$`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub prompt: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewSkill {
    pub name: String,
    pub description: String,
    pub prompt: String,
}

// ---------------------------------------------------------------------------------- Nutzung

/// Verbrauch in einem Zeitfenster (aus den Läufen in nrzX gezählt).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageWindow {
    /// "5h" | "today" | "7d" | "30d"
    pub key: String,
    pub runs: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

/// Limit beim Anbieter (z. B. Claude-Abo: 5-Stunden-Fenster, Woche).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageLimit {
    /// "five_hour" | "seven_day" | "seven_day_opus" | "primary" | "secondary" …
    pub key: String,
    /// Beschriftung, z. B. "5 Stunden", "Woche"
    pub label: String,
    /// Verbraucht in Prozent (0–100)
    pub used_percent: f64,
    /// Zurückgesetzt um (Unix-Sekunden)
    pub resets_at: Option<i64>,
}

/// Nutzung eines Agent-Kontos.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountUsage {
    pub account_id: i64,
    pub kind: AgentKind,
    pub label: String,
    pub auth_kind: String,
    /// Fenster "5h", "today", "7d", "30d"
    pub windows: Vec<UsageWindow>,
    /// Limits beim Anbieter (leer = Anbieter meldet keine)
    pub limits: Vec<UsageLimit>,
    /// Woher die Limits stammen: "api" (Anbieter gefragt) | "session" (letzte Sitzung des CLIs) | ""
    pub limits_source: String,
    /// Warum keine Limits abgefragt werden konnten
    pub limits_error: String,
    /// Zeitpunkt der Limits (Unix-Sekunden)
    pub limits_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

/// `GET /api/v1/usage`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageOverview {
    pub accounts: Vec<AccountUsage>,
    pub generated_at: i64,
}

// ---------------------------------------------------------------------------------- Notizblock

/// Vorbereiteter Prompt bzw. Idee im Notizblock (`GET /api/v1/notes`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptNote {
    pub id: i64,
    pub project_id: Option<i64>,
    pub text: String,
    /// Angehängte Bilder (gehen beim Übernehmen in den Composer mit)
    pub attachments: Vec<crate::Attachment>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// `POST /api/v1/notes` und `PATCH /api/v1/notes/<id>`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NoteInput {
    pub text: Option<String>,
    /// Projekt (None = unverändert bzw. ohne Projekt beim Anlegen)
    pub project_id: Option<i64>,
    /// Projekt entfernen
    pub clear_project: bool,
    /// UUIDs der Anhänge (`POST /api/v1/attachments`); None = unverändert
    pub attachments: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------------- KI-Ketten

/// Eine Stufe einer KI-Kette: das Konto bestimmt den Agenten; `model` leer = wie bisher (gleicher
/// Agent) bzw. das Standardmodell des Kontos.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChainStep {
    pub account_id: i64,
    pub model: String,
}

/// KI-Kette: Ist ein Konto am Limit, macht der Thread mit der nächsten Stufe weiter
/// (z. B. Claude Abo 1 → Claude Abo 2 → Grok Abo 1).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiChain {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub steps: Vec<ChainStep>,
}

/// Konto am Limit (bis zum Zurücksetzen überspringt die Kette es).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChainBlock {
    pub account_id: i64,
    /// Unix-Sekunden
    pub until: i64,
}

/// `GET/PUT /api/v1/chains`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChainSettings {
    pub chains: Vec<AiChain>,
    /// Nur lesen: Konten, die gerade am Limit sind
    pub blocked: Vec<ChainBlock>,
}

/// Bot (Bot-Modus): dauerhafter KI-Teamkollege mit Persönlichkeit, Gedächtnis, eigenem
/// Arbeitsordner und Routinen. `GET /api/v1/bots`, `GET /api/v1/bots/<id>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Bot {
    pub id: i64,
    pub name: String,
    pub emoji: String,
    pub role: String,
    pub instructions: String,
    pub memory: String,
    pub agent: AgentKind,
    pub account_id: Option<i64>,
    pub model: String,
    pub effort: String,
    /// Aktuelles Gespräch (Thread)
    pub thread_id: Option<i64>,
    pub routines: Vec<BotRoutine>,
    /// Eigene MCP-Server (nur dieser Bot nutzt sie)
    pub mcp: Vec<BotMcp>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// MCP-Server, den nur ein Bot nutzt: über eine Adresse (HTTP, optional mit Token) oder als
/// Befehl auf diesem Rechner (stdio).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BotMcp {
    /// Name beim Agenten (Buchstaben, Ziffern, `-`, `_`)
    pub name: String,
    /// Adresse, z. B. `https://mcp.notion.com/mcp` – leer: `command`
    pub url: String,
    /// Bearer-Token für die Adresse (optional)
    pub token: String,
    /// Befehl, z. B. `npx -y @modelcontextprotocol/server-memory`
    pub command: String,
    pub enabled: bool,
}

/// `POST /api/v1/bots`, `PATCH /api/v1/bots/<id>` (beim Ändern zählen nur gesetzte Felder).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BotInput {
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub role: Option<String>,
    pub instructions: Option<String>,
    pub memory: Option<String>,
    pub agent: Option<AgentKind>,
    /// `Some(None)` = Standardkonto der Art
    pub account_id: Option<Option<i64>>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Eigene MCP-Server (ersetzt die Liste)
    pub mcp: Option<Vec<BotMcp>>,
}

/// Routine eines Bots: zu einer Uhrzeit an bestimmten Wochentagen einen Auftrag schicken.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BotRoutine {
    pub id: i64,
    pub title: String,
    pub prompt: String,
    /// Minuten nach Mitternacht (Ortszeit)
    pub time_min: i64,
    /// Bitmaske der Wochentage: Mo = 1, Di = 2 … So = 64
    pub days: i64,
    /// Minuten östlich von UTC (Ortszeit beim Speichern)
    pub tz_offset: i64,
    pub enabled: bool,
    pub last_run_at: i64,
}

/// `POST /api/v1/bots/<id>/routines`, `PATCH /api/v1/bots/<id>/routines/<rid>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BotRoutineInput {
    pub title: String,
    pub prompt: String,
    pub time_min: i64,
    pub days: i64,
    pub tz_offset: i64,
    pub enabled: bool,
}

/// Titel aus der ersten Nachricht: erste Zeile, Markdown-Zeichen weg, höchstens 60 Zeichen.
pub fn title_from(text: &str) -> String {
    let line = text.lines().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("");
    let line: String = line.trim_start_matches(['#', '>', '-', '*', ' ', '/']).replace(['`', '*'], "");
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut out = String::new();
    for w in words {
        if out.chars().count() + w.chars().count() + 1 > 60 {
            if out.is_empty() {
                out = w.chars().take(59).collect::<String>();
            }
            out.push('…');
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.is_empty() {
        "Neuer Thread".into()
    } else {
        // Erster Buchstabe gross
        let mut c = out.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or(out)
    }
}

#[cfg(test)]
mod tests {
    use super::split_estimate;

    #[test]
    fn estimate_line() {
        let (e, rest) = split_estimate("ESTIMATE: ca. 10 Min.\n- Tests laufen\n- danach Release");
        assert_eq!(e, "ca. 10 Min.");
        assert_eq!(rest, "- Tests laufen\n- danach Release");
        let (e, _) = split_estimate("**Schätzung:** `1–2 h`\n- x");
        assert_eq!(e, "1–2 h");
        let (e, rest) = split_estimate("- nur Stand");
        assert!(e.is_empty());
        assert_eq!(rest, "- nur Stand");
    }
}
