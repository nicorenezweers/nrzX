//! Dateien im Arbeitsordner eines Threads – für „Open in editor“: Verzeichnis auflisten, alle
//! Dateien (für die Schnellsuche), Datei lesen und schreiben. Pfade sind relativ zum
//! Arbeitsordner; `..`, absolute Pfade, `.git` und Symlinks nach draussen werden abgewiesen.
//!
//! Routen unter `/workspace/<thread>/`: `GET files?path=`, `GET files/all`, `GET file?path=`,
//! `PUT file`.

use super::model;
use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::git;
use crate::AppState;
use nrzx_shared::workspace::{FileContent, FileEntry, FileWrite, RawToken};
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::{get, put, routes, Route, State};
use std::path::{Component, Path, PathBuf};

pub fn routes() -> Vec<Route> {
    routes![list, all, read, write, raw_token]
}

/// Routen unter `/` (ausserhalb der API): Dateien für den integrierten Browser.
pub fn raw_routes() -> Vec<Route> {
    routes![raw]
}

// ------------------------------------------------------------------ Integrierter Browser

/// Freigaben für den integrierten Browser: Token → (Benutzer, Thread, gültig bis). Der Token steht
/// im Pfad (`/raw/<token>/index.html`), damit relative Links der Seite (Skripte, Bilder) ohne
/// Anmeldung funktionieren – die Seite läuft in einer Sandbox ohne Zugriff auf die Sitzung.
static RAW: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, (i64, i64, i64)>>> =
    std::sync::LazyLock::new(Default::default);

/// Gültigkeit einer Freigabe (Sekunden).
const RAW_FOR: i64 = 12 * 3600;

/// `POST /workspace/<id>/raw-token` – Freigabe des Arbeitsordners für den integrierten Browser.
#[rocket::post("/workspace/<id>/raw-token")]
async fn raw_token(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<RawToken>> {
    root(state, user.id(), id).await?;
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let now = crate::db::now();
    let mut map = RAW.lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, v| v.2 > now);
    map.insert(token.clone(), (user.id(), id, now + RAW_FOR));
    Ok(Json(RawToken { url: format!("/raw/{token}/") }))
}

/// Datei aus dem Arbeitsordner, mit Sandbox: eigener (leerer) Ursprung, keine Cookies der App,
/// darf nur in der App gerahmt werden.
#[derive(rocket::Responder)]
struct RawFile {
    inner: (rocket::http::ContentType, Vec<u8>),
    csp: rocket::http::Header<'static>,
    frame: rocket::http::Header<'static>,
    cache: rocket::http::Header<'static>,
}

/// `GET /raw/<token>/<path..>` – Datei für den integrierten Browser (Ordner: `index.html`).
#[get("/raw/<token>/<path..>")]
async fn raw(state: &State<AppState>, token: &str, path: PathBuf) -> AppResult<RawFile> {
    let (user_id, id) = {
        let map = RAW.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(token) {
            Some((u, t, until)) if *until > crate::db::now() => (*u, *t),
            _ => return Err(AppError::NotFound),
        }
    };
    let root = root(state, user_id, id).await?;
    let rel = clean(&path.to_string_lossy())?;
    let mut full = resolve(&root, &rel)?;
    if full.is_dir() {
        full = full.join("index.html");
    }
    let meta = tokio::fs::metadata(&full).await.map_err(|_| AppError::NotFound)?;
    if !meta.is_file() || meta.len() > 50 * 1024 * 1024 {
        return Err(AppError::NotFound);
    }
    let bytes = tokio::fs::read(&full).await?;
    let ct = full
        .extension()
        .and_then(|e| e.to_str())
        .and_then(rocket::http::ContentType::from_extension)
        .unwrap_or(rocket::http::ContentType::Binary);
    Ok(RawFile {
        inner: (ct, bytes),
        csp: rocket::http::Header::new(
            "Content-Security-Policy",
            "sandbox allow-scripts allow-forms allow-popups allow-modals allow-pointer-lock allow-downloads; \
             default-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob: https: http:; frame-ancestors 'self'",
        ),
        frame: rocket::http::Header::new("X-Frame-Options", "SAMEORIGIN"),
        cache: rocket::http::Header::new("Cache-Control", "no-store"),
    })
}

/// Grösste Datei, die gelesen/bearbeitet wird.
pub const MAX_EDIT: u64 = 2 * 1024 * 1024;

/// Relativen Pfad prüfen und zerlegen (leer = Wurzel).
fn clean(rel: &str) -> AppResult<PathBuf> {
    let rel = rel.trim().trim_start_matches("./");
    let p = Path::new(rel);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => {
                if s == ".git" {
                    return Err(AppError::forbidden("Der Ordner .git ist hier tabu."));
                }
                out.push(s);
            }
            Component::CurDir => {}
            _ => return Err(AppError::bad("Ungültiger Pfad.")),
        }
    }
    Ok(out)
}

/// Pfad im Arbeitsordner auflösen; Symlinks dürfen nicht hinausführen.
fn resolve(root: &Path, rel: &Path) -> AppResult<PathBuf> {
    let root = std::fs::canonicalize(root)?;
    let full = root.join(rel);
    // Für neue Dateien den nächsten vorhandenen Ordner prüfen
    let mut probe = full.clone();
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => break,
        }
    }
    let real = std::fs::canonicalize(&probe)?;
    if !real.starts_with(&root) {
        return Err(AppError::forbidden("Dieser Pfad liegt ausserhalb des Arbeitsordners."));
    }
    Ok(full)
}

async fn root(state: &AppState, user_id: i64, id: i64) -> AppResult<PathBuf> {
    let t = model::own_thread(&state.db, user_id, id).await?;
    if let Some(pid) = t.project_id {
        let p = model::project(&state.db, pid).await?.ok_or(AppError::NotFound)?;
        if p.status != "ready" {
            return Err(AppError::conflict("Das Projekt ist noch nicht bereit."));
        }
    }
    super::thread_workdir(state, id).await
}

fn rel_string(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// `GET /workspace/<id>/files?path=` – Inhalt eines Ordners (Ordner zuerst, ohne `.git`).
#[get("/workspace/<id>/files?<path>")]
async fn list(state: &State<AppState>, user: CurrentUser, id: i64, path: Option<&str>) -> AppResult<Json<Vec<FileEntry>>> {
    let root = root(state, user.id(), id).await?;
    let rel = clean(path.unwrap_or(""))?;
    let dir = resolve(&root, &rel)?;
    if !dir.is_dir() {
        return Err(AppError::NotFound);
    }
    let mut out = Vec::new();
    let mut rd = tokio::fs::read_dir(&dir).await?;
    while let Some(e) = rd.next_entry().await? {
        let name = e.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        let meta = match tokio::fs::metadata(e.path()).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        out.push(FileEntry { path: rel_string(&rel.join(&name)), name, dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() as i64 } });
    }
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(Json(out))
}

/// `GET /workspace/<id>/files/all` – alle Dateien (versioniert und neu, ohne ignorierte),
/// höchstens 20 000 – für die Schnellsuche.
#[get("/workspace/<id>/files/all")]
async fn all(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<Vec<String>>> {
    let root = root(state, user.id(), id).await?;
    let mut files: Vec<String> = if super::gitops::is_git_dir(&root) {
        let out = git::run_with(state, &root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"], None, &[]).await?;
        out.stdout_str().split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect()
    } else {
        let root2 = root.clone();
        tokio::task::spawn_blocking(move || {
            let mut v = Vec::new();
            walk(&root2, &root2, &mut v);
            v
        })
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
    };
    files.sort();
    files.dedup();
    files.truncate(20_000);
    Ok(Json(files))
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    if out.len() >= 20_000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == ".git" || name == "node_modules" || name == "target" {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(root, &e.path(), out);
        } else if let Ok(rel) = e.path().strip_prefix(root) {
            out.push(rel_string(rel));
        }
    }
}

/// `GET /workspace/<id>/file?path=` – Textdatei lesen (Binärdateien und sehr grosse ohne Inhalt).
#[get("/workspace/<id>/file?<path>")]
async fn read(state: &State<AppState>, user: CurrentUser, id: i64, path: &str) -> AppResult<Json<FileContent>> {
    let root = root(state, user.id(), id).await?;
    let rel = clean(path)?;
    let full = resolve(&root, &rel)?;
    let meta = tokio::fs::metadata(&full).await.map_err(|_| AppError::NotFound)?;
    if !meta.is_file() {
        return Err(AppError::NotFound);
    }
    let mut c = FileContent { path: rel_string(&rel), size: meta.len() as i64, ..Default::default() };
    if meta.len() > MAX_EDIT {
        c.too_large = true;
        return Ok(Json(c));
    }
    let bytes = tokio::fs::read(&full).await?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        c.binary = true;
        return Ok(Json(c));
    }
    match String::from_utf8(bytes) {
        Ok(s) => c.content = s,
        Err(_) => c.binary = true,
    }
    Ok(Json(c))
}

/// `PUT /workspace/<id>/file` – Textdatei schreiben (legt fehlende Ordner an).
#[put("/workspace/<id>/file", data = "<body>")]
async fn write(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<FileWrite>) -> AppResult<Status> {
    let root = root(state, user.id(), id).await?;
    let rel = clean(&body.path)?;
    if rel.as_os_str().is_empty() {
        return Err(AppError::bad("Bitte einen Dateinamen angeben."));
    }
    if body.content.len() as u64 > MAX_EDIT {
        return Err(AppError::bad("Die Datei ist zu gross."));
    }
    let full = resolve(&root, &rel)?;
    if full.is_dir() {
        return Err(AppError::bad("Das ist ein Ordner."));
    }
    if let Some(parent) = full.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&full, body.content.as_bytes()).await?;
    let t = model::own_thread(&state.db, user.id(), id).await?;
    let _ = super::gitops::publish_changes(state, &t).await;
    Ok(Status::NoContent)
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn paths() {
        assert_eq!(clean("src/main.rs").unwrap().to_string_lossy(), "src/main.rs");
        assert_eq!(clean("").unwrap().to_string_lossy(), "");
        assert!(clean("../x").is_err());
        assert!(clean("/etc/passwd").is_err());
        assert!(clean("a/.git/config").is_err());
        assert!(clean("a/../../b").is_err());
    }
}
