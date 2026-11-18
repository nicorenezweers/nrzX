//! Bot-Modus: dauerhafte KI-Teamkollegen (wie Grok Bot). Ein Bot hat Name, Emoji, Rolle,
//! Anweisungen, ein Gedächtnis, das er selbst pflegt (nrzx `remember`/`rewrite_memory`),
//! einen eigenen Arbeitsordner `<data>/bots/<id>` („Computer“, bleibt über alle Gespräche) und
//! Routinen (zu einer Uhrzeit an bestimmten Wochentagen einen Auftrag schicken).
//!
//! Ein Gespräch ist ein gewöhnlicher Thread ohne Projekt mit `bot_id`; `bots.thread_id` ist das
//! aktuelle. Läufe, Warteschlange, Fragen und Live-Ereignisse kommen so unverändert aus
//! [`super::threads`] und [`super::runs`]; nur der Systemhinweis ist ein anderer ([`hint`]).

use super::{runs, threads};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::{hub, AppState};
use nrzx_shared::agents::{Bot, BotInput, BotMcp, BotRoutine, BotRoutineInput};
use nrzx_shared::{AgentKind, HubEvent};
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::{delete, get, patch, post, routes, Route, State};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn routes() -> Vec<Route> {
    routes![list, detail, create, update, remove, new_conversation, open_conversation, routine_create, routine_update, routine_remove]
}

/// Höchstlänge von Anweisungen und Gedächtnis (Zeichen)
const MAX_TEXT: usize = 20_000;

#[derive(sqlx::FromRow)]
struct BotRow {
    id: i64,
    name: String,
    emoji: String,
    role: String,
    instructions: String,
    memory: String,
    agent_kind: String,
    account_id: Option<i64>,
    model: String,
    effort: String,
    thread_id: Option<i64>,
    created_at: i64,
    updated_at: i64,
    mcp: String,
}

#[derive(sqlx::FromRow)]
struct RoutineRow {
    id: i64,
    bot_id: i64,
    title: String,
    prompt: String,
    time_min: i64,
    days: i64,
    tz_offset: i64,
    enabled: bool,
    last_run_at: i64,
}

impl RoutineRow {
    fn to_json(&self) -> BotRoutine {
        BotRoutine {
            id: self.id,
            title: self.title.clone(),
            prompt: self.prompt.clone(),
            time_min: self.time_min,
            days: self.days,
            tz_offset: self.tz_offset,
            enabled: self.enabled,
            last_run_at: self.last_run_at,
        }
    }
}

/// Arbeitsordner („Computer“) eines Bots.
pub fn bot_dir(state: &AppState, bot_id: i64) -> PathBuf {
    state.config.data_dir.join("bots").join(bot_id.to_string())
}

async fn row(state: &AppState, user_id: i64, id: i64) -> AppResult<BotRow> {
    sqlx::query_as("SELECT * FROM bots WHERE id = ? AND user_id = ?").bind(id).bind(user_id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound)
}

async fn to_json(state: &AppState, r: BotRow) -> AppResult<Bot> {
    let routines: Vec<RoutineRow> = sqlx::query_as("SELECT * FROM bot_routines WHERE bot_id = ? ORDER BY time_min, id").bind(r.id).fetch_all(&state.db).await?;
    // Gelöschtes Gespräch (z. B. über die Thread-Liste): kein aktuelles
    let thread_id = match r.thread_id {
        Some(t) => sqlx::query_scalar("SELECT id FROM threads WHERE id = ? AND bot_id = ?").bind(t).bind(r.id).fetch_optional(&state.db).await?,
        None => None,
    };
    Ok(Bot {
        id: r.id,
        name: r.name,
        emoji: r.emoji,
        role: r.role,
        instructions: r.instructions,
        memory: r.memory,
        agent: AgentKind::parse(&r.agent_kind).unwrap_or_default(),
        account_id: r.account_id,
        model: r.model,
        effort: r.effort,
        thread_id,
        routines: routines.iter().map(RoutineRow::to_json).collect(),
        mcp: serde_json::from_str(&r.mcp).unwrap_or_default(),
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

/// Eigene MCP-Server prüfen: Name eindeutig und gültig (nicht `nrzx`/`nrzgit`), Adresse http(s)
/// oder ein Befehl.
fn clean_mcp(list: &[BotMcp]) -> AppResult<Vec<BotMcp>> {
    let mut out: Vec<BotMcp> = Vec::new();
    for m in list.iter().take(20) {
        let name: String = m.name.trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(40).collect();
        let url = m.url.trim().to_string();
        let command = m.command.trim().to_string();
        if name.is_empty() && url.is_empty() && command.is_empty() {
            continue;
        }
        if name.is_empty() {
            return Err(AppError::bad("Jeder MCP-Server braucht einen Namen (Buchstaben, Ziffern, - oder _)."));
        }
        if matches!(name.as_str(), "nrzx" | "nrzgit") || out.iter().any(|o| o.name.eq_ignore_ascii_case(&name)) {
            return Err(AppError::bad(format!("Den Namen „{name}“ gibt es schon.")));
        }
        if url.is_empty() && command.is_empty() {
            return Err(AppError::bad(format!("„{name}“: bitte eine Adresse oder einen Befehl angeben.")));
        }
        if !url.is_empty() && !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(AppError::bad(format!("„{name}“: die Adresse muss mit https:// beginnen.")));
        }
        out.push(BotMcp { name, url, token: m.token.trim().to_string(), command: if m.url.trim().is_empty() { command } else { String::new() }, enabled: m.enabled });
    }
    Ok(out)
}

/// Eingeschaltete eigene MCP-Server eines Bots (für seine Läufe).
pub async fn mcp_servers(state: &AppState, bot_id: i64) -> Vec<BotMcp> {
    let raw: Option<String> = sqlx::query_scalar("SELECT mcp FROM bots WHERE id = ?").bind(bot_id).fetch_optional(&state.db).await.ok().flatten();
    raw.and_then(|r| serde_json::from_str::<Vec<BotMcp>>(&r).ok()).unwrap_or_default().into_iter().filter(|m| m.enabled).collect()
}

fn clip(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

fn long_text(s: &str, what: &str) -> AppResult<String> {
    if s.chars().count() > MAX_TEXT {
        return Err(AppError::bad(format!("{what} ist zu lang (höchstens {MAX_TEXT} Zeichen).")));
    }
    Ok(s.trim().to_string())
}

/// `GET /bots`
#[get("/bots")]
async fn list(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<Vec<Bot>>> {
    let rows: Vec<BotRow> = sqlx::query_as("SELECT * FROM bots WHERE user_id = ? ORDER BY name COLLATE NOCASE, id").bind(user.id()).fetch_all(&state.db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(to_json(state, r).await?);
    }
    Ok(Json(out))
}

/// `GET /bots/<id>`
#[get("/bots/<id>")]
async fn detail(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<Bot>> {
    let r = row(state, user.id(), id).await?;
    Ok(Json(to_json(state, r).await?))
}

/// `POST /bots` – Bot anlegen (mit erstem, leerem Gespräch).
#[post("/bots", data = "<body>")]
async fn create(state: &State<AppState>, user: CurrentUser, body: Json<BotInput>) -> AppResult<Json<Bot>> {
    let b = body.into_inner();
    let name = clip(b.name.as_deref().unwrap_or(""), 60);
    if name.is_empty() {
        return Err(AppError::bad("Bitte einen Namen eingeben."));
    }
    let kind = b.agent.unwrap_or_default();
    let (account_id, model, effort) =
        threads::resolve_agent(state, user.id(), kind, b.account_id.flatten(), b.model.as_deref().unwrap_or(""), b.effort.as_deref().unwrap_or("")).await?;
    let ts = now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO bots (user_id, name, emoji, role, instructions, memory, agent_kind, account_id, model, effort, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user.id())
    .bind(&name)
    .bind(clip(b.emoji.as_deref().unwrap_or(""), 8))
    .bind(clip(b.role.as_deref().unwrap_or(""), 80))
    .bind(long_text(b.instructions.as_deref().unwrap_or(""), "Die Anweisung")?)
    .bind(long_text(b.memory.as_deref().unwrap_or(""), "Das Gedächtnis")?)
    .bind(kind.key())
    .bind(account_id)
    .bind(&model)
    .bind(&effort)
    .bind(ts)
    .bind(ts)
    .fetch_one(&state.db)
    .await?;
    if let Some(list) = &b.mcp {
        let json = serde_json::to_string(&clean_mcp(list)?).unwrap_or_else(|_| "[]".into());
        sqlx::query("UPDATE bots SET mcp = ? WHERE id = ?").bind(json).bind(id).execute(&state.db).await?;
    }
    tokio::fs::create_dir_all(bot_dir(state, id)).await?;
    start_conversation(state, user.id(), id).await?;
    let r = row(state, user.id(), id).await?;
    Ok(Json(to_json(state, r).await?))
}

/// `PATCH /bots/<id>` – Persönlichkeit, Gedächtnis oder Agent ändern. Ein neuer Agent gilt ab
/// dem nächsten Gespräch und sofort für das aktuelle, solange es nicht läuft.
#[patch("/bots/<id>", data = "<body>")]
async fn update(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<BotInput>) -> AppResult<Json<Bot>> {
    let mut r = row(state, user.id(), id).await?;
    let b = body.into_inner();
    if let Some(n) = &b.name {
        let n = clip(n, 60);
        if n.is_empty() {
            return Err(AppError::bad("Bitte einen Namen eingeben."));
        }
        r.name = n;
    }
    if let Some(e) = &b.emoji {
        r.emoji = clip(e, 8);
    }
    if let Some(x) = &b.role {
        r.role = clip(x, 80);
    }
    if let Some(x) = &b.instructions {
        r.instructions = long_text(x, "Die Anweisung")?;
    }
    if let Some(x) = &b.memory {
        r.memory = long_text(x, "Das Gedächtnis")?;
    }
    if let Some(list) = &b.mcp {
        r.mcp = serde_json::to_string(&clean_mcp(list)?).unwrap_or_else(|_| "[]".into());
    }
    let agent_changed = b.agent.is_some() || b.account_id.is_some() || b.model.is_some() || b.effort.is_some();
    if agent_changed {
        let kind = b.agent.unwrap_or_else(|| AgentKind::parse(&r.agent_kind).unwrap_or_default());
        let account = match b.account_id {
            Some(a) => a,
            None if b.agent.is_some() => None,
            None => r.account_id,
        };
        let model = b.model.clone().unwrap_or_else(|| if b.agent.is_some() { String::new() } else { r.model.clone() });
        let effort = b.effort.clone().unwrap_or_else(|| if b.agent.is_some() { String::new() } else { r.effort.clone() });
        let (account_id, model, effort) = threads::resolve_agent(state, user.id(), kind, account, &model, &effort).await?;
        r.agent_kind = kind.key().to_string();
        r.account_id = account_id;
        r.model = model;
        r.effort = effort;
    }
    sqlx::query("UPDATE bots SET name = ?, emoji = ?, role = ?, instructions = ?, memory = ?, agent_kind = ?, account_id = ?, model = ?, effort = ?, mcp = ?, updated_at = ? WHERE id = ?")
        .bind(&r.name)
        .bind(&r.emoji)
        .bind(&r.role)
        .bind(&r.instructions)
        .bind(&r.memory)
        .bind(&r.agent_kind)
        .bind(r.account_id)
        .bind(&r.model)
        .bind(&r.effort)
        .bind(&r.mcp)
        .bind(now())
        .bind(id)
        .execute(&state.db)
        .await?;
    // Titel der Gespräche folgen dem Namen; ein neuer Agent gilt auch im ruhenden aktuellen Gespräch
    sqlx::query("UPDATE threads SET title = ? WHERE bot_id = ? AND parent_id IS NULL").bind(&r.name).bind(id).execute(&state.db).await?;
    if let Some(t) = r.thread_id.filter(|t| agent_changed && !super::runtime::get(state).is_active(*t)) {
        sqlx::query("UPDATE threads SET agent_kind = ?, account_id = ?, model = ?, effort = ?, provider_session_id = '' WHERE id = ? AND bot_id = ?")
            .bind(&r.agent_kind)
            .bind(r.account_id)
            .bind(&r.model)
            .bind(&r.effort)
            .bind(t)
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM threads WHERE bot_id = ?").bind(id).fetch_all(&state.db).await?;
    for t in ids {
        let _ = threads::publish(state, t).await;
    }
    let r = row(state, user.id(), id).await?;
    Ok(Json(to_json(state, r).await?))
}

/// `DELETE /bots/<id>` – Bot mit allen Gesprächen, Routinen und seinem Arbeitsordner löschen.
#[delete("/bots/<id>")]
async fn remove(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Status> {
    row(state, user.id(), id).await?;
    let roots: Vec<i64> = sqlx::query_scalar("SELECT id FROM threads WHERE bot_id = ?").bind(id).fetch_all(&state.db).await?;
    let mut ids = roots.clone();
    let mut i = 0;
    while i < ids.len() {
        let kids: Vec<i64> = sqlx::query_scalar("SELECT id FROM threads WHERE parent_id = ?").bind(ids[i]).fetch_all(&state.db).await?;
        ids.extend(kids);
        i += 1;
    }
    for t in &roots {
        super::stop_thread(state, *t).await?;
    }
    super::terminals::close_for_threads(state, &ids);
    for t in &ids {
        let _ = crate::workspace::forget_thread(state, *t).await;
    }
    sqlx::query("DELETE FROM threads WHERE bot_id = ?").bind(id).execute(&state.db).await?;
    sqlx::query("DELETE FROM bots WHERE id = ?").bind(id).execute(&state.db).await?;
    for t in ids {
        hub::publish(state, user.id(), HubEvent::ThreadDeleted { id: t });
    }
    let dir = bot_dir(state, id);
    if dir.exists() {
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
    Ok(Status::NoContent)
}

/// Neues, leeres Gespräch anlegen und zum aktuellen machen (das Gedächtnis bleibt).
async fn start_conversation(state: &AppState, user_id: i64, bot_id: i64) -> AppResult<i64> {
    let b = row(state, user_id, bot_id).await?;
    let kind = AgentKind::parse(&b.agent_kind).unwrap_or_default();
    let (account_id, model, effort) = threads::resolve_agent(state, user_id, kind, b.account_id, &b.model, &b.effort).await?;
    let tid = threads::insert(state, user_id, None, None, &b.name, kind, account_id, &model, &effort, false).await?;
    sqlx::query("UPDATE threads SET bot_id = ?, resume_auto = (SELECT auto_resume FROM users WHERE id = ?) WHERE id = ?")
        .bind(bot_id)
        .bind(user_id)
        .bind(tid)
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE bots SET thread_id = ?, updated_at = ? WHERE id = ?").bind(tid).bind(now()).bind(bot_id).execute(&state.db).await?;
    threads::publish(state, tid).await?;
    Ok(tid)
}

/// `POST /bots/<id>/conversation` – neues Gespräch beginnen.
#[post("/bots/<id>/conversation")]
async fn new_conversation(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<Bot>> {
    start_conversation(state, user.id(), id).await?;
    let r = row(state, user.id(), id).await?;
    Ok(Json(to_json(state, r).await?))
}

/// `POST /bots/<id>/conversation/<tid>` – früheres Gespräch wieder zum aktuellen machen.
#[post("/bots/<id>/conversation/<tid>")]
async fn open_conversation(state: &State<AppState>, user: CurrentUser, id: i64, tid: i64) -> AppResult<Json<Bot>> {
    row(state, user.id(), id).await?;
    let t = threads::get_row(state, user.id(), tid).await?;
    if t.bot_id != Some(id) {
        return Err(AppError::NotFound);
    }
    sqlx::query("UPDATE bots SET thread_id = ? WHERE id = ?").bind(tid).bind(id).execute(&state.db).await?;
    let r = row(state, user.id(), id).await?;
    Ok(Json(to_json(state, r).await?))
}

fn check_routine(b: &BotRoutineInput) -> AppResult<()> {
    if b.prompt.trim().is_empty() {
        return Err(AppError::bad("Bitte einen Auftrag für die Routine eingeben."));
    }
    if b.prompt.chars().count() > MAX_TEXT {
        return Err(AppError::bad("Der Auftrag ist zu lang."));
    }
    if !(0..1440).contains(&b.time_min) {
        return Err(AppError::bad("Ungültige Uhrzeit."));
    }
    if !(1..=127).contains(&b.days) {
        return Err(AppError::bad("Bitte mindestens einen Wochentag wählen."));
    }
    if !(-14 * 60..=14 * 60).contains(&b.tz_offset) {
        return Err(AppError::bad("Ungültige Zeitzone."));
    }
    Ok(())
}

/// `POST /bots/<id>/routines`
#[post("/bots/<id>/routines", data = "<body>")]
async fn routine_create(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<BotRoutineInput>) -> AppResult<Json<Bot>> {
    row(state, user.id(), id).await?;
    check_routine(&body)?;
    sqlx::query("INSERT INTO bot_routines (bot_id, title, prompt, time_min, days, tz_offset, enabled, last_run_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(clip(&body.title, 80))
        .bind(body.prompt.trim())
        .bind(body.time_min)
        .bind(body.days)
        .bind(body.tz_offset)
        .bind(body.enabled)
        // Nicht sofort nachholen, wenn die Uhrzeit heute schon vorbei ist
        .bind(now())
        .bind(now())
        .execute(&state.db)
        .await?;
    let r = row(state, user.id(), id).await?;
    Ok(Json(to_json(state, r).await?))
}

/// `PATCH /bots/<id>/routines/<rid>`
#[patch("/bots/<id>/routines/<rid>", data = "<body>")]
async fn routine_update(state: &State<AppState>, user: CurrentUser, id: i64, rid: i64, body: Json<BotRoutineInput>) -> AppResult<Json<Bot>> {
    row(state, user.id(), id).await?;
    check_routine(&body)?;
    let n = sqlx::query("UPDATE bot_routines SET title = ?, prompt = ?, time_min = ?, days = ?, tz_offset = ?, enabled = ?, last_run_at = MAX(last_run_at, ?) WHERE id = ? AND bot_id = ?")
        .bind(clip(&body.title, 80))
        .bind(body.prompt.trim())
        .bind(body.time_min)
        .bind(body.days)
        .bind(body.tz_offset)
        .bind(body.enabled)
        .bind(now())
        .bind(rid)
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    let r = row(state, user.id(), id).await?;
    Ok(Json(to_json(state, r).await?))
}

/// `DELETE /bots/<id>/routines/<rid>`
#[delete("/bots/<id>/routines/<rid>")]
async fn routine_remove(state: &State<AppState>, user: CurrentUser, id: i64, rid: i64) -> AppResult<Json<Bot>> {
    row(state, user.id(), id).await?;
    sqlx::query("DELETE FROM bot_routines WHERE id = ? AND bot_id = ?").bind(rid).bind(id).execute(&state.db).await?;
    let r = row(state, user.id(), id).await?;
    Ok(Json(to_json(state, r).await?))
}

// ------------------------------------------------------------------ Routinen

/// Fälliger Zeitpunkt (Unix-Sekunden) einer Routine am heutigen Tag ihrer Ortszeit, wenn sie
/// heute läuft.
fn due_today(time_min: i64, days: i64, tz_offset: i64, now: i64) -> Option<i64> {
    let local = now + tz_offset * 60;
    let day = local.div_euclid(86_400);
    // 1.1.1970 war ein Donnerstag: Montag = 0
    let weekday = (day + 3).rem_euclid(7);
    if days & (1 << weekday) == 0 {
        return None;
    }
    Some(day * 86_400 + time_min * 60 - tz_offset * 60)
}

/// Alle 30 s fällige Routinen ausführen (verpasste höchstens eine Stunde später nachholen).
pub fn spawn_loop(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if let Err(e) = tick(&state).await {
                tracing::warn!("Routinen der Bots: {e}");
            }
        }
    });
}

async fn tick(state: &AppState) -> AppResult<()> {
    let ts = now();
    let list: Vec<RoutineRow> = sqlx::query_as("SELECT * FROM bot_routines WHERE enabled = 1").fetch_all(&state.db).await?;
    for r in list {
        let Some(due) = due_today(r.time_min, r.days, r.tz_offset, ts) else { continue };
        if ts < due || r.last_run_at >= due || ts - due > 3600 {
            continue;
        }
        sqlx::query("UPDATE bot_routines SET last_run_at = ? WHERE id = ?").bind(ts).bind(r.id).execute(&state.db).await?;
        if let Err(e) = fire(state, &r).await {
            tracing::warn!("Routine {} von Bot {}: {e}", r.id, r.bot_id);
        }
    }
    Ok(())
}

async fn fire(state: &AppState, r: &RoutineRow) -> AppResult<()> {
    let (user_id, thread_id): (i64, Option<i64>) = sqlx::query_as("SELECT user_id, thread_id FROM bots WHERE id = ?").bind(r.bot_id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound)?;
    let thread_id = match thread_id {
        Some(t) if threads::row(state, t).await.is_ok() => t,
        _ => start_conversation(state, user_id, r.bot_id).await?,
    };
    let title = if r.title.trim().is_empty() { "Routine".to_string() } else { format!("Routine „{}“", r.title.trim()) };
    let text = format!("[{title}] {}", r.prompt.trim());
    if super::runtime::get(state).is_active(thread_id) {
        super::queue::push(state, thread_id, &text, &[]).await
    } else {
        runs::start(state, thread_id, &text, &[]).await.map(|_| ())
    }
}

// ------------------------------------------------------------------ Lauf: Hinweis und Werkzeuge

/// Systemhinweis für ein Gespräch mit einem Bot (statt des Hinweises für Coding-Agenten).
pub async fn hint(state: &AppState, t: &threads::ThreadRow, workdir: &Path, forges: &str) -> Option<String> {
    let bot_id = t.bot_id?;
    let b: BotRow = sqlx::query_as("SELECT * FROM bots WHERE id = ?").bind(bot_id).fetch_optional(&state.db).await.ok()??;
    let mut h = format!("You are \"{}\"", b.name);
    if !b.role.is_empty() {
        h.push_str(&format!(" ({})", b.role));
    }
    h.push_str(
        ", a persistent AI teammate of the user in nrzX's Bot mode – a personal assistant bot, not only a coder. \
         Talk naturally: direct, warm, concise; Markdown when it helps. When asked, do the work yourself \
         (research the web, write, plan, analyse, run commands, build files or small apps).\n",
    );
    h.push_str(&format!("Your own computer: cwd {} – this folder persists across all your conversations; keep useful files, notes and scripts there.\n", workdir.display()));
    if !b.instructions.trim().is_empty() {
        h.push_str("Instructions from the user:\n");
        h.push_str(b.instructions.trim());
        h.push('\n');
    }
    h.push_str("Your memory (persists across conversations):\n");
    h.push_str(if b.memory.trim().is_empty() { "(empty)" } else { b.memory.trim() });
    h.push('\n');
    h.push_str(
        "Save lasting facts (who the user is, preferences, ongoing projects, decisions) with nrzx remember – one short fact each; \
         reorganise the whole memory with nrzx rewrite_memory when it gets long or outdated. Never store secrets.\n",
    );
    // Experte für eine Sache, eigene Apps (MCP), was man ihm sagt, merkt er sich
    if !b.role.is_empty() {
        h.push_str(&format!("You are the user's expert for: {} – stay focused on that and be excellent at it.\n", b.role));
    }
    let own: Vec<BotMcp> = serde_json::from_str::<Vec<BotMcp>>(&b.mcp).unwrap_or_default().into_iter().filter(|m| m.enabled).collect();
    if !own.is_empty() {
        let names: Vec<&str> = own.iter().map(|m| m.name.as_str()).collect();
        h.push_str(&format!("Your own apps (MCP servers only you can use): {} – use them when they help.\n", names.join(", ")));
    }
    h.push_str("When the user tells you something to keep (\"merk dir …\", \"remember …\"), save it with nrzx remember right away.\n");
    h.push_str("Messages starting with [Routine …] are scheduled tasks the user set up: do them and report the result briefly.\n");
    h.push_str("If a decision truly needs the user, call nrzx ask_user with short options; otherwise decide yourself.\n");
    h.push_str(forges);
    h.push_str("Reply in the user's language.");
    Some(h)
}

/// Werkzeuge des Gedächtnisses (nur in Gesprächen mit einem Bot).
pub fn memory_tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "remember",
            "title": "Remember a fact",
            "description": "Add one short, lasting fact to your memory (it is shown to you in every future conversation). Not for secrets or one-off details.",
            "inputSchema": {"type": "object", "properties": {"fact": {"type": "string"}}, "required": ["fact"]}
        }),
        serde_json::json!({
            "name": "rewrite_memory",
            "title": "Rewrite memory",
            "description": "Replace your whole memory with a cleaned-up version (Markdown bullet list). Keep everything still relevant.",
            "inputSchema": {"type": "object", "properties": {"memory": {"type": "string"}}, "required": ["memory"]}
        }),
    ]
}

/// `remember` / `rewrite_memory` ausführen.
pub async fn memory_call(state: &AppState, t: &threads::ThreadRow, name: &str, args: &serde_json::Value) -> AppResult<serde_json::Value> {
    let bot_id = t.bot_id.ok_or_else(|| AppError::bad("Nur in Gesprächen mit einem Bot."))?;
    let memory: String = sqlx::query_scalar("SELECT memory FROM bots WHERE id = ?").bind(bot_id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound)?;
    let next = if name == "remember" {
        let fact = args["fact"].as_str().unwrap_or("").trim().replace('\n', " ");
        if fact.is_empty() {
            return Err(AppError::bad("fact fehlt"));
        }
        let fact = fact.trim_start_matches(['-', '*', ' ']).to_string();
        if memory.trim().is_empty() { format!("- {fact}") } else { format!("{}\n- {fact}", memory.trim_end()) }
    } else {
        args["memory"].as_str().unwrap_or("").trim().to_string()
    };
    let next = long_text(&next, "Das Gedächtnis").map_err(|_| AppError::bad("Memory is full – use rewrite_memory to condense it."))?;
    sqlx::query("UPDATE bots SET memory = ?, updated_at = ? WHERE id = ?").bind(&next).bind(now()).bind(bot_id).execute(&state.db).await?;
    Ok(serde_json::json!({"saved": true, "memoryChars": next.chars().count()}))
}

#[cfg(test)]
mod tests {
    #[test]
    fn own_mcp_servers_are_checked() {
        use nrzx_shared::agents::BotMcp;
        let ok = super::clean_mcp(&[
            BotMcp { name: " notion ".into(), url: "https://mcp.notion.com/mcp".into(), token: "t".into(), command: "x".into(), enabled: true },
            BotMcp { name: "mem".into(), command: "npx -y @modelcontextprotocol/server-memory".into(), enabled: false, ..Default::default() },
            BotMcp::default(),
        ])
        .unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].name, "notion");
        assert!(ok[0].command.is_empty(), "mit Adresse kein Befehl");
        assert_eq!(ok[1].command, "npx -y @modelcontextprotocol/server-memory");
        assert!(super::clean_mcp(&[BotMcp { name: "nrzx".into(), url: "https://a".into(), ..Default::default() }]).is_err());
        assert!(super::clean_mcp(&[BotMcp { name: "a".into(), url: "ftp://a".into(), ..Default::default() }]).is_err());
        assert!(super::clean_mcp(&[BotMcp { name: "a".into(), ..Default::default() }]).is_err());
        let twice = BotMcp { name: "a".into(), url: "https://a".into(), ..Default::default() };
        assert!(super::clean_mcp(&[twice.clone(), twice]).is_err());
    }

    use super::due_today;

    #[test]
    fn routine_due() {
        // Montag, 2.1.2023, 00:00 UTC
        let monday = 1_672_617_600;
        // Mo 08:00 UTC
        assert_eq!(due_today(480, 1, 0, monday + 3600), Some(monday + 8 * 3600));
        // nur Dienstag: heute nicht
        assert_eq!(due_today(480, 2, 0, monday + 3600), None);
        // UTC+1: 08:00 Ortszeit = 07:00 UTC
        assert_eq!(due_today(480, 1, 60, monday + 3600), Some(monday + 7 * 3600));
    }
}
