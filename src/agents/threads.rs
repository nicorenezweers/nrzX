//! Threads: Liste (mit Subagenten-Zählern), anlegen (mit erstem Lauf), Detail, Nachrichten,
//! fortsetzen, stoppen, umbenennen, anheften, gelesen markieren, ablegen, löschen und
//! Agent/Konto/Modell/Stufe wechseln. Jeder Thread gehört genau einem Benutzer – fremde
//! Threads gibt es für andere nicht (404).

use super::kinds::spec;
use super::runtime::LocalPort;
use super::{accounts, messages, runs, subagents};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::{hub, AppState};
use nrzx_shared::agents::{NewSubagents, NewThread, PostMessage, RetractedMessage, RunInfo, SystemData, ThreadDetail, ThreadOrder, ThreadPatch, UserMessageData};
use nrzx_shared::{AgentKind, HubEvent, Message, ThreadStatus, ThreadSummary};
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::{delete, get, patch, post, routes, Route, State};

pub fn routes() -> Vec<Route> {
    routes![
        list,
        create,
        detail,
        update,
        reorder,
        remove,
        message_list,
        message_post,
        stop,
        mark_read,
        settle,
        reopen,
        clear,
        start_subagents,
        retract
    ]
}

/// Zeile aus `threads`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThreadRow {
    pub id: i64,
    pub user_id: i64,
    pub project_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub title: String,
    pub status: String,
    pub agent_kind: String,
    pub account_id: Option<i64>,
    pub model: String,
    pub effort: String,
    pub branch: String,
    pub use_worktree: bool,
    pub worktree_path: String,
    pub provider_session_id: String,
    pub unread: bool,
    pub pinned: bool,
    pub hidden: bool,
    pub sort_key: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_activity_at: i64,
    #[sqlx(default)]
    pub read_only: bool,
    #[sqlx(default)]
    pub resume_at: Option<i64>,
    #[sqlx(default)]
    pub freestyle: bool,
    #[sqlx(default)]
    pub resume_auto: bool,
    #[sqlx(default)]
    pub queued: String,
    #[sqlx(default)]
    pub additions: i64,
    #[sqlx(default)]
    pub deletions: i64,
    /// Gespräch eines Bots (`bots`)
    #[sqlx(default)]
    pub bot_id: Option<i64>,
    /// Deep Research (`agents::research`): schreibt ein Research Paper
    #[sqlx(default)]
    pub research: bool,
    /// Gespräch über diese Recherche
    #[sqlx(default)]
    pub discuss_of: Option<i64>,
}

impl ThreadRow {
    pub fn kind(&self) -> AgentKind {
        AgentKind::parse(&self.agent_kind).unwrap_or_default()
    }
    pub fn status(&self) -> ThreadStatus {
        ThreadStatus::parse(&self.status)
    }
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    #[sqlx(flatten)]
    t: ThreadRow,
    project_name: Option<String>,
    account_label: String,
    children_running: i64,
    run_started_at: Option<i64>,
    snippet: String,
    last_run: String,
    asking: bool,
}

const SUMMARY: &str = "SELECT t.*, p.name AS project_name, COALESCE(a.label, '') AS account_label, \
    (SELECT COUNT(*) FROM threads c WHERE c.parent_id = t.id AND c.status IN ('working', 'waiting')) AS children_running, \
    (SELECT r.started_at FROM runs r WHERE r.thread_id = t.id AND r.status IN ('queued', 'running') ORDER BY r.id DESC LIMIT 1) AS run_started_at, \
    COALESCE((SELECT substr(m.content, 1, 240) FROM messages m WHERE m.thread_id = t.id AND m.role = 'assistant' AND m.content <> '' ORDER BY m.id DESC LIMIT 1), '') AS snippet, \
    COALESCE((SELECT r.status FROM runs r WHERE r.thread_id = t.id ORDER BY r.id DESC LIMIT 1), '') AS last_run, \
    EXISTS (SELECT 1 FROM messages q WHERE q.thread_id = t.id AND q.role = 'question' AND json_extract(q.data, '$.answer') IS NULL \
        AND COALESCE(json_extract(q.data, '$.expired'), 0) = 0 \
        AND q.id > COALESCE((SELECT MAX(u.id) FROM messages u WHERE u.thread_id = t.id AND u.role = 'user'), 0)) AS asking \
    FROM threads t LEFT JOIN projects p ON p.id = t.project_id LEFT JOIN agent_accounts a ON a.id = t.account_id";

fn to_summary(r: SummaryRow) -> ThreadSummary {
    ThreadSummary {
        id: r.t.id,
        project_id: r.t.project_id,
        project_name: r.project_name,
        parent_id: r.t.parent_id,
        title: r.t.title.clone(),
        status: r.t.status(),
        agent: r.t.kind(),
        account_id: r.t.account_id,
        account_label: r.account_label,
        model: r.t.model.clone(),
        effort: r.t.effort.clone(),
        branch: r.t.branch.clone(),
        unread: r.t.unread,
        pinned: r.t.pinned,
        read_only: r.t.read_only,
        freestyle: r.t.freestyle,
        resume_auto: r.t.resume_auto,
        queued: serde_json::from_str(&r.t.queued).unwrap_or_default(),
        resume_at: r.t.resume_at,
        hidden: r.t.hidden,
        sort_key: r.t.sort_key,
        bot_id: r.t.bot_id,
        children_running: r.children_running,
        run_started_at: r.run_started_at,
        additions: r.t.additions,
        deletions: r.t.deletions,
        created_at: r.t.created_at,
        updated_at: r.t.updated_at,
        last_activity_at: r.t.last_activity_at,
        snippet: one_line(&r.snippet, 120),
        last_run: r.last_run,
        asking: r.asking,
        research: r.t.research,
        discuss_of: r.t.discuss_of,
    }
}

/// Erste nicht leere Zeile ohne Markdown-Zeichen, höchstens `max` Zeichen.
fn one_line(s: &str, max: usize) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let clean: String = line.trim_start_matches(['#', '>', '-', '*', ' ']).replace(['*', '`'], "");
    clean.chars().take(max).collect()
}

/// Thread des Benutzers laden (fremde und unbekannte: 404).
pub async fn get_row(state: &AppState, user_id: i64, id: i64) -> AppResult<ThreadRow> {
    sqlx::query_as("SELECT * FROM threads WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)
}

/// Thread ohne Benutzerprüfung (intern).
pub async fn row(state: &AppState, id: i64) -> AppResult<ThreadRow> {
    sqlx::query_as("SELECT * FROM threads WHERE id = ?").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound)
}

pub async fn summary(state: &AppState, id: i64) -> AppResult<ThreadSummary> {
    let r: SummaryRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SUMMARY} WHERE t.id = ?")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(to_summary(r))
}

/// Unterthreads (Subagenten) eines Threads.
pub async fn children(state: &AppState, parent_id: i64) -> AppResult<Vec<ThreadSummary>> {
    let rows: Vec<SummaryRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SUMMARY} WHERE t.parent_id = ? ORDER BY t.id")))
        .bind(parent_id)
        .fetch_all(&state.db)
        .await?;
    Ok(rows.into_iter().map(to_summary).collect())
}

/// Aktuellen Stand an den Benutzer senden (und an den Elternthread, dessen Zähler sich ändert).
pub async fn publish(state: &AppState, id: i64) -> AppResult<()> {
    let s = summary(state, id).await?;
    let user_id = row(state, id).await?.user_id;
    let parent = s.parent_id;
    hub::publish(state, user_id, HubEvent::ThreadUpdated { thread: s });
    if let Some(p) = parent {
        if let Ok(ps) = summary(state, p).await {
            hub::publish(state, user_id, HubEvent::ThreadUpdated { thread: ps });
        }
    }
    Ok(())
}

/// Status setzen (und senden).
pub async fn set_status(state: &AppState, id: i64, status: ThreadStatus, unread: Option<bool>) -> AppResult<()> {
    let ts = now();
    match unread {
        Some(u) => {
            sqlx::query("UPDATE threads SET status = ?, unread = ?, updated_at = ?, last_activity_at = ? WHERE id = ?")
                .bind(status.key())
                .bind(u)
                .bind(ts)
                .bind(ts)
                .bind(id)
                .execute(&state.db)
                .await?
        }
        None => {
            sqlx::query("UPDATE threads SET status = ?, updated_at = ? WHERE id = ?")
                .bind(status.key())
                .bind(ts)
                .bind(id)
                .execute(&state.db)
                .await?
        }
    };
    publish(state, id).await
}

/// Titel aus der ersten Nachricht (siehe [`nrzx_shared::agents::title_from`]).
pub fn title_from(text: &str) -> String {
    nrzx_shared::agents::title_from(text)
}

/// Agent, Konto, Modell und Stufe prüfen (Konto: gegeben oder Standardkonto der Art).
pub async fn resolve_agent(
    state: &AppState,
    user_id: i64,
    kind: AgentKind,
    account_id: Option<i64>,
    model: &str,
    effort: &str,
) -> AppResult<(Option<i64>, String, String)> {
    let acc = match account_id {
        Some(id) => {
            let a = accounts::get_row(state, user_id, id).await?;
            if a.kind() != kind {
                return Err(AppError::bad("Das Konto gehört zu einer anderen Agent-Art."));
            }
            Some(a)
        }
        None => accounts::default_for(state, user_id, kind).await?,
    };
    let Some(acc) = acc else {
        return Err(AppError::bad(format!(
            "Für {} ist noch kein Konto eingerichtet (Einstellungen → Agenten).",
            kind.name()
        )));
    };
    let model = if model.trim().is_empty() { acc.default_model.clone() } else { model.trim().to_string() };
    let effort = if effort.is_empty() { acc.default_effort.clone() } else { effort.to_string() };
    let effort = spec(kind).effort(&effort).unwrap_or("").to_string();
    if model.chars().count() > 200 || model.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(AppError::bad("Ungültiger Modellname."));
    }
    Ok((Some(acc.id), model, effort))
}

/// Thread anlegen (ohne Lauf). Prüft Projekt, Elternthread, Agent und Konto.
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    state: &AppState,
    user_id: i64,
    project_id: Option<i64>,
    parent_id: Option<i64>,
    title: &str,
    kind: AgentKind,
    account_id: Option<i64>,
    model: &str,
    effort: &str,
    use_worktree: bool,
) -> AppResult<i64> {
    if let Some(p) = project_id {
        let ok: Option<(i64,)> = sqlx::query_as("SELECT id FROM projects WHERE id = ? AND user_id = ?")
            .bind(p)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
        if ok.is_none() {
            return Err(AppError::NotFound);
        }
    }
    let ts = now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO threads (user_id, project_id, parent_id, title, status, agent_kind, account_id, model, effort, use_worktree, created_at, updated_at, last_activity_at) \
         VALUES (?, ?, ?, ?, 'idle', ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(project_id)
    .bind(parent_id)
    .bind(title)
    .bind(kind.key())
    .bind(account_id)
    .bind(model)
    .bind(effort)
    .bind(use_worktree)
    .bind(ts)
    .bind(ts)
    .bind(ts)
    .fetch_one(&state.db)
    .await?;
    Ok(id)
}

/// `GET /threads?project=&status=&parent=&q=&limit=` – Threads des Benutzers.
/// `parent=none` nur Hauptthreads, `parent=<id>` die Subagenten eines Threads.
#[get("/threads?<project>&<status>&<parent>&<q>&<limit>")]
async fn list(
    state: &State<AppState>,
    user: CurrentUser,
    project: Option<i64>,
    status: Option<&str>,
    parent: Option<&str>,
    q: Option<&str>,
    limit: Option<i64>,
) -> AppResult<Json<Vec<ThreadSummary>>> {
    let mut sql = format!("{SUMMARY} WHERE t.user_id = ?");
    if project.is_some() {
        sql.push_str(" AND t.project_id = ?");
    }
    let statuses: Vec<String> = status
        .map(|s| s.split(',').map(|x| ThreadStatus::parse(x.trim()).key().to_string()).collect())
        .unwrap_or_default();
    if !statuses.is_empty() {
        sql.push_str(&format!(" AND t.status IN ({})", vec!["?"; statuses.len()].join(", ")));
    }
    let parent_id = match parent {
        Some("none") | Some("") => {
            sql.push_str(" AND t.parent_id IS NULL");
            None
        }
        Some(p) => {
            let id: i64 = p.parse().map_err(|_| AppError::bad("parent: Zahl oder none"))?;
            sql.push_str(" AND t.parent_id = ?");
            Some(id)
        }
        None => None,
    };
    let needle = q.map(|q| format!("%{}%", q.trim().to_lowercase())).filter(|q| q.len() > 2);
    if needle.is_some() {
        sql.push_str(" AND lower(t.title) LIKE ?");
    }
    sql.push_str(" ORDER BY t.pinned DESC, COALESCE(t.sort_key, t.last_activity_at) DESC, t.id DESC LIMIT ?");
    let mut query = sqlx::query_as::<_, SummaryRow>(sqlx::AssertSqlSafe(sql)).bind(user.id());
    if let Some(p) = project {
        query = query.bind(p);
    }
    for s in &statuses {
        query = query.bind(s.clone());
    }
    if let Some(p) = parent_id {
        query = query.bind(p);
    }
    if let Some(n) = &needle {
        query = query.bind(n.clone());
    }
    let rows = query.bind(limit.unwrap_or(500).clamp(1, 2000)).fetch_all(&state.db).await?;
    Ok(Json(rows.into_iter().map(to_summary).collect()))
}

/// `POST /threads` – Thread anlegen und den ersten Lauf starten.
#[post("/threads", data = "<body>")]
async fn create(state: &State<AppState>, user: CurrentUser, _port: LocalPort, body: Json<NewThread>) -> AppResult<Json<ThreadSummary>> {
    let b = body.into_inner();
    let prompt = b.prompt.trim().to_string();
    if prompt.is_empty() && b.tasks.is_empty() {
        return Err(AppError::bad("Bitte eine Nachricht eingeben."));
    }
    let (account_id, model, effort) = resolve_agent(state, user.id(), b.agent, b.account_id, &b.model, &b.effort).await?;
    let title = match b.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => t.chars().take(120).collect(),
        None if !prompt.is_empty() => title_from(&prompt),
        None => b.tasks.iter().map(|t| t.title.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(", ").chars().take(80).collect(),
    };
    if b.read_only && !b.tasks.is_empty() {
        return Err(AppError::bad("Im Nur-Lesen-Modus lassen sich keine Subagenten starten."));
    }
    // Nur lesen: kein eigener Worktree – der Agent liest den Hauptcheckout des Projekts. Deep
    // Research braucht kein Projekt: es arbeitet in seinem eigenen Ordner
    let use_worktree = b.use_worktree && !b.read_only && !b.research;
    let project_id = if b.research { None } else { b.project_id };
    let id = insert(state, user.id(), project_id, None, &title, b.agent, account_id, &model, &effort, use_worktree).await?;
    if b.research {
        // Recherchiert ohne Rückfragen; vor dem ersten Lauf, damit er Hinweis und Ordner der Recherche bekommt
        sqlx::query("UPDATE threads SET research = 1, freestyle = 1 WHERE id = ?").bind(id).execute(&state.db).await?;
    }
    if b.read_only {
        sqlx::query("UPDATE threads SET read_only = 1 WHERE id = ?").bind(id).execute(&state.db).await?;
    }
    if b.freestyle && !b.read_only {
        sqlx::query("UPDATE threads SET freestyle = 1 WHERE id = ?").bind(id).execute(&state.db).await?;
    }
    // Vorgabe des Benutzers für das Weitermachen nach einem Rate-Limit
    sqlx::query("UPDATE threads SET resume_auto = (SELECT auto_resume FROM users WHERE id = ?) WHERE id = ?")
        .bind(user.id())
        .bind(id)
        .execute(&state.db)
        .await?;
    if !prompt.is_empty() {
        if let Err(e) = runs::start(state, id, &prompt, &b.attachments).await {
            // Thread ohne Lauf nicht stehen lassen
            sqlx::query("DELETE FROM threads WHERE id = ?").bind(id).execute(&state.db).await?;
            return Err(e);
        }
    } else {
        publish(state, id).await?;
    }
    if !b.tasks.is_empty() {
        let parent = row(state, id).await?;
        subagents::spawn(state, &parent, b.tasks, true).await?;
    }
    Ok(Json(summary(state, id).await?))
}

/// `GET /threads/<id>` – Thread mit Eltern, Subagenten und Läufen.
#[get("/threads/<id>")]
async fn detail(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ThreadDetail>> {
    let t = get_row(state, user.id(), id).await?;
    let parent = match t.parent_id {
        Some(p) => summary(state, p).await.ok(),
        None => None,
    };
    let runs: Vec<RunRow> = sqlx::query_as("SELECT * FROM runs WHERE thread_id = ? ORDER BY id DESC LIMIT 50")
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    let workdir = crate::workspace::thread_workdir(state, id).await.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Json(ThreadDetail {
        thread: summary(state, id).await?,
        parent,
        children: children(state, id).await?,
        runs: runs.into_iter().map(RunRow::to_json).collect(),
        workdir,
        session_id: t.provider_session_id,
    }))
}

/// Zeile aus `runs`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunRow {
    pub id: i64,
    pub thread_id: i64,
    pub agent_kind: String,
    pub account_id: Option<i64>,
    pub model: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub error: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub effort: String,
    pub session_id: String,
}

impl RunRow {
    pub fn to_json(self) -> RunInfo {
        RunInfo {
            id: self.id,
            thread_id: self.thread_id,
            agent: AgentKind::parse(&self.agent_kind).unwrap_or_default(),
            account_id: self.account_id,
            model: self.model,
            effort: self.effort,
            status: self.status,
            exit_code: self.exit_code,
            error: self.error,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cost_usd: self.cost_usd,
            started_at: self.started_at,
            finished_at: self.finished_at,
        }
    }
}

/// `PATCH /threads/<id>` – Titel, Anheften, Agent/Konto/Modell/Stufe.
#[patch("/threads/<id>", data = "<body>")]
async fn update(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<ThreadPatch>) -> AppResult<Json<ThreadSummary>> {
    let t = get_row(state, user.id(), id).await?;
    let b = body.into_inner();
    let ts = now();
    if let Some(title) = &b.title {
        let title: String = title.trim().chars().take(120).collect();
        if title.is_empty() {
            return Err(AppError::bad("Der Titel darf nicht leer sein."));
        }
        sqlx::query("UPDATE threads SET title = ?, updated_at = ? WHERE id = ?").bind(title).bind(ts).bind(id).execute(&state.db).await?;
    }
    if let Some(p) = b.pinned {
        sqlx::query("UPDATE threads SET pinned = ?, updated_at = ? WHERE id = ?").bind(p).bind(ts).bind(id).execute(&state.db).await?;
    }
    if let Some(h) = b.hidden {
        sqlx::query("UPDATE threads SET hidden = ?, updated_at = ? WHERE id = ?").bind(h).bind(ts).bind(id).execute(&state.db).await?;
    }
    if let Some(r) = b.read_only {
        sqlx::query("UPDATE threads SET read_only = ?, freestyle = CASE WHEN ? THEN 0 ELSE freestyle END, updated_at = ? WHERE id = ?")
            .bind(r)
            .bind(r)
            .bind(ts)
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    if let Some(f) = b.freestyle {
        let ro = b.read_only.unwrap_or(t.read_only);
        sqlx::query("UPDATE threads SET freestyle = ?, updated_at = ? WHERE id = ?").bind(f && !ro).bind(ts).bind(id).execute(&state.db).await?;
    }
    if let Some(a) = b.resume_auto {
        sqlx::query("UPDATE threads SET resume_auto = ? WHERE id = ?").bind(a).bind(id).execute(&state.db).await?;
        // Gilt auch als Vorgabe für künftige Threads
        sqlx::query("UPDATE users SET auto_resume = ? WHERE id = ?").bind(a).bind(user.id()).execute(&state.db).await?;
    }
    if b.agent.is_some() || b.account_id.is_some() || b.model.is_some() || b.effort.is_some() {
        let kind = b.agent.unwrap_or(t.kind());
        let kind_changed = kind != t.kind();
        let account = if kind_changed && b.account_id.is_none() { None } else { b.account_id.or(t.account_id) };
        let model = match &b.model {
            Some(m) => m.clone(),
            None if kind_changed => String::new(),
            None => t.model.clone(),
        };
        let effort = b.effort.clone().unwrap_or_else(|| t.effort.clone());
        let (account_id, model, effort) = resolve_agent(state, user.id(), kind, account, &model, &effort).await?;
        let switched = kind_changed || account_id != t.account_id;
        if switched && super::runtime::get(state).is_active(id) {
            return Err(AppError::conflict("Der Agent arbeitet noch – erst stoppen oder warten."));
        }
        sqlx::query("UPDATE threads SET agent_kind = ?, account_id = ?, model = ?, effort = ?, updated_at = ? WHERE id = ?")
            .bind(kind.key())
            .bind(account_id)
            .bind(&model)
            .bind(&effort)
            .bind(ts)
            .bind(id)
            .execute(&state.db)
            .await?;
        if switched {
            // Sitzungen gehören zu CLI und Konto – der nächste Lauf beginnt neu (mit Verlauf)
            sqlx::query("UPDATE threads SET provider_session_id = '' WHERE id = ?").bind(id).execute(&state.db).await?;
            let label = match account_id {
                Some(a) => accounts::get_row(state, user.id(), a).await.map(|a| a.label).unwrap_or_default(),
                None => String::new(),
            };
            let text = format!("Agent: {} · {}", kind.name(), label);
            let data = messages::jv(&SystemData::of("agent_switch"));
            messages::insert(state, user.id(), id, None, "system", &text, &data).await?;
            // Wartet der Thread auf ein Limit: Timer nach den Limits des neuen Kontos richten
            super::resume::after_switch(state, id).await?;
        }
    }
    publish(state, id).await?;
    Ok(Json(summary(state, id).await?))
}

/// `POST /threads/order` – Reihenfolge aus der Seitenleiste (oben zuerst). Die Threads bekommen
/// feste Plätze knapp unter dem neuesten von ihnen; neue Threads erscheinen weiter oben.
#[post("/threads/order", data = "<body>")]
async fn reorder(state: &State<AppState>, user: CurrentUser, body: Json<ThreadOrder>) -> AppResult<Json<Vec<ThreadSummary>>> {
    let mut ids: Vec<i64> = Vec::new();
    for id in body.into_inner().ids {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Ok(Json(Vec::new()));
    }
    if ids.len() > 2000 {
        return Err(AppError::bad("Zu viele Threads."));
    }
    let mut top = i64::MIN;
    for id in &ids {
        let t = get_row(state, user.id(), *id).await?;
        top = top.max(t.sort_key.unwrap_or(t.last_activity_at));
    }
    let mut tx = state.db.begin().await?;
    for (i, id) in ids.iter().enumerate() {
        sqlx::query("UPDATE threads SET sort_key = ? WHERE id = ? AND user_id = ?").bind(top - i as i64).bind(id).bind(user.id()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        publish(state, id).await?;
        out.push(summary(state, id).await?);
    }
    Ok(Json(out))
}

/// `DELETE /threads/<id>` – Läufe (auch der Subagenten) stoppen, dann löschen.
#[delete("/threads/<id>")]
async fn remove(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Status> {
    let t = get_row(state, user.id(), id).await?;
    super::stop_thread(state, id).await?;
    let mut ids = vec![id];
    let mut i = 0;
    while i < ids.len() {
        let kids: Vec<(i64,)> = sqlx::query_as("SELECT id FROM threads WHERE parent_id = ?").bind(ids[i]).fetch_all(&state.db).await?;
        ids.extend(kids.into_iter().map(|(k,)| k));
        i += 1;
    }
    // Deep Research: das Gespräch darüber (eigener Thread) geht mit
    let chats: Vec<(i64,)> = sqlx::query_as("SELECT id FROM threads WHERE discuss_of = ?").bind(id).fetch_all(&state.db).await?;
    for (c,) in &chats {
        super::stop_thread(state, *c).await?;
        ids.push(*c);
    }
    super::terminals::close_for_threads(state, &ids);
    for tid in &ids {
        if let Err(e) = crate::workspace::forget_thread(state, *tid).await {
            tracing::warn!("Worktree von Thread {tid} entfernen: {e}");
        }
    }
    sqlx::query("DELETE FROM threads WHERE id = ? OR discuss_of = ?").bind(id).bind(id).execute(&state.db).await?;
    // … und der Ordner mit dem Paper
    if t.research {
        let _ = tokio::fs::remove_dir_all(super::research::dir(state, id)).await;
    }
    for tid in ids {
        hub::publish(state, user.id(), HubEvent::ThreadDeleted { id: tid });
    }
    if let Some(p) = t.parent_id {
        let _ = publish(state, p).await;
    }
    Ok(Status::NoContent)
}

/// `GET /threads/<id>/messages?after=&limit=` bzw. `?tail=n` (nur die letzten n, für nrzXMobile)
#[get("/threads/<id>/messages?<after>&<limit>&<tail>")]
async fn message_list(state: &State<AppState>, user: CurrentUser, id: i64, after: Option<i64>, limit: Option<i64>, tail: Option<i64>) -> AppResult<Json<Vec<Message>>> {
    get_row(state, user.id(), id).await?;
    if let Some(n) = tail {
        return Ok(Json(messages::tail(state, id, n.clamp(1, 2000)).await?));
    }
    Ok(Json(messages::list(state, id, after.unwrap_or(0), limit.unwrap_or(5000).clamp(1, 20000)).await?))
}

/// `POST /threads/<id>/messages` – Gespräch fortsetzen (neuer Lauf, mit Sitzungs-ID des CLIs).
#[post("/threads/<id>/messages", data = "<body>")]
async fn message_post(state: &State<AppState>, user: CurrentUser, _port: LocalPort, id: i64, body: Json<PostMessage>) -> AppResult<Json<ThreadSummary>> {
    let t = get_row(state, user.id(), id).await?;
    let text = body.text.trim();
    if text.is_empty() && body.attachments.is_empty() {
        return Err(AppError::bad("Bitte eine Nachricht eingeben."));
    }
    if t.account_id.is_none() {
        // Konto wurde gelöscht: Standardkonto der Art nehmen
        let (account_id, _, _) = resolve_agent(state, user.id(), t.kind(), None, &t.model, &t.effort).await?;
        sqlx::query("UPDATE threads SET account_id = ?, provider_session_id = '' WHERE id = ?").bind(account_id).bind(id).execute(&state.db).await?;
    }
    if super::runtime::get(state).is_active(id) {
        // Agent arbeitet noch: zwischenrufen (unterbrechen) oder in die Warteschlange
        if body.interrupt {
            super::queue::interrupt(state, id, text, &body.attachments).await?;
        } else {
            super::queue::push(state, id, text, &body.attachments).await?;
        }
        return Ok(Json(summary(state, id).await?));
    }
    runs::start(state, id, text, &body.attachments).await?;
    Ok(Json(summary(state, id).await?))
}

/// `POST /threads/<id>/stop` – Lauf (und Subagenten) stoppen.
#[post("/threads/<id>/stop")]
async fn stop(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ThreadSummary>> {
    get_row(state, user.id(), id).await?;
    super::stop_thread(state, id).await?;
    // Stopp hebt auch ein geplantes Weitermachen nach einem Rate-Limit auf
    sqlx::query("UPDATE threads SET resume_at = NULL, resume_attempts = 0 WHERE id = ?").bind(id).execute(&state.db).await?;
    publish(state, id).await?;
    Ok(Json(summary(state, id).await?))
}

/// `POST /threads/<id>/read` – gelesen (fertig → ruhend).
#[post("/threads/<id>/read")]
async fn mark_read(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ThreadSummary>> {
    let t = get_row(state, user.id(), id).await?;
    let status = if t.status() == ThreadStatus::Done { ThreadStatus::Idle } else { t.status() };
    sqlx::query("UPDATE threads SET unread = 0, status = ? WHERE id = ?").bind(status.key()).bind(id).execute(&state.db).await?;
    publish(state, id).await?;
    Ok(Json(summary(state, id).await?))
}

/// `POST /threads/<id>/settle` – ablegen („Settled“).
#[post("/threads/<id>/settle")]
async fn settle(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ThreadSummary>> {
    let t = get_row(state, user.id(), id).await?;
    if t.status().is_active() {
        return Err(AppError::conflict("Der Agent arbeitet noch – erst stoppen oder warten."));
    }
    sqlx::query("UPDATE threads SET status = 'settled', unread = 0, updated_at = ? WHERE id = ?").bind(now()).bind(id).execute(&state.db).await?;
    publish(state, id).await?;
    Ok(Json(summary(state, id).await?))
}

/// `POST /threads/<id>/reopen` – abgelegten Thread zurückholen.
#[post("/threads/<id>/reopen")]
async fn reopen(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ThreadSummary>> {
    let t = get_row(state, user.id(), id).await?;
    if t.status() == ThreadStatus::Settled {
        sqlx::query("UPDATE threads SET status = 'idle', updated_at = ? WHERE id = ?").bind(now()).bind(id).execute(&state.db).await?;
    }
    publish(state, id).await?;
    Ok(Json(summary(state, id).await?))
}

/// `POST /threads/<id>/clear` – Kontext zurücksetzen: der nächste Lauf beginnt eine neue
/// Sitzung beim CLI (ohne den bisherigen Verlauf).
#[post("/threads/<id>/clear")]
async fn clear(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ThreadSummary>> {
    get_row(state, user.id(), id).await?;
    if super::runtime::get(state).is_active(id) {
        return Err(AppError::conflict("Der Agent arbeitet noch – erst stoppen oder warten."));
    }
    sqlx::query("UPDATE threads SET provider_session_id = '' WHERE id = ?").bind(id).execute(&state.db).await?;
    let data = messages::jv(&SystemData::of("context_reset"));
    messages::insert(state, user.id(), id, None, "system", "Kontext zurückgesetzt", &data).await?;
    publish(state, id).await?;
    Ok(Json(summary(state, id).await?))
}

/// `POST /threads/<id>/subagents` – der Benutzer startet Subagenten (je Aufgabe ein Thread).
#[post("/threads/<id>/subagents", data = "<body>")]
async fn start_subagents(state: &State<AppState>, user: CurrentUser, _port: LocalPort, id: i64, body: Json<NewSubagents>) -> AppResult<Json<Vec<ThreadSummary>>> {
    let t = get_row(state, user.id(), id).await?;
    if t.read_only {
        return Err(AppError::bad("Im Nur-Lesen-Modus lassen sich keine Subagenten starten."));
    }
    let kids = subagents::spawn(state, &t, body.into_inner().tasks, true).await?;
    Ok(Json(kids))
}

/// `POST /threads/<id>/messages/<mid>/retract` – Benutzernachricht zurückziehen: laufenden
/// Agenten stoppen, die Nachricht und alles danach entfernen und Text und Anhänge zurückgeben
/// (zum Bearbeiten im Composer). Die Sitzung beim CLI beginnt danach neu (mit dem Verlauf bis
/// zur zurückgezogenen Nachricht). Läufe bleiben für die Nutzungsstatistik erhalten; Änderungen
/// am Arbeitsordner bleiben bestehen.
#[post("/threads/<id>/messages/<mid>/retract")]
async fn retract(state: &State<AppState>, user: CurrentUser, id: i64, mid: i64) -> AppResult<Json<RetractedMessage>> {
    get_row(state, user.id(), id).await?;
    let m: Option<(String, String, String)> = sqlx::query_as("SELECT role, content, data FROM messages WHERE id = ? AND thread_id = ?")
        .bind(mid)
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let Some((role, content, data)) = m else {
        return Err(AppError::NotFound);
    };
    if role != "user" {
        return Err(AppError::bad("Nur eigene Nachrichten lassen sich zurückziehen."));
    }
    if super::runtime::get(state).is_active(id) {
        runs::stop_tree(state, id).await?;
    }
    let data: UserMessageData = serde_json::from_str(&data).unwrap_or_default();
    sqlx::query("DELETE FROM messages WHERE thread_id = ? AND id >= ?").bind(id).bind(mid).execute(&state.db).await?;
    let status = ThreadRow::status(&get_row(state, user.id(), id).await?);
    let status = if status.is_active() || status == ThreadStatus::Settled { status } else { ThreadStatus::Idle };
    sqlx::query("UPDATE threads SET provider_session_id = '', resume_at = NULL, resume_attempts = 0, status = ?, unread = 0, updated_at = ? WHERE id = ?")
        .bind(status.key())
        .bind(now())
        .bind(id)
        .execute(&state.db)
        .await?;
    hub::publish(state, user.id(), HubEvent::MessagesRemoved { thread_id: id, from_id: mid });
    publish(state, id).await?;
    Ok(Json(RetractedMessage { text: content, attachments: data.attachments, thread: summary(state, id).await? }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn titles() {
        assert_eq!(super::title_from("  \n# füge einen Login hinzu\nmehr"), "Füge einen Login hinzu");
        let long = "wort ".repeat(30);
        let t = super::title_from(&long);
        assert!(t.chars().count() <= 61 && t.ends_with('…'));
        assert_eq!(super::title_from(""), "Neuer Thread");
    }
}
