//! Git-Kern (aus nrzGit): Aufrufe des Programms `git`, Commits lesen (`log`) und Diffs parsen
//! (`diff`). Klonen, Worktrees, Commits und Pushes des Arbeitsbereichs bauen darauf auf.

pub mod diff;
pub mod log;

use crate::error::{AppError, AppResult};
use crate::AppState;
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

/// Leere Objekt-ID (Branch neu angelegt / gelöscht).
pub const ZERO_SHA: &str = "0000000000000000000000000000000000000000";

/// Ergebnis eines Git-Aufrufs.
#[derive(Debug)]
pub struct Output {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// `git` mit festen Grundeinstellungen: keine Rückfragen, keine System-/Benutzer-Konfiguration,
/// UTF-8-Pfade ohne Oktal-Escapes, jedes Verzeichnis als sicher.
pub fn command(state: &AppState, dir: &Path) -> tokio::process::Command {
    let mut cmd = crate::platform::command(&state.config.git_binary);
    cmd.current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("LC_ALL", "C")
        .env("HOME", &state.config.data_dir)
        .args(["-c", "safe.directory=*", "-c", "core.quotePath=false", "-c", "init.defaultBranch=main"])
        .kill_on_drop(true);
    cmd
}

/// Git ausführen, optional mit Eingabe und Umgebung. Fehler nur, wenn `git` nicht startet.
pub async fn run_with(
    state: &AppState,
    dir: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
    env: &[(&str, &str)],
) -> AppResult<Output> {
    let mut cmd = command(state, dir);
    cmd.args(args)
        .envs(env.iter().copied())
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::internal(format!("git startet nicht ({}): {e}", state.config.git_binary)))?;
    if let Some(input) = stdin {
        if let Some(mut pipe) = child.stdin.take() {
            let data = input.to_vec();
            // Schreiben parallel zum Lesen, sonst blockiert git bei grossen Ein-/Ausgaben
            tokio::spawn(async move {
                let _ = pipe.write_all(&data).await;
                let _ = pipe.shutdown().await;
            });
        }
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::internal(format!("git: {e}")))?;
    Ok(Output { status: out.status.code().unwrap_or(-1), stdout: out.stdout, stderr: out.stderr })
}

/// Git ausführen; Ausgabe bei Erfolg, sonst Fehler mit stderr (nur im Protokoll sichtbar).
pub async fn run(state: &AppState, dir: &Path, args: &[&str]) -> AppResult<Vec<u8>> {
    let out = run_with(state, dir, args, None, &[]).await?;
    if !out.ok() {
        return Err(AppError::internal(format!("git {}: {}", args.join(" "), out.stderr_str().trim())));
    }
    Ok(out.stdout)
}

/// Wie [`run`], Ausgabe als Text.
pub async fn run_str(state: &AppState, dir: &Path, args: &[&str]) -> AppResult<String> {
    Ok(String::from_utf8_lossy(&run(state, dir, args).await?).into_owned())
}

/// Nur ob `git` mit Status 0 endet (z. B. `merge-base --is-ancestor`).
pub async fn run_ok(state: &AppState, dir: &Path, args: &[&str]) -> AppResult<bool> {
    Ok(run_with(state, dir, args, None, &[]).await?.ok())
}

/// Neues Bare-Repository mit Standard-Branch.
pub async fn init_bare(state: &AppState, path: &Path, default_branch: &str) -> AppResult<()> {
    tokio::fs::create_dir_all(path).await?;
    let branch = format!("--initial-branch={default_branch}");
    run(state, path, &["init", "--bare", "--quiet", &branch, "."]).await?;
    // Pushes über HTTP/SSH laufen über receive-pack direkt; Reflogs helfen bei Pannen.
    run(state, path, &["config", "core.logAllRefUpdates", "true"]).await?;
    Ok(())
}

/// Revision (Branch, Tag, SHA, `HEAD~2` …) zur Commit-SHA auflösen; `None`, wenn es sie nicht gibt.
pub async fn rev_parse(state: &AppState, dir: &Path, rev: &str) -> AppResult<Option<String>> {
    if rev.starts_with('-') {
        return Ok(None);
    }
    let spec = format!("{rev}^{{commit}}");
    let out = run_with(state, dir, &["rev-parse", "--verify", "--quiet", "--end-of-options", &spec], None, &[]).await?;
    Ok(out.ok().then(|| out.stdout_str().trim().to_string()).filter(|s| !s.is_empty()))
}

/// Namen aller Branches (ohne `refs/heads/`).
pub async fn branch_names(state: &AppState, dir: &Path) -> AppResult<Vec<String>> {
    let out = run_str(state, dir, &["for-each-ref", "--format=%(refname:strip=2)", "refs/heads/"]).await?;
    Ok(out.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
}

/// Namen aller Tags (ohne `refs/tags/`), neueste zuerst.
pub async fn tag_names(state: &AppState, dir: &Path) -> AppResult<Vec<String>> {
    let out = run_str(state, dir, &["for-each-ref", "--sort=-creatordate", "--format=%(refname:strip=2)", "refs/tags/"]).await?;
    Ok(out.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
}

/// Alle Referenzen mit SHA (`refs/heads/main` → sha) – für den Vergleich vor/nach einem Push.
pub async fn refs(state: &AppState, dir: &Path) -> AppResult<Vec<(String, String)>> {
    let out = run_str(state, dir, &["for-each-ref", "--format=%(refname) %(objectname)"]).await?;
    Ok(out
        .lines()
        .filter_map(|l| l.split_once(' ').map(|(r, s)| (r.to_string(), s.to_string())))
        .collect())
}

/// Hat das Repository mindestens einen Commit?
pub async fn is_empty(state: &AppState, dir: &Path) -> AppResult<bool> {
    let out = run_str(state, dir, &["for-each-ref", "--count=1", "--format=%(objectname)", "refs/heads/"]).await?;
    Ok(out.trim().is_empty())
}

/// Branch- oder Tag-Name prüfen (`git check-ref-format`).
pub async fn check_ref_name(state: &AppState, dir: &Path, name: &str) -> AppResult<()> {
    let full = format!("refs/heads/{name}");
    if name.is_empty() || name.starts_with('-') || !run_ok(state, dir, &["check-ref-format", &full]).await? {
        return Err(AppError::bad(format!("„{name}“ ist kein gültiger Name für einen Branch oder Tag.")));
    }
    Ok(())
}

/// Grösse des Verzeichnisses in Bytes (für `repositories.size`).
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for e in entries.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => total += dir_size(&e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}
