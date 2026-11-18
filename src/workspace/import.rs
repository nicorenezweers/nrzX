//! Übernahme aus nrzCode: Agent-Konten (mit ihren Anmeldungen), die Verbindung zu nrzGit und die
//! Projekte – damit man sich in nrzX nicht überall neu anmelden muss.
//!
//! Gelesen wird die Datenbank von nrzCode (`<nrzCode>/nrzcode.db`, nur lesend; Vorgabe
//! `~/Library/Application Support/nrzCode`, sonst `NRZX_NRZCODE_DIR`) für den Benutzer mit gleichem
//! Namen, sonst den ersten Administrator. Aus den Homes der Konten kommen nur die Dateien der
//! Anmeldung und Einstellungen der CLIs mit (nicht Caches, Toolchains oder Code – die Homes sind
//! oft viele Gigabyte gross). Projekte aus nrzGit werden nacheinander frisch geklont, lokale kopiert.
//!
//! Routen: `GET /import/nrzcode` (was es gibt), `POST /import/nrzcode` (übernehmen).

use super::{model, projects};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::workspace::{NrzCodeAccount, NrzCodeImportPreview, NrzCodeImportRequest, NrzCodeImportResult, NrzCodeProject};
use nrzx_shared::AgentKind;
use rocket::serde::json::Json;
use rocket::{get, post, routes, Route, State};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

pub fn routes() -> Vec<Route> {
    routes![preview, run]
}

/// Dateien und Ordner der Anmeldung je CLI (relativ zum Home des Kontos).
const LOGIN_FILES: &[&str] = &[
    // Claude Code (CLAUDE_CONFIG_DIR = ~/.claude)
    ".claude/.credentials.json",
    ".claude/.claude.json",
    ".claude/settings.json",
    ".claude.json",
    // Codex (CODEX_HOME = ~/.codex)
    ".codex/auth.json",
    ".codex/config.toml",
    // Gemini CLI
    ".gemini/oauth_creds.json",
    ".gemini/google_accounts.json",
    ".gemini/settings.json",
    ".gemini/installation_id",
    // OpenCode
    ".local/share/opencode/auth.json",
    ".config/opencode",
    // Kilo Code, Grok CLI
    ".config/kilo",
    ".kilocode/cli/config.json",
    ".grok/user-settings.json",
    ".grok/settings.json",
];

/// Grösste Datei, die beim Kopieren der Anmeldung mitkommt.
const MAX_FILE: u64 = 4 * 1024 * 1024;

/// Datenordner von nrzCode: `NRZX_NRZCODE_DIR`, sonst unter `HOME`, sonst im Benutzerordner des
/// angemeldeten Benutzers (falls `HOME` umgebogen ist, z. B. in einem Agenten).
fn nrzcode_dir() -> PathBuf {
    if let Ok(d) = std::env::var("NRZX_NRZCODE_DIR") {
        return PathBuf::from(d);
    }
    let rel = "Library/Application Support/nrzCode";
    let mut candidates: Vec<PathBuf> = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rel)).into_iter().collect();
    if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")) {
        candidates.push(PathBuf::from("/Users").join(user).join(rel));
    }
    candidates.iter().find(|d| d.join("nrzcode.db").is_file()).cloned().unwrap_or_else(|| candidates.into_iter().next().unwrap_or_default())
}

/// Datenbank von nrzCode nur lesend öffnen (`None`, wenn es keine gibt).
async fn open(dir: &Path) -> Option<SqlitePool> {
    let db = dir.join("nrzcode.db");
    if !db.is_file() {
        return None;
    }
    let opts = SqliteConnectOptions::new().filename(&db).read_only(true).create_if_missing(false);
    SqlitePool::connect_with(opts).await.ok()
}

/// Benutzer in nrzCode: gleicher Name, sonst der erste Administrator, sonst der erste.
async fn source_user(db: &SqlitePool, username: &str) -> Option<i64> {
    let same: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE lower(username) = lower(?)").bind(username).fetch_optional(db).await.ok().flatten();
    if same.is_some() {
        return same;
    }
    sqlx::query_scalar("SELECT id FROM users ORDER BY is_admin DESC, id LIMIT 1").fetch_optional(db).await.ok().flatten()
}

type SrcAccount = (i64, String, String, String, String, String, String, String);
type SrcProject = (i64, String, String, String, String);

async fn src_accounts(db: &SqlitePool, user: i64) -> Vec<SrcAccount> {
    sqlx::query_as("SELECT id, kind, label, auth_kind, secret, secret_env, default_model, default_effort FROM agent_accounts WHERE user_id = ? ORDER BY id")
        .bind(user)
        .fetch_all(db)
        .await
        .unwrap_or_default()
}

async fn src_projects(db: &SqlitePool, user: i64) -> Vec<SrcProject> {
    sqlx::query_as("SELECT id, name, repo, clone_url, default_branch FROM projects WHERE user_id = ? AND status = 'ready' ORDER BY last_activity_at DESC")
        .bind(user)
        .fetch_all(db)
        .await
        .unwrap_or_default()
}

async fn src_nrzgit(db: &SqlitePool, user: i64) -> Option<(String, String, String, String)> {
    sqlx::query_as("SELECT url, token, username, public_url FROM nrzgit_connections WHERE user_id = ? AND kind = 'nrzgit' AND verified_at IS NOT NULL")
        .bind(user)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

/// Konto mit gleicher Art und gleichem Namen (falls es das schon gibt).
async fn account_id(state: &AppState, user: i64, kind: &str, label: &str) -> AppResult<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT id FROM agent_accounts WHERE user_id = ? AND kind = ? AND lower(label) = lower(?) ORDER BY id LIMIT 1")
        .bind(user)
        .bind(kind)
        .bind(label)
        .fetch_optional(&state.db)
        .await?)
}

async fn account_exists(state: &AppState, user: i64, kind: &str, label: &str) -> AppResult<bool> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_accounts WHERE user_id = ? AND kind = ? AND lower(label) = lower(?)")
        .bind(user)
        .bind(kind)
        .bind(label)
        .fetch_one(&state.db)
        .await?
        > 0)
}

async fn project_exists(state: &AppState, user: i64, name: &str, repo: &str) -> AppResult<bool> {
    let n: i64 = if repo.is_empty() {
        sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE user_id = ? AND repo = '' AND lower(name) = lower(?)").bind(user).bind(name).fetch_one(&state.db).await?
    } else {
        sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE user_id = ? AND lower(repo) = lower(?)").bind(user).bind(repo).fetch_one(&state.db).await?
    };
    Ok(n > 0)
}

async fn has_nrzgit(state: &AppState, user: i64) -> AppResult<bool> {
    Ok(super::nrzgit_access(state, user).await?.is_some())
}

/// `GET /import/nrzcode` – was nrzCode auf diesem Rechner hat.
#[get("/import/nrzcode")]
async fn preview(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<NrzCodeImportPreview>> {
    let dir = nrzcode_dir();
    let Some(db) = open(&dir).await else {
        return Ok(Json(NrzCodeImportPreview::default()));
    };
    let me = crate::db::user_by_id(&state.db, user.id()).await?.ok_or(AppError::NotFound)?;
    let Some(src) = source_user(&db, &me.username).await else {
        return Ok(Json(NrzCodeImportPreview::default()));
    };
    let mut out = NrzCodeImportPreview { found: true, path: dir.to_string_lossy().into_owned(), ..Default::default() };
    for (_, kind, label, ..) in src_accounts(&db, src).await {
        let Some(k) = AgentKind::parse(&kind) else { continue };
        out.accounts.push(NrzCodeAccount { exists: account_exists(state, user.id(), &kind, &label).await?, kind: k, label });
    }
    out.nrzgit = src_nrzgit(&db, src).await.map(|c| c.0);
    out.nrzgit_exists = has_nrzgit(state, user.id()).await?;
    for (_, name, repo, ..) in src_projects(&db, src).await {
        out.projects.push(NrzCodeProject { exists: project_exists(state, user.id(), &name, &repo).await?, name, repo });
    }
    db.close().await;
    Ok(Json(out))
}

/// `POST /import/nrzcode` – übernehmen, was gewählt ist (schon Vorhandenes wird übersprungen).
#[post("/import/nrzcode", data = "<body>")]
async fn run(state: &State<AppState>, user: CurrentUser, body: Json<NrzCodeImportRequest>) -> AppResult<Json<NrzCodeImportResult>> {
    let dir = nrzcode_dir();
    let db = open(&dir).await.ok_or_else(|| AppError::bad("Kein nrzCode auf diesem Rechner gefunden."))?;
    let me = crate::db::user_by_id(&state.db, user.id()).await?.ok_or(AppError::NotFound)?;
    let src = source_user(&db, &me.username).await.ok_or_else(|| AppError::bad("In nrzCode gibt es kein Konto."))?;
    let uid = user.id();
    let mut out = NrzCodeImportResult::default();

    if body.accounts {
        let has_default: bool = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_accounts WHERE user_id = ? AND is_default = 1").bind(uid).fetch_one(&state.db).await? > 0;
        let mut first = !has_default;
        for (old_id, kind, label, auth_kind, secret, secret_env, model, effort) in src_accounts(&db, src).await {
            if AgentKind::parse(&kind).is_none() {
                continue;
            }
            let from = dir.join("accounts").join(old_id.to_string()).join("home");
            // Gibt es das Konto schon: Anmeldung frisch aus nrzCode holen und verknüpfen
            if let Some(existing) = account_id(state, uid, &kind, &label).await? {
                let to = crate::agents::accounts::home(state, existing);
                let src = from.clone();
                // Zuerst abgleichen (die neuere Anmeldung gewinnt, auch eine aus nrzX), dann den Rest holen
                let copied = tokio::task::spawn_blocking(move || {
                    crate::agents::accounts::sync_files(&src, &to);
                    copy_login(&src, &to)
                })
                .await
                .unwrap_or(0);
                sqlx::query("UPDATE agent_accounts SET linked_home = ?, status = 'unknown', status_message = '', updated_at = ? WHERE id = ?")
                    .bind(from.to_string_lossy().into_owned())
                    .bind(now())
                    .bind(existing)
                    .execute(&state.db)
                    .await?;
                tracing::info!("nrzCode: Anmeldung von „{label}“ aufgefrischt ({copied} Dateien), verknüpft");
                out.refreshed += 1;
                continue;
            }
            let ts = now();
            let new_id: i64 = sqlx::query_scalar(
                "INSERT INTO agent_accounts (user_id, kind, label, auth_kind, secret, secret_env, default_model, default_effort, is_default, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(uid)
            .bind(&kind)
            .bind(&label)
            .bind(&auth_kind)
            .bind(&secret)
            .bind(&secret_env)
            .bind(&model)
            .bind(&effort)
            .bind(first)
            .bind(ts)
            .bind(ts)
            .fetch_one(&state.db)
            .await?;
            first = false;
            // Verknüpft: die Anmeldung bleibt mit nrzCode im Abgleich (die neuere gewinnt)
            sqlx::query("UPDATE agent_accounts SET linked_home = ? WHERE id = ?").bind(from.to_string_lossy().into_owned()).bind(new_id).execute(&state.db).await?;
            let to = crate::agents::accounts::home(state, new_id);
            let copied = tokio::task::spawn_blocking(move || copy_login(&from, &to)).await.unwrap_or(0);
            tracing::info!("nrzCode: Konto „{label}“ übernommen ({copied} Dateien der Anmeldung)");
            out.accounts += 1;
        }
    }

    if body.nrzgit {
        match src_nrzgit(&db, src).await {
            Some(_) if has_nrzgit(state, uid).await? => out.skipped.push("nrzGit ist schon verbunden".into()),
            Some((url, token, username, public_url)) => {
                let ts = now();
                sqlx::query(
                    "INSERT INTO nrzgit_connections (user_id, kind, url, token, username, verified_at, last_error, public_url, has_mcp, created_at, updated_at) \
                     VALUES (?, 'nrzgit', ?, ?, ?, ?, '', ?, 1, ?, ?) ON CONFLICT (user_id, kind) DO UPDATE SET url = excluded.url, token = excluded.token, \
                     username = excluded.username, verified_at = excluded.verified_at, last_error = '', public_url = excluded.public_url, updated_at = excluded.updated_at",
                )
                .bind(uid)
                .bind(&url)
                .bind(&token)
                .bind(&username)
                .bind(ts)
                .bind(&public_url)
                .bind(ts)
                .bind(ts)
                .execute(&state.db)
                .await?;
                super::gitops::write_docker_config(state, uid).await?;
                out.nrzgit = true;
            }
            None => out.skipped.push("nrzCode ist nicht mit nrzGit verbunden".into()),
        }
    }

    if body.projects {
        let can_clone = has_nrzgit(state, uid).await?;
        let mut clones = Vec::new();
        let mut copies = Vec::new();
        for (old_id, name, repo, clone_url, branch) in src_projects(&db, src).await {
            if project_exists(state, uid, &name, &repo).await? {
                out.skipped.push(format!("Projekt „{name}“ gibt es schon"));
                continue;
            }
            if repo.is_empty() {
                let p = projects::insert(state, uid, &name, "", "", "", &branch, "cloning").await?;
                copies.push((p.id, dir.join("workspaces").join(old_id.to_string()).join("repo")));
            } else if !can_clone {
                out.skipped.push(format!("Projekt „{name}“: erst nrzGit verbinden"));
                continue;
            } else {
                let p = projects::insert(state, uid, &name, &repo, super::nrzgit::KIND_NRZGIT, &clone_url, &branch, "cloning").await?;
                clones.push(p.id);
            }
            out.projects += 1;
        }
        // Im Hintergrund nacheinander (nicht alle Klone auf einmal)
        let st = state.inner().clone();
        tokio::spawn(async move {
            for (id, from) in copies {
                let to = model::repo_dir(&st, id);
                let ok = tokio::task::spawn_blocking(move || copy_tree(&from, &to)).await.unwrap_or(false);
                let _ = projects::set_status(&st, id, if ok { "ready" } else { "error" }, if ok { "" } else { "Kopieren aus nrzCode fehlgeschlagen" }).await;
            }
            for id in clones {
                projects::clone_now(st.clone(), id).await;
            }
        });
    }
    db.close().await;
    Ok(Json(out))
}

/// Dateien der Anmeldung aus einem Home ins andere kopieren; Anzahl der Dateien.
fn copy_login(from: &Path, to: &Path) -> usize {
    let mut n = 0;
    for rel in LOGIN_FILES {
        let src = from.join(rel);
        let dst = to.join(rel);
        if src.is_dir() {
            n += copy_small_dir(&src, &dst);
        } else if src.is_file() && std::fs::metadata(&src).is_ok_and(|m| m.len() <= MAX_FILE) {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(&src, &dst).is_ok() {
                restrict(&dst);
                n += 1;
            }
        }
    }
    n
}

/// Kleinen Konfigurationsordner kopieren (ohne grosse Dateien, Logs und Sitzungen).
fn copy_small_dir(from: &Path, to: &Path) -> usize {
    let mut n = 0;
    let Ok(entries) = std::fs::read_dir(from) else { return 0 };
    let _ = std::fs::create_dir_all(to);
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), "node_modules" | "logs" | "log" | "cache" | "sessions" | "projects" | "snapshot") {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            n += copy_small_dir(&path, &to.join(&name));
        } else if std::fs::metadata(&path).is_ok_and(|m| m.len() <= MAX_FILE) && std::fs::copy(&path, to.join(&name)).is_ok() {
            restrict(&to.join(&name));
            n += 1;
        }
    }
    n
}

/// Anmeldedaten nur für den Besitzer lesbar.
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Ordner vollständig kopieren (lokale Projekte, mit `.git`).
fn copy_tree(from: &Path, to: &Path) -> bool {
    if !from.is_dir() {
        return false;
    }
    if std::fs::create_dir_all(to).is_err() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(from) else { return false };
    for e in entries.flatten() {
        let path = e.path();
        let target = to.join(e.file_name());
        let ok = match e.file_type() {
            Ok(t) if t.is_dir() => copy_tree(&path, &target),
            Ok(t) if t.is_symlink() => std::fs::read_link(&path).is_ok_and(|l| symlink(&l, &target)),
            Ok(_) => std::fs::copy(&path, &target).is_ok(),
            Err(_) => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn symlink(link: &Path, target: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(link, target).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = (link, target);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_only_login_files() {
        let tmp = std::env::temp_dir().join(format!("nrzx-import-{}", std::process::id()));
        let (from, to) = (tmp.join("from"), tmp.join("to"));
        std::fs::create_dir_all(from.join(".claude/projects")).unwrap();
        std::fs::create_dir_all(from.join(".cargo")).unwrap();
        std::fs::write(from.join(".claude/.credentials.json"), "{}").unwrap();
        std::fs::write(from.join(".claude/projects/x.jsonl"), "log").unwrap();
        std::fs::write(from.join(".cargo/big"), "toolchain").unwrap();
        std::fs::create_dir_all(from.join(".config/opencode")).unwrap();
        std::fs::write(from.join(".config/opencode/opencode.json"), "{}").unwrap();
        assert_eq!(copy_login(&from, &to), 2);
        assert!(to.join(".claude/.credentials.json").is_file());
        assert!(!to.join(".claude/projects").exists() && !to.join(".cargo").exists());
        assert!(to.join(".config/opencode/opencode.json").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
