//! Arbeitsbereich: Verbindung zu nrzGit, Projekte (Repositories), Worktrees, Änderungen, Commits, Pushes, Pull Requests, Dateien, Vorschau, Benutzer und Einstellungen.

#[allow(unused_imports)]
use crate::common::*;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------- Anmeldung

/// `PUT /api/v1/me/avatar` – eigenes Profilbild als Data-URL oder Base64 (PNG, JPEG, WebP, GIF;
/// höchstens 2 MB).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvatarUpload {
    pub image: String,
}

/// `POST /api/v1/auth/setup` – erstes Konto (wird Administrator).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub full_name: String,
    /// Dürfen sich weitere Personen selbst registrieren?
    #[serde(default)]
    pub registration_open: bool,
}

/// `POST /api/v1/auth/register` – nur bei offener Registrierung.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub full_name: String,
}

/// `POST /api/v1/auth/login` – Benutzername oder E-Mail plus Passwort.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub login: String,
    pub password: String,
}

// ---------------------------------------------------------------------------------- Konto

/// `PUT /api/v1/me/profile`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdate {
    pub full_name: String,
    pub email: String,
}

/// `PUT /api/v1/me/password`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordChange {
    pub current_password: String,
    pub new_password: String,
}

/// `PUT /api/v1/me/appearance` – Thema ("dark" | "light" | "system") und Sprache ("de", "en" …).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    pub theme: String,
    pub language: String,
}

/// Zahlen für die Statistik (`GET /api/v1/me/stats`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Stats {
    pub projects: i64,
    pub threads: i64,
    /// Threads, in denen gerade ein Agent arbeitet
    pub threads_active: i64,
    pub subagents: i64,
    pub runs: i64,
    pub runs_today: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    /// Alle Läufe zusammen (Tokens genau aufgeschlüsselt, Dauer)
    pub total: TokenSums,
    /// Läufe nach Ausgang
    pub runs_done: i64,
    pub runs_error: i64,
    pub runs_stopped: i64,
    /// Nachrichten (Benutzer, Agent) und Werkzeugaufrufe
    pub user_messages: i64,
    pub agent_messages: i64,
    pub tool_calls: i64,
    /// Zeiträume: "today", "7d", "30d", "all"
    pub windows: Vec<StatWindow>,
    /// Läufe je Agent, meiste zuerst
    pub agents: Vec<AgentStat>,
    /// Je Agent und Modell, meiste Tokens zuerst
    pub models: Vec<ModelStat>,
}

/// Nutzung über einen Zeitraum mit Verlauf (`GET /api/v1/me/usage?range=24h|7d|30d|90d`),
/// z. B. für das Fenster in der Menüleiste.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageReport {
    pub range: String,
    /// Beginn des ersten Abschnitts (Unix-Sekunden) und Länge eines Abschnitts
    pub from: i64,
    pub bucket_seconds: i64,
    /// Threads mit mindestens einem Lauf im Zeitraum
    pub sessions: i64,
    pub total: TokenSums,
    /// Je Agent, meiste Kosten bzw. Tokens zuerst
    pub agents: Vec<AgentUsage>,
    /// Verlauf je Agent (gleich viele Abschnitte, lückenlos)
    pub series: Vec<UsageSeries>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentUsage {
    pub agent: AgentKind,
    pub sessions: i64,
    pub sums: TokenSums,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageSeries {
    pub agent: AgentKind,
    pub points: Vec<UsagePoint>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsagePoint {
    pub cost_usd: f64,
    pub tokens: i64,
}

/// Statistik eines Threads (`GET /api/v1/threads/<id>/stats`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadStats {
    /// Läufe dieses Threads
    pub own: TokenSums,
    /// Mit allen Subagenten (rekursiv)
    pub with_subagents: TokenSums,
    pub subagents: i64,
    pub user_messages: i64,
    pub agent_messages: i64,
    pub tool_calls: i64,
    /// Je Agent und Modell (mit Subagenten)
    pub models: Vec<ModelStat>,
}

/// Summen über Läufe.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TokenSums {
    pub runs: i64,
    /// Alle Eingabe-Tokens (einschliesslich Cache)
    pub input_tokens: i64,
    /// Alle Ausgabe-Tokens (einschliesslich Denken)
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub cost_usd: f64,
    /// Laufzeit der Agenten in Sekunden
    pub seconds: i64,
}

impl TokenSums {
    /// Eingabe ohne Cache (neu verarbeitete Tokens)
    pub fn fresh_input(&self) -> i64 {
        (self.input_tokens - self.cache_read_tokens - self.cache_write_tokens).max(0)
    }
    pub fn total_tokens(&self) -> i64 {
        self.input_tokens + self.output_tokens
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatWindow {
    pub key: String,
    pub sums: TokenSums,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentStat {
    pub agent: AgentKind,
    pub threads: i64,
    pub runs: i64,
    pub sums: TokenSums,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelStat {
    pub agent: AgentKind,
    /// Leer = Vorgabe des CLIs
    pub model: String,
    pub sums: TokenSums,
}

// ---------------------------------------------------------------------------------- Benutzerverwaltung

/// Benutzer in der Verwaltung (`GET /api/v1/users`, nur Administratoren).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub full_name: String,
    pub email: String,
    pub avatar_url: String,
    pub is_admin: bool,
    pub is_active: bool,
    pub projects: i64,
    pub threads: i64,
    pub nrzgit_connected: bool,
    pub created_at: i64,
    pub last_login_at: Option<i64>,
}

/// `POST /api/v1/users`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUser {
    pub username: String,
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub is_admin: bool,
}

/// `PATCH /api/v1/users/<id>` – nur gesetzte Felder ändern sich.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditUser {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_admin: Option<bool>,
    /// false = gesperrt (Sitzungen enden sofort)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

// ---------------------------------------------------------------------------------- Instanz

/// Einstellungen der Instanz (`GET/PUT /api/v1/settings/instance`, Administratoren).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSettings {
    /// Dürfen sich neue Personen selbst registrieren?
    pub registration_open: bool,
    /// Vorgabe-Adresse von nrzGit für neue Verbindungen
    pub nrzgit_url: String,
    /// Nur Anzeige (aus der Konfiguration)
    #[serde(default)]
    pub max_parallel_runs: i64,
    /// Nur Anzeige: Port des Vorschau-Proxys (0 = aus)
    #[serde(default)]
    pub preview_port: i64,
    /// Nur Anzeige
    #[serde(default)]
    pub data_dir: String,
    #[serde(default)]
    pub version: String,
    /// Nur Anzeige: Anzahl Benutzer
    #[serde(default)]
    pub users: i64,
}

// ---------------------------------------------------------------------------------- nrzGit

/// Verbindung zu nrzGit (`GET /api/v1/nrzgit?kind=`, alle: `GET /api/v1/nrzgit/connections`).
/// Das Token wird nie ausgeliefert.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NrzGitStatus {
    /// Adresse und Token hinterlegt und erfolgreich geprüft
    pub connected: bool,
    /// Gespeicherte Adresse (leer = keine Verbindung)
    pub url: String,
    /// Vorgabe-Adresse der Instanz (für das Formular)
    pub default_url: String,
    /// Benutzername in nrzGit
    pub username: String,
    /// Anfang und Ende des Tokens, z. B. "nrzg_ab…9f"
    pub token_hint: String,
    pub verified_at: Option<i64>,
    /// Seite in nrzGit, auf der man ein Token für Agenten (MCP) erstellt
    pub token_url: String,
    /// Fehler der letzten Prüfung (leer = in Ordnung)
    pub error: String,
    /// Adresse für Links im Browser (leer = wie `url`)
    #[serde(default)]
    pub public_url: String,
    /// Art der Plattform: immer "nrzgit"
    #[serde(default)]
    pub kind: String,
    /// nrzGit hat einen MCP-Server für die Agenten (immer)
    #[serde(default)]
    pub has_mcp: bool,
}

/// `PUT /api/v1/nrzgit` – Adresse und Token speichern (wird sofort geprüft).
/// Ohne Token bleibt das gespeicherte (nur bei gleicher Adresse).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NrzGitConnect {
    /// Adresse, unter der der Server von nrzX nrzGit erreicht (im Docker-Netz oft intern,
    /// z. B. `http://nrz-git_app_1:8380`)
    pub url: String,
    #[serde(default)]
    pub token: String,
    /// Adresse für Links im Browser (leer = wie `url`), z. B. `http://umbrel.local:8390`
    #[serde(default)]
    pub public_url: String,
    /// Erwartete Plattform: "nrzgit" oder leer. Antwortet
    /// dort die andere, wird nichts gespeichert.
    #[serde(default)]
    pub kind: String,
}

/// `GET /api/v1/nrzgit/candidates` – mögliche Adressen von nrzGit, vom Server aus geprüft.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NrzGitCandidate {
    pub url: String,
    /// z. B. "Docker-Name (Umbrel)", "Gateway des Docker-Netzes"
    pub label: String,
    /// Antwortet dort ein nrzGit?
    pub reachable: bool,
    pub version: String,
    /// Grund, wenn nicht erreichbar
    pub error: String,
    /// "nrzgit", wenn erreichbar
    #[serde(default)]
    pub kind: String,
}

/// Repository in nrzGit (für „Projekt hinzufügen“).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NrzGitRepo {
    pub id: i64,
    /// `owner/name`
    pub full_name: String,
    pub owner: String,
    pub owner_is_org: bool,
    pub name: String,
    pub description: String,
    pub private: bool,
    pub empty: bool,
    pub default_branch: String,
    /// Seite in nrzGit
    pub web_url: String,
    pub updated_at: i64,
    /// Schon als Projekt übernommen
    pub project_id: Option<i64>,
    /// Plattform: "nrzgit"
    #[serde(default)]
    pub forge: String,
}

/// Möglicher Besitzer eines neuen Repositorys: ich selbst oder eine Organisation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NrzGitOwner {
    pub username: String,
    pub full_name: String,
    pub is_org: bool,
}

// ---------------------------------------------------------------------------------- Projekte

/// `POST /api/v1/projects/import` – Repository aus nrzGit übernehmen (klonen).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProject {
    /// `owner/name`
    pub repo: String,
    /// Plattform: "nrzgit" (leer = die erste verbundene)
    #[serde(default)]
    pub forge: String,
}

/// `POST /api/v1/projects/create` – neues Repository in nrzGit anlegen und übernehmen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRepoProject {
    /// Benutzer oder Organisation in nrzGit (leer = ich)
    #[serde(default)]
    pub owner: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub private: bool,
    /// Mit README.md anlegen
    #[serde(default)]
    pub readme: bool,
    /// Plattform: "nrzgit" (leer = die erste verbundene)
    #[serde(default)]
    pub forge: String,
}

/// `POST /api/v1/projects/local` – lokales Projekt ohne nrzGit (`git init`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLocalProject {
    pub name: String,
}

/// `PATCH /api/v1/projects/<id>`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditProject {
    pub name: String,
}

/// Projekt mit Einzelheiten (`GET /api/v1/projects/<id>`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    #[serde(flatten)]
    pub summary: ProjectSummary,
    /// Seite des Repositorys in nrzGit (leer bei lokalen Projekten)
    pub web_url: String,
    pub clone_url: String,
    /// Branch des Hauptcheckouts
    pub branch: String,
    /// Ordner auf dem Server
    pub path: String,
    pub created_at: i64,
    /// Letzte Commits des Hauptcheckouts
    pub commits: Vec<Commit>,
}

// ---------------------------------------------------------------------------------- Git eines Threads

/// Geänderte Datei im Arbeitsordner eines Threads (gegen die Basis, inkl. unversionierter Dateien).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    /// Bei Umbenennungen der alte Pfad
    pub old_path: String,
    /// "added" | "modified" | "deleted" | "renamed" | "copied" | "typechange"
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
    pub binary: bool,
}

/// Zuletzt angelegter Pull Request eines Threads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestRef {
    pub number: i64,
    pub title: String,
    pub base: String,
    pub head: String,
    /// Seite des Pull Requests in nrzGit
    pub url: String,
    pub created_at: i64,
}

/// Zustand des Arbeitsordners eines Threads (`GET /api/v1/workspace/<thread>/status`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStatus {
    pub thread_id: i64,
    /// Titel des Threads
    pub title: String,
    pub project_id: Option<i64>,
    pub project_name: String,
    /// `owner/name` in nrzGit (leer bei lokalen Projekten und ohne Projekt)
    pub repo: String,
    /// Rechner, auf dem der Agent arbeitet
    pub host: String,
    /// Arbeitsordner auf dem Server
    pub path: String,
    /// Ordner ist ein Git-Repository
    pub is_git: bool,
    /// Thread soll einen eigenen Worktree haben
    pub use_worktree: bool,
    /// Eigener Worktree ist eingerichtet
    pub worktree: bool,
    /// Projekt wird noch geklont / hat einen Fehler
    pub project_status: String,
    pub branch: String,
    pub default_branch: String,
    /// Upstream, z. B. "origin/nrz/login-12" (leer = noch nicht gepusht)
    pub upstream: String,
    pub ahead: i64,
    pub behind: i64,
    /// Remote "origin" vorhanden (Push/Pull möglich)
    pub has_remote: bool,
    pub additions: i64,
    pub deletions: i64,
    pub files: Vec<ChangedFile>,
    /// Noch nicht committete Änderungen (gegen HEAD, inkl. neuer Dateien) – für den Commit
    pub uncommitted: Vec<ChangedFile>,
    /// Seite des Repositorys in nrzGit
    pub repo_url: String,
    pub pull_request: Option<PullRequestRef>,
    /// Ein Agent arbeitet gerade in diesem Thread
    pub busy: bool,
}

/// `POST /api/v1/workspace/<thread>/commit`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRequest {
    pub message: String,
    /// Nur diese Dateien (leer = alle Änderungen)
    #[serde(default)]
    pub files: Vec<String>,
    /// Danach pushen
    #[serde(default)]
    pub push: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitResult {
    pub sha: String,
    pub short_sha: String,
    pub summary: String,
    pub files: i64,
    pub pushed: bool,
}

/// Ergebnis von Push und Pull.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitResult {
    pub message: String,
    pub branch: String,
    pub ahead: i64,
    pub behind: i64,
}

/// Branch in der Auswahl.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub current: bool,
    /// Nur auf dem Server (origin), noch nicht lokal
    pub remote_only: bool,
    /// In einem anderen Worktree ausgecheckt (Wechsel nicht möglich)
    pub checked_out_elsewhere: bool,
    pub last_commit_at: i64,
}

/// `GET /api/v1/workspace/<thread>/branches`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchList {
    pub current: String,
    pub default_branch: String,
    pub branches: Vec<BranchInfo>,
}

/// `POST /api/v1/workspace/<thread>/branch` – wechseln oder (`create`) neu anlegen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchRequest {
    pub name: String,
    #[serde(default)]
    pub create: bool,
}

/// `POST /api/v1/workspace/<thread>/pull-request` – Branch pushen und Pull Request in nrzGit anlegen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestRequest {
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// Ziel-Branch (leer = Standard-Branch des Projekts)
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub draft: bool,
}

/// Eintrag im Dateibaum (`GET /api/v1/workspace/<thread>/files?path=`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    /// Pfad relativ zum Arbeitsordner
    pub path: String,
    pub dir: bool,
    pub size: i64,
}

/// Datei zum Anzeigen/Bearbeiten (`GET /api/v1/workspace/<thread>/file?path=`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub size: i64,
    /// Binärdatei – kein Inhalt
    pub binary: bool,
    /// Zu gross zum Bearbeiten – kein Inhalt
    pub too_large: bool,
}

/// `PUT /api/v1/workspace/<thread>/file`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileWrite {
    pub path: String,
    pub content: String,
}

// ---------------------------------------------------------------------------------- Vorschau

/// Lauschender Port (`GET /api/v1/previews?threadId=`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPort {
    pub port: u16,
    /// Programm, z. B. "node" (leer = unbekannt)
    pub process: String,
    /// Gestartet im Arbeitsordner dieses Threads
    pub this_thread: bool,
}

/// `POST /api/v1/previews/token`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTokenRequest {
    pub thread_id: i64,
    pub port: u16,
    /// Pfad nach dem Öffnen (Vorgabe "/")
    #[serde(default)]
    pub path: String,
}

/// Kurzlebige Adresse zum Öffnen der Vorschau (setzt ein Cookie und leitet weiter).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewToken {
    pub url: String,
    /// Basis der Vorschau, z. B. "http://umbrel.local:9301"
    pub origin: String,
    pub expires_at: i64,
}

// ---------------------------------------------------------------------------------- nrzXMobile

/// `GET /api/v1/mobile` – Verbindung dieses Benutzers zu nrzXMobile (App auf dem Umbrel).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MobileStatus {
    /// Verbunden eingerichtet (Adresse und Token gespeichert)
    pub configured: bool,
    /// Adresse von nrzXMobile für PCs, z. B. `http://umbrel.local:8471`
    pub url: String,
    /// Name dieses PCs in nrzXMobile
    pub name: String,
    pub connected: bool,
    /// Letzter Fehler der Verbindung
    pub error: String,
}

/// `POST /api/v1/mobile` – mit dem Code aus nrzXMobile koppeln.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MobilePair {
    pub url: String,
    pub code: String,
    /// Name des PCs (leer = Rechnername)
    pub name: String,
}

/// Antwort von nrzXMobile auf `POST /pair/claim`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MobileClaim {
    pub pc_id: String,
    pub token: String,
}

/// `POST /workspace/<id>/raw-token` – Freigabe des Arbeitsordners für den integrierten Browser:
/// `url` ist der Anfang (`/raw/<token>/`), dahinter der Pfad der Datei.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RawToken {
    pub url: String,
}

// ---------------------------------------------------------------------------------- Übernahme aus nrzCode

/// Ein Agent-Konto aus nrzCode (zum Übernehmen).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NrzCodeAccount {
    pub kind: crate::AgentKind,
    pub label: String,
    /// Gibt es in nrzX schon ein Konto dieser Art mit dieser Beschriftung?
    pub exists: bool,
}

/// Ein Projekt aus nrzCode (zum Übernehmen).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NrzCodeProject {
    pub name: String,
    /// `owner/name` in nrzGit (leer bei lokalen Projekten – die werden kopiert)
    pub repo: String,
    pub exists: bool,
}

/// `GET /api/v1/import/nrzcode` – was nrzCode auf diesem Rechner hat (Konten mit ihren
/// Anmeldungen, Verbindung zu nrzGit, Projekte). `found = false`, wenn es kein nrzCode gibt.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NrzCodeImportPreview {
    pub found: bool,
    /// Datenordner von nrzCode
    pub path: String,
    pub accounts: Vec<NrzCodeAccount>,
    /// Adresse von nrzGit, wenn nrzCode verbunden ist
    pub nrzgit: Option<String>,
    /// nrzX ist schon mit nrzGit verbunden
    pub nrzgit_exists: bool,
    pub projects: Vec<NrzCodeProject>,
}

/// `POST /api/v1/import/nrzcode` – was übernommen werden soll.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NrzCodeImportRequest {
    pub accounts: bool,
    pub nrzgit: bool,
    pub projects: bool,
}

/// Ergebnis der Übernahme.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NrzCodeImportResult {
    pub accounts: i64,
    /// Schon vorhandene Konten, deren Anmeldung neu übernommen und verknüpft wurde
    pub refreshed: i64,
    pub nrzgit: bool,
    pub projects: i64,
    /// Übersprungenes mit Grund (schon vorhanden, Fehler …)
    pub skipped: Vec<String>,
}
