//! Mikrofon für Dieter: Aufnahme im Browser (`getUserMedia` + Web Audio), Sätze an Sprachpausen
//! trennen (Energie mit gleitender Rauschschwelle), auf 16 kHz umrechnen und als WAV an den
//! Server geben. Dazu Vorlesen über die Sprachausgabe des Systems (`speechSynthesis`) – solange
//! Dieter spricht, hört er nicht zu (sonst versteht er sich selbst). Welches Mikrofon, wählt man
//! in den Einstellungen (je Gerät); fehlt es, nimmt Dieter den Standard des Systems.
//!
//! Die Aufnahme ruft keine Dioxus-Funktionen auf (sie läuft ausserhalb der Komponenten), sondern
//! schickt [`Event`]s über einen Kanal; eine Aufgabe der Komponente liest sie.

use futures_channel::mpsc::UnboundedSender;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Abtastrate für Whisper.
pub const RATE: usize = 16_000;
/// Vorlauf vor dem erkannten Sprachbeginn (Sekunden), damit der erste Laut nicht fehlt.
const PRE_ROLL: f32 = 0.4;
/// So lange still = Satz zu Ende (Millisekunden).
const END_SILENCE_MS: f32 = 850.0;
/// Längster Satz (Sekunden; Whisper sieht höchstens 30).
const MAX_SECONDS: usize = 28;
/// Live-Diktat: so oft (Millisekunden neuer Sprache) geht der angefangene Satz zur Erkennung
/// (schneller Durchgang, ~0,2 s – immer nur einer unterwegs).
const PARTIAL_MS: f32 = 380.0;

/// Was die Aufnahme meldet.
#[derive(Debug, Clone)]
pub enum Event {
    /// Lautstärke (RMS, 0…1) des letzten Blocks.
    Level(f32),
    /// Ein ganzer Satz: Mono, 16 kHz.
    Utterance(Vec<f32>),
    /// Live-Diktat: der Satz bis jetzt (noch nicht zu Ende), Mono, 16 kHz.
    Partial(Vec<f32>),
}

/// Offene Aufnahme: Mikrofon → Prozessor (jeder Block an einen Rückruf) → stummer Ausgang.
struct Capture {
    ctx: web_sys::AudioContext,
    stream: web_sys::MediaStream,
    src: web_sys::MediaStreamAudioSourceNode,
    node: web_sys::ScriptProcessorNode,
    sink: web_sys::GainNode,
    _cb: Closure<dyn FnMut(web_sys::AudioProcessingEvent)>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.node.set_onaudioprocess(None);
        let _ = self.src.disconnect();
        let _ = self.node.disconnect();
        let _ = self.sink.disconnect();
        stop_tracks(&self.stream);
        let _ = self.ctx.close();
    }
}

/// Dieters Aufnahme; der Kanal bleibt für den Wechsel des Mikrofons ([`choose`]).
struct Mic {
    _capture: Capture,
    tx: UnboundedSender<Event>,
}

thread_local! {
    static MIC: RefCell<Option<Mic>> = const { RefCell::new(None) };
    /// Dieter spricht gerade – Aufnahme verwirft alles.
    static MUTED: Cell<bool> = const { Cell::new(false) };
    /// Mikrofon-Test in den Einstellungen (unabhängig von Dieter) und sein Pegel.
    static PROBE: RefCell<Option<Capture>> = const { RefCell::new(None) };
    static PROBE_LEVEL: Cell<f32> = const { Cell::new(0.0) };
}

fn js_err(e: JsValue) -> String {
    let name = js_sys::Reflect::get(&e, &"name".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
    let msg = js_sys::Reflect::get(&e, &"message".into()).ok().and_then(|v| v.as_string()).or_else(|| e.as_string()).unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" | "SecurityError" => "denied".into(),
        "NotFoundError" | "OverconstrainedError" => "nodevice".into(),
        _ if msg.is_empty() => name,
        _ => msg,
    }
}

fn stop_tracks(stream: &web_sys::MediaStream) {
    for t in stream.get_tracks().iter() {
        if let Ok(t) = t.dyn_into::<web_sys::MediaStreamTrack>() {
            t.stop();
        }
    }
}

/// Name des Geräts hinter einer Aufnahme.
fn track_label(stream: &web_sys::MediaStream) -> String {
    stream.get_audio_tracks().iter().find_map(|t| t.dyn_into::<web_sys::MediaStreamTrack>().ok()).map(|t| t.label()).unwrap_or_default()
}

fn media_devices() -> Result<web_sys::MediaDevices, String> {
    let window = web_sys::window().ok_or("unsupported")?;
    let devices = window.navigator().media_devices().map_err(|_| "unsupported".to_string())?;
    if js_sys::Reflect::get(&devices, &"getUserMedia".into()).map(|f| f.is_undefined()).unwrap_or(true) {
        return Err("unsupported".into());
    }
    Ok(devices)
}

/// Aufnahme aufbauen; `make` bekommt die Abtastrate und liefert den Rückruf für jeden Block.
/// Scheitert das, wird das Mikrofon gleich wieder freigegeben.
fn capture<F: FnMut(&[f32]) + 'static>(stream: web_sys::MediaStream, make: impl FnOnce(f32) -> F) -> Result<Capture, String> {
    let built = (|| -> Result<_, JsValue> {
        let ctx = web_sys::AudioContext::new()?;
        let src = ctx.create_media_stream_source(&stream)?;
        let node = ctx.create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(2048, 1, 1)?;
        // Ausgang stumm: der Prozessor braucht ein Ziel, hören soll man sich nicht
        let sink = ctx.create_gain()?;
        sink.gain().set_value(0.0);
        let mut on_block = make(ctx.sample_rate());
        let cb = Closure::<dyn FnMut(web_sys::AudioProcessingEvent)>::new(move |e: web_sys::AudioProcessingEvent| {
            let Ok(buf) = e.input_buffer() else { return };
            let Ok(data) = buf.get_channel_data(0) else { return };
            on_block(&data);
        });
        node.set_onaudioprocess(Some(cb.as_ref().unchecked_ref()));
        src.connect_with_audio_node(&node)?;
        node.connect_with_audio_node(&sink)?;
        sink.connect_with_audio_node(&ctx.destination())?;
        // WebKit startet den Kontext manchmal angehalten
        let _ = ctx.resume();
        Ok((ctx, src, node, sink, cb))
    })();
    match built {
        Ok((ctx, src, node, sink, _cb)) => Ok(Capture { ctx, stream, src, node, sink, _cb }),
        Err(e) => {
            stop_tracks(&stream);
            Err(js_err(e))
        }
    }
}

// ------------------------------------------------------------------ Mikrofon wählen

/// Gewähltes Mikrofon dieses Geräts (`localStorage`, Mikrofone sind je Rechner verschieden):
/// Geräte-ID und Name (falls sich die ID ändert). Fehlt beides: Standard des Systems.
const DEVICE_KEY: &str = "nrz-dieter-mic";
const LABEL_KEY: &str = "nrz-dieter-mic-label";

/// Ein Mikrofon des Geräts.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub id: String,
    /// Leer, solange der Browser keine Erlaubnis hat.
    pub label: String,
}

/// Gewähltes Mikrofon: (ID, Name); leere ID = Standard des Systems.
pub fn chosen() -> (String, String) {
    (crate::util::storage_get(DEVICE_KEY).unwrap_or_default(), crate::util::storage_get(LABEL_KEY).unwrap_or_default())
}

/// Mikrofon wählen (leere ID = Standard des Systems). Hört Dieter gerade zu, wechselt er sofort.
pub async fn choose(id: &str, label: &str) -> Result<(), String> {
    if id.is_empty() {
        crate::util::storage_remove(DEVICE_KEY);
        crate::util::storage_remove(LABEL_KEY);
    } else {
        crate::util::storage_set(DEVICE_KEY, id);
        crate::util::storage_set(LABEL_KEY, label);
    }
    match MIC.with(|m| m.borrow().as_ref().map(|m| m.tx.clone())) {
        Some(tx) => start(tx).await,
        None => Ok(()),
    }
}

/// Mikrofone des Geräts. Ohne Erlaubnis nennen Browser weder Namen noch IDs – `unlock` öffnet
/// dafür einmal kurz das Mikrofon (fragt dabei nötigenfalls nach der Erlaubnis).
pub async fn inputs(unlock: bool) -> Result<Vec<Input>, String> {
    let devices = media_devices()?;
    if unlock {
        stop_tracks(&open(&devices, "").await?);
    }
    let list = wasm_bindgen_futures::JsFuture::from(devices.enumerate_devices().map_err(js_err)?).await.map_err(js_err)?;
    let get = |d: &JsValue, k: &str| js_sys::Reflect::get(d, &k.into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
    let mut out: Vec<Input> = Vec::new();
    for d in js_sys::Array::from(&list).iter() {
        let id = get(&d, "deviceId");
        // „default“/„communications“ (Chrome, Windows) verweisen nur auf eines der Geräte
        if get(&d, "kind") != "audioinput" || id.is_empty() || id == "default" || id == "communications" || out.iter().any(|x| x.id == id) {
            continue;
        }
        out.push(Input { label: get(&d, "label"), id });
    }
    Ok(out)
}

/// Meldet an- und abgesteckte Geräte (solange der Wert lebt).
pub struct DeviceWatch {
    devices: web_sys::MediaDevices,
    cb: Closure<dyn FnMut()>,
}

impl Drop for DeviceWatch {
    fn drop(&mut self) {
        let _ = self.devices.remove_event_listener_with_callback("devicechange", self.cb.as_ref().unchecked_ref());
    }
}

/// Auf an- und abgesteckte Geräte hören; jede Änderung kommt als `()` über den Kanal.
pub fn watch_devices() -> Option<(DeviceWatch, futures_channel::mpsc::UnboundedReceiver<()>)> {
    let devices = media_devices().ok()?;
    let (tx, rx) = futures_channel::mpsc::unbounded();
    let cb = Closure::<dyn FnMut()>::new(move || {
        let _ = tx.unbounded_send(());
    });
    devices.add_event_listener_with_callback("devicechange", cb.as_ref().unchecked_ref()).ok()?;
    Some((DeviceWatch { devices, cb }, rx))
}

/// Mikrofon öffnen: `id` leer = Standard des Systems, sonst genau dieses Gerät.
async fn open(devices: &web_sys::MediaDevices, id: &str) -> Result<web_sys::MediaStream, String> {
    let audio = js_sys::Object::new();
    for (k, v) in [("echoCancellation", true), ("noiseSuppression", true), ("autoGainControl", true)] {
        let _ = js_sys::Reflect::set(&audio, &k.into(), &v.into());
    }
    let _ = js_sys::Reflect::set(&audio, &"channelCount".into(), &1.into());
    if !id.is_empty() {
        let exact = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&exact, &"exact".into(), &id.into());
        let _ = js_sys::Reflect::set(&audio, &"deviceId".into(), &exact);
    }
    let constraints = web_sys::MediaStreamConstraints::new();
    constraints.set_audio(&audio);
    let promise = devices.get_user_media_with_constraints(&constraints).map_err(js_err)?;
    wasm_bindgen_futures::JsFuture::from(promise).await.map_err(js_err)?.dyn_into().map_err(js_err)
}

/// Gewähltes Mikrofon öffnen. Fehlt es (abgesteckt, neue ID), ein Gerät mit demselben Namen,
/// sonst den Standard des Systems – Dieter bleibt nie stumm, nur weil ein Gerät fehlt.
async fn open_chosen(devices: &web_sys::MediaDevices) -> Result<web_sys::MediaStream, String> {
    let (id, label) = chosen();
    if id.is_empty() {
        return open(devices, "").await;
    }
    match open(devices, &id).await {
        Err(e) if e == "nodevice" => {}
        other => return other,
    }
    if !label.is_empty() {
        let same = inputs(false).await.unwrap_or_default().into_iter().find(|d| d.label == label && d.id != id);
        if let Some(d) = same {
            if let Ok(s) = open(devices, &d.id).await {
                crate::util::storage_set(DEVICE_KEY, &d.id);
                return Ok(s);
            }
        }
    }
    open(devices, "").await
}

/// Aufnahme beenden (Mikrofon frei).
pub fn stop() {
    MIC.with(|m| m.borrow_mut().take());
    cancel_speech();
}

/// Aufnahme mit dem gewählten Mikrofon starten. Fehler: `denied` (keine Erlaubnis), `nodevice`,
/// `unsupported` oder Text.
pub async fn start(tx: UnboundedSender<Event>) -> Result<(), String> {
    stop();
    let stream = open_chosen(&media_devices()?).await?;
    let sender = tx.clone();
    let capture = capture(stream, move |rate| {
        let mut vad = Vad::new(rate);
        move |data: &[f32]| {
            for ev in vad.push(data, MUTED.with(Cell::get)) {
                let _ = sender.unbounded_send(ev);
            }
        }
    })?;
    MIC.with(|m| *m.borrow_mut() = Some(Mic { _capture: capture, tx }));
    Ok(())
}

/// Mikrofon ausprobieren (Einstellungen): gewähltes Gerät öffnen, Pegel über [`probe_level`].
/// Ergebnis: Name des geöffneten Geräts.
pub async fn probe_start() -> Result<String, String> {
    probe_stop();
    let stream = open_chosen(&media_devices()?).await?;
    let label = track_label(&stream);
    let capture = capture(stream, |_| {
        |data: &[f32]| {
            let rms = (data.iter().map(|x| x * x).sum::<f32>() / data.len().max(1) as f32).sqrt();
            PROBE_LEVEL.with(|l| l.set(rms));
        }
    })?;
    PROBE.with(|p| *p.borrow_mut() = Some(capture));
    Ok(label)
}

/// Pegel des Tests (RMS, 0…1).
pub fn probe_level() -> f32 {
    PROBE_LEVEL.with(Cell::get)
}

/// Test beenden (Mikrofon frei).
pub fn probe_stop() {
    PROBE.with(|p| p.borrow_mut().take());
    PROBE_LEVEL.with(|l| l.set(0.0));
}

// ------------------------------------------------------------------ Diktieren im Composer

/// Diktat eines Eingabefelds (unabhängig von Dieter): gleiche Satztrennung, jeder Satz geht
/// gleich zur Erkennung. Beim Beenden kommt der angefangene Satz noch mit, dann schliesst der Kanal.
struct Dictation {
    owner: String,
    _capture: Capture,
    vad: std::rc::Rc<RefCell<Vad>>,
    tx: UnboundedSender<Event>,
}

thread_local! {
    static DICTATION: RefCell<Option<Dictation>> = const { RefCell::new(None) };
}

/// Diktat für das Feld `owner` starten (ein laufendes anderes endet). Fehler wie bei [`start`].
pub async fn dictate_start(owner: &str, tx: UnboundedSender<Event>) -> Result<(), String> {
    DICTATION.with(|d| d.borrow_mut().take());
    let stream = open_chosen(&media_devices()?).await?;
    let vad = std::rc::Rc::new(RefCell::new(Vad::new(48_000.0)));
    let (shared, sender) = (vad.clone(), tx.clone());
    let capture = capture(stream, move |rate| {
        // Live: während man spricht, kommt der Satz bis jetzt immer wieder mit (`Event::Partial`)
        let mut v = Vad::new(rate);
        v.partials = true;
        *shared.borrow_mut() = v;
        move |data: &[f32]| {
            for ev in shared.borrow_mut().push(data, false) {
                let _ = sender.unbounded_send(ev);
            }
        }
    })?;
    DICTATION.with(|d| *d.borrow_mut() = Some(Dictation { owner: owner.to_string(), _capture: capture, vad, tx }));
    Ok(())
}

/// Läuft gerade ein Diktat für `owner`?
pub fn dictating(owner: &str) -> bool {
    DICTATION.with(|d| d.borrow().as_ref().is_some_and(|x| x.owner == owner))
}

/// Diktat von `owner` beenden: Mikrofon frei, angefangener Satz geht noch zur Erkennung.
pub fn dictate_stop(owner: &str) {
    let Some(d) = DICTATION.with(|d| {
        let mut g = d.borrow_mut();
        if g.as_ref().is_some_and(|x| x.owner == owner) { g.take() } else { None }
    }) else {
        return;
    };
    if let Some(pcm) = d.vad.borrow_mut().flush() {
        let _ = d.tx.unbounded_send(Event::Utterance(pcm));
    }
    // `d` fällt hier: Aufnahme zu, Kanal geschlossen
}

/// Sprach-Erkennung über Energie: Beginn nach ~90 ms über der Schwelle, Ende nach ~0,85 s Stille.
struct Vad {
    rate: f32,
    step: f64,
    pos: f64,
    last: f32,
    pre: VecDeque<f32>,
    speech: Vec<f32>,
    active: bool,
    loud_ms: f32,
    quiet_ms: f32,
    voiced_ms: f32,
    noise: f32,
    /// Zwischenstände melden (Live-Diktat)
    partials: bool,
    /// Sprache seit dem letzten Zwischenstand (Millisekunden)
    partial_ms: f32,
}

impl Vad {
    fn new(rate: f32) -> Vad {
        let rate = if rate > 0.0 { rate } else { 48_000.0 };
        Vad {
            rate,
            step: rate as f64 / RATE as f64,
            pos: 0.0,
            last: 0.0,
            pre: VecDeque::new(),
            speech: Vec::new(),
            active: false,
            loud_ms: 0.0,
            quiet_ms: 0.0,
            voiced_ms: 0.0,
            noise: 0.004,
            partials: false,
            partial_ms: 0.0,
        }
    }

    /// Lineares Umrechnen auf 16 kHz (mit Zustand über die Blöcke hinweg), vorher ein einfacher
    /// Tiefpass gegen Spiegelfrequenzen.
    fn resample(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        let n = input.len();
        let mut out = Vec::with_capacity((n as f64 / self.step) as usize + 2);
        let mut x = self.pos;
        while x < (n - 1) as f64 {
            let i = x.floor();
            let frac = (x - i) as f32;
            let a = if i < 0.0 { self.last } else { input[i as usize] };
            let b = input[(i + 1.0) as usize];
            // Mittel über die Nachbarn (grob, reicht für Sprache)
            let c = if i >= 1.0 { input[i as usize - 1] } else { a };
            out.push(((a + (b - a) * frac) * 2.0 + c + b) / 4.0);
            x += self.step;
        }
        self.pos = x - n as f64;
        self.last = input[n - 1];
        out
    }

    /// Angefangenen Satz abschliessen (beim Beenden des Diktats).
    fn flush(&mut self) -> Option<Vec<f32>> {
        let was = std::mem::replace(&mut self.active, false);
        let pcm = std::mem::take(&mut self.speech);
        (was && self.voiced_ms >= 300.0).then_some(pcm)
    }

    fn push(&mut self, input: &[f32], muted: bool) -> Vec<Event> {
        let mut out = Vec::new();
        if input.is_empty() {
            return out;
        }
        let rms = (input.iter().map(|x| x * x).sum::<f32>() / input.len() as f32).sqrt();
        out.push(Event::Level(if muted { 0.0 } else { rms }));
        let samples = self.resample(input);
        let ms = input.len() as f32 / self.rate * 1000.0;
        if muted {
            if self.active {
            }
            self.active = false;
            self.speech.clear();
            self.pre.clear();
            self.loud_ms = 0.0;
            return out;
        }
        let threshold = (self.noise * 3.0).max(0.012);
        if !self.active {
            self.pre.extend(samples.iter().copied());
            let keep = (PRE_ROLL * RATE as f32) as usize;
            while self.pre.len() > keep {
                self.pre.pop_front();
            }
            if rms > threshold {
                self.loud_ms += ms;
            } else {
                self.loud_ms = 0.0;
                self.noise = (self.noise * 0.95 + rms * 0.05).clamp(0.001, 0.05);
            }
            if self.loud_ms >= 90.0 {
                self.active = true;
                self.quiet_ms = 0.0;
                self.voiced_ms = self.loud_ms;
                self.partial_ms = 0.0;
                self.speech = self.pre.drain(..).collect();
            }
        } else {
            self.speech.extend(samples);
            if rms < threshold * 0.8 {
                self.quiet_ms += ms;
            } else {
                self.quiet_ms = 0.0;
                self.voiced_ms += ms;
            }
            // Live: den Satz bis jetzt erkennen lassen, solange gesprochen wird
            self.partial_ms += ms;
            if self.partials && self.partial_ms >= PARTIAL_MS && self.voiced_ms >= 200.0 && self.quiet_ms < END_SILENCE_MS * 0.6 {
                self.partial_ms = 0.0;
                out.push(Event::Partial(self.speech.clone()));
            }
            if self.quiet_ms >= END_SILENCE_MS || self.speech.len() >= MAX_SECONDS * RATE {
                self.active = false;
                self.loud_ms = 0.0;
                let pcm = std::mem::take(&mut self.speech);
                // Nur Räuspern oder ein Klick: verwerfen
                if self.voiced_ms >= 300.0 {
                    out.push(Event::Utterance(pcm));
                }
            }
        }
        out
    }
}

/// Zwischenstände beruhigen (wie „LocalAgreement“ bei Streaming-Whisper): Whisper erkennt bei
/// jedem Zwischenstand den ganzen Satz neu und ändert dabei gern frühere Wörter. Wörter, die zwei
/// Zwischenstände nacheinander gleich erkannt haben, stehen fest und ändern sich nicht mehr;
/// dahinter steht der neueste Rest. Den ganzen Satz liefert danach die genaue Erkennung.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Steady {
    stable: Vec<String>,
    prev: Vec<String>,
}

impl Steady {
    /// Neuer Zwischenstand → Text zum Anzeigen.
    pub fn push(&mut self, said: &str) -> String {
        fn norm(w: &str) -> String {
            w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
        }
        let words: Vec<String> = said.split_whitespace().map(String::from).collect();
        let s = self.stable.len();
        // Wo stehen die festen Wörter im neuen Stand? (Whisper setzt vorne gern ein Wort davor)
        let at = (0..=2).find(|o| o + s <= words.len() && (0..s).all(|i| norm(&words[o + i]) == norm(&self.stable[i])));
        let tail: Vec<String> = match at {
            Some(o) => words[o + s..].to_vec(),
            None => words.get(s..).map(|w| w.to_vec()).unwrap_or_default(),
        };
        let prev_tail: Vec<String> = self.prev.get(s..).map(|w| w.to_vec()).unwrap_or_default();
        // Was der letzte und dieser Stand gleich sehen, steht ab jetzt fest – in der Schreibweise,
        // die schon dasteht (sonst flackerte „Bitte“ zu „bitte,“)
        for (a, b) in prev_tail.iter().zip(tail.iter()) {
            if norm(a) != norm(b) {
                break;
            }
            self.stable.push(a.clone());
        }
        let shown_tail = &tail[(self.stable.len() - s).min(tail.len())..];
        let mut out = self.stable.clone();
        out.extend(shown_tail.iter().cloned());
        self.prev = out.clone();
        out.join(" ")
    }

    /// Neuer Satz.
    pub fn reset(&mut self) {
        self.stable.clear();
        self.prev.clear();
    }
}

/// WAV (PCM 16 Bit, mono, 16 kHz).
pub fn wav(pcm: &[f32]) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut v = Vec::with_capacity(44 + pcm.len() * 2);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&(RATE as u32).to_le_bytes());
    v.extend_from_slice(&((RATE * 2) as u32).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        v.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    v
}

// ------------------------------------------------------------------ Vorlesen

fn synth() -> Option<web_sys::SpeechSynthesis> {
    web_sys::window()?.speech_synthesis().ok()
}

/// Männliche Systemstimmen (macOS, Windows/Edge, Chrome) – Dieter spricht mit einer davon.
const MALE: &[&str] = &[
    // Deutsch
    "markus", "yannick", "martin", "viktor", "stefan", "conrad", "killian", "florian", "ralf", "bernd", "christoph", "jonas", "klaus",
    "kasper", "hans", "jan", "felix", "eddy", "reed", "rocko", "grandpa",
    // Englisch und andere
    "daniel", "alex", "aaron", "arthur", "fred", "tom", "oliver", "evan", "nathan", "ralph", "gordon", "lee", "rishi", "guy", "ryan",
    "david", "mark", "george", "james", "christopher", "eric", "roger", "andrew", "brian", "thomas", "jorge", "diego", "pablo", "luca",
    "male", "man",
];

/// Roboterhafte Stimmen (Eloquence) nur, wenn es keine andere männliche gibt.
const ROBOTIC: &[&str] = &["eddy", "reed", "rocko", "grandpa", "fred", "ralph"];

fn words(name: &str) -> Vec<String> {
    name.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Ist das (dem Namen nach) eine männliche Stimme?
pub fn is_male(name: &str) -> bool {
    let w = words(name);
    !w.iter().any(|x| x == "female" || x == "woman") && w.iter().any(|x| MALE.contains(&x.as_str()))
}

/// Güte einer männlichen Stimme: natürlich/Premium vor normal vor roboterhaft.
fn male_rank(v: &web_sys::SpeechSynthesisVoice) -> i32 {
    let name = v.name();
    if !is_male(&name) {
        return -1;
    }
    let w = words(&name);
    let mut r = 10;
    if w.iter().any(|x| matches!(x.as_str(), "premium" | "enhanced" | "natural" | "neural" | "online" | "siri")) {
        r += 10;
    }
    if w.iter().any(|x| ROBOTIC.contains(&x.as_str())) {
        r -= 8;
    }
    if v.local_service() {
        r += 2;
    }
    r
}

/// Stimmen des Systems für eine Sprache (`de`, `en` …): (Name, Sprache, männlich).
pub fn voices(base: &str) -> Vec<(String, String, bool)> {
    let Some(s) = synth() else { return Vec::new() };
    let base = base.to_lowercase();
    let mut out: Vec<(String, String, bool)> = s
        .get_voices()
        .iter()
        .filter_map(|v| v.dyn_into::<web_sys::SpeechSynthesisVoice>().ok())
        .filter(|v| base.is_empty() || v.lang().to_lowercase().starts_with(&base))
        .map(|v| (v.name(), v.lang(), is_male(&v.name())))
        .collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// Stimme wählen: gewünschte (Name), sonst die beste männliche der Sprache; `true` = männlich
/// gefunden (sonst wird tiefer gesprochen).
fn pick_voice(lang: &str, wanted: &str) -> (Option<web_sys::SpeechSynthesisVoice>, bool) {
    let Some(s) = synth() else { return (None, false) };
    let base = lang.split('-').next().unwrap_or("de").to_lowercase();
    let list: Vec<web_sys::SpeechSynthesisVoice> = s.get_voices().iter().filter_map(|v| v.dyn_into::<web_sys::SpeechSynthesisVoice>().ok()).collect();
    if !wanted.trim().is_empty() {
        if let Some(v) = list.iter().find(|v| v.name() == wanted.trim()) {
            return (Some(v.clone()), true);
        }
    }
    let same: Vec<&web_sys::SpeechSynthesisVoice> = list.iter().filter(|v| v.lang().to_lowercase().starts_with(&base)).collect();
    if let Some(v) = same.iter().filter(|v| male_rank(v) >= 0).max_by_key(|v| male_rank(v)) {
        return (Some((*v).clone()), true);
    }
    // Keine männliche Stimme: normale Systemstimme der Sprache, tiefer gestimmt
    let good = ["anna", "helena", "petra", "samantha", "karen", "google", "microsoft"];
    let fallback = same.iter().find(|v| good.iter().any(|n| v.name().to_lowercase().contains(n)) && v.local_service()).or(same.first()).map(|v| (*v).clone());
    (fallback, false)
}

/// Name der Stimme, die Dieter gerade nehmen würde (für die Einstellungen).
pub fn auto_voice(lang: &str) -> Option<String> {
    pick_voice(lang, "").0.filter(|v| is_male(&v.name())).map(|v| v.name())
}

/// Text vorlesen (`lang` wie `de-DE`, `voice` = Name der Stimme, leer = automatisch eine
/// männliche); `done` läuft danach (auch wenn es nichts vorzulesen gibt). Währenddessen hört
/// das Mikrofon nicht zu.
pub fn speak(text: &str, lang: &str, voice: &str, done: impl FnOnce() + 'static) {
    let text = text.trim();
    let Some(s) = synth().filter(|_| !text.is_empty()) else {
        done();
        return;
    };
    let Ok(u) = web_sys::SpeechSynthesisUtterance::new_with_text(text) else {
        done();
        return;
    };
    u.set_lang(lang);
    u.set_rate(1.05);
    let (best, male) = pick_voice(lang, voice);
    if let Some(v) = &best {
        u.set_voice(Some(v));
    }
    // Ohne männliche Stimme: tiefer, damit Dieter nach Dieter klingt
    u.set_pitch(if male { 1.0 } else { 0.72 });
    MUTED.with(|m| m.set(true));
    // Einmal fertig melden – nach `end`, `error` oder spätestens nach einer Frist (manche
    // Browser melden das Ende nicht zuverlässig)
    let once: std::rc::Rc<RefCell<Option<Box<dyn FnOnce()>>>> = std::rc::Rc::new(RefCell::new(Some(Box::new(done))));
    let finish = Closure::<dyn FnMut()>::new(move || {
        if let Some(d) = once.borrow_mut().take() {
            let cb = Closure::once_into_js(move || {
                MUTED.with(|m| m.set(false));
                SPEAKING.with(|c| c.borrow_mut().take());
                d();
            });
            if let Some(w) = web_sys::window() {
                // Kurz nachklingen lassen, bevor wieder zugehört wird
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), 300);
            }
        }
    });
    u.set_onend(Some(finish.as_ref().unchecked_ref()));
    u.set_onerror(Some(finish.as_ref().unchecked_ref()));
    if let Some(w) = web_sys::window() {
        let limit = (text.chars().count() as i32 * 90 + 2500).min(60_000);
        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(finish.as_ref().unchecked_ref(), limit);
    }
    finish.forget();
    s.cancel();
    s.speak(&u);
    // Verweis halten, sonst räumt der Browser die Ausgabe weg und meldet kein Ende
    SPEAKING.with(|c| *c.borrow_mut() = Some(u));
}

thread_local! {
    static SPEAKING: RefCell<Option<web_sys::SpeechSynthesisUtterance>> = const { RefCell::new(None) };
}

/// Vorlesen abbrechen.
pub fn cancel_speech() {
    if let Some(s) = synth() {
        s.cancel();
    }
    MUTED.with(|m| m.set(false));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn male_voices() {
        assert!(is_male("Markus"));
        assert!(is_male("Microsoft Stefan - German (Germany)"));
        assert!(is_male("Microsoft Conrad Online (Natural) - German (Germany)"));
        assert!(is_male("Google UK English Male"));
        assert!(is_male("Yannick (Premium)"));
        assert!(!is_male("Google UK English Female"));
        assert!(!is_male("Google Deutsch"));
        assert!(!is_male("Anna"));
        assert!(!is_male("Microsoft Katja - German (Germany)"));
    }

    #[test]
    fn steady_partials() {
        let mut s = Steady::default();
        assert_eq!(s.push("Bitte prüf"), "Bitte prüf");
        // „Bitte“ zweimal gleich → fest; „prüf“ → „prüfe“ ist noch offen
        assert_eq!(s.push("bitte, prüfe noch"), "Bitte prüfe noch");
        // Whisper ändert vorne – das feste „Bitte“ bleibt, und kein Wort doppelt
        assert_eq!(s.push("Also bitte prüfe noch einmal"), "Bitte prüfe noch einmal");
        // kürzer erkannt: was fest steht, bleibt stehen
        assert_eq!(s.push("Bitte"), "Bitte prüfe noch");
        s.reset();
        assert_eq!(s.push("Neu"), "Neu");
    }

    #[test]
    fn wav_header_is_valid() {
        let w = wav(&[0.0, 0.5, -0.5]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes([w[24], w[25], w[26], w[27]]), 16_000);
    }

    #[test]
    fn vad_detects_a_sentence() {
        let mut v = Vad::new(48_000.0);
        let quiet = vec![0.001f32; 2048];
        let loud: Vec<f32> = (0..2048).map(|i| ((i as f32) * 0.05).sin() * 0.2).collect();
        let mut events = Vec::new();
        for _ in 0..10 {
            events.extend(v.push(&quiet, false));
        }
        for _ in 0..20 {
            events.extend(v.push(&loud, false));
        }
        for _ in 0..30 {
            events.extend(v.push(&quiet, false));
        }
        let utterances: Vec<usize> = events.iter().filter_map(|e| if let Event::Utterance(p) = e { Some(p.len()) } else { None }).collect();
        assert_eq!(utterances.len(), 1);
        // ~0,85 s Sprache + Vorlauf + Pause, bei 16 kHz
        assert!(utterances[0] > RATE && utterances[0] < 3 * RATE, "{}", utterances[0]);
    }
}
