//! JSON-Formen der API (`/api/v1`), gemeinsam für Server und Oberfläche.
//! Felder in camelCase, Zeitangaben als Unix-Sekunden, IDs als Zahlen.
//!
//! * [`common`] – Bausteine für beide Bereiche (Agenten-Arten, Threads, Nachrichten, Ereignisse, Diffs)
//! * [`agents`] – Agent-Konten, Werkzeuge, Läufe, Subagenten, Terminals
//! * [`workspace`] – nrzGit-Verbindung, Projekte, Worktrees, Änderungen, Vorschau, Benutzer
//! * [`voice`] – Sprachsteuerung „Dieter“ (Spracherkennung, Befehle an die Fenster des Rasters)
//! * [`diff`] – Zeilen-Diff für die Bearbeitungen der Agenten im Chat

pub mod agents;
pub mod common;
pub mod diff;
pub mod team;
pub mod voice;
pub mod workspace;

pub use common::*;
