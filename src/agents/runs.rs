//! Läufe: ein Aufruf des Agent-CLIs je Benutzernachricht. Der Prozess läuft im Arbeitsordner
//! des Threads (`workspace::thread_workdir`) mit der Umgebung des Kontos, nicht-interaktiv,
//! mit JSON-Ausgabe und ohne Rückfragen. Die Ausgabe wird zeilenweise vereinheitlicht
//! (`parse`) und als Nachrichten gespeichert und live gesendet. Höchstens
//! `config.max_parallel_runs` Läufe gleichzeitig, die übrigen warten in der Schlange.
//!
//! Jeder Lauf bekommt die MCP-Server `nrzx` (`http://127.0.0.1:<port>/mcp`, Token je Lauf)
//! und – falls verbunden – `nrzgit` (`<nrzgit>/mcp`),
//! jeweils mit dem Bearer-Token des Benutzers.

use super::accounts::{self, AccountRow};
use super::attachments::{self, AttachmentRow};
use super::kinds::spec;
use super::messages::{self, clip};
use super::parse::{describe, Ev, Parser};
use super::threads::{self, ThreadRow};
use super::{env, runtime, tools};
use crate::auth::session::{hash_token, new_token};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::{EditDiff, SystemData, ToolCall, UserMessageData};
use nrzx_shared::{AgentKind, Message, ThreadStatus};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::watch;

/// Werkzeuge von Claude Code im Nur-Lesen-Modus (lesen, suchen, lesende git-Befehle).
const CLAUDE_READ_ONLY_TOOLS: &[&str] = &[
    "Read",
    "Grep",
    "Glob",
    "LS",
    "WebSearch",
    "WebFetch",
    "TodoWrite",
    "Task",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git diff:*)",
    "Bash(git blame:*)",
    "Bash(git status:*)",
    "Bash(git grep:*)",
    "Bash(ls:*)",
];

/// Längste Eingabe als Programmargument (Linux: 128 KiB je Argument).
const MAX_ARG_PROMPT: usize = 100_000;

/// Lauf starten: Benutzernachricht speichern, Lauf anlegen (Warteschlange), Prozess im
/// Hintergrund. Liefert die Lauf-ID.
pub async fn start(state: &AppState, thread_id: i64, text: &str, attachment_ids: &[String]) -> AppResult<i64> {
    start_with(state, thread_id, text, attachment_ids, false).await
}

/// Wie [`start`]; `auto_resume` = von nrzX nach einem Rate-Limit gesendet (sonst setzt eine
/// Nachricht des Benutzers ein geplantes Weitermachen zurück).
pub async fn start_with(state: &AppState, thread_id: i64, text: &str, attachment_ids: &[String], auto_resume: bool) -> AppResult<i64> {
    start_inner(state, thread_id, text, text, attachment_ids, auto_resume, false).await
}

/// Zwischenruf senden: im Chat steht `shown`, das CLI bekommt `prompt` (mit Hinweis).
pub async fn start_interjection(state: &AppState, thread_id: i64, shown: &str, prompt: &str, attachment_ids: &[String]) -> AppResult<i64> {
    start_inner(state, thread_id, shown, prompt, attachment_ids, false, true).await
}

async fn start_inner(state: &AppState, thread_id: i64, text: &str, prompt: &str, attachment_ids: &[String], auto_resume: bool, interjection: bool) -> AppResult<i64> {
    // KI-Kette: Konto am Limit → gleich mit der nächsten Stufe beginnen
    if !runtime::get(state).is_active(thread_id) {
        if let Err(e) = super::chains::preflight(state, thread_id).await {
            tracing::warn!("KI-Kette vor dem Lauf von Thread {thread_id}: {e}");
        }
    }
    let t = threads::row(state, thread_id).await?;
    let rt = runtime::get(state);
    let atts = attachments::load(state, t.user_id, attachment_ids).await?;
    let account_id = t.account_id.ok_or_else(|| AppError::bad("Kein Konto für diesen Thread."))?;
    let (stop_tx, stop_rx) = watch::channel(false);
    let (soft_tx, soft_rx) = watch::channel(0u8);
    let slot = Arc::new(tokio::sync::Mutex::new(None));
    {
        let mut active = rt.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.contains_key(&thread_id) {
            return Err(AppError::conflict("Der Agent arbeitet noch – bitte warten oder stoppen."));
        }
        active.insert(thread_id, runtime::ActiveRun { run_id: 0, stop: stop_tx, soft: soft_tx, permit: slot.clone() });
    }
    let started = async {
        let data = messages::jv(&UserMessageData { attachments: atts.iter().map(AttachmentRow::to_json).collect(), auto_resume, interjection });
        let user_msg = messages::insert(state, t.user_id, thread_id, None, "user", text, &data).await?;
        let ts = now();
        let run_id: i64 = sqlx::query_scalar(
            "INSERT INTO runs (thread_id, agent_kind, account_id, model, effort, status, started_at) VALUES (?, ?, ?, ?, ?, 'queued', ?) RETURNING id",
        )
        .bind(thread_id)
        .bind(&t.agent_kind)
        .bind(account_id)
        .bind(&t.model)
        .bind(&t.effort)
        .bind(ts)
        .fetch_one(&state.db)
        .await?;
        sqlx::query("UPDATE messages SET run_id = ? WHERE id = ?").bind(run_id).bind(user_msg.id).execute(&state.db).await?;
        if t.title.trim().is_empty() {
            sqlx::query("UPDATE threads SET title = ? WHERE id = ?").bind(threads::title_from(text)).bind(thread_id).execute(&state.db).await?;
        }
        if auto_resume {
            sqlx::query("UPDATE threads SET resume_at = NULL WHERE id = ?").bind(thread_id).execute(&state.db).await?;
        } else {
            sqlx::query("UPDATE threads SET resume_at = NULL, resume_attempts = 0 WHERE id = ?").bind(thread_id).execute(&state.db).await?;
        }
        sqlx::query("UPDATE threads SET status = 'working', unread = 0, updated_at = ?, last_activity_at = ? WHERE id = ?")
            .bind(ts)
            .bind(ts)
            .bind(thread_id)
            .execute(&state.db)
            .await?;
        Ok::<(i64, i64), AppError>((run_id, user_msg.id))
    }
    .await;
    let (run_id, user_msg_id) = match started {
        Ok(v) => v,
        Err(e) => {
            rt.active.lock().unwrap_or_else(|e| e.into_inner()).remove(&thread_id);
            return Err(e);
        }
    };
    if let Some(a) = rt.active.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&thread_id) {
        a.run_id = run_id;
    }
    threads::publish(state, thread_id).await?;
    let job = Job { state: state.clone(), thread_id, run_id, user_msg_id, prompt: prompt.to_string(), attachments: atts, stop: stop_rx, soft: soft_rx, slot };
    tokio::spawn(job.run());
    Ok(run_id)
}

/// Lauf eines Threads stoppen (falls einer läuft). Wartet nicht.
pub fn signal_stop(state: &AppState, thread_id: i64) -> bool {
    let rt = runtime::get(state);
    let active = rt.active.lock().unwrap_or_else(|e| e.into_inner());
    match active.get(&thread_id) {
        Some(a) => {
            let _ = a.stop.send(true);
            true
        }
        None => false,
    }
}

/// So lange wartet ein Zwischenruf höchstens auf einen passenden Moment (Ende eines Werkzeugs
/// oder einer Antwort), dann wird der Lauf trotzdem beendet.
pub const SOFT_STOP_GRACE: Duration = Duration::from_secs(20);

/// Lauf eines Threads im nächsten passenden Moment beenden (Zwischenruf). `false` = läuft keiner.
pub fn signal_soft_stop(state: &AppState, thread_id: i64) -> bool {
    signal_interrupt(state, thread_id, SOFT)
}

/// Lauf eines Threads sofort für einen Zwischenruf beenden („Jetzt“ – ohne auf einen passenden
/// Moment zu warten und ohne „Gestoppt“). `false` = läuft keiner.
pub fn signal_interrupt_now(state: &AppState, thread_id: i64) -> bool {
    signal_interrupt(state, thread_id, NOW)
}

/// Stufen des Zwischenruf-Signals: im nächsten passenden Moment bzw. sofort.
const SOFT: u8 = 1;
const NOW: u8 = 2;

fn signal_interrupt(state: &AppState, thread_id: i64, level: u8) -> bool {
    let rt = runtime::get(state);
    let active = rt.active.lock().unwrap_or_else(|e| e.into_inner());
    match active.get(&thread_id) {
        Some(a) => {
            // Nie zurückstufen: ein „Jetzt“ bleibt ein „Jetzt“
            a.soft.send_modify(|v| *v = (*v).max(level));
            true
        }
        None => false,
    }
}

/// Thread und alle Subagenten (rekursiv) stoppen und warten, bis die Prozesse beendet sind.
pub async fn stop_tree(state: &AppState, thread_id: i64) -> AppResult<()> {
    let mut ids = vec![thread_id];
    let mut i = 0;
    while i < ids.len() {
        let kids: Vec<(i64,)> = sqlx::query_as("SELECT id FROM threads WHERE parent_id = ?").bind(ids[i]).fetch_all(&state.db).await?;
        ids.extend(kids.into_iter().map(|(k,)| k));
        i += 1;
    }
    let rt = runtime::get(state);
    for id in &ids {
        signal_stop(state, *id);
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while ids.iter().any(|id| rt.is_active(*id)) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Hängengebliebene Zustände ohne Prozess aufräumen
    for id in &ids {
        if !rt.is_active(*id) {
            let st: Option<(String,)> = sqlx::query_as("SELECT status FROM threads WHERE id = ?").bind(id).fetch_optional(&state.db).await?;
            if st.is_some_and(|(s,)| ThreadStatus::parse(&s).is_active()) {
                threads::set_status(state, *id, ThreadStatus::Idle, None).await?;
            }
        }
    }
    Ok(())
}

/// Nach einem Neustart: Läufe, die noch als laufend gelten, sind abgebrochen.
pub async fn recover(state: &AppState) -> AppResult<()> {
    let ts = now();
    sqlx::query("UPDATE runs SET status = 'error', error = 'Server neu gestartet', finished_at = ? WHERE status IN ('queued', 'running')")
        .bind(ts)
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE threads SET status = 'error', unread = 1, updated_at = ? WHERE status IN ('working', 'waiting')")
        .bind(ts)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM run_tokens").execute(&state.db).await?;
    let _ = tokio::fs::remove_dir_all(state.config.tmp_dir().join("runs")).await;
    Ok(())
}

// ------------------------------------------------------------------ Ausführung

struct Job {
    state: AppState,
    thread_id: i64,
    run_id: i64,
    user_msg_id: i64,
    prompt: String,
    attachments: Vec<AttachmentRow>,
    stop: watch::Receiver<bool>,
    soft: watch::Receiver<u8>,
    slot: Arc<tokio::sync::Mutex<Option<tokio::sync::OwnedSemaphorePermit>>>,
}

/// Ergebnis eines Laufs.
enum Outcome {
    Done,
    Stopped,
    /// Für einen Zwischenruf beendet: kein „Gestoppt“, die Warteschlange geht gleich raus
    Interrupted,
    Failed(String),
}

impl Job {
    /// Fester Rückgabetyp: über die Warteschlange startet ein Lauf am Ende den nächsten.
    fn run(self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(self.run_inner())
    }

    async fn run_inner(mut self) {
        let state = self.state.clone();
        let rt = runtime::get(&state);
        let thread_id = self.thread_id;
        let run_id = self.run_id;
        let rundir = state.config.tmp_dir().join("runs").join(run_id.to_string());
        let user_id = threads::row(&state, thread_id).await.map(|t| t.user_id).unwrap_or(0);
        let mut writer = Writer::new(state.clone(), user_id, thread_id, run_id);

        // Platz in der Warteschlange
        let permit = tokio::select! {
            p = rt.permits.clone().acquire_owned() => p.ok().map(Some),
            _ = stop_signal(&mut self.stop) => None,
            // Zwischenruf, bevor der Lauf überhaupt begann
            _ = interrupt_signal(&mut self.soft, SOFT) => Some(None),
        };
        let outcome = match permit {
            None => Outcome::Stopped,
            Some(None) => Outcome::Interrupted,
            Some(Some(p)) => {
                *self.slot.lock().await = Some(p);
                let _ = sqlx::query("UPDATE runs SET status = 'running', started_at = ? WHERE id = ?").bind(now()).bind(run_id).execute(&state.db).await;
                let _ = threads::publish(&state, thread_id).await;
                match self.execute(&rundir, &mut writer).await {
                    Ok(o) => o,
                    Err(e) => Outcome::Failed(e.to_string()),
                }
            }
        };
        if let Err(e) = writer.finish().await {
            tracing::warn!("Lauf {run_id}: {e}");
        }
        let stopped = matches!(outcome, Outcome::Stopped);
        if matches!(outcome, Outcome::Interrupted) {
            tracing::info!("Lauf {run_id}: für einen Zwischenruf beendet");
        }
        if let Err(e) = self.finalize(outcome, &writer).await {
            tracing::error!("Lauf {run_id} abschliessen: {e}");
        }
        rt.active.lock().unwrap_or_else(|e| e.into_inner()).remove(&thread_id);
        drop(self.slot.lock().await.take());
        let _ = tokio::fs::remove_dir_all(&rundir).await;
        if let Err(e) = crate::workspace::after_run(&state, thread_id).await {
            tracing::warn!("after_run {thread_id}: {e}");
        }
        let _ = threads::publish(&state, thread_id).await;
        let _ = rt.finished.send(thread_id);
        // Während des Laufs gesendete Nachrichten jetzt abschicken
        if !stopped {
            // Eigene Aufgabe: flush startet wieder einen Lauf
            let st = state.clone();
            tokio::spawn(async move {
                if let Err(e) = super::queue::flush(&st, thread_id).await {
                    tracing::warn!("Warteschlange {thread_id}: {e}");
                }
            });
        }
    }

    async fn finalize(&self, outcome: Outcome, w: &Writer) -> AppResult<()> {
        let state = &self.state;
        let ts = now();
        let (status, error, thread_status, unread) = match &outcome {
            Outcome::Stopped | Outcome::Interrupted => ("stopped", String::new(), ThreadStatus::Idle, false),
            Outcome::Failed(e) => ("error", e.clone(), ThreadStatus::Error, true),
            Outcome::Done => ("done", String::new(), ThreadStatus::Done, true),
        };
        if let Outcome::Failed(e) = &outcome {
            if w.errors == 0 {
                messages::insert(state, w.user_id, self.thread_id, Some(self.run_id), "error", e, &json!({})).await?;
            }
        }
        if let Outcome::Stopped = outcome {
            let data = messages::jv(&SystemData::of("stopped"));
            messages::insert(state, w.user_id, self.thread_id, Some(self.run_id), "system", "Gestoppt", &data).await?;
        }
        sqlx::query(
            "UPDATE runs SET status = ?, exit_code = ?, error = ?, input_tokens = ?, output_tokens = ?, cost_usd = ?, cache_read_tokens = ?, cache_write_tokens = ?, reasoning_tokens = ?, session_id = ?, finished_at = ? WHERE id = ?",
        )
        .bind(status)
        .bind(w.exit_code)
        .bind(clip(&error, 2000))
        .bind(w.tally.usage.0)
        .bind(w.tally.usage.1)
        .bind(w.tally.usage.2)
        .bind(w.tally.detail.0)
        .bind(w.tally.detail.1)
        .bind(w.tally.detail.2)
        .bind(w.session.clone().unwrap_or_default())
        .bind(ts)
        .bind(self.run_id)
        .execute(&state.db)
        .await?;
        sqlx::query("DELETE FROM run_tokens WHERE run_id = ?").bind(self.run_id).execute(&state.db).await?;
        if let Some(sid) = &w.session {
            sqlx::query("UPDATE threads SET provider_session_id = ? WHERE id = ?").bind(sid).bind(self.thread_id).execute(&state.db).await?;
        }
        sqlx::query("UPDATE threads SET status = ?, unread = ?, updated_at = ?, last_activity_at = ? WHERE id = ?")
            .bind(thread_status.key())
            .bind(unread)
            .bind(ts)
            .bind(ts)
            .bind(self.thread_id)
            .execute(&state.db)
            .await?;
        match &outcome {
            Outcome::Failed(e) => super::resume::after_failure(state, self.thread_id, self.run_id, e).await?,
            Outcome::Done => {
                sqlx::query("UPDATE threads SET resume_attempts = 0 WHERE id = ?").bind(self.thread_id).execute(&state.db).await?;
                // Fertig und nichts mehr in der Warteschlange: Subagenten werden nicht mehr gebraucht
                if super::queue::load(state, self.thread_id).await?.is_empty() {
                    super::subagents::close_finished(state, self.thread_id).await?;
                }
                let t = threads::row(state, self.thread_id).await?;
                super::subagents::close_if_orphaned(state, &t).await?;
            }
            Outcome::Stopped | Outcome::Interrupted => {}
        }
        Ok(())
    }

    async fn execute(&mut self, rundir: &Path, w: &mut Writer) -> AppResult<Outcome> {
        let state = self.state.clone();
        let t = threads::row(&state, self.thread_id).await?;
        let kind = t.kind();
        let account_id = t.account_id.ok_or_else(|| AppError::bad("Kein Konto für diesen Thread."))?;
        let acc = accounts::get_row(&state, t.user_id, account_id).await?;
        let Some((bin, _)) = tools::find_agent(&state, kind) else {
            return Ok(Outcome::Failed(format!("{} ist nicht installiert (Einstellungen → Werkzeuge).", kind.name())));
        };
        let workdir = crate::workspace::thread_workdir(&state, self.thread_id).await?;
        w.workdirs = vec![workdir.to_string_lossy().into_owned()];
        if let Ok(c) = tokio::fs::canonicalize(&workdir).await {
            w.workdirs.push(c.to_string_lossy().into_owned());
        }
        tokio::fs::create_dir_all(rundir).await?;
        accounts::ensure_home(&state, acc.id).await?;
        // Mit nrzCode verknüpfte Anmeldung: das neueste Token nehmen
        accounts::sync_login(&state, acc.id).await;
        accounts::touch(&state, acc.id).await?;
        let t = threads::row(&state, self.thread_id).await?; // Branch ggf. von thread_workdir gesetzt

        // MCP-Zugang dieses Laufs
        let token = new_token();
        sqlx::query("INSERT INTO run_tokens (token_hash, run_id, thread_id, created_at) VALUES (?, ?, ?, ?)")
            .bind(hash_token(&token))
            .bind(self.run_id)
            .bind(self.thread_id)
            .bind(now())
            .execute(&state.db)
            .await?;
        let mcp = Mcp {
            url: format!("http://127.0.0.1:{}/mcp", runtime::get(&state).port()),
            token,
            // nrzGit – wenn verbunden
            forges: crate::workspace::forge_accesses(&state, t.user_id)
                .await?
                .into_iter()
                .filter_map(|a| a.mcp().map(|(url, token)| Forge { name: a.server_name(), url, token }))
                .collect(),
            // Claude Code lädt MCP-Werkzeuge bei Bedarf (Tool Search) – die anderen CLIs alle vorab
            gateway: !uses_tool_search(kind),
            bot: match t.bot_id {
                Some(b) => super::bots::mcp_servers(&state, b).await,
                None => Vec::new(),
            },
        };

        // Anhänge in den Laufordner kopieren
        let mut files: Vec<(PathBuf, bool, String)> = Vec::new();
        if !self.attachments.is_empty() {
            let dir = rundir.join("attachments");
            tokio::fs::create_dir_all(&dir).await?;
            for (i, a) in self.attachments.iter().enumerate() {
                let target = dir.join(format!("{}-{}", i + 1, a.name));
                tokio::fs::copy(attachments::file_path(&state, &a.uuid)?, &target).await?;
                files.push((target, a.is_image(), a.content_type.clone()));
            }
        }

        let resume = if spec(kind).supports_resume && !t.provider_session_id.is_empty() { Some(t.provider_session_id.clone()) } else { None };
        let history = if resume.is_none() { history(&state, self.thread_id, self.user_msg_id).await? } else { String::new() };
        // Gespräch mit einem Bot: Persönlichkeit, Gedächtnis und Arbeitsordner statt Coding-Hinweis;
        // Deep Research und das Gespräch darüber: eigener Hinweis (Paper schreiben bzw. darüber reden)
        let hint = match super::bots::hint(&state, &t, &workdir, &forge_hint(&mcp)).await {
            Some(h) => h,
            None => match super::research::hint(&state, &t, &workdir).await {
                Some(h) => h,
                None => format!("{}{}", system_hint(&t, &workdir, &mcp), crate::team::hint(&state, &t).await),
            },
        };
        // Mit `@@name` einbezogene Repositories (andere Projekte) als Referenz
        let refs = crate::workspace::referenced_repos(&state, t.user_id, &self.prompt, t.project_id).await.unwrap_or_default();
        let ctx = Ctx { kind, t: &t, acc: &acc, workdir: &workdir, rundir, mcp: &mcp, hint, history, prompt: &self.prompt, files: &files, refs: &refs, resume };
        let inv = invocation(&state, &ctx).await?;

        let envs = env::merge(vec![
            env::base(&state),
            accounts::env_for(&state, &acc),
            crate::workspace::agent_env(&state, t.user_id).await?,
            inv.env.clone(),
        ]);
        let mut cmd = crate::platform::command(&bin);
        cmd.args(&inv.args)
            .current_dir(&workdir)
            .env_clear()
            .envs(envs)
            .stdin(if inv.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        crate::platform::own_group(&mut cmd);
        // Ein offenes CLI des Threads schreibt in dieselbe Sitzung: schliessen
        super::terminals::close_cli(&state, self.thread_id);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Ok(Outcome::Failed(format!("{} startet nicht: {e}", bin.display()))),
        };
        let pid = child.id();
        if let (Some(input), Some(mut stdin)) = (inv.stdin, child.stdin.take()) {
            tokio::spawn(async move {
                let _ = stdin.write_all(&input).await;
                let _ = stdin.shutdown().await;
            });
        }
        let stderr_tail = Arc::new(std::sync::Mutex::new(String::new()));
        if let Some(mut err) = child.stderr.take() {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = err.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
                    t.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if t.len() > 16_000 {
                        let cut = t.len() - 12_000;
                        let cut = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(cut);
                        t.replace_range(..cut, "");
                    }
                }
            });
        }
        let stdout = child.stdout.take().ok_or_else(|| AppError::internal("stdout fehlt"))?;
        let mut reader = BufReader::with_capacity(1 << 16, stdout);
        let mut parser = Parser::new(kind);
        let mut line = Vec::new();
        let mut exit: Option<std::process::ExitStatus> = None;
        let mut stopped = false;
        let mut drain_until: Option<tokio::time::Instant> = None;
        // Zwischenruf: auf einen passenden Moment warten (kein Werkzeug offen und eines bzw. eine
        // Antwort ist gerade fertig), spätestens bis `soft_until`
        let mut soft_until: Option<tokio::time::Instant> = None;
        let mut interrupted = false;
        let mut open_tools: usize = 0;
        // „Jetzt“: eigener Empfänger, damit beide Stufen nebeneinander warten können
        let mut now = self.soft.clone();
        loop {
            tokio::select! {
                biased;
                _ = stop_signal(&mut self.stop) => {
                    stopped = true;
                    break;
                }
                _ = interrupt_signal(&mut now, NOW) => {
                    (stopped, interrupted) = (true, true);
                    break;
                }
                _ = interrupt_signal(&mut self.soft, SOFT), if soft_until.is_none() => {
                    soft_until = Some(tokio::time::Instant::now() + SOFT_STOP_GRACE);
                }
                _ = tokio::time::sleep_until(soft_until.unwrap_or_else(tokio::time::Instant::now)), if soft_until.is_some() && drain_until.is_none() => {
                    (stopped, interrupted) = (true, true);
                    break;
                }
                r = reader.read_until(b'\n', &mut line) => match r {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&line);
                        let mut moment = false;
                        for ev in parser.line(&text) {
                            match &ev {
                                Ev::ToolStart { .. } => open_tools += 1,
                                Ev::ToolEnd { .. } => {
                                    open_tools = open_tools.saturating_sub(1);
                                    moment = true;
                                }
                                Ev::Text(_) | Ev::CloseText => moment = true,
                                _ => {}
                            }
                            w.apply(ev).await?;
                        }
                        // Erst nach einer ganzen Zeile leeren: wird das Lesen von einem anderen
                        // Zweig unterbrochen, bleibt der Anfang der Zeile erhalten
                        line.clear();
                        // Erst am Ende der Zeile prüfen: Text und Werkzeugbeginn kommen oft zusammen
                        if moment && open_tools == 0 && soft_until.is_some() && exit.is_none() {
                            (stopped, interrupted) = (true, true);
                            break;
                        }
                    }
                },
                s = child.wait(), if exit.is_none() => {
                    exit = s.ok();
                    // Programm beendet: Rest der Ausgabe noch kurz lesen
                    drain_until = Some(tokio::time::Instant::now() + Duration::from_secs(3));
                }
                _ = tokio::time::sleep_until(drain_until.unwrap_or_else(tokio::time::Instant::now)), if drain_until.is_some() => break,
            }
        }
        if stopped {
            // Ganze Prozessgruppe beenden (auch Befehle, die der Agent gestartet hat)
            if let Some(pid) = pid {
                crate::platform::kill_tree(pid, false);
            }
            if tokio::time::timeout(Duration::from_secs(3), child.wait()).await.is_err() {
                if let Some(pid) = pid {
                    crate::platform::kill_tree(pid, true);
                }
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            return Ok(if interrupted { Outcome::Interrupted } else { Outcome::Stopped });
        }
        if exit.is_none() {
            // Ausgabe zu, Programm läuft noch: kurz warten, dann beenden
            exit = match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
                Ok(Ok(s)) => Some(s),
                _ => {
                    if let Some(pid) = pid {
                        crate::platform::kill_tree(pid, true);
                    }
                    let _ = child.start_kill();
                    child.wait().await.ok()
                }
            };
        }
        w.exit_code = exit.and_then(|s| s.code()).map(i64::from);
        let code = w.exit_code.unwrap_or(-1);
        let tail = stderr_tail.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(err) = &w.result_error {
            return Ok(Outcome::Failed(if err.is_empty() { format!("{} meldet einen Fehler.", kind.name()) } else { err.clone() }));
        }
        if code != 0 {
            let lines: Vec<&str> = tail.lines().filter(|l| !l.trim().is_empty()).collect();
            let last = lines[lines.len().saturating_sub(15)..].join("\n");
            let msg = if last.is_empty() {
                format!("{} wurde mit Code {code} beendet.", kind.name())
            } else {
                format!("{} wurde mit Code {code} beendet:\n{}", kind.name(), super::parse::strip_ansi(&last))
            };
            return Ok(Outcome::Failed(msg));
        }
        Ok(Outcome::Done)
    }
}

/// Kurzer Nebenlauf ohne Werkzeuge mit Schreibrechten und ohne MCP (z. B. für `/btw`): gleiches
/// CLI und Konto wie der Thread, aber eigene Sitzung, günstiges Modell und niedrige Stufe. Der
/// Thread selbst (Sitzung, Verlauf, laufender Agent) bleibt unberührt. Liefert die Antwort.
/// Eigene Wahl für einen Nebenlauf (Agent, Konto, Modell, Stufe), z. B. für den Zwischenstand.
#[derive(Debug, Clone, Default)]
pub struct SidePick {
    pub agent: AgentKind,
    pub account_id: Option<i64>,
    pub model: String,
    pub effort: String,
}

pub async fn oneshot(state: &AppState, thread_id: i64, prompt: &str, pick: Option<SidePick>) -> AppResult<String> {
    let t = threads::row(state, thread_id).await?;
    let workdir = crate::workspace::thread_workdir(state, thread_id).await?;
    let hint = "You answer side questions about the progress of another coding agent from the transcript you are given. \
                Do not modify anything. Be brief. Reply in the user's language."
        .to_string();
    side_run(state, t, &workdir, prompt, pick, hint).await
}

/// Kurzer Nebenlauf ohne Thread (Sprachsteuerung „Dieter“): nur lesen, in einem leeren
/// Arbeitsordner, ohne Modell ein schnelles (Claude: Haiku) mit niedriger Stufe.
pub async fn oneshot_user(state: &AppState, user_id: i64, prompt: &str, mut pick: SidePick, hint: String) -> AppResult<String> {
    if pick.model.trim().is_empty() {
        pick.model = crate::voice::dieter::fast_model(pick.agent).to_string();
        if pick.effort.is_empty() {
            pick.effort = crate::voice::dieter::fast_effort(pick.agent);
        }
    }
    let ts = now();
    let t = ThreadRow {
        id: 0,
        user_id,
        project_id: None,
        parent_id: None,
        title: "Dieter".into(),
        status: "idle".into(),
        agent_kind: pick.agent.key().into(),
        account_id: None,
        model: String::new(),
        effort: String::new(),
        branch: String::new(),
        use_worktree: false,
        worktree_path: String::new(),
        provider_session_id: String::new(),
        unread: false,
        pinned: false,
        hidden: true,
        sort_key: None,
        created_at: ts,
        updated_at: ts,
        last_activity_at: ts,
        read_only: true,
        resume_at: None,
        freestyle: false,
        resume_auto: false,
        queued: String::new(),
        additions: 0,
        deletions: 0,
        bot_id: None,
        research: false,
        discuss_of: None,
    };
    let workdir = state.config.tmp_dir().join("dieter").join(uuid::Uuid::new_v4().simple().to_string());
    tokio::fs::create_dir_all(&workdir).await?;
    let result = side_run(state, t, &workdir, prompt, Some(pick), hint).await;
    let _ = tokio::fs::remove_dir_all(&workdir).await;
    result
}

/// Nebenlauf mit den Daten von `t` (Benutzer, Konto, Modell) in `workdir`.
async fn side_run(state: &AppState, t: ThreadRow, workdir: &Path, prompt: &str, pick: Option<SidePick>, hint: String) -> AppResult<String> {
    let (kind, acc) = match &pick {
        // Eigene Wahl: Konto muss dem Benutzer gehören und zum Agenten passen
        Some(p) => {
            let acc = match p.account_id {
                Some(id) => Some(accounts::get_row(state, t.user_id, id).await?).filter(|a| a.kind == p.agent.key()),
                None => None,
            };
            let acc = match acc {
                Some(a) => a,
                None => accounts::default_for(state, t.user_id, p.agent)
                    .await?
                    .ok_or_else(|| AppError::bad(format!("Kein Konto für {}.", p.agent.name())))?,
            };
            (p.agent, acc)
        }
        None => {
            let account_id = t.account_id.ok_or_else(|| AppError::bad("Kein Konto für diesen Thread."))?;
            (t.kind(), accounts::get_row(state, t.user_id, account_id).await?)
        }
    };
    let Some((bin, _)) = tools::find_agent(state, kind) else {
        return Err(AppError::bad(format!("{} ist nicht installiert.", kind.name())));
    };
    let rundir = state.config.tmp_dir().join("oneshot").join(uuid::Uuid::new_v4().simple().to_string());
    tokio::fs::create_dir_all(&rundir).await?;
    accounts::ensure_home(state, acc.id).await?;

    let mut side = t.clone();
    side.read_only = true;
    side.provider_session_id = String::new();
    side.agent_kind = kind.key().to_string();
    side.account_id = Some(acc.id);
    match &pick {
        Some(p) => {
            side.model = p.model.clone();
            side.effort = if p.effort.is_empty() || spec(kind).effort(&p.effort).is_some() { p.effort.clone() } else { String::new() };
        }
        None => {
            if kind == AgentKind::Claude {
                side.model = "haiku".into();
            }
            side.effort = if spec(kind).effort("low").is_some() { "low".into() } else { String::new() };
        }
    }
    let mcp = Mcp { url: String::new(), token: String::new(), forges: Vec::new(), gateway: true, bot: Vec::new() };
    let ctx = Ctx { kind, t: &side, acc: &acc, workdir, rundir: &rundir, mcp: &mcp, hint, history: String::new(), prompt, files: &[], refs: &[], resume: None };
    let inv = invocation(state, &ctx).await;
    let result = async {
        let inv = inv?;
        let envs = env::merge(vec![env::base(state), accounts::env_for(state, &acc), crate::workspace::agent_env(state, t.user_id).await?, inv.env.clone()]);
        let mut cmd = crate::platform::command(&bin);
        cmd.args(&inv.args)
            .current_dir(&workdir)
            .env_clear()
            .envs(envs)
            .stdin(if inv.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| AppError::internal(format!("{} startet nicht: {e}", bin.display())))?;
        if let (Some(input), Some(mut stdin)) = (inv.stdin, child.stdin.take()) {
            tokio::spawn(async move {
                let _ = stdin.write_all(&input).await;
                let _ = stdin.shutdown().await;
            });
        }
        let stdout = child.stdout.take().ok_or_else(|| AppError::internal("stdout fehlt"))?;
        let mut stderr = child.stderr.take();
        let started = now();
        let read = async {
            let mut reader = BufReader::new(stdout);
            let mut parser = Parser::new(kind);
            let (mut text, mut last, mut error) = (String::new(), String::new(), None::<String>);
            let mut tally = Tally::default();
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                for ev in parser.line(&String::from_utf8_lossy(&line)) {
                    tally.apply(&ev);
                    match ev {
                        Ev::TextDelta(d) => text.push_str(&d),
                        Ev::Text(t) => {
                            last = t;
                            text.clear();
                        }
                        Ev::Result { error: true, text: t } => error = Some(t),
                        Ev::Result { error: false, text: t } if last.trim().is_empty() && text.trim().is_empty() => last = t,
                        Ev::Error(e) => error = Some(e),
                        _ => {}
                    }
                }
            }
            (if text.trim().is_empty() { last } else { text }, error, tally)
        };
        let (answer, error, tally) = match tokio::time::timeout(Duration::from_secs(180), read).await {
            Ok(v) => v,
            Err(_) => {
                let _ = child.start_kill();
                return Err(AppError::internal("Keine Antwort innerhalb von 3 Minuten."));
            }
        };
        log_side_usage(state, &side, &tally, started, error.is_none() && !answer.trim().is_empty()).await;
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.ok().and_then(|s| s.ok());
        if answer.trim().is_empty() {
            let mut tail = String::new();
            if let Some(err) = stderr.as_mut() {
                let _ = tokio::time::timeout(Duration::from_secs(1), err.read_to_string(&mut tail)).await;
            }
            let msg = error.filter(|e| !e.trim().is_empty()).unwrap_or_else(|| {
                let last: Vec<&str> = tail.lines().filter(|l| !l.trim().is_empty()).collect();
                last[last.len().saturating_sub(5)..].join("\n")
            });
            return Err(AppError::internal(if msg.is_empty() {
                format!("{} hat nicht geantwortet (Code {:?}).", kind.name(), status.and_then(|s| s.code()))
            } else {
                super::parse::strip_ansi(&msg)
            }));
        }
        Ok(answer.trim().to_string())
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&rundir).await;
    result
}

// ------------------------------------------------------------------ Verbrauch

/// Verbrauch eines Nebenlaufs (Seiten-Chat, Dieter) in die Nutzung schreiben – er hat keinen
/// eigenen Lauf in `runs`, zählt aber beim Abo mit.
async fn log_side_usage(state: &AppState, t: &ThreadRow, tally: &Tally, started: i64, ok: bool) {
    if tally.usage.0 == 0 && tally.usage.1 == 0 && tally.usage.2 == 0.0 {
        return;
    }
    let r = sqlx::query(
        "INSERT INTO usage_log (run_id, user_id, thread_id, thread_key, agent_kind, account_id, model, status, input_tokens, output_tokens, \
         cache_read_tokens, cache_write_tokens, reasoning_tokens, cost_usd, started_at, finished_at) \
         VALUES (NULL, ?, ?, (SELECT u.thread_key FROM usage_log u WHERE u.thread_id = ? AND u.run_id IS NOT NULL LIMIT 1), \
         ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(t.user_id)
    .bind((t.id > 0).then_some(t.id))
    .bind(t.id)
    .bind(&t.agent_kind)
    .bind(t.account_id)
    .bind(&t.model)
    .bind(if ok { "done" } else { "error" })
    .bind(tally.usage.0)
    .bind(tally.usage.1)
    .bind(tally.detail.0)
    .bind(tally.detail.1)
    .bind(tally.detail.2)
    .bind(tally.usage.2)
    .bind(started)
    .bind(now())
    .execute(&state.db)
    .await;
    if let Err(e) = r {
        tracing::warn!("Nebenlauf: Verbrauch speichern: {e}");
    }
}

/// Verbrauch eines Laufs aus den Ereignissen des CLIs.
#[derive(Debug, Default)]
pub struct Tally {
    /// Eingabe (mit Cache), Ausgabe, Kosten
    pub usage: (i64, i64, f64),
    /// Cache gelesen, Cache geschrieben, Denk-Tokens
    pub detail: (i64, i64, i64),
    /// Stand je Antwort des Modells während des Laufs (Claude): Eingabe, Ausgabe, Cache gelesen,
    /// Cache geschrieben
    live: HashMap<String, [i64; 4]>,
    /// Gesamtverbrauch vom CLI gemeldet (ersetzt den laufenden Stand)
    total: bool,
}

impl Tally {
    /// Ereignis übernehmen; `true`, wenn sich der Verbrauch geändert hat.
    pub fn apply(&mut self, ev: &Ev) -> bool {
        match *ev {
            Ev::UsageAdd { input, output, cost, cache_read, cache_write, reasoning } => {
                self.usage.0 += input;
                self.usage.1 += output;
                self.usage.2 += cost;
                self.detail.0 += cache_read;
                self.detail.1 += cache_write;
                self.detail.2 += reasoning;
                true
            }
            Ev::UsageTotal { input, output, cost, cache_read, cache_write, reasoning } => {
                self.usage = (input, output, cost);
                self.detail = (cache_read, cache_write, reasoning);
                self.total = true;
                true
            }
            Ev::UsageLive { ref id, input, output, cache_read, cache_write } => {
                if self.total {
                    return false;
                }
                let e = self.live.entry(id.clone()).or_insert([0; 4]);
                let before = *e;
                for (slot, v) in e.iter_mut().zip([input, output, cache_read, cache_write]) {
                    *slot = (*slot).max(v);
                }
                if *e == before {
                    return false;
                }
                let sum = self.live.values().fold([0i64; 4], |mut a, x| {
                    for (s, v) in a.iter_mut().zip(x) {
                        *s += v;
                    }
                    a
                });
                self.usage.0 = sum[0];
                self.usage.1 = sum[1];
                self.detail.0 = sum[2];
                self.detail.1 = sum[3];
                true
            }
            _ => false,
        }
    }
}

// ------------------------------------------------------------------ Nachrichten schreiben

/// Übersetzt die einheitlichen Ereignisse in Nachrichten (gestreamter Text wird fortgeschrieben).
pub struct Writer {
    state: AppState,
    pub user_id: i64,
    thread_id: i64,
    run_id: i64,
    pub workdirs: Vec<String>,
    text: Option<(Message, bool)>,
    last_flush: Instant,
    raw: Option<Message>,
    tools: HashMap<String, (Message, ToolCall)>,
    /// Dateien vor einer Bearbeitung (je Werkzeug-ID): Pfad → Inhalt (None = gab es nicht)
    pre: HashMap<String, Vec<(PathBuf, Option<String>)>>,
    /// Zuletzt gesehener Inhalt bearbeiteter Dateien (Agenten, die erst nach dem Ändern melden)
    seen: HashMap<PathBuf, String>,
    pub session: Option<String>,
    /// Verbrauch (Tokens, Kosten) aus den Ereignissen des CLIs
    pub tally: Tally,
    /// Zuletzt gespeicherter Verbrauch (während des Laufs alle paar Sekunden)
    usage_saved: Instant,
    pub errors: usize,
    pub result_error: Option<String>,
    pub texts: usize,
    pub exit_code: Option<i64>,
}

impl Writer {
    fn new(state: AppState, user_id: i64, thread_id: i64, run_id: i64) -> Self {
        Writer {
            state,
            user_id,
            thread_id,
            run_id,
            workdirs: Vec::new(),
            text: None,
            last_flush: Instant::now(),
            raw: None,
            tools: HashMap::new(),
            pre: HashMap::new(),
            seen: HashMap::new(),
            session: None,
            tally: Tally::default(),
            usage_saved: Instant::now().checked_sub(Duration::from_secs(60)).unwrap_or_else(Instant::now),
            errors: 0,
            result_error: None,
            texts: 0,
            exit_code: None,
        }
    }

    async fn insert(&self, role: &str, content: &str, data: &Value) -> AppResult<Message> {
        messages::insert(&self.state, self.user_id, self.thread_id, Some(self.run_id), role, content, data).await
    }

    /// Stand der Dateien merken, die ein Werkzeug gleich bearbeitet (sobald ihr Pfad bekannt ist).
    /// Codex meldet Dateiänderungen erst danach – dort hilft das nicht.
    async fn remember_files(&mut self, id: &str, call: &ToolCall) {
        if !matches!(call.kind.as_str(), "edit" | "write") || call.name == "file_change" {
            return;
        }
        let Some(workdir) = self.workdirs.first().map(PathBuf::from) else { return };
        for path in super::edits::paths(call, &workdir) {
            if self.pre.get(id).is_some_and(|l| l.iter().any(|(p, _)| *p == path)) {
                continue;
            }
            if let Some(content) = super::edits::snapshot(&path).await {
                self.pre.entry(id.to_string()).or_default().push((path, content));
            }
        }
    }

    /// Abschnitte einer abgeschlossenen Bearbeitung (None: nichts Genaueres als bisher).
    async fn exact_diffs(&mut self, id: &str, call: &ToolCall) -> Option<Vec<EditDiff>> {
        use super::edits;
        let workdir = PathBuf::from(self.workdirs.first()?);
        let pre = self.pre.get(id).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for path in edits::paths(call, &workdir) {
            let Some(now) = edits::snapshot(&path).await else { continue };
            let new = now.unwrap_or_default();
            let before = pre.iter().find(|(p, _)| *p == path).map(|(_, c)| c.clone().unwrap_or_default()).filter(|o| *o != new);
            let old = match before {
                Some(o) => Some(o),
                None => match edits::reconstruct(&new, &call.diffs).or_else(|| self.seen.get(&path).filter(|o| **o != new).cloned()) {
                    Some(o) => Some(o),
                    None => edits::git_head(&workdir, &path).await.filter(|o| *o != new),
                },
            };
            if let Some(old) = old {
                out.extend(edits::region_diffs(&edits::shown_path(&path, &self.workdirs), &old, &new));
            }
            self.seen.insert(path, new);
        }
        (!out.is_empty()).then_some(out)
    }

    /// Verbrauch schon während des Laufs speichern (höchstens alle 5 Sekunden): die Nutzung zählt
    /// laufende Sitzungen mit, gestoppte und abgebrochene behalten ihren Stand.
    async fn save_usage(&mut self) {
        if self.usage_saved.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.usage_saved = Instant::now();
        let t = &self.tally;
        let r = sqlx::query(
            "UPDATE runs SET input_tokens = ?, output_tokens = ?, cost_usd = ?, cache_read_tokens = ?, cache_write_tokens = ?, reasoning_tokens = ? WHERE id = ?",
        )
        .bind(t.usage.0)
        .bind(t.usage.1)
        .bind(t.usage.2)
        .bind(t.detail.0)
        .bind(t.detail.1)
        .bind(t.detail.2)
        .bind(self.run_id)
        .execute(&self.state.db)
        .await;
        if let Err(e) = r {
            tracing::warn!("Lauf {}: Verbrauch speichern: {e}", self.run_id);
        }
    }

    async fn close_text(&mut self) -> AppResult<()> {
        if let Some((mut m, _)) = self.text.take() {
            let content = m.content.clone();
            messages::update(&self.state, self.user_id, &mut m, &content, &json!({})).await?;
        }
        Ok(())
    }

    async fn close_raw(&mut self) -> AppResult<()> {
        if let Some(mut m) = self.raw.take() {
            let content = m.content.clone();
            let data = m.data.clone();
            messages::update(&self.state, self.user_id, &mut m, &content, &data).await?;
        }
        Ok(())
    }

    pub async fn apply(&mut self, ev: Ev) -> AppResult<()> {
        if !matches!(ev, Ev::Raw(_)) {
            self.close_raw().await?;
        }
        match ev {
            Ev::Session(id) => {
                sqlx::query("UPDATE threads SET provider_session_id = ? WHERE id = ?").bind(&id).bind(self.thread_id).execute(&self.state.db).await?;
                self.session = Some(id);
            }
            Ev::TextDelta(t) => {
                if t.is_empty() {
                    return Ok(());
                }
                match &mut self.text {
                    Some((m, _)) => {
                        m.content.push_str(&t);
                        if self.last_flush.elapsed() > Duration::from_millis(250) {
                            let content = m.content.clone();
                            messages::update(&self.state, self.user_id, m, &content, &json!({})).await?;
                            self.last_flush = Instant::now();
                        }
                    }
                    None => {
                        let m = self.insert("assistant", &t, &json!({})).await?;
                        self.texts += 1;
                        self.text = Some((m, true));
                        self.last_flush = Instant::now();
                    }
                }
            }
            Ev::Text(t) => match self.text.take() {
                Some((mut m, true)) => {
                    messages::update(&self.state, self.user_id, &mut m, &t, &json!({})).await?;
                }
                other => {
                    self.text = other;
                    self.close_text().await?;
                    self.insert("assistant", &t, &json!({})).await?;
                    self.texts += 1;
                }
            },
            Ev::CloseText => self.close_text().await?,
            Ev::Thinking(t) => {
                self.close_text().await?;
                let first: String = t.lines().map(|l| l.trim().trim_matches('*')).find(|l| !l.is_empty()).unwrap_or("").chars().take(100).collect();
                let call = ToolCall {
                    tool_id: String::new(),
                    name: "thinking".into(),
                    kind: "think".into(),
                    target: first.clone(),
                    summary: first,
                    input: Value::Null,
                    output: clip(&t, 8000),
                    status: "ok".into(),
                    diffs: Vec::new(),
                    started_at: now(),
                    finished_at: Some(now()),
                };
                self.insert("tool", "", &messages::jv(&call)).await?;
            }
            Ev::ToolStart { id, name, input } => {
                self.close_text().await?;
                if self.tools.contains_key(&id) && !id.is_empty() {
                    return Ok(());
                }
                let (kind, target, summary, diffs) = describe(&name, &input, &self.workdirs);
                let call = ToolCall {
                    tool_id: id.clone(),
                    name,
                    kind,
                    target,
                    summary,
                    input: clip_json(input),
                    output: String::new(),
                    status: "running".into(),
                    diffs,
                    started_at: now(),
                    finished_at: None,
                };
                let m = self.insert("tool", "", &messages::jv(&call)).await?;
                self.remember_files(&id, &call).await;
                self.tools.insert(id, (m, call));
            }
            Ev::ToolUpdate { id, input } => {
                let mut snap = None;
                if let Some((m, call)) = self.tools.get_mut(&id) {
                    let (kind, target, summary, diffs) = describe(&call.name, &input, &self.workdirs);
                    call.kind = kind;
                    call.target = target;
                    call.summary = summary;
                    call.diffs = diffs;
                    call.input = clip_json(input);
                    let data = messages::jv(&*call);
                    messages::update(&self.state, self.user_id, m, "", &data).await?;
                    snap = Some(call.clone());
                }
                if let Some(call) = snap {
                    self.remember_files(&id, &call).await;
                }
            }
            Ev::ToolEnd { id, output, error } => {
                if !self.tools.contains_key(&id) {
                    let call = ToolCall {
                        tool_id: id.clone(),
                        name: "tool".into(),
                        kind: "other".into(),
                        summary: "tool".into(),
                        status: "running".into(),
                        started_at: now(),
                        ..Default::default()
                    };
                    let m = self.insert("tool", "", &messages::jv(&call)).await?;
                    self.tools.insert(id.clone(), (m, call));
                }
                // Bearbeitungen: genaue Abschnitte mit Zeilennummern (Datei vorher/nachher)
                let exact = match self.tools.get(&id) {
                    Some((_, call)) if !error && matches!(call.kind.as_str(), "edit" | "write") => {
                        let call = call.clone();
                        self.exact_diffs(&id, &call).await
                    }
                    _ => None,
                };
                self.pre.remove(&id);
                if let Some((m, call)) = self.tools.get_mut(&id) {
                    call.output = clip(&super::parse::strip_ansi(&output), 6000);
                    call.status = if error { "error".into() } else { "ok".into() };
                    call.finished_at = Some(now());
                    if let Some(d) = exact {
                        call.diffs = d;
                    }
                    let data = messages::jv(&*call);
                    messages::update(&self.state, self.user_id, m, "", &data).await?;
                }
            }
            ev @ (Ev::UsageAdd { .. } | Ev::UsageTotal { .. } | Ev::UsageLive { .. }) => {
                if self.tally.apply(&ev) {
                    self.save_usage().await;
                }
            }
            Ev::McpFailed(names) => {
                // Nur melden, wenn sich die Liste seit der letzten Meldung im Thread geändert hat
                let list = names.join(", ");
                let last: Option<String> = sqlx::query_scalar(
                    "SELECT content FROM messages WHERE thread_id = ? AND role = 'system' AND data LIKE '%\"kind\":\"mcp\"%' ORDER BY id DESC LIMIT 1",
                )
                .bind(self.thread_id)
                .fetch_optional(&self.state.db)
                .await?;
                if last.as_deref() != Some(list.as_str()) {
                    self.close_text().await?;
                    let data = messages::jv(&SystemData::of("mcp"));
                    self.insert("system", &list, &data).await?;
                }
            }
            Ev::System(t) => {
                self.close_text().await?;
                let data = messages::jv(&SystemData::of("notice"));
                self.insert("system", &t, &data).await?;
            }
            Ev::Error(t) => {
                if t.trim().is_empty() {
                    return Ok(());
                }
                self.close_text().await?;
                self.insert("error", &clip(&t, 4000), &json!({})).await?;
                self.errors += 1;
            }
            Ev::Raw(t) => {
                if t.trim().is_empty() {
                    return Ok(());
                }
                match &mut self.raw {
                    Some(m) => {
                        if m.content.len() < 20_000 {
                            m.content.push('\n');
                            m.content.push_str(&t);
                        }
                        if self.last_flush.elapsed() > Duration::from_millis(500) {
                            let content = m.content.clone();
                            let data = m.data.clone();
                            messages::update(&self.state, self.user_id, m, &content, &data).await?;
                            self.last_flush = Instant::now();
                        }
                    }
                    None => {
                        let data = messages::jv(&SystemData::of("raw"));
                        self.raw = Some(self.insert("system", &t, &data).await?);
                    }
                }
            }
            Ev::Result { error, text } => {
                self.close_text().await?;
                if error {
                    if !text.trim().is_empty() {
                        self.insert("error", &clip(&text, 4000), &json!({})).await?;
                        self.errors += 1;
                    }
                    self.result_error = Some(text);
                } else if self.texts == 0 && !text.trim().is_empty() {
                    self.insert("assistant", &text, &json!({})).await?;
                    self.texts += 1;
                }
            }
        }
        Ok(())
    }

    async fn finish(&mut self) -> AppResult<()> {
        self.close_text().await?;
        self.close_raw().await?;
        // Werkzeuge ohne Ergebnis (Lauf abgebrochen) als beendet markieren
        let ids: Vec<String> = self.tools.iter().filter(|(_, (_, c))| c.status == "running").map(|(k, _)| k.clone()).collect();
        for id in ids {
            if let Some((m, call)) = self.tools.get_mut(&id) {
                call.status = "error".into();
                call.finished_at = Some(now());
                let data = messages::jv(&*call);
                messages::update(&self.state, self.user_id, m, "", &data).await?;
            }
        }
        Ok(())
    }
}

/// Grosse Eingaben (z. B. ganze Dateien bei „Write“) kürzen.
fn clip_json(v: Value) -> Value {
    match v {
        Value::String(s) if s.chars().count() > 4000 => Value::String(clip(&s, 4000)),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, clip_json(v))).collect()),
        Value::Array(a) => Value::Array(a.into_iter().take(200).map(clip_json).collect()),
        other => other,
    }
}

// ------------------------------------------------------------------ Aufruf je CLI

pub struct Mcp {
    pub url: String,
    pub token: String,
    /// MCP-Server von nrzGit (wenn verbunden)
    pub forges: Vec<Forge>,
    /// Die Plattformen nicht direkt anbinden, sondern über `nrzgit_find`/`nrzgit_call` bzw.
    /// `nrzgit_find`/`nrzgit_call` von nrzx (spart die ~150 Werkzeug-Schemas je Plattform im
    /// Kontext bei CLIs ohne eigene Werkzeugsuche)
    pub gateway: bool,
    /// Eigene MCP-Server des Bots (nur in seinen Gesprächen)
    pub bot: Vec<nrzx_shared::agents::BotMcp>,
}

/// Eigener MCP-Server eines Bots als Eintrag für Claude Code (`mcpServers`).
fn bot_server_claude(m: &nrzx_shared::agents::BotMcp) -> serde_json::Value {
    if !m.url.is_empty() {
        let mut v = json!({"type": "http", "url": m.url});
        if !m.token.is_empty() {
            v["headers"] = json!({"Authorization": format!("Bearer {}", m.token)});
        }
        v
    } else {
        let mut parts = m.command.split_whitespace().map(String::from);
        let command = parts.next().unwrap_or_default();
        json!({"type": "stdio", "command": command, "args": parts.collect::<Vec<_>>()})
    }
}

/// MCP-Server einer Git-Plattform.
pub struct Forge {
    /// Name des Servers beim Agenten: `nrzgit`
    pub name: &'static str,
    /// `<plattform>/mcp`
    pub url: String,
    /// Zugriffstoken des Benutzers
    pub token: String,
}

impl Forge {
    /// Umgebungsvariable mit dem Token: `NRZGIT_TOKEN`
    fn token_var(&self) -> String {
        format!("{}_TOKEN", self.name.to_uppercase())
    }

    /// Umgebungsvariable mit der Adresse des MCP-Servers: `NRZGIT_MCP_URL`
    fn url_var(&self) -> String {
        format!("{}_MCP_URL", self.name.to_uppercase())
    }
}

impl Mcp {
    /// nrzGit als eigener MCP-Server (nur ohne Gateway).
    fn direct_git(&self) -> &[Forge] {
        if self.gateway {
            &[]
        } else {
            &self.forges
        }
    }
}

struct Ctx<'a> {
    kind: AgentKind,
    t: &'a ThreadRow,
    acc: &'a AccountRow,
    workdir: &'a Path,
    rundir: &'a Path,
    mcp: &'a Mcp,
    hint: String,
    history: String,
    prompt: &'a str,
    /// (Pfad, Bild?, Content-Type)
    files: &'a [(PathBuf, bool, String)],
    /// Mit `@@name` einbezogene Repositories (nur lesen, als Referenz)
    refs: &'a [crate::workspace::RepoRef],
    resume: Option<String>,
}

pub struct Invocation {
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    pub env: Vec<(String, String)>,
}

/// CLIs, die viele MCP-Werkzeuge erst bei Bedarf laden.
pub fn uses_tool_search(kind: AgentKind) -> bool {
    kind == AgentKind::Claude
}

/// Hinweise zu nrzGit (auch für Gespräche mit Bots).
pub fn forge_hint(mcp: &Mcp) -> String {
    let mut h = String::new();
    for f in &mcp.forges {
        let (title, what) = ("nrzGit", "repos, issues, PRs, releases, registry, ports");
        if mcp.gateway {
            h.push_str(&format!("The user's git platform {title} ({what}): nrzx {0}_find, then {0}_call.\n", f.name));
        } else if mcp.forges.len() > 1 {
            h.push_str(&format!("{} = the user's git platform {title} ({what}).\n", f.name));
        } else {
            h.push_str(&format!("{} = the user's git platform ({what}).\n", f.name));
        }
    }
    if mcp.forges.len() > 1 {
        h.push_str("Use the platform the project's git remote points to.\n");
    }
    h
}

/// Fertige Änderungen ausliefern, ohne dass der Benutzer es jedes Mal sagen muss (Version, Commit,
/// Push, Release, nrzStore) – ausser er sagt ausdrücklich, dass nicht. Nur Haupt-Threads (keine
/// Subagenten, nicht im Nur-Lesen-Modus).
const SHIP_HINT: &str = "Ship finished code changes without being asked, unless the user explicitly says not to: bump the version \
    (if the project has one), commit and push, then publish a release the way the project does (if it makes releases); on a branch \
    other than the default one, push it and open a pull request instead. If the project is an app in the user's nrzStore (repo \
    nrzStore on nrzGit, rules in its AGENT.md), update the store as well.\n";

/// Systemhinweis – bewusst knapp, er steht in jedem Lauf im Kontext.
fn system_hint(t: &ThreadRow, workdir: &Path, mcp: &Mcp) -> String {
    let mut h = format!("You run inside nrzX (web UI for coding agents). cwd: {}", workdir.display());
    if !t.branch.is_empty() {
        h.push_str(&format!(", branch {}", t.branch));
    }
    h.push_str(".\n");
    if t.read_only {
        h.push_str(
            "READ-ONLY: answer questions about the code, cite path:line. Do not modify files, commit, install or run state-changing commands; \
             if asked for a change, describe it instead.\n",
        );
    } else {
        h.push_str(
            "Be token-efficient: read only what you need (targeted search over whole files), no redundant re-reads, \
             no long recaps. End with a short summary.\n\
             Delegate only large, independent parts via nrzx spawn_subagents (give simple tasks effort \"low\"); small work do yourself.\n",
        );
        if t.parent_id.is_some() {
            h.push_str("You are a subagent for one task: do only that, commit on the current branch, end with what changed and what is open.\n");
        } else if t.freestyle {
            h.push_str("FREESTYLE: never ask the user anything – make every decision yourself, sensibly, and mention notable choices in the summary.\n");
        } else {
            h.push_str(
                "If a decision truly needs the user (ambiguous goal, risky or irreversible step), call nrzx ask_user with short options; \
                 otherwise decide yourself.\n",
            );
        }
        if t.parent_id.is_none() {
            h.push_str(SHIP_HINT);
        }
        h.push_str(&forge_hint(mcp));
    }
    h.push_str("Reply in the user's language.");
    h
}

/// Bisheriger Verlauf (Benutzer/Agent) für einen neuen Kontext – nach dem letzten Zurücksetzen.
async fn history(state: &AppState, thread_id: i64, before: i64) -> AppResult<String> {
    let rows: Vec<(String, String, String)> = sqlx::query_as("SELECT role, content, data FROM messages WHERE thread_id = ? AND id < ? ORDER BY id")
        .bind(thread_id)
        .bind(before)
        .fetch_all(&state.db)
        .await?;
    let start = rows.iter().rposition(|(role, _, data)| role == "system" && data.contains("\"context_reset\"")).map(|i| i + 1).unwrap_or(0);
    let mut parts: Vec<String> = rows[start..]
        .iter()
        .filter(|(role, content, _)| matches!(role.as_str(), "user" | "assistant") && !content.trim().is_empty())
        .map(|(role, content, _)| format!("{}: {}", if role == "user" { "User" } else { "Assistant" }, clip(content.trim(), 2000)))
        .collect();
    if parts.is_empty() {
        return Ok(String::new());
    }
    let mut total = 0;
    let mut keep = Vec::new();
    while let Some(p) = parts.pop() {
        total += p.len();
        if total > 16_000 || keep.len() >= 16 {
            break;
        }
        keep.push(p);
    }
    keep.reverse();
    Ok(format!("Earlier conversation in this thread (for context):\n\n{}\n\n---\n", keep.join("\n\n")))
}

/// Abschnitt im Prompt zu den mit `@@name` einbezogenen Repositories.
fn refs_note(refs: &[crate::workspace::RepoRef]) -> String {
    if refs.is_empty() {
        return String::new();
    }
    let mut p = String::from(
        "\n\nReferenced repositories (@@): other projects of the user, for reference – read their code (structure, APIs, types, config) \
         e.g. to integrate them with each other. Do not modify them unless the user explicitly asks.\n",
    );
    for r in refs {
        p.push_str(&format!("- @@{}: {}", r.name, r.path.display()));
        if !r.repo.is_empty() && !r.repo.eq_ignore_ascii_case(&r.name) {
            p.push_str(&format!(" ({})", r.repo));
        }
        if !r.branch.is_empty() {
            p.push_str(&format!(", branch {}", r.branch));
        }
        p.push('\n');
    }
    p
}

/// Prompt mit Hinweis (bei CLIs ohne Systemprompt-Option), Verlauf und Anhängen.
fn full_prompt(ctx: &Ctx, with_hint: bool, mention: &[&PathBuf]) -> String {
    let mut p = String::new();
    if with_hint {
        p.push_str(&ctx.hint);
        p.push_str("\n\n");
    }
    p.push_str(&ctx.history);
    p.push_str(ctx.prompt);
    if !mention.is_empty() {
        p.push_str("\n\nAttached files:\n");
        for f in mention {
            p.push_str(&format!("- {}\n", f.display()));
        }
    }
    p.push_str(&refs_note(ctx.refs));
    if p.len() > MAX_ARG_PROMPT {
        let cut = (0..=MAX_ARG_PROMPT).rev().find(|i| p.is_char_boundary(*i)).unwrap_or(0);
        p.truncate(cut);
    }
    p
}

fn mcp_env(mcp: &Mcp) -> Vec<(String, String)> {
    let mut env = vec![("NRZX_MCP_URL".to_string(), mcp.url.clone()), ("NRZX_MCP_TOKEN".to_string(), mcp.token.clone())];
    for f in &mcp.forges {
        env.push((f.url_var(), f.url.clone()));
        env.push((f.token_var(), f.token.clone()));
    }
    env
}

/// Programmargumente, Eingabe und Umgebung für das CLI.
async fn invocation(state: &AppState, ctx: &Ctx<'_>) -> AppResult<Invocation> {
    let t = ctx.t;
    let s = spec(ctx.kind);
    let effort = s.effort(&t.effort);
    let ro = t.read_only;
    // Nur lesen: keine MCP-Server (nrzx startet Subagenten, nrzgit kann schreiben)
    let mut env = if ro { Vec::new() } else { mcp_env(ctx.mcp) };
    env.push(("NRZX_THREAD_ID".into(), t.id.to_string()));
    env.push(("TERM".into(), "dumb".into()));
    env.push(("NO_COLOR".into(), "1".into()));
    let images: Vec<&(PathBuf, bool, String)> = ctx.files.iter().filter(|f| f.1).collect();
    let all_paths: Vec<&PathBuf> = ctx.files.iter().map(|f| &f.0).collect();
    let mut args: Vec<String> = Vec::new();
    let mut stdin = None;
    match ctx.kind {
        AgentKind::Claude => {
            let mut servers = serde_json::Map::new();
            servers.insert(
                "nrzx".into(),
                json!({"type": "http", "url": ctx.mcp.url, "headers": {"Authorization": format!("Bearer {}", ctx.mcp.token)}}),
            );
            for f in ctx.mcp.direct_git() {
                servers.insert(f.name.into(), json!({"type": "http", "url": f.url, "headers": {"Authorization": format!("Bearer {}", f.token)}}));
            }
            // Eigene MCP-Server des Bots
            for m in &ctx.mcp.bot {
                servers.insert(m.name.clone(), bot_server_claude(m));
            }
            let cfg = ctx.rundir.join("mcp.json");
            tokio::fs::write(&cfg, serde_json::to_vec_pretty(&json!({"mcpServers": if ro { serde_json::Map::new() } else { servers }})).unwrap_or_default()).await?;
            args.extend(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"].map(String::from));
            if ro {
                // Ohne Freigabe für alles: was nicht erlaubt ist, lehnt Claude Code im Druckmodus ab
                args.extend(["--permission-mode".into(), "default".into(), "--allowedTools".into()]);
                args.extend(CLAUDE_READ_ONLY_TOOLS.iter().map(|s| s.to_string()));
                args.extend(["--disallowedTools".into()]);
                args.extend(["Edit", "Write", "MultiEdit", "NotebookEdit"].map(String::from));
                args.push("--strict-mcp-config".into());
            } else {
                args.extend(["--permission-mode".into(), "bypassPermissions".into()]);
                // Rückfragen laufen über nrzx ask_user (das eingebaute Werkzeug geht im Druckmodus nicht)
                args.extend(["--disallowedTools".into(), "AskUserQuestion".into()]);
            }
            if !t.model.is_empty() {
                args.extend(["--model".into(), t.model.clone()]);
            }
            if let Some(e) = effort {
                args.extend(["--effort".into(), e.into()]);
            }
            if let Some(sid) = &ctx.resume {
                args.extend(["--resume".into(), sid.clone()]);
            }
            args.extend(["--mcp-config".into(), cfg.to_string_lossy().into_owned()]);
            args.extend(["--append-system-prompt".into(), ctx.hint.clone()]);
            args.extend(["--add-dir".into(), ctx.rundir.to_string_lossy().into_owned()]);
            for r in ctx.refs {
                args.extend(["--add-dir".into(), r.path.to_string_lossy().into_owned()]);
            }
            let others: Vec<&PathBuf> = ctx.files.iter().filter(|f| !f.1).map(|f| &f.0).collect();
            let text = full_prompt(ctx, false, &others);
            if images.is_empty() {
                stdin = Some(text.into_bytes());
            } else {
                // Bilder als Bild-Eingabe (stream-json über stdin)
                args.extend(["--input-format".into(), "stream-json".into()]);
                let mut content = vec![json!({"type": "text", "text": text})];
                for (path, _, ct) in &images {
                    let bytes = tokio::fs::read(path).await?;
                    if bytes.len() > 5 * 1024 * 1024 {
                        continue;
                    }
                    use base64::Engine;
                    content.push(json!({"type": "image", "source": {"type": "base64", "media_type": ct, "data": base64::engine::general_purpose::STANDARD.encode(bytes)}}));
                }
                let msg = json!({"type": "user", "message": {"role": "user", "content": content}});
                stdin = Some(format!("{msg}\n").into_bytes());
            }
            env.push(("IS_SANDBOX".into(), "1".into()));
            // Connectoren von claude.ai (Drive, Docs …) kosten in jedem Lauf Kontext – hier nicht nötig
            env.push(("ENABLE_CLAUDEAI_MCP_SERVERS".into(), "false".into()));
            env.push(("MCP_TOOL_TIMEOUT".into(), "3600000".into()));
            env.push(("MCP_TIMEOUT".into(), "30000".into()));
        }
        AgentKind::Codex => {
            args.push("exec".into());
            if let Some(sid) = &ctx.resume {
                args.extend(["resume".into(), sid.clone()]);
            }
            args.extend(["--json", "--skip-git-repo-check"].map(String::from));
            if !ro {
                args.push("--dangerously-bypass-approvals-and-sandbox".into());
            }
            if !t.model.is_empty() {
                args.extend(["-m".into(), t.model.clone()]);
            }
            let mut cfg = Vec::new();
            if let Some(e) = effort {
                // Codex kennt „xhigh“ bei allen aktuellen Modellen, „max“ nur bei manchen
                let e = if e == "max" { "xhigh" } else { e };
                cfg.push(format!("model_reasoning_effort=\"{e}\""));
            }
            if ro {
                // Sandbox nur lesend, ohne Rückfragen (Schreibversuche schlagen fehl)
                cfg.push("sandbox_mode=\"read-only\"".into());
                cfg.push("approval_policy=\"never\"".into());
            }
            for (name, url, var) in [("nrzx", ctx.mcp.url.clone(), "NRZX_MCP_TOKEN".to_string())]
                .into_iter()
                .chain(ctx.mcp.direct_git().iter().map(|f| (f.name, f.url.clone(), f.token_var())))
                .filter(|_| !ro)
            {
                cfg.push(format!("mcp_servers.{name}.url={}", toml_str(&url)));
                cfg.push(format!("mcp_servers.{name}.bearer_token_env_var={}", toml_str(&var)));
                cfg.push(format!("mcp_servers.{name}.tool_timeout_sec=3600"));
                cfg.push(format!("mcp_servers.{name}.startup_timeout_sec=30"));
            }
            // Eigene MCP-Server des Bots
            for m in ctx.mcp.bot.iter().filter(|_| !ro) {
                let name = &m.name;
                if !m.url.is_empty() {
                    cfg.push(format!("mcp_servers.{name}.url={}", toml_str(&m.url)));
                    if !m.token.is_empty() {
                        cfg.push(format!("mcp_servers.{name}.http_headers.Authorization={}", toml_str(&format!("Bearer {}", m.token))));
                    }
                } else {
                    let mut parts = m.command.split_whitespace();
                    cfg.push(format!("mcp_servers.{name}.command={}", toml_str(parts.next().unwrap_or_default())));
                    let args: Vec<String> = parts.map(toml_str).collect();
                    cfg.push(format!("mcp_servers.{name}.args=[{}]", args.join(",")));
                }
                cfg.push(format!("mcp_servers.{name}.startup_timeout_sec=30"));
            }
            cfg.push(format!("developer_instructions={}", toml_str(&ctx.hint)));
            for c in cfg {
                args.extend(["-c".into(), c]);
            }
            for (p, _, _) in &images {
                args.extend(["-i".into(), p.to_string_lossy().into_owned()]);
            }
            let others: Vec<&PathBuf> = ctx.files.iter().filter(|f| !f.1).map(|f| &f.0).collect();
            stdin = Some(full_prompt(ctx, false, &others).into_bytes());
            args.push("-".into());
        }
        AgentKind::Gemini => {
            gemini_settings(ctx.acc, state, ctx.mcp.direct_git()).await?;
            let new_session = ctx.resume.is_none();
            let prompt = full_prompt(ctx, new_session, &all_paths);
            args.extend(["--output-format", "stream-json", "--skip-trust"].map(String::from));
            if ro {
                // Ohne Freigaben stehen im nicht-interaktiven Modus nur lesende Werkzeuge bereit;
                // die MCP-Server aus den Einstellungen des Kontos bleiben aus
                args.extend(["--approval-mode", "default", "--allowed-mcp-server-names", "none"].map(String::from));
            } else {
                args.push("--yolo".into());
            }
            if !t.model.is_empty() {
                args.extend(["-m".into(), t.model.clone()]);
            }
            if let Some(sid) = &ctx.resume {
                args.extend(["--resume".into(), sid.clone()]);
            }
            if !ctx.files.is_empty() {
                args.extend(["--include-directories".into(), ctx.rundir.to_string_lossy().into_owned()]);
            }
            for r in ctx.refs {
                args.extend(["--include-directories".into(), r.path.to_string_lossy().into_owned()]);
            }
            args.extend(["-p".into(), prompt]);
        }
        AgentKind::Opencode | AgentKind::Kilo => {
            let mut servers = serde_json::Map::new();
            servers.insert(
                "nrzx".into(),
                json!({"type": "remote", "url": ctx.mcp.url, "enabled": true, "oauth": false, "timeout": 3_600_000,
                       "headers": {"Authorization": format!("Bearer {}", ctx.mcp.token)}}),
            );
            for f in ctx.mcp.direct_git() {
                servers.insert(
                    f.name.into(),
                    json!({"type": "remote", "url": f.url, "enabled": true, "oauth": false, "timeout": 600_000,
                           "headers": {"Authorization": format!("Bearer {}", f.token)}}),
                );
            }
            // Eigene MCP-Server des Bots
            for m in &ctx.mcp.bot {
                let v = if !m.url.is_empty() {
                    let mut v = json!({"type": "remote", "url": m.url, "enabled": true, "timeout": 600_000});
                    if !m.token.is_empty() {
                        v["headers"] = json!({"Authorization": format!("Bearer {}", m.token)});
                    }
                    v
                } else {
                    json!({"type": "local", "command": m.command.split_whitespace().collect::<Vec<_>>(), "enabled": true})
                };
                servers.insert(m.name.clone(), v);
            }
            let cfg = if ro {
                json!({"autoupdate": false,
                       "permission": {"edit": "deny", "bash": "deny", "webfetch": "allow"},
                       "tools": {"write": false, "edit": false, "patch": false, "bash": false}})
            } else {
                json!({"mcp": servers, "autoupdate": false})
            };
            let var = if ctx.kind == AgentKind::Kilo { "KILO_CONFIG_CONTENT" } else { "OPENCODE_CONFIG_CONTENT" };
            env.push((var.into(), cfg.to_string()));
            args.extend(["run", "--format", "json", "--auto"].map(String::from));
            if !t.model.is_empty() {
                args.extend(["-m".into(), t.model.clone()]);
            }
            if let Some(e) = effort {
                args.extend(["--variant".into(), e.into()]);
            }
            if let Some(sid) = &ctx.resume {
                args.extend(["-s".into(), sid.clone()]);
            }
            for p in &all_paths {
                args.extend(["-f".into(), p.to_string_lossy().into_owned()]);
            }
            let new_session = ctx.resume.is_none();
            args.push("--".into());
            args.push(full_prompt(ctx, new_session, &[]));
        }
        AgentKind::Grok => {
            grok_settings(ctx.workdir, ctx.mcp).await;
            args.extend(["-d".into(), ctx.workdir.to_string_lossy().into_owned()]);
            if !t.model.is_empty() {
                args.extend(["-m".into(), t.model.clone()]);
            }
            args.extend(["-p".into(), full_prompt(ctx, true, &all_paths)]);
        }
    }
    Ok(Invocation { args, stdin, env })
}

/// TOML-Zeichenkette (JSON-Escapes sind gültiges TOML).
fn toml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Gemini liest MCP-Server nur aus den Einstellungen: im Home des Kontos die Server mit
/// Umgebungsvariablen eintragen (Token und Adresse je Lauf über die Umgebung).
async fn gemini_settings(acc: &AccountRow, state: &AppState, forges: &[Forge]) -> AppResult<()> {
    let path = accounts::home(state, acc.id).join(".gemini").join("settings.json");
    let mut v: Value = match tokio::fs::read_to_string(&path).await {
        Ok(t) => match serde_json::from_str(&t) {
            Ok(v) => v,
            // Datei mit Kommentaren o. Ä. nicht überschreiben
            Err(_) => return Ok(()),
        },
        Err(_) => json!({}),
    };
    if !v.is_object() {
        v = json!({});
    }
    let servers = v.as_object_mut().and_then(|o| {
        o.entry("mcpServers").or_insert_with(|| json!({}));
        o.get_mut("mcpServers").and_then(|m| m.as_object_mut())
    });
    if let Some(servers) = servers {
        servers.insert(
            "nrzx".into(),
            json!({"httpUrl": "$NRZX_MCP_URL", "headers": {"Authorization": "Bearer $NRZX_MCP_TOKEN"}, "timeout": 3_600_000, "trust": true}),
        );
        for name in ["nrzgit"] {
            match forges.iter().find(|f| f.name == name) {
                Some(f) => {
                    servers.insert(
                        name.into(),
                        json!({"httpUrl": format!("${}", f.url_var()), "headers": {"Authorization": format!("Bearer ${}", f.token_var())},
                               "timeout": 600_000, "trust": true}),
                    );
                }
                None => {
                    servers.remove(name);
                }
            }
        }
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, serde_json::to_vec_pretty(&v).unwrap_or_default()).await?;
    Ok(())
}

/// Grok CLI liest MCP-Server nur aus `.grok/settings.json` im Arbeitsordner. Nur anlegen,
/// wenn die Datei fehlt oder von nrzX stammt, und in `.git/info/exclude` eintragen.
async fn grok_settings(workdir: &Path, mcp: &Mcp) {
    let dir = workdir.join(".grok");
    let path = dir.join("settings.json");
    if let Ok(t) = tokio::fs::read_to_string(&path).await {
        if !t.contains("\"nrzx\"") {
            return;
        }
    }
    let mut servers = serde_json::Map::new();
    servers.insert(
        "nrzx".into(),
        json!({"name": "nrzx", "transport": {"type": "http", "url": mcp.url, "headers": {"Authorization": format!("Bearer {}", mcp.token)}}}),
    );
    for f in mcp.direct_git() {
        servers.insert(
            f.name.into(),
            json!({"name": f.name, "transport": {"type": "http", "url": f.url, "headers": {"Authorization": format!("Bearer {}", f.token)}}}),
        );
    }
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        return;
    }
    let _ = tokio::fs::write(&path, serde_json::to_vec_pretty(&json!({"mcpServers": servers})).unwrap_or_default()).await;
    // Nicht als Änderung im Repository zeigen
    let git = workdir.join(".git");
    let info = if git.is_dir() {
        Some(git.join("info"))
    } else if let Ok(t) = tokio::fs::read_to_string(&git).await {
        // Worktree: „gitdir: <repo>/.git/worktrees/<name>“ → gemeinsames info/
        t.trim().strip_prefix("gitdir:").map(|p| {
            let p = PathBuf::from(p.trim());
            p.parent().and_then(|x| x.parent()).map(|x| x.join("info")).unwrap_or(p.join("info"))
        })
    } else {
        None
    };
    if let Some(info) = info {
        let exclude = info.join("exclude");
        let current = tokio::fs::read_to_string(&exclude).await.unwrap_or_default();
        if !current.lines().any(|l| l.trim() == ".grok/") {
            let _ = tokio::fs::create_dir_all(&info).await;
            let _ = tokio::fs::write(&exclude, format!("{current}{}.grok/\n", if current.is_empty() || current.ends_with('\n') { "" } else { "\n" })).await;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn toml_strings() {
        assert_eq!(super::toml_str("a \"b\"\nc"), "\"a \\\"b\\\"\\nc\"");
    }
}

/// Wartet, bis der Lauf gestoppt werden soll (ohne den Wächter über ein `await` zu halten).
async fn stop_signal(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|s| *s).await;
}

/// Wartet auf einen Zwischenruf ab der Stufe `level` ([`SOFT`] oder [`NOW`]).
async fn interrupt_signal(rx: &mut watch::Receiver<u8>, level: u8) {
    if rx.wait_for(|s| *s >= level).await.is_err() {
        // Sender weg: nie auslösen
        std::future::pending::<()>().await;
    }
}
