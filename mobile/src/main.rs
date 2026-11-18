//! nrzXMobile – nrzX für unterwegs. Läuft auf dem Umbrel (App `nrz-x-mobile` im
//! nrzStore) und hat zwei Ports:
//!
//! * **Oberfläche** (`NRZMOBILE_PORT`, 8470): die Handy-Oberfläche (`mobile/ui`, eingebettet) und
//!   ihre API. Eigene Konten mit Passwort und Sitzungs-Cookie (siehe [`auth`]); jeder sieht nur
//!   seine eigenen PCs und deren Chats:
//!   `GET /api/auth/state`, `POST /api/auth/{setup,login,logout,password}`, `GET|POST /api/users`,
//!   `DELETE /api/users/{id}` (Administratoren), `GET /api/info`, `GET /api/pcs`,
//!   `DELETE /api/pcs/{id}`, `POST /api/pair` (Code zum Koppeln, gehört dem Konto),
//!   `GET /api/pc/{id}/events` (Server-Sent Events des PCs) und `/api/pc/{id}/v1/{pfad}` – wird
//!   unverändert als `/api/v1/{pfad}` an den PC weitergegeben.
//! * **PCs** (`NRZMOBILE_PC_PORT`, 8471, direkt): `POST /pair/claim` (Code → Token) und
//!   `GET /connect` – der WebSocket, den nrzX am PC von sich aus aufbaut
//!   (`Authorization: Bearer <token>`). Protokoll siehe `src/mobile.rs` in nrzX.
//!
//! Konten, Sitzungen (nur Hashes) und gekoppelte PCs (Name, Besitzer, Hash des Tokens) liegen in
//! `<daten>/store.json` (nur für den Besitzer lesbar; übernimmt beim ersten Start `pcs.json`).

mod auth;
mod update;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::middleware;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post};
use axum::{Extension, Json, Router};
use auth::CurrentUser;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, oneshot};

mod files {
    pub struct StaticFile {
        pub path: &'static str,
        pub body: &'static [u8],
        pub gzip: Option<&'static [u8]>,
    }
    include!(concat!(env!("OUT_DIR"), "/files.rs"));
}

/// Ein gekoppelter PC.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Pc {
    id: String,
    name: String,
    token_hash: String,
    created_at: i64,
    last_seen: i64,
    /// Konto, dem der PC gehört (leer: aus der Zeit vor den Konten – niemandem zugänglich, beim
    /// Einrichten des ersten Kontos verworfen)
    owner: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Store {
    users: Vec<auth::User>,
    sessions: Vec<auth::Session>,
    pcs: Vec<Pc>,
}

/// Offene Verbindung eines PCs.
struct Conn {
    conn_id: u64,
    tx: mpsc::UnboundedSender<Message>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<(u16, String)>>>>,
}

struct App {
    data: PathBuf,
    store: Mutex<Store>,
    /// Codes zum Koppeln: (Code, gültig bis, Konto)
    codes: Mutex<Vec<(String, i64, String)>>,
    attempts: auth::Attempts,
    /// Läuft gerade ein Update aus der App?
    updating: tokio::sync::Mutex<()>,
    conns: Mutex<HashMap<String, Conn>>,
    /// (PC-ID, JSON) für die Server-Sent Events
    events: broadcast::Sender<(String, String)>,
    next: AtomicU64,
    pc_port: u16,
    /// Adresse für PCs, wie sie die Oberfläche beim Koppeln zeigt (leer = Host der Seite + PC-Port)
    pc_url: String,
}

type Shared = Arc<App>;

pub(crate) fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub(crate) fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn random(n: usize, alphabet: &[u8]) -> String {
    use rand::Rng;
    let mut r = rand::thread_rng();
    (0..n).map(|_| alphabet[r.gen_range(0..alphabet.len())] as char).collect()
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_string())
}

impl App {
    fn save(&self) {
        let s = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = self.data.join("store.json.tmp");
        if write_private(&tmp, &serde_json::to_vec_pretty(&*s).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, self.data.join("store.json"));
        }
    }

    /// PC des Kontos (sonst `None` – fremde PCs gibt es für niemanden sonst).
    fn owned(&self, user: &CurrentUser, pc: &str) -> bool {
        self.store.lock().unwrap_or_else(|e| e.into_inner()).pcs.iter().any(|p| p.id == pc && p.owner == user.id)
    }

    /// Offene Verbindung eines PCs schliessen.
    fn disconnect(&self, id: &str) {
        if let Some(c) = self.conns.lock().unwrap_or_else(|e| e.into_inner()).remove(id) {
            let _ = c.tx.send(Message::Close(None));
        }
    }

    fn online(&self, id: &str) -> bool {
        self.conns.lock().unwrap_or_else(|e| e.into_inner()).contains_key(id)
    }

    /// Anfrage an einen PC; Antwort (Status, Rumpf) oder `None` (nicht verbunden, Zeitüberschreitung).
    async fn request(&self, pc: &str, method: &str, path: &str, body: Option<String>) -> Option<(u16, String)> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let conns = self.conns.lock().unwrap_or_else(|e| e.into_inner());
            let c = conns.get(pc)?;
            c.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
            let msg = json!({ "t": "req", "id": id, "method": method, "path": path, "body": body }).to_string();
            c.tx.send(Message::Text(msg.into())).ok()?;
        }
        tokio::time::timeout(Duration::from_secs(60), rx).await.ok()?.ok()
    }
}

#[tokio::main]
async fn main() {
    let port: u16 = env("NRZMOBILE_PORT", "8470").parse().unwrap_or(8470);
    if std::env::args().nth(1).as_deref() == Some("version") {
        println!("{}", update::VERSION);
        return;
    }
    // In der App installierte neuere Version? Dann gleich in sie wechseln
    update::launch_newer(&PathBuf::from(env("NRZMOBILE_DATA_DIR", "data")));
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        let ok = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(3)).is_ok();
        std::process::exit(if ok { 0 } else { 1 });
    }
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new(env("NRZMOBILE_LOG", "info"))).init();
    let pc_port: u16 = env("NRZMOBILE_PC_PORT", "8471").parse().unwrap_or(8471);
    let address = env("NRZMOBILE_ADDRESS", "0.0.0.0");
    let data = PathBuf::from(env("NRZMOBILE_DATA_DIR", "data"));
    let _ = std::fs::create_dir_all(&data);
    // Bestand: store.json, sonst die PCs aus der alten pcs.json (ohne Besitzer)
    let store: Store = std::fs::read(data.join("store.json"))
        .or_else(|_| std::fs::read(data.join("pcs.json")))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let app = Arc::new(App {
        data,
        store: Mutex::new(store),
        codes: Mutex::new(Vec::new()),
        attempts: Default::default(),
        updating: tokio::sync::Mutex::new(()),
        conns: Mutex::new(HashMap::new()),
        events: broadcast::channel(1024).0,
        next: AtomicU64::new(1),
        pc_port,
        pc_url: env("NRZMOBILE_PC_URL", ""),
    });

    app.save();
    if std::env::var("NRZMOBILE_PASSWORD").is_ok_and(|p| !p.is_empty()) {
        tracing::warn!("NRZMOBILE_PASSWORD wird nicht mehr benutzt – nrzXMobile hat jetzt eigene Konten (erstes Konto beim ersten Aufruf anlegen)");
    }
    let ui = Router::new()
        .route("/api/auth/state", get(auth::state))
        .route("/api/auth/setup", post(auth::setup))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/users", get(auth::list_users).post(auth::create_user))
        .route("/api/users/{id}", delete(auth::delete_user))
        .route("/api/info", get(info))
        .route("/api/update", get(update::check).post(update::install))
        .route("/api/pcs", get(list_pcs))
        .route("/api/pcs/{id}", delete(remove_pc))
        .route("/api/pair", post(new_code))
        .route("/api/pc/{id}/events", get(events))
        .route("/api/pc/{id}/v1/{*path}", any(proxy))
        .route("/api/{*rest}", any(|| async { err(StatusCode::NOT_FOUND, "Unbekannte Adresse.") }))
        .fallback(static_file)
        .layer(middleware::from_fn_with_state(app.clone(), auth::guard))
        .layer(middleware::from_fn(auth::security_headers))
        .with_state(app.clone());
    let pcs = Router::new().route("/pair/claim", post(claim)).route("/connect", get(connect)).with_state(app.clone());

    let ui_listener = tokio::net::TcpListener::bind((address.as_str(), port)).await.expect("Port der Oberfläche belegt");
    let pc_listener = tokio::net::TcpListener::bind((address.as_str(), pc_port)).await.expect("Port für PCs belegt");
    tracing::info!("nrzXMobile {} – Oberfläche auf {address}:{port}, PCs auf {address}:{pc_port}", env!("CARGO_PKG_VERSION"));
    let shutdown = || async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let a = axum::serve(ui_listener, ui).with_graceful_shutdown(shutdown());
    let b = axum::serve(pc_listener, pcs).with_graceful_shutdown(shutdown());
    let _ = tokio::join!(a, b);
}

// ------------------------------------------------------------------ API der Oberfläche

/// Datei nur für den Besitzer lesbar schreiben (Hashes von Passwörtern und Sitzungen).
fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(data)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data)
    }
}

pub(crate) fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

async fn info(State(app): State<Shared>) -> Json<serde_json::Value> {
    Json(json!({ "version": env!("CARGO_PKG_VERSION"), "pcPort": app.pc_port, "pcUrl": app.pc_url }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PcInfo {
    id: String,
    name: String,
    online: bool,
    last_seen: i64,
}

async fn list_pcs(State(app): State<Shared>, Extension(user): Extension<CurrentUser>) -> Json<Vec<PcInfo>> {
    let pcs: Vec<Pc> = app.store.lock().unwrap_or_else(|e| e.into_inner()).pcs.iter().filter(|p| p.owner == user.id).cloned().collect();
    Json(pcs.into_iter().map(|p| PcInfo { online: app.online(&p.id), id: p.id, name: p.name, last_seen: p.last_seen }).collect())
}

async fn remove_pc(State(app): State<Shared>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> StatusCode {
    if !app.owned(&user, &id) {
        return StatusCode::NOT_FOUND;
    }
    app.store.lock().unwrap_or_else(|e| e.into_inner()).pcs.retain(|p| p.id != id);
    app.save();
    app.disconnect(&id);
    StatusCode::NO_CONTENT
}

/// Neuer Code zum Koppeln (zehn Minuten gültig).
async fn new_code(State(app): State<Shared>, Extension(user): Extension<CurrentUser>) -> Json<serde_json::Value> {
    let code = random(8, b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789");
    let until = now() + 600;
    let mut codes = app.codes.lock().unwrap_or_else(|e| e.into_inner());
    codes.retain(|(_, t, _)| *t > now());
    // Höchstens fünf offene Codes je Konto
    let mine = codes.iter().filter(|(_, _, o)| *o == user.id).count();
    if mine >= 5 {
        if let Some(i) = codes.iter().position(|(_, _, o)| *o == user.id) {
            codes.remove(i);
        }
    }
    codes.push((code.clone(), until, user.id.clone()));
    Json(json!({ "code": format!("{}-{}", &code[..4], &code[4..]), "expiresAt": until, "pcPort": app.pc_port, "pcUrl": app.pc_url }))
}

/// Live-Ereignisse eines PCs (und ob er verbunden ist) als Server-Sent Events.
async fn events(State(app): State<Shared>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> Response {
    if !app.owned(&user, &id) {
        return err(StatusCode::NOT_FOUND, "PC nicht gefunden.");
    }
    let rx = app.events.subscribe();
    let first = json!({ "kind": if app.online(&id) { "online" } else { "offline" } }).to_string();
    let want = id.clone();
    let live = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(move |m| {
        let want = want.clone();
        async move {
            match m {
                Ok((pc, data)) if pc == want => Some(Ok::<Event, Infallible>(Event::default().data(data))),
                _ => None,
            }
        }
    });
    let stream = futures_util::stream::once(async move { Ok(Event::default().data(first)) }).chain(live);
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))).into_response()
}

/// `/api/pc/{id}/v1/{pfad}` → `/api/v1/{pfad}` am PC.
async fn proxy(State(app): State<Shared>, Extension(user): Extension<CurrentUser>, Path((id, _)): Path<(String, String)>, method: Method, uri: Uri, body: String) -> Response {
    if !app.owned(&user, &id) {
        return err(StatusCode::NOT_FOUND, "PC nicht gefunden.");
    }
    let prefix = format!("/api/pc/{id}/v1/");
    let rest = uri.path_and_query().map(|p| p.as_str()).unwrap_or("").strip_prefix(&prefix).unwrap_or("").to_string();
    let path = format!("/api/v1/{rest}");
    let body = (!body.is_empty()).then_some(body);
    match app.request(&id, method.as_str(), &path, body).await {
        Some((status, text)) => {
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            (status, [(header::CONTENT_TYPE, "application/json")], text).into_response()
        }
        None if !app.online(&id) => err(StatusCode::SERVICE_UNAVAILABLE, "Der PC ist gerade nicht verbunden."),
        None => err(StatusCode::GATEWAY_TIMEOUT, "Der PC antwortet nicht."),
    }
}

async fn static_file(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    let file = files::FILES.iter().find(|f| f.path == path).or_else(|| {
        // Seiten der Oberfläche (ohne Dateiendung): index.html
        (!path.contains('.')).then(|| files::FILES.iter().find(|f| f.path == "index.html")).flatten()
    });
    let Some(f) = file else {
        if files::FILES.is_empty() {
            return (StatusCode::OK, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], "<h1>nrzXMobile</h1><p>Die Oberfläche wurde nicht mitgebaut (scripts/mobile.sh).</p>").into_response();
        }
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = match f.path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "css" => "text/css",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "json" | "webmanifest" => "application/manifest+json",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    };
    // Dateien mit Prüfsumme im Namen (Dioxus) lange zwischenspeichern, index.html nie
    let cache = if f.path == "index.html" { "no-cache" } else if f.path.starts_with("assets/") || f.path.starts_with("wasm/") { "public, max-age=31536000, immutable" } else { "public, max-age=3600" };
    let gzip_ok = headers.get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("gzip"));
    match (f.gzip, gzip_ok) {
        (Some(gz), true) => ([(header::CONTENT_TYPE, mime), (header::CONTENT_ENCODING, "gzip"), (header::CACHE_CONTROL, cache)], gz).into_response(),
        _ => ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, cache)], f.body).into_response(),
    }
}

// ------------------------------------------------------------------ PCs

#[derive(Deserialize)]
struct Claim {
    code: String,
    #[serde(default)]
    name: String,
}

/// Code einlösen: neuer PC mit eigenem Token.
async fn claim(State(app): State<Shared>, Json(body): Json<Claim>) -> Response {
    let code: String = body.code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    let owner = {
        let mut codes = app.codes.lock().unwrap_or_else(|e| e.into_inner());
        codes.retain(|(_, t, _)| *t > now());
        let hit = codes.iter().position(|(c, _, _)| *c == code);
        hit.map(|i| codes.remove(i).2)
    };
    let Some(owner) = owner.filter(|o| app.store.lock().unwrap_or_else(|e| e.into_inner()).users.iter().any(|u| u.id == *o)) else {
        return err(StatusCode::BAD_REQUEST, "Code unbekannt oder abgelaufen – in nrzXMobile einen neuen erzeugen.");
    };
    let token = random(48, b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
    let pc = Pc {
        id: random(12, b"abcdefghijklmnopqrstuvwxyz0123456789"),
        name: if body.name.trim().is_empty() { "nrzX".into() } else { body.name.trim().chars().take(60).collect() },
        token_hash: hash(&token),
        created_at: now(),
        last_seen: now(),
        owner,
    };
    let id = pc.id.clone();
    app.store.lock().unwrap_or_else(|e| e.into_inner()).pcs.push(pc);
    app.save();
    tracing::info!("PC {id} gekoppelt");
    let _ = app.events.send((String::new(), json!({ "kind": "paired", "pcId": id }).to_string()));
    Json(json!({ "pcId": id, "token": token })).into_response()
}

async fn connect(State(app): State<Shared>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let token = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("").trim().to_string();
    let h = hash(&token);
    let pc = app.store.lock().unwrap_or_else(|e| e.into_inner()).pcs.iter().find(|p| !token.is_empty() && p.token_hash == h).cloned();
    let Some(pc) = pc else { return err(StatusCode::UNAUTHORIZED, "Unbekannter PC – bitte neu koppeln.") };
    let name = headers.get("x-nrz-name").and_then(|v| v.to_str().ok()).map(|s| s.trim().chars().take(60).collect::<String>()).filter(|s| !s.is_empty());
    ws.on_upgrade(move |socket| serve_pc(app, pc.id, name, socket))
}

/// Verbindung eines PCs: Antworten den wartenden Anfragen zuordnen, Ereignisse weitergeben.
async fn serve_pc(app: Shared, id: String, name: Option<String>, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<(u16, String)>>>> = Arc::default();
    let conn_id = app.next.fetch_add(1, Ordering::Relaxed);
    {
        let mut store = app.store.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = store.pcs.iter_mut().find(|p| p.id == id) {
            p.last_seen = now();
            if let Some(n) = &name {
                p.name = n.clone();
            }
        }
    }
    app.save();
    if let Some(old) = app.conns.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), Conn { conn_id, tx: tx.clone(), pending: pending.clone() }) {
        let _ = old.tx.send(Message::Close(None));
    }
    tracing::info!("PC {id} verbunden");
    let _ = app.events.send((id.clone(), json!({ "kind": "online" }).to_string()));
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let close = matches!(m, Message::Close(_));
            if sink.send(m).await.is_err() || close {
                break;
            }
        }
    });
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
                    match v["t"].as_str() {
                        Some("res") => {
                            let rid = v["id"].as_u64().unwrap_or(0);
                            let status = v["status"].as_u64().unwrap_or(502) as u16;
                            let body = v["body"].as_str().unwrap_or("").to_string();
                            if let Some(w) = pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&rid) {
                                let _ = w.send((status, body));
                            }
                        }
                        Some("event") => {
                            let _ = app.events.send((id.clone(), json!({ "kind": "event", "event": v["data"] }).to_string()));
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            _ = ping.tick() => {
                if tx.send(Message::Ping(Vec::new().into())).is_err() {
                    break;
                }
            }
        }
    }
    writer.abort();
    let mine = {
        let mut conns = app.conns.lock().unwrap_or_else(|e| e.into_inner());
        if conns.get(&id).is_some_and(|c| c.conn_id == conn_id) {
            conns.remove(&id);
            true
        } else {
            false
        }
    };
    if mine {
        if let Some(p) = app.store.lock().unwrap_or_else(|e| e.into_inner()).pcs.iter_mut().find(|p| p.id == id) {
            p.last_seen = now();
        }
        app.save();
        tracing::info!("PC {id} getrennt");
        let _ = app.events.send((id.clone(), json!({ "kind": "offline" }).to_string()));
    }
}
