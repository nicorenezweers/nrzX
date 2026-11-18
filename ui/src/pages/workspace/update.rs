//! Update der App als Knopf unten in der Seitenleiste – nur in der App und nur, wenn eines da ist;
//! sonst steht dort nichts (kein Hinweis unten rechts). Ein Klick lädt und installiert es
//! (`update:install`), der Knopf zeigt dabei den Fortschritt; schlägt es fehl, versucht ein Klick
//! es noch einmal.
//!
//! Die App meldet ihren Stand über `window.__nrzUpdateState(status, text)` (`update_ui` in
//! `macos/src/main.rs`): `available` (Version), `progress` (Anteil 0–1, -1 unbekannt),
//! `installing`, `failed` (Text). `current` und `error` (Ergebnis einer Prüfung) ändern am Knopf
//! nichts. `update:state` fragt nach einem schon gefundenen Update (nach dem Neuladen).

use crate::i18n::{tr, trf};
use crate::util;
use dioxus::prelude::*;

#[derive(Clone, PartialEq, Debug)]
enum UpdateUi {
    /// Kein Update bekannt: kein Knopf.
    Idle,
    /// Gefunden (Version).
    Available(String),
    /// Wird geladen (Anteil 0–1, `None` = unbekannt).
    Loading(Option<f64>),
    Installing,
    /// Laden bzw. Installieren fehlgeschlagen (Text).
    Failed(String),
}

static UPDATE: GlobalSignal<UpdateUi> = Signal::global(|| UpdateUi::Idle);

/// Stand der App annehmen und nach einem schon gefundenen Update fragen (einmal beim Start).
pub fn install() {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    if !util::in_app() {
        return;
    }
    let Some(w) = web_sys::window() else { return };
    let cb = Closure::<dyn FnMut(String, String)>::new(|status: String, text: String| {
        let next = match status.as_str() {
            "available" => UpdateUi::Available(text),
            "progress" => UpdateUi::Loading(text.parse::<f64>().ok().filter(|p| *p >= 0.0)),
            "installing" => UpdateUi::Installing,
            // Fehlgeschlagen ohne bekanntes Update (gibt es eigentlich nicht): kein Knopf
            "failed" if *UPDATE.peek() != UpdateUi::Idle => UpdateUi::Failed(text),
            _ => return,
        };
        *UPDATE.write() = next;
    });
    let _ = js_sys::Reflect::set(&w, &"__nrzUpdateState".into(), cb.as_ref().unchecked_ref());
    cb.forget();
    util::native("update:state");
}

/// Knopf unten in der Seitenleiste – nur mit Update.
#[component]
pub fn UpdateButton() -> Element {
    let row = "flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[13px] [&_svg]:size-4 [&_svg]:shrink-0";
    let install = move |_| {
        util::native("update:install");
    };
    match UPDATE() {
        UpdateUi::Idle => rsx! {},
        UpdateUi::Available(v) => rsx! {
            button {
                class: "{row} font-medium text-info transition-colors hover:bg-info/10",
                title: tr("Lädt das Update und startet nrzX neu."),
                onclick: install,
                icons::CircleArrowDown {}
                span { class: "min-w-0 flex-1 truncate", {trf("Update auf {0} installieren", &[&v])} }
            }
        },
        UpdateUi::Loading(p) => rsx! {
            div { class: "{row} cursor-default text-muted-foreground",
                icons::LoaderCircle { class: "animate-spin" }
                span { class: "min-w-0 flex-1 truncate", {tr("Update wird geladen …")} }
                if let Some(p) = p {
                    span { class: "shrink-0 tabular-nums", {format!("{:.0} %", p * 100.0)} }
                }
            }
        },
        UpdateUi::Installing => rsx! {
            div { class: "{row} cursor-default text-muted-foreground",
                icons::LoaderCircle { class: "animate-spin" }
                span { class: "min-w-0 flex-1 truncate", {tr("Update wird installiert …")} }
            }
        },
        UpdateUi::Failed(e) => rsx! {
            button {
                class: "{row} text-destructive transition-colors hover:bg-destructive/10",
                title: "{e}",
                onclick: install,
                icons::CircleAlert {}
                span { class: "min-w-0 flex-1 truncate", {tr("Update fehlgeschlagen – erneut versuchen")} }
            }
        },
    }
}
