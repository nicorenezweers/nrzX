//! Subagenten: Kind-Threads (`parent_id`), je mit eigenem Worktree, parallel gestartet – vom
//! Agenten über den MCP-Server (`spawn_subagents`) oder vom Benutzer (`/subagents`).
//! `wait` wartet, bis alle fertig sind (Elternthread solange `waiting`, sein Platz in der
//! Warteschlange wird so lange abgegeben), und berichtet je Subagent Status, letzte Antwort,
//! Branch und Änderungen.

use super::messages::{self, clip};
use super::threads::{self, ThreadRow};
use super::{runs, runtime};
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::{SubagentTask, SubagentsData};
use nrzx_shared::{ThreadStatus, ThreadSummary};
use serde::Serialize;
use std::time::{Duration, Instant};

/// Höchstens so viele Aufgaben je Aufruf und so tief verschachtelt.
pub const MAX_TASKS: usize = 10;
pub const MAX_DEPTH: usize = 3;

/// Bericht über einen Subagenten (für `wait_for_subagents` und `subagent_status`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub id: i64,
    pub title: String,
    pub agent: String,
    pub status: String,
    pub running: bool,
    pub branch: String,
    pub last_answer: String,
    pub error: String,
    pub changes: Changes,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    /// `git status --porcelain` (gekürzt)
    pub files: Vec<String>,
    /// `git diff --shortstat HEAD`
    pub stat: String,
    /// Commits auf dem Branch seit dem Hauptbranch
    pub commits: Vec<String>,
}

async fn depth(state: &AppState, t: &ThreadRow) -> AppResult<usize> {
    let mut d = 0;
    let mut cur = t.parent_id;
    while let Some(p) = cur {
        d += 1;
        if d > MAX_DEPTH + 1 {
            break;
        }
        cur = threads::row(state, p).await?.parent_id;
    }
    Ok(d)
}

/// Subagenten anlegen und starten. Legt im Elternthread eine Nachricht `subagents` an.
pub async fn spawn(state: &AppState, parent: &ThreadRow, tasks: Vec<SubagentTask>, by_user: bool) -> AppResult<Vec<ThreadSummary>> {
    if tasks.is_empty() {
        return Err(AppError::bad("Keine Aufgaben angegeben."));
    }
    if tasks.len() > MAX_TASKS {
        return Err(AppError::bad(format!("Höchstens {MAX_TASKS} Subagenten auf einmal.")));
    }
    if depth(state, parent).await? >= MAX_DEPTH {
        return Err(AppError::bad("Subagenten sind zu tief verschachtelt."));
    }
    // Erst alles prüfen, dann anlegen
    let mut plans = Vec::new();
    for task in &tasks {
        let prompt = task.prompt.trim();
        if prompt.is_empty() {
            return Err(AppError::bad("Jede Aufgabe braucht einen Prompt."));
        }
        let kind = task.agent.unwrap_or(parent.kind());
        let same = kind == parent.kind();
        let account = task.account_id.or(if same { parent.account_id } else { None });
        let model = task.model.clone().unwrap_or_else(|| if same { parent.model.clone() } else { String::new() });
        let effort = task.effort.clone().unwrap_or_else(|| if same { parent.effort.clone() } else { String::new() });
        let (account_id, model, effort) = threads::resolve_agent(state, parent.user_id, kind, account, &model, &effort).await?;
        let title = if task.title.trim().is_empty() { threads::title_from(prompt) } else { task.title.trim().chars().take(120).collect() };
        plans.push((title, prompt.to_string(), kind, account_id, model, effort));
    }
    let mut ids = Vec::new();
    let mut titles = Vec::new();
    for (title, prompt, kind, account_id, model, effort) in plans {
        let id = threads::insert(state, parent.user_id, parent.project_id, Some(parent.id), &title, kind, account_id, &model, &effort, true).await?;
        ids.push((id, prompt));
        titles.push(title);
    }
    let data = messages::jv(&SubagentsData { children: ids.iter().map(|(i, _)| *i).collect(), titles, by_user });
    let run_id = runtime::get(state).active_run(parent.id).filter(|r| *r > 0);
    let text = if by_user { "Subagenten gestartet" } else { "" };
    messages::insert(state, parent.user_id, parent.id, run_id, "subagents", text, &data).await?;
    for (id, prompt) in &ids {
        if let Err(e) = runs::start(state, *id, prompt, &[]).await {
            messages::insert(state, parent.user_id, *id, None, "error", &e.to_string(), &serde_json::json!({})).await?;
            threads::set_status(state, *id, ThreadStatus::Error, Some(true)).await?;
        }
    }
    threads::publish(state, parent.id).await?;
    let mut out = Vec::new();
    for (id, _) in ids {
        out.push(threads::summary(state, id).await?);
    }
    Ok(out)
}

/// IDs der Subagenten (gegeben – nur eigene Kinder – oder alle).
async fn child_ids(state: &AppState, parent_id: i64, ids: Option<Vec<i64>>) -> AppResult<Vec<i64>> {
    let all: Vec<(i64,)> = sqlx::query_as("SELECT id FROM threads WHERE parent_id = ? ORDER BY id").bind(parent_id).fetch_all(&state.db).await?;
    let all: Vec<i64> = all.into_iter().map(|(i,)| i).collect();
    Ok(match ids {
        Some(ids) if !ids.is_empty() => {
            if let Some(bad) = ids.iter().find(|i| !all.contains(i)) {
                return Err(AppError::bad(format!("Thread {bad} ist kein Subagent dieses Threads.")));
            }
            ids
        }
        _ => all,
    })
}

/// Berichte über Subagenten.
pub async fn reports(state: &AppState, parent_id: i64, ids: Option<Vec<i64>>) -> AppResult<Vec<Report>> {
    let ids = child_ids(state, parent_id, ids).await?;
    let mut out = Vec::new();
    for id in ids {
        out.push(report(state, id).await?);
    }
    Ok(out)
}

async fn report(state: &AppState, id: i64) -> AppResult<Report> {
    let t = threads::row(state, id).await?;
    let rt = runtime::get(state);
    let last: Option<(String,)> = sqlx::query_as("SELECT content FROM messages WHERE thread_id = ? AND role = 'assistant' ORDER BY id DESC LIMIT 1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let error: Option<(String,)> = if t.status() == ThreadStatus::Error {
        sqlx::query_as("SELECT content FROM messages WHERE thread_id = ? AND role = 'error' ORDER BY id DESC LIMIT 1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
    } else {
        None
    };
    let running = rt.is_active(id) || t.status().is_active();
    Ok(Report {
        id,
        title: t.title.clone(),
        agent: t.kind().key().into(),
        status: t.status().key().into(),
        running,
        branch: t.branch.clone(),
        last_answer: last.map(|(c,)| clip(&c, 3000)).unwrap_or_default(),
        error: error.map(|(c,)| clip(&c, 1500)).unwrap_or_default(),
        changes: if running { Changes::default() } else { changes(state, &t).await },
    })
}

/// Änderungen im Arbeitsordner eines Threads (Git), ohne Fehler bei Ordnern ohne Git.
async fn changes(state: &AppState, t: &ThreadRow) -> Changes {
    let Ok(dir) = crate::workspace::thread_workdir(state, t.id).await else { return Changes::default() };
    if !dir.join(".git").exists() {
        return Changes::default();
    }
    let mut c = Changes::default();
    if let Ok(out) = crate::git::run_with(state, &dir, &["status", "--porcelain=v1"], None, &[]).await {
        c.files = out.stdout_str().lines().take(50).map(|l| l.to_string()).collect();
    }
    if let Ok(out) = crate::git::run_with(state, &dir, &["diff", "--shortstat", "HEAD"], None, &[]).await {
        c.stat = out.stdout_str().trim().to_string();
    }
    if let Some(p) = t.project_id {
        let base: Option<(String,)> = sqlx::query_as("SELECT default_branch FROM projects WHERE id = ?").bind(p).fetch_optional(&state.db).await.ok().flatten();
        if let Some((base,)) = base {
            if !base.is_empty() && base != t.branch && !base.starts_with('-') {
                let range = format!("{base}..HEAD");
                if let Ok(out) = crate::git::run_with(state, &dir, &["log", "--oneline", "-n", "20", &range], None, &[]).await {
                    if out.ok() {
                        c.commits = out.stdout_str().lines().map(|l| l.to_string()).collect();
                    }
                }
            }
        }
    }
    c
}

/// Warten, bis die Subagenten fertig sind oder die Zeit abläuft. Der Elternthread steht
/// solange auf `waiting` und gibt seinen Platz in der Warteschlange ab.
pub async fn wait(state: &AppState, parent_id: i64, ids: Option<Vec<i64>>, timeout: Duration) -> AppResult<(bool, Vec<Report>)> {
    let ids = child_ids(state, parent_id, ids).await?;
    let rt = runtime::get(state);
    let mut rx = rt.finished.subscribe();
    let parent_active = rt.is_active(parent_id);
    if parent_active {
        threads::set_status(state, parent_id, ThreadStatus::Waiting, None).await?;
        if let Some(slot) = rt.permit_slot(parent_id) {
            drop(slot.lock().await.take());
        }
    }
    let deadline = Instant::now() + timeout;
    let mut all_done;
    loop {
        all_done = true;
        for id in &ids {
            let st: Option<(String,)> = sqlx::query_as("SELECT status FROM threads WHERE id = ?").bind(id).fetch_optional(&state.db).await?;
            if rt.is_active(*id) || st.is_some_and(|(s,)| ThreadStatus::parse(&s).is_active()) {
                all_done = false;
                break;
            }
        }
        if all_done || Instant::now() >= deadline {
            break;
        }
        let left = deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(5));
        tokio::select! {
            _ = rx.recv() => {}
            _ = tokio::time::sleep(left) => {}
        }
        // Elternlauf gestoppt: nicht weiter warten
        if parent_active && !rt.is_active(parent_id) {
            break;
        }
    }
    if parent_active {
        // Platz in der Warteschlange zurückholen (solange der Elternlauf noch lebt)
        while rt.is_active(parent_id) {
            match tokio::time::timeout(Duration::from_secs(1), rt.permits.clone().acquire_owned()).await {
                Ok(Ok(p)) => {
                    if let Some(slot) = rt.permit_slot(parent_id) {
                        *slot.lock().await = Some(p);
                    }
                    break;
                }
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }
        if rt.is_active(parent_id) {
            threads::set_status(state, parent_id, ThreadStatus::Working, None).await?;
        }
    }
    let mut out = Vec::new();
    for id in ids {
        out.push(report(state, id).await?);
    }
    Ok((all_done, out))
}

/// Einen Subagenten (und dessen Subagenten) stoppen.
pub async fn stop(state: &AppState, parent_id: i64, child_id: i64) -> AppResult<Report> {
    child_ids(state, parent_id, Some(vec![child_id])).await?;
    runs::stop_tree(state, child_id).await?;
    report(state, child_id).await
}

/// Kinder eines Threads, die ein Agent gestartet hat (nicht der Benutzer per /subagents).
async fn agent_spawned(state: &AppState, parent_id: i64) -> AppResult<Vec<i64>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT data FROM messages WHERE thread_id = ? AND role = 'subagents'").bind(parent_id).fetch_all(&state.db).await?;
    Ok(rows
        .iter()
        .filter_map(|(d,)| serde_json::from_str::<SubagentsData>(d).ok())
        .filter(|d| !d.by_user)
        .flat_map(|d| d.children)
        .collect())
}

/// Subagenten, die der Elternthread nicht mehr braucht, schliessen (ablegen): Aufgerufen, wenn
/// der Lauf des Elternthreads fertig ist und nichts mehr in seiner Warteschlange steht. Abgelegt
/// werden die fertigen, von einem Agenten gestarteten Subagenten samt ihren Nachfahren – laufende
/// bleiben offen und werden abgelegt, wenn ihr eigener Lauf endet ([`close_if_orphaned`]).
/// Vom Benutzer gestartete bleiben offen. Verlauf, Branch und Worktree bleiben erhalten;
/// „Wieder öffnen“ holt einen Subagenten zurück.
pub async fn close_finished(state: &AppState, parent_id: i64) -> AppResult<usize> {
    let rt = runtime::get(state);
    let mut closed = 0;
    for child in agent_spawned(state, parent_id).await? {
        let ids: Vec<(i64, String)> = sqlx::query_as(
            "WITH RECURSIVE tree(id) AS (SELECT ? UNION ALL SELECT t.id FROM threads t JOIN tree ON t.parent_id = tree.id) \
             SELECT t.id, t.status FROM threads t WHERE t.id IN (SELECT id FROM tree)",
        )
        .bind(child)
        .fetch_all(&state.db)
        .await?;
        for (id, status) in ids {
            let st = ThreadStatus::parse(&status);
            if st == ThreadStatus::Settled || st.is_active() || rt.is_active(id) {
                continue;
            }
            sqlx::query("UPDATE threads SET status = 'settled', unread = 0, updated_at = ? WHERE id = ?")
                .bind(crate::db::now())
                .bind(id)
                .execute(&state.db)
                .await?;
            threads::publish(state, id).await?;
            closed += 1;
        }
    }
    Ok(closed)
}

/// Nach dem Lauf eines Subagenten: arbeitet der Elternthread nicht mehr (er hat nicht auf ihn
/// gewartet) und steht bei ihm nichts in der Warteschlange, wird der Subagent gleich abgelegt –
/// samt seiner fertigen Subagenten. Nur für Subagenten, die ein Agent gestartet hat.
pub async fn close_if_orphaned(state: &AppState, t: &ThreadRow) -> AppResult<()> {
    let Some(parent_id) = t.parent_id else { return Ok(()) };
    let rt = runtime::get(state);
    let parent = threads::row(state, parent_id).await?;
    if rt.is_active(parent_id) || parent.status().is_active() || !agent_spawned(state, parent_id).await?.contains(&t.id) {
        return Ok(());
    }
    if super::queue::load(state, parent_id).await?.is_empty() {
        sqlx::query("UPDATE threads SET status = 'settled', unread = 0, updated_at = ? WHERE id = ? AND status NOT IN ('working', 'waiting')")
            .bind(crate::db::now())
            .bind(t.id)
            .execute(&state.db)
            .await?;
        threads::publish(state, t.id).await?;
        close_finished(state, t.id).await?;
    }
    Ok(())
}
