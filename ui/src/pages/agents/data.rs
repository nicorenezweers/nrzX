//! Gemeinsame Daten und Helfer des Bereichs: Agent-Arten mit Konten (einmal geladen, nach
//! Änderungen neu), Skills, Auswahl Agent/Konto/Modell/Stufe, Beschriftungen und Zeitangaben.

use crate::api;
use crate::i18n::{tr, trf};
use dioxus::prelude::*;
use nrzx_shared::agents::{AgentInfo, Skill};
use nrzx_shared::{AgentKind, ThreadSummary};
use serde::{Deserialize, Serialize};

/// Agent-Arten mit Konten (`GET /agents`), `None` = noch nicht geladen.
pub static AGENTS: GlobalSignal<Option<Vec<AgentInfo>>> = Signal::global(|| None);
/// Skills des Benutzers (`GET /agents/skills`).
pub static SKILLS: GlobalSignal<Option<Vec<Skill>>> = Signal::global(|| None);
/// Text und Anhänge, die ein Composer übernehmen soll (zurückgezogene Nachricht, Notiz):
/// `thread_id` = Composer dieses Threads, `None` = Composer für einen neuen Thread.
#[derive(Clone, PartialEq, Debug)]
pub struct ComposerDraft {
    pub thread_id: Option<i64>,
    pub text: String,
    pub attachments: Vec<nrzx_shared::Attachment>,
}
pub static COMPOSER_DRAFT: GlobalSignal<Option<ComposerDraft>> = Signal::global(|| None);
/// Notizblock (`GET /notes`), `None` = noch nicht geladen.
pub static NOTES: GlobalSignal<Option<Vec<nrzx_shared::agents::PromptNote>>> = Signal::global(|| None);
/// Notizblock neu laden.
pub async fn reload_notes() {
    if let Ok(list) = api::get::<Vec<nrzx_shared::agents::PromptNote>>("/notes").await {
        *NOTES.write() = Some(list);
    }
}

/// Sekundentakt für laufende Zeitangaben („Working 25s“).
pub static NOW: GlobalSignal<i64> = Signal::global(crate::util::now);

/// Agent-Arten neu laden.
pub async fn reload_agents() {
    if let Ok(list) = api::get::<Vec<AgentInfo>>("/agents").await {
        *AGENTS.write() = Some(list);
    }
}

/// Skills neu laden.
pub async fn reload_skills() {
    if let Ok(list) = api::get::<Vec<Skill>>("/agents/skills").await {
        *SKILLS.write() = Some(list);
    }
}

/// Agent-Arten laden, falls noch nicht geschehen; liefert den aktuellen Stand.
pub fn use_agents() -> Option<Vec<AgentInfo>> {
    use_hook(|| {
        if AGENTS.peek().is_none() {
            spawn(reload_agents());
        }
    });
    AGENTS.read().clone()
}

/// Sekundentakt starten (einmal je Komponente, die ihn braucht).
pub fn use_clock() -> i64 {
    use_future(|| async {
        loop {
            gloo_timers::future::TimeoutFuture::new(1000).await;
            let now = crate::util::now();
            if *NOW.peek() != now {
                *NOW.write() = now;
            }
        }
    });
    NOW()
}

pub fn info(kind: AgentKind) -> Option<AgentInfo> {
    AGENTS.read().as_ref().and_then(|l| l.iter().find(|a| a.kind == kind).cloned())
}

/// Modell zum Anzeigen: Name aus den Vorschlägen, sonst die ID; leer = „Standard“.
pub fn model_label(kind: AgentKind, id: &str) -> String {
    if id.is_empty() {
        return tr("Standard");
    }
    info(kind)
        .and_then(|i| i.models.iter().find(|m| m.id == id).map(|m| tr(&m.label)))
        .unwrap_or_else(|| id.to_string())
}

/// Denk-Stufe zum Anzeigen.
pub fn effort_label(e: &str) -> String {
    match e {
        "low" => tr("Niedrig"),
        "medium" => tr("Mittel"),
        "high" => tr("Hoch"),
        "xhigh" => tr("Extra hoch"),
        "max" => tr("Max"),
        _ => String::new(),
    }
}

/// 25s, 1m 20s, 2h 5m
pub fn duration(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        let r = s % 60;
        if r == 0 { format!("{}m", s / 60) } else { format!("{}m {}s", s / 60, r) }
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

/// Laufzeit eines Threads (aktueller Lauf) in Sekunden.
pub fn running_for(t: &ThreadSummary, now: i64) -> i64 {
    t.run_started_at.map(|s| now - s).unwrap_or(0).max(0)
}

/// Gewählter Agent mit Konto, Modell und Stufe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Selection {
    pub agent: AgentKind,
    pub account_id: Option<i64>,
    pub model: String,
    pub effort: String,
}

const SELECTION_KEY: &str = "nrz-agent-selection";

impl Selection {
    pub fn of_thread(t: &ThreadSummary) -> Self {
        Selection { agent: t.agent, account_id: t.account_id, model: t.model.clone(), effort: t.effort.clone() }
    }

    /// Zuletzt gewählte Auswahl (Browser), sonst der erste Agent mit Konto.
    pub fn remembered() -> Self {
        if let Some(s) = crate::util::storage_get(SELECTION_KEY).and_then(|t| serde_json::from_str::<Selection>(&t).ok()) {
            return s;
        }
        Selection::default()
    }

    pub fn remember(&self) {
        if let Ok(t) = serde_json::to_string(self) {
            crate::util::storage_set(SELECTION_KEY, &t);
        }
    }

    /// Konto gültig machen: gegebenes Konto der Art, sonst das Standardkonto.
    pub fn normalized(mut self, agents: &[AgentInfo]) -> Self {
        let Some(info) = agents.iter().find(|a| a.kind == self.agent) else { return self };
        if !info.accounts.iter().any(|a| Some(a.id) == self.account_id) {
            self.account_id = info.accounts.iter().find(|a| a.is_default).or(info.accounts.first()).map(|a| a.id);
        }
        if !self.effort.is_empty() && !info.efforts.contains(&self.effort) {
            self.effort.clear();
        }
        self
    }

    /// Anderer Agent: Standardkonto, dessen Modell-Vorgabe, Stufe nur falls unterstützt.
    pub fn with_agent(&self, kind: AgentKind, agents: &[AgentInfo]) -> Self {
        let mut s = Selection { agent: kind, account_id: None, model: String::new(), effort: self.effort.clone() };
        s = s.normalized(agents);
        if let Some(acc) = agents.iter().find(|a| a.kind == kind).and_then(|i| i.accounts.iter().find(|a| Some(a.id) == s.account_id)) {
            s.model = acc.default_model.clone();
            if !acc.default_effort.is_empty() {
                s.effort = acc.default_effort.clone();
            }
        }
        s
    }

    /// „Opus 5.5 Hoch“
    pub fn label(&self) -> String {
        let m = model_label(self.agent, &self.model);
        let e = effort_label(&self.effort);
        if e.is_empty() { m } else { format!("{m} {e}") }
    }
}

/// Text „1 Subagent“ / „4 Subagenten“.
pub fn subagents_text(n: usize) -> String {
    if n == 1 { tr("1 Subagent") } else { trf("{0} Subagenten", &[&n]) }
}
