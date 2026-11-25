//! Agent-Konten: beliebig viele je Agent-Art (z. B. „Claude Max privat“ und „Claude Team“),
//! jedes mit eigenem Home-Verzeichnis `<data>/accounts/<id>/home`. Jeder Prozess des Kontos
//! läuft mit `HOME`, `XDG_*` und den CLI-eigenen Variablen (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
//! `GEMINI_CLI_HOME` …) dort – so teilen sich zwei Abos desselben Anbieters nie eine Anmeldung.
//!
//! Anmeldearten: `login` (Anmeldung im CLI über ein Terminal, gespeichert im Home), `api_key`
//! und `oauth_token` (als Umgebungsvariable). Geheimnisse werden nie ausgeliefert.

use super::kinds::spec;
use super::{terminals, tools};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::{AccountUpdate, AgentAccount, LoginSession, NewAccount};
use nrzx_shared::AgentKind;
use rocket::serde::json::Json;
use rocket::{delete, get, patch, post, routes, Route, State};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

pub fn routes() -> Vec<Route> {
    routes![list, create, update, remove, check, login]
}

/// Zeile aus `agent_accounts`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountRow {
    pub id: i64,
    pub user_id: i64,
    pub kind: String,
    pub label: String,
    pub auth_kind: String,
    pub secret: String,
    pub secret_env: String,
    pub default_model: String,
    pub default_effort: String,
    pub status: String,
    pub status_message: String,
    pub is_default: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: Option<i64>,
}

impl AccountRow {
    pub fn kind(&self) -> AgentKind {
        AgentKind::parse(&self.kind).unwrap_or_default()
    }
}

/// `sk-ant-…cdef`
pub fn secret_hint(secret: &str) -> String {
    let s = secret.trim();
    if s.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 10 {
        return "…".into();
    }
    let head: String = match s.find('-') {
        Some(i) if i <= 6 => s[..=i].to_string(),
        _ => chars.iter().take(3).collect(),
    };
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// Home-Verzeichnis eines Kontos.
pub fn home(state: &AppState, id: i64) -> PathBuf {
    state.config.accounts_dir().join(id.to_string()).join("home")
}

fn to_json(state: &AppState, r: &AccountRow, threads: i64) -> AgentAccount {
    AgentAccount {
        id: r.id,
        kind: r.kind(),
        label: r.label.clone(),
        auth_kind: r.auth_kind.clone(),
        secret_env: effective_env(r),
        secret_hint: secret_hint(&r.secret),
        default_model: r.default_model.clone(),
        default_effort: r.default_effort.clone(),
        status: r.status.clone(),
        status_message: r.status_message.clone(),
        is_default: r.is_default,
        home: home(state, r.id).to_string_lossy().into_owned(),
        threads,
        created_at: r.created_at,
        updated_at: r.updated_at,
        last_used_at: r.last_used_at,
    }
}

/// Umgebungsvariable des Schlüssels (gewählt oder Vorgabe der Agent-Art).
fn effective_env(r: &AccountRow) -> String {
    if r.auth_kind == "login" {
        return String::new();
    }
    let s = spec(r.kind());
    let allowed = s.auth(&r.auth_kind).map(|a| a.env_vars).unwrap_or(&[]);
    if !r.secret_env.is_empty() && allowed.contains(&r.secret_env.as_str()) {
        r.secret_env.clone()
    } else {
        allowed.first().map(|s| s.to_string()).unwrap_or_default()
    }
}

/// Umgebung für Prozesse des Kontos: eigenes Home, XDG-Ordner, CLI-eigene Variablen und
/// der Schlüssel bzw. das Token.
pub fn env_for(state: &AppState, r: &AccountRow) -> Vec<(String, String)> {
    let home = home(state, r.id);
    let p = |rel: &str| home.join(rel).to_string_lossy().into_owned();
    let mut env = crate::platform::home_vars(&home);
    env.extend([
        ("XDG_CONFIG_HOME".into(), p(".config")),
        ("XDG_DATA_HOME".into(), p(".local/share")),
        ("XDG_STATE_HOME".into(), p(".local/state")),
        ("XDG_CACHE_HOME".into(), p(".cache")),
        ("CLAUDE_CONFIG_DIR".into(), p(".claude")),
        ("CODEX_HOME".into(), p(".codex")),
        ("GEMINI_CLI_HOME".into(), home.to_string_lossy().into_owned()),
        // Keine Selbst-Aktualisierung: Versionen verwaltet nrzX (Einstellungen → Werkzeuge)
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
        ("OPENCODE_DISABLE_AUTOUPDATE".into(), "1".into()),
        ("KILO_DISABLE_AUTOUPDATE".into(), "1".into()),
    ]);
    if r.kind() == AgentKind::Grok {
        // Versionen verwaltet nrzX. Das offizielle CLI sucht den Schlüssel als XAI_API_KEY.
        env.push(("GROK_DISABLE_AUTOUPDATER".into(), "1".into()));
    }
    // macOS: Im Konto-Home gibt es keinen Schlüsselbund. Schreibversuche dorthin scheitern still,
    // statt dass macOS „Schlüsselbund nicht gefunden“ zeigt (`security`-Ersatz vor dem System)
    if let Some(dir) = crate::platform::keychain_shim_dir(&state.config.data_dir) {
        env.push(("PATH".into(), super::env::path_var_with(state, &[dir])));
        // Gemini CLI: Anmeldungen in Dateien im Home statt Probe-Eintrag im Schlüsselbund
        env.push(("GEMINI_FORCE_FILE_STORAGE".into(), "true".into()));
    }
    let secret = r.secret.trim();
    if r.auth_kind != "login" && !secret.is_empty() {
        let var = effective_env(r);
        if !var.is_empty() {
            env.push((var.clone(), secret.to_string()));
        }
        // Zusätzliche Namen, unter denen die CLIs den Schlüssel ebenfalls suchen
        match (r.kind(), var.as_str()) {
            (AgentKind::Codex, "OPENAI_API_KEY") => env.push(("CODEX_API_KEY".into(), secret.to_string())),
            (AgentKind::Grok, "GROK_API_KEY") => env.push(("XAI_API_KEY".into(), secret.to_string())),
            (AgentKind::Grok, "XAI_API_KEY") => env.push(("GROK_API_KEY".into(), secret.to_string())),
            (AgentKind::Gemini, "GEMINI_API_KEY") => env.push(("GOOGLE_API_KEY".into(), secret.to_string())),
            _ => {}
        }
    }
    env
}

/// Dateien mit den eigentlichen Anmeldedaten je CLI (Tokens, die sich erneuern).
const CRED_FILES: &[&str] = &[
    ".claude/.credentials.json",
    ".codex/auth.json",
    ".gemini/oauth_creds.json",
    ".local/share/opencode/auth.json",
    ".kilocode/cli/config.json",
    ".grok/user-settings.json",
    ".grok/auth.json",
];

/// Mit nrzCode verknüpfte Anmeldung abgleichen: je Datei gewinnt die neuere – erneuert eine der
/// beiden Apps ihr Token (die alte Erneuerung gilt dann nicht mehr), bekommt es die andere. Vor
/// Läufen, Terminals und Abfragen der Limits sowie alle 30 Sekunden ([`spawn_login_sync`]).
pub async fn sync_login(state: &AppState, id: i64) -> usize {
    let linked: Option<String> = sqlx::query_scalar("SELECT linked_home FROM agent_accounts WHERE id = ?").bind(id).fetch_optional(&state.db).await.ok().flatten();
    let Some(src) = linked.filter(|s| !s.trim().is_empty()) else { return 0 };
    let ours = home(state, id);
    let theirs = PathBuf::from(src);
    let n = tokio::task::spawn_blocking(move || sync_files(&theirs, &ours)).await.unwrap_or(0);
    if n > 0 {
        tracing::info!("Anmeldung von Konto {id} mit nrzCode abgeglichen ({n} Dateien)");
    }
    n
}

/// Alle verknüpften Konten regelmässig abgleichen.
pub fn spawn_login_sync(state: AppState) {
    tokio::spawn(async move {
        loop {
            let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM agent_accounts WHERE linked_home != ''").fetch_all(&state.db).await.unwrap_or_default();
            for id in ids {
                sync_login(&state, id).await;
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

/// Anmeldedateien zweier Homes abgleichen (die neuere gewinnt; nur, was in `a` schon existiert,
/// kommt nach `b` – in `a` legt der Abgleich nichts Neues an). Anzahl kopierter Dateien.
pub fn sync_files(a: &Path, b: &Path) -> usize {
    let mut n = 0;
    for rel in CRED_FILES {
        let (pa, pb) = (a.join(rel), b.join(rel));
        let ma = std::fs::metadata(&pa).and_then(|m| m.modified()).ok();
        let mb = std::fs::metadata(&pb).and_then(|m| m.modified()).ok();
        let (from, to) = match (ma, mb) {
            (Some(x), Some(y)) if x > y => (&pa, &pb),
            (Some(x), Some(y)) if y > x => (&pb, &pa),
            (Some(_), None) => (&pa, &pb),
            _ => continue,
        };
        if std::fs::read(from).ok() == std::fs::read(to).ok() {
            continue;
        }
        if copy_secret(from, to) {
            n += 1;
        }
    }
    n
}

/// Datei mit Anmeldedaten kopieren (über eine Zwischendatei, nur für den Besitzer lesbar, mit der
/// Änderungszeit der Quelle – beide gelten danach als gleich alt).
fn copy_secret(from: &Path, to: &Path) -> bool {
    if let Some(p) = to.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let tmp = to.with_extension("nrzx-sync");
    if std::fs::copy(from, &tmp).is_err() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    if let Ok(t) = std::fs::metadata(from).and_then(|m| m.modified()) {
        if let Ok(f) = std::fs::File::options().write(true).open(&tmp) {
            let _ = f.set_modified(t);
        }
    }
    if std::fs::rename(&tmp, to).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

/// Claude Code im Terminal ohne Rückfragen beim Start: die Ordner `dirs` (und die Arbeitsordner
/// von nrzX) gelten als vertraut („Yes, I trust this folder“), die Warnung zum Bypass-Modus ist
/// bestätigt, und `claude` startet im Bypass-Modus. Läufe im Chat geben ihren Modus ausdrücklich
/// mit (`--permission-mode`), das CLI eines Threads ebenso – die Vorgabe gilt nur ohne Angabe.
/// Kaputte Dateien bleiben unberührt.
pub async fn prepare_claude(state: &AppState, id: i64, dirs: &[&Path]) {
    let home = home(state, id);
    let dir = home.join(".claude");
    let _ = tokio::fs::create_dir_all(&dir).await;
    let mut trusted: Vec<String> = dirs.iter().map(|d| d.to_string_lossy().into_owned()).collect();
    trusted.push(state.config.workspaces_dir().to_string_lossy().into_owned());
    trusted.push(state.config.scratch_dir().to_string_lossy().into_owned());
    // CLAUDE_CONFIG_DIR/.claude.json; die alte Lage im Home nur, wenn es sie gibt
    let mut configs = vec![dir.join(".claude.json")];
    if home.join(".claude.json").is_file() {
        configs.push(home.join(".claude.json"));
    }
    for path in configs {
        merge_json(&path, |o| {
            o.insert("bypassPermissionsModeAccepted".into(), true.into());
            if let Some(projects) = o.entry("projects").or_insert_with(|| serde_json::json!({})).as_object_mut() {
                for d in &trusted {
                    if let Some(p) = projects.entry(d.clone()).or_insert_with(|| serde_json::json!({})).as_object_mut() {
                        p.insert("hasTrustDialogAccepted".into(), true.into());
                        p.insert("hasCompletedProjectOnboarding".into(), true.into());
                    }
                }
            }
        })
        .await;
    }
    merge_json(&dir.join("settings.json"), |o| {
        o.insert("skipDangerousModePermissionPrompt".into(), true.into());
        if let Some(p) = o.entry("permissions").or_insert_with(|| serde_json::json!({})).as_object_mut() {
            p.insert("defaultMode".into(), "bypassPermissions".into());
        }
    })
    .await;
}

/// JSON-Objekt in einer Datei ändern (fehlt sie: neu). Nur schreiben, wenn sich etwas ändert –
/// über eine Zwischendatei, mit den Rechten der alten (sonst nur für den Besitzer lesbar).
async fn merge_json(path: &Path, f: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>)) {
    let mut value = match tokio::fs::read(path).await {
        Ok(b) if b.iter().all(u8::is_ascii_whitespace) => serde_json::json!({}),
        Ok(b) => match serde_json::from_slice::<serde_json::Value>(&b) {
            Ok(v) if v.is_object() => v,
            _ => return,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(_) => return,
    };
    let before = value.clone();
    if let Some(o) = value.as_object_mut() {
        f(o);
    }
    if value == before {
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(&value) else { return };
    let tmp = path.with_extension("nrzx-tmp");
    if tokio::fs::write(&tmp, bytes).await.is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600);
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    }
    if tokio::fs::rename(&tmp, path).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
}

/// Home mit den üblichen Unterordnern anlegen.
pub async fn ensure_home(state: &AppState, id: i64) -> AppResult<PathBuf> {
    let home = home(state, id);
    for sub in [".config", ".local/share", ".local/state", ".cache", ".claude", ".codex", ".gemini", "AppData/Roaming", "AppData/Local"] {
        tokio::fs::create_dir_all(home.join(sub)).await?;
    }
    Ok(home)
}

const SELECT_COUNT: &str = "SELECT a.*, (SELECT COUNT(*) FROM threads t WHERE t.account_id = a.id) AS threads FROM agent_accounts a";

#[derive(sqlx::FromRow)]
struct RowWithCount {
    #[sqlx(flatten)]
    row: AccountRow,
    threads: i64,
}

/// Alle Konten eines Benutzers (Standardkonto zuerst, dann nach Name).
pub async fn list_for(state: &AppState, user_id: i64) -> AppResult<Vec<AgentAccount>> {
    let rows: Vec<RowWithCount> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_COUNT} WHERE a.user_id = ? ORDER BY a.kind, a.is_default DESC, a.label COLLATE NOCASE"
    )))
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.iter().map(|r| to_json(state, &r.row, r.threads)).collect())
}

/// Konto des Benutzers laden (sonst 404).
pub async fn get_row(state: &AppState, user_id: i64, id: i64) -> AppResult<AccountRow> {
    sqlx::query_as("SELECT * FROM agent_accounts WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)
}

/// Standardkonto einer Art (oder das älteste).
pub async fn default_for(state: &AppState, user_id: i64, kind: AgentKind) -> AppResult<Option<AccountRow>> {
    Ok(sqlx::query_as("SELECT * FROM agent_accounts WHERE user_id = ? AND kind = ? ORDER BY is_default DESC, id LIMIT 1")
        .bind(user_id)
        .bind(kind.key())
        .fetch_optional(&state.db)
        .await?)
}

async fn json_of(state: &AppState, id: i64) -> AppResult<AgentAccount> {
    let r: RowWithCount = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT_COUNT} WHERE a.id = ?")))
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(to_json(state, &r.row, r.threads))
}

/// `GET /accounts?kind=`
#[get("/accounts?<kind>")]
async fn list(state: &State<AppState>, user: CurrentUser, kind: Option<&str>) -> AppResult<Json<Vec<AgentAccount>>> {
    let all = list_for(state, user.id()).await?;
    Ok(Json(match kind.and_then(AgentKind::parse) {
        Some(k) => all.into_iter().filter(|a| a.kind == k).collect(),
        None => all,
    }))
}

fn check_auth_kind(kind: AgentKind, auth_kind: &str) -> AppResult<()> {
    if spec(kind).auth(auth_kind).is_none() {
        return Err(AppError::bad(format!("{} unterstützt diese Anmeldeart nicht.", kind.name())));
    }
    Ok(())
}

fn check_label(label: &str) -> AppResult<String> {
    let l = label.trim();
    if l.is_empty() || l.chars().count() > 80 {
        return Err(AppError::bad("Bitte einen Namen mit höchstens 80 Zeichen angeben."));
    }
    Ok(l.to_string())
}

fn check_effort(kind: AgentKind, effort: &str) -> AppResult<String> {
    if effort.is_empty() || spec(kind).effort(effort).is_some() {
        Ok(effort.to_string())
    } else {
        Err(AppError::bad("Unbekannte Denk-Stufe."))
    }
}

async fn set_default(state: &AppState, user_id: i64, kind: &str, id: i64) -> AppResult<()> {
    sqlx::query("UPDATE agent_accounts SET is_default = (id = ?) WHERE user_id = ? AND kind = ?")
        .bind(id)
        .bind(user_id)
        .bind(kind)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// `POST /accounts` – Konto anlegen (Home wird angelegt, Status geprüft).
#[post("/accounts", data = "<body>")]
async fn create(state: &State<AppState>, user: CurrentUser, body: Json<NewAccount>) -> AppResult<Json<AgentAccount>> {
    let b = body.into_inner();
    let label = check_label(&b.label)?;
    let auth_kind = if b.auth_kind.is_empty() { spec(b.kind).auth[0].key.to_string() } else { b.auth_kind.clone() };
    check_auth_kind(b.kind, &auth_kind)?;
    let effort = check_effort(b.kind, &b.default_effort)?;
    if auth_kind != "login" && b.secret.trim().is_empty() {
        return Err(AppError::bad("Bitte den Schlüssel bzw. das Token eintragen."));
    }
    let ts = now();
    let has_any: Option<(i64,)> = sqlx::query_as("SELECT id FROM agent_accounts WHERE user_id = ? AND kind = ? LIMIT 1")
        .bind(user.id())
        .bind(b.kind.key())
        .fetch_optional(&state.db)
        .await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO agent_accounts (user_id, kind, label, auth_kind, secret, secret_env, default_model, default_effort, is_default, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?) RETURNING id",
    )
    .bind(user.id())
    .bind(b.kind.key())
    .bind(&label)
    .bind(&auth_kind)
    .bind(b.secret.trim())
    .bind(b.secret_env.trim())
    .bind(b.default_model.trim())
    .bind(&effort)
    .bind(ts)
    .bind(ts)
    .fetch_one(&state.db)
    .await?;
    if b.is_default || has_any.is_none() {
        set_default(state, user.id(), b.kind.key(), id).await?;
    }
    ensure_home(state, id).await?;
    let row = get_row(state, user.id(), id).await?;
    refresh_status(state, &row).await?;
    Ok(Json(json_of(state, id).await?))
}

/// `PATCH /accounts/<id>`
#[patch("/accounts/<id>", data = "<body>")]
async fn update(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<AccountUpdate>) -> AppResult<Json<AgentAccount>> {
    let mut r = get_row(state, user.id(), id).await?;
    let b = body.into_inner();
    let mut recheck = false;
    if let Some(l) = &b.label {
        r.label = check_label(l)?;
    }
    if let Some(a) = &b.auth_kind {
        check_auth_kind(r.kind(), a)?;
        recheck |= *a != r.auth_kind;
        r.auth_kind = a.clone();
    }
    if let Some(s) = &b.secret {
        r.secret = s.trim().to_string();
        recheck = true;
    }
    if let Some(e) = &b.secret_env {
        r.secret_env = e.trim().to_string();
        recheck = true;
    }
    if let Some(m) = &b.default_model {
        r.default_model = m.trim().to_string();
    }
    if let Some(e) = &b.default_effort {
        r.default_effort = check_effort(r.kind(), e)?;
    }
    sqlx::query(
        "UPDATE agent_accounts SET label = ?, auth_kind = ?, secret = ?, secret_env = ?, default_model = ?, default_effort = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&r.label)
    .bind(&r.auth_kind)
    .bind(&r.secret)
    .bind(&r.secret_env)
    .bind(&r.default_model)
    .bind(&r.default_effort)
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    if b.is_default == Some(true) {
        set_default(state, user.id(), &r.kind, id).await?;
    }
    if recheck {
        refresh_status(state, &r).await?;
    }
    Ok(Json(json_of(state, id).await?))
}

/// `DELETE /accounts/<id>` – Konto und sein Home löschen (nicht während ein Lauf es nutzt).
#[delete("/accounts/<id>")]
async fn remove(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<rocket::http::Status> {
    let r = get_row(state, user.id(), id).await?;
    let busy: Option<(i64,)> = sqlx::query_as("SELECT id FROM runs WHERE account_id = ? AND status IN ('queued', 'running') LIMIT 1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    if busy.is_some() {
        return Err(AppError::conflict("Ein Agent arbeitet gerade mit diesem Konto."));
    }
    terminals::close_for_account(state, id);
    sqlx::query("DELETE FROM agent_accounts WHERE id = ?").bind(id).execute(&state.db).await?;
    if r.is_default {
        if let Some(next) = default_for(state, user.id(), r.kind()).await? {
            set_default(state, user.id(), &r.kind, next.id).await?;
        }
    }
    let dir = state.config.accounts_dir().join(id.to_string());
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir).await?;
    }
    Ok(rocket::http::Status::NoContent)
}

/// `POST /accounts/<id>/check` – Status prüfen (ohne kostenpflichtige Aufrufe).
#[post("/accounts/<id>/check")]
async fn check(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<AgentAccount>> {
    let r = get_row(state, user.id(), id).await?;
    refresh_status(state, &r).await?;
    Ok(Json(json_of(state, id).await?))
}

/// `POST /accounts/<id>/login` – Terminal im Konto mit dem Anmeldebefehl öffnen.
#[post("/accounts/<id>/login")]
async fn login(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<LoginSession>> {
    let r = get_row(state, user.id(), id).await?;
    let s = spec(r.kind());
    if s.login_command.is_empty() {
        return Err(AppError::bad(format!("{} kennt keine Anmeldung im CLI – bitte einen API-Schlüssel eintragen.", r.kind().name())));
    }
    let Some((bin, _)) = tools::find_agent(state, r.kind()) else {
        return Err(AppError::bad(format!("{} ist nicht installiert (Einstellungen → Werkzeuge).", r.kind().name())));
    };
    if r.kind() == AgentKind::Grok && tools::legacy_grok(&bin) {
        return Err(AppError::bad(
            "Installiert ist das alte Grok-CLI (@vibe-kit/grok-cli). Es kennt keine Anmeldung. Unter Einstellungen → Werkzeuge Grok aktualisieren.",
        ));
    }
    ensure_home(state, id).await?;
    let mut extra = Vec::new();
    if r.kind() == AgentKind::Gemini {
        // Google-Anmeldung ohne Browser auf dem Server: Link anzeigen, Code einfügen
        extra.push(("NO_BROWSER".to_string(), "true".to_string()));
    }
    let info = terminals::open_for_account(state, user.id(), &r, s.login_command, extra).await?;
    Ok(Json(LoginSession { terminal: info, command: s.login_command.into(), help: s.login_help.into() }))
}

/// Status eines Kontos prüfen und speichern: "ok" | "login_required" | "error".
pub async fn refresh_status(state: &AppState, r: &AccountRow) -> AppResult<(String, String)> {
    let (status, message) = probe(state, r).await;
    sqlx::query("UPDATE agent_accounts SET status = ?, status_message = ?, updated_at = ? WHERE id = ?")
        .bind(&status)
        .bind(&message)
        .bind(now())
        .bind(r.id)
        .execute(&state.db)
        .await?;
    Ok((status, message))
}

fn ok(msg: &str) -> (String, String) {
    ("ok".into(), msg.into())
}

fn needs_login(msg: &str) -> (String, String) {
    ("login_required".into(), msg.into())
}

/// Prüft die Anmeldung: Dateien im Home oder ein lokaler Statusaufruf des CLIs.
async fn probe(state: &AppState, r: &AccountRow) -> (String, String) {
    let kind = r.kind();
    let home = home(state, r.id);
    if r.auth_kind != "login" {
        if r.secret.trim().is_empty() {
            return needs_login("Kein Schlüssel hinterlegt.");
        }
        return ok("Schlüssel hinterlegt.");
    }
    let Some((bin, _)) = tools::find_agent(state, kind) else {
        return ("error".into(), format!("{} ist nicht installiert.", kind.name()));
    };
    match kind {
        AgentKind::Claude => match run_quiet(state, r, &bin, &["auth", "status", "--json"]).await {
            Some((_, out)) => {
                let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_default();
                if v["loggedIn"].as_bool() == Some(true) {
                    let how = v["authMethod"].as_str().unwrap_or("");
                    ok(&format!("Angemeldet ({how})."))
                } else {
                    needs_login("Nicht angemeldet.")
                }
            }
            None => ("error".into(), "Status nicht abrufbar.".into()),
        },
        AgentKind::Codex => match run_quiet(state, r, &bin, &["login", "status"]).await {
            Some((true, out)) => ok(out.lines().next().unwrap_or("Angemeldet.").trim()),
            Some((false, _)) => needs_login("Nicht angemeldet."),
            None => ("error".into(), "Status nicht abrufbar.".into()),
        },
        AgentKind::Gemini => {
            if home.join(".gemini").join("oauth_creds.json").is_file() {
                ok("Mit Google angemeldet.")
            } else {
                needs_login("Nicht angemeldet.")
            }
        }
        AgentKind::Opencode | AgentKind::Kilo => {
            let dir = if kind == AgentKind::Kilo { "kilo" } else { "opencode" };
            let file = home.join(".local/share").join(dir).join("auth.json");
            let n = tokio::fs::read_to_string(&file)
                .await
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v.as_object().map(|o| o.len()))
                .unwrap_or(0);
            if n > 0 {
                ok(&format!("{n} Anbieter angemeldet."))
            } else {
                needs_login("Kein Anbieter angemeldet.")
            }
        }
        AgentKind::Grok => match tokio::fs::read_to_string(home.join(".grok/auth.json")).await {
            Ok(t) if grok_session_present(&t) => ok("Mit Grok angemeldet."),
            _ => needs_login("Nicht angemeldet."),
        },
    }
}

/// `grok login` schreibt die Sitzung nach `~/.grok/auth.json` (Objekt, nicht leer).
fn grok_session_present(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text).ok().is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty()))
}

/// Kurzer, lokaler Aufruf des CLIs in der Umgebung des Kontos (Erfolg, Ausgabe).
async fn run_quiet(state: &AppState, r: &AccountRow, bin: &std::path::Path, args: &[&str]) -> Option<(bool, String)> {
    let mut cmd = crate::platform::command(bin);
    cmd.args(args).stdin(Stdio::null()).kill_on_drop(true).current_dir(home(state, r.id));
    super::env::apply_base(&mut cmd, state);
    for (k, v) in env_for(state, r) {
        cmd.env(k, v);
    }
    let out = tokio::time::timeout(Duration::from_secs(20), cmd.output()).await.ok()?.ok()?;
    Some((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Konto als zuletzt benutzt markieren.
pub async fn touch(state: &AppState, id: i64) -> AppResult<()> {
    sqlx::query("UPDATE agent_accounts SET last_used_at = ? WHERE id = ?").bind(now()).bind(id).execute(&state.db).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn hints() {
        assert_eq!(super::secret_hint(""), "");
        assert_eq!(super::secret_hint("sk-ant-api03-abcdefghijklmnop"), "sk-…mnop");
        assert_eq!(super::secret_hint("abcdefghijklmnopqrstuvwxyz"), "abc…wxyz");
        assert_eq!(super::secret_hint("short"), "…");
    }

    #[test]
    fn grok_auth_file() {
        assert!(super::grok_session_present(r#"{"https://auth.x.ai":{"email":"a@b.c"}}"#));
        assert!(!super::grok_session_present("{}"));
        assert!(!super::grok_session_present(""));
        assert!(!super::grok_session_present("not json"));
    }

    /// Verknüpfte Anmeldung: die neuere Datei gewinnt (in beide Richtungen), in nrzCode entsteht nichts Neues.
    #[test]
    fn login_sync_newest_wins() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let rel = ".claude/.credentials.json";
        std::fs::create_dir_all(a.path().join(".claude")).unwrap();
        std::fs::write(a.path().join(rel), "alt").unwrap();
        // nur in nrzCode → kommt nach nrzX
        assert_eq!(super::sync_files(a.path(), b.path()), 1);
        assert_eq!(std::fs::read_to_string(b.path().join(rel)).unwrap(), "alt");
        // gleich → nichts
        assert_eq!(super::sync_files(a.path(), b.path()), 0);
        // nrzX erneuert (neuer) → geht nach nrzCode
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(b.path().join(rel), "neu-x").unwrap();
        assert_eq!(super::sync_files(a.path(), b.path()), 1);
        assert_eq!(std::fs::read_to_string(a.path().join(rel)).unwrap(), "neu-x");
        // nrzCode erneuert → kommt nach nrzX
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(a.path().join(rel), "neu-c").unwrap();
        assert_eq!(super::sync_files(a.path(), b.path()), 1);
        assert_eq!(std::fs::read_to_string(b.path().join(rel)).unwrap(), "neu-c");
        // nur in nrzX → in nrzCode wird nichts angelegt
        std::fs::create_dir_all(b.path().join(".codex")).unwrap();
        std::fs::write(b.path().join(".codex/auth.json"), "x").unwrap();
        super::sync_files(a.path(), b.path());
        assert!(!a.path().join(".codex/auth.json").exists());
    }

    /// Bestehende Einträge bleiben, neue kommen dazu; kaputte Dateien bleiben unberührt.
    #[tokio::test]
    async fn merge_json_keeps_and_adds() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".claude.json");
        std::fs::write(&p, r#"{"oauthAccount":{"email":"x"},"projects":{"/a":{"allowedTools":[]}}}"#).unwrap();
        super::merge_json(&p, |o| {
            o.insert("bypassPermissionsModeAccepted".into(), true.into());
            let projects = o.get_mut("projects").and_then(|v| v.as_object_mut()).unwrap();
            projects.entry("/b".to_string()).or_insert_with(|| serde_json::json!({})).as_object_mut().unwrap().insert("hasTrustDialogAccepted".into(), true.into());
        })
        .await;
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["oauthAccount"]["email"], "x");
        assert!(v["projects"]["/a"]["allowedTools"].is_array());
        assert_eq!(v["projects"]["/b"]["hasTrustDialogAccepted"], true);
        assert_eq!(v["bypassPermissionsModeAccepted"], true);
        // kaputt: nicht anfassen
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{nope").unwrap();
        super::merge_json(&bad, |o| {
            o.insert("x".into(), true.into());
        })
        .await;
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{nope");
        // fehlt: neu
        let new = dir.path().join("settings.json");
        super::merge_json(&new, |o| {
            o.insert("y".into(), 1.into());
        })
        .await;
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&new).unwrap()).unwrap();
        assert_eq!(v["y"], 1);
    }
}
