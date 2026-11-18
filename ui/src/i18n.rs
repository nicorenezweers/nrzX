//! Texte. nrzX ist deutsch: Die deutschen Texte im Quelltext erscheinen so, wie sie dort stehen –
//! `tr("Repository")`, mit Platzhaltern `trf("vor {0} Tagen", &[&n])`. Die Funktionen bleiben,
//! damit alle Texte an einer Stelle durchlaufen (Platzhalter, später evtl. wieder Sprachen).

use dioxus::prelude::*;
use std::collections::HashMap;
use std::fmt::Display;

/// Aktuelle Sprache (immer `de`).
pub static LANG: GlobalSignal<String> = Signal::global(|| "de".to_string());
/// Katalog der aktuellen Sprache (leer bei Deutsch).
static CATALOG: GlobalSignal<HashMap<String, String>> = Signal::global(HashMap::new);

/// Text in der aktuellen Sprache.
pub fn tr(source: &str) -> String {
    if LANG.read().as_str() == "de" {
        return source.to_string();
    }
    CATALOG
        .read()
        .get(source)
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| source.to_string())
}

/// Text mit Platzhaltern `{0}`, `{1}`, … in der aktuellen Sprache.
pub fn trf(source: &str, args: &[&dyn Display]) -> String {
    fill(&tr(source), args)
}

fn fill(text: &str, args: &[&dyn Display]) -> String {
    let mut out = text.replace("{{", "\u{1}").replace("}}", "\u{2}");
    for (i, a) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), &a.to_string());
    }
    out.replace('\u{1}', "{").replace('\u{2}', "}")
}

/// Sprache für Datumsangaben (`Intl`/`toLocaleDateString`).
pub fn locale() -> &'static str {
    "de-CH"
}

/// Beim Start: nrzX ist immer deutsch.
pub async fn init() {
    set_language("de", false).await;
}

/// Sprache setzen – nrzX kennt nur Deutsch (die Texte im Quelltext); andere Codes werden zu `de`.
pub async fn set_language(_code: &str, _remember: bool) {
    *CATALOG.write() = HashMap::new();
    *LANG.write() = "de".to_string();
    set_document_lang("de");
}

fn set_document_lang(code: &str) {
    if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element()) {
        let _ = el.set_attribute("lang", code);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fills_placeholders() {
        assert_eq!(super::fill("„{0}“ von {1} – {{x}}", &[&"A", &3]), "„A“ von 3 – {x}");
    }
}
