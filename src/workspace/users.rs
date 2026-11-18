//! Benutzer und Anmeldung (wie in nrzGit): Status der Instanz, Ersteinrichtung (erstes Konto
//! wird Administrator), Registrierung, Anmelden, Abmelden, `/me`, Profil, Passwort,
//! Erscheinungsbild, Statistik, Profilbild (eigenes oder Identicon) und die Benutzerverwaltung für
//! Administratoren.
//!
//! Routen: `GET /auth/status`, `POST /auth/setup|register|login|logout`, `GET /me`,
//! `PUT /me/profile|password|appearance`, `PUT/DELETE /me/avatar`, `GET /me/stats`, `GET /users/<name>/avatar`,
//! `GET/POST /users`, `PATCH/DELETE /users/<id>`.

use super::model::{self, check_email, check_len, me_json};
use crate::auth::{password, session, AdminUser, CurrentUser};
use crate::db::{self, now, User};
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::workspace::{
    AgentStat, Appearance, AvatarUpload, CreateUser, EditUser, LoginRequest, PasswordChange, ProfileUpdate, RegisterRequest,
    SetupRequest, StatWindow, Stats, ThreadStats, TokenSums, UserInfo, ModelStat, AgentUsage, UsagePoint, UsageReport, UsageSeries,
};
use nrzx_shared::{AgentKind, AuthStatus, HubEvent, Me};
use rocket::http::{ContentType, CookieJar, Header, Status};
use rocket::request::{FromRequest, Outcome, Request};
use rocket::response::{self, Responder, Response};
use rocket::serde::json::Json;
use rocket::{delete, get, patch, post, put, routes, Route, State};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{LazyLock, Mutex};

pub fn routes() -> Vec<Route> {
    routes![
        status,
        setup,
        register,
        login,
        logout,
        me,
        update_profile,
        change_password,
        appearance,
        stats,
        thread_stats,
        usage_report,
        avatar,
        put_avatar,
        delete_avatar,
        list_users,
        create_user_route,
        edit_user,
        delete_user
    ]
}

/// `User-Agent` der Anfrage (für die Sitzungsliste).
pub struct UserAgent(pub String);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for UserAgent {
    type Error = std::convert::Infallible;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        Outcome::Success(UserAgent(req.headers().get_one("User-Agent").unwrap_or("").to_string()))
    }
}

async fn has_users(db: &SqlitePool) -> AppResult<bool> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(db).await?;
    Ok(n > 0)
}

pub async fn registration_open(db: &SqlitePool) -> AppResult<bool> {
    Ok(db::setting(db, "registration_open", "false").await? == "true")
}

/// `GET /auth/status` – ohne Anmeldung.
#[get("/auth/status")]
async fn status(state: &State<AppState>) -> AppResult<Json<AuthStatus>> {
    Ok(Json(AuthStatus {
        needs_setup: !has_users(&state.db).await?,
        registration_open: registration_open(&state.db).await?,
        version: env!("CARGO_PKG_VERSION").into(),
    }))
}

/// Neues Konto prüfen und anlegen. Ohne `admin` wird nur das allererste Konto Administrator.
pub async fn create_account(
    db: &SqlitePool,
    username: &str,
    email: &str,
    pw: &str,
    full_name: &str,
    admin: Option<bool>,
) -> AppResult<User> {
    let username = username.trim();
    crate::web::check_name(username, "Benutzername")?;
    let email = check_email(email)?;
    password::check_strength(pw)?;
    let full_name = check_len(full_name, 100, "Voller Name")?;
    let taken: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE lower_name = ? OR lower(email) = lower(?)")
        .bind(username.to_lowercase())
        .bind(&email)
        .fetch_one(db)
        .await?;
    if taken > 0 {
        return Err(AppError::conflict("Name oder E-Mail-Adresse werden schon verwendet."));
    }
    let hash = password::hash(pw.to_string()).await?;
    let ts = now();
    // Ein Ausdruck: auch bei gleichzeitigen Anfragen wird nur das erste Konto Administrator
    let admin_sql = match admin {
        Some(true) => "1",
        Some(false) => "0",
        None => "(SELECT COUNT(*) = 0 FROM users)",
    };
    let sql = format!(
        "INSERT INTO users (username, lower_name, full_name, email, password_hash, is_admin, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, {admin_sql}, ?, ?) RETURNING id"
    );
    let id: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(username)
        .bind(username.to_lowercase())
        .bind(&full_name)
        .bind(&email)
        .bind(hash)
        .bind(ts)
        .bind(ts)
        .fetch_one(db)
        .await
        .map_err(|e| AppError::unique(e, "Name oder E-Mail-Adresse werden schon verwendet."))?;
    db::user_by_id(db, id).await?.ok_or(AppError::NotFound)
}

/// Sitzung öffnen und Cookie setzen.
async fn sign_in(state: &AppState, cookies: &CookieJar<'_>, user: &User, ua: &str) -> AppResult<Me> {
    let token = session::create(&state.db, user.id, ua).await?;
    session::set_cookie(cookies, token, state.config.cookie_secure);
    me_json(&state.db, user).await
}

/// `POST /auth/setup` – Ersteinrichtung: erstes Konto (Administrator).
#[post("/auth/setup", data = "<body>")]
async fn setup(state: &State<AppState>, cookies: &CookieJar<'_>, ua: UserAgent, body: Json<SetupRequest>) -> AppResult<Json<Me>> {
    let db = &state.db;
    if has_users(db).await? {
        return Err(AppError::conflict("nrzX ist schon eingerichtet."));
    }
    let user = create_account(db, &body.username, &body.email, &body.password, &body.full_name, None).await?;
    if !user.is_admin {
        return Err(AppError::conflict("nrzX ist schon eingerichtet."));
    }
    db::set_setting(db, "registration_open", if body.registration_open { "true" } else { "false" }).await?;
    Ok(Json(sign_in(state, cookies, &user, &ua.0).await?))
}

/// `POST /auth/register` – nur bei offener Registrierung. Meldet an.
#[post("/auth/register", data = "<body>")]
async fn register(
    state: &State<AppState>,
    cookies: &CookieJar<'_>,
    ua: UserAgent,
    body: Json<RegisterRequest>,
) -> AppResult<(Status, Json<Me>)> {
    let db = &state.db;
    if has_users(db).await? && !registration_open(db).await? {
        return Err(AppError::forbidden("Die Registrierung ist geschlossen."));
    }
    let user = create_account(db, &body.username, &body.email, &body.password, &body.full_name, None).await?;
    Ok((Status::Created, Json(sign_in(state, cookies, &user, &ua.0).await?)))
}

// ------------------------------------------------------------------ Anmelden

/// Fehlversuche je Anmeldename: (Anzahl, erster Versuch).
static FAILURES: LazyLock<Mutex<HashMap<String, (u32, i64)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const MAX_FAILURES: u32 = 10;
const FAILURE_WINDOW: i64 = 15 * 60;

fn throttled(login: &str) -> bool {
    let mut map = FAILURES.lock().unwrap_or_else(|e| e.into_inner());
    let ts = now();
    map.retain(|_, (_, first)| ts - *first < FAILURE_WINDOW);
    map.get(login).is_some_and(|(n, _)| *n >= MAX_FAILURES)
}

fn record_failure(login: &str) {
    let mut map = FAILURES.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(login.to_string()).or_insert((0, now())).0 += 1;
}

fn clear_failures(login: &str) {
    FAILURES.lock().unwrap_or_else(|e| e.into_inner()).remove(login);
}

/// `POST /auth/login` – Name oder E-Mail plus Passwort.
#[post("/auth/login", data = "<body>")]
async fn login(state: &State<AppState>, cookies: &CookieJar<'_>, ua: UserAgent, body: Json<LoginRequest>) -> AppResult<Json<Me>> {
    let db = &state.db;
    let login = body.login.trim().to_lowercase();
    if login.is_empty() || body.password.is_empty() {
        return Err(AppError::bad("Bitte Benutzername und Passwort angeben."));
    }
    if throttled(&login) {
        return Err(AppError::forbidden("Zu viele Fehlversuche – bitte in einer Viertelstunde erneut versuchen."));
    }
    let user: Option<User> = sqlx::query_as("SELECT * FROM users WHERE lower_name = ? OR lower(email) = ?")
        .bind(&login)
        .bind(&login)
        .fetch_optional(db)
        .await?;
    // Immer gleich lange prüfen – auch bei unbekannten Namen
    let hash = user.as_ref().map(|u| u.password_hash.clone()).unwrap_or_else(|| password::DUMMY_HASH.clone());
    let ok = password::verify(body.password.clone(), hash).await;
    let user = match user {
        Some(u) if ok => u,
        _ => {
            record_failure(&login);
            return Err(AppError::bad("Benutzername oder Passwort stimmen nicht."));
        }
    };
    if !user.is_active {
        return Err(AppError::forbidden("Dieses Konto ist gesperrt."));
    }
    clear_failures(&login);
    Ok(Json(sign_in(state, cookies, &user, &ua.0).await?))
}

/// `POST /auth/logout` – Sitzung beenden (auch ohne gültige Sitzung kein Fehler).
#[post("/auth/logout")]
async fn logout(state: &State<AppState>, cookies: &CookieJar<'_>) -> AppResult<Status> {
    if let Some(token) = session::token_from(cookies) {
        session::destroy(&state.db, &token).await?;
    }
    session::clear_cookie(cookies);
    Ok(Status::NoContent)
}

// ------------------------------------------------------------------ Eigenes Konto

/// `GET /me` – der angemeldete Benutzer.
#[get("/me")]
async fn me(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<Me>> {
    Ok(Json(me_json(&state.db, &user.0).await?))
}

/// `PUT /me/profile` – voller Name und E-Mail.
#[put("/me/profile", data = "<body>")]
async fn update_profile(state: &State<AppState>, user: CurrentUser, body: Json<ProfileUpdate>) -> AppResult<Json<Me>> {
    let full_name = check_len(&body.full_name, 100, "Voller Name")?;
    let email = check_email(&body.email)?;
    let taken: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE lower(email) = lower(?) AND id <> ?")
        .bind(&email)
        .bind(user.id())
        .fetch_one(&state.db)
        .await?;
    if taken > 0 {
        return Err(AppError::conflict("Diese E-Mail-Adresse wird schon verwendet."));
    }
    sqlx::query("UPDATE users SET full_name = ?, email = ?, updated_at = ? WHERE id = ?")
        .bind(&full_name)
        .bind(&email)
        .bind(now())
        .bind(user.id())
        .execute(&state.db)
        .await?;
    let u = db::user_by_id(&state.db, user.id()).await?.ok_or(AppError::NotFound)?;
    Ok(Json(me_json(&state.db, &u).await?))
}

/// `PUT /me/password` – neues Passwort; alle anderen Sitzungen enden.
#[put("/me/password", data = "<body>")]
async fn change_password(
    state: &State<AppState>,
    user: CurrentUser,
    cookies: &CookieJar<'_>,
    ua: UserAgent,
    body: Json<PasswordChange>,
) -> AppResult<Status> {
    if !password::verify(body.current_password.clone(), user.0.password_hash.clone()).await {
        return Err(AppError::bad("Das bisherige Passwort stimmt nicht."));
    }
    password::check_strength(&body.new_password)?;
    let hash = password::hash(body.new_password.clone()).await?;
    sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
        .bind(hash)
        .bind(now())
        .bind(user.id())
        .execute(&state.db)
        .await?;
    session::destroy_all(&state.db, user.id()).await?;
    let token = session::create(&state.db, user.id(), &ua.0).await?;
    session::set_cookie(cookies, token, state.config.cookie_secure);
    Ok(Status::NoContent)
}

/// `PUT /me/appearance` – Thema und Sprache.
#[put("/me/appearance", data = "<body>")]
async fn appearance(state: &State<AppState>, user: CurrentUser, body: Json<Appearance>) -> AppResult<Json<Appearance>> {
    let theme = match body.theme.as_str() {
        "light" | "system" | "dark" | "mondnacht" => body.theme.clone(),
        _ => return Err(AppError::bad("Unbekanntes Thema.")),
    };
    let language = body.language.trim().to_string();
    if language.len() > 10 || !language.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
        return Err(AppError::bad("Unbekannte Sprache."));
    }
    sqlx::query("UPDATE users SET theme = ?, language = ?, updated_at = ? WHERE id = ?")
        .bind(&theme)
        .bind(&language)
        .bind(now())
        .bind(user.id())
        .execute(&state.db)
        .await?;
    Ok(Json(Appearance { theme, language }))
}

/// Summen über Läufe als SQL (Tabelle `r`): Anzahl, Tokens genau aufgeschlüsselt, Kosten, Dauer.
const SUMS: &str = "COUNT(r.id), COALESCE(SUM(r.input_tokens), 0), COALESCE(SUM(r.output_tokens), 0), \
     COALESCE(SUM(r.cache_read_tokens), 0), COALESCE(SUM(r.cache_write_tokens), 0), COALESCE(SUM(r.reasoning_tokens), 0), \
     COALESCE(SUM(r.cost_usd), 0.0), COALESCE(SUM(MAX(COALESCE(r.finished_at, r.started_at) - r.started_at, 0)), 0)";

type SumRow = (i64, i64, i64, i64, i64, i64, f64, i64);

fn sums(r: SumRow) -> TokenSums {
    TokenSums {
        runs: r.0,
        input_tokens: r.1,
        output_tokens: r.2,
        cache_read_tokens: r.3,
        cache_write_tokens: r.4,
        reasoning_tokens: r.5,
        cost_usd: r.6,
        seconds: r.7,
    }
}

/// `GET /me/stats?tz=` – Projekte, Threads, Läufe, Tokens (genau aufgeschlüsselt), Kosten und Dauer.
#[get("/me/stats?<tz>")]
async fn stats(state: &State<AppState>, user: CurrentUser, tz: Option<i64>) -> AppResult<Json<Stats>> {
    let db = &state.db;
    let uid = user.id();
    let projects: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE user_id = ?").bind(uid).fetch_one(db).await?;
    let (threads, active, subagents): (i64, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE parent_id IS NULL), SUM(status IN ('working', 'waiting')), SUM(parent_id IS NOT NULL) FROM threads WHERE user_id = ?",
    )
    .bind(uid)
    .fetch_one(db)
    .await?;
    let t = now();
    let midnight = crate::agents::usage::start_of_day(t, tz);
    // Nur feste Teile, keine Eingaben des Benutzers im SQL-Text. Verbrauch aus `usage_log`: bleibt
    // beim Löschen von Threads erhalten
    let from = "FROM usage_log r WHERE r.user_id = ?";
    let mut windows = Vec::new();
    for (key, since) in [("today", midnight), ("7d", t - 7 * 86_400), ("30d", t - 30 * 86_400), ("all", 0)] {
        let row: SumRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SUMS} {from} AND r.started_at >= ?"))).bind(uid).bind(since).fetch_one(db).await?;
        windows.push(StatWindow { key: key.into(), sums: sums(row) });
    }
    let total = windows.last().map(|w| w.sums.clone()).unwrap_or_default();
    let runs_today = windows[0].sums.runs;
    let (done, error, stopped): (Option<i64>, Option<i64>, Option<i64>) =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT SUM(r.status = 'done'), SUM(r.status = 'error'), SUM(r.status = 'stopped') {from}"))).bind(uid).fetch_one(db).await?;
    let (user_msgs, agent_msgs, tools): (Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT SUM(m.role = 'user'), SUM(m.role = 'assistant'), SUM(m.role = 'tool') FROM messages m JOIN threads t ON t.id = m.thread_id WHERE t.user_id = ?",
    )
    .bind(uid)
    .fetch_one(db)
    .await?;
    let mut per_agent: Vec<(String, i64, i64, i64, i64, i64, i64, i64, f64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT r.agent_kind, COUNT(DISTINCT r.thread_key), {SUMS} {from} GROUP BY r.agent_kind ORDER BY COUNT(r.id) DESC"
    )))
    .bind(uid)
    .fetch_all(db)
    .await?;
    // Threads ohne Lauf zählen beim Agenten mit (Threads je Agent aus `threads`)
    let thread_counts: Vec<(String, i64)> = sqlx::query_as("SELECT agent_kind, COUNT(*) FROM threads WHERE user_id = ? GROUP BY agent_kind")
        .bind(uid)
        .fetch_all(db)
        .await?;
    for (k, n) in &thread_counts {
        match per_agent.iter_mut().find(|r| &r.0 == k) {
            Some(r) => r.1 = r.1.max(*n),
            None => per_agent.push((k.clone(), *n, 0, 0, 0, 0, 0, 0, 0.0, 0)),
        }
    }
    let per_model: Vec<(String, String, i64, i64, i64, i64, i64, i64, f64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT r.agent_kind, r.model, {SUMS} {from} GROUP BY r.agent_kind, r.model \
         ORDER BY SUM(r.input_tokens + r.output_tokens) DESC, COUNT(r.id) DESC LIMIT 30"
    )))
    .bind(uid)
    .fetch_all(db)
    .await?;
    Ok(Json(Stats {
        projects,
        threads,
        threads_active: active.unwrap_or(0),
        subagents: subagents.unwrap_or(0),
        runs: total.runs,
        runs_today,
        input_tokens: total.input_tokens,
        output_tokens: total.output_tokens,
        cost_usd: total.cost_usd,
        total,
        runs_done: done.unwrap_or(0),
        runs_error: error.unwrap_or(0),
        runs_stopped: stopped.unwrap_or(0),
        user_messages: user_msgs.unwrap_or(0),
        agent_messages: agent_msgs.unwrap_or(0),
        tool_calls: tools.unwrap_or(0),
        windows,
        agents: per_agent
            .into_iter()
            .filter_map(|(k, threads, a, b, c, d, e, f, g, h)| {
                AgentKind::parse(&k).map(|agent| AgentStat { agent, threads, runs: a, sums: sums((a, b, c, d, e, f, g, h)) })
            })
            .collect(),
        models: per_model
            .into_iter()
            .filter_map(|(k, model, a, b, c, d, e, f, g, h)| AgentKind::parse(&k).map(|agent| ModelStat { agent, model, sums: sums((a, b, c, d, e, f, g, h)) }))
            .collect(),
    }))
}

/// `GET /me/usage?range=24h|7d|30d|90d` – Nutzung im Zeitraum: Summen, je Agent und Verlauf
/// (24h stündlich, 7d in 6-Stunden-Abschnitten, 30d täglich, 90d in 3-Tages-Abschnitten).
#[get("/me/usage?<range>")]
async fn usage_report(state: &State<AppState>, user: CurrentUser, range: Option<&str>) -> AppResult<Json<UsageReport>> {
    let db = &state.db;
    let uid = user.id();
    let range = range.unwrap_or("24h");
    let (span, bucket) = match range {
        "24h" => (86_400, 3_600),
        "7d" => (7 * 86_400, 6 * 3_600),
        "30d" => (30 * 86_400, 86_400),
        "90d" => (90 * 86_400, 3 * 86_400),
        _ => return Err(AppError::bad("Zeitraum: 24h, 7d, 30d oder 90d.")),
    };
    let n = span / bucket;
    // Abschnitte an ganzen Stunden bzw. Tagen ausrichten, der letzte enthält „jetzt“
    let from = (now() / bucket + 1) * bucket - span;
    let base = "FROM usage_log r WHERE r.user_id = ? AND r.started_at >= ?";
    let total: SumRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SUMS} {base}"))).bind(uid).bind(from).fetch_one(db).await?;
    let sessions: i64 =
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(DISTINCT r.thread_key) {base}"))).bind(uid).bind(from).fetch_one(db).await?;
    let per_agent: Vec<(String, i64, i64, i64, i64, i64, i64, i64, f64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT r.agent_kind, COUNT(DISTINCT r.thread_key), {SUMS} {base} GROUP BY r.agent_kind \
         ORDER BY SUM(r.cost_usd) DESC, SUM(r.input_tokens + r.output_tokens) DESC"
    )))
    .bind(uid)
    .bind(from)
    .fetch_all(db)
    .await?;
    let points: Vec<(String, i64, f64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT r.agent_kind, (r.started_at - ?) / ?, COALESCE(SUM(r.cost_usd), 0.0), COALESCE(SUM(r.input_tokens + r.output_tokens), 0) \
         {base} GROUP BY r.agent_kind, (r.started_at - ?) / ?"
    )))
    .bind(from)
    .bind(bucket)
    .bind(uid)
    .bind(from)
    .bind(from)
    .bind(bucket)
    .fetch_all(db)
    .await?;
    let agents: Vec<AgentUsage> = per_agent
        .into_iter()
        .filter_map(|(k, sessions, a, b, c, d, e, f, g, h)| AgentKind::parse(&k).map(|agent| AgentUsage { agent, sessions, sums: sums((a, b, c, d, e, f, g, h)) }))
        .collect();
    let series = agents
        .iter()
        .map(|a| {
            let mut pts = vec![UsagePoint::default(); n as usize];
            for (k, i, cost, tokens) in &points {
                if AgentKind::parse(k) == Some(a.agent) && *i >= 0 && *i < n {
                    pts[*i as usize] = UsagePoint { cost_usd: *cost, tokens: *tokens };
                }
            }
            UsageSeries { agent: a.agent, points: pts }
        })
        .collect();
    Ok(Json(UsageReport { range: range.into(), from, bucket_seconds: bucket, sessions, total: sums(total), agents, series }))
}

/// `GET /threads/<id>/stats` – Tokens, Kosten und Laufzeit eines Threads, auch mit allen Subagenten.
#[get("/threads/<id>/stats")]
async fn thread_stats(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ThreadStats>> {
    let db = &state.db;
    let own: Option<i64> = sqlx::query_scalar("SELECT id FROM threads WHERE id = ? AND user_id = ?").bind(id).bind(user.id()).fetch_optional(db).await?;
    if own.is_none() {
        return Err(AppError::NotFound);
    }
    // Thread und alle Subagenten (rekursiv)
    let tree = "WITH RECURSIVE tree(id) AS (SELECT ? UNION ALL SELECT t.id FROM threads t JOIN tree ON t.parent_id = tree.id)";
    let own_row: SumRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SUMS} FROM runs r WHERE r.thread_id = ?"))).bind(id).fetch_one(db).await?;
    let all_row: SumRow =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{tree} SELECT {SUMS} FROM runs r WHERE r.thread_id IN (SELECT id FROM tree)"))).bind(id).fetch_one(db).await?;
    let subagents: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("{tree} SELECT COUNT(*) - 1 FROM tree"))).bind(id).fetch_one(db).await?;
    let (u, a, tools): (Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{tree} SELECT SUM(role = 'user'), SUM(role = 'assistant'), SUM(role = 'tool') FROM messages WHERE thread_id IN (SELECT id FROM tree)"
    )))
    .bind(id)
    .fetch_one(db)
    .await?;
    let per_model: Vec<(String, String, i64, i64, i64, i64, i64, i64, f64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{tree} SELECT r.agent_kind, r.model, {SUMS} FROM runs r WHERE r.thread_id IN (SELECT id FROM tree) \
         GROUP BY r.agent_kind, r.model ORDER BY SUM(r.input_tokens + r.output_tokens) DESC"
    )))
    .bind(id)
    .fetch_all(db)
    .await?;
    Ok(Json(ThreadStats {
        own: sums(own_row),
        with_subagents: sums(all_row),
        subagents: subagents.max(0),
        user_messages: u.unwrap_or(0),
        agent_messages: a.unwrap_or(0),
        tool_calls: tools.unwrap_or(0),
        models: per_model
            .into_iter()
            .filter_map(|(k, model, a, b, c, d, e, f, g, h)| AgentKind::parse(&k).map(|agent| ModelStat { agent, model, sums: sums((a, b, c, d, e, f, g, h)) }))
            .collect(),
    }))
}

// ------------------------------------------------------------------ Avatar

/// `If-None-Match` der Anfrage.
pub struct IfNoneMatch(Option<String>);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for IfNoneMatch {
    type Error = std::convert::Infallible;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        Outcome::Success(IfNoneMatch(req.headers().get_one("If-None-Match").map(str::to_string)))
    }
}

pub struct Avatar {
    body: Vec<u8>,
    content_type: ContentType,
    etag: String,
    not_modified: bool,
    /// Adresse mit Version (`?v=`): ändert sich nie
    immutable: bool,
}

impl<'r> Responder<'r, 'static> for Avatar {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        let mut res = Response::build();
        // Ohne Version jedes Mal nachfragen (ETag → 304), damit ein neues Bild sofort erscheint
        let cache = if self.immutable { "public, max-age=31536000, immutable" } else { "no-cache" };
        res.header(Header::new("Cache-Control", cache)).header(Header::new("ETag", self.etag.clone()));
        if self.not_modified {
            res.status(Status::NotModified);
        } else {
            res.header(self.content_type).sized_body(self.body.len(), Cursor::new(self.body));
        }
        res.ok()
    }
}

/// Ablage des eigenen Profilbilds.
fn avatar_path(state: &AppState, user_id: i64) -> std::path::PathBuf {
    state.config.data_dir.join("avatars").join(user_id.to_string())
}

/// Bildformat an den ersten Bytes erkennen (nur Rasterbilder – kein SVG mit Skripten).
fn image_type(b: &[u8]) -> Option<ContentType> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ContentType::PNG)
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ContentType::JPEG)
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some(ContentType::new("image", "webp"))
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some(ContentType::GIF)
    } else {
        None
    }
}

const MAX_AVATAR: usize = 2 * 1024 * 1024;

/// `GET /users/<name>/avatar` – eigenes Profilbild oder das Identicon als SVG (`?s=` wird
/// ignoriert, die Oberfläche skaliert). Mit `?v=` (Version) lange zwischengespeichert.
#[get("/users/<name>/avatar?<s>&<v>")]
async fn avatar(state: &State<AppState>, name: &str, s: Option<u32>, v: Option<i64>, inm: IfNoneMatch) -> Avatar {
    let _ = s;
    let lower = name.to_lowercase();
    let matches = |etag: &str| inm.0.as_deref().is_some_and(|x| x.split(',').any(|t| t.trim().trim_start_matches("W/") == etag));
    let custom: Option<(i64, i64)> =
        sqlx::query_as("SELECT id, avatar_at FROM users WHERE lower_name = ?").bind(&lower).fetch_optional(&state.db).await.ok().flatten();
    if let Some((id, at)) = custom.filter(|(_, at)| *at > 0) {
        if let Ok(body) = tokio::fs::read(avatar_path(state, id)).await {
            if let Some(content_type) = image_type(&body) {
                let etag = format!("\"a{id}-{at}\"");
                let not_modified = matches(&etag);
                return Avatar { body, content_type, not_modified, etag, immutable: v == Some(at) };
            }
        }
    }
    let body = identicon(&lower).into_bytes();
    let etag = format!("\"{}\"", &hex::encode(Sha256::digest(&body))[..20]);
    let not_modified = matches(&etag);
    Avatar { body, content_type: ContentType::SVG, etag, not_modified, immutable: false }
}

/// `PUT /me/avatar` – eigenes Profilbild (die Oberfläche schickt ein quadratisches PNG mit 256 px).
#[put("/me/avatar", data = "<body>")]
async fn put_avatar(state: &State<AppState>, user: CurrentUser, body: Json<AvatarUpload>) -> AppResult<Json<Me>> {
    use base64::Engine;
    let raw = body.image.trim();
    let data = raw.split_once("base64,").map(|(_, d)| d).unwrap_or(raw);
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|_| AppError::bad("Das Bild ist ungültig."))?;
    if bytes.len() > MAX_AVATAR {
        return Err(AppError::bad("Das Bild ist zu gross (höchstens 2 MB)."));
    }
    if image_type(&bytes).is_none() {
        return Err(AppError::bad("Bitte ein Bild als PNG, JPEG, WebP oder GIF wählen."));
    }
    let path = avatar_path(state, user.id());
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, &bytes).await?;
    tokio::fs::rename(&tmp, &path).await?;
    // Neue Version (immer grösser als die alte → neue Adresse, kein altes Bild aus dem Cache)
    let ts = now();
    sqlx::query("UPDATE users SET avatar_at = MAX(?, avatar_at + 1), updated_at = ? WHERE id = ?").bind(ts).bind(ts).bind(user.id()).execute(&state.db).await?;
    let u = db::user_by_id(&state.db, user.id()).await?.ok_or(AppError::NotFound)?;
    Ok(Json(me_json(&state.db, &u).await?))
}

/// `DELETE /me/avatar` – eigenes Profilbild entfernen (wieder das Identicon).
#[delete("/me/avatar")]
async fn delete_avatar(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<Me>> {
    let _ = tokio::fs::remove_file(avatar_path(state, user.id())).await;
    sqlx::query("UPDATE users SET avatar_at = 0, updated_at = ? WHERE id = ?").bind(now()).bind(user.id()).execute(&state.db).await?;
    let u = db::user_by_id(&state.db, user.id()).await?.ok_or(AppError::NotFound)?;
    Ok(Json(me_json(&state.db, &u).await?))
}

/// Identicon wie bei GitHub/Forgejo: 5×5 Felder, links-rechts gespiegelt, Farbe aus dem Hash.
pub fn identicon(seed: &str) -> String {
    let hash = Sha256::digest(seed.as_bytes());
    let hue = u16::from_be_bytes([hash[0], hash[1]]) as u32 % 360;
    let sat = 45 + hash[2] as u32 % 20;
    let light = 52 + hash[3] as u32 % 12;
    let color = format!("hsl({hue},{sat}%,{light}%)");
    let mut cells = String::new();
    let mut filled = 0;
    for row in 0..5u32 {
        for col in 0..3u32 {
            if hash[4 + (row * 3 + col) as usize] & 1 == 1 {
                filled += 1;
                let xs = if col == 2 { vec![2] } else { vec![col, 4 - col] };
                for x in xs {
                    cells.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"14\" height=\"14\"/>", 7 + x * 14, 7 + row * 14));
                }
            }
        }
    }
    if filled == 0 {
        cells.push_str("<rect x=\"35\" y=\"35\" width=\"14\" height=\"14\"/>");
    }
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 84 84\" width=\"84\" height=\"84\" shape-rendering=\"crispEdges\">\
         <rect width=\"84\" height=\"84\" fill=\"#f0f0f0\"/><g fill=\"{color}\">{cells}</g></svg>"
    )
}

// ------------------------------------------------------------------ Verwaltung

async fn user_info(db: &SqlitePool, u: &User) -> AppResult<UserInfo> {
    let (projects, threads, connected): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM projects WHERE user_id = ?), (SELECT COUNT(*) FROM threads WHERE user_id = ? AND parent_id IS NULL), \
         (SELECT COUNT(*) FROM nrzgit_connections WHERE user_id = ? AND verified_at IS NOT NULL)",
    )
    .bind(u.id)
    .bind(u.id)
    .bind(u.id)
    .fetch_one(db)
    .await?;
    Ok(UserInfo {
        id: u.id,
        username: u.username.clone(),
        full_name: u.full_name.clone(),
        email: u.email.clone(),
        avatar_url: db::avatar_url(&u.username, u.avatar_at),
        is_admin: u.is_admin,
        is_active: u.is_active,
        projects,
        threads,
        nrzgit_connected: connected > 0,
        created_at: u.created_at,
        last_login_at: u.last_login_at,
    })
}

/// `GET /users` – alle Benutzer (Administratoren).
#[get("/users")]
async fn list_users(state: &State<AppState>, _admin: AdminUser) -> AppResult<Json<Vec<UserInfo>>> {
    let users: Vec<User> = sqlx::query_as("SELECT * FROM users ORDER BY lower_name").fetch_all(&state.db).await?;
    let mut out = Vec::with_capacity(users.len());
    for u in &users {
        out.push(user_info(&state.db, u).await?);
    }
    Ok(Json(out))
}

/// `POST /users` – Benutzer anlegen (Administratoren).
#[post("/users", data = "<body>")]
async fn create_user_route(state: &State<AppState>, _admin: AdminUser, body: Json<CreateUser>) -> AppResult<(Status, Json<UserInfo>)> {
    let u = create_account(&state.db, &body.username, &body.email, &body.password, &body.full_name, Some(body.is_admin)).await?;
    Ok((Status::Created, Json(user_info(&state.db, &u).await?)))
}

async fn admin_count(db: &SqlitePool) -> AppResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin = 1 AND is_active = 1").fetch_one(db).await?)
}

/// `PATCH /users/<id>` – Administrator, gesperrt, Passwort, Name, E-Mail ändern.
#[patch("/users/<id>", data = "<body>")]
async fn edit_user(state: &State<AppState>, admin: AdminUser, id: i64, body: Json<EditUser>) -> AppResult<Json<UserInfo>> {
    let db = &state.db;
    let u = db::user_by_id(db, id).await?.ok_or(AppError::NotFound)?;
    let removes_admin = (body.is_admin == Some(false) && u.is_admin) || (body.is_active == Some(false) && u.is_admin && u.is_active);
    if removes_admin && admin_count(db).await? <= 1 {
        return Err(AppError::conflict("Der letzte Administrator kann nicht herabgestuft oder gesperrt werden."));
    }
    if id == admin.0.id && (body.is_active == Some(false) || body.is_admin == Some(false)) {
        return Err(AppError::conflict("Das eigene Konto kann hier nicht gesperrt oder herabgestuft werden."));
    }
    if let Some(a) = body.is_admin {
        sqlx::query("UPDATE users SET is_admin = ? WHERE id = ?").bind(a).bind(id).execute(db).await?;
    }
    if let Some(active) = body.is_active {
        sqlx::query("UPDATE users SET is_active = ? WHERE id = ?").bind(active).bind(id).execute(db).await?;
        if !active {
            session::destroy_all(db, id).await?;
            stop_user_threads(state, id).await?;
        }
    }
    if let Some(pw) = &body.password {
        password::check_strength(pw)?;
        let hash = password::hash(pw.clone()).await?;
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?").bind(hash).bind(id).execute(db).await?;
        session::destroy_all(db, id).await?;
    }
    if let Some(name) = &body.full_name {
        let name = check_len(name, 100, "Voller Name")?;
        sqlx::query("UPDATE users SET full_name = ? WHERE id = ?").bind(name).bind(id).execute(db).await?;
    }
    if let Some(email) = &body.email {
        let email = check_email(email)?;
        sqlx::query("UPDATE users SET email = ? WHERE id = ?")
            .bind(email)
            .bind(id)
            .execute(db)
            .await
            .map_err(|e| AppError::unique(e, "Diese E-Mail-Adresse wird schon verwendet."))?;
    }
    sqlx::query("UPDATE users SET updated_at = ? WHERE id = ?").bind(now()).bind(id).execute(db).await?;
    let u = db::user_by_id(db, id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(user_info(db, &u).await?))
}

/// Alle laufenden Agenten eines Benutzers anhalten.
async fn stop_user_threads(state: &AppState, user_id: i64) -> AppResult<()> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM threads WHERE user_id = ? AND parent_id IS NULL")
        .bind(user_id)
        .fetch_all(&state.db)
        .await?;
    for id in ids {
        if let Err(e) = crate::agents::stop_thread(state, id).await {
            tracing::warn!("Thread {id} anhalten: {e}");
        }
    }
    Ok(())
}

/// `DELETE /users/<id>` – Benutzer mit Projekten, Threads und Arbeitsordnern löschen.
#[delete("/users/<id>")]
async fn delete_user(state: &State<AppState>, admin: AdminUser, id: i64) -> AppResult<Status> {
    let db = &state.db;
    let u = db::user_by_id(db, id).await?.ok_or(AppError::NotFound)?;
    if id == admin.0.id {
        return Err(AppError::conflict("Das eigene Konto kann nicht gelöscht werden."));
    }
    if u.is_admin && admin_count(db).await? <= 1 {
        return Err(AppError::conflict("Der letzte Administrator kann nicht gelöscht werden."));
    }
    stop_user_threads(state, id).await?;
    session::destroy_all(db, id).await?;
    let projects: Vec<i64> = sqlx::query_scalar("SELECT id FROM projects WHERE user_id = ?").bind(id).fetch_all(db).await?;
    let threads: Vec<i64> = sqlx::query_scalar("SELECT id FROM threads WHERE user_id = ?").bind(id).fetch_all(db).await?;
    sqlx::query("DELETE FROM users WHERE id = ?").bind(id).execute(db).await?;
    let mut dirs: Vec<std::path::PathBuf> = projects.iter().map(|p| model::project_dir(state, *p)).collect();
    dirs.extend(threads.iter().map(|t| model::scratch_dir(state, *t)));
    dirs.push(super::gitops::docker_dir(state, id));
    let _ = tokio::fs::remove_file(avatar_path(state, id)).await;
    tokio::task::spawn_blocking(move || {
        for d in dirs {
            if d.exists() {
                let _ = std::fs::remove_dir_all(&d);
            }
        }
    });
    // Offene Tabs des gelöschten Benutzers aufräumen
    for p in projects {
        crate::hub::publish(state, id, HubEvent::ProjectDeleted { id: p });
    }
    Ok(Status::NoContent)
}

#[cfg(test)]
mod tests {
    #[test]
    fn identicon_is_stable() {
        let a = super::identicon("anna");
        assert_eq!(a, super::identicon("anna"));
        assert_ne!(a, super::identicon("max"));
        assert!(a.starts_with("<svg"));
    }
}
