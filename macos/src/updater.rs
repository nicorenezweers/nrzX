//! Automatische Updates aus den Releases von nrzGit.
//!
//! Windows: Tag `windows-<version>` mit `nrzX-<version>-x64.exe`. Das laufende Programm wird
//! umbenannt (das erlaubt Windows), die neue Datei an seine Stelle kopiert und neu gestartet.
//!
//! Seit 1.0.12 gibt es ein gemeinsames Release `v<version>` mit DMG und EXE.
//! Seit 1.0.29 liegt für Windows zusätzlich ein Installer bei (`nrzX-<version>-Setup.exe`, ohne
//! `x64` im Namen – ältere Versionen suchen die EXE mit `x64` und nähmen sonst den Installer).
//!
//! Releases der macOS-App haben den Tag `macos-<version>` (z. B. `macos-1.0.6`) und eine
//! angehängte DMG (`nrzX-<version>-<arch>.dmg`), optional dazu `SHA256SUMS`. Die App fragt
//! beim Start und danach alle paar Stunden die Release-Liste ab, meldet eine neuere Version
//! (Hinweis mit Knopf im Fenster und Eintrag im Menü), lädt die DMG, prüft die Prüfsumme,
//! ersetzt das Programm-Bundle und startet neu.

use futures::StreamExt;
use semver::Version;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// nrzGit-Repository, aus dem die Updates kommen. Mit `NRZX_UPDATE_REPO` überschreibbar
/// (volle API-Adresse des Repositorys).
pub const DEFAULT_REPO_API: &str = "https://git.zweers.ch/api/v1/repos/nicorenezweers/nrzX";
#[cfg(target_os = "macos")]
pub const TAG_PREFIX: &str = "macos-";
#[cfg(not(target_os = "macos"))]
pub const TAG_PREFIX: &str = "windows-";
/// Endung der Datei im Release.
const ASSET_EXT: &str = if cfg!(target_os = "macos") { ".dmg" } else { ".exe" };
/// Abstand zwischen zwei automatischen Prüfungen.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone)]
pub struct Update {
    pub version: Version,
    pub file_name: String,
    pub file_url: String,
    pub sha256: Option<String>,
}

pub fn repo_api() -> String {
    std::env::var("NRZX_UPDATE_REPO").unwrap_or_else(|_| DEFAULT_REPO_API.into()).trim_end_matches('/').to_string()
}

/// Server-Adresse aus der API-Adresse (`http://host:port`) – für relative Links.
fn origin(api: &str) -> String {
    match api.find("/api/") {
        Some(i) => api[..i].to_string(),
        None => api.to_string(),
    }
}

/// Laufende Version. `NRZX_APP_VERSION` überschreibt sie (zum Testen des Updaters).
pub fn current_version() -> Version {
    std::env::var("NRZX_APP_VERSION")
        .ok()
        .and_then(|v| Version::parse(v.trim()).ok())
        .unwrap_or_else(|| Version::parse(env!("CARGO_PKG_VERSION")).expect("Version"))
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("nrzX-desktop/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

/// Architektur-Kennung im Namen der DMG.
fn arch_tag() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" if cfg!(windows) => "x64",
        "x86_64" => "x86_64",
        other => other,
    }
}

/// Neueste veröffentlichte Version, wenn sie neuer als `current` ist.
pub async fn check(current: &Version) -> Result<Option<Update>, String> {
    let api = repo_api();
    let http = client()?;
    let resp = http
        .get(format!("{api}/releases?limit=100"))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("nrzGit nicht erreichbar: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("nrzGit antwortet mit {}", resp.status()));
    }
    let body: Value = resp.json().await.map_err(|e| e.to_string())?;
    let items = body.get("items").and_then(Value::as_array).or_else(|| body.as_array()).cloned().unwrap_or_default();
    let Some((version, release)) = newest(&items) else {
        return Ok(None);
    };
    if &version <= current {
        return Ok(None);
    }
    let base = origin(&api);
    let files = attachments(&release, &base);
    let Some((file_name, file_url)) = pick_dmg(&files) else {
        return Err(format!("Release {version} hat keine Datei ({ASSET_EXT}) für {}", arch_tag()));
    };
    let sha256 = match files.iter().find(|(n, _)| n.eq_ignore_ascii_case("SHA256SUMS")) {
        Some((_, url)) => {
            let text = http.get(url).timeout(Duration::from_secs(20)).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
            sum_for(&text, &file_name)
        }
        None => None,
    };
    Ok(Some(Update {
        version,
        file_name,
        file_url,
        sha256,
    }))
}

/// Höchste Version unter den veröffentlichten `macos-*`-Releases.
fn newest(items: &[Value]) -> Option<(Version, Value)> {
    items
        .iter()
        .filter(|r| !r.get("isDraft").and_then(Value::as_bool).unwrap_or(false))
        .filter(|r| !r.get("isPrerelease").and_then(Value::as_bool).unwrap_or(false))
        // Nur Releases mit passender Datei – reine Tags (z. B. aus einem Fork übernommene) zählen nicht
        .filter(|r| pick_dmg(&attachments(r, "")).is_some())
        .filter_map(|r| {
            let tag = r.get("tagName").and_then(Value::as_str)?;
            // Gemeinsames Release für beide Plattformen (`v<version>`) oder das alte je Plattform
            let v = tag.strip_prefix(TAG_PREFIX).or_else(|| tag.strip_prefix('v'))?.trim_start_matches('v');
            Some((Version::parse(v).ok()?, r.clone()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
}

/// Angehängte Dateien als (Name, absolute Adresse).
fn attachments(release: &Value, base: &str) -> Vec<(String, String)> {
    let list = release
        .get("attachments")
        .or_else(|| release.get("assets"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    list.iter()
        .filter_map(|a| {
            let name = a.get("name").and_then(Value::as_str)?.to_string();
            let url = ["browserDownloadUrl", "downloadUrl", "url", "browser_download_url"]
                .iter()
                .find_map(|k| a.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))?;
            let url = if url.starts_with('/') { format!("{base}{url}") } else { url.to_string() };
            Some((name, url))
        })
        .collect()
}

/// Datei (DMG bzw. EXE) für diese Architektur, sonst eine universelle, sonst irgendeine. Der
/// Windows-Installer (`nrzX-<version>-Setup.exe`) zählt nicht – die App ersetzt nur ihr Programm.
fn pick_dmg(files: &[(String, String)]) -> Option<(String, String)> {
    let dmgs: Vec<&(String, String)> = files
        .iter()
        .filter(|(n, _)| n.to_lowercase().ends_with(ASSET_EXT) && !n.to_lowercase().contains("setup"))
        .collect();
    let lower = |n: &str| n.to_lowercase();
    dmgs.iter()
        .find(|(n, _)| lower(n).contains(arch_tag()))
        .or_else(|| dmgs.iter().find(|(n, _)| lower(n).contains("universal")))
        .or_else(|| dmgs.first())
        .map(|f| (*f).clone())
}

/// Zeile `<sha256>  <name>` aus einer SHA256SUMS-Datei.
fn sum_for(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let sum = it.next()?;
        let file = it.next()?.trim_start_matches('*');
        (file == name && sum.len() == 64).then(|| sum.to_lowercase())
    })
}

/// Lädt die DMG in den Cache-Ordner; `progress` bekommt 0.0 … 1.0 (oder -1, wenn die Grösse
/// unbekannt ist).
pub async fn download(update: &Update, progress: impl Fn(f64)) -> Result<PathBuf, String> {
    let dir = cache_dir();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let target = dir.join(&update.file_name);
    let part = dir.join(format!("{}.part", update.file_name));
    let resp = client()?.get(&update.file_url).send().await.map_err(|e| format!("Download fehlgeschlagen: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Download fehlgeschlagen: {}", resp.status()));
    }
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut done: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download abgebrochen: {e}"))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        done += chunk.len() as u64;
        progress(total.map(|t| done as f64 / t.max(1) as f64).unwrap_or(-1.0));
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    if let Some(expected) = &update.sha256 {
        let got = hex::encode(hasher.finalize());
        if &got != expected {
            let _ = tokio::fs::remove_file(&part).await;
            return Err("Prüfsumme stimmt nicht – Update abgebrochen".into());
        }
    }
    tokio::fs::rename(&part, &target).await.map_err(|e| e.to_string())?;
    Ok(target)
}

fn cache_dir() -> PathBuf {
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("nrzX").join("updates");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library/Caches/nrzX/updates")
}

/// Bundle der laufenden App (`…/nrzX.app`), wenn sie aus einem Bundle läuft.
#[cfg(target_os = "macos")]
pub fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
    (bundle.extension().is_some_and(|e| e == "app") && exe.parent()?.ends_with("Contents/MacOS")).then_some(bundle)
}

/// Ergebnis der Installation.
pub enum Installed {
    /// Bundle ersetzt – App neu starten.
    Replaced(PathBuf),
    /// Kein Bundle oder keine Schreibrechte: DMG im Finder geöffnet, der Benutzer zieht die App selbst.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    OpenedDmg(String),
}

/// Windows: laufendes Programm umbenennen, neue EXE an seine Stelle.
#[cfg(not(target_os = "macos"))]
pub fn install(file: &Path) -> Result<Installed, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = exe.with_extension("old.exe");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("Programm lässt sich nicht ersetzen: {e}"))?;
    if let Err(e) = std::fs::copy(file, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("Neue Version lässt sich nicht einsetzen: {e}"));
    }
    let _ = std::fs::remove_file(file);
    Ok(Installed::Replaced(exe))
}

/// Windows: Rest eines Updates (umbenanntes altes Programm) wegräumen.
pub fn cleanup() {
    #[cfg(not(target_os = "macos"))]
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old.exe"));
    }
}

/// Windows: neue Version starten; sie wartet, bis diese beendet ist (`--relaunched`).
#[cfg(not(target_os = "macos"))]
pub fn relaunch_after_exit(exe: &Path) {
    let _ = Command::new(exe).arg("--relaunched").spawn();
}

/// DMG einhängen, App darin ins laufende Bundle kopieren, aushängen.
#[cfg(target_os = "macos")]
pub fn install(dmg: &Path) -> Result<Installed, String> {
    let Some(bundle) = current_bundle() else {
        open_dmg(dmg)?;
        return Ok(Installed::OpenedDmg("Die App läuft nicht aus einem Programm-Bundle – die DMG wurde geöffnet.".into()));
    };
    let parent = bundle.parent().ok_or("Bundle ohne Ordner")?.to_path_buf();

    let mount = std::env::temp_dir().join(format!("nrzx-update-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&mount);
    run(Command::new("/usr/bin/hdiutil").arg("attach").arg(dmg).args(["-nobrowse", "-readonly", "-noautoopen", "-mountpoint"]).arg(&mount))?;
    let result = (|| {
        let app = std::fs::read_dir(&mount)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or("In der DMG ist keine App")?;
        let staged = parent.join(".nrzX-update.app");
        let old = parent.join(".nrzX-old.app");
        let _ = std::fs::remove_dir_all(&staged);
        let _ = std::fs::remove_dir_all(&old);
        if let Err(e) = run(Command::new("/usr/bin/ditto").arg(&app).arg(&staged)) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(format!("Keine Schreibrechte in {}: {e}", parent.display()));
        }
        let _ = Command::new("/usr/bin/xattr").args(["-dr", "com.apple.quarantine"]).arg(&staged).status();
        std::fs::rename(&bundle, &old).map_err(|e| format!("Altes Programm lässt sich nicht verschieben: {e}"))?;
        if let Err(e) = std::fs::rename(&staged, &bundle) {
            let _ = std::fs::rename(&old, &bundle);
            return Err(format!("Neues Programm lässt sich nicht einsetzen: {e}"));
        }
        let _ = std::fs::remove_dir_all(&old);
        Ok(bundle.clone())
    })();
    let _ = Command::new("/usr/bin/hdiutil").arg("detach").arg(&mount).arg("-quiet").status();
    let _ = std::fs::remove_dir(&mount);
    match result {
        Ok(b) => {
            let _ = std::fs::remove_file(dmg);
            Ok(Installed::Replaced(b))
        }
        Err(e) => {
            open_dmg(dmg)?;
            Ok(Installed::OpenedDmg(format!("{e}\n\nDie DMG wurde geöffnet – ziehe nrzX in den Ordner Programme.")))
        }
    }
}

#[cfg(target_os = "macos")]
fn open_dmg(dmg: &Path) -> Result<(), String> {
    run(Command::new("/usr/bin/open").arg(dmg))
}

/// Startet das Bundle neu, sobald dieser Prozess beendet ist.
#[cfg(target_os = "macos")]
pub fn relaunch_after_exit(bundle: &Path) {
    let pid = std::process::id().to_string();
    let _ = Command::new("/bin/sh")
        .args(["-c", "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; /usr/bin/open \"$0\"", &bundle.to_string_lossy(), &pid])
        .spawn();
}

#[cfg(target_os = "macos")]
fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[cfg(target_os = "macos")]
    fn picks_newest_macos_release() {
        let dmg = |v: &str| json!([{"name": format!("nrzX-{v}-{}.dmg", arch_tag()), "url": format!("/d/{v}.dmg")}]);
        let items = vec![
            json!({"tagName": "umbrel-9.9.9", "isDraft": false, "isPrerelease": false, "attachments": dmg("9.9.9")}),
            json!({"tagName": "macos-1.0.6", "isDraft": false, "isPrerelease": false, "attachments": dmg("1.0.6")}),
            json!({"tagName": "macos-1.1.0", "isDraft": true, "isPrerelease": false, "attachments": dmg("1.1.0")}),
            json!({"tagName": "macos-1.0.10", "isDraft": false, "isPrerelease": false, "attachments": dmg("1.0.10")}),
            json!({"tagName": "macos-2.0.0", "isDraft": false, "isPrerelease": true, "attachments": dmg("2.0.0")}),
        ];
        assert_eq!(newest(&items).unwrap().0, Version::new(1, 0, 10));
        let mut items = items;
        items.push(json!({"tagName": "v1.0.12", "isDraft": false, "isPrerelease": false, "attachments": dmg("1.0.12")}));
        assert_eq!(newest(&items).unwrap().0, Version::new(1, 0, 12));
        // Reiner Tag (ohne Datei, z. B. aus dem Fork übernommen) mit höherer Nummer zählt nicht
        items.push(json!({"tagName": "v9.0.0", "isDraft": false, "isPrerelease": false, "isTagOnly": true, "attachments": []}));
        assert_eq!(newest(&items).unwrap().0, Version::new(1, 0, 12));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn picks_dmg_and_sum() {
        let files = vec![
            ("SHA256SUMS".to_string(), "u0".to_string()),
            ("nrzX-1.0.6-x86_64.dmg".to_string(), "u1".to_string()),
            ("nrzX-1.0.6-arm64.dmg".to_string(), "u2".to_string()),
        ];
        let (name, _) = pick_dmg(&files).unwrap();
        assert!(name.contains(arch_tag()));
        let sum = "a".repeat(64);
        assert_eq!(sum_for(&format!("{sum}  {name}\n"), &name), Some(sum));
    }

    #[test]
    fn relative_attachment_urls() {
        let r = json!({"attachments": [{"name": "a.dmg", "downloadUrl": "/x/a.dmg"}]});
        assert_eq!(attachments(&r, "http://h:1"), vec![("a.dmg".to_string(), "http://h:1/x/a.dmg".to_string())]);
    }
}
