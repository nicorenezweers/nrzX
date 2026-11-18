//! Git im Arbeitsbereich: Anmeldung bei nrzGit für `git` (HTTP Basic mit Benutzername und
//! Token als `http.<url>/.extraHeader`, nie in der Remote-Adresse), Arbeitsordner und Worktrees
//! der Threads, Änderungen gegen die Basis zählen und als Diff liefern, Docker-Anmeldung für die
//! Registry von nrzGit.

use super::model::{self, ProjectRow, ThreadRow};
use super::NrzGitAccess;
use crate::db::{now, User};
use crate::error::{AppError, AppResult};
use crate::git;
use crate::AppState;
use base64::Engine;
use nrzx_shared::workspace::ChangedFile;
use nrzx_shared::{Diff, DiffLineKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// Leerer Baum (Basis eines Repositorys ohne Commit).
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

// ------------------------------------------------------------------ Anmeldung

/// `Authorization: Basic …` aus Benutzername und Token.
pub fn basic_header(access: &NrzGitAccess) -> String {
    let user = if access.username.is_empty() { "token" } else { access.username.as_str() };
    let raw = format!("{user}:{}", access.token);
    format!("Authorization: Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

/// Git-Konfiguration über die Umgebung (`GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_n`,
/// `GIT_CONFIG_VALUE_n`): der Anmelde-Kopf gilt je Verbindung nur für Adressen unter dieser
/// Plattform. So steht das Token weder in der Remote-Adresse noch in der Prozessliste.
pub fn auth_config(accesses: &[NrzGitAccess]) -> Vec<(String, String)> {
    let mut env = vec![("GIT_CONFIG_COUNT".to_string(), accesses.len().to_string())];
    for (i, access) in accesses.iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{i}"), format!("http.{}/.extraHeader", access.url)));
        env.push((format!("GIT_CONFIG_VALUE_{i}"), basic_header(access)));
    }
    env
}

/// Name und E-Mail für Commits eines Benutzers.
pub fn identity(user: &User) -> (String, String) {
    let name = if user.full_name.trim().is_empty() { user.username.clone() } else { user.full_name.trim().to_string() };
    let email = if user.email.trim().is_empty() {
        format!("{}@nrzx.local", user.username.to_lowercase())
    } else {
        user.email.trim().to_string()
    };
    (name, email)
}

/// Git mit Anmeldung bei nrzGit (falls verbunden) und Identität des Benutzers ausführen.
pub async fn run_auth(
    state: &AppState,
    dir: &Path,
    user_id: i64,
    args: &[&str],
) -> AppResult<git::Output> {
    let mut env: Vec<(String, String)> = Vec::new();
    let accesses = super::forge_accesses(state, user_id).await?;
    if !accesses.is_empty() {
        env.extend(auth_config(&accesses));
    }
    if let Some(user) = crate::db::user_by_id(&state.db, user_id).await? {
        let (name, email) = identity(&user);
        for k in ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"] {
            env.push((k.into(), name.clone()));
        }
        for k in ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"] {
            env.push((k.into(), email.clone()));
        }
    }
    let env_ref: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    git::run_with(state, dir, args, None, &env_ref).await
}

/// Fehler von `git` für den Benutzer: letzte Zeilen der Fehlerausgabe (ohne Fortschritt).
pub fn user_error(what: &str, out: &git::Output) -> AppError {
    let text = out.stderr_str();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("hint:") && !l.contains('\r'))
        .collect();
    let tail = lines[lines.len().saturating_sub(4)..].join(" ");
    let tail: String = tail.chars().take(400).collect();
    if tail.is_empty() {
        AppError::conflict(format!("{what} ist fehlgeschlagen."))
    } else {
        AppError::conflict(format!("{what} ist fehlgeschlagen: {tail}"))
    }
}

// ------------------------------------------------------------------ Docker-Registry

/// `<data>/docker/<benutzer>` – eigener `DOCKER_CONFIG`-Ordner mit Anmeldung an der Registry von nrzGit.
pub fn docker_dir(state: &AppState, user_id: i64) -> PathBuf {
    state.config.data_dir.join("docker").join(user_id.to_string())
}

/// `config.json` für Docker schreiben (bzw. ohne Verbindung leeren). Host der Registry ist der
/// Host von nrzGit (OCI-Registry unter `/v2/`).
pub async fn write_docker_config(state: &AppState, user_id: i64) -> AppResult<()> {
    let dir = docker_dir(state, user_id);
    tokio::fs::create_dir_all(&dir).await?;
    let mut auths = serde_json::Map::new();
    for a in super::forge_accesses(state, user_id).await? {
        let user = if a.username.is_empty() { "token".to_string() } else { a.username.clone() };
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{}", a.token));
        auths.insert(super::nrzgit::registry_host(&a.url, &a.kind), serde_json::json!({ "auth": auth }));
    }
    let body = serde_json::to_vec_pretty(&serde_json::json!({ "auths": auths }))
        .map_err(|e| AppError::internal(e.to_string()))?;
    let path = dir.join("config.json");
    tokio::fs::write(&path, body).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).await;
    }
    Ok(())
}

// ------------------------------------------------------------------ Arbeitsordner und Worktrees

/// Sperre je Projekt: Worktrees, Branch-Wechsel und Klonen nicht gleichzeitig.
static LOCKS: LazyLock<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn project_lock(project_id: i64) -> Arc<tokio::sync::Mutex<()>> {
    LOCKS.lock().unwrap_or_else(|e| e.into_inner()).entry(project_id).or_default().clone()
}

/// Ist der Ordner ein Git-Arbeitsverzeichnis?
pub fn is_git_dir(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Wartet (höchstens eine Minute), bis ein Projekt geklont ist.
pub(crate) async fn wait_ready(state: &AppState, project_id: i64) -> AppResult<ProjectRow> {
    for _ in 0..120 {
        let p = model::project(&state.db, project_id).await?.ok_or(AppError::NotFound)?;
        match p.status.as_str() {
            "ready" => return Ok(p),
            "error" => {
                return Err(AppError::conflict(format!("Das Projekt „{}“ konnte nicht geklont werden: {}", p.name, p.status_message)))
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
        }
    }
    Err(AppError::conflict("Das Projekt wird noch geklont – bitte gleich noch einmal versuchen."))
}

/// Startpunkt für neue Branches: `origin/<standard>`, sonst der lokale Standard-Branch, sonst HEAD.
async fn start_point(state: &AppState, repo: &Path, default_branch: &str) -> AppResult<Option<String>> {
    for cand in [format!("refs/remotes/origin/{default_branch}"), format!("refs/heads/{default_branch}"), "HEAD".into()] {
        if git::rev_parse(state, repo, &cand).await?.is_some() {
            return Ok(Some(cand));
        }
    }
    Ok(None)
}

/// Aktueller Branch eines Arbeitsordners (leer bei abgetrenntem HEAD).
pub async fn current_branch(state: &AppState, dir: &Path) -> AppResult<String> {
    let out = git::run_with(state, dir, &["symbolic-ref", "--quiet", "--short", "HEAD"], None, &[]).await?;
    Ok(if out.ok() { out.stdout_str().trim().to_string() } else { String::new() })
}

/// Thread-Zeile mit Branch/Worktree aktualisieren und an die Oberfläche melden.
async fn set_thread_git(state: &AppState, thread_id: i64, branch: &str, worktree: &str, use_worktree: Option<bool>) -> AppResult<()> {
    let mut sql = String::from("UPDATE threads SET branch = ?, worktree_path = ?, updated_at = ?");
    if use_worktree.is_some() {
        sql.push_str(", use_worktree = ?");
    }
    sql.push_str(" WHERE id = ?");
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(branch).bind(worktree).bind(now());
    if let Some(u) = use_worktree {
        q = q.bind(u);
    }
    q.bind(thread_id).execute(&state.db).await?;
    if let Err(e) = crate::agents::publish_thread(state, thread_id).await {
        tracing::warn!("Thread {thread_id} melden: {e}");
    }
    Ok(())
}

/// Arbeitsordner eines Threads (siehe [`super::thread_workdir`]).
pub async fn workdir(state: &AppState, t: &ThreadRow) -> AppResult<PathBuf> {
    let Some(project_id) = t.project_id else {
        // Gespräch eines Bots: sein eigener Arbeitsordner („Computer“), über alle Gespräche gleich;
        // Deep Research und das Gespräch darüber: der Ordner der Recherche (dort liegt paper.md)
        let (bot, research, discuss_of): (Option<i64>, bool, Option<i64>) =
            sqlx::query_as("SELECT bot_id, research, discuss_of FROM threads WHERE id = ?").bind(t.id).fetch_optional(&state.db).await?.unwrap_or_default();
        let dir = match (bot, discuss_of) {
            (Some(b), _) => state.config.data_dir.join("bots").join(b.to_string()),
            (None, Some(r)) => crate::agents::research::dir(state, r),
            (None, None) if research => crate::agents::research::dir(state, t.id),
            (None, None) => model::scratch_dir(state, t.id),
        };
        tokio::fs::create_dir_all(&dir).await?;
        return Ok(dir);
    };
    let p = wait_ready(state, project_id).await?;
    let repo = model::repo_dir(state, project_id);
    if !t.use_worktree {
        let branch = current_branch(state, &repo).await.unwrap_or_default();
        if !branch.is_empty() && branch != t.branch {
            set_thread_git(state, t.id, &branch, "", None).await?;
        }
        return Ok(repo);
    }
    let path = model::worktree_dir(state, project_id, t.id);
    if path.join(".git").exists() {
        return Ok(path);
    }
    let lock = project_lock(project_id);
    let _guard = lock.lock().await;
    if path.join(".git").exists() {
        return Ok(path);
    }
    // Repository ohne Commit: kein Worktree möglich, im Hauptcheckout arbeiten
    let Some(start) = start_point(state, &repo, &p.default_branch).await? else {
        return Ok(repo);
    };
    let _ = git::run_with(state, &repo, &["worktree", "prune"], None, &[]).await?;
    if path.exists() {
        tokio::fs::remove_dir_all(&path).await?;
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Eigener Branch nrz/<kurzname>-<id>; einen schon vorhandenen (z. B. nach „Worktree
    // entfernen“) wieder verwenden
    let branch = if t.branch.starts_with("nrz/") { t.branch.clone() } else { format!("nrz/{}-{}", model::slug(&t.title), t.id) };
    let path_s = path.to_string_lossy().to_string();
    let exists = git::rev_parse(state, &repo, &format!("refs/heads/{branch}")).await?.is_some();
    let out = if exists {
        git::run_with(state, &repo, &["worktree", "add", &path_s, &branch], None, &[]).await?
    } else {
        git::run_with(state, &repo, &["worktree", "add", "-b", &branch, &path_s, &start], None, &[]).await?
    };
    if !out.ok() {
        return Err(user_error("Worktree anlegen", &out));
    }
    // Neue Branches sollen nicht auf origin/<standard> zeigen (sonst landet „Push“ dort)
    if !exists {
        let _ = git::run_with(state, &path, &["branch", "--unset-upstream"], None, &[]).await;
    }
    set_thread_git(state, t.id, &branch, &path_s, None).await?;
    Ok(path)
}

/// Worktree eines Threads entfernen (Branch bleibt); der Thread arbeitet danach im Hauptcheckout.
pub async fn remove_worktree(state: &AppState, t: &ThreadRow) -> AppResult<()> {
    let Some(project_id) = t.project_id else {
        return Err(AppError::bad("Dieser Thread gehört zu keinem Projekt."));
    };
    let repo = model::repo_dir(state, project_id);
    let path = model::worktree_dir(state, project_id, t.id);
    let lock = project_lock(project_id);
    let _guard = lock.lock().await;
    if path.exists() {
        let path_s = path.to_string_lossy().to_string();
        let out = git::run_with(state, &repo, &["worktree", "remove", "--force", &path_s], None, &[]).await?;
        if !out.ok() && path.exists() {
            tokio::fs::remove_dir_all(&path).await?;
        }
        let _ = git::run_with(state, &repo, &["worktree", "prune"], None, &[]).await;
    }
    let branch = current_branch(state, &repo).await.unwrap_or_default();
    set_thread_git(state, t.id, &branch, "", Some(false)).await
}

/// Eigenen Worktree für einen Thread einrichten.
pub async fn setup_worktree(state: &AppState, t: &ThreadRow) -> AppResult<PathBuf> {
    if t.project_id.is_none() {
        return Err(AppError::bad("Dieser Thread gehört zu keinem Projekt."));
    }
    sqlx::query("UPDATE threads SET use_worktree = 1 WHERE id = ?").bind(t.id).execute(&state.db).await?;
    let mut t = t.clone();
    t.use_worktree = true;
    workdir(state, &t).await
}

// ------------------------------------------------------------------ Änderungen

/// Basis für „Änderungen“: Abzweigpunkt vom Standard-Branch (auf origin, sonst lokal). Im
/// Hauptcheckout auf dem Standard-Branch sind das die noch nicht gepushten Commits plus alles
/// Ungespeicherte; in einem Worktree alles, was der Branch gegenüber dem Standard-Branch ändert.
pub async fn base_rev(state: &AppState, dir: &Path, default_branch: &str) -> AppResult<String> {
    if git::rev_parse(state, dir, "HEAD").await?.is_none() {
        return Ok(EMPTY_TREE.into());
    }
    for cand in [format!("refs/remotes/origin/{default_branch}"), format!("refs/heads/{default_branch}")] {
        if git::rev_parse(state, dir, &cand).await?.is_some() {
            let out = git::run_with(state, dir, &["merge-base", "HEAD", &cand], None, &[]).await?;
            let mb = out.stdout_str().trim().to_string();
            if out.ok() && !mb.is_empty() {
                return Ok(mb);
            }
        }
    }
    Ok("HEAD".into())
}

/// Vorübergehender Index mit allen Dateien des Arbeitsordners (auch unversionierten, ohne
/// ignorierte) – der echte Index der Agenten bleibt unberührt.
struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    async fn new(state: &AppState, dir: &Path) -> AppResult<TempIndex> {
        let real = git::run_str(state, dir, &["rev-parse", "--git-path", "index"]).await?;
        let real = PathBuf::from(real.trim());
        let real = if real.is_absolute() { real } else { dir.join(real) };
        let path = state.config.tmp_dir().join(format!("index-{}", uuid::Uuid::new_v4()));
        if real.exists() {
            tokio::fs::copy(&real, &path).await?;
        }
        let me = TempIndex { path };
        let out = git::run_with(state, dir, &["add", "-A", "--ignore-errors"], None, &me.env()).await?;
        if !out.ok() && !me.path.exists() {
            return Err(AppError::internal(format!("git add: {}", out.stderr_str().trim())));
        }
        Ok(me)
    }

    fn env(&self) -> [(&str, &str); 1] {
        [("GIT_INDEX_FILE", self.path.to_str().unwrap_or(""))]
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Status-Buchstabe von `git diff --name-status` → Wort.
fn status_word(code: &str) -> &'static str {
    match code.chars().next() {
        Some('A') => "added",
        Some('D') => "deleted",
        Some('R') => "renamed",
        Some('C') => "copied",
        Some('T') => "typechange",
        _ => "modified",
    }
}

/// Geänderte Dateien gegen die Basis (inkl. unversionierter Dateien).
pub async fn changes(state: &AppState, dir: &Path, default_branch: &str) -> AppResult<Vec<ChangedFile>> {
    if !is_git_dir(dir) {
        return Ok(Vec::new());
    }
    let base = base_rev(state, dir, default_branch).await?;
    changes_against(state, dir, &base).await
}

/// Noch nicht committete Änderungen (gegen HEAD, inkl. unversionierter Dateien).
pub async fn uncommitted(state: &AppState, dir: &Path) -> AppResult<Vec<ChangedFile>> {
    if !is_git_dir(dir) {
        return Ok(Vec::new());
    }
    let base = if git::rev_parse(state, dir, "HEAD").await?.is_some() { "HEAD" } else { EMPTY_TREE };
    changes_against(state, dir, base).await
}

/// Geänderte Dateien gegen eine Revision (über einen vorübergehenden Index).
async fn changes_against(state: &AppState, dir: &Path, base: &str) -> AppResult<Vec<ChangedFile>> {
    let idx = TempIndex::new(state, dir).await?;
    let names = git::run_with(state, dir, &["diff", "--cached", "-M", "-z", "--name-status", base, "--"], None, &idx.env()).await?;
    let nums = git::run_with(state, dir, &["diff", "--cached", "-M", "-z", "--numstat", base, "--"], None, &idx.env()).await?;
    if !names.ok() {
        return Err(AppError::internal(format!("git diff: {}", names.stderr_str().trim())));
    }
    // --name-status -z: STATUS \0 pfad \0 (bei R/C: STATUS \0 alt \0 neu \0)
    let mut files: Vec<ChangedFile> = Vec::new();
    let text = names.stdout_str();
    let mut parts = text.split('\0').filter(|s| !s.is_empty());
    while let Some(code) = parts.next() {
        let status = status_word(code);
        let (old_path, path) = if matches!(status, "renamed" | "copied") {
            let old = parts.next().unwrap_or("").to_string();
            (old, parts.next().unwrap_or("").to_string())
        } else {
            (String::new(), parts.next().unwrap_or("").to_string())
        };
        files.push(ChangedFile { path, old_path, status: status.into(), ..Default::default() });
    }
    // --numstat -z: "plus\tminus\tpfad\0" bzw. bei Umbenennung "plus\tminus\t\0alt\0neu\0"
    let text = nums.stdout_str();
    let mut parts = text.split('\0');
    while let Some(head) = parts.next() {
        if head.is_empty() {
            continue;
        }
        let mut f = head.splitn(3, '\t');
        let (a, d, p) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""));
        let path = if p.is_empty() {
            let _old = parts.next();
            parts.next().unwrap_or("").to_string()
        } else {
            p.to_string()
        };
        if let Some(file) = files.iter_mut().find(|x| x.path == path) {
            file.binary = a == "-";
            file.additions = a.parse().unwrap_or(0);
            file.deletions = d.parse().unwrap_or(0);
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Diff gegen die Basis (inkl. unversionierter Dateien), optional nur eine Datei, mit Syntaxfarben.
pub async fn diff(state: &AppState, dir: &Path, default_branch: &str, path: Option<&str>) -> AppResult<Diff> {
    if !is_git_dir(dir) {
        return Ok(Diff::default());
    }
    let base = base_rev(state, dir, default_branch).await?;
    let idx = TempIndex::new(state, dir).await?;
    let mut args = vec!["diff", "--cached", "--no-color", "--no-ext-diff", "-M", "-p", &base, "--"];
    if let Some(p) = path {
        args.push(p);
    }
    let out = git::run_with(state, dir, &args, None, &idx.env()).await?;
    if !out.ok() {
        return Err(AppError::internal(format!("git diff: {}", out.stderr_str().trim())));
    }
    let text = out.stdout_str();
    let diff = tokio::task::spawn_blocking(move || {
        let mut d = git::diff::parse(&text);
        highlight_diff(&mut d);
        d
    })
    .await
    .map_err(|e| AppError::internal(format!("Diff: {e}")))?;
    Ok(diff)
}

/// Zeilen eines Diffs mit Syntaxfarben versehen (wie in nrzGit): je Abschnitt die alte und die
/// neue Seite als zusammenhängenden Text hervorheben und den Zeilen zuordnen.
pub fn highlight_diff(diff: &mut Diff) {
    let total: usize = diff.files.iter().flat_map(|f| f.hunks.iter()).map(|h| h.lines.len()).sum();
    if total > 20_000 {
        return;
    }
    for f in diff.files.iter_mut() {
        if f.binary || f.too_large {
            continue;
        }
        let syntax = crate::highlight::syntax_for(f.path(), "");
        for h in f.hunks.iter_mut() {
            let side = |want: DiffLineKind| -> String {
                h.lines
                    .iter()
                    .filter(|l| l.kind == DiffLineKind::Context || l.kind == want)
                    .map(|l| format!("{}\n", l.content))
                    .collect()
            };
            let old_html = crate::highlight::highlight_lines(syntax, &side(DiffLineKind::Del));
            let new_html = crate::highlight::highlight_lines(syntax, &side(DiffLineKind::Add));
            let (mut oi, mut ni) = (0usize, 0usize);
            for l in h.lines.iter_mut() {
                l.html = match l.kind {
                    DiffLineKind::Context => {
                        oi += 1;
                        ni += 1;
                        new_html.get(ni - 1).cloned()
                    }
                    DiffLineKind::Add => {
                        ni += 1;
                        new_html.get(ni - 1).cloned()
                    }
                    DiffLineKind::Del => {
                        oi += 1;
                        old_html.get(oi - 1).cloned()
                    }
                    DiffLineKind::Meta => None,
                };
            }
        }
    }
}

/// Änderungen eines Threads zählen und als `HubEvent::ChangesUpdated` melden.
pub async fn publish_changes(state: &AppState, t: &ThreadRow) -> AppResult<(i64, i64, i64)> {
    let dir = match current_dir(state, t).await? {
        Some(d) => d,
        None => return Ok((0, 0, 0)),
    };
    let default_branch = match t.project_id {
        Some(p) => model::project(&state.db, p).await?.map(|p| p.default_branch).unwrap_or_else(|| "main".into()),
        None => "main".into(),
    };
    let files = changes(state, &dir, &default_branch).await?;
    let adds = files.iter().map(|f| f.additions).sum();
    let dels = files.iter().map(|f| f.deletions).sum();
    let n = files.len() as i64;
    store_changes(state, t.id, adds, dels).await;
    crate::hub::publish(
        state,
        t.user_id,
        nrzx_shared::HubEvent::ChangesUpdated { thread_id: t.id, additions: adds, deletions: dels, files: n },
    );
    Ok((adds, dels, n))
}

/// Gezählte Änderungen am Thread merken (für `ThreadSummary::additions`/`deletions`).
pub async fn store_changes(state: &AppState, thread_id: i64, additions: i64, deletions: i64) {
    let _ = sqlx::query("UPDATE threads SET additions = ?, deletions = ? WHERE id = ?")
        .bind(additions)
        .bind(deletions)
        .bind(thread_id)
        .execute(&state.db)
        .await;
}

/// Vorhandener Arbeitsordner eines Threads, ohne etwas anzulegen (`None`, wenn es ihn noch nicht gibt).
pub async fn current_dir(state: &AppState, t: &ThreadRow) -> AppResult<Option<PathBuf>> {
    let dir = match t.project_id {
        None => model::scratch_dir(state, t.id),
        Some(p) => {
            let wt = model::worktree_dir(state, p, t.id);
            let repo = model::repo_dir(state, p);
            if !t.use_worktree {
                repo
            } else if wt.join(".git").exists() {
                wt
            } else if is_git_dir(&repo) && git::rev_parse(state, &repo, "HEAD").await?.is_none() {
                // Repository ohne Commit: der Thread arbeitet im Hauptcheckout
                repo
            } else {
                // Worktree (noch) nicht angelegt – nicht den Hauptcheckout zeigen
                return Ok(None);
            }
        }
    };
    Ok(dir.exists().then_some(dir))
}
