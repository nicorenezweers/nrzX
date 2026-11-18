//! Sprachsteuerung „Dieter“: Spracherkennung (Whisper, lokal auf dem Server) und Befehle an die
//! Fenster des Rasters (`/api/v1/voice/…`).

use crate::AgentKind;
use serde::{Deserialize, Serialize};

/// Einstellungen von Dieter je Benutzer (`GET/PUT /voice/settings`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VoiceSettings {
    /// Whisper-Modell für die Spracherkennung: `tiny`, `base`, `small`.
    pub stt_model: String,
    /// Sprache der Erkennung (`de`, `en` …), leer = automatisch erkennen.
    pub language: String,
    /// Agent, der die Befehle versteht und die Prompts schreibt (`None` = automatisch, günstig).
    pub agent: Option<AgentKind>,
    pub account_id: Option<i64>,
    /// Modell dieses Agenten (leer = ein schnelles, günstiges Modell).
    pub model: String,
    /// Antworten von Dieter vorlesen.
    pub speak: bool,
    /// Stimme fürs Vorlesen (Name der Systemstimme); leer = automatisch eine männliche.
    pub voice: String,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        VoiceSettings { stt_model: "base".into(), language: "de".into(), agent: None, account_id: None, model: String::new(), speak: true, voice: String::new() }
    }
}

/// Ein Whisper-Modell (`GET /voice/models`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SttModel {
    /// `tiny`, `base`, `small`
    pub id: String,
    pub label: String,
    pub detail: String,
    /// Grösse des Downloads in Bytes.
    pub size: i64,
    pub installed: bool,
    pub downloading: bool,
    /// Fortschritt des Downloads (0…1).
    pub progress: f64,
    /// Letzter Fehler beim Laden.
    pub error: String,
}

/// Ergebnis von `POST /voice/transcribe` (Rumpf: WAV, 16 kHz, mono, 16 Bit).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Transcription {
    /// Erkannter Text; leer bei Stille oder Geräuschen.
    pub text: String,
    pub language: String,
    /// Dauer der Erkennung in Millisekunden.
    pub ms: i64,
}

/// Ein Fenster im Raster, so wie Dieter es kennt.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DieterPane {
    /// Vorname des Fensters („Lena“).
    pub name: String,
    /// Thread im Fenster; `None` = leeres Fenster (ein Auftrag beginnt dort einen neuen Chat).
    pub thread_id: Option<i64>,
    /// `chat` (auch leer) | `terminal` | `browser`; leer = Chat
    pub kind: String,
    /// Adresse des Browsers bzw. Ordner des Terminals
    pub detail: String,
    /// Lage im Fenster („links oben“ …), damit Dieter Richtungen versteht
    pub place: String,
}

/// Ein Wortwechsel mit Dieter (für Rückbezüge wie „und sag ihr auch …“).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DieterTurn {
    /// `user` oder `dieter`
    pub role: String,
    pub text: String,
}

/// `POST /voice/command` – gesprochener Satz und Lage im Raster.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DieterRequest {
    pub text: String,
    pub panes: Vec<DieterPane>,
    /// Name des gross gezeigten Fensters.
    pub focused: Option<String>,
    /// Darf Dieter ein weiteres Fenster öffnen (Raster noch nicht voll)?
    pub can_add: bool,
    pub history: Vec<DieterTurn>,
}

/// Was Dieter tun will.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DieterAction {
    /// `send` (Auftrag/Nachricht an ein Fenster), `stop`, `focus`, `unfocus`, `new` (weiteres Fenster)
    #[serde(rename = "type", alias = "kind")]
    pub kind: String,
    /// Name des Fensters
    pub to: String,
    /// Prompt für den Agenten (bei `send` und `new`), Text der Notiz (`note`)
    pub prompt: String,
    /// `move`: Richtung (`left`, `right`, `up`, `down`, `center`) oder Name eines anderen Fensters;
    /// `workspace`: Name des Arbeitsbereichs; `open`: `terminal` | `browser` | `chat`
    pub target: String,
    /// `open` (Browser) und `navigate`: Adresse
    pub url: String,
}

/// Antwort von `POST /voice/command`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DieterReply {
    /// Gesprochene Antwort (kurz, ohne Markdown); leer = nichts sagen.
    pub reply: String,
    pub actions: Vec<DieterAction>,
}
