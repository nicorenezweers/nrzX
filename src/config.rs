//! Einstellungen. Quelle ist Rockets Figment: `Rocket.toml` und Umgebungsvariablen mit
//! dem Präfix `NRZX_` (z. B. `NRZX_DATA_DIR`, `NRZX_PREVIEW_PORT`).

use rocket::figment::providers::{Env, Format, Toml};
use rocket::figment::Figment;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Datenordner: Datenbank, Agent-Konten, Arbeitsbereiche, Werkzeuge.
    pub data_dir: PathBuf,
    /// Öffentliche Adresse, z. B. "http://umbrel.local:9300" (leer = aus der Anfrage).
    pub root_url: String,
    /// Port des Vorschau-Proxys (zeigt Server der Agenten im Browser), 0 = aus.
    pub preview_port: u16,
    /// Cookies nur über HTTPS senden – einschalten, sobald nrzX hinter HTTPS läuft.
    pub cookie_secure: bool,
    /// Hintergrundarbeiten starten. Tests schalten sie aus.
    pub background: bool,
    /// Programm `git` (Pfad oder Name im PATH).
    pub git_binary: String,
    /// Wie viele Agenten gleichzeitig laufen dürfen (je Instanz).
    pub max_parallel_runs: usize,
    /// Vorgabe für die nrzGit-Adresse (z. B. im Umbrel-Netz), wenn der Benutzer keine einträgt.
    pub nrzgit_url: String,
    /// Desktop-Modus (macOS-App): Token für die Anmeldung ohne Passwort (`/__desktop/login`).
    /// Leer = aus.
    pub desktop_token: String,
    /// Desktop-Modus: Benutzername und voller Name für das lokale Konto beim ersten Start.
    pub desktop_user: String,
    pub desktop_full_name: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("./data"),
            root_url: String::new(),
            preview_port: 9301,
            cookie_secure: false,
            background: true,
            git_binary: "git".into(),
            max_parallel_runs: 8,
            nrzgit_url: String::new(),
            desktop_token: String::new(),
            desktop_user: String::new(),
            desktop_full_name: String::new(),
        }
    }
}

impl Config {
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("nrzx.db")
    }

    /// `<data>/accounts/<id>` – je Agent-Konto ein eigenes Home (`home/`).
    pub fn accounts_dir(&self) -> PathBuf {
        self.data_dir.join("accounts")
    }

    /// `<data>/workspaces/<projekt-id>` – Repository (`repo/`) und Worktrees (`worktrees/<thread>`).
    pub fn workspaces_dir(&self) -> PathBuf {
        self.data_dir.join("workspaces")
    }

    /// `<data>/scratch/<thread-id>` – Arbeitsordner für Threads ohne Projekt.
    pub fn scratch_dir(&self) -> PathBuf {
        self.data_dir.join("scratch")
    }

    /// `<data>/tools` – installierte Agent-CLIs (npm-Präfix `tools/npm`) und weitere Werkzeuge.
    pub fn tools_dir(&self) -> PathBuf {
        self.data_dir.join("tools")
    }

    /// `<data>/attachments` – angehängte Dateien (Bilder im Composer).
    pub fn attachments_dir(&self) -> PathBuf {
        self.data_dir.join("attachments")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.data_dir.join("tmp")
    }

    /// Heruntergeladene Modelle (Spracherkennung): `models/whisper-<grösse>/`.
    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }
}

/// Rocket-Konfiguration plus `NRZX_*`-Umgebung (statt `ROCKET_*`).
pub fn figment() -> Figment {
    Figment::from(rocket::Config::default())
        .merge(Toml::file(Env::var_or("NRZX_CONFIG", "Rocket.toml")).nested())
        // NRZX_LOG ist der tracing-Filter (main.rs), keine Rocket-Einstellung
        .merge(Env::prefixed("NRZX_").ignore(&["PROFILE", "LOG"]).global())
        .select(rocket::figment::Profile::from_env_or(
            "NRZX_PROFILE",
            rocket::Config::DEFAULT_PROFILE,
        ))
}
