//! Benutzer und Sitzungen von nrzXMobile.
//!
//! * Konten mit Benutzername und Passwort (Argon2id). Das erste Konto legt man beim ersten Aufruf
//!   an (`POST /api/auth/setup`, nur solange es keines gibt) – es ist Administrator. Kopplungen aus
//!   der Zeit ohne Konten werden dabei verworfen (die PCs koppeln sich neu). Weitere Konten legt
//!   ein Administrator an.
//! * Anmeldung setzt das Cookie `nrzm_session` (HttpOnly, SameSite=Lax, `Secure` hinter HTTPS) mit
//!   einem zufälligen Token; gespeichert wird nur dessen SHA-256. Sitzungen laufen 30 Tage nach der
//!   letzten Nutzung ab, eine Passwortänderung beendet alle anderen.
//! * Jeder PC gehört genau einem Konto: Liste, Ereignisse und Weiterleitung an den PC prüfen das –
//!   niemand sieht die Chats eines anderen, auch ein Administrator nicht.
//! * Schutz: Anfragen, die etwas ändern, brauchen den Kopf `X-Nrzm: 1` (fremde Seiten können ihn
//!   nicht setzen) und – falls vorhanden – einen passenden `Origin`; nach Fehlversuchen wird die
//!   Anmeldung je Benutzername und Adresse zunehmend gesperrt.

use crate::{err, hash, now, random, Shared};
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const COOKIE: &str = "nrzm_session";
/// Gültigkeit einer Sitzung ab der letzten Nutzung.
const SESSION_TTL: i64 = 30 * 86_400;
const MIN_PASSWORD: usize = 8;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct User {
    pub id: String,
    pub username: String,
    /// Argon2id im PHC-Format
    pub password_hash: String,
    pub admin: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Session {
    /// SHA-256 des Tokens
    pub hash: String,
    pub user_id: String,
    pub created_at: i64,
    pub last_used: i64,
    pub agent: String,
}

/// Angemeldetes Konto (Erweiterung der Anfrage).
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: String,
    pub username: String,
    pub admin: bool,
    pub session: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    id: String,
    username: String,
    admin: bool,
    created_at: i64,
}

impl From<&User> for UserInfo {
    fn from(u: &User) -> Self {
        UserInfo { id: u.id.clone(), username: u.username.clone(), admin: u.admin, created_at: u.created_at }
    }
}

// ------------------------------------------------------------------ Passwörter

fn hash_password(pw: &str) -> String {
    use argon2::password_hash::{PasswordHasher, SaltString};
    use rand::RngCore;
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let salt = SaltString::encode_b64(&salt).expect("Salz");
    argon2::Argon2::default().hash_password(pw.as_bytes(), &salt).map(|h| h.to_string()).unwrap_or_default()
}

fn verify_password(pw: &str, phc: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(phc).is_ok_and(|h| argon2::Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
}

/// Für unbekannte Benutzer trotzdem rechnen (gleiche Dauer – verrät nicht, ob es das Konto gibt).
static DUMMY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| hash_password("nrzm-dummy-password"));

fn clean_username(raw: &str) -> Result<String, &'static str> {
    let u = raw.trim().to_lowercase();
    if u.len() < 2 || u.len() > 32 || !u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err("Benutzername: 2–32 Zeichen, nur Buchstaben, Ziffern, Punkt, Binde- und Unterstrich.");
    }
    Ok(u)
}

fn check_password(pw: &str) -> Result<(), &'static str> {
    if pw.chars().count() < MIN_PASSWORD {
        return Err("Das Passwort braucht mindestens 8 Zeichen.");
    }
    if pw.len() > 512 {
        return Err("Das Passwort ist zu lang.");
    }
    Ok(())
}

// ------------------------------------------------------------------ Cookies und Sitzungen

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Hinter HTTPS (Proxy meldet `X-Forwarded-Proto: https`) bekommt das Cookie `Secure`.
fn secure(headers: &HeaderMap) -> bool {
    headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).is_some_and(|v| v.eq_ignore_ascii_case("https"))
}

fn set_cookie(headers: &HeaderMap, token: &str, max_age: i64) -> HeaderValue {
    let s = if secure(headers) { "; Secure" } else { "" };
    HeaderValue::from_str(&format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{s}")).expect("Cookie")
}

/// Konto zum Cookie (und die Sitzung auffrischen).
pub fn user_from(app: &Shared, headers: &HeaderMap) -> Option<CurrentUser> {
    let token = cookie_token(headers)?;
    let h = hash(&token);
    let t = now();
    let mut store = app.store.lock().unwrap_or_else(|e| e.into_inner());
    store.sessions.retain(|s| s.last_used + SESSION_TTL > t);
    let s = store.sessions.iter_mut().find(|s| s.hash == h)?;
    let touch = t - s.last_used > 3600;
    if touch {
        s.last_used = t;
    }
    let uid = s.user_id.clone();
    let u = store.users.iter().find(|u| u.id == uid)?.clone();
    drop(store);
    if touch {
        app.save();
    }
    Some(CurrentUser { id: u.id, username: u.username, admin: u.admin, session: h })
}

fn new_session(app: &Shared, user_id: &str, headers: &HeaderMap) -> String {
    let token = random(43, b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
    let agent: String = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").chars().take(160).collect();
    let t = now();
    app.store.lock().unwrap_or_else(|e| e.into_inner()).sessions.push(Session { hash: hash(&token), user_id: user_id.into(), created_at: t, last_used: t, agent });
    app.save();
    token
}

fn logged_in(app: &Shared, user: &User, headers: &HeaderMap) -> Response {
    let token = new_session(app, &user.id, headers);
    let mut r = Json(json!({ "user": UserInfo::from(user) })).into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(headers, &token, SESSION_TTL));
    r
}

// ------------------------------------------------------------------ Schutz für alle /api-Aufrufe

/// Ohne Anmeldung erreichbar.
fn public(path: &str) -> bool {
    matches!(path, "/api/info" | "/api/auth/state" | "/api/auth/login" | "/api/auth/setup")
}

/// Ändernde Anfragen: eigener Kopf und gleicher Ursprung; danach Anmeldung (ausser öffentlich).
pub async fn guard(State(app): State<Shared>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if !path.starts_with("/api/") {
        return next.run(req).await;
    }
    let headers = req.headers();
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        if headers.get("x-nrzm").and_then(|v| v.to_str().ok()) != Some("1") {
            return err(StatusCode::FORBIDDEN, "Anfrage abgelehnt.");
        }
        if let (Some(origin), Some(host)) = (headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()), headers.get(header::HOST).and_then(|v| v.to_str().ok())) {
            let o = origin.split_once("://").map(|(_, r)| r).unwrap_or(origin);
            if o != host && headers.get("x-forwarded-host").and_then(|v| v.to_str().ok()) != Some(o) {
                return err(StatusCode::FORBIDDEN, "Anfrage von einer fremden Seite abgelehnt.");
            }
        }
    }
    match user_from(&app, headers) {
        Some(u) => {
            req.extensions_mut().insert(u);
        }
        None if public(&path) => {}
        None => return err(StatusCode::UNAUTHORIZED, "Bitte anmelden."),
    }
    next.run(req).await
}

/// Sicherheits-Köpfe für jede Antwort.
pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut r = next.run(req).await;
    let h = r.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'",
        ),
    );
    r
}

// ------------------------------------------------------------------ Sperre nach Fehlversuchen

/// Fehlversuche je Schlüssel (`u:<name>`, `a:<adresse>`): (Anzahl, gesperrt bis).
pub type Attempts = std::sync::Mutex<std::collections::HashMap<String, (u32, i64)>>;

fn client_addr(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
        .unwrap_or("direkt")
        .trim()
        .to_string()
}

fn locked_for(app: &Shared, keys: &[String]) -> i64 {
    let t = now();
    let a = app.attempts.lock().unwrap_or_else(|e| e.into_inner());
    keys.iter().filter_map(|k| a.get(k)).map(|(_, until)| until - t).max().unwrap_or(0).max(0)
}

fn record(app: &Shared, keys: &[String], ok: bool) {
    let mut a = app.attempts.lock().unwrap_or_else(|e| e.into_inner());
    for k in keys {
        if ok {
            a.remove(k);
            continue;
        }
        let e = a.entry(k.clone()).or_insert((0, 0));
        e.0 += 1;
        // Ab dem 5. Fehlversuch je Benutzername (20. je Adresse – hinter einem Proxy teilen sich
        // alle eine): 30 s, verdoppelt, höchstens eine Stunde
        let from = if k.starts_with("a:") { 20 } else { 5 };
        if e.0 >= from {
            e.1 = now() + (30i64 << (e.0 - from).min(7)).min(3600);
        }
    }
    if a.len() > 10_000 {
        let t = now();
        a.retain(|_, (_, until)| *until > t);
    }
}

// ------------------------------------------------------------------ API

/// `GET /api/auth/state` – muss eingerichtet werden? Wer ist angemeldet?
pub async fn state(State(app): State<Shared>, user: Option<Extension<CurrentUser>>) -> Json<serde_json::Value> {
    let setup = app.store.lock().unwrap_or_else(|e| e.into_inner()).users.is_empty();
    let me = user.map(|Extension(u)| json!({ "id": u.id, "username": u.username, "admin": u.admin }));
    Json(json!({ "setup": setup, "user": me }))
}

#[derive(Deserialize)]
pub struct Credentials {
    username: String,
    password: String,
}

/// `POST /api/auth/setup` – erstes Konto (Administrator); nur solange es keines gibt.
pub async fn setup(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<Credentials>) -> Response {
    let username = match clean_username(&b.username) {
        Ok(u) => u,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    if let Err(e) = check_password(&b.password) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let phc = hash_password(&b.password);
    let user = {
        let mut store = app.store.lock().unwrap_or_else(|e| e.into_inner());
        if !store.users.is_empty() {
            return err(StatusCode::CONFLICT, "nrzXMobile ist schon eingerichtet – bitte anmelden.");
        }
        let user = User { id: random(12, b"abcdefghijklmnopqrstuvwxyz0123456789"), username, password_hash: phc, admin: true, created_at: now() };
        store.users.push(user.clone());
        // Kopplungen aus der Zeit ohne Konten gehören niemandem: nicht dem ersten Konto geben
        // (wer zuerst einrichtet, bekäme sonst fremde Chats), sondern verwerfen – nrzX am PC
        // koppelt sich danach einmal neu mit einem Code dieses Kontos
        let orphans: Vec<String> = store.pcs.iter().filter(|p| p.owner.is_empty()).map(|p| p.id.clone()).collect();
        store.pcs.retain(|p| !p.owner.is_empty());
        (user, orphans)
    };
    let (user, orphans) = user;
    app.save();
    for pc in orphans {
        app.disconnect(&pc);
    }
    tracing::info!("Erstes Konto {} angelegt", user.username);
    logged_in(&app, &user, &headers)
}

/// `POST /api/auth/login`
pub async fn login(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<Credentials>) -> Response {
    let name = b.username.trim().to_lowercase();
    let keys = [format!("u:{name}"), format!("a:{}", client_addr(&headers))];
    let wait = locked_for(&app, &keys);
    if wait > 0 {
        return err(StatusCode::TOO_MANY_REQUESTS, &format!("Zu viele Fehlversuche – bitte in {wait} s noch einmal."));
    }
    let user = app.store.lock().unwrap_or_else(|e| e.into_inner()).users.iter().find(|u| u.username == name).cloned();
    let pw = b.password.clone();
    let phc = user.as_ref().map(|u| u.password_hash.clone()).unwrap_or_else(|| DUMMY.clone());
    let ok = tokio::task::spawn_blocking(move || verify_password(&pw, &phc)).await.unwrap_or(false) && user.is_some();
    record(&app, &keys, ok);
    match user.filter(|_| ok) {
        Some(u) => logged_in(&app, &u, &headers),
        None => {
            tracing::warn!("Fehlgeschlagene Anmeldung für {name:?}");
            err(StatusCode::UNAUTHORIZED, "Benutzername oder Passwort falsch.")
        }
    }
}

/// `POST /api/auth/logout`
pub async fn logout(State(app): State<Shared>, headers: HeaderMap, Extension(u): Extension<CurrentUser>) -> Response {
    app.store.lock().unwrap_or_else(|e| e.into_inner()).sessions.retain(|s| s.hash != u.session);
    app.save();
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(&headers, "", 0));
    r
}

#[derive(Deserialize)]
pub struct NewPassword {
    current: String,
    new: String,
}

/// `POST /api/auth/password` – eigenes Passwort ändern; beendet alle anderen Sitzungen.
pub async fn change_password(State(app): State<Shared>, Extension(u): Extension<CurrentUser>, Json(b): Json<NewPassword>) -> Response {
    if let Err(e) = check_password(&b.new) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let phc = app.store.lock().unwrap_or_else(|e| e.into_inner()).users.iter().find(|x| x.id == u.id).map(|x| x.password_hash.clone()).unwrap_or_default();
    let cur = b.current.clone();
    if !tokio::task::spawn_blocking(move || verify_password(&cur, &phc)).await.unwrap_or(false) {
        return err(StatusCode::BAD_REQUEST, "Das aktuelle Passwort stimmt nicht.");
    }
    let new = hash_password(&b.new);
    {
        let mut store = app.store.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(x) = store.users.iter_mut().find(|x| x.id == u.id) {
            x.password_hash = new;
        }
        store.sessions.retain(|s| s.user_id != u.id || s.hash == u.session);
    }
    app.save();
    StatusCode::NO_CONTENT.into_response()
}

fn admin_only(u: &CurrentUser) -> Option<Response> {
    (!u.admin).then(|| err(StatusCode::FORBIDDEN, "Nur für Administratoren."))
}

/// `GET /api/users` (Administrator)
pub async fn list_users(State(app): State<Shared>, Extension(u): Extension<CurrentUser>) -> Response {
    if let Some(r) = admin_only(&u) {
        return r;
    }
    let users: Vec<UserInfo> = app.store.lock().unwrap_or_else(|e| e.into_inner()).users.iter().map(UserInfo::from).collect();
    Json(users).into_response()
}

#[derive(Deserialize)]
pub struct NewUser {
    username: String,
    password: String,
    #[serde(default)]
    admin: bool,
}

/// `POST /api/users` (Administrator) – weiteres Konto.
pub async fn create_user(State(app): State<Shared>, Extension(u): Extension<CurrentUser>, Json(b): Json<NewUser>) -> Response {
    if let Some(r) = admin_only(&u) {
        return r;
    }
    let username = match clean_username(&b.username) {
        Ok(x) => x,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    if let Err(e) = check_password(&b.password) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let phc = hash_password(&b.password);
    let user = User { id: random(12, b"abcdefghijklmnopqrstuvwxyz0123456789"), username, password_hash: phc, admin: b.admin, created_at: now() };
    {
        let mut store = app.store.lock().unwrap_or_else(|e| e.into_inner());
        if store.users.iter().any(|x| x.username == user.username) {
            return err(StatusCode::CONFLICT, "Diesen Benutzernamen gibt es schon.");
        }
        store.users.push(user.clone());
    }
    app.save();
    tracing::info!("Konto {} angelegt von {}", user.username, u.username);
    Json(UserInfo::from(&user)).into_response()
}

/// `DELETE /api/users/{id}` (Administrator) – mit Sitzungen und PCs des Kontos.
pub async fn delete_user(State(app): State<Shared>, Extension(u): Extension<CurrentUser>, Path(id): Path<String>) -> Response {
    if let Some(r) = admin_only(&u) {
        return r;
    }
    if id == u.id {
        return err(StatusCode::BAD_REQUEST, "Das eigene Konto lässt sich nicht löschen.");
    }
    let pcs: Vec<String> = {
        let mut store = app.store.lock().unwrap_or_else(|e| e.into_inner());
        if !store.users.iter().any(|x| x.id == id) {
            return err(StatusCode::NOT_FOUND, "Konto nicht gefunden.");
        }
        store.users.retain(|x| x.id != id);
        store.sessions.retain(|s| s.user_id != id);
        let pcs = store.pcs.iter().filter(|p| p.owner == id).map(|p| p.id.clone()).collect();
        store.pcs.retain(|p| p.owner != id);
        pcs
    };
    app.save();
    for pc in pcs {
        app.disconnect(&pc);
    }
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_and_names() {
        let h = hash_password("geheim123");
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("geheim123", &h));
        assert!(!verify_password("geheim124", &h));
        assert!(!verify_password("x", "kein-hash"));
        assert_eq!(clean_username(" Nico.R "), Ok("nico.r".into()));
        assert!(clean_username("a").is_err());
        assert!(clean_username("na me").is_err());
        assert!(check_password("kurz").is_err());
    }

    #[test]
    fn cookie_parsing() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; nrzm_session=abc ; b=2"));
        assert_eq!(cookie_token(&h).as_deref(), Some("abc"));
        assert!(!secure(&h));
        h.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(set_cookie(&h, "t", 10).to_str().unwrap().ends_with("; Secure"));
    }
}
