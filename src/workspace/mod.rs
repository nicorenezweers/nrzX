//! Arbeitsbereich: Verbindung zu nrzGit (Adresse + Zugriffstoken je Benutzer und
//! Plattform, beide gleichzeitig möglich), Projekte
//! (Repositories klonen oder in nrzGit anlegen), Worktrees je Thread, Änderungen, Commits,
//! Pushes und Pull Requests, Dateien, Vorschau-Proxy, Benutzer und Einstellungen der App.
//!
//! Eingehängt in `lib.rs`: `api_routes()` unter `/api/v1`, `routes()` unter `/`.
//! `start()` läuft einmal nach dem Start (z. B. Vorschau-Proxy auf `config.preview_port`).
//!
//! Module: `users` (Anmeldung, Konto, Verwaltung), `settings` (Instanz), `nrzgit` (Verbindung
//! und API-Client), `projects`, `threadgit` (Git eines Threads), `files`, `preview` (Ports,
//! Token, Proxy), `gitops` (Git-Anmeldung, Worktrees, Änderungen), `model` (Zeilen, Rechte).
//! API-Beschreibung: `docs/api-workspace.md`.

pub mod files;
mod import;
pub mod gitops;
pub mod model;
pub mod nrzgit;
pub mod preview;
pub mod projects;
pub mod settings;
pub mod threadgit;
pub mod users;

use crate::error::{AppError, AppResult};
use crate::AppState;
use rocket::Route;
use std::path::PathBuf;

/// Routen unter `/api/v1`.
pub fn api_routes() -> Vec<Route> {
    let mut r = Vec::new();
    r.extend(users::routes());
    r.extend(settings::routes());
    r.extend(nrzgit::routes());
    r.extend(projects::routes());
    r.extend(threadgit::routes());
    r.extend(files::routes());
    r.extend(preview::routes());
    r.extend(import::routes());
    r
}

/// Routen unter `/` (ausserhalb der API).
pub fn routes() -> Vec<Route> {
    files::raw_routes()
}

/// Hintergrundarbeiten starten (nur wenn `config.background`): Vorschau-Proxy und abgebrochenes
/// Klonen wieder aufnehmen.
pub fn start(state: AppState) {
    preview::start(&state);
    tokio::spawn(projects::resume(state));
}

/// Zugang eines Benutzers zu nrzGit.
#[derive(Debug, Clone)]
pub struct NrzGitAccess {
    /// Basis-Adresse ohne `/` am Ende, z. B. `http://umbrel.local:8390`
    pub url: String,
    /// Benutzername auf der Plattform
    pub username: String,
    /// Zugriffstoken (`nrzg_…`) – für API, MCP und Git über HTTP
    pub token: String,
    /// Adresse für Links im Browser (ohne `/` am Ende); gleich `url`, wenn keine eigene angegeben ist
    pub public_url: String,
    /// Immer `nrzgit` – zugleich der Name des MCP-Servers
    pub kind: String,
    /// Hat nrzGit einen MCP-Server? (immer)
    pub has_mcp: bool,
}

impl NrzGitAccess {
    /// Adresse des MCP-Servers von nrzGit und Token.
    pub fn mcp(&self) -> Option<(String, String)> {
        Some((format!("{}/mcp", self.url.trim_end_matches('/')), self.token.clone()))
    }

    /// Name des MCP-Servers bei den Agenten.
    pub fn server_name(&self) -> &'static str {
        nrzgit::KIND_NRZGIT
    }
}

/// Geprüfte Verbindung des Benutzers zu nrzGit (als Liste: leer oder ein Eintrag).
pub async fn forge_accesses(state: &AppState, user_id: i64) -> AppResult<Vec<NrzGitAccess>> {
    let rows: Vec<(String, String, String, String, String, bool)> = sqlx::query_as(
        "SELECT url, username, token, public_url, kind, has_mcp FROM nrzgit_connections \
         WHERE user_id = ? AND verified_at IS NOT NULL AND kind = 'nrzgit'",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(url, username, token, public_url, kind, has_mcp)| {
            let public_url = if public_url.is_empty() { url.clone() } else { public_url };
            let has_mcp = has_mcp || kind == nrzgit::KIND_NRZGIT;
            NrzGitAccess { url, username, token, public_url, kind, has_mcp }
        })
        .collect())
}

/// Zugang des Benutzers zu nrzGit (`kind` = `nrzgit`).
pub async fn forge_access(state: &AppState, user_id: i64, kind: &str) -> AppResult<Option<NrzGitAccess>> {
    Ok(forge_accesses(state, user_id).await?.into_iter().find(|a| a.kind == kind))
}

/// Verbindung des Benutzers zu nrzGit (`None`, wenn nicht verbunden).
pub async fn nrzgit_access(state: &AppState, user_id: i64) -> AppResult<Option<NrzGitAccess>> {
    Ok(forge_accesses(state, user_id).await?.into_iter().next())
}

/// Zugang zur Plattform eines Projekts (`None` bei lokalen Projekten oder ohne Verbindung).
pub async fn project_access(state: &AppState, p: &model::ProjectRow) -> AppResult<Option<NrzGitAccess>> {
    if !p.is_nrzgit() {
        return Ok(None);
    }
    forge_access(state, p.user_id, p.forge_kind()).await
}

/// Mit `@@name` im Prompt einbezogenes Repository (ein anderes Projekt des Benutzers) – der Agent
/// liest es als Referenz, z. B. um zwei Projekte ineinander zu integrieren.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoRef {
    pub name: String,
    /// `owner/name` auf der Git-Plattform (leer bei lokalen Projekten)
    pub repo: String,
    /// Hauptcheckout des Projekts
    pub path: PathBuf,
    pub branch: String,
}

/// Höchstzahl der Repositories je Nachricht
const MAX_REPO_REFS: usize = 8;

/// `@@name`-Wörter im Text (ohne `@@`, Satzzeichen am Ende weg).
pub fn repo_tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let Some(rest) = word.strip_prefix("@@") else { continue };
        let name = rest.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"', '\'', '`']);
        if !name.is_empty() && !out.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
        }
    }
    out
}

/// Name, wie ihn `@@` schreibt: klein, Leerzeichen als `-`.
pub fn repo_token_key(name: &str) -> String {
    name.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join("-")
}

/// Projekte hinter `@@name` (Name des Projekts, `owner/name` oder nur der Repository-Name),
/// ohne das eigene Projekt des Threads. Wird ein Projekt noch geklont, wird kurz gewartet; nicht
/// gefundene oder fehlerhafte fallen weg.
pub async fn referenced_repos(state: &AppState, user_id: i64, text: &str, own_project: Option<i64>) -> AppResult<Vec<RepoRef>> {
    let tokens = repo_tokens(text);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let projects: Vec<(i64, String, String)> = sqlx::query_as("SELECT id, name, repo FROM projects WHERE user_id = ? ORDER BY id").bind(user_id).fetch_all(&state.db).await?;
    let mut ids: Vec<i64> = Vec::new();
    for tok in tokens {
        let key = repo_token_key(&tok);
        let hit = projects
            .iter()
            .find(|(_, name, repo)| repo_token_key(name) == key || (!repo.is_empty() && repo.to_lowercase() == key))
            .or_else(|| projects.iter().find(|(_, _, repo)| repo.rsplit('/').next().is_some_and(|n| !n.is_empty() && n.to_lowercase() == key)));
        if let Some((id, _, _)) = hit {
            if Some(*id) != own_project && !ids.contains(id) {
                ids.push(*id);
            }
        }
    }
    let mut out = Vec::new();
    for id in ids.into_iter().take(MAX_REPO_REFS) {
        let Ok(p) = gitops::wait_ready(state, id).await else { continue };
        let path = model::repo_dir(state, id);
        let branch = gitops::current_branch(state, &path).await.unwrap_or_default();
        out.push(RepoRef { name: p.name.clone(), repo: p.repo.clone(), path, branch });
    }
    Ok(out)
}

/// Arbeitsordner eines Threads: eigener Worktree im Projekt (`workspaces/<projekt>/worktrees/<thread>`
/// auf Branch `nrz/<kurzname>-<id>` vom Standard-Branch, beim ersten Aufruf angelegt), der
/// Hauptcheckout des Projekts (ohne Worktree) oder (ohne Projekt) `scratch/<thread>`.
pub async fn thread_workdir(state: &AppState, thread_id: i64) -> AppResult<PathBuf> {
    let t = model::thread(&state.db, thread_id).await?.ok_or(AppError::NotFound)?;
    gitops::workdir(state, &t).await
}

/// Zusätzliche Umgebung für Agent-Prozesse eines Benutzers: Git-Anmeldung bei nrzGit
/// (`GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n` mit `http.<url>/.extraHeader`),
/// Autor und Committer aus dem Konto, `NRZGIT_URL`/`NRZGIT_USERNAME`/`NRZGIT_TOKEN`/`NRZGIT_REGISTRY`
/// und `DOCKER_CONFIG` (Anmeldung an der Container-Registry von nrzGit).
pub async fn agent_env(state: &AppState, user_id: i64) -> AppResult<Vec<(String, String)>> {
    let mut env: Vec<(String, String)> = vec![("GIT_TERMINAL_PROMPT".into(), "0".into())];
    if let Some(user) = crate::db::user_by_id(&state.db, user_id).await? {
        let (name, email) = gitops::identity(&user);
        env.push(("GIT_AUTHOR_NAME".into(), name.clone()));
        env.push(("GIT_AUTHOR_EMAIL".into(), email.clone()));
        env.push(("GIT_COMMITTER_NAME".into(), name));
        env.push(("GIT_COMMITTER_EMAIL".into(), email));
    }
    let accesses = forge_accesses(state, user_id).await?;
    if !accesses.is_empty() {
        env.extend(gitops::auth_config(&accesses));
        for access in &accesses {
            // NRZGIT_*
            let prefix = access.server_name().to_uppercase();
            env.push((format!("{prefix}_URL"), access.url.clone()));
            env.push((format!("{prefix}_USERNAME"), access.username.clone()));
            env.push((format!("{prefix}_TOKEN"), access.token.clone()));
            env.push((format!("{prefix}_REGISTRY"), nrzgit::registry_host(&access.url, &access.kind)));
        }
        let docker = gitops::docker_dir(state, user_id);
        if !docker.join("config.json").exists() {
            gitops::write_docker_config(state, user_id).await?;
        }
        env.push(("DOCKER_CONFIG".into(), docker.to_string_lossy().to_string()));
    }
    Ok(env)
}

/// Nach einem Lauf: Änderungen im Arbeitsordner zählen (`HubEvent::ChangesUpdated`) und die
/// Aktivität des Projekts vermerken.
pub async fn after_run(state: &AppState, thread_id: i64) -> AppResult<()> {
    let Some(t) = model::thread(&state.db, thread_id).await? else {
        return Ok(());
    };
    if let Some(pid) = t.project_id {
        model::touch_project(&state.db, pid).await?;
        model::publish_project(state, pid).await?;
    }
    gitops::publish_changes(state, &t).await?;
    Ok(())
}

/// Bevor ein Thread gelöscht wird: seinen Worktree (falls vorhanden) entfernen. Der Branch
/// bleibt im Projekt-Repository, damit keine Arbeit verloren geht.
pub async fn forget_thread(state: &AppState, thread_id: i64) -> AppResult<()> {
    if let Some(t) = model::thread(&state.db, thread_id).await? {
        if t.project_id.is_some() && !t.worktree_path.is_empty() {
            gitops::remove_worktree(state, &t).await?;
        }
    }
    let scratch = state.config.scratch_dir().join(thread_id.to_string());
    if scratch.exists() {
        let _ = tokio::fs::remove_dir_all(&scratch).await;
    }
    Ok(())
}
