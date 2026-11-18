//! Allgemeine Routen: `api` (Gesundheit, Markdown, Ereignisse), `spa` liefert die Oberfläche
//! (Dioxus/WebAssembly), `static_files` Schrift, Icons, Übersetzungen und Syntaxfarben,
//! `catchers` beantwortet Fehler als JSON oder HTML.

pub mod api;
pub mod catchers;
pub mod spa;
pub mod static_files;
