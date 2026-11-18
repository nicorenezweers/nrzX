//! Einstellungen → KI-Ketten: Reihenfolge der Abos, wenn eines am Limit ist
//! (z. B. Claude Abo 1 → Claude Abo 2 → Grok Abo 1). Der Thread wechselt dann von selbst und
//! macht sofort weiter.

use super::data::{model_label, use_agents};
use super::icons::AgentIcon;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;
use crate::i18n::{tr, trf};
use dioxus::prelude::*;
use nrzx_shared::agents::{AgentInfo, AiChain, ChainSettings, ChainStep};

/// Konto-ID → (Agent-Info, Beschriftung).
fn account_of(agents: &[AgentInfo], id: i64) -> Option<(AgentInfo, String)> {
    agents.iter().find_map(|a| a.accounts.iter().find(|x| x.id == id).map(|x| (a.clone(), x.label.clone())))
}

#[component]
pub fn ChainsSettings() -> Element {
    let agents = use_agents();
    let mut data = use_signal(|| None::<ChainSettings>);
    let mut dirty = use_signal(|| false);
    let mut busy = use_signal(|| false);
    use_future(move || async move {
        match api::get::<ChainSettings>("/chains").await {
            Ok(s) => data.set(Some(s)),
            Err(e) => toast_error(e),
        }
    });
    let Some(d) = data() else {
        return rsx! { Loading {} };
    };
    let agents = agents.unwrap_or_default();
    let accounts: Vec<(i64, AgentInfo, String)> = agents.iter().flat_map(|a| a.accounts.iter().map(move |x| (x.id, a.clone(), x.label.clone()))).collect();
    let mut edit = move |f: Box<dyn FnOnce(&mut Vec<AiChain>)>| {
        let mut d = data.peek().clone().unwrap_or_default();
        f(&mut d.chains);
        data.set(Some(d));
        dirty.set(true);
    };
    let save = move |_| {
        let body = data.peek().clone().unwrap_or_default();
        busy.set(true);
        spawn(async move {
            match api::put::<ChainSettings, _>("/chains", &body).await {
                Ok(s) => {
                    data.set(Some(s));
                    dirty.set(false);
                }
                Err(e) => toast_error(e),
            }
            busy.set(false);
        });
    };
    let all_ids: Vec<i64> = accounts.iter().map(|a| a.0).collect();
    let now = crate::util::now();
    rsx! {
        Section {
            title: tr("KI-Ketten"),
            description: tr("Hast du mehrere Abos, legt eine Kette fest, wer übernimmt, wenn eines am Limit ist. Der Thread wechselt dann von selbst auf die nächste Stufe und macht sofort weiter – mit dem bisherigen Verlauf."),
            div { class: "grid max-w-3xl gap-4",
                if accounts.len() < 2 {
                    Callout { text: tr("Für eine Kette braucht es mindestens zwei Konten (Einstellungen → Agenten).") }
                }
                for (ci , c) in d.chains.iter().cloned().enumerate() {
                    div { key: "{c.id}-{ci}", class: if c.enabled { "rounded-xl border bg-card p-4 shadow-sm" } else { "rounded-xl border bg-card/50 p-4 opacity-80" },
                        div { class: "mb-3 flex flex-wrap items-center gap-2",
                            icons::Workflow { class: "size-4 text-muted-foreground" }
                            input {
                                class: "{INPUT} h-8 max-w-64 flex-1",
                                value: "{c.name}",
                                placeholder: tr("Name der Kette"),
                                oninput: move |e| {
                                    let v = e.value();
                                    edit(Box::new(move |l| l[ci].name = v));
                                },
                            }
                            div { class: "flex-1" }
                            Toggle {
                                checked: c.enabled,
                                onchange: move |v: bool| edit(Box::new(move |l| l[ci].enabled = v)),
                                label: if c.enabled { tr("Aktiv") } else { tr("Aus") },
                            }
                            button {
                                class: "grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-destructive",
                                title: tr("Kette löschen"),
                                onclick: move |_| edit(Box::new(move |l| {
                                    l.remove(ci);
                                })),
                                icons::Trash2 { class: "size-4" }
                            }
                        }
                        // Ablauf auf einen Blick
                        if !c.steps.is_empty() {
                            div { class: "mb-3 flex flex-wrap items-center gap-1.5 text-xs",
                                for (si , st) in c.steps.iter().enumerate() {
                                    if si > 0 {
                                        icons::ArrowRight { class: "size-3.5 text-muted-foreground" }
                                    }
                                    span { class: "inline-flex items-center gap-1.5 rounded-full border bg-background px-2 py-0.5",
                                        match account_of(&agents, st.account_id) {
                                            Some((a, label)) => rsx! {
                                                AgentIcon { kind: a.kind, size: 12 }
                                                "{a.name} · {label}"
                                            },
                                            None => rsx! { {tr("Konto fehlt")} },
                                        }
                                        if let Some(b) = d.blocked.iter().find(|b| b.account_id == st.account_id && b.until > now) {
                                            span { class: "text-warning", title: tr("Am Limit – wird übersprungen"), {trf("· am Limit bis {0}", &[&crate::util::clock(b.until)])} }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "grid gap-2",
                            for (si , st) in c.steps.iter().cloned().enumerate() {
                                StepRow {
                                    key: "{si}-{st.account_id}",
                                    index: si,
                                    count: c.steps.len(),
                                    step: st.clone(),
                                    agents: agents.clone(),
                                    taken: c.steps.iter().filter(|x| x.account_id != st.account_id).map(|x| x.account_id).collect::<Vec<_>>(),
                                    onchange: move |n: ChainStep| edit(Box::new(move |l| l[ci].steps[si] = n)),
                                    onmove: move |up: bool| edit(Box::new(move |l| {
                                        let s = &mut l[ci].steps;
                                        let j = if up { si.saturating_sub(1) } else { (si + 1).min(s.len() - 1) };
                                        s.swap(si, j);
                                    })),
                                    onremove: move |_| edit(Box::new(move |l| {
                                        l[ci].steps.remove(si);
                                    })),
                                }
                            }
                            {
                                let free: Vec<i64> = all_ids.iter().copied().filter(|id| !c.steps.iter().any(|s| s.account_id == *id)).collect();
                                rsx! {
                                    if let Some(first) = free.first().copied() {
                                        button {
                                            class: "inline-flex h-8 w-fit items-center gap-1.5 rounded-md border border-dashed px-3 text-sm text-muted-foreground hover:bg-accent hover:text-foreground",
                                            onclick: move |_| edit(Box::new(move |l| l[ci].steps.push(ChainStep { account_id: first, model: String::new() }))),
                                            icons::Plus { class: "size-3.5" }
                                            {tr("Stufe hinzufügen")}
                                        }
                                    }
                                }
                            }
                        }
                        if c.steps.len() < 2 {
                            p { class: "mt-2 text-xs text-muted-foreground", {tr("Mindestens zwei Stufen – sonst gibt es nichts zum Wechseln.")} }
                        }
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    button {
                        class: "{btn(BTN_OUTLINE)} {BTN_SM}",
                        disabled: accounts.len() < 2,
                        onclick: {
                            let all = all_ids.clone();
                            move |_| {
                                let all = all.clone();
                                edit(Box::new(move |l| {
                                    let n = l.len() + 1;
                                    l.push(AiChain {
                                        id: format!("c{}", crate::util::now()),
                                        name: if n == 1 { tr("Haupt-Kette") } else { trf("Kette {0}", &[&n]) },
                                        enabled: true,
                                        steps: all.iter().take(3).map(|id| ChainStep { account_id: *id, model: String::new() }).collect(),
                                    });
                                }));
                            }
                        },
                        icons::Plus {}
                        {tr("Neue Kette")}
                    }
                    div { class: "flex-1" }
                    if dirty() {
                        span { class: "text-xs text-muted-foreground", {tr("Nicht gespeichert")} }
                    }
                    button { class: "{btn(BTN_PRIMARY)} {BTN_SM}", disabled: !dirty() || busy(), onclick: save,
                        crate::pages::workspace::parts::BusyIcon { busy: busy() }
                        {tr("Speichern")}
                    }
                }
                div { class: "rounded-lg border bg-muted/30 p-3 text-xs leading-relaxed text-muted-foreground",
                    p { {tr("So funktioniert es: Bricht ein Lauf mit einem Nutzungslimit ab, merkt sich nrzX das Konto bis zum Zurücksetzen und gibt den Thread an die nächste Stufe der Kette, die nicht selbst am Limit ist. Vor jedem Lauf prüft nrzX das auch – ein Konto am Limit wird gleich übersprungen.")} }
                    p { class: "mt-1.5", {tr("Bei einem anderen Agenten (z. B. Claude → Grok) beginnt eine neue Sitzung; der Verlauf wird mitgegeben. Ohne passende Kette wartet der Thread wie bisher bis zum Zurücksetzen.")} }
                }
            }
        }
    }
}

/// Eine Stufe: Konto (bestimmt den Agenten) und optional ein Modell.
#[component]
fn StepRow(
    index: usize,
    count: usize,
    step: ChainStep,
    agents: Vec<AgentInfo>,
    taken: Vec<i64>,
    onchange: EventHandler<ChainStep>,
    onmove: EventHandler<bool>,
    onremove: EventHandler<()>,
) -> Element {
    let current = account_of(&agents, step.account_id);
    let models = current.as_ref().map(|(a, _)| a.models.clone()).unwrap_or_default();
    let kind = current.as_ref().map(|(a, _)| a.kind);
    let small = "grid size-7 place-items-center rounded text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-30 disabled:pointer-events-none";
    rsx! {
        div { class: "flex flex-wrap items-center gap-2 rounded-lg border bg-background px-2.5 py-2",
            span { class: "grid size-6 shrink-0 place-items-center rounded-full bg-muted text-xs font-semibold tabular-nums", "{index + 1}" }
            select {
                class: "{SELECT} h-8 min-w-48 flex-1",
                onchange: {
                    let step = step.clone();
                    move |e: FormEvent| {
                        if let Ok(id) = e.value().parse::<i64>() {
                            onchange.call(ChainStep { account_id: id, model: if id == step.account_id { step.model.clone() } else { String::new() } });
                        }
                    }
                },
                if current.is_none() {
                    option { value: "", selected: true, {tr("Konto wählen")} }
                }
                for a in agents.iter() {
                    for acc in a.accounts.iter().filter(|x| x.id == step.account_id || !taken.contains(&x.id)) {
                        option { key: "{acc.id}", value: "{acc.id}", selected: acc.id == step.account_id, "{a.name} · {acc.label}" }
                    }
                }
            }
            select {
                class: "{SELECT} h-8 w-44",
                title: tr("Modell dieser Stufe"),
                onchange: {
                    let step = step.clone();
                    move |e: FormEvent| onchange.call(ChainStep { account_id: step.account_id, model: e.value() })
                },
                option { value: "", selected: step.model.is_empty(), {tr("Modell: wie bisher / Standard")} }
                for m in models.iter() {
                    option { key: "{m.id}", value: "{m.id}", selected: step.model == m.id, {kind.map(|k| model_label(k, &m.id)).unwrap_or_else(|| m.label.clone())} }
                }
                if !step.model.is_empty() && !models.iter().any(|m| m.id == step.model) {
                    option { value: "{step.model}", selected: true, "{step.model}" }
                }
            }
            div { class: "flex items-center",
                button { class: small, title: tr("Nach oben"), disabled: index == 0, onclick: move |_| onmove.call(true), icons::ChevronUp { class: "size-4" } }
                button { class: small, title: tr("Nach unten"), disabled: index + 1 >= count, onclick: move |_| onmove.call(false), icons::ChevronDown { class: "size-4" } }
                button { class: small, title: tr("Stufe entfernen"), onclick: move |_| onremove.call(()), icons::X { class: "size-4" } }
            }
        }
    }
}
