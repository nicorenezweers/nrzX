//! Aktualisieren aus der App heraus (Einstellungen → Updates, nur Administratoren).
//!
//! Neue Versionen liegen wie für den nrzStore im Repository nrzXMobile auf nrzGit (Tags
//! `umbrel-X.Y.Z`, darin `SHA256SUMS` und `nrzx-mobile-<arch>.gz`). Die neueste wird geladen,
//! geprüft (SHA-256, startet und meldet ihre Version) und nach `<daten>/update/` gelegt; danach
//! ersetzt sich der laufende Prozess per `exec` durch sie – ohne Neustart des Containers.
//!
//! Beim Start wechselt das Programm aus dem Store (`/app`, schreibgeschützt) in die Version im
//! Datenordner, solange diese neuer ist ([`launch_newer`]). Bringt der Store später dieselbe oder
//! eine neuere Version, läuft wieder die aus dem Store.
//!
//! nrzGit: `NRZMOBILE_UPDATE_URL`, sonst die Adresse, über die die App gerade geöffnet ist
//! (Port 8390), sonst das Gateway des Containers (der Umbrel-Host) auf Port 8390.

use crate::auth::CurrentUser;
use crate::{err, Shared};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

const REPO: &str = "nicorenezweers/nrzXMobile";
/// Gesetzt im Prozess, der schon gewechselt hat (nicht noch einmal wechseln).
const SWITCHED: &str = "NRZMOBILE_SWITCHED";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim().trim_start_matches('v').split('.').map(|p| p.trim().parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

/// `a` neuer als `b`?
pub fn newer(a: &str, b: &str) -> bool {
    matches!((parse(a), parse(b)), (Some(x), Some(y)) if x > y)
}

fn update_dir(data: &Path) -> PathBuf {
    data.join("update")
}

fn arch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("amd64"),
        "aarch64" => Some("arm64"),
        _ => None,
    }
}

/// Beim Start: liegt im Datenordner eine neuere Version, in sie wechseln.
pub fn launch_newer(data: &Path) {
    if std::env::var_os(SWITCHED).is_some() {
        return;
    }
    let dir = update_dir(data);
    let bin = dir.join("nrzx-mobile");
    let Ok(v) = std::fs::read_to_string(dir.join("version")) else { return };
    if !bin.is_file() || !newer(&v, VERSION) {
        return;
    }
    let err = exec(&bin);
    eprintln!("nrzXMobile: Version {} aus {} lässt sich nicht starten ({err}) – bleibe bei {VERSION}", v.trim(), bin.display());
}

/// Prozess durch `bin` ersetzen (gleiche Argumente); kehrt nur bei einem Fehler zurück.
fn exec(bin: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = std::process::Command::new(bin).args(std::env::args_os().skip(1)).env(SWITCHED, "1").exec();
        e.to_string()
    }
    #[cfg(not(unix))]
    {
        let _ = bin;
        "nur unter Linux/macOS".into()
    }
}

/// Mögliche Adressen von nrzGit (in dieser Reihenfolge versucht).
fn bases(headers: &HeaderMap) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(u) = std::env::var("NRZMOBILE_UPDATE_URL") {
        if !u.trim().is_empty() {
            out.push(u.trim().trim_end_matches('/').to_string());
        }
    }
    let host = headers.get("x-forwarded-host").or(headers.get(header::HOST)).and_then(|v| v.to_str().ok()).unwrap_or("");
    let name = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host).trim_matches(['[', ']']);
    if !name.is_empty() && name != "localhost" && name != "127.0.0.1" {
        out.push(format!("http://{name}:8390"));
    }
    if let Some(gw) = gateway() {
        out.push(format!("http://{gw}:8390"));
    }
    out.push("http://127.0.0.1:8390".into());
    out.dedup();
    out
}

/// Standard-Gateway aus `/proc/net/route` (im Container: der Host).
fn gateway() -> Option<String> {
    let text = std::fs::read_to_string("/proc/net/route").ok()?;
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() > 2 && f[1] == "00000000" {
            let g = u32::from_str_radix(f[2], 16).ok()?;
            let b = g.to_le_bytes();
            return Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]));
        }
    }
    None
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().connect_timeout(Duration::from_secs(4)).timeout(Duration::from_secs(120)).build().unwrap_or_default()
}

#[derive(Deserialize)]
struct Tags {
    items: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

/// Neueste Version auf nrzGit: (Adresse, Version).
async fn latest(headers: &HeaderMap) -> Result<(String, String), String> {
    let c = client();
    let mut last = String::from("nrzGit nicht erreichbar");
    for base in bases(headers) {
        let url = format!("{base}/api/v1/repos/{REPO}/tags?limit=100");
        match c.get(&url).send().await {
            Ok(r) if r.status().is_success() => {
                let tags: Tags = r.json().await.map_err(|e| format!("Unerwartete Antwort von nrzGit: {e}"))?;
                let best = tags
                    .items
                    .iter()
                    .filter_map(|t| t.name.strip_prefix("umbrel-"))
                    .filter(|v| parse(v).is_some())
                    .max_by_key(|v| parse(v))
                    .map(str::to_string)
                    .ok_or("Auf nrzGit gibt es noch keine Version.")?;
                return Ok((base, best));
            }
            Ok(r) => last = format!("nrzGit ({base}) antwortet mit {}", r.status()),
            Err(e) => last = format!("nrzGit ({base}) nicht erreichbar: {e}"),
        }
    }
    Err(last)
}

fn admin_only(u: &CurrentUser) -> Option<Response> {
    (!u.admin).then(|| err(StatusCode::FORBIDDEN, "Nur für Administratoren."))
}

/// `GET /api/update` – gibt es eine neuere Version? (Administratoren)
pub async fn check(Extension(u): Extension<CurrentUser>, headers: HeaderMap) -> Response {
    if let Some(r) = admin_only(&u) {
        return r;
    }
    match latest(&headers).await {
        Ok((_, v)) => Json(json!({ "current": VERSION, "latest": v, "available": newer(&v, VERSION) })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, &e),
    }
}

/// `POST /api/update` – neueste Version laden, prüfen, einsetzen und neu starten (Administratoren).
pub async fn install(State(app): State<Shared>, Extension(u): Extension<CurrentUser>, headers: HeaderMap) -> Response {
    if let Some(r) = admin_only(&u) {
        return r;
    }
    let Some(arch) = arch() else { return err(StatusCode::BAD_REQUEST, "Für diese Architektur gibt es keine Updates.") };
    let (base, version) = match latest(&headers).await {
        Ok(x) => x,
        Err(e) => return err(StatusCode::BAD_GATEWAY, &e),
    };
    if !newer(&version, VERSION) {
        return err(StatusCode::CONFLICT, &format!("Version {VERSION} ist schon die neueste."));
    }
    let _guard = match app.updating.try_lock() {
        Ok(g) => g,
        Err(_) => return err(StatusCode::CONFLICT, "Ein Update läuft schon."),
    };
    let bin = match download(&app.data, &base, &version, arch).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("Update auf {version} fehlgeschlagen: {e}");
            return err(StatusCode::BAD_GATEWAY, &e);
        }
    };
    tracing::info!("Update auf {version} von {} eingesetzt (durch {}) – starte neu", base, u.username);
    // Erst antworten, dann wechseln
    let app = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        app.save();
        let e = exec(&bin);
        tracing::error!("Neue Version lässt sich nicht starten: {e}");
    });
    Json(json!({ "version": version })).into_response()
}

/// Laden, prüfen und nach `<daten>/update/` legen; gibt das Programm zurück.
async fn download(data: &Path, base: &str, version: &str, arch: &str) -> Result<PathBuf, String> {
    let c = client();
    let raw = format!("{base}/{REPO}/raw/tag/umbrel-{version}");
    let get = |name: String| {
        let c = c.clone();
        let url = format!("{raw}/{name}");
        async move {
            let r = c.get(&url).send().await.map_err(|e| format!("{url}: {e}"))?;
            if !r.status().is_success() {
                return Err(format!("{url}: {}", r.status()));
            }
            r.bytes().await.map(|b| b.to_vec()).map_err(|e| format!("{url}: {e}"))
        }
    };
    let file = format!("nrzx-mobile-{arch}.gz");
    let sums = String::from_utf8(get("SHA256SUMS".into()).await?).map_err(|_| "SHA256SUMS ist kein Text".to_string())?;
    let want = sums
        .lines()
        .find_map(|l| l.split_once(char::is_whitespace).filter(|(_, n)| n.trim().trim_start_matches('*') == file).map(|(h, _)| h.trim().to_lowercase()))
        .ok_or_else(|| format!("{file} fehlt in SHA256SUMS"))?;
    let gz = get(file.clone()).await?;
    let have: String = Sha256::digest(&gz).iter().map(|b| format!("{b:02x}")).collect();
    if have != want {
        return Err(format!("Prüfsumme von {file} stimmt nicht – Update abgebrochen."));
    }
    let mut bytes = Vec::new();
    {
        use std::io::Read;
        flate2::read::GzDecoder::new(&gz[..]).read_to_end(&mut bytes).map_err(|e| format!("{file} lässt sich nicht entpacken: {e}"))?;
    }
    let dir = update_dir(data);
    let tmp = data.join("update.new");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let bin = tmp.join("nrzx-mobile");
    std::fs::write(&bin, &bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    // Startet sie hier und ist es die erwartete Version?
    let out = tokio::process::Command::new(&bin).arg("version").output().await.map_err(|e| format!("Neue Version startet nicht ({e}) – ist der Datenordner ausführbar?"))?;
    let reported = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if reported != version {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("Neue Version meldet „{reported}“ statt {version} – Update abgebrochen."));
    }
    std::fs::write(tmp.join("version"), version).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&tmp, &dir).map_err(|e| e.to_string())?;
    Ok(dir.join("nrzx-mobile"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("1.2.0", "1.1.9"));
        assert!(newer("1.10.0", "1.9.3"));
        assert!(!newer("1.1.1", "1.1.1"));
        assert!(!newer("1.0.0", "1.1.0"));
        assert!(!newer("x", "1.0.0"));
    }

    #[test]
    fn update_bases() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "100.78.107.162:8470".parse().unwrap());
        assert!(bases(&h).contains(&"http://100.78.107.162:8390".to_string()));
    }
}
