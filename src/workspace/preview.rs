//! Vorschau: Server, die Agenten im Container starten (z. B. `npm run dev` auf Port 5173),
//! erscheinen im Browser über einen eigenen Proxy auf `config.preview_port`. So funktionieren
//! auch Seiten mit absoluten Pfaden (`/assets/…`), Weiterleitungen und WebSockets (Hot Reload).
//!
//! Zugang nur mit Vorschau-Token: `POST /api/v1/previews/token` {threadId, port} gibt eine
//! kurzlebige Adresse `http://<host>:<preview_port>/__nrzx/open?token=…`. Sie setzt das
//! Cookie `nrzx_preview` (Sitzung für genau diesen Port) und leitet auf `/` weiter. Cookies
//! von nrzX (`nrzx_*`) reicht der Proxy nie an die Seite weiter.
//!
//! `GET /api/v1/previews?threadId=` listet lauschende Ports (Linux: `/proc/net/tcp{,6}`,
//! macOS: `lsof`) – eigene und solche ohne bekannten Besitzer, nie die anderer Benutzer und nie
//! die von nrzX selbst.

use super::model;
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::web::BaseUrl;
use crate::AppState;
use base64::Engine;
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, StatusCode};
use nrzx_shared::workspace::{PreviewPort, PreviewToken, PreviewTokenRequest};
use rocket::serde::json::Json;
use rocket::{get, post, routes, FromForm, Route, State};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

pub fn routes() -> Vec<Route> {
    routes![ports, token]
}

/// Cookie der Vorschau-Sitzung.
pub const COOKIE: &str = "nrzx_preview";
/// Pfad, der das Token einlöst.
pub const OPEN_PATH: &str = "/__nrzx/open";
/// Gültigkeit eines Tokens (Sekunden) – es wird sofort im Browser eingelöst.
const TOKEN_TTL: i64 = 120;
/// Gültigkeit einer Vorschau-Sitzung (Sekunden).
const SESSION_TTL: i64 = 12 * 3600;

#[derive(Debug, Clone)]
struct Grant {
    user_id: i64,
    port: u16,
    path: String,
    expires: i64,
}

#[derive(Debug, Clone)]
struct Session {
    port: u16,
    expires: i64,
}

static GRANTS: LazyLock<Mutex<HashMap<String, Grant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SESSIONS: LazyLock<Mutex<HashMap<String, Session>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn random_token() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

/// Ports von nrzX selbst – nie als Vorschau.
fn own_ports(state: &AppState) -> Vec<u16> {
    let mut v = vec![9300, 9301, state.config.preview_port];
    if let Ok(p) = crate::config::figment().extract_inner::<u16>("port") {
        v.push(p);
    }
    v
}

// ------------------------------------------------------------------ Lauschende Ports

/// Ein lauschender TCP-Port mit Prozess (falls bekannt).
#[derive(Debug, Clone, Default)]
pub struct Listener {
    pub port: u16,
    pub process: String,
    pub cwd: Option<PathBuf>,
    pub pid_known: bool,
}

/// Alle lauschenden TCP-Ports (ohne Doppelte).
pub async fn listeners() -> Vec<Listener> {
    let mut list = platform_listeners().await;
    list.sort_by_key(|l| (l.port, !l.pid_known));
    list.dedup_by_key(|l| l.port);
    list
}

#[cfg(target_os = "linux")]
async fn platform_listeners() -> Vec<Listener> {
    tokio::task::spawn_blocking(|| {
        // inode → port aus /proc/net/tcp und tcp6 (Zustand 0A = LISTEN)
        let mut by_inode: HashMap<u64, u16> = HashMap::new();
        for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(text) = std::fs::read_to_string(file) else { continue };
            for line in text.lines().skip(1) {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() < 10 || f[3] != "0A" {
                    continue;
                }
                let port = f[1].rsplit(':').next().and_then(|h| u16::from_str_radix(h, 16).ok()).unwrap_or(0);
                let inode = f[9].parse::<u64>().unwrap_or(0);
                if port > 0 {
                    by_inode.insert(inode, port);
                }
            }
        }
        let mut out: Vec<Listener> = Vec::new();
        let mut seen: HashMap<u64, ()> = HashMap::new();
        if let Ok(procs) = std::fs::read_dir("/proc") {
            for p in procs.flatten() {
                let name = p.file_name().to_string_lossy().to_string();
                if !name.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let Ok(fds) = std::fs::read_dir(p.path().join("fd")) else { continue };
                for fd in fds.flatten() {
                    let Ok(link) = std::fs::read_link(fd.path()) else { continue };
                    let link = link.to_string_lossy();
                    let Some(inode) = link.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')).and_then(|s| s.parse::<u64>().ok()) else {
                        continue;
                    };
                    if let Some(port) = by_inode.get(&inode) {
                        if seen.insert(inode, ()).is_none() {
                            let process = std::fs::read_to_string(p.path().join("comm")).unwrap_or_default().trim().to_string();
                            let cwd = std::fs::read_link(p.path().join("cwd")).ok();
                            out.push(Listener { port: *port, process, cwd, pid_known: true });
                        }
                    }
                }
            }
        }
        for (inode, port) in by_inode {
            if !seen.contains_key(&inode) {
                out.push(Listener { port, ..Default::default() });
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

#[cfg(not(target_os = "linux"))]
async fn platform_listeners() -> Vec<Listener> {
    // lsof -F: Zeilen p<pid>, c<befehl>, n<adresse:port>
    let Ok(out) = crate::platform::command("lsof").args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"]).output().await else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut list: Vec<(u32, String, u16)> = Vec::new();
    let (mut pid, mut cmd) = (0u32, String::new());
    for line in text.lines() {
        match line.split_at_checked(1) {
            Some(("p", v)) => pid = v.parse().unwrap_or(0),
            Some(("c", v)) => cmd = v.to_string(),
            Some(("n", v)) => {
                if let Some(port) = v.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
                    list.push((pid, cmd.clone(), port));
                }
            }
            _ => {}
        }
    }
    // Arbeitsordner der Prozesse
    let mut pids: Vec<String> = list.iter().map(|(p, _, _)| p.to_string()).collect();
    pids.sort();
    pids.dedup();
    let mut cwds: HashMap<u32, PathBuf> = HashMap::new();
    if !pids.is_empty() {
        if let Ok(out) = crate::platform::command("lsof").args(["-a", "-p", &pids.join(","), "-d", "cwd", "-Fn"]).output().await {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            let mut pid = 0u32;
            for line in text.lines() {
                match line.split_at_checked(1) {
                    Some(("p", v)) => pid = v.parse().unwrap_or(0),
                    Some(("n", v)) => {
                        cwds.insert(pid, PathBuf::from(v));
                    }
                    _ => {}
                }
            }
        }
    }
    list.into_iter()
        .map(|(pid, process, port)| Listener { port, process, cwd: cwds.get(&pid).cloned(), pid_known: pid > 0 })
        .collect()
}

/// Besitzer eines Arbeitsordners: Benutzer des Projekts bzw. Threads, in dem der Prozess läuft.
async fn owner_of(state: &AppState, cwd: &Path) -> Option<i64> {
    let ws = std::fs::canonicalize(state.config.workspaces_dir()).unwrap_or(state.config.workspaces_dir());
    let scratch = std::fs::canonicalize(state.config.scratch_dir()).unwrap_or(state.config.scratch_dir());
    let id_after = |base: &Path| -> Option<i64> {
        cwd.strip_prefix(base).ok()?.components().next()?.as_os_str().to_str()?.parse().ok()
    };
    if let Some(pid) = id_after(&ws) {
        return sqlx::query_scalar("SELECT user_id FROM projects WHERE id = ?").bind(pid).fetch_optional(&state.db).await.ok().flatten();
    }
    if let Some(tid) = id_after(&scratch) {
        return sqlx::query_scalar("SELECT user_id FROM threads WHERE id = ?").bind(tid).fetch_optional(&state.db).await.ok().flatten();
    }
    None
}

/// Darf der Benutzer diesen Port sehen? (Eigene Prozesse ja, fremde nein, unbekannte nur ohne Prozessangabe.)
async fn visible(state: &AppState, user_id: i64, l: &Listener) -> bool {
    if own_ports(state).contains(&l.port) {
        return false;
    }
    match &l.cwd {
        Some(cwd) => owner_of(state, cwd).await == Some(user_id),
        None => !l.pid_known,
    }
}

#[derive(Debug, FromForm)]
pub struct PortsQuery {
    #[field(name = "threadId")]
    thread_id: Option<i64>,
    /// Ohne Thread (integrierter Browser): Ports im Ordner dieses Projekts zuerst
    #[field(name = "projectId")]
    project_id: Option<i64>,
}

/// `GET /previews?threadId=` bzw. `?projectId=` – lauschende Ports, die zu diesem Benutzer
/// gehören (die des Threads bzw. Projekts zuerst).
#[get("/previews?<q..>")]
async fn ports(state: &State<AppState>, user: CurrentUser, q: PortsQuery) -> AppResult<Json<Vec<PreviewPort>>> {
    let dir = match (q.thread_id, q.project_id) {
        (Some(tid), _) => {
            let t = model::own_thread(&state.db, user.id(), tid).await?;
            super::gitops::current_dir(state, &t).await?.and_then(|d| std::fs::canonicalize(d).ok())
        }
        (None, Some(pid)) => {
            model::own_project(&state.db, user.id(), pid).await?;
            std::fs::canonicalize(model::project_dir(state, pid)).ok()
        }
        (None, None) => None,
    };
    let mut out = Vec::new();
    for l in listeners().await {
        if !visible(state, user.id(), &l).await {
            continue;
        }
        let this_thread = matches!((&l.cwd, &dir), (Some(c), Some(d)) if c.starts_with(d));
        out.push(PreviewPort { port: l.port, process: l.process.clone(), this_thread });
    }
    out.sort_by(|a, b| b.this_thread.cmp(&a.this_thread).then(a.port.cmp(&b.port)));
    Ok(Json(out))
}

/// Host ohne Port, z. B. `umbrel.local` aus `umbrel.local:9300`, `[::1]` aus `[::1]:9300`.
fn host_without_port(host: &str) -> String {
    if host.starts_with('[') {
        return host.split_once(']').map(|(h, _)| format!("{h}]")).unwrap_or_else(|| host.to_string());
    }
    match host.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h.to_string(),
        _ => host.to_string(),
    }
}

/// Basis der Vorschau für eine Anfrage, z. B. `http://umbrel.local:9301`.
pub fn preview_origin(state: &AppState, base_url: &str) -> String {
    let host = base_url.split_once("://").map(|(_, r)| r).unwrap_or(base_url).split('/').next().unwrap_or("localhost");
    format!("http://{}:{}", host_without_port(host), state.config.preview_port)
}

/// Token für die Vorschau anlegen (auch für Tests).
pub async fn grant(state: &AppState, user_id: i64, port: u16, path: &str) -> AppResult<String> {
    if port == 0 || own_ports(state).contains(&port) {
        return Err(AppError::bad("Dieser Port lässt sich nicht als Vorschau öffnen."));
    }
    for l in listeners().await {
        if l.port == port {
            if let Some(cwd) = &l.cwd {
                if let Some(owner) = owner_of(state, cwd).await {
                    if owner != user_id {
                        return Err(AppError::forbidden("Dieser Port gehört zu einem anderen Benutzer."));
                    }
                }
            }
        }
    }
    let path = if path.starts_with('/') && !path.starts_with("//") && !path.contains('\\') { path.to_string() } else { "/".into() };
    let token = random_token();
    let ts = now();
    let mut grants = GRANTS.lock().unwrap_or_else(|e| e.into_inner());
    grants.retain(|_, g| g.expires > ts);
    grants.insert(token.clone(), Grant { user_id, port, path, expires: ts + TOKEN_TTL });
    Ok(token)
}

/// `POST /previews/token` – kurzlebige Adresse zum Öffnen einer Vorschau.
#[post("/previews/token", data = "<body>")]
async fn token(state: &State<AppState>, user: CurrentUser, base: BaseUrl, body: Json<PreviewTokenRequest>) -> AppResult<Json<PreviewToken>> {
    // Ohne Thread (0): integrierter Browser – `grant` prüft, wem der Port gehört
    if body.thread_id > 0 {
        model::own_thread(&state.db, user.id(), body.thread_id).await?;
    }
    if state.config.preview_port == 0 {
        return Err(AppError::conflict("Die Vorschau ist ausgeschaltet (preview_port = 0)."));
    }
    let token = grant(state, user.id(), body.port, &body.path).await?;
    let origin = preview_origin(state, &base.0);
    Ok(Json(PreviewToken { url: format!("{origin}{OPEN_PATH}?token={token}"), origin, expires_at: now() + TOKEN_TTL }))
}

// ------------------------------------------------------------------ Proxy

/// Proxy auf `config.preview_port` starten (aus `workspace::start`).
pub fn start(state: &AppState) {
    let port = state.config.preview_port;
    if port == 0 {
        return;
    }
    let address: String = crate::config::figment().extract_inner("address").unwrap_or_else(|_| "0.0.0.0".into());
    let addr: SocketAddr = match format!("{address}:{port}").parse().or_else(|_| format!("[{address}]:{port}").parse()) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Vorschau: Adresse {address}:{port}: {e}");
            return;
        }
    };
    let listener = match std::net::TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Vorschau: Port {port} nicht verfügbar: {e}");
            return;
        }
    };
    tokio::spawn(async move {
        tracing::info!("Vorschau-Proxy auf {addr}");
        if let Err(e) = serve(listener).await {
            tracing::error!("Vorschau-Proxy beendet: {e}");
        }
    });
}

/// Proxy auf einem offenen Socket betreiben (auch für Tests).
pub async fn serve(listener: std::net::TcpListener) -> Result<(), hyper::Error> {
    let _ = listener.set_nonblocking(true);
    let make = make_service_fn(move |conn: &hyper::server::conn::AddrStream| {
        let remote = conn.remote_addr();
        async move {
            Ok::<_, Infallible>(service_fn(move |req| async move { Ok::<_, Infallible>(handle(remote, req).await) }))
        }
    });
    hyper::Server::from_tcp(listener)?.http1_preserve_header_case(true).serve(make).await
}

fn page(status: StatusCode, text: &str) -> Response<Body> {
    let html = format!(
        "<!doctype html><html lang=de><meta charset=utf-8><title>Vorschau · nrzX</title>\
         <body style=\"background:#171717;color:#e5e5e5;font-family:sans-serif;display:grid;place-items:center;min-height:90vh\">\
         <main style=\"max-width:32rem;text-align:center\"><p>{}</p></main>",
        crate::highlight::escape(text)
    );
    let mut res = Response::new(Body::from(html));
    *res.status_mut() = status;
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|c| {
        let (k, v) = c.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then(|| urlencoding::decode(v).map(|s| s.into_owned()).unwrap_or_default())
    })
}

async fn handle(remote: SocketAddr, req: Request<Body>) -> Response<Body> {
    if req.uri().path() == OPEN_PATH {
        return open(&req);
    }
    let session = cookie_value(req.headers(), COOKIE).and_then(|sid| {
        let ts = now();
        let mut sessions = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, s| s.expires > ts);
        sessions.get(&sid).cloned()
    });
    let Some(session) = session else {
        return page(StatusCode::FORBIDDEN, "Diese Vorschau ist abgelaufen oder gehört nicht zu diesem Browser – bitte in nrzX erneut öffnen.");
    };
    match forward(session.port, remote, req).await {
        Ok(res) => res,
        Err(text) => page(StatusCode::BAD_GATEWAY, &text),
    }
}

/// Token einlösen: Cookie setzen und weiterleiten.
fn open(req: &Request<Body>) -> Response<Body> {
    let token = query_param(req.uri().query().unwrap_or(""), "token").unwrap_or_default();
    let grant = GRANTS.lock().unwrap_or_else(|e| e.into_inner()).remove(&token);
    let Some(grant) = grant.filter(|g| g.expires > now()) else {
        return page(StatusCode::FORBIDDEN, "Dieser Link zur Vorschau ist abgelaufen – bitte in nrzX erneut öffnen.");
    };
    let sid = random_token();
    {
        let mut sessions = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
        sessions.insert(sid.clone(), Session { port: grant.port, expires: now() + SESSION_TTL });
    }
    tracing::debug!(user = grant.user_id, port = grant.port, "Vorschau geöffnet");
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::FOUND;
    let h = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&grant.path) {
        h.insert(header::LOCATION, v);
    }
    if let Ok(v) = HeaderValue::from_str(&format!("{COOKIE}={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_TTL}")) {
        h.insert(header::SET_COOKIE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    res
}

/// Adresse, unter der der Port antwortet (IPv4, sonst IPv6 – z. B. Vite auf `localhost` unter macOS).
async fn target_addr(port: u16) -> Option<SocketAddr> {
    static CACHE: LazyLock<Mutex<HashMap<u16, (SocketAddr, i64)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    let ts = now();
    if let Some((a, t)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(&port) {
        if ts - *t < 30 {
            return Some(*a);
        }
    }
    for addr in [SocketAddr::from(([127, 0, 0, 1], port)), SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port))] {
        if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(800), tokio::net::TcpStream::connect(addr)).await {
            CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(port, (addr, ts));
            return Some(addr);
        }
    }
    None
}

const HOP_BY_HOP: &[&str] = &["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "proxy-connection", "te", "trailer", "transfer-encoding", "upgrade"];

/// Anfrage an `127.0.0.1:<port>` weiterreichen (inkl. WebSocket-Upgrade) und die Antwort anpassen.
async fn forward(port: u16, remote: SocketAddr, mut req: Request<Body>) -> Result<Response<Body>, String> {
    let addr = target_addr(port).await.ok_or_else(|| format!("Auf Port {port} antwortet gerade nichts – läuft der Server noch?"))?;
    let upgrade = req.headers().get(header::UPGRADE).cloned();
    let client_upgrade = upgrade.as_ref().map(|_| hyper::upgrade::on(&mut req));
    let original_host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
    let path = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/".into());
    let host_s = match addr {
        SocketAddr::V4(_) => format!("127.0.0.1:{port}"),
        SocketAddr::V6(_) => format!("[::1]:{port}"),
    };
    let uri: hyper::Uri = format!("http://{host_s}{path}").parse().map_err(|_| "Ungültige Adresse.".to_string())?;

    let mut out = Request::builder().method(req.method().clone()).uri(uri).version(hyper::Version::HTTP_11);
    let headers = out.headers_mut().ok_or_else(|| "Ungültige Anfrage.".to_string())?;
    for (k, v) in req.headers() {
        let name = k.as_str();
        if HOP_BY_HOP.contains(&name) || name == "host" || name == "cookie" || name == "origin" {
            continue;
        }
        headers.append(k.clone(), v.clone());
    }
    // Entwicklungsserver (Vite, Next …) erwarten sich selbst als Host und Herkunft
    headers.insert(header::HOST, HeaderValue::from_str(&format!("localhost:{port}")).map_err(|e| e.to_string())?);
    if req.headers().contains_key(header::ORIGIN) {
        headers.insert(header::ORIGIN, HeaderValue::from_str(&format!("http://localhost:{port}")).map_err(|e| e.to_string())?);
    }
    // Cookies von nrzX (Sitzung!) nie an die Seite weitergeben
    let cookies: Vec<String> = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|c| !c.is_empty() && !c.starts_with("nrzx_"))
        .map(str::to_string)
        .collect();
    if !cookies.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&cookies.join("; ")) {
            headers.insert(header::COOKIE, v);
        }
    }
    if let Ok(v) = HeaderValue::from_str(&original_host) {
        headers.insert(HeaderName::from_static("x-forwarded-host"), v);
    }
    headers.insert(HeaderName::from_static("x-forwarded-proto"), HeaderValue::from_static("http"));
    if let Ok(v) = HeaderValue::from_str(&remote.ip().to_string()) {
        headers.insert(HeaderName::from_static("x-forwarded-for"), v);
    }
    if let Some(u) = &upgrade {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(header::UPGRADE, u.clone());
    }
    let body = std::mem::replace(req.body_mut(), Body::empty());
    let out = out.body(body).map_err(|e| e.to_string())?;

    let client = hyper::Client::builder().http1_preserve_header_case(true).build_http::<Body>();
    let mut res = client.request(out).await.map_err(|e| format!("Port {port} antwortet nicht: {e}"))?;

    if res.status() == StatusCode::SWITCHING_PROTOCOLS {
        let server_upgrade = hyper::upgrade::on(&mut res);
        if let Some(client_upgrade) = client_upgrade {
            tokio::spawn(async move {
                if let (Ok(mut a), Ok(mut b)) = (client_upgrade.await, server_upgrade.await) {
                    let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                }
            });
        }
        let mut back = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
        if let Some(h) = back.headers_mut() {
            for (k, v) in res.headers() {
                h.append(k.clone(), v.clone());
            }
        }
        return back.body(Body::empty()).map_err(|e| e.to_string());
    }

    let (mut parts, body) = res.into_parts();
    rewrite_response_headers(&mut parts.headers, port);
    Ok(Response::from_parts(parts, body))
}

/// Antwortköpfe anpassen: Weiterleitungen auf den Proxy umschreiben, Einbetten im iframe
/// erlauben, keine Cookies mit Namen von nrzX, keine Domain-Bindung an `localhost`.
pub fn rewrite_response_headers(headers: &mut HeaderMap, port: u16) {
    for h in HOP_BY_HOP {
        headers.remove(*h);
    }
    if let Some(loc) = headers.get(header::LOCATION).and_then(|v| v.to_str().ok()).map(str::to_string) {
        if let Some(rel) = local_redirect(&loc, port) {
            if let Ok(v) = HeaderValue::from_str(&rel) {
                headers.insert(header::LOCATION, v);
            }
        }
    }
    headers.remove(HeaderName::from_static("x-frame-options"));
    if let Some(csp) = headers.get(header::CONTENT_SECURITY_POLICY).and_then(|v| v.to_str().ok()).map(str::to_string) {
        let kept: Vec<&str> = csp.split(';').map(str::trim).filter(|d| !d.is_empty() && !d.starts_with("frame-ancestors")).collect();
        headers.remove(header::CONTENT_SECURITY_POLICY);
        if !kept.is_empty() {
            if let Ok(v) = HeaderValue::from_str(&kept.join("; ")) {
                headers.insert(header::CONTENT_SECURITY_POLICY, v);
            }
        }
    }
    let cookies: Vec<HeaderValue> = headers.get_all(header::SET_COOKIE).iter().cloned().collect();
    if !cookies.is_empty() {
        headers.remove(header::SET_COOKIE);
        for c in cookies {
            let Ok(s) = c.to_str() else { continue };
            if s.trim_start().starts_with("nrzx_") {
                continue;
            }
            let cleaned: Vec<&str> = s.split(';').filter(|p| !p.trim().to_ascii_lowercase().starts_with("domain=")).collect();
            if let Ok(v) = HeaderValue::from_str(&cleaned.join(";")) {
                headers.append(header::SET_COOKIE, v);
            }
        }
    }
}

/// `http://localhost:5173/x?y` → `/x?y` (nur für den weitergereichten Port).
fn local_redirect(location: &str, port: u16) -> Option<String> {
    let rest = location.strip_prefix("http://").or_else(|| location.strip_prefix("https://"))?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let locals = ["localhost", "127.0.0.1", "[::1]", "0.0.0.0"];
    let ok = locals.iter().any(|h| host == format!("{h}:{port}"));
    ok.then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_and_hosts() {
        assert_eq!(local_redirect("http://localhost:5173/login?x=1", 5173).as_deref(), Some("/login?x=1"));
        assert_eq!(local_redirect("http://127.0.0.1:5173", 5173).as_deref(), Some("/"));
        assert_eq!(local_redirect("http://localhost:3000/", 5173), None);
        assert_eq!(local_redirect("https://example.com/", 5173), None);
        assert_eq!(local_redirect("/relative", 5173), None);
        assert_eq!(host_without_port("umbrel.local:9300"), "umbrel.local");
        assert_eq!(host_without_port("[::1]:9300"), "[::1]");
        assert_eq!(host_without_port("example.com"), "example.com");
    }

    #[test]
    fn response_headers() {
        let mut h = HeaderMap::new();
        h.insert(header::LOCATION, HeaderValue::from_static("http://localhost:3000/next"));
        h.insert("x-frame-options", HeaderValue::from_static("DENY"));
        h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; frame-ancestors 'none'"));
        h.append(header::SET_COOKIE, HeaderValue::from_static("a=1; Domain=localhost; Path=/"));
        h.append(header::SET_COOKIE, HeaderValue::from_static("nrzx_session=x"));
        rewrite_response_headers(&mut h, 3000);
        assert_eq!(h.get(header::LOCATION).unwrap(), "/next");
        assert!(h.get("x-frame-options").is_none());
        assert_eq!(h.get(header::CONTENT_SECURITY_POLICY).unwrap(), "default-src 'self'");
        let cookies: Vec<_> = h.get_all(header::SET_COOKIE).iter().collect();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0], "a=1; Path=/");
    }
}
