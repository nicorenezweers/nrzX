//! Spracherkennung mit Whisper (OpenAI) in reinem Rust über candle – ohne Python, ohne C++.
//!
//! * Modell, Konfiguration und Tokenizer kommen aus `openai/whisper-<grösse>` (Hugging Face) und
//!   liegen im Datenordner unter `models/whisper-<grösse>/`.
//! * Eingabe: Mono-PCM mit 16 kHz (f32, −1…1), höchstens 30 Sekunden (ein Fenster von Whisper).
//! * Ausgabe: Text ohne Zeitstempel; Stille und typische Halluzinationen fallen weg.
//! * Ein Vorgabetext (`prompt`) mit Namen („Dieter, Lena, Benni …“) hilft Whisper, sie richtig zu
//!   schreiben – er geht als `<|startofprev|>`-Kontext vor die eigentliche Aufgabe.
//! * Kurze Sätze: Der Encoder sieht nur so viel Audio wie nötig (mindestens 10 s statt immer 30 s) –
//!   etwa doppelt so schnell; ist das Ergebnis unsicher, läuft es noch einmal mit dem ganzen Fenster.
//! * Zwischenstände (Live-Diktat, Anruf): [`Engine::transcribe_fast`] – ein Durchgang mit noch
//!   kürzerem Fenster, ohne Wiederholung (Modell base: etwa 0,2 s statt 0,5–0,7 s).

use candle_core::{DType, Device, IndexOp, Tensor, D};
use candle_nn::{ops::softmax, VarBuilder};
use candle_transformers::models::whisper::{self as m, audio, model::Whisper, Config};
use std::collections::HashMap;
use std::path::Path;

pub type R<T> = Result<T, String>;

/// Abtastrate, die Whisper erwartet.
pub const SAMPLE_RATE: usize = m::SAMPLE_RATE;

/// Mindestlänge des Fensters (Mel-Frames, 100 je Sekunde) und Rand nach dem Ende der Aufnahme.
const CTX_MIN: usize = 1000;
/// … für Zwischenstände: kürzer (schneller, etwas ungenauer – der ganze Satz kommt danach genau)
const CTX_MIN_FAST: usize = 600;
const CTX_PAD: usize = 300;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Ergebnis einer Erkennung.
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub text: String,
    /// Sprache (`de`, `en` …), erkannt oder vorgegeben.
    pub language: String,
    /// Wahrscheinlichkeit „keine Sprache“ (0…1).
    pub no_speech: f64,
    /// Mittlere Log-Wahrscheinlichkeit der Tokens.
    pub avg_logprob: f64,
}

/// Geladenes Modell (eines zur Zeit; Aufrufe nacheinander).
pub struct Engine {
    model: Whisper,
    tok: Tokenizer,
    filters: Vec<f32>,
    device: Device,
    /// Maske über das Vokabular: 0 = erlaubt, −∞ = unterdrückt.
    suppress: Tensor,
    sot: u32,
    eot: u32,
    prev: u32,
    transcribe: u32,
    no_timestamps: u32,
    no_speech: Option<u32>,
    languages: Vec<(String, u32)>,
    blank: Option<u32>,
}

impl Engine {
    /// Modell aus einem Ordner mit `config.json`, `tokenizer.json`, `model.safetensors` laden.
    pub fn load(dir: &Path) -> R<Engine> {
        let device = Device::Cpu;
        let config: Config = serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).map_err(err)?).map_err(err)?;
        let tok = Tokenizer::load(&dir.join("tokenizer.json"))?;
        // SAFETY: Die Datei gehört nrzX und wird nicht verändert, solange das Modell geladen ist.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], m::DTYPE, &device).map_err(err)? };
        let model = Whisper::load(&vb, config.clone()).map_err(err)?;
        let filters = mel_filters(config.num_mel_bins);
        let id = |s: &str| tok.special(s).ok_or_else(|| format!("Token {s} fehlt im Tokenizer"));
        let sot = id(m::SOT_TOKEN)?;
        let eot = id(m::EOT_TOKEN)?;
        let prev = id("<|startofprev|>")?;
        let transcribe = id(m::TRANSCRIBE_TOKEN)?;
        let no_timestamps = id(m::NO_TIMESTAMPS_TOKEN)?;
        let no_speech = m::NO_SPEECH_TOKENS.iter().find_map(|t| tok.special(t));
        let languages = tok.languages();
        let blank = tok.vocab.get("Ġ").copied();
        // Nur Text-Tokens und das Ende sind erlaubt (keine Zeitstempel, keine Steuer-Tokens)
        let suppress: Vec<f32> = (0..config.vocab_size as u32)
            .map(|i| if config.suppress_tokens.contains(&i) || (i > eot && i < config.vocab_size as u32) { f32::NEG_INFINITY } else { 0.0 })
            .collect();
        let suppress = Tensor::new(suppress.as_slice(), &device).map_err(err)?;
        Ok(Engine { model, tok, filters, device, suppress, sot, eot, prev, transcribe, no_timestamps, no_speech, languages, blank })
    }

    /// Erkennen. `language`: `de`, `en` … oder leer (automatisch). `prompt`: Wörter und Namen,
    /// die vorkommen können.
    pub fn transcribe(&mut self, pcm: &[f32], language: &str, prompt: &str) -> R<Transcript> {
        let pcm = &pcm[..pcm.len().min(m::N_SAMPLES)];
        let cfg = self.model.config.clone();
        let mel = audio::pcm_to_mel(&cfg, pcm, &self.filters);
        let n_mels = cfg.num_mel_bins;
        let frames = mel.len() / n_mels;
        let mel = Tensor::from_vec(mel, (1, n_mels, frames), &self.device).map_err(err)?;
        let full = m::N_FRAMES.min(frames);
        let short = ((pcm.len() / m::HOP_LENGTH + CTX_PAD).max(CTX_MIN).min(full)) & !1;
        let first = self.run(&mel, short, language, prompt)?;
        if short >= full || (!first.text.is_empty() && first.avg_logprob > -0.8) {
            return Ok(first);
        }
        // Unsicher mit dem kurzen Fenster: mit den vollen 30 s wiederholen
        let second = self.run(&mel, full, language, prompt)?;
        Ok(if second.text.is_empty() || (!first.text.is_empty() && first.avg_logprob >= second.avg_logprob) { first } else { second })
    }

    /// Schnell für Zwischenstände (Live-Diktat): nur das kurze Fenster, keine Wiederholung.
    pub fn transcribe_fast(&mut self, pcm: &[f32], language: &str, prompt: &str) -> R<Transcript> {
        let pcm = &pcm[..pcm.len().min(m::N_SAMPLES)];
        let cfg = self.model.config.clone();
        let mel = audio::pcm_to_mel(&cfg, pcm, &self.filters);
        let n_mels = cfg.num_mel_bins;
        let frames = mel.len() / n_mels;
        let mel = Tensor::from_vec(mel, (1, n_mels, frames), &self.device).map_err(err)?;
        let full = m::N_FRAMES.min(frames);
        let short = ((pcm.len() / m::HOP_LENGTH + CTX_PAD).max(CTX_MIN_FAST).min(full)) & !1;
        self.run(&mel, short, language, prompt)
    }

    fn run(&mut self, mel: &Tensor, frames: usize, language: &str, prompt: &str) -> R<Transcript> {
        let cfg = self.model.config.clone();
        let mel = mel.narrow(2, 0, frames).map_err(err)?;
        let features = self.model.encoder.forward(&mel, true).map_err(err)?;

        let lang = match self.languages.iter().find(|(code, _)| code == language) {
            Some((code, id)) => (code.clone(), *id),
            None => self.detect_language(&features)?,
        };

        let mut tokens: Vec<u32> = Vec::new();
        let hint = self.tok.encode(prompt.trim());
        if !hint.is_empty() {
            tokens.push(self.prev);
            // Höchstens die Hälfte des Kontexts für den Vorgabetext (wie bei OpenAI)
            let keep = (cfg.max_target_positions / 2).saturating_sub(1);
            tokens.extend(hint[hint.len().saturating_sub(keep)..].iter().copied());
        }
        let sot_at = tokens.len();
        tokens.extend([self.sot, lang.1, self.transcribe, self.no_timestamps]);
        let start = tokens.len();

        let mut sum_logprob = 0f64;
        let mut no_speech = 0f64;
        let limit = cfg.max_target_positions / 2;
        for i in 0..limit {
            let input = Tensor::new(tokens.as_slice(), &self.device).map_err(err)?.unsqueeze(0).map_err(err)?;
            let ys = self.model.decoder.forward(&input, &features, i == 0).map_err(err)?;
            if i == 0 {
                if let Some(ns) = self.no_speech {
                    let logits = self.model.decoder.final_linear(&ys.i((..1, sot_at..sot_at + 1)).map_err(err)?).map_err(err)?.i(0).map_err(err)?.i(0).map_err(err)?;
                    no_speech = softmax(&logits, 0).map_err(err)?.i(ns as usize).map_err(err)?.to_scalar::<f32>().map_err(err)? as f64;
                }
            }
            let (_, seq, _) = ys.dims3().map_err(err)?;
            let logits = self.model.decoder.final_linear(&ys.i((..1, seq - 1..)).map_err(err)?).map_err(err)?.i(0).map_err(err)?.i(0).map_err(err)?;
            let mut logits = logits.broadcast_add(&self.suppress).map_err(err)?;
            if i == 0 {
                // Kein leerer Anfang (Leerzeichen oder sofort Ende)
                let mut mask = vec![0f32; cfg.vocab_size];
                for t in [Some(self.eot), self.blank].into_iter().flatten() {
                    if let Some(x) = mask.get_mut(t as usize) {
                        *x = f32::NEG_INFINITY;
                    }
                }
                logits = logits.broadcast_add(&Tensor::new(mask.as_slice(), &self.device).map_err(err)?).map_err(err)?;
            }
            let probs = softmax(&logits, D::Minus1).map_err(err)?;
            let next = logits.argmax(D::Minus1).map_err(err)?.to_scalar::<u32>().map_err(err)?;
            let p = probs.i(next as usize).map_err(err)?.to_scalar::<f32>().map_err(err)? as f64;
            if next == self.eot {
                break;
            }
            tokens.push(next);
            sum_logprob += p.max(1e-12).ln();
            if tokens.len() >= cfg.max_target_positions || repeats(&tokens[start..]) {
                break;
            }
        }
        let out = &tokens[start..];
        let avg_logprob = if out.is_empty() { -10.0 } else { sum_logprob / out.len() as f64 };
        let mut text = self.tok.decode(out).trim().to_string();
        // Stille: Whisper ist unsicher und hält „keine Sprache“ für wahrscheinlich
        if (no_speech > m::NO_SPEECH_THRESHOLD && avg_logprob < m::LOGPROB_THRESHOLD) || hallucination(&text) {
            text.clear();
        }
        Ok(Transcript { text, language: lang.0, no_speech, avg_logprob })
    }

    fn detect_language(&mut self, features: &Tensor) -> R<(String, u32)> {
        let input = Tensor::new(&[self.sot], &self.device).map_err(err)?.unsqueeze(0).map_err(err)?;
        let ys = self.model.decoder.forward(&input, features, true).map_err(err)?;
        let logits = self.model.decoder.final_linear(&ys.i((..1, 0..1)).map_err(err)?).map_err(err)?.i(0).map_err(err)?.i(0).map_err(err)?;
        let logits: Vec<f32> = logits.to_dtype(DType::F32).map_err(err)?.to_vec1().map_err(err)?;
        self.languages
            .iter()
            .max_by(|a, b| logits[a.1 as usize].total_cmp(&logits[b.1 as usize]))
            .cloned()
            .ok_or_else(|| "Keine Sprach-Tokens im Tokenizer".to_string())
    }
}

/// Endlosschleifen erkennen (dasselbe Stück vier Mal hintereinander).
fn repeats(t: &[u32]) -> bool {
    for n in 1..=8usize {
        if t.len() >= n * 4 {
            let tail = &t[t.len() - n..];
            if (1..4).all(|k| &t[t.len() - n * (k + 1)..t.len() - n * k] == tail) {
                return true;
            }
        }
    }
    false
}

/// Typische Erfindungen von Whisper bei Stille oder Rauschen (Untertitel aus den Trainingsdaten).
fn hallucination(text: &str) -> bool {
    let t = text.to_lowercase();
    let t = t.trim_matches(|c: char| !c.is_alphanumeric());
    const KNOWN: [&str; 14] = [
        "untertitel",
        "amara.org",
        "vielen dank fürs zuschauen",
        "vielen dank für's zuschauen",
        "danke fürs zuschauen",
        "bis zum nächsten mal",
        "copyright",
        "swr 20",
        "wdr 20",
        "thanks for watching",
        "thank you for watching",
        "subtitles by",
        "please subscribe",
        "zdf",
    ];
    t.is_empty() || KNOWN.iter().any(|k| t.contains(k)) || t.chars().all(|c| !c.is_alphabetic())
}

/// Mel-Filterbank wie `librosa.filters.mel(sr=16000, n_fft=400, n_mels, norm="slaney")`
/// (Zeilen: Mel-Bänder, Spalten: 201 FFT-Bins).
pub fn mel_filters(n_mels: usize) -> Vec<f32> {
    let sr = m::SAMPLE_RATE as f64;
    let n_fft = m::N_FFT;
    let bins = n_fft / 2 + 1;
    // Slaney-Skala: linear bis 1 kHz, darüber logarithmisch
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    let hz_to_mel = |f: f64| if f >= min_log_hz { min_log_mel + (f / min_log_hz).ln() / logstep } else { f / f_sp };
    let mel_to_hz = |mm: f64| if mm >= min_log_mel { min_log_hz * (logstep * (mm - min_log_mel)).exp() } else { f_sp * mm };
    let fft_freqs: Vec<f64> = (0..bins).map(|i| i as f64 * sr / n_fft as f64).collect();
    let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(sr / 2.0));
    let mel_f: Vec<f64> = (0..n_mels + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (n_mels + 1) as f64)).collect();
    let mut out = vec![0f32; n_mels * bins];
    for i in 0..n_mels {
        let (l, c, r) = (mel_f[i], mel_f[i + 1], mel_f[i + 2]);
        let enorm = 2.0 / (r - l);
        for (k, &f) in fft_freqs.iter().enumerate() {
            let lower = (f - l) / (c - l);
            let upper = (r - f) / (r - c);
            out[i * bins + k] = (lower.min(upper).max(0.0) * enorm) as f32;
        }
    }
    out
}

// ------------------------------------------------------------------ Tokenizer (Byte-BPE wie GPT-2)

struct Tokenizer {
    vocab: HashMap<String, u32>,
    rev: Vec<String>,
    added: HashMap<String, u32>,
    ranks: HashMap<(String, String), u32>,
    byte_char: [char; 256],
    char_byte: HashMap<char, u8>,
    split: regex::Regex,
}

impl Tokenizer {
    fn load(path: &Path) -> R<Tokenizer> {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).map_err(err)?).map_err(err)?;
        let mut vocab = HashMap::new();
        for (k, id) in v["model"]["vocab"].as_object().ok_or("tokenizer.json ohne Vokabular")? {
            if let Some(id) = id.as_u64() {
                vocab.insert(k.clone(), id as u32);
            }
        }
        let mut added = HashMap::new();
        for t in v["added_tokens"].as_array().cloned().unwrap_or_default() {
            if let (Some(c), Some(id)) = (t["content"].as_str(), t["id"].as_u64()) {
                added.insert(c.to_string(), id as u32);
            }
        }
        let mut ranks = HashMap::new();
        for (i, mg) in v["model"]["merges"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            let pair = match mg {
                serde_json::Value::String(s) => s.split_once(' ').map(|(a, b)| (a.to_string(), b.to_string())),
                serde_json::Value::Array(a) => match (a.first().and_then(|x| x.as_str()), a.get(1).and_then(|x| x.as_str())) {
                    (Some(x), Some(y)) => Some((x.to_string(), y.to_string())),
                    _ => None,
                },
                _ => None,
            };
            if let Some(p) = pair {
                ranks.insert(p, i as u32);
            }
        }
        let size = vocab.values().chain(added.values()).copied().max().unwrap_or(0) as usize + 1;
        let mut rev = vec![String::new(); size];
        for (k, id) in vocab.iter().chain(added.iter()) {
            rev[*id as usize] = k.clone();
        }
        let byte_char = bytes_to_unicode();
        let char_byte = byte_char.iter().enumerate().map(|(b, c)| (*c, b as u8)).collect();
        let split = regex::Regex::new(r"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+").map_err(err)?;
        Ok(Tokenizer { vocab, rev, added, ranks, byte_char, char_byte, split })
    }

    fn special(&self, s: &str) -> Option<u32> {
        self.added.get(s).or_else(|| self.vocab.get(s)).copied()
    }

    /// Sprach-Tokens `<|de|>`, `<|en|>` … (Code, ID).
    fn languages(&self) -> Vec<(String, u32)> {
        let mut v: Vec<(String, u32)> = self
            .added
            .iter()
            .filter_map(|(k, id)| {
                let code = k.strip_prefix("<|")?.strip_suffix("|>")?;
                (code.len() >= 2 && code.len() <= 3 && code.chars().all(|c| c.is_ascii_lowercase())).then(|| (code.to_string(), *id))
            })
            .collect();
        v.sort_by_key(|x| x.1);
        v
    }

    fn decode(&self, ids: &[u32]) -> String {
        let mut bytes = Vec::new();
        for id in ids {
            let Some(s) = self.rev.get(*id as usize) else { continue };
            if self.added.contains_key(s) {
                continue;
            }
            for c in s.chars() {
                if let Some(b) = self.char_byte.get(&c) {
                    bytes.push(*b);
                }
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn encode(&self, text: &str) -> Vec<u32> {
        if text.is_empty() {
            return Vec::new();
        }
        // Wie bei OpenAI beginnt der Vorgabetext mit einem Leerzeichen
        let text = format!(" {text}");
        let mut out = Vec::new();
        for piece in self.split.find_iter(&text) {
            let word: String = piece.as_str().bytes().map(|b| self.byte_char[b as usize]).collect();
            out.extend(self.bpe(&word));
        }
        out
    }

    fn bpe(&self, word: &str) -> Vec<u32> {
        if let Some(id) = self.vocab.get(word) {
            return vec![*id];
        }
        let mut parts: Vec<String> = word.chars().map(String::from).collect();
        while parts.len() > 1 {
            let best = (0..parts.len() - 1)
                .filter_map(|i| self.ranks.get(&(parts[i].clone(), parts[i + 1].clone())).map(|r| (*r, i)))
                .min();
            let Some((_, i)) = best else { break };
            let merged = format!("{}{}", parts[i], parts[i + 1]);
            parts[i] = merged;
            parts.remove(i + 1);
        }
        parts.iter().filter_map(|p| self.vocab.get(p).copied()).collect()
    }
}

/// Zuordnung Byte → druckbares Zeichen von GPT-2.
fn bytes_to_unicode() -> [char; 256] {
    let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32).chain(0xA1..=0xAC).chain(0xAE..=0xFF).collect();
    let mut cs = bs.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    let mut table = ['\0'; 256];
    for (b, c) in bs.iter().zip(cs.iter()) {
        table[*b as usize] = char::from_u32(*c).unwrap_or('?');
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_table_is_bijective() {
        let t = bytes_to_unicode();
        let set: std::collections::HashSet<char> = t.iter().copied().collect();
        assert_eq!(set.len(), 256);
        assert_eq!(t[b'A' as usize], 'A');
        assert_eq!(t[b' ' as usize], 'Ġ');
    }

    #[test]
    fn filters_have_shape() {
        let f = mel_filters(80);
        assert_eq!(f.len(), 80 * 201);
        assert!(f.iter().all(|x| x.is_finite() && *x >= 0.0));
    }

    #[test]
    fn detects_loops_and_noise() {
        assert!(repeats(&[1, 2, 1, 2, 1, 2, 1, 2]));
        assert!(!repeats(&[1, 2, 3, 4, 5, 6, 7, 8]));
        assert!(hallucination("Untertitel der Amara.org-Community"));
        assert!(hallucination("..."));
        assert!(!hallucination("Dieter, schalte dich an."));
    }
}
