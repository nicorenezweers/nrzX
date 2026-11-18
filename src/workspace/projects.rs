//! Projekte: Repositories aus nrzGit übernehmen (im Hintergrund nach
//! `<data>/workspaces/<id>/repo` klonen, Status cloning → ready/error live per
//! `HubEvent::ProjectUpdated`), neue Repositories in nrzGit anlegen, lokale Projekte ohne nrzGit
//! (`git init`), aktualisieren (fetch/pull), umbenennen und löschen.
//!
//! Routen: `GET /projects`, `GET/PATCH/DELETE /projects/<id>`, `POST /projects/import`,
//! `POST /projects/create`, `POST /projects/local`, `POST /projects/<id>/refresh`.

use super::gitops::{self, run_auth, user_error};
use super::model::{self, check_len, ProjectRow};
use super::nrzgit::{self, Client, RemoteCreateRepo};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::git;
use crate::AppState;
use nrzx_shared::workspace::{CreateLocalProject, CreateRepoProject, EditProject, GitResult, ImportProject, ProjectDetail};
use nrzx_shared::{HubEvent, ProjectSummary};
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::{delete, get, patch, post, routes, Route, State};

pub fn routes() -> Vec<Route> {
    routes![list, detail, import, create, local, edit, refresh, remove]
}

/// `GET /projects` – Projekte des Benutzers, zuletzt aktive zuerst.
#[get("/projects")]
async fn list(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<Vec<ProjectSummary>>> {
    let rows: Vec<ProjectRow> = sqlx::query_as(
        "SELECT id, user_id, name, repo, forge, clone_url, default_branch, status, status_message, created_at, updated_at, last_activity_at \
         FROM projects WHERE user_id = ?",
    )
    .bind(user.id())
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for p in &rows {
        out.push(model::summary(&state.db, p).await?);
    }
    out.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at).then(a.name.cmp(&b.name)));
    Ok(Json(out))
}

/// `GET /projects/<id>` – Projekt mit Adresse in nrzGit, Ordner und letzten Commits.
#[get("/projects/<id>")]
async fn detail(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ProjectDetail>> {
    let p = model::own_project(&state.db, user.id(), id).await?;
    let repo = model::repo_dir(state, id);
    let ready = p.status == "ready" && gitops::is_git_dir(&repo);
    let branch = if ready { gitops::current_branch(state, &repo).await? } else { String::new() };
    let commits = if ready { git::log::commits(state, &repo, &["-n", "8", "HEAD", "--"]).await? } else { Vec::new() };
    let web_url = match super::project_access(state, &p).await? {
        Some(a) => format!("{}/{}", a.public_url, p.repo),
        None => String::new(),
    };
    Ok(Json(ProjectDetail {
        summary: model::summary(&state.db, &p).await?,
        web_url,
        clone_url: p.clone_url.clone(),
        branch,
        path: repo.to_string_lossy().to_string(),
        created_at: p.created_at,
        commits,
    }))
}

/// Projekt anlegen (Status `cloning` oder `ready`) und melden.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert(state: &AppState, user_id: i64, name: &str, repo: &str, forge: &str, clone_url: &str, default_branch: &str, status: &str) -> AppResult<ProjectRow> {
    let ts = now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO projects (user_id, name, repo, forge, clone_url, default_branch, status, created_at, updated_at, last_activity_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(repo)
    .bind(forge)
    .bind(clone_url)
    .bind(if default_branch.is_empty() { "main" } else { default_branch })
    .bind(status)
    .bind(ts)
    .bind(ts)
    .bind(ts)
    .fetch_one(&state.db)
    .await?;
    model::publish_project(state, id).await?;
    model::project(&state.db, id).await?.ok_or(AppError::NotFound)
}

pub(crate) async fn set_status(state: &AppState, id: i64, status: &str, message: &str) -> AppResult<()> {
    let message: String = message.chars().take(500).collect();
    sqlx::query("UPDATE projects SET status = ?, status_message = ?, updated_at = ? WHERE id = ?")
        .bind(status)
        .bind(message)
        .bind(now())
        .bind(id)
        .execute(&state.db)
        .await?;
    model::publish_project(state, id).await
}

/// Klonen im Hintergrund starten; meldet Fortschritt und Ergebnis live.
pub fn spawn_clone(state: AppState, project_id: i64) {
    tokio::spawn(clone_now(state, project_id));
}

/// Klonen und das Ergebnis melden (wartet, bis es fertig ist).
pub(crate) async fn clone_now(state: AppState, project_id: i64) {
    {
        let result = clone(&state, project_id).await;
        let Ok(Some(p)) = model::project(&state.db, project_id).await else { return };
        match result {
            Ok(()) => {
                let _ = set_status(&state, project_id, "ready", "").await;
                crate::hub::publish(&state, p.user_id, HubEvent::Notice { text: format!("Projekt „{}“ ist bereit.", p.name), error: false });
            }
            Err(e) => {
                let msg = match &e {
                    AppError::Internal(m) => m.clone(),
                    other => other.public_message(),
                };
                tracing::warn!("Projekt {project_id} klonen: {msg}");
                let _ = set_status(&state, project_id, "error", &msg).await;
                crate::hub::publish(
                    &state,
                    p.user_id,
                    HubEvent::Notice { text: format!("Projekt „{}“ konnte nicht geklont werden.", p.name), error: true },
                );
            }
        }
    }
}

/// Repository eines Projekts frisch klonen (Hauptcheckout).
async fn clone(state: &AppState, project_id: i64) -> AppResult<()> {
    let p = model::project(&state.db, project_id).await?.ok_or(AppError::NotFound)?;
    let lock = gitops::project_lock(project_id);
    let _guard = lock.lock().await;
    let dir = model::project_dir(state, project_id);
    let repo = model::repo_dir(state, project_id);
    tokio::fs::create_dir_all(&dir).await?;
    if repo.exists() {
        tokio::fs::remove_dir_all(&repo).await?;
    }
    let target = repo.to_string_lossy().to_string();
    let out = run_auth(state, &dir, p.user_id, &["clone", "--origin", "origin", "--", &p.clone_url, &target]).await?;
    if !out.ok() {
        return Err(user_error("Klonen", &out));
    }
    // Standard-Branch so, wie nrzGit ihn meldet (bzw. wie origin/HEAD zeigt)
    let head = git::run_with(state, &repo, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], None, &[]).await?;
    let remote_default = head.stdout_str().trim().strip_prefix("origin/").unwrap_or("").to_string();
    if git::is_empty(state, &repo).await? {
        // Leeres Repository: HEAD auf den Standard-Branch, damit der erste Commit dort landet
        let r = format!("refs/heads/{}", p.default_branch);
        git::run(state, &repo, &["symbolic-ref", "HEAD", &r]).await?;
    } else if !remote_default.is_empty() && remote_default != p.default_branch {
        sqlx::query("UPDATE projects SET default_branch = ? WHERE id = ?")
            .bind(&remote_default)
            .bind(project_id)
            .execute(&state.db)
            .await?;
    }
    Ok(())
}

/// Projekt, das dieses Repository der Plattform schon ist (Gross-/Kleinschreibung egal).
async fn existing_project(state: &AppState, user_id: i64, forge: &str, full_name: &str) -> AppResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM projects WHERE user_id = ? AND lower(repo) = lower(?) AND (CASE forge WHEN '' THEN 'nrzgit' ELSE forge END) = ?",
    )
    .bind(user_id)
    .bind(full_name.trim())
    .bind(forge)
    .fetch_optional(&state.db)
    .await?)
}

/// Schon übernommenes Repository: das bestehende Projekt statt eines Fehlers (die Oberfläche
/// wählt es dann einfach für den neuen Thread).
async fn existing_summary(state: &AppState, user_id: i64, id: i64) -> AppResult<(Status, Json<ProjectSummary>)> {
    let p = model::own_project(&state.db, user_id, id).await?;
    Ok((Status::Ok, Json(model::summary(&state.db, &p).await?)))
}

/// `POST /projects/import` – Repository aus nrzGit übernehmen; klont im Hintergrund.
#[post("/projects/import", data = "<body>")]
async fn import(state: &State<AppState>, user: CurrentUser, body: Json<ImportProject>) -> AppResult<(Status, Json<ProjectSummary>)> {
    let access = nrzgit::require(state, user.id(), nrzgit::parse_kind(Some(&body.forge))?).await?;
    nrzgit::split_repo(&body.repo)?;
    if let Some(id) = existing_project(state, user.id(), &access.kind, &body.repo).await? {
        return existing_summary(state, user.id(), id).await;
    }
    let remote = Client::of(&access).repo(body.repo.trim()).await?;
    if let Some(id) = existing_project(state, user.id(), &access.kind, &remote.full_name).await? {
        return existing_summary(state, user.id(), id).await;
    }
    let url = nrzgit::clone_url(&access.url, &remote);
    let p = insert(state, user.id(), &remote.name, &remote.full_name, &access.kind, &url, &remote.default_branch, "cloning").await?;
    spawn_clone(state.inner().clone(), p.id);
    Ok((Status::Created, Json(model::summary(&state.db, &p).await?)))
}

/// `POST /projects/create` – neues Repository in nrzGit anlegen und übernehmen.
#[post("/projects/create", data = "<body>")]
async fn create(state: &State<AppState>, user: CurrentUser, body: Json<CreateRepoProject>) -> AppResult<(Status, Json<ProjectSummary>)> {
    let access = nrzgit::require(state, user.id(), nrzgit::parse_kind(Some(&body.forge))?).await?;
    let name = body.name.trim();
    crate::web::check_name(name, "Name")?;
    let owner = if body.owner.trim().is_empty() { access.username.clone() } else { body.owner.trim().to_string() };
    let full = format!("{owner}/{name}");
    // Schon ein Projekt: dieses wählen; gibt es das Repository auf der Plattform schon,
    // übernehmen statt neu anlegen
    if let Some(id) = existing_project(state, user.id(), &access.kind, &full).await? {
        return existing_summary(state, user.id(), id).await;
    }
    if let Ok(remote) = Client::of(&access).repo(&full).await {
        if let Some(id) = existing_project(state, user.id(), &access.kind, &remote.full_name).await? {
            return existing_summary(state, user.id(), id).await;
        }
        let url = nrzgit::clone_url(&access.url, &remote);
        let p = insert(state, user.id(), &remote.name, &remote.full_name, &access.kind, &url, &remote.default_branch, "cloning").await?;
        spawn_clone(state.inner().clone(), p.id);
        return Ok((Status::Created, Json(model::summary(&state.db, &p).await?)));
    }
    let req = RemoteCreateRepo {
        owner: if owner == access.username { String::new() } else { owner.clone() },
        name: name.to_string(),
        description: check_len(&body.description, 500, "Beschreibung")?,
        private: Some(body.private),
        auto_init: body.readme,
    };
    let remote = Client::of(&access).create_repo(&req).await?;
    let url = nrzgit::clone_url(&access.url, &remote);
    let p = insert(state, user.id(), &remote.name, &remote.full_name, &access.kind, &url, &remote.default_branch, "cloning").await?;
    spawn_clone(state.inner().clone(), p.id);
    Ok((Status::Created, Json(model::summary(&state.db, &p).await?)))
}

/// `POST /projects/local` – lokales Projekt ohne nrzGit: `git init` mit README und erstem Commit.
#[post("/projects/local", data = "<body>")]
async fn local(state: &State<AppState>, user: CurrentUser, body: Json<CreateLocalProject>) -> AppResult<(Status, Json<ProjectSummary>)> {
    let name = check_len(&body.name, 100, "Name")?;
    if name.is_empty() {
        return Err(AppError::bad("Bitte einen Namen angeben."));
    }
    let p = insert(state, user.id(), &name, "", "", "", "main", "cloning").await?;
    let repo = model::repo_dir(state, p.id);
    let result: AppResult<()> = async {
        tokio::fs::create_dir_all(&repo).await?;
        git::run(state, &repo, &["init", "--quiet", "--initial-branch=main"]).await?;
        tokio::fs::write(repo.join("README.md"), format!("# {name}\n")).await?;
        git::run(state, &repo, &["add", "README.md"]).await?;
        let out = run_auth(state, &repo, user.id(), &["commit", "--quiet", "-m", "Projekt angelegt"]).await?;
        if !out.ok() {
            return Err(user_error("Erster Commit", &out));
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => set_status(state, p.id, "ready", "").await?,
        Err(e) => {
            set_status(state, p.id, "error", &e.public_message()).await?;
            return Err(e);
        }
    }
    let p = model::own_project(&state.db, user.id(), p.id).await?;
    Ok((Status::Created, Json(model::summary(&state.db, &p).await?)))
}

/// `PATCH /projects/<id>` – umbenennen (nur in nrzX).
#[patch("/projects/<id>", data = "<body>")]
async fn edit(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<EditProject>) -> AppResult<Json<ProjectSummary>> {
    model::own_project(&state.db, user.id(), id).await?;
    let name = check_len(&body.name, 100, "Name")?;
    if name.is_empty() {
        return Err(AppError::bad("Bitte einen Namen angeben."));
    }
    sqlx::query("UPDATE projects SET name = ?, updated_at = ? WHERE id = ?").bind(&name).bind(now()).bind(id).execute(&state.db).await?;
    model::publish_project(state, id).await?;
    // Threads tragen den Projektnamen in der Seitenleiste
    let threads: Vec<i64> = sqlx::query_scalar("SELECT id FROM threads WHERE project_id = ?").bind(id).fetch_all(&state.db).await?;
    for t in threads {
        let _ = crate::agents::publish_thread(state, t).await;
    }
    let p = model::own_project(&state.db, user.id(), id).await?;
    Ok(Json(model::summary(&state.db, &p).await?))
}

/// `POST /projects/<id>/refresh` – von origin holen und den Hauptcheckout vorspulen (nur wenn
/// er sauber auf dem Standard-Branch steht). Nach einem Fehler beim Klonen: neu klonen.
#[post("/projects/<id>/refresh")]
async fn refresh(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<GitResult>> {
    let p = model::own_project(&state.db, user.id(), id).await?;
    let repo = model::repo_dir(state, id);
    if p.status == "cloning" {
        return Err(AppError::conflict("Das Projekt wird noch geklont."));
    }
    if p.status == "error" || !gitops::is_git_dir(&repo) {
        if p.clone_url.is_empty() {
            return Err(AppError::conflict("Das lokale Repository fehlt."));
        }
        set_status(state, id, "cloning", "").await?;
        spawn_clone(state.inner().clone(), id);
        return Ok(Json(GitResult { message: "Das Projekt wird neu geklont.".into(), ..Default::default() }));
    }
    let has_remote = git::run_ok(state, &repo, &["remote", "get-url", "origin"]).await?;
    if !has_remote {
        return Ok(Json(GitResult { message: "Lokales Projekt – nichts zu holen.".into(), ..Default::default() }));
    }
    let lock = gitops::project_lock(id);
    let _guard = lock.lock().await;
    let out = run_auth(state, &repo, user.id(), &["fetch", "--prune", "origin"]).await?;
    if !out.ok() {
        return Err(user_error("Holen", &out));
    }
    let branch = gitops::current_branch(state, &repo).await?;
    let mut message = "Neueste Stände geholt.".to_string();
    let upstream = format!("refs/remotes/origin/{branch}");
    if !branch.is_empty() && git::rev_parse(state, &repo, &upstream).await?.is_some() {
        let dirty = !git::run_str(state, &repo, &["status", "--porcelain"]).await?.trim().is_empty();
        let before = git::rev_parse(state, &repo, "HEAD").await?;
        if !dirty {
            let ff = git::run_with(state, &repo, &["merge", "--ff-only", "--quiet", &upstream], None, &[]).await?;
            let after = git::rev_parse(state, &repo, "HEAD").await?;
            message = if !ff.ok() {
                format!("Geholt – „{branch}“ lässt sich nicht vorspulen (eigene Commits).")
            } else if before != after {
                format!("„{branch}“ wurde aktualisiert.")
            } else {
                "Schon auf dem neuesten Stand.".into()
            };
        } else {
            message = format!("Geholt – „{branch}“ hat ungespeicherte Änderungen und bleibt, wie er ist.");
        }
    }
    model::touch_project(&state.db, id).await?;
    model::publish_project(state, id).await?;
    Ok(Json(GitResult { message, branch, ..Default::default() }))
}

/// `DELETE /projects/<id>` – Projekt mit allen Threads und Arbeitsordnern löschen (das
/// Repository in nrzGit bleibt). Laufende Agenten werden vorher angehalten.
#[delete("/projects/<id>")]
async fn remove(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Status> {
    model::own_project(&state.db, user.id(), id).await?;
    let threads: Vec<i64> = sqlx::query_scalar("SELECT id FROM threads WHERE project_id = ?").bind(id).fetch_all(&state.db).await?;
    for t in &threads {
        if let Err(e) = crate::agents::stop_thread(state, *t).await {
            tracing::warn!("Thread {t} anhalten: {e}");
        }
    }
    let lock = gitops::project_lock(id);
    let _guard = lock.lock().await;
    sqlx::query("DELETE FROM projects WHERE id = ?").bind(id).execute(&state.db).await?;
    let dir = model::project_dir(state, id);
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir).await?;
    }
    for t in threads {
        crate::hub::publish(state, user.id(), HubEvent::ThreadDeleted { id: t });
    }
    crate::hub::publish(state, user.id(), HubEvent::ProjectDeleted { id });
    Ok(Status::NoContent)
}

/// Nach einem Neustart: abgebrochenes Klonen wieder aufnehmen.
pub async fn resume(state: AppState) {
    let ids: Vec<i64> = match sqlx::query_scalar("SELECT id FROM projects WHERE status = 'cloning' AND clone_url <> ''").fetch_all(&state.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Projekte prüfen: {e}");
            return;
        }
    };
    for id in ids {
        spawn_clone(state.clone(), id);
    }
}
