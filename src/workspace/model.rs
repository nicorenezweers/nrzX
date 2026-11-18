//! Zeilen aus der Datenbank (Projekte, Threads), Rechteprüfung und Ordner des Arbeitsbereichs.

use crate::db::{self, User};
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::{Me, ProjectSummary};
use sqlx::{FromRow, SqlitePool};
use std::path::PathBuf;

/// Zeile aus `projects`.
#[derive(Debug, Clone, Default, FromRow)]
pub struct ProjectRow {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub repo: String,
    /// Plattform des Repositorys: `nrzgit` (leer bei lokalen Projekten)
    pub forge: String,
    pub clone_url: String,
    pub default_branch: String,
    pub status: String,
    pub status_message: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_activity_at: i64,
}

impl ProjectRow {
    /// Projekt aus nrzGit (sonst lokal).
    pub fn is_nrzgit(&self) -> bool {
        !self.repo.is_empty()
    }

    /// Plattform des Projekts (`nrzgit`, wenn bei alten Zeilen nichts vermerkt ist).
    pub fn forge_kind(&self) -> &str {
        if self.forge.is_empty() {
            super::nrzgit::KIND_NRZGIT
        } else {
            &self.forge
        }
    }
}

/// Die Felder von `threads`, die der Arbeitsbereich braucht.
#[derive(Debug, Clone, Default, FromRow)]
pub struct ThreadRow {
    pub id: i64,
    pub user_id: i64,
    pub project_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub title: String,
    pub status: String,
    pub branch: String,
    pub use_worktree: bool,
    pub worktree_path: String,
}

impl ThreadRow {
    /// Arbeitet gerade ein Agent darin?
    pub fn busy(&self) -> bool {
        matches!(self.status.as_str(), "working" | "waiting")
    }
}

const THREAD_COLUMNS: &str = "id, user_id, project_id, parent_id, title, status, branch, use_worktree, worktree_path";

pub async fn project(db: &SqlitePool, id: i64) -> AppResult<Option<ProjectRow>> {
    Ok(sqlx::query_as("SELECT id, user_id, name, repo, forge, clone_url, default_branch, status, status_message, created_at, updated_at, last_activity_at FROM projects WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?)
}

/// Projekt des Benutzers – fremde Projekte gibt es für ihn nicht (404).
pub async fn own_project(db: &SqlitePool, user_id: i64, id: i64) -> AppResult<ProjectRow> {
    match project(db, id).await? {
        Some(p) if p.user_id == user_id => Ok(p),
        _ => Err(AppError::NotFound),
    }
}

pub async fn thread(db: &SqlitePool, id: i64) -> AppResult<Option<ThreadRow>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {THREAD_COLUMNS} FROM threads WHERE id = ?")))
        .bind(id)
        .fetch_optional(db)
        .await?)
}

/// Thread des Benutzers – fremde Threads gibt es für ihn nicht (404).
pub async fn own_thread(db: &SqlitePool, user_id: i64, id: i64) -> AppResult<ThreadRow> {
    match thread(db, id).await? {
        Some(t) if t.user_id == user_id => Ok(t),
        _ => Err(AppError::NotFound),
    }
}

/// `<data>/workspaces/<projekt>`
pub fn project_dir(state: &AppState, project_id: i64) -> PathBuf {
    state.config.workspaces_dir().join(project_id.to_string())
}

/// `<data>/workspaces/<projekt>/repo` – Hauptcheckout.
pub fn repo_dir(state: &AppState, project_id: i64) -> PathBuf {
    project_dir(state, project_id).join("repo")
}

/// `<data>/workspaces/<projekt>/worktrees/<thread>`
pub fn worktree_dir(state: &AppState, project_id: i64, thread_id: i64) -> PathBuf {
    project_dir(state, project_id).join("worktrees").join(thread_id.to_string())
}

/// `<data>/scratch/<thread>`
pub fn scratch_dir(state: &AppState, thread_id: i64) -> PathBuf {
    state.config.scratch_dir().join(thread_id.to_string())
}

/// Projekt für Listen und Ereignisse.
pub async fn summary(db: &SqlitePool, p: &ProjectRow) -> AppResult<ProjectSummary> {
    let (threads, last): (i64, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*), MAX(last_activity_at) FROM threads WHERE project_id = ? AND parent_id IS NULL",
    )
    .bind(p.id)
    .fetch_one(db)
    .await?;
    Ok(ProjectSummary {
        id: p.id,
        name: p.name.clone(),
        repo: p.repo.clone(),
        forge: if p.is_nrzgit() { p.forge_kind().to_string() } else { String::new() },
        default_branch: p.default_branch.clone(),
        status: p.status.clone(),
        status_message: p.status_message.clone(),
        threads,
        last_activity_at: p.last_activity_at.max(last.unwrap_or(0)),
    })
}

/// Projekt neu laden und als `HubEvent::ProjectUpdated` an seinen Benutzer senden.
pub async fn publish_project(state: &AppState, project_id: i64) -> AppResult<()> {
    if let Some(p) = project(&state.db, project_id).await? {
        let summary = summary(&state.db, &p).await?;
        crate::hub::publish(state, p.user_id, nrzx_shared::HubEvent::ProjectUpdated { project: summary });
    }
    Ok(())
}

/// Letzte Aktivität eines Projekts vermerken.
pub async fn touch_project(db: &SqlitePool, project_id: i64) -> AppResult<()> {
    sqlx::query("UPDATE projects SET last_activity_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(project_id)
        .execute(db)
        .await?;
    Ok(())
}

/// `Me` für die Oberfläche (mit Stand der nrzGit-Verbindung).
pub async fn me_json(db: &SqlitePool, user: &User) -> AppResult<Me> {
    // Geprüfte Verbindungen, nrzGit zuerst; für Links im Browser die öffentliche Adresse
    let conns: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT kind, url, public_url FROM nrzgit_connections WHERE user_id = ? AND verified_at IS NOT NULL \
         AND kind = 'nrzgit'",
    )
    .bind(user.id)
    .fetch_all(db)
    .await?;
    let forges: Vec<nrzx_shared::ForgeRef> = conns
        .into_iter()
        .map(|(kind, url, public_url)| nrzx_shared::ForgeRef { kind, url: if public_url.is_empty() { url } else { public_url } })
        .collect();
    let (connected, url) = (!forges.is_empty(), forges.first().map(|f| f.url.clone()).unwrap_or_default());
    Ok(Me {
        id: user.id,
        username: user.username.clone(),
        full_name: user.full_name.clone(),
        email: user.email.clone(),
        avatar_url: db::avatar_url(&user.username, user.avatar_at),
        custom_avatar: user.avatar_at > 0,
        is_admin: user.is_admin,
        language: user.language.clone(),
        theme: if user.theme.is_empty() { "dark".into() } else { user.theme.clone() },
        nrzgit_connected: connected,
        nrzgit_url: url,
        forges,
    })
}

/// Kurzname für Branches: Kleinbuchstaben, Ziffern und `-`, höchstens 32 Zeichen.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'ä' => 'a',
            'ö' => 'o',
            'ü' => 'u',
            'ß' => 's',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 32 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "thread".into()
    } else {
        out
    }
}

/// Länge prüfen und Text bereinigt zurückgeben.
pub fn check_len(text: &str, max: usize, what: &str) -> AppResult<String> {
    let t = text.trim();
    if t.chars().count() > max {
        return Err(AppError::bad(format!("{what}: höchstens {max} Zeichen.")));
    }
    Ok(t.to_string())
}

/// E-Mail-Adresse grob prüfen.
pub fn check_email(email: &str) -> AppResult<String> {
    let e = email.trim();
    let ok = e.len() <= 254
        && !e.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | ','))
        && e.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.'));
    if !ok {
        return Err(AppError::bad("Bitte eine gültige E-Mail-Adresse angeben."));
    }
    Ok(e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn slugs() {
        assert_eq!(super::slug("Login-Seite überarbeiten!"), "login-seite-uberarbeiten");
        assert_eq!(super::slug("  "), "thread");
        assert!(super::slug(&"x".repeat(100)).len() <= 32);
    }
}
