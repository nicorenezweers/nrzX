//! Dateien des Arbeitsordners für `@` im Composer: `GET /threads/<id>/files?q=` bzw. für neue
//! Threads `GET /agents/files?project=&q=` (Hauptcheckout des Projekts). In Git-Repositories
//! über `git ls-files` (beachtet `.gitignore`), sonst ein begrenzter Durchlauf des Ordners.

use super::threads;
use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::FileEntry;
use rocket::serde::json::Json;
use rocket::{get, routes, Route, State};
use std::collections::BTreeSet;
use std::path::Path;

pub fn routes() -> Vec<Route> {
    routes![thread_files, project_files]
}

const SKIP: &[&str] = &[".git", "node_modules", "target", ".next", "dist", "build", ".venv", "__pycache__", ".cache"];

/// Alle Dateien (relativ), höchstens `max`.
async fn all_files(state: &AppState, dir: &Path, max: usize) -> Vec<String> {
    if dir.join(".git").exists() {
        if let Ok(out) = crate::git::run_with(state, dir, &["ls-files", "--cached", "--others", "--exclude-standard", "-z"], None, &[]).await {
            if out.ok() {
                return out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()).take(max).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
            }
        }
    }
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let mut stack = vec![(dir.clone(), 0usize)];
        while let Some((d, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let p = e.path();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if depth < 8 && !SKIP.contains(&name.as_str()) {
                        stack.push((p, depth + 1));
                    }
                } else if let Ok(rel) = p.strip_prefix(&dir) {
                    out.push(rel.to_string_lossy().into_owned());
                    if out.len() >= max {
                        return out;
                    }
                }
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

/// Passende Dateien und Ordner: Dateiname vor Pfad, kürzere zuerst.
pub fn rank(files: &[String], q: &str, limit: usize) -> Vec<FileEntry> {
    let q = q.trim().trim_start_matches('@').to_lowercase();
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for f in files {
        let mut cur = Path::new(f).parent();
        while let Some(p) = cur {
            let s = p.to_string_lossy();
            if s.is_empty() {
                break;
            }
            dirs.insert(format!("{s}/"));
            cur = p.parent();
        }
    }
    let mut scored: Vec<(u32, usize, FileEntry)> = files
        .iter()
        .map(|f| (f.clone(), false))
        .chain(dirs.into_iter().map(|d| (d, true)))
        .filter_map(|(path, dir)| {
            let lower = path.to_lowercase();
            let name = lower.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
            let score = if q.is_empty() {
                if dir { 3 } else { 2 }
            } else if name.starts_with(&q) {
                0
            } else if name.contains(&q) {
                1
            } else if lower.contains(&q) {
                2
            } else if subsequence(&lower, &q) {
                4
            } else {
                return None;
            };
            Some((score, path.len(), FileEntry { path, dir }))
        })
        .collect();
    scored.sort_by(|a, b| (a.0, a.1, &a.2.path).cmp(&(b.0, b.1, &b.2.path)));
    scored.into_iter().take(limit).map(|(_, _, e)| e).collect()
}

fn subsequence(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

/// `GET /threads/<id>/files?q=`
#[get("/threads/<id>/files?<q>")]
async fn thread_files(state: &State<AppState>, user: CurrentUser, id: i64, q: Option<&str>) -> AppResult<Json<Vec<FileEntry>>> {
    threads::get_row(state, user.id(), id).await?;
    let dir = crate::workspace::thread_workdir(state, id).await?;
    let files = all_files(state, &dir, 50_000).await;
    Ok(Json(rank(&files, q.unwrap_or(""), 30)))
}

/// `GET /agents/files?project=&q=` – Dateien des Projekts (Hauptcheckout) für neue Threads.
#[get("/agents/files?<project>&<q>")]
async fn project_files(state: &State<AppState>, user: CurrentUser, project: i64, q: Option<&str>) -> AppResult<Json<Vec<FileEntry>>> {
    let ok: Option<(i64,)> = sqlx::query_as("SELECT id FROM projects WHERE id = ? AND user_id = ?").bind(project).bind(user.id()).fetch_optional(&state.db).await?;
    ok.ok_or(AppError::NotFound)?;
    let dir = state.config.workspaces_dir().join(project.to_string()).join("repo");
    if !dir.is_dir() {
        return Ok(Json(Vec::new()));
    }
    let files = all_files(state, &dir, 50_000).await;
    Ok(Json(rank(&files, q.unwrap_or(""), 30)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn ranking() {
        let files: Vec<String> = ["src/main.rs", "src/agents/mod.rs", "README.md", "ui/src/main.rs"].iter().map(|s| s.to_string()).collect();
        let r = super::rank(&files, "main", 10);
        assert_eq!(r[0].path, "src/main.rs");
        let r = super::rank(&files, "agents", 10);
        assert_eq!(r[0].path, "src/agents/");
        assert!(super::rank(&files, "zzz", 10).is_empty());
    }
}
