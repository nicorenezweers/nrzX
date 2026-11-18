//! Rechte von macOS (Datenschutz & Sicherheit) für die Ordner, in denen Agenten arbeiten.
//!
//! Die Agenten laufen als Kindprozesse der App; macOS schreibt ihre Zugriffe der App zu und fragt
//! sonst erst mitten in einem Lauf nach (oder ein Agent wartet unbemerkt auf den Dialog). Darum
//! fragt die App beim Start fehlende Ordner (Schreibtisch, Dokumente, Downloads) von selbst an
//! ([`ask_missing_folders`]) – erteilt bleibt erteilt, solange die Signatur der App gleich bleibt
//! (`cargo xtask macos` signiert dafür stabil). Fehlt etwas, zeigt [`show_hint`] den Weg in die
//! Systemeinstellungen. Das Mikrofon fragt WebKit selbst beim ersten Sprechen an.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Granted,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Desktop,
    Documents,
    Downloads,
}

impl Kind {
    fn folder(self) -> PathBuf {
        crate::server::home().join(match self {
            Kind::Desktop => "Desktop",
            Kind::Documents => "Documents",
            Kind::Downloads => "Downloads",
        })
    }
}

/// Ordner einmal lesen: Das löst den Dialog von macOS aus und hält an, bis er beantwortet ist –
/// nicht im Event-Loop aufrufen.
fn request(kind: Kind) -> State {
    let dir = kind.folder();
    match std::fs::read_dir(&dir) {
        Ok(mut it) => {
            let _ = it.next();
            State::Granted
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::Granted,
        Err(e) => {
            tracing::info!("Zugriff auf {}: {e}", dir.display());
            State::Denied
        }
    }
}

/// Festplattenvollzugriff: Die TCC-Datenbank des Benutzers ist nur damit lesbar (ohne Dialog).
fn full_disk() -> bool {
    std::fs::File::open(crate::server::home().join("Library/Application Support/com.apple.TCC/TCC.db")).is_ok()
}

/// Fehlende Ordnerrechte anfragen – ohne Dialog, wenn sie schon erteilt sind. Ergebnis: verweigerte
/// Ordner. Blockiert, bis die Dialoge beantwortet sind.
pub fn ask_missing_folders() -> Vec<Kind> {
    if full_disk() {
        return Vec::new();
    }
    [Kind::Desktop, Kind::Documents, Kind::Downloads].into_iter().filter(|k| request(*k) == State::Denied).collect()
}

/// Hinweis auf verweigerte Ordner einmal je Version zeigen (Datei im Datenordner merkt es sich).
pub fn hint_due() -> bool {
    let file = crate::server::data_dir().join(".permission-hint");
    let version = env!("CARGO_PKG_VERSION");
    if std::fs::read_to_string(&file).is_ok_and(|v| v.trim() == version) {
        return false;
    }
    let _ = std::fs::write(&file, version);
    true
}

/// Dialog bei fehlenden Ordnerrechten mit Weg in die Systemeinstellungen.
pub fn show_hint() {
    let choice = rfd::MessageDialog::new()
        .set_title("nrzX fehlen Rechte")
        .set_description(
            "nrzX darf auf Schreibtisch, Dokumente oder Downloads nicht zugreifen – Agenten scheitern dort mit „Operation not permitted“. \
             In den Systemeinstellungen unter Datenschutz & Sicherheit → Dateien und Ordner (oder Festplattenvollzugriff) erlauben.",
        )
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::OkCancelCustom("Systemeinstellungen öffnen".into(), "Später".into()))
        .show();
    if matches!(choice, rfd::MessageDialogResult::Custom(ref t) if t == "Systemeinstellungen öffnen") {
        let url = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
        let _ = std::process::Command::new("/usr/bin/open").arg(url).spawn();
    }
}
