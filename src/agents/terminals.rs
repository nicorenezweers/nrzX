//! Terminals (PTY mit `portable-pty`): Shell oder Befehl im Arbeitsordner eines Threads, eines
//! Projekts oder im Home eines Agent-Kontos (dann in dessen Umgebung – so meldet man sich in
//! den CLIs an). Ausgabe live über `GET /terminals/<id>/stream` (Server-Sent Events, Base64),
//! Eingabe mit `POST …/input`, Grösse mit `POST …/resize`, schliessen mit `DELETE`.
//!
//! `POST /threads/<id>/cli` startet das CLI des Agenten selbst (Claude Code, Codex …) statt des
//! Chats: im Arbeitsordner des Threads, mit dessen Konto, Modell und Sitzung. Es läuft weiter,
//! während die Oberfläche den Chat zeigt (schneller Wechsel); höchstens [`MAX_CLIS`] je Benutzer.
//! Startet ein Lauf des Threads, wird es geschlossen (Exit-Code [`EXIT_FOR_RUN`]) – zwei Programme
//! in derselben Sitzung kämen sich in die Quere; die Oberfläche startet es danach neu.

use super::accounts::{self, AccountRow};
use super::kinds::spec;
use super::{env, runtime, threads, tools};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use base64::Engine;
use nrzx_shared::agents::{TerminalInfo, TerminalInput, TerminalOpen, TerminalSize};
use nrzx_shared::AgentKind;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use rocket::http::Status;
use rocket::response::stream::{Event, EventStream};
use rocket::serde::json::Json;
use rocket::{delete, get, post, routes, Route, Shutdown, State};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub fn routes() -> Vec<Route> {
    routes![open, open_cli, list, info, stream, input, resize, close]
}

/// Puffer für neue Zuschauer (letzte Ausgabe).
const BUFFER: usize = 256 * 1024;
/// Höchstens so viele offene Terminals je Benutzer.
const MAX_PER_USER: usize = 16;
/// Höchstens so viele laufende CLIs je Benutzer (ältere werden geschlossen).
const MAX_CLIS: usize = 4;
/// Exit-Code eines CLIs, das der Server schliesst, weil ein Lauf des Threads startet.
pub const EXIT_FOR_RUN: i32 = -2;

#[derive(Clone, Debug)]
pub enum TermEvent {
    Data(Vec<u8>),
    Exit(i32),
}

pub struct Terminal {
    pub id: String,
    pub user_id: i64,
    pub title: String,
    pub cwd: String,
    pub thread_id: Option<i64>,
    pub account_id: Option<i64>,
    /// CLI des Agenten dieses Threads (`POST /threads/<id>/cli`)
    pub cli_of: Option<i64>,
    /// Geschlossen, weil ein Lauf startet (Exit-Code [`EXIT_FOR_RUN`])
    closed_for_run: AtomicBool,
    pub created_at: i64,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    buffer: Mutex<Vec<u8>>,
    tx: broadcast::Sender<TermEvent>,
    exit: Mutex<Option<i32>>,
    exited_at: Mutex<Option<Instant>>,
    size: Mutex<(u16, u16)>,
}

impl Terminal {
    pub fn info(&self) -> TerminalInfo {
        let exit = *self.exit.lock().unwrap_or_else(|e| e.into_inner());
        let (cols, rows) = *self.size.lock().unwrap_or_else(|e| e.into_inner());
        TerminalInfo {
            id: self.id.clone(),
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            thread_id: self.thread_id,
            account_id: self.account_id,
            running: exit.is_none(),
            exit_code: exit,
            cols,
            rows,
            created_at: self.created_at,
        }
    }

    fn kill(&self) {
        let _ = self.child.lock().unwrap_or_else(|e| e.into_inner()).kill();
    }

    fn write(&self, data: &[u8]) -> std::io::Result<()> {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(data)?;
        w.flush()
    }
}

/// Shell des Servers (`$SHELL`, sonst bash, sonst sh).
fn shell() -> String {
    if cfg!(windows) {
        return crate::platform::default_shell();
    }
    if let Ok(s) = std::env::var("SHELL") {
        if Path::new(&s).is_file() {
            return s;
        }
    }
    for s in ["/bin/bash", "/usr/bin/bash", "/bin/zsh", "/bin/sh"] {
        if Path::new(s).is_file() {
            return s.into();
        }
    }
    "sh".into()
}

/// Was im Terminal läuft.
pub enum Launch<'a> {
    /// Shell des Servers, optional mit einem Befehl (`-c`)
    Shell(Option<&'a str>),
    /// CLI des Agenten eines Threads: Programm mit Argumenten, ohne Shell dazwischen
    Cli { program: PathBuf, args: Vec<String>, thread_id: i64 },
}

/// Terminal starten.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    state: &AppState,
    user_id: i64,
    title: &str,
    cwd: &Path,
    envs: Vec<(String, String)>,
    launch: Launch<'_>,
    size: (u16, u16),
    thread_id: Option<i64>,
    account_id: Option<i64>,
) -> AppResult<Arc<Terminal>> {
    let rt = runtime::get(state);
    {
        let all = rt.terminals.lock().unwrap_or_else(|e| e.into_inner());
        let mine = all.values().filter(|t| t.user_id == user_id && t.exit.lock().map(|e| e.is_none()).unwrap_or(false)).count();
        if mine >= MAX_PER_USER {
            return Err(AppError::conflict("Zu viele offene Terminals – bitte erst welche schliessen."));
        }
    }
    let (cols, rows) = (size.0.clamp(10, 500), size.1.clamp(4, 300));
    let pair = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| AppError::internal(format!("PTY: {e}")))?;
    let (mut cmd, cli_of) = match launch {
        Launch::Shell(command) => {
            let sh = shell();
            let mut cmd = CommandBuilder::new(&sh);
            if let Some(c) = command {
                // PowerShell/cmd unter Windows: -Command bzw. /C statt -c
                let flag = if !cfg!(windows) {
                    "-c"
                } else if sh.to_lowercase().ends_with("cmd.exe") {
                    "/C"
                } else {
                    "-Command"
                };
                cmd.arg(flag);
                cmd.arg(c);
            }
            (cmd, None)
        }
        Launch::Cli { program, args, thread_id } => {
            // Windows: npm-Shims aufgelöst wie bei den Läufen
            let (program, pre) = crate::platform::resolve_program(&program);
            let mut cmd = CommandBuilder::new(program);
            cmd.args(pre);
            cmd.args(args);
            (cmd, Some(thread_id))
        }
    };
    cmd.cwd(cwd);
    cmd.env_clear();
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("PS1", "\\w \\$ ");
    let child = pair.slave.spawn_command(cmd).map_err(|e| AppError::internal(format!("Terminal startet nicht: {e}")))?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| AppError::internal(format!("PTY: {e}")))?;
    let writer = pair.master.take_writer().map_err(|e| AppError::internal(format!("PTY: {e}")))?;
    let term = Arc::new(Terminal {
        id: uuid::Uuid::new_v4().simple().to_string(),
        user_id,
        title: title.to_string(),
        cwd: cwd.to_string_lossy().into_owned(),
        thread_id,
        account_id,
        cli_of,
        closed_for_run: AtomicBool::new(false),
        created_at: now(),
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        child: Mutex::new(child),
        buffer: Mutex::new(Vec::new()),
        tx: broadcast::channel(1024).0,
        exit: Mutex::new(None),
        exited_at: Mutex::new(None),
        size: Mutex::new((cols, rows)),
    });
    rt.terminals.lock().unwrap_or_else(|e| e.into_inner()).insert(term.id.clone(), term.clone());
    let t = term.clone();
    std::thread::Builder::new()
        .name(format!("terminal-{}", &t.id[..8]))
        .spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = buf[..n].to_vec();
                        {
                            let mut b = t.buffer.lock().unwrap_or_else(|e| e.into_inner());
                            b.extend_from_slice(&chunk);
                            if b.len() > BUFFER {
                                let cut = b.len() - BUFFER * 3 / 4;
                                b.drain(..cut);
                            }
                        }
                        let _ = t.tx.send(TermEvent::Data(chunk));
                    }
                }
            }
            // Auf das Ende des Prozesses warten (höchstens 5 s, dann beenden)
            let mut code = -1;
            for i in 0..100 {
                let r = t.child.lock().unwrap_or_else(|e| e.into_inner()).try_wait();
                match r {
                    Ok(Some(s)) => {
                        code = s.exit_code() as i32;
                        break;
                    }
                    Ok(None) if i < 99 => std::thread::sleep(Duration::from_millis(50)),
                    _ => {
                        t.kill();
                        break;
                    }
                }
            }
            if t.closed_for_run.load(Ordering::Relaxed) {
                code = EXIT_FOR_RUN;
            }
            *t.exit.lock().unwrap_or_else(|e| e.into_inner()) = Some(code);
            *t.exited_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
            let _ = t.tx.send(TermEvent::Exit(code));
        })
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(term)
}

/// Terminal im Home eines Kontos (Anmeldung im CLI).
pub async fn open_for_account(state: &AppState, user_id: i64, acc: &AccountRow, command: &str, extra: Vec<(String, String)>) -> AppResult<TerminalInfo> {
    let home = accounts::ensure_home(state, acc.id).await?;
    let envs = env::merge(vec![env::base(state), accounts::env_for(state, acc), extra]);
    let title = format!("{} · {}", acc.kind().name(), acc.label);
    let t = spawn(state, user_id, &title, &home, envs, Launch::Shell(Some(command)), (100, 30), None, Some(acc.id))?;
    Ok(t.info())
}

fn get_term(state: &AppState, user_id: i64, id: &str) -> AppResult<Arc<Terminal>> {
    let rt = runtime::get(state);
    let all = rt.terminals.lock().unwrap_or_else(|e| e.into_inner());
    all.get(id).filter(|t| t.user_id == user_id).cloned().ok_or(AppError::NotFound)
}

fn remove(state: &AppState, pred: impl Fn(&Terminal) -> bool) {
    let rt = runtime::get(state);
    let mut all = rt.terminals.lock().unwrap_or_else(|e| e.into_inner());
    let ids: Vec<String> = all.values().filter(|t| pred(t)).map(|t| t.id.clone()).collect();
    for id in ids {
        if let Some(t) = all.remove(&id) {
            t.kill();
        }
    }
}

/// Terminals eines Kontos schliessen (Konto gelöscht).
pub fn close_for_account(state: &AppState, account_id: i64) {
    remove(state, |t| t.account_id == Some(account_id));
}

/// Terminals von Threads schliessen (Threads gelöscht), auch deren CLIs.
pub fn close_for_threads(state: &AppState, ids: &[i64]) {
    remove(state, |t| t.thread_id.or(t.cli_of).is_some_and(|id| ids.contains(&id)));
}

/// Beendete Terminals nach zehn Minuten wegräumen.
pub fn reap(state: &AppState) {
    remove(state, |t| t.exited_at.lock().map(|e| e.is_some_and(|at| at.elapsed() > Duration::from_secs(600))).unwrap_or(false));
}

/// `POST /terminals` – Shell (oder `command`) im Thread-, Projekt- oder Konto-Ordner.
#[post("/terminals", data = "<body>")]
async fn open(state: &State<AppState>, user: CurrentUser, body: Json<TerminalOpen>) -> AppResult<Json<TerminalInfo>> {
    let b = body.into_inner();
    let size = (b.cols.unwrap_or(100), b.rows.unwrap_or(30));
    let command = b.command.as_deref().map(str::trim).filter(|c| !c.is_empty());
    let (title, cwd, envs, thread_id, account_id): (String, PathBuf, Vec<(String, String)>, Option<i64>, Option<i64>) = if let Some(aid) = b.account_id {
        let acc = accounts::get_row(state, user.id(), aid).await?;
        let home = accounts::ensure_home(state, aid).await?;
        let envs = env::merge(vec![env::base(state), accounts::env_for(state, &acc)]);
        (format!("{} · {}", acc.kind().name(), acc.label), home, envs, None, Some(aid))
    } else if let Some(tid) = b.thread_id {
        let t = threads::get_row(state, user.id(), tid).await?;
        let dir = crate::workspace::thread_workdir(state, tid).await?;
        let mut lists = vec![env::base(state)];
        if let Some(aid) = t.account_id {
            if let Ok(acc) = accounts::get_row(state, user.id(), aid).await {
                accounts::sync_login(state, aid).await;
                // `claude` im Terminal: Ordner vertraut, Bypass-Modus ohne Rückfrage
                if acc.kind() == AgentKind::Claude {
                    accounts::prepare_claude(state, aid, &[&dir]).await;
                }
                lists.push(accounts::env_for(state, &acc));
            }
        }
        lists.push(crate::workspace::agent_env(state, user.id()).await?);
        (t.title.clone(), dir, env::merge(lists), Some(tid), t.account_id)
    } else if let Some(pid) = b.project_id {
        let name: Option<(String,)> = sqlx::query_as("SELECT name FROM projects WHERE id = ? AND user_id = ?").bind(pid).bind(user.id()).fetch_optional(&state.db).await?;
        let (name,) = name.ok_or(AppError::NotFound)?;
        let dir = state.config.workspaces_dir().join(pid.to_string()).join("repo");
        let dir = if dir.is_dir() { dir } else { state.config.workspaces_dir() };
        let envs = env::merge(vec![env::base(state), crate::workspace::agent_env(state, user.id()).await?]);
        (name, dir, envs, None, None)
    } else {
        let envs = env::merge(vec![env::base(state), crate::workspace::agent_env(state, user.id()).await?]);
        ("Terminal".into(), state.config.workspaces_dir(), envs, None, None)
    };
    let t = spawn(state, user.id(), &title, &cwd, envs, Launch::Shell(command), size, thread_id, account_id)?;
    Ok(Json(t.info()))
}

/// `POST /threads/<id>/cli` – das CLI des Agenten im Terminal statt des Chats: im Arbeitsordner
/// des Threads, mit dessen Konto und Modell; setzt die Sitzung des Threads fort (Grok kennt
/// keine: neue Sitzung). Läuft schon eines für den Thread, kommt dieses zurück. `409`, solange ein
/// Lauf arbeitet.
#[post("/threads/<id>/cli", data = "<body>")]
async fn open_cli(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<TerminalSize>) -> AppResult<Json<TerminalInfo>> {
    let t = threads::get_row(state, user.id(), id).await?;
    if let Some(open) = find_cli(state, user.id(), id) {
        return Ok(Json(open.info()));
    }
    if runtime::get(state).is_active(id) {
        return Err(AppError::conflict("Der Agent arbeitet gerade – das CLI geht nach dem Lauf auf."));
    }
    // Höchstens MAX_CLIS: die ältesten laufenden schliessen
    {
        let rt = runtime::get(state);
        let all = rt.terminals.lock().unwrap_or_else(|e| e.into_inner());
        let mut running: Vec<(i64, i64)> = all
            .values()
            .filter(|x| x.user_id == user.id() && x.exit.lock().map(|e| e.is_none()).unwrap_or(false))
            .filter_map(|x| x.cli_of.map(|tid| (x.created_at, tid)))
            .collect();
        drop(all);
        running.sort();
        let excess = (running.len() + 1).saturating_sub(MAX_CLIS);
        for (_, tid) in running.into_iter().take(excess) {
            remove(state, |x| x.cli_of == Some(tid));
        }
    }
    let kind = t.kind();
    let Some((program, _)) = tools::find_agent(state, kind) else {
        return Err(AppError::bad(format!("{} ist nicht installiert (Einstellungen → Werkzeuge).", kind.name())));
    };
    let dir = crate::workspace::thread_workdir(state, id).await?;
    let mut lists = vec![env::base(state)];
    if let Some(aid) = t.account_id {
        if let Ok(acc) = accounts::get_row(state, user.id(), aid).await {
            accounts::ensure_home(state, aid).await?;
            accounts::sync_login(state, aid).await;
            // Claude Code: ohne „Yes, I trust this folder“ und ohne Bestätigung des Bypass-Modus
            if kind == AgentKind::Claude {
                accounts::prepare_claude(state, aid, &[&dir]).await;
            }
            lists.push(accounts::env_for(state, &acc));
        }
    }
    lists.push(crate::workspace::agent_env(state, user.id()).await?);
    let (args, extra) = cli_args(kind, &t, &dir);
    lists.push(extra);
    let b = body.into_inner();
    let size = (if b.cols > 0 { b.cols } else { 100 }, if b.rows > 0 { b.rows } else { 30 });
    let title = format!("{} · {}", kind.name(), t.title);
    let launch = Launch::Cli { program, args, thread_id: id };
    let term = spawn(state, user.id(), &title, &dir, env::merge(lists), launch, size, None, t.account_id)?;
    Ok(Json(term.info()))
}

/// Argumente und Umgebung für das CLI eines Threads: Sitzung fortsetzen, Modell, und Freigaben wie
/// im Chat (ohne Rückfragen; „Nur lesen“: das CLI fragt bzw. lehnt Schreiben ab).
fn cli_args(kind: AgentKind, t: &threads::ThreadRow, dir: &Path) -> (Vec<String>, Vec<(String, String)>) {
    let session = (spec(kind).supports_resume && !t.provider_session_id.is_empty()).then(|| t.provider_session_id.clone());
    let model = (!t.model.is_empty()).then(|| t.model.clone());
    let mut args: Vec<String> = Vec::new();
    let mut env = Vec::new();
    match kind {
        AgentKind::Claude => {
            if let Some(s) = session {
                args.extend(["--resume".into(), s]);
            }
            if let Some(m) = model {
                args.extend(["--model".into(), m]);
            }
            // Bypass ausdrücklich (bzw. „Nur lesen“: fragen) – die Vorgabe im Konto ist Bypass
            let mode = if t.read_only { "default" } else { "bypassPermissions" };
            args.extend(["--permission-mode".into(), mode.into()]);
        }
        AgentKind::Codex => {
            if let Some(s) = session {
                args.extend(["resume".into(), s]);
            }
            if let Some(m) = model {
                args.extend(["-m".into(), m]);
            }
            if t.read_only {
                args.extend(["--sandbox".into(), "read-only".into()]);
            } else {
                args.push("--dangerously-bypass-approvals-and-sandbox".into());
            }
        }
        AgentKind::Gemini => {
            if let Some(s) = session {
                args.extend(["--resume".into(), s]);
            }
            if let Some(m) = model {
                args.extend(["-m".into(), m]);
            }
            if !t.read_only {
                args.push("--yolo".into());
            }
        }
        AgentKind::Opencode | AgentKind::Kilo => {
            if let Some(s) = session {
                args.extend(["-s".into(), s]);
            }
            if let Some(m) = model {
                args.extend(["-m".into(), m]);
            }
            let cfg = if t.read_only {
                serde_json::json!({"autoupdate": false, "permission": {"edit": "deny", "bash": "deny", "webfetch": "allow"}})
            } else {
                serde_json::json!({"autoupdate": false})
            };
            let var = if kind == AgentKind::Kilo { "KILO_CONFIG_CONTENT" } else { "OPENCODE_CONFIG_CONTENT" };
            env.push((var.into(), cfg.to_string()));
        }
        AgentKind::Grok => {
            args.extend(["--cwd".into(), dir.to_string_lossy().into_owned()]);
            if let Some(s) = session {
                args.extend(["--resume".into(), s]);
            }
            if let Some(m) = model {
                args.extend(["-m".into(), m]);
            }
            if t.read_only {
                args.extend(["--sandbox".into(), "read-only".into()]);
            } else {
                args.push("--always-approve".into());
            }
            env.push(("GROK_DISABLE_AUTOUPDATER".into(), "1".into()));
        }
    }
    (args, env)
}

/// Laufendes CLI eines Threads.
fn find_cli(state: &AppState, user_id: i64, thread_id: i64) -> Option<Arc<Terminal>> {
    let rt = runtime::get(state);
    let all = rt.terminals.lock().unwrap_or_else(|e| e.into_inner());
    all.values()
        .find(|t| t.user_id == user_id && t.cli_of == Some(thread_id) && t.exit.lock().map(|e| e.is_none()).unwrap_or(false))
        .cloned()
}

/// CLI eines Threads schliessen, weil ein Lauf startet: nur ein Programm schreibt in die Sitzung.
/// Exit-Code [`EXIT_FOR_RUN`] – die Oberfläche startet es nach dem Lauf neu.
pub fn close_cli(state: &AppState, thread_id: i64) {
    let rt = runtime::get(state);
    let mut all = rt.terminals.lock().unwrap_or_else(|e| e.into_inner());
    let ids: Vec<String> = all.values().filter(|t| t.cli_of == Some(thread_id)).map(|t| t.id.clone()).collect();
    for id in ids {
        if let Some(t) = all.remove(&id) {
            t.closed_for_run.store(true, Ordering::Relaxed);
            t.kill();
        }
    }
}

/// `GET /terminals` – offene Terminals des Benutzers.
#[get("/terminals")]
fn list(state: &State<AppState>, user: CurrentUser) -> Json<Vec<TerminalInfo>> {
    let rt = runtime::get(state);
    let all = rt.terminals.lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<TerminalInfo> = all.values().filter(|t| t.user_id == user.id()).map(|t| t.info()).collect();
    v.sort_by_key(|t| t.created_at);
    Json(v)
}

#[get("/terminals/<id>")]
fn info(state: &State<AppState>, user: CurrentUser, id: &str) -> AppResult<Json<TerminalInfo>> {
    Ok(Json(get_term(state, user.id(), id)?.info()))
}

/// `GET /terminals/<id>/stream` – Ausgabe (Base64-Stücke, zuerst der Puffer), am Ende das
/// Ereignis `exit` mit dem Exit-Code.
#[get("/terminals/<id>/stream")]
fn stream(state: &State<AppState>, user: CurrentUser, id: &str, mut end: Shutdown) -> AppResult<EventStream![Event + 'static]> {
    let t = get_term(state, user.id(), id)?;
    Ok(EventStream! {
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut rx = t.tx.subscribe();
        let initial = t.buffer.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if !initial.is_empty() {
            yield Event::data(b64.encode(&initial));
        }
        let exit = *t.exit.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(code) = exit {
            yield Event::data(code.to_string()).event("exit");
            return;
        }
        loop {
            rocket::tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(TermEvent::Data(d)) => yield Event::data(b64.encode(&d)),
                    Ok(TermEvent::Exit(code)) => {
                        yield Event::data(code.to_string()).event("exit");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                },
                _ = &mut end => break,
            }
        }
    })
}

/// `POST /terminals/<id>/input`
#[post("/terminals/<id>/input", data = "<body>")]
async fn input(state: &State<AppState>, user: CurrentUser, id: &str, body: Json<TerminalInput>) -> AppResult<Status> {
    let t = get_term(state, user.id(), id)?;
    if t.exit.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
        return Err(AppError::conflict("Das Terminal ist beendet."));
    }
    let data = body.into_inner().data.into_bytes();
    tokio::task::spawn_blocking(move || t.write(&data)).await.map_err(|e| AppError::internal(e.to_string()))??;
    Ok(Status::NoContent)
}

/// `POST /terminals/<id>/resize`
#[post("/terminals/<id>/resize", data = "<body>")]
fn resize(state: &State<AppState>, user: CurrentUser, id: &str, body: Json<TerminalSize>) -> AppResult<Status> {
    let t = get_term(state, user.id(), id)?;
    let cols = body.cols.clamp(10, 500);
    let rows = body.rows.clamp(4, 300);
    t.master
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| AppError::internal(format!("PTY: {e}")))?;
    *t.size.lock().unwrap_or_else(|e| e.into_inner()) = (cols, rows);
    Ok(Status::NoContent)
}

/// `DELETE /terminals/<id>` – Prozess beenden und Terminal schliessen.
#[delete("/terminals/<id>")]
fn close(state: &State<AppState>, user: CurrentUser, id: &str) -> AppResult<Status> {
    get_term(state, user.id(), id)?;
    remove(state, |t| t.id == id);
    Ok(Status::NoContent)
}
