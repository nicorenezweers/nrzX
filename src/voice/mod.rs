//! Sprachsteuerung „Dieter“: Die Oberfläche nimmt Sätze über das Mikrofon auf (Sprachpausen
//! trennen sie), dieser Bereich erkennt sie mit Whisper – lokal, in reinem Rust (`whisper`) – und
//! lässt einen schnellen Agenten daraus Aufträge für die Fenster des Rasters schreiben (`dieter`).
//!
//! Routen unter `/api/v1`:
//! * `GET/PUT /voice/settings` – Modell der Erkennung, Sprache, Dieters Agent/Konto/Modell, Vorlesen
//! * `GET /voice/models`, `POST /voice/models/<id>/download`, `DELETE /voice/models/<id>` – Whisper-Modelle
//!   (von Hugging Face, `openai/whisper-<id>`, in `models/whisper-<id>/` des Datenordners)
//! * `POST /voice/transcribe?names=Lena,Benni` – WAV (16 kHz, mono) → Text
//! * `POST /voice/command` – erkannter Satz + Fenster → Antwort und Aktionen

pub mod dieter;
pub mod whisper;

use crate::auth::{AdminUser, CurrentUser};
use crate::db;
use crate::error::{AppError, AppResult};
use crate::AppState;
use futures::StreamExt;
use nrzx_shared::voice::{DieterReply, DieterRequest, SttModel, Transcription, VoiceSettings};
use rocket::data::{Data, ToByteUnit};
use rocket::serde::json::Json;
use rocket::{delete, get, post, put, routes, Route, State};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;
use tokio::io::AsyncWriteExt;

pub fn api_routes() -> Vec<Route> {
    routes![get_settings, put_settings, models, download, remove, transcribe, command]
}

/// Bekannte Modelle: ID, Name, Beschreibung, Grösse aller Dateien in Bytes.
const MODELS: [(&str, &str, &str, i64); 3] = [
    ("tiny", "Whisper Tiny", "Sehr schnell, aber ungenauer – für schwache Rechner.", 153_544_121),
    ("base", "Whisper Base", "Schnell und zuverlässig für Befehle – empfohlen.", 292_886_385),
    ("small", "Whisper Small", "Am genauesten, braucht mehr Speicher und Zeit.", 969_477_513),
];
const FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];
const HF: &str = "https://huggingface.co";

fn known(id: &str) -> bool {
    MODELS.iter().any(|m| m.0 == id)
}

fn model_dir(state: &AppState, id: &str) -> PathBuf {
    state.config.models_dir().join(format!("whisper-{id}"))
}

fn installed(dir: &Path) -> bool {
    FILES.iter().all(|f| dir.join(f).is_file())
}

// ------------------------------------------------------------------ Einstellungen

fn settings_key(user_id: i64) -> String {
    format!("voice:{user_id}")
}

pub async fn load_settings(state: &AppState, user_id: i64) -> AppResult<VoiceSettings> {
    let raw = db::setting(&state.db, &settings_key(user_id), "").await?;
    let mut s: VoiceSettings = serde_json::from_str(&raw).unwrap_or_default();
    if !known(&s.stt_model) {
        s.stt_model = VoiceSettings::default().stt_model;
    }
    Ok(s)
}

/// `GET /voice/settings`
#[get("/voice/settings")]
async fn get_settings(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<VoiceSettings>> {
    Ok(Json(load_settings(state, user.id()).await?))
}

/// `PUT /voice/settings`
#[put("/voice/settings", data = "<body>")]
async fn put_settings(state: &State<AppState>, user: CurrentUser, body: Json<VoiceSettings>) -> AppResult<Json<VoiceSettings>> {
    let mut s = body.into_inner();
    if !known(&s.stt_model) {
        return Err(AppError::bad("Unbekanntes Sprachmodell."));
    }
    s.language = s.language.trim().to_lowercase();
    if !(s.language.is_empty() || (2..=3).contains(&s.language.len()) && s.language.chars().all(|c| c.is_ascii_lowercase())) {
        return Err(AppError::bad("Unbekannte Sprache."));
    }
    s.model = s.model.trim().chars().take(200).collect();
    s.voice = s.voice.trim().chars().filter(|c| !c.is_control()).take(200).collect();
    if s.agent.is_none() {
        s.account_id = None;
    }
    if let (Some(kind), Some(id)) = (s.agent, s.account_id) {
        let acc = crate::agents::accounts::get_row(state, user.id(), id).await?;
        if acc.kind != kind.key() {
            return Err(AppError::bad("Das Konto gehört zu einem anderen Agenten."));
        }
    }
    db::set_setting(&state.db, &settings_key(user.id()), &serde_json::to_string(&s).unwrap_or_default()).await?;
    Ok(Json(s))
}

// ------------------------------------------------------------------ Modelle

#[derive(Default, Clone)]
struct Progress {
    done: u64,
    running: bool,
    error: String,
}

static DOWNLOADS: LazyLock<Mutex<HashMap<String, Progress>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Geladenes Modell (ID, Engine) – eines zur Zeit, Erkennungen laufen nacheinander.
static ENGINE: LazyLock<Mutex<Option<(String, whisper::Engine)>>> = LazyLock::new(|| Mutex::new(None));

fn progress(id: &str) -> Progress {
    DOWNLOADS.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned().unwrap_or_default()
}

fn set_progress(id: &str, f: impl FnOnce(&mut Progress)) {
    let mut map = DOWNLOADS.lock().unwrap_or_else(|e| e.into_inner());
    f(map.entry(id.to_string()).or_default());
}

fn model_list(state: &AppState) -> Vec<SttModel> {
    MODELS
        .iter()
        .map(|(id, label, detail, size)| {
            let p = progress(id);
            SttModel {
                id: id.to_string(),
                label: label.to_string(),
                detail: detail.to_string(),
                size: *size,
                installed: installed(&model_dir(state, id)),
                downloading: p.running,
                progress: if p.running { (p.done as f64 / *size as f64).min(0.99) } else { 0.0 },
                error: p.error,
            }
        })
        .collect()
}

/// `GET /voice/models`
#[get("/voice/models")]
async fn models(state: &State<AppState>, _user: CurrentUser) -> Json<Vec<SttModel>> {
    Json(model_list(state))
}

/// `POST /voice/models/<id>/download` – im Hintergrund laden (Fortschritt über `GET /voice/models`).
#[post("/voice/models/<id>/download")]
async fn download(state: &State<AppState>, _user: CurrentUser, id: &str) -> AppResult<Json<Vec<SttModel>>> {
    if !known(id) {
        return Err(AppError::NotFound);
    }
    let start = {
        let mut map = DOWNLOADS.lock().unwrap_or_else(|e| e.into_inner());
        let p = map.entry(id.to_string()).or_default();
        if p.running {
            false
        } else {
            *p = Progress { done: 0, running: true, error: String::new() };
            true
        }
    };
    if start && !installed(&model_dir(state, id)) {
        let state = state.inner().clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let result = fetch(&state, &id).await;
            set_progress(&id, |p| {
                p.running = false;
                if let Err(e) = &result {
                    p.error = e.clone();
                }
            });
            match result {
                Ok(()) => tracing::info!("Sprachmodell whisper-{id} geladen"),
                Err(e) => tracing::warn!("Sprachmodell whisper-{id}: {e}"),
            }
        });
    } else if start {
        set_progress(id, |p| p.running = false);
    }
    Ok(Json(model_list(state)))
}

/// Dateien des Modells in einen Zwischenordner laden und erst am Ende an ihren Platz legen.
async fn fetch(state: &AppState, id: &str) -> Result<(), String> {
    let dir = model_dir(state, id);
    let part = dir.with_extension("part");
    let _ = tokio::fs::remove_dir_all(&part).await;
    tokio::fs::create_dir_all(&part).await.map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder().user_agent(concat!("nrzX/", env!("CARGO_PKG_VERSION"))).build().map_err(|e| e.to_string())?;
    for f in FILES {
        let url = format!("{HF}/openai/whisper-{id}/resolve/main/{f}");
        let res = client.get(&url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("{f}: {e}"))?;
        let mut file = tokio::fs::File::create(part.join(f)).await.map_err(|e| e.to_string())?;
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("{f}: {e}"))?;
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
            set_progress(id, |p| p.done += chunk.len() as u64);
        }
        file.flush().await.map_err(|e| e.to_string())?;
    }
    // Prüfen, ob es sich laden lässt, bevor es als installiert gilt
    let check = part.clone();
    tokio::task::spawn_blocking(move || whisper::Engine::load(&check).map(|_| ()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Modell defekt: {e}"))?;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::rename(&part, &dir).await.map_err(|e| e.to_string())
}

/// `DELETE /voice/models/<id>` (Administratoren)
#[delete("/voice/models/<id>")]
async fn remove(state: &State<AppState>, _admin: AdminUser, id: &str) -> AppResult<Json<Vec<SttModel>>> {
    if !known(id) {
        return Err(AppError::NotFound);
    }
    if progress(id).running {
        return Err(AppError::conflict("Das Modell wird gerade geladen."));
    }
    {
        let mut g = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().is_some_and(|(k, _)| k == id) {
            *g = None;
        }
    }
    let dir = model_dir(state, id);
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir).await?;
    }
    set_progress(id, |p| p.error.clear());
    Ok(Json(model_list(state)))
}

// ------------------------------------------------------------------ Erkennen

/// `POST /voice/transcribe?names=…` – Rumpf: WAV (PCM 16 Bit oder Float, beliebige Rate, wird zu
/// 16 kHz mono). `names`: Namen der Fenster, damit Whisper sie richtig schreibt.
/// `POST /voice/transcribe?names=…&fast=true` – WAV erkennen. `fast`: Zwischenstand (Live-Diktat,
/// Anruf) – ein schneller Durchgang statt der genauen Erkennung.
#[post("/voice/transcribe?<names>&<fast>", data = "<data>")]
async fn transcribe(state: &State<AppState>, user: CurrentUser, names: Option<String>, fast: Option<bool>, data: Data<'_>) -> AppResult<Json<Transcription>> {
    let fast = fast.unwrap_or(false);
    let bytes = data.open(24.mebibytes()).into_bytes().await?;
    if !bytes.is_complete() {
        return Err(AppError::bad("Die Aufnahme ist zu lang."));
    }
    let pcm = decode_wav(&bytes).ok_or_else(|| AppError::bad("Unbekanntes Audioformat (erwartet: WAV)."))?;
    // Kürzer als eine Fünftelsekunde: nichts zu erkennen
    if pcm.len() < whisper::SAMPLE_RATE / 5 {
        return Ok(Json(Transcription::default()));
    }
    let s = load_settings(state, user.id()).await?;
    let id = s.stt_model.clone();
    let dir = model_dir(state, &id);
    if !installed(&dir) {
        let label = MODELS.iter().find(|m| m.0 == id).map(|m| m.1).unwrap_or("Whisper");
        return Err(AppError::conflict(format!("Das Sprachmodell „{label}“ ist noch nicht geladen – Einstellungen → Dieter.")));
    }
    let prompt = stt_prompt(names.as_deref().unwrap_or(""));
    let language = s.language.clone();
    let started = Instant::now();
    let result = tokio::task::spawn_blocking(move || -> Result<whisper::Transcript, String> {
        let mut g = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().map(|(k, _)| k != &id).unwrap_or(true) {
            *g = None;
            *g = Some((id.clone(), whisper::Engine::load(&dir)?));
        }
        match g.as_mut() {
            Some((_, engine)) if fast => engine.transcribe_fast(&pcm, &language, &prompt),
            Some((_, engine)) => engine.transcribe(&pcm, &language, &prompt),
            None => Err("Modell nicht geladen".into()),
        }
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))?
    .map_err(|e| AppError::bad(format!("Spracherkennung: {e}")))?;
    Ok(Json(Transcription { text: result.text, language: result.language, ms: started.elapsed().as_millis() as i64 }))
}

/// Vorgabetext für Whisper: „Dieter“ und die Namen der Fenster (richtig geschrieben).
fn stt_prompt(names: &str) -> String {
    let mut list = vec!["Dieter".to_string()];
    for n in names.split(',').map(str::trim).filter(|n| !n.is_empty() && n.chars().count() <= 24).take(8) {
        if !list.iter().any(|x| x.eq_ignore_ascii_case(n)) {
            list.push(n.to_string());
        }
    }
    format!("{}.", list.join(", "))
}

/// WAV lesen (RIFF, PCM 8/16/24/32 Bit oder Float 32) → mono, 16 kHz, f32.
pub fn decode_wav(b: &[u8]) -> Option<Vec<f32>> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let (mut fmt, mut channels, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    let mut i = 12;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        let body = &b[i + 8..(i + 8 + len).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            fmt = u16::from_le_bytes([body[0], body[1]]);
            channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            bits = u16::from_le_bytes([body[14], body[15]]);
            // WAVE_FORMAT_EXTENSIBLE: eigentliches Format im Untertyp
            if fmt == 0xFFFE && body.len() >= 26 {
                fmt = u16::from_le_bytes([body[24], body[25]]);
            }
        } else if id == b"data" {
            if channels == 0 || rate == 0 {
                return None;
            }
            let width = (bits / 8) as usize;
            let frame = width * channels as usize;
            if width == 0 || frame == 0 {
                return None;
            }
            let sample = |s: &[u8]| -> f32 {
                match (fmt, bits) {
                    (1, 8) => (s[0] as f32 - 128.0) / 128.0,
                    (1, 16) => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
                    (1, 24) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
                    (1, 32) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
                    (3, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                    _ => 0.0,
                }
            };
            if !matches!((fmt, bits), (1, 8) | (1, 16) | (1, 24) | (1, 32) | (3, 32)) {
                return None;
            }
            let mono: Vec<f32> = body
                .chunks_exact(frame)
                .map(|f| f.chunks_exact(width).map(sample).sum::<f32>() / channels as f32)
                .collect();
            return Some(resample(&mono, rate as usize, whisper::SAMPLE_RATE));
        }
        i += 8 + len + (len & 1);
    }
    None
}

/// Lineares Umrechnen der Abtastrate (für Sprache genau genug).
fn resample(input: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let n = (input.len() as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    (0..n)
        .map(|k| {
            let x = k as f64 * step;
            let i = x as usize;
            let frac = (x - i as f64) as f32;
            let a = input[i.min(input.len() - 1)];
            let b = input[(i + 1).min(input.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

// ------------------------------------------------------------------ Dieter

/// `POST /voice/command`
#[post("/voice/command", data = "<body>")]
async fn command(state: &State<AppState>, user: CurrentUser, body: Json<DieterRequest>) -> AppResult<Json<DieterReply>> {
    let s = load_settings(state, user.id()).await?;
    Ok(Json(dieter::answer(state, user.id(), &s, body.into_inner()).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, samples: &[i16]) -> Vec<u8> {
        let mut v = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        v.extend(b"RIFF");
        v.extend((36 + data_len).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend((rate * 2).to_le_bytes());
        v.extend(2u16.to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend(data_len.to_le_bytes());
        for s in samples {
            v.extend(s.to_le_bytes());
        }
        v
    }

    #[test]
    fn reads_and_resamples_wav() {
        let pcm = decode_wav(&wav(16000, &[0, 16384, -16384, 32767])).unwrap();
        assert_eq!(pcm.len(), 4);
        assert!((pcm[1] - 0.5).abs() < 1e-4);
        let pcm = decode_wav(&wav(48000, &vec![1000; 4800])).unwrap();
        assert_eq!(pcm.len(), 1600);
        assert!(decode_wav(b"nope").is_none());
    }

    #[test]
    fn prompt_lists_names_once() {
        assert_eq!(stt_prompt("Lena, Benni,lena,,"), "Dieter, Lena, Benni.");
        assert_eq!(stt_prompt(""), "Dieter.");
    }
}
