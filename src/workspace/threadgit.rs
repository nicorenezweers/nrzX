//! Git eines Threads (rechte Spalte der Thread-Seite): Zustand (Branch, ahead/behind,
//! Änderungen), Diff mit Syntaxfarben, Commit (alle oder ausgewählte Dateien), Push, Pull,
//! Branches auflisten/wechseln/anlegen, eigenen Worktree einrichten/entfernen und Pull Requests
//! in nrzGit anlegen.
//!
//! Routen unter `/workspace/<thread>/`: `GET status`, `GET diff?path=`, `POST commit`,
//! `POST push`, `POST pull`, `GET branches`, `POST branch`, `POST/DELETE worktree`,
//! `POST pull-request`.

use super::gitops::{self, run_auth, user_error};
use super::model::{self, ProjectRow, ThreadRow};
use super::nrzgit::{self, Client, RemoteCreatePull};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::git;
use crate::AppState;
use nrzx_shared::workspace::{
    BranchInfo, BranchList, BranchRequest, CommitRequest, CommitResult, GitResult, PullRequestRef, PullRequestRequest,
    WorkspaceStatus,
};
use nrzx_shared::Diff;
use rocket::serde::json::Json;
use rocket::{delete, get, post, routes, Route, State};
use std::path::{Path, PathBuf};

pub fn routes() -> Vec<Route> {
    routes![status, diff, commit, push, pull, branches, branch, setup_worktree, remove_worktree, pull_request]
}

/// Thread des Benutzers mit Projekt (falls vorhanden).
async fn load(state: &AppState, user_id: i64, id: i64) -> AppResult<(ThreadRow, Option<ProjectRow>)> {
    let t = model::own_thread(&state.db, user_id, id).await?;
    let p = match t.project_id {
        Some(pid) => model::project(&state.db, pid).await?,
        None => None,
    };
    Ok((t, p))
}

/// Arbeitsordner, der ein Git-Repository sein muss.
async fn git_dir(state: &AppState, t: &ThreadRow, p: &Option<ProjectRow>) -> AppResult<PathBuf> {
    if let Some(p) = p {
        if p.status != "ready" {
            return Err(AppError::conflict("Das Projekt ist noch nicht bereit."));
        }
    }
    let dir = super::thread_workdir(state, t.id).await?;
    if !gitops::is_git_dir(&dir) {
        return Err(AppError::bad("Der Arbeitsordner dieses Threads ist kein Git-Repository."));
    }
    Ok(dir)
}

fn not_busy(t: &ThreadRow) -> AppResult<()> {
    if t.busy() {
        return Err(AppError::conflict("Der Agent arbeitet gerade in diesem Thread – bitte warten oder anhalten."));
    }
    Ok(())
}

fn default_branch(p: &Option<ProjectRow>) -> String {
    p.as_ref().map(|p| p.default_branch.clone()).unwrap_or_else(|| "main".into())
}

/// Upstream des aktuellen Branches (oder `origin/<branch>`, falls vorhanden) – leer, wenn keiner.
async fn upstream(state: &AppState, dir: &Path, branch: &str) -> AppResult<String> {
    let out = git::run_with(state, dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"], None, &[]).await?;
    let up = out.stdout_str().trim().to_string();
    if out.ok() && !up.is_empty() {
        return Ok(up);
    }
    if !branch.is_empty() && git::rev_parse(state, dir, &format!("refs/remotes/origin/{branch}")).await?.is_some() {
        return Ok(format!("origin/{branch}"));
    }
    Ok(String::new())
}

/// (ahead, behind) gegenüber dem Upstream; ohne Upstream: Commits seit dem Abzweig vom Standard-Branch.
async fn ahead_behind(state: &AppState, dir: &Path, up: &str, default_branch: &str) -> AppResult<(i64, i64)> {
    if git::rev_parse(state, dir, "HEAD").await?.is_none() {
        return Ok((0, 0));
    }
    if !up.is_empty() {
        let range = format!("HEAD...{up}");
        let out = git::run_with(state, dir, &["rev-list", "--left-right", "--count", &range], None, &[]).await?;
        let text = out.stdout_str();
        let mut it = text.split_whitespace();
        return Ok((it.next().and_then(|x| x.parse().ok()).unwrap_or(0), it.next().and_then(|x| x.parse().ok()).unwrap_or(0)));
    }
    let base = gitops::base_rev(state, dir, default_branch).await?;
    if base == "HEAD" || base == gitops::EMPTY_TREE {
        return Ok((0, 0));
    }
    let range = format!("{base}..HEAD");
    let n = git::run_str(state, dir, &["rev-list", "--count", &range]).await.unwrap_or_default();
    Ok((n.trim().parse().unwrap_or(0), 0))
}

async fn has_origin(state: &AppState, dir: &Path) -> AppResult<bool> {
    git::run_ok(state, dir, &["remote", "get-url", "origin"]).await
}

/// Neuester Pull Request des Threads.
async fn last_pull_request(state: &AppState, thread_id: i64) -> AppResult<Option<PullRequestRef>> {
    let row: Option<(i64, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT number, title, base, head, url, created_at FROM workspace_pull_requests WHERE thread_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|(number, title, base, head, url, created_at)| PullRequestRef { number, title, base, head, url, created_at }))
}

/// Zustand des Arbeitsordners zusammenstellen (legt den Worktree bei Bedarf an).
pub async fn build_status(state: &AppState, user_id: i64, id: i64) -> AppResult<WorkspaceStatus> {
    let (t, p) = load(state, user_id, id).await?;
    let mut s = WorkspaceStatus {
        thread_id: t.id,
        title: t.title.clone(),
        project_id: t.project_id,
        project_name: p.as_ref().map(|p| p.name.clone()).unwrap_or_default(),
        repo: p.as_ref().map(|p| p.repo.clone()).unwrap_or_default(),
        host: "nrzX".into(),
        use_worktree: t.use_worktree && t.project_id.is_some(),
        project_status: p.as_ref().map(|p| p.status.clone()).unwrap_or_else(|| "ready".into()),
        default_branch: default_branch(&p),
        branch: t.branch.clone(),
        busy: t.busy(),
        pull_request: last_pull_request(state, t.id).await?,
        ..Default::default()
    };
    if let Some(p) = &p {
        if let Some(access) = super::project_access(state, p).await? {
            s.repo_url = format!("{}/{}", access.public_url, p.repo);
        }
    }
    if s.project_status != "ready" {
        return Ok(s);
    }
    let dir = super::thread_workdir(state, t.id).await?;
    s.path = dir.to_string_lossy().to_string();
    s.worktree = t.project_id.is_some_and(|pid| dir == model::worktree_dir(state, pid, t.id));
    s.is_git = gitops::is_git_dir(&dir);
    if !s.is_git {
        return Ok(s);
    }
    s.branch = gitops::current_branch(state, &dir).await?;
    s.upstream = upstream(state, &dir, &s.branch).await?;
    (s.ahead, s.behind) = ahead_behind(state, &dir, &s.upstream, &s.default_branch).await?;
    s.has_remote = has_origin(state, &dir).await?;
    s.files = gitops::changes(state, &dir, &s.default_branch).await?;
    s.additions = s.files.iter().map(|f| f.additions).sum();
    s.deletions = s.files.iter().map(|f| f.deletions).sum();
    s.uncommitted = gitops::uncommitted(state, &dir).await?;
    gitops::store_changes(state, t.id, s.additions, s.deletions).await;
    crate::hub::publish(
        state,
        user_id,
        nrzx_shared::HubEvent::ChangesUpdated { thread_id: t.id, additions: s.additions, deletions: s.deletions, files: s.files.len() as i64 },
    );
    Ok(s)
}

/// `GET /workspace/<id>/status`
#[get("/workspace/<id>/status")]
async fn status(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<WorkspaceStatus>> {
    Ok(Json(build_status(state, user.id(), id).await?))
}

/// `GET /workspace/<id>/diff?path=` – Änderungen gegen die Basis (eine Datei oder alle).
#[get("/workspace/<id>/diff?<path>")]
async fn diff(state: &State<AppState>, user: CurrentUser, id: i64, path: Option<&str>) -> AppResult<Json<Diff>> {
    let (t, p) = load(state, user.id(), id).await?;
    let dir = git_dir(state, &t, &p).await?;
    let path = path.map(str::trim).filter(|s| !s.is_empty());
    Ok(Json(gitops::diff(state, &dir, &default_branch(&p), path).await?))
}

/// Branch pushen (`-u origin <branch>`).
async fn push_branch(state: &AppState, user_id: i64, dir: &Path) -> AppResult<String> {
    let branch = gitops::current_branch(state, dir).await?;
    if branch.is_empty() {
        return Err(AppError::conflict("Es ist kein Branch ausgecheckt."));
    }
    if !has_origin(state, dir).await? {
        return Err(AppError::conflict("Kein Remote „origin“ – lokale Projekte lassen sich nicht pushen."));
    }
    let out = run_auth(state, dir, user_id, &["push", "--set-upstream", "origin", &branch]).await?;
    if !out.ok() {
        return Err(user_error("Push", &out));
    }
    Ok(branch)
}

/// `POST /workspace/<id>/commit` – alle oder ausgewählte Dateien committen, optional pushen.
#[post("/workspace/<id>/commit", data = "<body>")]
async fn commit(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<CommitRequest>) -> AppResult<Json<CommitResult>> {
    let (t, p) = load(state, user.id(), id).await?;
    let dir = git_dir(state, &t, &p).await?;
    let message = body.message.trim();
    if message.is_empty() {
        return Err(AppError::bad("Bitte eine Commit-Nachricht schreiben."));
    }
    let mut paths: Vec<String> = Vec::new();
    if !body.files.is_empty() {
        // Bei Umbenennungen gehört der alte Pfad dazu (sonst bliebe er als gelöscht liegen)
        let changed = gitops::uncommitted(state, &dir).await?;
        for f in &body.files {
            if f.trim().is_empty() || Path::new(f).is_absolute() || f.split('/').any(|c| c == "..") {
                return Err(AppError::bad(format!("Ungültiger Pfad: {f}")));
            }
            paths.push(f.clone());
            if let Some(c) = changed.iter().find(|c| &c.path == f && !c.old_path.is_empty()) {
                paths.push(c.old_path.clone());
            }
        }
    }
    // Vorhandene Dateien vormerken, gelöschte aus dem Index nehmen (`git add` mit einem Pfad,
    // den es nicht mehr gibt, bricht ab)
    let (present, missing): (Vec<&String>, Vec<&String>) = paths.iter().partition(|p| dir.join(p).symlink_metadata().is_ok());
    if paths.is_empty() || !present.is_empty() {
        let mut add: Vec<&str> = vec!["--literal-pathspecs", "add", "-A", "--"];
        add.extend(present.iter().map(|s| s.as_str()));
        let out = git::run_with(state, &dir, &add, None, &[]).await?;
        if !out.ok() {
            return Err(user_error("Dateien vormerken", &out));
        }
    }
    if !missing.is_empty() {
        let mut rm: Vec<&str> = vec!["--literal-pathspecs", "rm", "--cached", "--quiet", "--ignore-unmatch", "-r", "--"];
        rm.extend(missing.iter().map(|s| s.as_str()));
        let out = git::run_with(state, &dir, &rm, None, &[]).await?;
        if !out.ok() {
            return Err(user_error("Dateien vormerken", &out));
        }
    }
    if git::run_ok(state, &dir, &["diff", "--cached", "--quiet"]).await? {
        return Err(AppError::bad("Keine Änderungen zum Committen."));
    }
    let mut args: Vec<&str> = vec!["--literal-pathspecs", "commit", "--quiet", "-m", message];
    if !paths.is_empty() {
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
    }
    let out = run_auth(state, &dir, user.id(), &args).await?;
    if !out.ok() {
        return Err(user_error("Commit", &out));
    }
    let c = git::log::commit(state, &dir, "HEAD").await?.ok_or_else(|| AppError::internal("Commit nicht gefunden"))?;
    let files = git::run_str(state, &dir, &["show", "--name-only", "--format=", "HEAD"]).await?.lines().filter(|l| !l.is_empty()).count();
    let mut pushed = false;
    if body.push {
        push_branch(state, user.id(), &dir).await?;
        pushed = true;
    }
    if let Some(pid) = t.project_id {
        model::touch_project(&state.db, pid).await?;
    }
    gitops::publish_changes(state, &t).await?;
    Ok(Json(CommitResult { sha: c.sha, short_sha: c.short_sha, summary: c.summary, files: files as i64, pushed }))
}

async fn result(state: &AppState, dir: &Path, default_branch: &str, message: String) -> AppResult<GitResult> {
    let branch = gitops::current_branch(state, dir).await?;
    let up = upstream(state, dir, &branch).await?;
    let (ahead, behind) = ahead_behind(state, dir, &up, default_branch).await?;
    Ok(GitResult { message, branch, ahead, behind })
}

/// `POST /workspace/<id>/push`
#[post("/workspace/<id>/push")]
async fn push(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<GitResult>> {
    let (t, p) = load(state, user.id(), id).await?;
    let dir = git_dir(state, &t, &p).await?;
    let branch = push_branch(state, user.id(), &dir).await?;
    if let Some(pid) = t.project_id {
        model::touch_project(&state.db, pid).await?;
    }
    gitops::publish_changes(state, &t).await?;
    Ok(Json(result(state, &dir, &default_branch(&p), format!("„{branch}“ gepusht.")).await?))
}

/// `POST /workspace/<id>/pull` – holen und zusammenführen (vorspulen, sonst Merge; bei
/// Konflikten wird abgebrochen). Ohne eigenen Upstream: den Standard-Branch hereinholen.
#[post("/workspace/<id>/pull")]
async fn pull(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<GitResult>> {
    let (t, p) = load(state, user.id(), id).await?;
    not_busy(&t)?;
    let dir = git_dir(state, &t, &p).await?;
    if !has_origin(state, &dir).await? {
        return Err(AppError::conflict("Kein Remote „origin“ – lokale Projekte haben nichts zu holen."));
    }
    let out = run_auth(state, &dir, user.id(), &["fetch", "--prune", "origin"]).await?;
    if !out.ok() {
        return Err(user_error("Holen", &out));
    }
    let branch = gitops::current_branch(state, &dir).await?;
    let mut target = upstream(state, &dir, &branch).await?;
    if target.is_empty() {
        target = format!("origin/{}", default_branch(&p));
        if git::rev_parse(state, &dir, &target).await?.is_none() {
            return Ok(Json(result(state, &dir, &default_branch(&p), "Nichts zu holen.".into()).await?));
        }
    }
    let before = git::rev_parse(state, &dir, "HEAD").await?;
    let ff = git::run_with(state, &dir, &["merge", "--ff-only", "--quiet", &target], None, &[]).await?;
    if !ff.ok() {
        let out = run_auth(state, &dir, user.id(), &["merge", "--no-edit", "--quiet", &target]).await?;
        if !out.ok() {
            let _ = git::run_with(state, &dir, &["merge", "--abort"], None, &[]).await;
            return Err(user_error(&format!("Zusammenführen mit {target}"), &out));
        }
    }
    let after = git::rev_parse(state, &dir, "HEAD").await?;
    gitops::publish_changes(state, &t).await?;
    let message = if before == after { "Schon auf dem neuesten Stand.".to_string() } else { format!("{target} übernommen.") };
    Ok(Json(result(state, &dir, &default_branch(&p), message).await?))
}

/// `GET /workspace/<id>/branches` – lokale Branches und solche, die es nur auf origin gibt.
#[get("/workspace/<id>/branches")]
async fn branches(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<BranchList>> {
    let (t, p) = load(state, user.id(), id).await?;
    let dir = git_dir(state, &t, &p).await?;
    let current = gitops::current_branch(state, &dir).await?;
    let own = std::fs::canonicalize(&dir).unwrap_or(dir.clone());
    let text = git::run_str(state, &dir, &["for-each-ref", "--format=%(refname:strip=2)%09%(committerdate:unix)%09%(worktreepath)", "refs/heads/"]).await?;
    let mut list: Vec<BranchInfo> = Vec::new();
    for line in text.lines() {
        let mut f = line.split('\t');
        let name = f.next().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let when = f.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        let wt = f.next().unwrap_or("");
        let elsewhere = !wt.is_empty() && std::fs::canonicalize(wt).map(|w| w != own).unwrap_or(true);
        list.push(BranchInfo { current: name == current, name, remote_only: false, checked_out_elsewhere: elsewhere, last_commit_at: when });
    }
    let remote = git::run_str(state, &dir, &["for-each-ref", "--format=%(refname:strip=3)%09%(committerdate:unix)", "refs/remotes/origin/"]).await?;
    for line in remote.lines() {
        let mut f = line.split('\t');
        let name = f.next().unwrap_or("").to_string();
        if name.is_empty() || name == "HEAD" || list.iter().any(|b| b.name == name) {
            continue;
        }
        let when = f.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        list.push(BranchInfo { name, remote_only: true, last_commit_at: when, ..Default::default() });
    }
    list.sort_by(|a, b| b.current.cmp(&a.current).then(b.last_commit_at.cmp(&a.last_commit_at)));
    Ok(Json(BranchList { current, default_branch: default_branch(&p), branches: list }))
}

/// `POST /workspace/<id>/branch` – Branch wechseln oder neu anlegen. Im Hauptcheckout gilt der
/// Wechsel für alle Threads, die dort arbeiten.
#[post("/workspace/<id>/branch", data = "<body>")]
async fn branch(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<BranchRequest>) -> AppResult<Json<WorkspaceStatus>> {
    let (t, p) = load(state, user.id(), id).await?;
    not_busy(&t)?;
    let dir = git_dir(state, &t, &p).await?;
    let name = body.name.trim();
    git::check_ref_name(state, &dir, name).await?;
    let lock = t.project_id.map(gitops::project_lock);
    let _guard = match &lock {
        Some(l) => Some(l.lock().await),
        None => None,
    };
    let local = git::rev_parse(state, &dir, &format!("refs/heads/{name}")).await?.is_some();
    let out = if body.create {
        if local {
            return Err(AppError::conflict(format!("Den Branch „{name}“ gibt es schon.")));
        }
        git::run_with(state, &dir, &["switch", "--quiet", "-c", name], None, &[]).await?
    } else if local {
        git::run_with(state, &dir, &["switch", "--quiet", name], None, &[]).await?
    } else if git::rev_parse(state, &dir, &format!("refs/remotes/origin/{name}")).await?.is_some() {
        let track = format!("origin/{name}");
        git::run_with(state, &dir, &["switch", "--quiet", "-c", name, "--track", &track], None, &[]).await?
    } else {
        return Err(AppError::bad(format!("Den Branch „{name}“ gibt es nicht.")));
    };
    if !out.ok() {
        return Err(user_error("Branch wechseln", &out));
    }
    let worktree = t.project_id.is_some_and(|pid| dir == model::worktree_dir(state, pid, t.id));
    let ids: Vec<i64> = match (worktree, t.project_id) {
        (false, Some(pid)) => sqlx::query_scalar("SELECT id FROM threads WHERE project_id = ? AND (use_worktree = 0 OR id = ?)")
            .bind(pid)
            .bind(t.id)
            .fetch_all(&state.db)
            .await?,
        _ => vec![t.id],
    };
    for tid in ids {
        sqlx::query("UPDATE threads SET branch = ?, updated_at = ? WHERE id = ?").bind(name).bind(now()).bind(tid).execute(&state.db).await?;
        let _ = crate::agents::publish_thread(state, tid).await;
    }
    drop(_guard);
    Ok(Json(build_status(state, user.id(), id).await?))
}

/// `POST /workspace/<id>/worktree` – eigenen Worktree auf neuem Branch einrichten.
#[post("/workspace/<id>/worktree")]
async fn setup_worktree(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<WorkspaceStatus>> {
    let (t, p) = load(state, user.id(), id).await?;
    not_busy(&t)?;
    if p.as_ref().is_some_and(|p| p.status != "ready") {
        return Err(AppError::conflict("Das Projekt ist noch nicht bereit."));
    }
    gitops::setup_worktree(state, &t).await?;
    Ok(Json(build_status(state, user.id(), id).await?))
}

/// `DELETE /workspace/<id>/worktree` – Worktree entfernen (Branch bleibt), danach im Hauptcheckout.
#[delete("/workspace/<id>/worktree")]
async fn remove_worktree(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<WorkspaceStatus>> {
    let (t, _) = load(state, user.id(), id).await?;
    not_busy(&t)?;
    gitops::remove_worktree(state, &t).await?;
    Ok(Json(build_status(state, user.id(), id).await?))
}

/// `POST /workspace/<id>/pull-request` – Branch pushen und Pull Request in nrzGit anlegen.
#[post("/workspace/<id>/pull-request", data = "<body>")]
async fn pull_request(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<PullRequestRequest>) -> AppResult<Json<PullRequestRef>> {
    let (t, p) = load(state, user.id(), id).await?;
    let Some(project) = p.clone().filter(|p| p.is_nrzgit()) else {
        return Err(AppError::bad("Pull Requests gibt es nur für Projekte aus nrzGit."));
    };
    let access = nrzgit::require(state, user.id(), Some(project.forge_kind())).await?;
    let dir = git_dir(state, &t, &p).await?;
    let base = if body.base.trim().is_empty() { project.default_branch.clone() } else { body.base.trim().to_string() };
    let head = gitops::current_branch(state, &dir).await?;
    if head.is_empty() || head == base {
        return Err(AppError::bad("Für einen Pull Request bitte auf einem eigenen Branch arbeiten (z. B. mit „Setup Worktree“)."));
    }
    let title = if body.title.trim().is_empty() { t.title.trim().to_string() } else { body.title.trim().to_string() };
    if title.is_empty() {
        return Err(AppError::bad("Bitte einen Titel angeben."));
    }
    push_branch(state, user.id(), &dir).await?;
    let issue = Client::of(&access)
        .create_pull(&project.repo, &RemoteCreatePull { base: base.clone(), head: head.clone(), title: title.clone(), body: body.body.clone(), draft: body.draft })
        .await?;
    let url = nrzgit::pull_url(&access.public_url, &access.kind, &project.repo, issue.number);
    let ts = now();
    sqlx::query("INSERT INTO workspace_pull_requests (thread_id, repo, number, title, base, head, url, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(t.id)
        .bind(&project.repo)
        .bind(issue.number)
        .bind(&title)
        .bind(&base)
        .bind(&head)
        .bind(&url)
        .bind(ts)
        .execute(&state.db)
        .await?;
    model::touch_project(&state.db, project.id).await?;
    Ok(Json(PullRequestRef { number: issue.number, title, base, head, url, created_at: ts }))
}
