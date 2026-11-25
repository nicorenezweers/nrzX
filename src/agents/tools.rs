//! Werkzeuge: die Agent-CLIs (npm-Pakete) und die Docker CLI. nrzX installiert sie in den
//! Datenordner – CLIs mit `npm install -g --prefix <data>/tools/npm`, Docker als statisches
//! Programm von download.docker.com nach `<data>/tools/bin` – und sucht Programme zuerst dort,
//! dann im `PATH`. Je Agent-Art lässt sich der Pfad mit `NRZX_BIN_<ART>` überschreiben
//! (z. B. `NRZX_BIN_CLAUDE=/pfad/zu/claude`, für Tests mit nachgeahmten CLIs).
//!
//! Fehlt npm, lädt nrzX zuerst Node.js (neueste LTS, Prüfsumme aus `SHASUMS256.txt`) von
//! nodejs.org nach `<data>/tools/node`; Agenten bekommen es im `PATH`.
//!
//! Installationen laufen im Hintergrund; ihr Protokoll kommt live über
//! `GET /tools/<key>/stream` (Server-Sent Events).

use super::kinds::{spec, SPECS};
use super::runtime;
use crate::auth::{AdminUser, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::{ToolInfo, ToolLatest};
use nrzx_shared::{AgentKind, HubEvent};
use rocket::response::stream::{Event, EventStream};
use rocket::serde::json::Json;
use rocket::{get, post, routes, Route, Shutdown, State};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast;

pub fn routes() -> Vec<Route> {
    routes![list, latest, install, stream]
}

pub const DOCKER: &str = "docker";

// ------------------------------------------------------------------ Programme finden

/// Ist das ein ausführbares Programm?
fn executable(p: &Path) -> bool {
    crate::platform::executable(p)
}

/// Programm im `PATH` des Servers suchen.
pub fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names = crate::platform::program_names(program);
    std::env::split_paths(&path).flat_map(|d| names.iter().map(move |n| d.join(n)).collect::<Vec<_>>()).find(|p| executable(p))
}

/// Umgebungsvariable zum Überschreiben, z. B. `NRZX_BIN_CLAUDE`.
pub fn env_override(key: &str) -> String {
    format!("NRZX_BIN_{}", key.to_uppercase())
}

/// Programm suchen: `NRZX_BIN_<KEY>`, `tools/npm/bin`, `tools/bin`, `PATH`.
/// Liefert Pfad und Herkunft ("env" | "nrzx" | "path").
pub fn find(state: &AppState, key: &str, programs: &[&str]) -> Option<(PathBuf, &'static str)> {
    if let Ok(v) = std::env::var(env_override(key)) {
        let v = v.trim();
        if !v.is_empty() {
            let p = PathBuf::from(v);
            if p.is_absolute() {
                return executable(&p).then_some((p, "env"));
            }
            return which(v).map(|p| (p, "env"));
        }
    }
    if let Some(p) = find_in_tools(&state.config.tools_dir(), programs) {
        return Some((p, "nrzx"));
    }
    programs.iter().find_map(|p| which(p)).map(|p| (p, "path"))
}

/// Programm nur unter den von nrzX installierten Werkzeugen suchen.
fn find_in_tools(tools: &Path, programs: &[&str]) -> Option<PathBuf> {
    // Windows: npm legt die Programme direkt in den Prefix (ohne bin/)
    for dir in [tools.join("npm").join("bin"), tools.join("npm"), tools.join("bin")] {
        for prog in programs {
            for name in crate::platform::program_names(prog) {
                let p = dir.join(name);
                if executable(&p) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Programm einer Agent-Art.
pub fn find_agent(state: &AppState, kind: AgentKind) -> Option<(PathBuf, &'static str)> {
    find(state, kind.key(), spec(kind).programs)
}

/// Version eines Programms (`--version`, eine Minute zwischengespeichert).
pub async fn version_of(state: &AppState, bin: &Path) -> String {
    let rt = runtime::get(state);
    if let Some((at, v)) = rt.versions.lock().unwrap_or_else(|e| e.into_inner()).get(bin) {
        if at.elapsed() < Duration::from_secs(60) {
            return v.clone();
        }
    }
    let mut cmd = crate::platform::command(bin);
    cmd.arg("--version").stdin(Stdio::null()).kill_on_drop(true);
    super::env::apply_base(&mut cmd, state);
    let v = match tokio::time::timeout(Duration::from_secs(15), cmd.output()).await {
        Ok(Ok(o)) => {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            extract_version(&text)
        }
        _ => String::new(),
    };
    rt.versions.lock().unwrap_or_else(|e| e.into_inner()).insert(bin.to_path_buf(), (Instant::now(), v.clone()));
    v
}

/// "2.1.316 (Claude Code)" → "2.1.316", "Docker version 29.9.2, build x" → "29.9.2".
pub fn extract_version(text: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\d+\.\d+(?:\.\d+)?(?:[-.][0-9A-Za-z.]+)?").unwrap());
    RE.find(text).map(|m| m.as_str().trim_end_matches([',', '.']).to_string()).unwrap_or_default()
}

fn forget_versions(state: &AppState) {
    runtime::get(state).versions.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

// ------------------------------------------------------------------ Liste

struct ToolDef {
    key: &'static str,
    name: &'static str,
    programs: &'static [&'static str],
    package: &'static str,
}

fn defs() -> Vec<ToolDef> {
    let mut v: Vec<ToolDef> = SPECS
        .iter()
        .map(|s| ToolDef { key: s.kind.key(), name: s.kind.name(), programs: s.programs, package: s.package })
        .collect();
    v.push(ToolDef { key: DOCKER, name: "Docker CLI", programs: &["docker"], package: "download.docker.com" });
    v
}

async fn info(state: &AppState, def: &ToolDef) -> ToolInfo {
    let rt = runtime::get(state);
    let found = find(state, def.key, def.programs);
    let version = match &found {
        Some((p, _)) => version_of(state, p).await,
        None => String::new(),
    };
    let job = rt.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(def.key).cloned();
    let latest = rt.latest.lock().unwrap_or_else(|e| e.into_inner()).get(def.key).cloned().unwrap_or_default();
    ToolInfo {
        key: def.key.into(),
        name: def.name.into(),
        program: def.programs[0].into(),
        package: def.package.into(),
        installed: found.is_some(),
        version,
        path: found.as_ref().map(|(p, _)| p.to_string_lossy().into_owned()).unwrap_or_default(),
        source: found.map(|(_, s)| s.to_string()).unwrap_or_default(),
        installing: job.as_ref().is_some_and(|j| !j.done.load(Ordering::Relaxed)),
        latest,
        error: job.map(|j| j.error.lock().unwrap_or_else(|e| e.into_inner()).clone()).unwrap_or_default(),
    }
}

/// `GET /tools` – CLIs und Docker mit Version und Herkunft.
#[get("/tools")]
async fn list(state: &State<AppState>, _user: CurrentUser) -> Json<Vec<ToolInfo>> {
    let mut out = Vec::new();
    for d in defs() {
        out.push(info(state, &d).await);
    }
    Json(out)
}

/// `GET /tools/latest` – neueste Versionen (npm bzw. download.docker.com).
#[get("/tools/latest")]
async fn latest(state: &State<AppState>, _user: CurrentUser) -> Json<Vec<ToolLatest>> {
    let mut tasks = Vec::new();
    for d in defs() {
        let st = state.inner().clone();
        tasks.push(tokio::spawn(async move {
            let v = if d.key == DOCKER {
                docker_latest().await.map(|(v, _)| v).unwrap_or_default()
            } else {
                npm_latest(&st, d.package).await
            };
            (d.key, v)
        }));
    }
    let rt = runtime::get(state);
    let mut out = Vec::new();
    for t in tasks {
        if let Ok((key, v)) = t.await {
            if !v.is_empty() {
                rt.latest.lock().unwrap_or_else(|e| e.into_inner()).insert(key.to_string(), v.clone());
            }
            out.push(ToolLatest { key: key.into(), latest: v });
        }
    }
    Json(out)
}

async fn npm_latest(state: &AppState, package: &str) -> String {
    let Some(npm) = npm_program(state) else { return String::new() };
    let mut cmd = crate::platform::command(npm);
    cmd.args(["view", package, "version"]).stdin(Stdio::null()).kill_on_drop(true);
    super::env::apply_base(&mut cmd, state);
    match tokio::time::timeout(Duration::from_secs(30), cmd.output()).await {
        Ok(Ok(o)) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

// ------------------------------------------------------------------ Installieren

#[derive(Clone, Debug)]
pub enum JobEvent {
    Line(String),
    Done(bool),
}

/// Eine laufende oder beendete Installation.
pub struct Job {
    pub key: String,
    pub log: Mutex<String>,
    pub tx: broadcast::Sender<JobEvent>,
    pub done: AtomicBool,
    pub ok: AtomicBool,
    pub error: Mutex<String>,
}

impl Job {
    fn line(&self, text: &str) {
        let text = text.trim_end();
        {
            let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
            log.push_str(text);
            log.push('\n');
            // Protokoll begrenzen
            if log.len() > 200_000 {
                let cut = log.len() - 150_000;
                let cut = (cut..log.len()).find(|i| log.is_char_boundary(*i)).unwrap_or(cut);
                log.replace_range(..cut, "");
            }
        }
        let _ = self.tx.send(JobEvent::Line(text.to_string()));
    }

    fn finish(&self, ok: bool, error: &str) {
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = error.to_string();
        self.ok.store(ok, Ordering::Relaxed);
        self.done.store(true, Ordering::Relaxed);
        let _ = self.tx.send(JobEvent::Done(ok));
    }
}

/// `POST /tools/<key>/install` – installieren bzw. auf die neueste Version aktualisieren
/// (nur Administratoren). Fortschritt über `GET /tools/<key>/stream`.
#[post("/tools/<key>/install")]
async fn install(state: &State<AppState>, admin: AdminUser, key: &str) -> AppResult<Json<ToolInfo>> {
    let def = defs().into_iter().find(|d| d.key == key).ok_or(AppError::NotFound)?;
    let rt = runtime::get(state);
    let job = {
        let mut jobs = rt.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.get(key).is_some_and(|j| !j.done.load(Ordering::Relaxed)) {
            return Err(AppError::conflict("Die Installation läuft bereits."));
        }
        let job = Arc::new(Job {
            key: key.to_string(),
            log: Mutex::new(String::new()),
            tx: broadcast::channel(1024).0,
            done: AtomicBool::new(false),
            ok: AtomicBool::new(false),
            error: Mutex::new(String::new()),
        });
        jobs.insert(key.to_string(), job.clone());
        job
    };
    let st = state.inner().clone();
    let user_id = admin.0.id;
    let package = def.package;
    let programs = def.programs;
    let name = def.name;
    let is_docker = def.key == DOCKER;
    tokio::spawn(async move {
        let result = if is_docker { install_docker(&st, &job).await } else { install_npm(&st, &job, package, programs).await };
        forget_versions(&st);
        match result {
            Ok(()) => {
                job.line("✓ Fertig.");
                job.finish(true, "");
                crate::hub::publish(&st, user_id, HubEvent::Notice { text: format!("{name} ist installiert."), error: false });
            }
            Err(e) => {
                job.line(&format!("✗ {e}"));
                job.finish(false, &e);
                crate::hub::publish(&st, user_id, HubEvent::Notice { text: format!("{name}: {e}"), error: true });
            }
        }
    });
    Ok(Json(info(state, &def).await))
}

async fn install_npm(state: &AppState, job: &Job, package: &str, programs: &[&str]) -> Result<(), String> {
    let npm = match npm_program(state) {
        Some(npm) => npm,
        None => install_node(state, job).await?,
    };
    let prefix = state.config.tools_dir().join("npm");
    tokio::fs::create_dir_all(&prefix).await.map_err(|e| e.to_string())?;
    // Das alte Paket legt ebenfalls `grok` ab und kennt `login --device-auth` nicht.
    if package == "@xai-official/grok" {
        npm_retire(state, job, &npm, &prefix, "@vibe-kit/grok-cli").await;
    }
    let spec = format!("{package}@latest");
    let args = [
        "install".to_string(),
        "-g".into(),
        "--prefix".into(),
        prefix.to_string_lossy().into_owned(),
        "--no-fund".into(),
        "--no-audit".into(),
        "--loglevel=http".into(),
        // npm ≥ 11 führt Install-Skripte nur für erlaubte Pakete aus (Claude, OpenCode und
        // Kilo legen dort ihr Programm für die Plattform ab)
        format!("--allow-scripts={package}"),
        spec.clone(),
    ];
    job.line(&format!("$ npm install -g --prefix {} {spec}", prefix.display()));
    let mut cmd = crate::platform::command(npm);
    cmd.args(&args)
        .current_dir(state.config.tools_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    super::env::apply_base(&mut cmd, state);
    cmd.env("npm_config_update_notifier", "false").env("NO_COLOR", "1");
    let mut child = cmd.spawn().map_err(|e| format!("npm startet nicht: {e}"))?;
    let out = child.stdout.take();
    let err = child.stderr.take();
    let pump = |r: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>| async move {
        let Some(r) = r else { return };
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            job.line(&l);
        }
    };
    let (_, _, status) = tokio::join!(
        pump(out.map(|o| Box::new(o) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)),
        pump(err.map(|o| Box::new(o) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)),
        child.wait()
    );
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => return Err(format!("npm endete mit Code {}.", s.code().unwrap_or(-1))),
        Err(e) => return Err(e.to_string()),
    }
    // npm kann Erfolg melden, ohne dass das Programm da ist – nachsehen statt „Fertig“ zu melden
    let bin = find_in_tools(&state.config.tools_dir(), programs)
        .ok_or_else(|| format!("npm meldet Erfolg, aber {} fehlt in {}.", programs[0], prefix.display()))?;
    job.line(&format!("Installiert: {}", bin.display()));
    // Version frisch abfragen, nicht aus der Zeit vor der Installation
    forget_versions(state);
    if version_of(state, &bin).await.is_empty() {
        job.line(&format!("Achtung: {} antwortet nicht auf --version – startet das Programm?", programs[0]));
    }
    if package == "@xai-official/grok" && legacy_grok(&bin) {
        return Err("grok zeigt nach der Installation noch auf @vibe-kit/grok-cli.".into());
    }
    Ok(())
}

/// Paket unter dem npm-Prefix entfernen. Fehler hier stoppen die folgende Installation nicht:
/// fehlt es, ist das der Normalfall.
async fn npm_retire(state: &AppState, job: &Job, npm: &Path, prefix: &Path, package: &str) {
    job.line(&format!("$ npm uninstall -g --prefix {} {package}", prefix.display()));
    let mut cmd = crate::platform::command(npm);
    cmd.args(["uninstall", "-g", "--prefix"]).arg(prefix).arg(package).current_dir(state.config.tools_dir()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    super::env::apply_base(&mut cmd, state);
    cmd.env("npm_config_update_notifier", "false").env("NO_COLOR", "1");
    match tokio::time::timeout(Duration::from_secs(120), cmd.output()).await {
        Ok(Ok(out)) => {
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                job.line(line);
            }
        }
        Ok(Err(e)) => job.line(&format!("Altes Paket nicht entfernt: {e}")),
        Err(_) => job.line("Altes Paket: Zeitüberschreitung, die Installation läuft weiter."),
    }
}

/// Das abgeschaltete `@vibe-kit/grok-cli`. Es hat kein `login --device-auth`.
pub fn legacy_grok(bin: &Path) -> bool {
    let path = bin.canonicalize().unwrap_or_else(|_| bin.to_path_buf());
    legacy_grok_text(&path.to_string_lossy())
}

fn legacy_grok_text(path: &str) -> bool {
    path.contains("@vibe-kit/grok-cli")
}

// ------------------------------------------------------------------ Node.js

const NODE_DIST: &str = "https://nodejs.org/dist";

/// Nur eine Node.js-Installation zugleich (mehrere CLIs können gleichzeitig installiert werden).
static NODE_INSTALL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Ordner mit `node` und `npm` des von nrzX installierten Node.js: `tools/node`, unter Unix
/// `tools/node/bin`.
pub fn own_node_dir(tools: &Path) -> PathBuf {
    if cfg!(windows) {
        tools.join("node")
    } else {
        tools.join("node").join("bin")
    }
}

/// npm für Installationen: aus dem `PATH` des Servers, sonst das von nrzX installierte.
fn npm_program(state: &AppState) -> Option<PathBuf> {
    which("npm").or_else(|| {
        let dir = own_node_dir(&state.config.tools_dir());
        crate::platform::program_names("npm").into_iter().map(|n| dir.join(n)).find(|p| executable(p))
    })
}

/// Node.js-Build für dieses System: Schlüssel in `index.json` (`files`) und Ende des Dateinamens.
fn node_build() -> Result<(String, String), String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => return Err(format!("Kein Node.js für die Architektur {other}.")),
    };
    Ok(match std::env::consts::OS {
        "windows" => (format!("win-{arch}-zip"), format!("win-{arch}.zip")),
        "macos" => (format!("osx-{arch}-tar"), format!("darwin-{arch}.tar.gz")),
        "linux" => (format!("linux-{arch}"), format!("linux-{arch}.tar.gz")),
        other => return Err(format!("Kein Node.js für {other}.")),
    })
}

/// Neueste LTS-Version mit einem Build `key` aus `index.json` (neueste Versionen zuerst).
pub fn newest_node_lts(index: &serde_json::Value, key: &str) -> Option<String> {
    index
        .as_array()?
        .iter()
        .find(|r| r["lts"].is_string() && r["files"].as_array().is_some_and(|f| f.iter().any(|x| x.as_str() == Some(key))))
        .and_then(|r| r["version"].as_str())
        .map(str::to_string)
}

/// SHA-256 einer Datei aus `SHASUMS256.txt` (`<hash>  <datei>`).
pub fn sha256_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, name) = l.trim().split_once(char::is_whitespace)?;
        (name.trim() == file).then(|| hash.to_ascii_lowercase())
    })
}

/// Text von einer Adresse (Index, Prüfsummen, Verzeichnisliste).
async fn fetch_text(url: &str) -> Result<String, String> {
    reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("{url} nicht erreichbar: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())
}

/// Node.js (neueste LTS) von nodejs.org nach `tools/node` – wenn npm fehlt. Liefert npm.
async fn install_node(state: &AppState, job: &Job) -> Result<PathBuf, String> {
    let _guard = match NODE_INSTALL.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            job.line("Warte auf die laufende Node.js-Installation …");
            NODE_INSTALL.lock().await
        }
    };
    // Inzwischen von einer anderen Installation geholt?
    if let Some(npm) = npm_program(state) {
        return Ok(npm);
    }
    job.line("npm fehlt – nrzX installiert zuerst Node.js (LTS) von nodejs.org.");
    let (key, suffix) = node_build()?;
    let index: serde_json::Value =
        serde_json::from_str(&fetch_text(&format!("{NODE_DIST}/index.json")).await?).map_err(|e| format!("index.json: {e}"))?;
    let version = newest_node_lts(&index, &key).ok_or_else(|| format!("Keine Node.js-Version für {key} gefunden."))?;
    let file = format!("node-{version}-{suffix}");
    let sums = fetch_text(&format!("{NODE_DIST}/{version}/SHASUMS256.txt")).await?;
    let expected = sha256_for(&sums, &file).ok_or_else(|| format!("Keine Prüfsumme für {file}."))?;
    let url = format!("{NODE_DIST}/{version}/{file}");
    job.line(&format!("Node.js {version}: {url}"));
    let tmp_dir = state.config.tmp_dir();
    tokio::fs::create_dir_all(&tmp_dir).await.map_err(|e| e.to_string())?;
    let archive = tmp_dir.join(format!("{}-{file}", uuid::Uuid::new_v4().simple()));
    let result = match download(job, &url, &archive).await {
        Ok(hash) if hash == expected => {
            job.line("Prüfsumme stimmt. Entpacken …");
            let (a, tools) = (archive.clone(), state.config.tools_dir());
            tokio::task::spawn_blocking(move || extract_node(&a, &tools)).await.map_err(|e| e.to_string()).and_then(|r| r)
        }
        Ok(_) => Err(format!("Prüfsumme von {file} stimmt nicht – Download verworfen.")),
        Err(e) => Err(e),
    };
    let _ = tokio::fs::remove_file(&archive).await;
    result?;
    let npm = npm_program(state).ok_or_else(|| "Node.js ist entpackt, aber npm fehlt.".to_string())?;
    job.line(&format!("Node.js {version} installiert: {}", own_node_dir(&state.config.tools_dir()).display()));
    Ok(npm)
}

/// Node.js-Archiv (ein oberster Ordner `node-v…`) nach `tools/node` entpacken: erst nach
/// `tools/node.partial`, dann umbenennen – ein abgebrochenes Entpacken hinterlässt kein halbes
/// Node.js.
fn extract_node(archive: &Path, tools: &Path) -> Result<(), String> {
    let partial = tools.join("node.partial");
    let _ = std::fs::remove_dir_all(&partial);
    let result = (|| {
        std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
        unpack(archive, &partial)?;
        let root = std::fs::read_dir(&partial)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.is_dir())
            .ok_or_else(|| "Archiv ohne Ordner.".to_string())?;
        let target = tools.join("node");
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        }
        rename_retry(&root, &target)
    })();
    let _ = std::fs::remove_dir_all(&partial);
    result
}

/// ZIP (Windows) oder tar.gz (sonst) in einen Ordner entpacken; Pfade, die aus dem Ordner
/// hinausführen, lehnen beide Bibliotheken ab.
fn unpack(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    if archive.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
        zip::ZipArchive::new(file).and_then(|mut z| z.extract(dest)).map_err(|e| format!("Entpacken: {e}"))
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(file)).unpack(dest).map_err(|e| format!("Entpacken: {e}"))
    }
}

/// Umbenennen; unter Windows halten Virenscanner frisch entpackte Dateien kurz offen, darum ein
/// paar Versuche.
fn rename_retry(from: &Path, to: &Path) -> Result<(), String> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if tries < 20 && cfg!(windows) => {
                tries += 1;
                tracing::debug!("Umbenennen nach {} (Versuch {tries}): {e}", to.display());
                std::thread::sleep(Duration::from_millis(300));
            }
            Err(e) => return Err(format!("{} → {}: {e}", from.display(), to.display())),
        }
    }
}

/// Datei mit Fortschritt im Protokoll herunterladen; liefert die SHA-256-Prüfsumme (hex).
async fn download(job: &Job, url: &str, dest: &Path) -> Result<String, String> {
    use futures::StreamExt;
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;
    let res = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(900))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Download fehlgeschlagen: {e}"))?;
    let total = res.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(dest).await.map_err(|e| e.to_string())?;
    let mut hash = sha2::Sha256::new();
    let (mut done, mut next_report) = (0u64, 0u64);
    let mut body = res.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| format!("Download abgebrochen: {e}"))?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        hash.update(&chunk);
        done += chunk.len() as u64;
        if total > 0 && done * 100 / total >= next_report {
            job.line(&format!("… {} % ({} MB)", done * 100 / total, done / 1_048_576));
            next_report = done * 100 / total + 10;
        }
    }
    // Erst wenn alles geschrieben ist, darf jemand die Datei lesen
    file.flush().await.map_err(|e| e.to_string())?;
    Ok(hex::encode(hash.finalize()))
}

// ------------------------------------------------------------------ Docker

/// Statische Docker-Programme für dieses System: Basisadresse und Endung der Archive (Windows:
/// ZIP mit `docker/docker.exe`, nur x86_64 – läuft auch auf Windows für ARM).
fn docker_base() -> Result<(String, &'static str), String> {
    if cfg!(windows) {
        return Ok(("https://download.docker.com/win/static/stable/x86_64/".into(), "zip"));
    }
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => return Err(format!("Keine Docker CLI für die Architektur {other}.")),
    };
    let os = if cfg!(target_os = "macos") { "mac" } else { "linux" };
    Ok((format!("https://download.docker.com/{os}/static/stable/{arch}/"), "tgz"))
}

/// Neueste Version (aus der Verzeichnisliste) und Adresse des Archivs.
async fn docker_latest() -> Result<(String, String), String> {
    let (base, ext) = docker_base()?;
    let html = fetch_text(&base).await?;
    let version = newest_docker_version(&html, ext).ok_or_else(|| "Keine Docker-Version gefunden.".to_string())?;
    Ok((version.clone(), format!("{base}docker-{version}.{ext}")))
}

/// Höchste Version aus `href="docker-29.9.2.tgz"` (bzw. `.zip`).
pub fn newest_docker_version(html: &str, ext: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r"docker-(\d+)\.(\d+)\.(\d+)\.{}", regex::escape(ext))).ok()?;
    re.captures_iter(html)
        .filter_map(|c| Some((c[1].parse::<u32>().ok()?, c[2].parse::<u32>().ok()?, c[3].parse::<u32>().ok()?)))
        .max()
        .map(|(a, b, c)| format!("{a}.{b}.{c}"))
}

async fn install_docker(state: &AppState, job: &Job) -> Result<(), String> {
    let (version, url) = docker_latest().await?;
    job.line(&format!("Docker CLI {version}: {url}"));
    let tmp_dir = state.config.tmp_dir();
    tokio::fs::create_dir_all(&tmp_dir).await.map_err(|e| e.to_string())?;
    let ext = if url.ends_with(".zip") { "zip" } else { "tgz" };
    let tmp = tmp_dir.join(format!("docker-{}.{ext}", uuid::Uuid::new_v4()));
    let result = match download(job, &url, &tmp).await {
        Ok(_) => {
            job.line("Entpacken …");
            let (a, bin_dir) = (tmp.clone(), state.config.tools_dir().join("bin"));
            tokio::task::spawn_blocking(move || extract_docker(&a, &bin_dir)).await.map_err(|e| e.to_string()).and_then(|r| r)
        }
        Err(e) => Err(e),
    };
    let _ = tokio::fs::remove_file(&tmp).await;
    job.line(&format!("Installiert: {}", result?.display()));
    Ok(())
}

/// Docker CLI aus dem Archiv nach `tools/bin`: `docker/docker` aus dem tgz bzw.
/// `docker/docker.exe` aus dem ZIP (Windows). Liefert den Pfad.
fn extract_docker(archive: &Path, bin_dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(bin_dir).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    if archive.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
        let target = bin_dir.join("docker.exe");
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let mut entry = zip.by_name("docker/docker.exe").map_err(|_| "Archiv enthält kein docker/docker.exe.".to_string())?;
        replace_file(&mut entry, &target)?;
        return Ok(target);
    }
    let target = bin_dir.join("docker");
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in ar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
        if path.trim_start_matches("./") == "docker/docker" {
            replace_file(&mut entry, &target)?;
            return Ok(target);
        }
    }
    Err("Archiv enthält kein docker/docker.".into())
}

/// Programm ersetzen: erst daneben schreiben, dann umbenennen. Unter Windows lässt sich ein
/// laufendes Programm nicht überschreiben, aber umbenennen – das alte wird zu `*.old`.
fn replace_file(from: &mut impl std::io::Read, target: &Path) -> Result<(), String> {
    let partial = target.with_extension("partial");
    let mut out = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
    std::io::copy(from, &mut out).map_err(|e| e.to_string())?;
    drop(out);
    crate::platform::make_executable(&partial).map_err(|e| e.to_string())?;
    if cfg!(windows) && target.exists() {
        let old = target.with_extension("old");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(target, &old);
    }
    std::fs::rename(&partial, target).map_err(|e| e.to_string())
}

/// `GET /tools/<key>/stream` – Protokoll der Installation (bisheriges Protokoll, dann live).
/// Ereignisse: Daten = eine Zeile; `done` mit "ok" bzw. "error" am Ende.
#[get("/tools/<key>/stream")]
fn stream(state: &State<AppState>, _user: CurrentUser, key: &str, mut end: Shutdown) -> EventStream![Event + 'static] {
    let job = runtime::get(state).jobs.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned();
    EventStream! {
        let Some(job) = job else {
            yield Event::data("none").event("done");
            return;
        };
        let mut rx = job.tx.subscribe();
        let log = job.log.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for line in log.lines() {
            yield Event::data(line.to_string());
        }
        if job.done.load(Ordering::Relaxed) {
            yield Event::data(if job.ok.load(Ordering::Relaxed) { "ok" } else { "error" }).event("done");
            return;
        }
        loop {
            rocket::tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(JobEvent::Line(l)) => yield Event::data(l),
                    Ok(JobEvent::Done(ok)) => {
                        yield Event::data(if ok { "ok" } else { "error" }).event("done");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                },
                _ = &mut end => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_package() {
        assert!(super::legacy_grok_text("/opt/npm/lib/node_modules/@vibe-kit/grok-cli/dist/index.js"));
        assert!(!super::legacy_grok_text("/opt/npm/lib/node_modules/@xai-official/grok/bin/grok"));
    }

    #[test]
    fn versions() {
        assert_eq!(super::extract_version("2.1.316 (Claude Code)"), "2.1.316");
        assert_eq!(super::extract_version("codex-cli 0.166.0\n"), "0.166.0");
        assert_eq!(super::extract_version("Docker version 29.9.2, build abc"), "29.9.2");
        let html = r#"<a href="docker-29.9.0.tgz"> <a href="docker-29.10.1.tgz"> <a href="docker-3.0.0.tgz"> <a href="docker-30.0.0.zip">"#;
        assert_eq!(super::newest_docker_version(html, "tgz").as_deref(), Some("29.10.1"));
        assert_eq!(super::newest_docker_version(html, "zip").as_deref(), Some("30.0.0"));
    }

    #[test]
    fn node_release() {
        let index = serde_json::json!([
            {"version": "v27.0.0", "lts": false, "files": ["win-x64-zip", "osx-arm64-tar"]},
            {"version": "v26.13.0", "lts": "Lithium", "files": ["linux-x64", "osx-arm64-tar"]},
            {"version": "v24.12.0", "lts": "Krypton", "files": ["linux-x64", "osx-arm64-tar", "win-x64-zip"]},
        ]);
        assert_eq!(super::newest_node_lts(&index, "osx-arm64-tar").as_deref(), Some("v26.13.0"));
        assert_eq!(super::newest_node_lts(&index, "win-x64-zip").as_deref(), Some("v24.12.0"));
        assert_eq!(super::newest_node_lts(&index, "aix-ppc64"), None);
        let sums = "d38bd5e5  node-v26.13.0-darwin-arm64.tar.gz\nDCC60D59  node-v26.13.0-win-x64.zip\n";
        assert_eq!(super::sha256_for(sums, "node-v26.13.0-win-x64.zip").as_deref(), Some("dcc60d59"));
        assert_eq!(super::sha256_for(sums, "node-v26.13.0-win-x64.7z"), None);
        let (key, suffix) = super::node_build().unwrap();
        assert!(suffix.ends_with(if cfg!(windows) { ".zip" } else { ".tar.gz" }), "{key} {suffix}");
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("nrzx-tools-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_zip(path: &std::path::Path, files: &[(&str, &[u8])]) {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    /// Node.js aus dem ZIP (Windows) und dem tar.gz (macOS/Linux, mit Symlink `bin/npm`) landet
    /// ohne den obersten Ordner in `tools/node`; ein altes, kaputtes `tools/node` wird ersetzt.
    #[test]
    fn node_archives() {
        let dir = tmp("node");
        let tools = dir.join("tools");
        std::fs::create_dir_all(tools.join("node")).unwrap();
        std::fs::write(tools.join("node").join("kaputt"), "").unwrap();
        let zip = dir.join("node-v26.13.0-win-x64.zip");
        write_zip(&zip, &[("node-v26.13.0-win-x64/node.exe", b"MZ"), ("node-v26.13.0-win-x64/node_modules/npm/bin/npm-cli.js", b"//")]);
        super::extract_node(&zip, &tools).unwrap();
        assert!(tools.join("node").join("node.exe").is_file());
        assert!(tools.join("node").join("node_modules").join("npm").join("bin").join("npm-cli.js").is_file());
        assert!(!tools.join("node").join("kaputt").exists());
        assert!(!tools.join("node.partial").exists());

        #[cfg(unix)]
        {
            let tgz = dir.join("node-v26.13.0-darwin-arm64.tar.gz");
            let mut b = tar::Builder::new(flate2::write::GzEncoder::new(std::fs::File::create(&tgz).unwrap(), flate2::Compression::fast()));
            let mut h = tar::Header::new_gnu();
            h.set_size(2);
            h.set_mode(0o755);
            b.append_data(&mut h, "node-v26.13.0-darwin-arm64/lib/node_modules/npm/bin/npm-cli.js", &b"//"[..]).unwrap();
            let mut l = tar::Header::new_gnu();
            l.set_entry_type(tar::EntryType::Symlink);
            l.set_size(0);
            b.append_link(&mut l, "node-v26.13.0-darwin-arm64/bin/npm", "../lib/node_modules/npm/bin/npm-cli.js").unwrap();
            b.into_inner().unwrap().finish().unwrap();
            super::extract_node(&tgz, &tools).unwrap();
            let npm = super::own_node_dir(&tools).join("npm");
            assert!(crate::platform::executable(&npm), "{}", npm.display());
            assert!(!tools.join("node").join("node.exe").exists());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Docker CLI: `docker/docker.exe` aus dem ZIP für Windows, `docker/docker` aus dem tgz.
    #[test]
    fn docker_archives() {
        let dir = tmp("docker");
        let bin = dir.join("bin");
        let zip = dir.join("docker-29.9.3.zip");
        write_zip(&zip, &[("docker/dockerd.exe", b"daemon"), ("docker/docker.exe", b"MZcli")]);
        assert_eq!(super::extract_docker(&zip, &bin).unwrap(), bin.join("docker.exe"));
        assert_eq!(std::fs::read(bin.join("docker.exe")).unwrap(), b"MZcli");
        assert!(!bin.join("dockerd.exe").exists());
        let empty = dir.join("leer.zip");
        write_zip(&empty, &[("docker/dockerd.exe", b"daemon")]);
        assert!(super::extract_docker(&empty, &bin).is_err());

        let tgz = dir.join("docker-29.9.3.tgz");
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(std::fs::File::create(&tgz).unwrap(), flate2::Compression::fast()));
        let mut h = tar::Header::new_gnu();
        h.set_size(3);
        h.set_mode(0o644);
        b.append_data(&mut h, "docker/docker", &b"cli"[..]).unwrap();
        b.into_inner().unwrap().finish().unwrap();
        assert_eq!(super::extract_docker(&tgz, &bin).unwrap(), bin.join("docker"));
        assert!(crate::platform::executable(&bin.join("docker")) || cfg!(windows));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
