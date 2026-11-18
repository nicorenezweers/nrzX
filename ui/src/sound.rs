//! Kleine Töne bei Statuswechseln einer Aufgabe: fertig (zwei helle Töne aufwärts), Rückfrage
//! (zweimal kurz), Fehler (zwei Töne abwärts). Erzeugt mit Web Audio – keine Tondateien.
//!
//! Im Vordergrund spielt die App den Ton selbst. Im Hintergrund kommt der Ton mit der Mitteilung
//! des Systems (`notify`); nur wenn Mitteilungen aus sind, spielt die App ihn auch dann – nie
//! doppelt. WebKit startet Audio erst nach einer Geste: der erste Klick bzw. Tastendruck schaltet
//! es frei (reines JavaScript, bleibt für die ganze Sitzung).

use crate::state::{tone, Tone};
use dioxus::prelude::*;
use nrzx_shared::{ThreadStatus, ThreadSummary};

const SCRIPT: &str = "(function(){if(window.__nrzSound)return;\
var ctx=function(){var C=window.__nrzAudioCtx;if(!C){var A=window.AudioContext||window.webkitAudioContext;if(!A)return null;C=window.__nrzAudioCtx=new A();}if(C.state==='suspended'){try{C.resume()}catch(e){}}return C;};\
var unlock=function(){try{ctx()}catch(e){}};window.addEventListener('pointerdown',unlock,true);window.addEventListener('keydown',unlock,true);\
window.__nrzSound=function(kind){try{var now=Date.now();if(window.__nrzSoundAt&&now-window.__nrzSoundAt<400)return;window.__nrzSoundAt=now;\
var C=ctx();if(!C)return;\
var notes=kind==='done'?[[659.25,0,0.14],[987.77,0.11,0.26]]:kind==='ask'?[[880,0,0.1],[1174.66,0.15,0.16]]:[[392,0,0.16],[293.66,0.15,0.26]];\
var type=kind==='error'?'triangle':'sine';var t0=C.currentTime+0.02;\
notes.forEach(function(n){var o=C.createOscillator(),g=C.createGain();o.type=type;o.frequency.value=n[0];var s=t0+n[1],e=s+n[2];\
g.gain.setValueAtTime(0.0001,s);g.gain.exponentialRampToValueAtTime(0.085,s+0.012);g.gain.exponentialRampToValueAtTime(0.0001,e);\
o.connect(g);g.connect(C.destination);o.start(s);o.stop(e+0.03);});}catch(e){}};})()";

/// Einmal beim Start: Töne und Freischalten bereitstellen.
pub fn install() {
    let _ = js_sys::eval(SCRIPT);
}

/// Ton spielen: "done" | "ask" | "error".
fn play(kind: &str) {
    let f = js_sys::Function::new_with_args("k", "if(window.__nrzSound)window.__nrzSound(k)");
    let _ = f.call1(&wasm_bindgen::JsValue::NULL, &wasm_bindgen::JsValue::from_str(kind));
}

/// Neuer Stand einer Aufgabe vom Server: bei Wechsel zu fertig, Rückfrage oder Fehler ein Ton
/// (nur Haupt-Threads, nicht Subagenten).
pub fn on_change(old: Option<&ThreadSummary>, new: &ThreadSummary) {
    let Some(old) = old else { return };
    if new.parent_id.is_some() {
        return;
    }
    let kind = if tone(new) == Tone::Asking && tone(old) != Tone::Asking {
        "ask"
    } else if new.status == ThreadStatus::Done && old.status.is_active() {
        "done"
    } else if new.status == ThreadStatus::Error && old.status.is_active() {
        "error"
    } else {
        return;
    };
    // Im Hintergrund klingt die Mitteilung des Systems – ausser Mitteilungen sind aus
    let focused = web_sys::window().and_then(|w| w.document()).is_some_and(|d| !d.hidden() && d.has_focus().unwrap_or(false));
    if focused || *crate::notify::MODE.peek() == crate::notify::Mode::Off {
        play(kind);
    }
}
