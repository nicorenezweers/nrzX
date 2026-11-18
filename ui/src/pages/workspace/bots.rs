//! Bot-Modus (Knopf „Bots“ neben dem Einklapp-Knopf): eigene KI-Teamkollegen, jeder Experte für
//! eine Sache. Jeder Bot hat Persönlichkeit, ein Gedächtnis, das er selbst pflegt (was man ihm
//! sagt, kann er sich merken), einen eigenen Arbeitsordner, Routinen, einen eigenen Chat und eigene
//! MCP-Server, die nur er nutzt. Ein Gespräch ist ein Thread mit `bot_id` – die Unterhaltung zeigt
//! `agents::ChatView` (mit Composer) wie bei Code-Threads.
//!
//! * `Bots` – Übersicht mit Karten und Vorlagen
//! * `BotPage` – Gespräch mit einem Bot (Kopf mit Gedächtnis, Routinen, Apps, früheren Gesprächen,
//!   Bearbeiten); offene Gespräche stehen wie die Chats eines Threads als Reiter oben
//!   (`conversations`, `tabs::ThreadTabs`), „+“ beginnt ein neues
//! * `BotList` – Liste in der Seitenleiste (statt der Aufgaben)
//! * `BotEditor` – Dialog: Profil, Gedächtnis, Routinen, Apps (MCP)

use super::parts::*;
use crate::Route;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;
use crate::i18n::{tr, trf};
use crate::state::{self, BOTS, THREADS};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::agents::{Bot, BotInput, BotMcp, BotRoutine, BotRoutineInput};
use nrzx_shared::{AgentKind, ThreadSummary};
use std::collections::HashMap;

/// Offener Editor: `Some(bot)` – `bot.id == 0` = neuer Bot (vorausgefüllt, z. B. aus einer Vorlage).
pub static EDIT_BOT: GlobalSignal<Option<(Bot, EditTab)>> = Signal::global(|| None);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EditTab {
    Profile,
    Memory,
    Routines,
    Apps,
}

/// Zuletzt offener Bot (Ziel des Umschalters).
const LAST_BOT_KEY: &str = "nrz-last-bot";

/// Wohin der Umschalter „Bot“ führt: zum zuletzt offenen Bot, sonst zur Übersicht.
pub fn bot_route() -> Route {
    let last = util::storage_get(LAST_BOT_KEY).and_then(|s| s.parse::<i64>().ok());
    let known = BOTS.peek().as_ref().map(|l| l.iter().any(|b| Some(b.id) == last)).unwrap_or(last.is_some());
    match last {
        Some(id) if known => Route::BotPage { id },
        _ => Route::Bots {},
    }
}

/// Bots einmal laden, falls noch nicht geschehen.
fn use_bots() -> Option<Vec<Bot>> {
    use_hook(|| {
        if BOTS.peek().is_none() {
            spawn(state::load_bots());
        }
    });
    BOTS.read().clone()
}

/// Vorlagen für neue Bots: (Emoji, Name, Rolle, Anweisung).
fn templates() -> Vec<(&'static str, String, String, String)> {
    vec![
        ("px:205", tr("Scout"), tr("Recherche"), tr("Recherchiere gründlich im Web, vergleiche mehrere Quellen und nenne sie. Fasse Ergebnisse knapp mit Stichpunkten zusammen und sage, wie sicher du dir bist.")),
        ("px:330", tr("Feder"), tr("Schreibpartner"), tr("Hilf mir beim Schreiben: entwirf, kürze und poliere Texte in meinem Ton. Frag nach Zielgruppe und Länge, wenn sie unklar sind.")),
        ("px:150", tr("Kompass"), tr("Persönlicher Assistent"), tr("Behalte meine Aufgaben, Termine und Vorhaben im Blick. Merke dir Wichtiges, plane mit mir den Tag und erinnere mich an Offenes.")),
        ("px:30", tr("Funke"), tr("Sparringspartner"), tr("Denk Ideen mit mir durch, stelle kritische Fragen, zeige blinde Flecken und schlage Alternativen vor. Sei ehrlich statt höflich.")),
    ]
}

/// Neuen Bot im Editor beginnen (leer oder aus einer Vorlage).
pub fn new_bot(template: Option<(&str, String, String, String)>) {
    let pick = HUES[(js_sys::Date::now() as usize / 7) % HUES.len()];
    let mut b = Bot { emoji: format!("px:{pick}"), ..Default::default() };
    if let Some((e, n, r, i)) = template {
        b.emoji = e.into();
        b.name = n;
        b.role = r;
        b.instructions = i;
    }
    *EDIT_BOT.write() = Some((b, EditTab::Profile));
}

// ------------------------------------------------------------------ Bausteine

/// Pixel-Katze (11 × 10): `E` Rand, `B` Fell, `W` Auge, `P` Pupille, `N` Nase.
const CAT: [&str; 10] = [
    ".E.......E.",
    ".EE.....EE.",
    ".EBBBBBBBE.",
    "EBBBBBBBBBE",
    "EBWWBBBWWBE",
    "EBWPBBBWPBE",
    "EBBBBNBBBBE",
    "EBBBBBBBBBE",
    ".EBBBBBBBE.",
    "..EEEEEEE..",
];

/// Farbtöne der Pixel-Avatare zur Auswahl.
const HUES: [u32; 8] = [265, 330, 205, 150, 30, 0, 50, 185];

/// Farbton eines Pixel-Avatars (`px:<farbton>`), sonst `None` (Emoji).
fn pixel_hue(emoji: &str, name: &str) -> Option<u32> {
    let e = emoji.trim();
    if e.is_empty() {
        return Some(hue(name));
    }
    e.strip_prefix("px:").map(|h| h.parse().unwrap_or_else(|_| hue(name)))
}

/// Avatar des Bots: Pixel-Katze in seiner Farbe (`px:<farbton>`) oder ein Emoji auf einer Fläche.
#[component]
pub fn BotAvatar(name: String, emoji: String, #[props(default = 28)] size: u32) -> Element {
    if let Some(h) = pixel_hue(&emoji, &name) {
        let mut cells = Vec::new();
        for (y, row) in CAT.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                let fill = match c {
                    'E' => format!("hsl({h} 55% 40%)"),
                    'B' => format!("hsl({h} 70% 62%)"),
                    'W' => "#f4f4f6".to_string(),
                    'P' => "#1b1b20".to_string(),
                    'N' => "hsl(340 75% 72%)".to_string(),
                    _ => continue,
                };
                cells.push((x, y, fill));
            }
        }
        return rsx! {
            svg { class: "shrink-0", view_box: "-0.5 -1 12 12", width: "{size}", height: "{size}", "shape-rendering": "crispEdges",
                for (i , (x , y , f)) in cells.into_iter().enumerate() {
                    rect { key: "{i}", x: "{x}", y: "{y}", width: "1.02", height: "1.02", fill: "{f}" }
                }
            }
        };
    }
    let h = hue(&name);
    let font = (size as f32 * 0.55).round() as u32;
    let radius = (size as f32 * 0.3).round() as u32;
    let shown = if emoji.trim().is_empty() { initials(&name) } else { emoji };
    rsx! {
        span {
            class: "inline-grid shrink-0 select-none place-items-center font-semibold",
            style: "width: {size}px; height: {size}px; font-size: {font}px; border-radius: {radius}px; background: linear-gradient(135deg, hsl({h} 70% 55% / 0.28), hsl({h} 70% 40% / 0.14)); color: hsl({h} 80% 70%); box-shadow: inset 0 0 0 1px hsl({h} 70% 60% / 0.25)",
            "{shown}"
        }
    }
}

fn current_thread(b: &Bot) -> Option<ThreadSummary> {
    b.thread_id.and_then(|t| THREADS.read().get(&t).cloned())
}

fn memory_count(memory: &str) -> usize {
    memory.lines().filter(|l| !l.trim().is_empty()).count()
}

// ------------------------------------------------------------------ Seitenleiste

/// Bots in der Seitenleiste (Suche filtert nach Name und Rolle).
#[component]
pub fn BotList(query: String) -> Element {
    let route = use_route::<Route>();
    let bots = use_bots();
    let current = match route {
        Route::BotPage { id } => Some(id),
        _ => None,
    };
    let q = query.trim().to_lowercase();
    let Some(list) = bots else {
        return rsx! { div { class: "px-3 py-6 text-center text-sm text-muted-foreground", {tr("Lädt …")} } };
    };
    let shown: Vec<Bot> = list.into_iter().filter(|b| q.is_empty() || b.name.to_lowercase().contains(&q) || b.role.to_lowercase().contains(&q)).collect();
    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col overflow-y-auto overflow-x-hidden px-2 pt-3 pb-2 [&>*]:min-w-0",
            if shown.is_empty() {
                div { class: "px-3 py-8 text-center text-sm text-muted-foreground",
                    p { if q.is_empty() { {tr("Noch keine Bots.")} } else { {tr("Keine Treffer.")} } }
                    if q.is_empty() {
                        button { class: "mt-3 inline-flex items-center gap-1.5 text-foreground hover:underline", onclick: move |_| new_bot(None),
                            icons::Plus { class: "size-4" }
                            {tr("Ersten Bot erstellen")}
                        }
                    }
                }
            }
            div { class: "grid gap-0.5",
                for b in shown {
                    BotItem { key: "{b.id}", active: current == Some(b.id), bot: b.clone() }
                }
            }
        }
    }
}

#[component]
fn BotItem(bot: Bot, active: bool) -> Element {
    let thread = current_thread(&bot);
    // Darunter: Anfang seiner letzten Antwort, sonst die Rolle
    let snippet = thread.as_ref().map(|t| t.snippet.clone()).filter(|s| !s.trim().is_empty());
    let sub = snippet.unwrap_or_else(|| if bot.role.is_empty() { tr("Bot") } else { bot.role.clone() });
    let working = thread.as_ref().is_some_and(|t| t.status.is_active());
    rsx! {
        Link {
            to: Route::BotPage { id: bot.id },
            class: if active { "flex min-w-0 items-center gap-3 rounded-xl bg-accent/50 px-3 py-2.5" } else { "flex min-w-0 items-center gap-3 rounded-xl px-3 py-2.5 transition-colors hover:bg-accent/30" },
            span { class: "relative shrink-0",
                BotAvatar { name: bot.name.clone(), emoji: bot.emoji.clone(), size: 34 }
                if working {
                    span { class: "absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-[var(--nrx-surf)] bg-warning" }
                }
            }
            div { class: "min-w-0 flex-1",
                div { class: "flex min-w-0 items-center gap-2",
                    span { class: if thread.as_ref().is_some_and(|t| t.unread) { "min-w-0 flex-1 truncate text-[15px] font-semibold" } else { "min-w-0 flex-1 truncate text-[15px] font-medium" }, "{bot.name}" }
                    if let Some(t) = thread.clone() {
                        if t.status.is_active() || t.unread {
                            ThreadState { thread: t }
                        }
                    }
                }
                div { class: "truncate text-[13px] text-muted-foreground", "{sub}" }
            }
        }
    }
}

// ------------------------------------------------------------------ Übersicht

#[component]
pub fn Bots() -> Element {
    let bots = use_bots();
    let Some(list) = bots else {
        return rsx! { Loading {} };
    };
    rsx! {
        div { class: "mx-auto w-full max-w-4xl px-4 py-10 md:px-8",
            div { class: "mb-8 flex flex-wrap items-end gap-4",
                div { class: "min-w-0 flex-1",
                    h1 { class: "text-2xl font-semibold tracking-tight", {tr("Deine Bots")} }
                    p { class: "mt-1 text-sm text-muted-foreground",
                        {tr("KI-Teamkollegen, jeder Experte für eine Sache – mit eigenem Gedächtnis, eigenem Chat, eigenem Arbeitsordner, Routinen und eigenen Apps (MCP).")}
                    }
                }
                button { class: "{BTN} {BTN_PRIMARY}", onclick: move |_| new_bot(None),
                    icons::Plus {}
                    {tr("Neuer Bot")}
                }
            }
            if !list.is_empty() {
                div { class: "grid gap-3 sm:grid-cols-2",
                    for b in list.clone() {
                        BotCard { key: "{b.id}", bot: b.clone() }
                    }
                }
            }
            h2 { class: "mt-10 mb-3 text-sm font-medium text-muted-foreground", if list.is_empty() { {tr("Mit einer Vorlage beginnen")} } else { {tr("Vorlagen")} } }
            div { class: "grid gap-3 sm:grid-cols-2",
                for (i, (emoji, name, role, instr)) in templates().into_iter().enumerate() {
                    button {
                        key: "{i}",
                        class: "group flex items-start gap-3 rounded-2xl border border-dashed border-white/15 p-4 text-left transition-colors hover:border-solid hover:bg-white/[0.05]",
                        onclick: {
                            let (n, r, ins) = (name.clone(), role.clone(), instr.clone());
                            move |_| new_bot(Some((emoji, n.clone(), r.clone(), ins.clone())))
                        },
                        BotAvatar { name: name.clone(), emoji: emoji.to_string(), size: 40 }
                        div { class: "min-w-0 flex-1",
                            div { class: "flex items-center gap-2",
                                span { class: "font-medium", "{role}" }
                                icons::Plus { class: "size-3.5 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100" }
                            }
                            p { class: "mt-1 line-clamp-2 text-xs text-muted-foreground", "{instr}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn BotCard(bot: Bot) -> Element {
    let thread = current_thread(&bot);
    let mem = memory_count(&bot.memory);
    let routines = bot.routines.iter().filter(|r| r.enabled).count();
    rsx! {
        Link { to: Route::BotPage { id: bot.id }, class: "flex items-start gap-3 rounded-2xl border border-white/10 bg-white/[0.04] p-4 transition-colors hover:bg-white/[0.08]",
            BotAvatar { name: bot.name.clone(), emoji: bot.emoji.clone(), size: 44 }
            div { class: "min-w-0 flex-1",
                div { class: "flex items-center gap-2",
                    span { class: "min-w-0 flex-1 truncate font-medium", "{bot.name}" }
                    if let Some(t) = thread {
                        ThreadState { thread: t }
                    }
                }
                if !bot.role.is_empty() {
                    p { class: "truncate text-sm text-muted-foreground", "{bot.role}" }
                }
                div { class: "mt-2 flex flex-wrap items-center gap-3 text-xs text-muted-foreground [&_svg]:size-3.5",
                    span { class: "inline-flex items-center gap-1", icons::Brain {} {trf("{0} Erinnerungen", &[&mem])} }
                    if routines > 0 {
                        span { class: "inline-flex items-center gap-1", icons::CalendarClock {} {trf("{0} Routinen", &[&routines])} }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Gespräche als Reiter

/// Offene Gespräche je Bot – Reiter oben, Thread-IDs in Reiter-Reihenfolge; gemerkt im Browser
/// (`nrz-bot-tabs`, z. B. `3:10,12;5:7`). Das aktuelle Gespräch des Bots steht immer dabei.
static TABS: GlobalSignal<HashMap<i64, Vec<i64>>> = Signal::global(load_tabs);
const TABS_KEY: &str = "nrz-bot-tabs";

fn load_tabs() -> HashMap<i64, Vec<i64>> {
    util::storage_get(TABS_KEY)
        .unwrap_or_default()
        .split(';')
        .filter_map(|e| {
            let (bot, list) = e.split_once(':')?;
            Some((bot.parse().ok()?, list.split(',').filter_map(|s| s.parse().ok()).collect()))
        })
        .collect()
}

fn save_tabs(bot: i64, list: Vec<i64>) {
    let mut all = TABS.peek().clone();
    if list.len() > 1 {
        all.insert(bot, list);
    } else {
        all.remove(&bot);
    }
    let text = all.iter().map(|(b, l)| format!("{b}:{}", l.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(","))).collect::<Vec<_>>().join(";");
    if text.is_empty() {
        util::storage_remove(TABS_KEY);
    } else {
        util::storage_set(TABS_KEY, &text);
    }
    *TABS.write() = all;
}

/// Aktuelles Gespräch des Bots.
fn current(bot: i64) -> Option<i64> {
    BOTS.read().as_ref().and_then(|l| l.iter().find(|b| b.id == bot)).and_then(|b| b.thread_id)
}

/// Aktuelles Gespräch gleich hier setzen (der Server folgt) – der Wechsel ist sofort zu sehen.
fn set_current(bot: i64, tid: i64) {
    if let Some(b) = BOTS.write().as_mut().and_then(|l| l.iter_mut().find(|b| b.id == bot)) {
        b.thread_id = Some(tid);
    }
}

/// Gespräche des Bots als Reiter: die offenen, die es noch gibt, und das aktuelle.
pub fn conversations(bot: i64) -> Vec<i64> {
    let mut list: Vec<i64> = {
        let threads = THREADS.read();
        TABS.read().get(&bot).into_iter().flatten().copied().filter(|t| threads.get(t).is_some_and(|x| x.bot_id == Some(bot))).collect()
    };
    if let Some(c) = current(bot) {
        if !list.contains(&c) {
            list.push(c);
        }
    }
    list
}

/// Mehr als ein Gespräch offen? Dann stehen sie als Reiter oben.
pub fn has_tabs(bot: i64) -> bool {
    conversations(bot).len() > 1
}

/// Platz des aktuellen Gesprächs in den Reitern.
pub fn active_index(bot: i64) -> usize {
    let c = current(bot);
    conversations(bot).iter().position(|t| Some(*t) == c).unwrap_or(0)
}

/// Aktuelles Gespräch beim Server setzen (hier ist es schon gesetzt); schlägt es fehl, gilt
/// wieder der Stand des Servers.
fn switch_to(bot: i64, tid: i64) {
    spawn(async move {
        match api::action::<Bot>(&format!("/bots/{bot}/conversation/{tid}")).await {
            Ok(b) => state::put_bot(b),
            Err(e) => {
                toast_error(e);
                state::load_bots().await;
            }
        }
    });
}

/// Gespräch zeigen und zum aktuellen machen; es kommt als Reiter dazu, die übrigen bleiben.
pub fn open_conversation(bot: i64, tid: i64) {
    let mut list = conversations(bot);
    if !list.contains(&tid) {
        list.push(tid);
    }
    save_tabs(bot, list);
    if current(bot) != Some(tid) {
        set_current(bot, tid);
        switch_to(bot, tid);
    }
}

/// Neues Gespräch als weiterer Reiter (das bisherige bleibt offen, das Gedächtnis sowieso).
pub async fn new_conversation(bot: i64) {
    let mut list = conversations(bot);
    match api::action::<Bot>(&format!("/bots/{bot}/conversation")).await {
        Ok(b) => {
            if let Some(t) = b.thread_id.filter(|t| !list.contains(t)) {
                list.push(t);
            }
            save_tabs(bot, list);
            state::put_bot(b);
        }
        Err(e) => toast_error(e),
    }
}

/// Reiter schliessen – das Gespräch bleibt (im Menü unter „Gespräche“). War es das aktuelle, wird
/// das links daneben aktuell; das letzte bleibt offen.
pub fn close_conversation(bot: i64, tid: i64) {
    let mut list = conversations(bot);
    if list.len() < 2 {
        return;
    }
    let at = list.iter().position(|t| *t == tid).unwrap_or(0);
    list.retain(|t| *t != tid);
    let next = list[at.saturating_sub(1).min(list.len() - 1)];
    // Erst das neue aktuelle setzen – sonst stünde das geschlossene als aktuelles gleich wieder da
    let was_current = current(bot) == Some(tid);
    if was_current {
        set_current(bot, next);
    }
    save_tabs(bot, list);
    if was_current {
        switch_to(bot, next);
    }
}

// ------------------------------------------------------------------ Gespräch

#[component]
pub fn BotPage(id: i64) -> Element {
    rsx! {
        for bid in std::iter::once(id) {
            BotView { key: "{bid}", id: bid }
        }
    }
}

#[component]
fn BotView(id: i64) -> Element {
    let nav = use_navigator();
    let bots = use_bots();
    let mut busy = use_signal(|| false);
    use_hook(move || util::storage_set(LAST_BOT_KEY, &id.to_string()));
    // Sein Gespräch gilt als gezeigt (gelesen markieren); beim Verlassen wieder frei
    let shown_thread = bots.as_ref().and_then(|l| l.iter().find(|b| b.id == id)).and_then(|b| b.thread_id);
    use_effect(use_reactive!(|shown_thread| {
        if *state::SHOWN_THREAD.peek() != shown_thread {
            *state::SHOWN_THREAD.write() = shown_thread;
        }
    }));
    use_drop(move || {
        if state::SHOWN_THREAD.peek().is_some() {
            *state::SHOWN_THREAD.write() = None;
        }
    });
    let Some(list) = bots else {
        return rsx! { Loading {} };
    };
    let Some(bot) = list.into_iter().find(|b| b.id == id) else {
        return rsx! {
            EmptyState { title: tr("Diesen Bot gibt es nicht mehr."),
                Link { to: Route::Bots {}, class: "{BTN} {BTN_OUTLINE}", {tr("Zu den Bots")} }
            }
        };
    };
    let mut past: Vec<ThreadSummary> = THREADS.read().values().filter(|t| t.bot_id == Some(id) && t.parent_id.is_none()).cloned().collect();
    past.sort_by_key(|t| std::cmp::Reverse(t.last_activity_at));
    let begin = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            new_conversation(id).await;
            busy.set(false);
        });
    };
    // Mit mehreren offenen Gesprächen steht „+“ in der Reihe der Reiter (schmal bleibt der Knopf)
    let tabs = has_tabs(id);
    let edit = {
        let b = bot.clone();
        move |tab: EditTab| *EDIT_BOT.write() = Some((b.clone(), tab))
    };
    let edit2 = edit.clone();
    let edit3 = edit.clone();
    let edit4 = edit.clone();
    let apps = bot.mcp.iter().filter(|m| m.enabled).count();
    let mem = memory_count(&bot.memory);
    let routines = bot.routines.iter().filter(|r| r.enabled).count();
    let remove = {
        let name = bot.name.clone();
        move |_| {
            let name = name.clone();
            spawn(async move {
                let ask = Ask::new(trf("„{0}“ löschen?", &[&name]), tr("Alle Gespräche, das Gedächtnis, die Routinen und der Arbeitsordner des Bots werden gelöscht."))
                    .confirm(tr("Löschen"))
                    .destructive();
                if !confirm(ask).await {
                    return;
                }
                match api::delete(&format!("/bots/{id}")).await {
                    Ok(()) => {
                        if let Some(l) = BOTS.write().as_mut() {
                            l.retain(|b| b.id != id);
                        }
                        nav.replace(Route::Bots {});
                    }
                    Err(e) => toast_error(e),
                }
            });
        }
    };
    let chip = "inline-flex h-7 items-center gap-1.5 rounded-full border border-white/10 px-2.5 text-xs text-muted-foreground hover:bg-white/[0.08] hover:text-foreground [&_svg]:size-3.5";
    rsx! {
        div { class: "flex h-full min-h-0 flex-col",
            // Kopf: wer der Bot ist, was er sich merkt, was er regelmässig tut
            div { class: "flex shrink-0 flex-wrap items-center gap-3 border-b border-white/[0.08] px-4 py-3 md:px-6",
                BotAvatar { name: bot.name.clone(), emoji: bot.emoji.clone(), size: 40 }
                div { class: "min-w-0 flex-1",
                    div { class: "truncate font-semibold", "{bot.name}" }
                    div { class: "flex flex-wrap items-center gap-x-3 text-xs text-muted-foreground",
                        if !bot.role.is_empty() {
                            span { class: "truncate", "{bot.role}" }
                        }
                        span { class: "inline-flex items-center gap-1",
                            crate::pages::agents::AgentIcon { kind: bot.agent, size: 12 }
                            {crate::pages::agents::model_label(bot.agent, &bot.model)}
                        }
                    }
                }
                div { class: "flex flex-wrap items-center gap-1.5",
                    button { class: chip, title: tr("Was sich der Bot über Gespräche hinweg merkt"), onclick: move |_| edit(EditTab::Memory),
                        icons::Brain {}
                        {trf("{0} Erinnerungen", &[&mem])}
                    }
                    button { class: chip, title: tr("Aufträge, die der Bot regelmässig erledigt"), onclick: move |_| edit2(EditTab::Routines),
                        icons::CalendarClock {}
                        if routines > 0 { {trf("{0} Routinen", &[&routines])} } else { {tr("Routine")} }
                    }
                    button { class: chip, title: tr("MCP-Server, die nur dieser Bot nutzt"), onclick: move |_| edit4(EditTab::Apps),
                        icons::Blocks {}
                        if apps > 0 { {trf("{0} Apps", &[&apps])} } else { {tr("Apps")} }
                    }
                    button { class: if tabs { "{BTN} {BTN_OUTLINE} {BTN_SM} md:hidden" } else { "{BTN} {BTN_OUTLINE} {BTN_SM}" }, disabled: busy(), title: tr("Neues Gespräch – das Gedächtnis bleibt"), onclick: begin,
                        icons::MessageSquarePlus {}
                        span { class: "hidden sm:inline", {tr("Neues Gespräch")} }
                    }
                    Dropdown {
                        align: Align::End,
                        class: "max-h-96 w-72 overflow-y-auto",
                        trigger_class: "grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground [&_svg]:size-4".to_string(),
                        trigger: rsx! { icons::Ellipsis {} },
                        MenuItem { onclick: move |_| edit3(EditTab::Profile),
                            icons::Pencil {}
                            {tr("Bot bearbeiten")}
                        }
                        if past.len() > 1 {
                            MenuSeparator {}
                            MenuLabel { {tr("Gespräche")} }
                            for t in past.clone() {
                                MenuItem { key: "{t.id}", active: bot.thread_id == Some(t.id), onclick: move |_| open_conversation(id, t.id),
                                    icons::History {}
                                    span { class: "min-w-0 flex-1 truncate", {util::datetime(t.created_at)} }
                                    span { class: "text-xs text-muted-foreground", {short_ago(t.last_activity_at)} }
                                }
                            }
                        }
                        MenuSeparator {}
                        MenuItem { onclick: remove,
                            icons::Trash2 { class: "text-destructive" }
                            span { class: "text-destructive", {tr("Bot löschen")} }
                        }
                    }
                }
            }
            div { class: "relative min-h-0 flex-1",
                match bot.thread_id {
                    Some(tid) => rsx! {
                        div { key: "{tid}", class: "absolute inset-0", crate::pages::agents::ChatView { thread_id: tid } }
                    },
                    None => rsx! {
                        div { class: "grid h-full place-items-center p-6",
                            div { class: "text-center",
                                BotAvatar { name: bot.name.clone(), emoji: bot.emoji.clone(), size: 64 }
                                p { class: "mt-4 text-sm text-muted-foreground", {tr("Kein offenes Gespräch.")} }
                                button { class: "{BTN} {BTN_PRIMARY} mt-4", disabled: busy(), onclick: begin,
                                    icons::MessageSquarePlus {}
                                    {tr("Gespräch beginnen")}
                                }
                            }
                        }
                    },
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Editor

const EMOJIS: [&str; 8] = ["🤖", "🦊", "🧠", "📈", "🎨", "📚", "🛠️", "⚡"];
const DAYS: [&str; 7] = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"];

/// Einmal im Rahmen einbinden.
#[component]
pub fn BotEditor() -> Element {
    let mut open = use_signal(|| false);
    let edit = EDIT_BOT();
    use_effect(move || open.set(EDIT_BOT().is_some()));
    // Schliessen über Esc, X oder daneben
    use_effect(move || {
        if !open() && EDIT_BOT.peek().is_some() {
            *EDIT_BOT.write() = None;
        }
    });
    let Some((bot, tab)) = edit else { return rsx! {} };
    let title = if bot.id == 0 { tr("Neuer Bot") } else { bot.name.clone() };
    rsx! {
        Modal { open, title, class: "max-w-xl",
            for key in std::iter::once(format!("{}-{:?}", bot.id, tab)) {
                EditorBody { key: "{key}", bot: bot.clone(), tab }
            }
        }
    }
}

#[component]
fn EditorBody(bot: Bot, tab: EditTab) -> Element {
    let nav = use_navigator();
    let agents = crate::pages::agents::use_agents().unwrap_or_default();
    let is_new = bot.id == 0;
    let mut tab = use_signal(|| tab);
    let mut name = use_signal(|| bot.name.clone());
    let mut emoji = use_signal(|| bot.emoji.clone());
    let mut role = use_signal(|| bot.role.clone());
    let mut instructions = use_signal(|| bot.instructions.clone());
    let mut memory = use_signal(|| bot.memory.clone());
    // Agent: beim neuen Bot die zuletzt im Composer gewählte Auswahl
    let first = if is_new {
        let s = crate::pages::agents::data::Selection::remembered().normalized(&agents);
        (s.agent, s.account_id, s.model, s.effort)
    } else {
        (bot.agent, bot.account_id, bot.model.clone(), bot.effort.clone())
    };
    let mut agent = use_signal(|| first.0);
    let mut account = use_signal(|| first.1);
    let mut model = use_signal(|| first.2.clone());
    let mut effort = use_signal(|| first.3.clone());
    let mut saving = use_signal(|| false);
    let info = agents.iter().find(|a| a.kind == agent()).cloned();
    let with_accounts: Vec<(AgentKind, String)> = agents.iter().filter(|a| !a.accounts.is_empty() || a.kind == agent()).map(|a| (a.kind, a.name.clone())).collect();
    let bot_id = bot.id;
    let routines = bot.routines.clone();

    let save = move |_| {
        if saving() {
            return;
        }
        if name().trim().is_empty() {
            toast_error(tr("Bitte einen Namen eingeben."));
            return;
        }
        saving.set(true);
        let body = BotInput {
            name: Some(name()),
            emoji: Some(emoji()),
            role: Some(role()),
            instructions: Some(instructions()),
            memory: Some(memory()),
            agent: Some(agent()),
            account_id: Some(account()),
            model: Some(model()),
            effort: Some(effort()),
            mcp: None,
        };
        spawn(async move {
            let r = if is_new { api::post::<Bot, _>("/bots", &body).await } else { api::patch::<Bot, _>(&format!("/bots/{bot_id}"), &body).await };
            saving.set(false);
            match r {
                Ok(b) => {
                    let id = b.id;
                    state::put_bot(b);
                    *EDIT_BOT.write() = None;
                    if is_new {
                        nav.push(Route::BotPage { id });
                    }
                }
                Err(e) => toast_error(e),
            }
        });
    };
    let tab_btn = |on: bool| {
        if on {
            "inline-flex items-center justify-center gap-1.5 rounded-md bg-white/[0.12] px-2 py-1.5 text-[13px] font-medium text-foreground [&_svg]:size-3.5"
        } else {
            "inline-flex items-center justify-center gap-1.5 rounded-md px-2 py-1.5 text-[13px] text-muted-foreground hover:text-foreground [&_svg]:size-3.5"
        }
    };
    rsx! {
        if !is_new {
            div { class: "mb-5 grid grid-cols-4 gap-1 rounded-lg bg-white/[0.06] p-1",
                button { class: tab_btn(tab() == EditTab::Profile), onclick: move |_| tab.set(EditTab::Profile), icons::Smile {} {tr("Profil")} }
                button { class: tab_btn(tab() == EditTab::Memory), onclick: move |_| tab.set(EditTab::Memory), icons::Brain {} {tr("Gedächtnis")} }
                button { class: tab_btn(tab() == EditTab::Routines), onclick: move |_| tab.set(EditTab::Routines), icons::CalendarClock {} {tr("Routinen")} }
                button { class: tab_btn(tab() == EditTab::Apps), onclick: move |_| tab.set(EditTab::Apps), icons::Blocks {} {tr("Apps")} }
            }
        }
        if tab() == EditTab::Routines {
            Routines { bot_id, routines }
        } else if tab() == EditTab::Apps {
            Apps { bot_id, list: bot.mcp.clone() }
        } else {
            div { class: "grid gap-4",
                if tab() == EditTab::Profile {
                    div { class: "flex items-end gap-3",
                        BotAvatar { name: name(), emoji: emoji(), size: 48 }
                        Field { label: tr("Name"), class: "flex-1",
                            input { class: INPUT, maxlength: "60", placeholder: tr("z. B. Lena"), value: "{name}", oninput: move |e| name.set(e.value()) }
                        }
                    }
                    div { class: "flex flex-wrap items-center gap-1",
                        // Pixel-Katzen in verschiedenen Farben, daneben Emojis
                        for h in HUES {
                            button {
                                key: "px{h}",
                                class: if emoji() == format!("px:{h}") { "grid size-9 place-items-center rounded-md bg-white/[0.12] ring-1 ring-white/30" } else { "grid size-9 place-items-center rounded-md hover:bg-white/[0.08]" },
                                onclick: move |_| emoji.set(format!("px:{h}")),
                                BotAvatar { name: String::new(), emoji: format!("px:{h}"), size: 26 }
                            }
                        }
                        span { class: "mx-1 h-6 w-px bg-white/10" }
                        for e in EMOJIS {
                            button {
                                key: "{e}",
                                class: if emoji() == e { "grid size-8 place-items-center rounded-md bg-accent text-lg ring-1 ring-ring" } else { "grid size-8 place-items-center rounded-md text-lg hover:bg-accent" },
                                onclick: move |_| emoji.set(e.to_string()),
                                "{e}"
                            }
                        }
                        input { class: "{INPUT} h-8 w-14 text-center", maxlength: "8", title: tr("Eigenes Emoji"), value: if emoji().starts_with("px:") { String::new() } else { emoji() }, placeholder: "🙂", oninput: move |e| emoji.set(e.value()) }
                    }
                    Field { label: tr("Experte für"), hint: tr("Eine Sache, die er richtig gut kann – steht unter dem Namen."),
                        input { class: INPUT, maxlength: "80", placeholder: tr("z. B. Zugreisen, Recherche, Buchhaltung"), value: "{role}", oninput: move |e| role.set(e.value()) }
                    }
                    Field { label: tr("Anweisungen"), hint: tr("Persönlichkeit, Ton und Aufgaben – gilt in jedem Gespräch."),
                        textarea { class: "{TEXTAREA} min-h-28", placeholder: tr("Wie soll sich der Bot verhalten?"), value: "{instructions}", oninput: move |e| instructions.set(e.value()) }
                    }
                    div { class: "grid gap-3 sm:grid-cols-2",
                        Field { label: tr("Agent"),
                            select {
                                class: SELECT,
                                onchange: {
                                    let agents = agents.clone();
                                    move |e: FormEvent| {
                                        if let Some(k) = AgentKind::parse(&e.value()) {
                                            let s = crate::pages::agents::data::Selection { agent: agent(), account_id: account(), model: model(), effort: effort() }.with_agent(k, &agents);
                                            agent.set(s.agent);
                                            account.set(s.account_id);
                                            model.set(s.model);
                                            effort.set(s.effort);
                                        }
                                    }
                                },
                                for (k, n) in with_accounts.clone() {
                                    option { key: "{k.key()}", value: "{k.key()}", selected: k == agent(), "{n}" }
                                }
                            }
                        }
                        Field { label: tr("Konto"),
                            select {
                                class: SELECT,
                                onchange: move |e: FormEvent| account.set(e.value().parse::<i64>().ok()),
                                for a in info.as_ref().map(|i| i.accounts.clone()).unwrap_or_default() {
                                    option { key: "{a.id}", value: "{a.id}", selected: Some(a.id) == account(), "{a.label}" }
                                }
                            }
                        }
                        Field { label: tr("Modell"),
                            select {
                                class: SELECT,
                                onchange: move |e: FormEvent| model.set(e.value()),
                                option { value: "", selected: model().is_empty(), {tr("Standard")} }
                                for m in info.as_ref().map(|i| i.models.clone()).unwrap_or_default() {
                                    option { key: "{m.id}", value: "{m.id}", selected: m.id == model(), "{m.label}" }
                                }
                                if !model().is_empty() && !info.as_ref().is_some_and(|i| i.models.iter().any(|m| m.id == model())) {
                                    option { value: "{model}", selected: true, "{model}" }
                                }
                            }
                        }
                        if info.as_ref().is_some_and(|i| !i.efforts.is_empty()) {
                            Field { label: tr("Denkstufe"),
                                select {
                                    class: SELECT,
                                    onchange: move |e: FormEvent| effort.set(e.value()),
                                    option { value: "", selected: effort().is_empty(), {tr("Standard")} }
                                    for x in info.as_ref().map(|i| i.efforts.clone()).unwrap_or_default() {
                                        option { key: "{x}", value: "{x}", selected: x == effort(), {crate::pages::agents::data::effort_label(&x)} }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    p { class: "text-sm text-muted-foreground",
                        {tr("Das merkt sich der Bot über alle Gespräche hinweg – nur er. Sag ihm einfach „Merk dir …“; er ergänzt es auch selbst, wenn er etwas Wichtiges lernt. Hier kannst du es lesen und ändern.")}
                    }
                    textarea { class: "{TEXTAREA} min-h-64 font-mono text-[13px]", placeholder: tr("- Ich heisse …\n- Ich mag kurze Antworten"), value: "{memory}", oninput: move |e| memory.set(e.value()) }
                }
                div { class: "flex justify-end gap-2 pt-1",
                    button { class: "{BTN} {BTN_GHOST}", onclick: move |_| *EDIT_BOT.write() = None, {tr("Abbrechen")} }
                    button { class: "{BTN} {BTN_PRIMARY}", disabled: saving(), onclick: save,
                        if is_new { icons::Sparkles {} } else { icons::Check {} }
                        if is_new { {tr("Bot erstellen")} } else { {tr("Speichern")} }
                    }
                }
            }
        }
    }
}

fn time_label(min: i64) -> String {
    format!("{:02}:{:02}", min / 60, min % 60)
}

fn days_label(days: i64) -> String {
    match days {
        127 => tr("Täglich"),
        31 => tr("Werktags"),
        96 => tr("Am Wochenende"),
        _ => DAYS.iter().enumerate().filter(|(i, _)| days & (1 << i) != 0).map(|(_, d)| tr(d)).collect::<Vec<_>>().join(", "),
    }
}

/// Routinen eines Bots: Liste und Formular (neu oder ändern).
#[component]
fn Routines(bot_id: i64, routines: Vec<BotRoutine>) -> Element {
    let mut list = use_signal(|| routines.clone());
    let mut editing = use_signal(|| None::<i64>);
    let mut title = use_signal(String::new);
    let mut prompt = use_signal(String::new);
    let mut time = use_signal(|| "08:00".to_string());
    let mut days = use_signal(|| 31i64);
    let mut saving = use_signal(|| false);
    let tz = -crate::pages::agents::tz_offset();

    let mut reset = move || {
        editing.set(None);
        title.set(String::new());
        prompt.set(String::new());
        time.set("08:00".into());
        days.set(31);
    };
    let mut apply = move |b: Bot| {
        list.set(b.routines.clone());
        state::put_bot(b.clone());
        if let Some((cur, tab)) = EDIT_BOT.peek().clone() {
            if cur.id == b.id {
                *EDIT_BOT.write() = Some((b, tab));
            }
        }
    };
    let send = move |r: BotRoutineInput, id: Option<i64>| {
        spawn(async move {
            saving.set(true);
            let res = match id {
                Some(rid) => api::patch::<Bot, _>(&format!("/bots/{bot_id}/routines/{rid}"), &r).await,
                None => api::post::<Bot, _>(&format!("/bots/{bot_id}/routines"), &r).await,
            };
            saving.set(false);
            match res {
                Ok(b) => {
                    apply(b);
                    reset();
                }
                Err(e) => toast_error(e),
            }
        });
    };
    let submit = move |_| {
        let (h, m) = time().split_once(':').map(|(h, m)| (h.parse::<i64>().unwrap_or(8), m.parse::<i64>().unwrap_or(0))).unwrap_or((8, 0));
        let r = BotRoutineInput { title: title(), prompt: prompt(), time_min: (h * 60 + m).clamp(0, 1439), days: days(), tz_offset: tz, enabled: true };
        send(r, editing());
    };
    rsx! {
        div { class: "grid gap-4",
            p { class: "text-sm text-muted-foreground",
                {tr("Der Bot bekommt den Auftrag zur gewählten Zeit in sein aktuelles Gespräch – z. B. jeden Morgen die wichtigsten News.")}
            }
            if !list().is_empty() {
                div { class: "grid gap-2",
                    for r in list() {
                        div { key: "{r.id}", class: "flex items-start gap-3 rounded-lg border p-3",
                            Toggle {
                                checked: r.enabled,
                                onchange: {
                                    let r = r.clone();
                                    move |v: bool| {
                                        let body = BotRoutineInput { title: r.title.clone(), prompt: r.prompt.clone(), time_min: r.time_min, days: r.days, tz_offset: r.tz_offset, enabled: v };
                                        send(body, Some(r.id));
                                    }
                                },
                            }
                            button {
                                class: "min-w-0 flex-1 text-left",
                                onclick: {
                                    let r = r.clone();
                                    move |_| {
                                        editing.set(Some(r.id));
                                        title.set(r.title.clone());
                                        prompt.set(r.prompt.clone());
                                        time.set(time_label(r.time_min));
                                        days.set(r.days);
                                    }
                                },
                                div { class: "flex items-center gap-2 text-sm",
                                    span { class: "font-medium tabular-nums", {time_label(r.time_min)} }
                                    span { class: "text-muted-foreground", {days_label(r.days)} }
                                    if !r.title.is_empty() {
                                        span { class: "truncate", "· {r.title}" }
                                    }
                                }
                                p { class: "mt-0.5 line-clamp-2 text-xs text-muted-foreground", "{r.prompt}" }
                            }
                            button {
                                class: "grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-destructive [&_svg]:size-4",
                                title: tr("Routine löschen"),
                                onclick: move |_| {
                                    let rid = r.id;
                                    spawn(async move {
                                        match api::delete_json::<Bot>(&format!("/bots/{bot_id}/routines/{rid}")).await {
                                            Ok(b) => apply(b),
                                            Err(e) => toast_error(e),
                                        }
                                    });
                                },
                                icons::Trash2 {}
                            }
                        }
                    }
                }
            }
            div { class: "grid gap-3 rounded-xl border border-dashed p-4",
                p { class: "text-sm font-medium", if editing().is_some() { {tr("Routine ändern")} } else { {tr("Neue Routine")} } }
                div { class: "flex flex-wrap items-center gap-2",
                    input { class: "{INPUT} w-28", r#type: "time", value: "{time}", oninput: move |e| time.set(e.value()) }
                    div { class: "flex gap-1",
                        for (i, d) in DAYS.iter().enumerate() {
                            button {
                                key: "{i}",
                                class: if days() & (1 << i) != 0 { "h-8 w-9 rounded-md bg-primary text-xs font-medium text-primary-foreground" } else { "h-8 w-9 rounded-md border text-xs text-muted-foreground hover:bg-accent" },
                                onclick: move |_| days.set(days() ^ (1 << i)),
                                {tr(d)}
                            }
                        }
                    }
                }
                input { class: INPUT, maxlength: "80", placeholder: tr("Titel (optional), z. B. Morgen-Briefing"), value: "{title}", oninput: move |e| title.set(e.value()) }
                textarea { class: "{TEXTAREA} min-h-20", placeholder: tr("Auftrag, z. B. Fasse die wichtigsten Tech-News von heute in 5 Punkten zusammen."), value: "{prompt}", oninput: move |e| prompt.set(e.value()) }
                div { class: "flex justify-end gap-2",
                    if editing().is_some() {
                        button { class: "{BTN} {BTN_GHOST} {BTN_SM}", onclick: move |_| reset(), {tr("Abbrechen")} }
                    }
                    button { class: "{BTN} {BTN_PRIMARY} {BTN_SM}", disabled: saving() || prompt().trim().is_empty() || days() == 0, onclick: submit,
                        if editing().is_some() { icons::Check {} } else { icons::Plus {} }
                        if editing().is_some() { {tr("Speichern")} } else { {tr("Routine hinzufügen")} }
                    }
                }
            }
        }
    }
}

/// Eigene MCP-Server eines Bots: nur er nutzt sie (z. B. Notion, Kalender, ein lokaler Dienst).
#[component]
fn Apps(bot_id: i64, list: Vec<BotMcp>) -> Element {
    let mut items = use_signal(|| list.clone());
    let mut saving = use_signal(|| false);
    let save = move |_| {
        if saving() {
            return;
        }
        saving.set(true);
        let body = BotInput { mcp: Some(items()), ..Default::default() };
        spawn(async move {
            match api::patch::<Bot, _>(&format!("/bots/{bot_id}"), &body).await {
                Ok(b) => {
                    items.set(b.mcp.clone());
                    state::put_bot(b.clone());
                    if let Some((cur, tab)) = EDIT_BOT.peek().clone() {
                        if cur.id == b.id {
                            *EDIT_BOT.write() = Some((b, tab));
                        }
                    }
                }
                Err(e) => toast_error(e),
            }
            saving.set(false);
        });
    };
    let mut set = move |i: usize, f: &dyn Fn(&mut BotMcp)| {
        if let Some(m) = items.write().get_mut(i) {
            f(m);
        }
    };
    rsx! {
        div { class: "grid gap-4",
            p { class: "text-sm text-muted-foreground",
                {tr("MCP-Server, die nur dieser Bot nutzt – über eine Adresse (mit Token) oder als Befehl auf diesem Mac. Andere Bots und deine Aufgaben sehen sie nicht.")}
            }
            for (i , m) in items().into_iter().enumerate() {
                div { key: "{i}", class: "grid gap-2 rounded-xl border border-white/10 p-3",
                    div { class: "flex items-center gap-2",
                        Toggle { checked: m.enabled, onchange: move |v: bool| set(i, &|x| x.enabled = v) }
                        input { class: "{INPUT} h-8 flex-1", maxlength: "40", placeholder: tr("Name, z. B. notion"), value: "{m.name}", oninput: move |e| { let v = e.value(); set(i, &|x| x.name = v.clone()) } }
                        button {
                            class: "grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-white/[0.08] hover:text-destructive [&_svg]:size-4",
                            title: tr("Entfernen"),
                            onclick: move |_| {
                                items.write().remove(i);
                            },
                            icons::Trash2 {}
                        }
                    }
                    input { class: "{INPUT} h-8 font-mono text-[12.5px]", placeholder: tr("Adresse, z. B. https://mcp.notion.com/mcp"), value: "{m.url}", oninput: move |e| { let v = e.value(); set(i, &|x| x.url = v.clone()) } }
                    if m.url.trim().is_empty() {
                        input { class: "{INPUT} h-8 font-mono text-[12.5px]", placeholder: tr("… oder Befehl, z. B. npx -y @modelcontextprotocol/server-memory"), value: "{m.command}", oninput: move |e| { let v = e.value(); set(i, &|x| x.command = v.clone()) } }
                    } else {
                        input { class: "{INPUT} h-8 font-mono text-[12.5px]", r#type: "password", placeholder: tr("Token (optional)"), value: "{m.token}", oninput: move |e| { let v = e.value(); set(i, &|x| x.token = v.clone()) } }
                    }
                }
            }
            button {
                class: "inline-flex items-center justify-center gap-1.5 rounded-xl border border-dashed border-white/15 py-2.5 text-sm text-muted-foreground hover:bg-white/[0.05] hover:text-foreground [&_svg]:size-4",
                onclick: move |_| items.write().push(BotMcp { enabled: true, ..Default::default() }),
                icons::Plus {}
                {tr("MCP-Server hinzufügen")}
            }
            div { class: "flex justify-end gap-2 pt-1",
                button { class: "{BTN} {BTN_GHOST}", onclick: move |_| *EDIT_BOT.write() = None, {tr("Schliessen")} }
                button { class: "{BTN} {BTN_PRIMARY}", disabled: saving(), onclick: save,
                    icons::Check {}
                    {tr("Speichern")}
                }
            }
        }
    }
}
