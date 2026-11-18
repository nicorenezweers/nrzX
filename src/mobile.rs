//! Verbindung zu nrzXMobile – der App für unterwegs auf dem Umbrel (Ordner `mobile/`).
//!
//! In Einstellungen → Mobil koppelt man mit Adresse und Code aus nrzXMobile
//! (`POST <adresse>/pair/claim` → Token). Danach hält nrzX einen **ausgehenden** WebSocket zu
//! `<adresse>/connect` offen (der PC braucht keinen offenen Port) und baut ihn nach Abbrüchen
//! selbst wieder auf. Über ihn kommen:
//!
//! * `{"t":"req","id":1,"method":"GET","path":"/api/v1/threads","body":null}` von nrzXMobile –
//!   nrzX führt die Anfrage gegen die eigene API aus (über `127.0.0.1`, angemeldet als der
//!   gekoppelte Benutzer mit einer eigenen Sitzung) und antwortet mit
//!   `{"t":"res","id":1,"status":200,"body":"…"}`. Erlaubt sind nur Threads, Nachrichten,
//!   Nutzung, Projekte, Agenten und das eigene Profil ([`allowed`]).
//! * `{"t":"event","data":<HubEvent>}` an nrzXMobile – alle Live-Ereignisse des Benutzers.
//!
//! Gespeichert je Benutzer unter dem Schlüssel `mobile:<id>` in `settings`.

use crate::auth::{session, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::AppState;
use futures::{SinkExt, StreamExt};
use nrzx_shared::workspace::{MobileClaim, MobilePair, MobileStatus};
use rocket::serde::json::Json;
use rocket::{delete, get, post, routes, Route, State};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

pub fn routes() -> Vec<Route> {
    routes![status, pair, unpair]
}

/// Gespeicherte Kopplung.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Link {
    url: String,
    token: String,
    pc_id: String,
    name: String,
}

/// Laufende Verbindung je Benutzer.
struct Conn {
    task: tokio::task::JoinHandle<()>,
    connected: bool,
    error: String,
}

static CONNS: LazyLock<Mutex<HashMap<i64, Conn>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn key(user_id: i64) -> String {
    format!("mobile:{user_id}")
}

async fn load(state: &AppState, user_id: i64) -> AppResult<Option<Link>> {
    let raw = crate::db::setting(&state.db, &key(user_id), "").await?;
    Ok(serde_json::from_str::<Link>(&raw).ok().filter(|l| !l.url.is_empty() && !l.token.is_empty()))
}

/// Adresse vereinheitlichen: Schema ergänzen, Schrägstrich am Ende weg.
fn normalize(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    if u.starts_with("http://") || u.starts_with("https://") { u.to_string() } else { format!("http://{u}") }
}

fn ws_url(url: &str) -> String {
    let u = normalize(url);
    let ws = if let Some(rest) = u.strip_prefix("https://") { format!("wss://{rest}") } else { format!("ws://{}", u.trim_start_matches("http://")) };
    format!("{ws}/connect")
}

/// Rechnername für den PC in nrzXMobile.
fn host_name() -> String {
    std::env::var("NRZX_DESKTOP_USER")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("nrzX · {s}"))
        .unwrap_or_else(|| "nrzX".into())
}

/// Beim Start: alle gekoppelten Benutzer verbinden.
pub async fn start_all(state: AppState) {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT key FROM settings WHERE key LIKE 'mobile:%'").fetch_all(&state.db).await.unwrap_or_default();
    for (k,) in rows {
        if let Some(uid) = k.strip_prefix("mobile:").and_then(|s| s.parse::<i64>().ok()) {
            if let Ok(Some(link)) = load(&state, uid).await {
                restart(&state, uid, Some(link));
            }
        }
    }
}

/// Verbindung eines Benutzers (neu) starten bzw. beenden (`None`).
fn restart(state: &AppState, user_id: i64, link: Option<Link>) {
    let mut conns = CONNS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = conns.remove(&user_id) {
        old.task.abort();
    }
    if let Some(link) = link {
        let st = state.clone();
        let task = tokio::spawn(async move { run(st, user_id, link).await });
        conns.insert(user_id, Conn { task, connected: false, error: String::new() });
    }
}

fn set_status(user_id: i64, connected: bool, error: &str) {
    if let Some(c) = CONNS.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&user_id) {
        c.connected = connected;
        c.error = error.to_string();
    }
}

/// Verbunden bleiben: nach Abbrüchen mit wachsender Pause (2 s … 60 s) neu verbinden.
async fn run(state: AppState, user_id: i64, link: Link) {
    let mut pause = 2u64;
    loop {
        let started = std::time::Instant::now();
        let err = connect_once(&state, user_id, &link).await.err().unwrap_or_default();
        set_status(user_id, false, &err);
        if err.contains("401") || err.contains("403") {
            tracing::warn!("nrzXMobile: Kopplung abgelehnt ({err}) – in den Einstellungen neu koppeln");
        }
        if started.elapsed() > Duration::from_secs(60) {
            pause = 2;
        }
        tokio::time::sleep(Duration::from_secs(pause)).await;
        pause = (pause * 2).min(60);
    }
}

#[derive(Deserialize)]
struct Req {
    id: u64,
    method: String,
    path: String,
    #[serde(default)]
    body: Option<String>,
}

/// Was nrzXMobile aufrufen darf.
fn allowed(method: &str, path: &str) -> bool {
    let p = path.split('?').next().unwrap_or("");
    if p.contains("..") || !matches!(method, "GET" | "POST" | "PATCH" | "DELETE") {
        return false;
    }
    ["/api/v1/threads", "/api/v1/usage", "/api/v1/projects", "/api/v1/agents", "/api/v1/me"]
        .iter()
        .any(|pre| p == *pre || p.starts_with(&format!("{pre}/")) || p.starts_with(&format!("{pre}?")))
}

async fn connect_once(state: &AppState, user_id: i64, link: &Link) -> Result<(), String> {
    let mut req = ws_url(&link.url).into_client_request().map_err(|e| e.to_string())?;
    let h = req.headers_mut();
    h.insert("Authorization", format!("Bearer {}", link.token).parse().map_err(|_| "Token ungültig".to_string())?);
    if let Ok(v) = link.name.parse() {
        h.insert("X-Nrz-Name", v);
    }
    let (ws, _) = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| "Zeitüberschreitung beim Verbinden".to_string())?
        .map_err(|e| match e {
            tokio_tungstenite::tungstenite::Error::Http(r) if r.status().as_u16() == 401 => {
                "Kopplung nicht mehr gültig (401) – in nrzXMobile unter Einstellungen → PC verbinden einen neuen Code holen und hier neu koppeln".to_string()
            }
            tokio_tungstenite::tungstenite::Error::Http(r) => format!("nrzXMobile antwortet mit {}", r.status()),
            e => format!("nicht erreichbar: {e}"),
        })?;
    set_status(user_id, true, "");
    tracing::info!("nrzXMobile verbunden ({})", link.url);
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
    });
    // Live-Ereignisse des Benutzers weitergeben
    let mut sub = state.hub.subscribe();
    let etx = tx.clone();
    let events = tokio::spawn(async move {
        loop {
            match sub.recv().await {
                Ok((uid, ev)) if uid == user_id => {
                    let msg = serde_json::json!({ "t": "event", "data": ev }).to_string();
                    if etx.send(Message::Text(msg.into())).is_err() {
                        break;
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    // Eigene Sitzung für die Anfragen an die API
    let token = session::create(&state.db, user_id, "nrzXMobile").await.map_err(|e| e.to_string())?;
    let base = format!("http://127.0.0.1:{}", crate::agents::runtime_port(state));
    let ua = if state.config.desktop_token.is_empty() {
        "nrzXMobile".to_string()
    } else {
        format!("nrzXMobile nrzXApp/{}", crate::desktop::app_marker(&state.config.desktop_token))
    };
    let client = reqwest::Client::builder().timeout(Duration::from_secs(60)).build().map_err(|e| e.to_string())?;
    let mut ping = tokio::time::interval(Duration::from_secs(25));
    let result = loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let Ok(r) = serde_json::from_str::<Req>(&t) else { continue };
                    let (client, base, token, ua, tx) = (client.clone(), base.clone(), token.clone(), ua.clone(), tx.clone());
                    tokio::spawn(async move {
                        let (status, body) = forward(&client, &base, &token, &ua, &r).await;
                        let msg = serde_json::json!({ "t": "res", "id": r.id, "status": status, "body": body }).to_string();
                        let _ = tx.send(Message::Text(msg.into()));
                    });
                }
                Some(Ok(Message::Ping(p))) => { let _ = tx.send(Message::Pong(p)); }
                Some(Ok(Message::Close(_))) | None => break Err("Verbindung beendet".to_string()),
                Some(Err(e)) => break Err(e.to_string()),
                Some(Ok(_)) => {}
            },
            _ = ping.tick() => {
                if tx.send(Message::Ping(Vec::new().into())).is_err() {
                    break Err("Verbindung beendet".to_string());
                }
            }
        }
    };
    writer.abort();
    events.abort();
    let _ = session::destroy(&state.db, &token).await;
    result
}

/// Anfrage von nrzXMobile gegen die eigene API ausführen.
async fn forward(client: &reqwest::Client, base: &str, token: &str, ua: &str, r: &Req) -> (u16, String) {
    if !allowed(&r.method, &r.path) {
        return (403, r#"{"error":"Von nrzXMobile aus nicht erlaubt."}"#.into());
    }
    let method = reqwest::Method::from_bytes(r.method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut q = client.request(method, format!("{base}{}", r.path)).header("Authorization", format!("Bearer {token}")).header("User-Agent", ua);
    if let Some(b) = &r.body {
        q = q.header("Content-Type", "application/json").body(b.clone());
    }
    match q.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            (status, resp.text().await.unwrap_or_default())
        }
        Err(e) => (502, serde_json::json!({ "error": e.to_string() }).to_string()),
    }
}

// ------------------------------------------------------------------ API

/// `GET /api/v1/mobile` – Stand der Kopplung.
#[get("/mobile")]
async fn status(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<MobileStatus>> {
    let link = load(state, user.id()).await?;
    let (connected, error) = CONNS.lock().unwrap_or_else(|e| e.into_inner()).get(&user.id()).map(|c| (c.connected, c.error.clone())).unwrap_or_default();
    Ok(Json(match link {
        Some(l) => MobileStatus { configured: true, url: l.url, name: l.name, connected, error },
        None => MobileStatus::default(),
    }))
}

/// `POST /api/v1/mobile` – mit Adresse und Code aus nrzXMobile koppeln.
#[post("/mobile", data = "<body>")]
async fn pair(state: &State<AppState>, user: CurrentUser, body: Json<MobilePair>) -> AppResult<Json<MobileStatus>> {
    let url = normalize(&body.url);
    let code: String = body.code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    if body.url.trim().is_empty() || code.is_empty() {
        return Err(AppError::bad("Bitte Adresse und Code aus nrzXMobile eingeben."));
    }
    let name = if body.name.trim().is_empty() { host_name() } else { body.name.trim().to_string() };
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().map_err(|e| AppError::internal(e.to_string()))?;
    let resp = client
        .post(format!("{url}/pair/claim"))
        .json(&serde_json::json!({ "code": code, "name": name }))
        .send()
        .await
        .map_err(|e| AppError::bad(format!("nrzXMobile unter {url} nicht erreichbar: {e}")))?;
    if !resp.status().is_success() {
        let s = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_else(|| format!("nrzXMobile antwortet mit {s}"));
        return Err(AppError::bad(msg));
    }
    let claim: MobileClaim = resp.json().await.map_err(|e| AppError::bad(format!("Unerwartete Antwort von nrzXMobile: {e}")))?;
    let link = Link { url: url.clone(), token: claim.token, pc_id: claim.pc_id, name: name.clone() };
    crate::db::set_setting(&state.db, &key(user.id()), &serde_json::to_string(&link).unwrap_or_default()).await?;
    restart(state, user.id(), Some(link));
    // Kurz warten, damit der Stand gleich „verbunden“ zeigt
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        if CONNS.lock().unwrap_or_else(|e| e.into_inner()).get(&user.id()).is_some_and(|c| c.connected || !c.error.is_empty()) {
            break;
        }
    }
    status(state, user).await
}

/// `DELETE /api/v1/mobile` – Kopplung aufheben.
#[delete("/mobile")]
async fn unpair(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<MobileStatus>> {
    crate::db::set_setting(&state.db, &key(user.id()), "").await?;
    restart(state, user.id(), None);
    Ok(Json(MobileStatus::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_paths() {
        assert_eq!(ws_url("umbrel.local:8471/"), "ws://umbrel.local:8471/connect");
        assert_eq!(ws_url("https://m.example.ch"), "wss://m.example.ch/connect");
        assert!(allowed("GET", "/api/v1/threads?parent=none"));
        assert!(allowed("POST", "/api/v1/threads/4/messages"));
        assert!(allowed("GET", "/api/v1/usage"));
        assert!(!allowed("GET", "/api/v1/settings"));
        assert!(!allowed("POST", "/api/v1/threads/../users"));
        assert!(!allowed("PUT", "/api/v1/threads/1"));
        assert!(!allowed("GET", "/api/v1/threadsx"));
    }
}
