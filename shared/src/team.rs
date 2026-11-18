//! Team: mehrere Leute arbeiten – jeder mit seinem eigenen nrzX – am selben Repository, und
//! ihre Agenten sprechen miteinander. Der gemeinsame Stand ([`TeamDoc`]) liegt als `team.json` in
//! der Referenz `refs/nrzx/team` des Repositorys; jede Instanz holt und schreibt ihn mit git.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Referenz im gemeinsamen Repository.
pub const TEAM_REF: &str = "refs/nrzx/team";
/// Datei im Baum der Referenz.
pub const TEAM_FILE: &str = "team.json";
/// Höchstens so viele Nachrichten bleiben im gemeinsamen Stand.
pub const MAX_MESSAGES: usize = 300;
/// Nachrichten älter als das (Sekunden) fallen heraus.
pub const MAX_AGE: i64 = 14 * 86_400;
/// Längste Nachricht (Zeichen).
pub const MAX_TEXT: usize = 6000;
/// Höchstens so viele Agenten meldet ein Mitglied.
pub const MAX_AGENTS: usize = 12;

/// Gemeinsamer Stand eines Teams (Inhalt von `team.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TeamDoc {
    pub v: u32,
    /// Mitglieder nach Schlüssel (Benutzername auf der Git-Plattform, klein geschrieben)
    pub members: BTreeMap<String, TeamMember>,
    /// Nachrichten, älteste zuerst
    pub messages: Vec<TeamMessage>,
}

/// Ein Mensch im Team mit den Agenten, die bei ihm an diesem Repository arbeiten.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TeamMember {
    /// Benutzername, wie er geschrieben wird
    pub name: String,
    pub full_name: String,
    /// Zuletzt gemeldet (Unix-Sekunden)
    pub seen_at: i64,
    pub agents: Vec<TeamAgent>,
}

/// Ein Agent (Thread) eines Mitglieds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TeamAgent {
    /// Nummer des Threads bei diesem Mitglied – Adresse: `name#id`
    pub id: i64,
    pub title: String,
    /// claude | codex | gemini …
    pub agent: String,
    /// working | waiting | idle | done | error …
    pub status: String,
    pub branch: String,
}

/// Nachricht im Team – von einem Menschen oder einem Agenten, an alle, einen Menschen oder
/// einen bestimmten Agenten.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TeamMessage {
    pub id: String,
    /// Schlüssel des Absenders
    pub from: String,
    /// Thread des Absenders, wenn ein Agent schreibt
    pub from_agent: Option<i64>,
    /// Titel dieses Threads (zur Anzeige)
    pub from_title: String,
    /// Schlüssel des Empfängers; leer = alle
    pub to: String,
    /// Bestimmter Agent (Thread) des Empfängers
    pub to_agent: Option<i64>,
    pub text: String,
    /// Antwort auf diese Nachricht
    pub reply_to: Option<String>,
    pub at: i64,
}

impl TeamMessage {
    /// Absender als Adresse: `name` oder `name#12`.
    pub fn sender(&self) -> String {
        address(&self.from, self.from_agent)
    }

    /// Empfänger als Adresse: `all`, `name` oder `name#12`.
    pub fn recipient(&self) -> String {
        if self.to.is_empty() {
            "all".into()
        } else {
            address(&self.to, self.to_agent)
        }
    }
}

/// `name` oder `name#12`.
pub fn address(member: &str, agent: Option<i64>) -> String {
    match agent {
        Some(id) => format!("{member}#{id}"),
        None => member.to_string(),
    }
}

/// Adresse zerlegen: `all`/leer → (leer, None), `Name` → (name, None), `name#12` → (name, 12).
/// Schlüssel sind klein geschrieben; ein `@` davor wird überlesen.
pub fn parse_address(s: &str) -> Option<(String, Option<i64>)> {
    let s = s.trim().trim_start_matches('@');
    if s.is_empty() || matches!(s.to_lowercase().as_str(), "all" | "alle" | "team" | "everyone" | "*") {
        return Some((String::new(), None));
    }
    match s.split_once('#') {
        Some((m, a)) => {
            let id: i64 = a.trim().parse().ok()?;
            let m = m.trim().to_lowercase();
            (!m.is_empty() && id > 0).then_some((m, Some(id)))
        }
        None => Some((s.to_lowercase(), None)),
    }
}

/// Team eines Projekts für die Oberfläche (`GET /projects/<id>/team`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TeamView {
    pub project_id: i64,
    pub project_name: String,
    /// Lässt sich ein Team einschalten? (Das Projekt braucht ein Remote „origin“.)
    pub available: bool,
    pub enabled: bool,
    /// Agenten antworten selbstständig auf Nachrichten, die direkt an sie gehen
    pub auto_reply: bool,
    /// Eigener Schlüssel im Team
    pub me: String,
    pub members: Vec<TeamMember>,
    pub messages: Vec<TeamMessage>,
    /// Letzter erfolgreicher Abgleich (Unix-Sekunden; 0 = noch nie)
    pub synced_at: i64,
    /// Fehler des letzten Abgleichs (leer = keiner)
    pub error: String,
    /// Noch nicht übertragene eigene Nachrichten
    pub pending: i64,
}

/// `PUT /projects/<id>/team`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TeamSettings {
    pub enabled: Option<bool>,
    pub auto_reply: Option<bool>,
}

/// `POST /projects/<id>/team/messages`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TeamPost {
    /// `all`, `name` oder `name#12`
    pub to: String,
    pub text: String,
    pub reply_to: Option<String>,
}

/// Zwei Stände zusammenführen: Nachrichten vereinigen (nach `id`), je Mitglied gewinnt der
/// neuere Eintrag. Danach kürzen ([`TeamDoc::prune`]).
pub fn merge(mut a: TeamDoc, b: &TeamDoc, now: i64) -> TeamDoc {
    for (k, m) in &b.members {
        match a.members.get(k) {
            Some(x) if x.seen_at >= m.seen_at => {}
            _ => {
                a.members.insert(k.clone(), m.clone());
            }
        }
    }
    for m in &b.messages {
        if !a.messages.iter().any(|x| x.id == m.id) {
            a.messages.push(m.clone());
        }
    }
    a.v = 1;
    a.prune(now);
    a
}

impl TeamDoc {
    /// Sortieren und begrenzen: alte Nachrichten und lange nicht gesehene Mitglieder entfernen.
    pub fn prune(&mut self, now: i64) {
        self.messages.retain(|m| !m.id.is_empty() && m.at > now - MAX_AGE);
        self.messages.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
        if self.messages.len() > MAX_MESSAGES {
            let cut = self.messages.len() - MAX_MESSAGES;
            self.messages.drain(..cut);
        }
        self.members.retain(|_, m| m.seen_at > now - 2 * MAX_AGE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(parse_address("all"), Some((String::new(), None)));
        assert_eq!(parse_address(""), Some((String::new(), None)));
        assert_eq!(parse_address("@Luna"), Some(("luna".into(), None)));
        assert_eq!(parse_address("Luna#12"), Some(("luna".into(), Some(12))));
        assert_eq!(parse_address("luna#x"), None);
        assert_eq!(address("luna", Some(3)), "luna#3");
    }

    #[test]
    fn merging() {
        let msg = |id: &str, at: i64| TeamMessage { id: id.into(), from: "a".into(), text: id.into(), at, ..Default::default() };
        let mut a = TeamDoc::default();
        a.messages = vec![msg("1", 100), msg("3", 300)];
        a.members.insert("a".into(), TeamMember { name: "a".into(), seen_at: 100, ..Default::default() });
        let mut b = TeamDoc::default();
        b.messages = vec![msg("2", 200), msg("3", 300)];
        b.members.insert("a".into(), TeamMember { name: "A".into(), seen_at: 200, ..Default::default() });
        b.members.insert("b".into(), TeamMember { name: "b".into(), seen_at: 150, ..Default::default() });
        let m = merge(a, &b, 400);
        assert_eq!(m.messages.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
        assert_eq!(m.members["a"].name, "A");
        assert_eq!(m.members.len(), 2);
    }
}
