//! Agenten: Unterhaltung eines Threads (`ChatView`), Composer, Subagenten (`Lineage`),
//! Agent-Konten (`AccountsSettings`), Werkzeuge (`ToolsSettings`), Terminal (`TerminalView`),
//! Agent-Icons (`AgentIcon`). Diese Komponenten bindet der Bereich `workspace` ein – Namen und
//! Props sind festgelegt.
//!
//! * `chat` – Nachrichten, Werkzeug-Gruppen, Subagenten-Karte, Statuszeile
//! * `composer` – Eingabe mit `@`/`$`/`/`, Anhänge, Auswahl Agent · Konto · Modell · Stufe
//! * `lineage` – Subagenten für die rechte Spalte
//! * `accounts`, `tools`, `chains` – Einstellungen (Konten, Werkzeuge, KI-Ketten)
//! * `terminal`, `vt` – Terminal in reinem Rust (VT100-Interpreter)
//! * `icons`, `data` – Agent-Icons, gemeinsame Daten und Helfer

mod accounts;
mod btw;
mod chains;
mod chat;
mod composer;
pub(crate) mod data;
mod icons;
mod lineage;
mod notes;
mod status;
mod terminal;
mod tools;
mod usage;
mod vt;

// Vertrag mit dem Bereich workspace (noch nicht alle dort eingebunden)
#[allow(unused_imports)]
pub use self::{
    accounts::AccountsSettings, btw::{BtwPanel, BTW_OPEN}, chains::ChainsSettings, chat::{prefetch_messages, ChatView, PendingChat}, composer::Composer, icons::AgentIcon, lineage::Lineage, terminal::{TerminalPane, TerminalView},
    tools::ToolsSettings, usage::{headline as usage_headline, level_color as usage_level_color, refresh_usage, reload_usage, tz_offset, UsageBoard, UsageSection, USAGE}, notes::NotesPanel, data::{duration, model_label, use_agents, AGENTS},
};
