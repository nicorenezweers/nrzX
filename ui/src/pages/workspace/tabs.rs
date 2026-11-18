//! Reihe oben im Fenster (wie in der Skizze): neben der Ampel der Knopf der Seitenleiste, rechts
//! bündig über der Aufgabenleiste „Neue Aufgabe“, über dem Chat-Paneel die Reiter des offenen
//! Threads – die Chats seiner Reihe, ein neuer Chat, Browser und Terminals (siehe `views`).
//! Reiter gibt es erst ab dem zweiten; sonst reicht das Paneel bis ganz nach oben und „+“ (neuer
//! Chat) steht oben rechts im Chat. Der gewählte Reiter hängt an der Oberkante (runde Übergänge).
//! Eingeklappt steht links eine schmale Leiste (Ampel, Knopf der Seitenleiste, neue Aufgabe,
//! Nutzung). Freie Fläche zieht das Fenster (`data-drag`). Die Suche öffnet ⌘K.

use super::parts::*;
use super::views::View;
use crate::components::overlay::toast_error;
use crate::i18n::{tr, trf};
use crate::state::THREADS;
use crate::util;
use crate::Route;
use dioxus::prelude::*;

/// Suche in der Aufgabenleiste offen (⌘K).
pub static SEARCH_OPEN: GlobalSignal<bool> = Signal::global(|| false);

/// Hat der Thread Reiter? Erst, wenn neben dem Chat eine weitere Ansicht offen ist – sonst
/// reicht das Paneel bis ganz nach oben.
pub fn has_tabs(id: i64) -> bool {
    super::views::list(id).len() > 1
}

/// Hat die Seite Reiter? Ein Thread ab der zweiten Ansicht, ein Bot ab dem zweiten offenen Gespräch.
pub fn route_has_tabs(route: &Route) -> bool {
    match route {
        Route::ThreadPage { id } => has_tabs(*id),
        Route::BotPage { id } => super::bots::has_tabs(*id),
        _ => false,
    }
}

/// Gibt es über dem Chat eine Reihe? Mit Reitern (Thread oder Bot) – und eingeklappt immer (dann
/// trägt sie neben der Ampel den Knopf der Seitenleiste und „Neue Aufgabe“).
pub fn has_row(route: &Route) -> bool {
    !LEFT() || route_has_tabs(route)
}

/// Klassen eines Paneels unter Reitern: der gewählte Reiter an seiner linken oberen Ecke (die dann
/// eckig ist) bzw. weiter rechts mit runden Übergängen auf beiden Seiten (`--nrx-ai`, `--nrx-x0`);
/// ohne Reiter keine.
fn attach(tabs: bool, ai: usize) -> &'static str {
    match (LEFT(), tabs) {
        // eingeklappt ohne Reiter: die Fläche reicht neben den Knöpfen bis oben (Reiter bis rechts)
        (false, false) => "md:rounded-tr-none nrx-notch-big",
        (false, true) => "nrx-notch-at",
        (true, false) => "",
        (true, true) if ai == 0 => "md:rounded-tl-none nrx-notch-tl",
        (true, true) => "nrx-notch-at",
    }
}

/// Klassen des Thread-Paneels (siehe `attach`).
pub fn panel_attach(id: i64) -> &'static str {
    attach(has_tabs(id), super::views::active_index(id))
}

/// Klassen des Paneels eines Bots mit seinen Gesprächen als Reiter (siehe `attach`).
pub fn bot_panel_attach(id: i64) -> &'static str {
    attach(super::bots::has_tabs(id), super::bots::active_index(id))
}

/// CSS-Variablen für die Aussparung im Umriss: Platz des gewählten Reiters und wo die Reiter
/// beginnen (eingeklappt nach Ampel, Knopf der Seitenleiste und „Neue Aufgabe“).
pub fn row_vars(route: &Route) -> String {
    let ai = match route {
        Route::ThreadPage { id } => super::views::active_index(*id),
        Route::BotPage { id } => super::bots::active_index(*id),
        _ => 0,
    };
    if LEFT() {
        format!("--nrx-ai: {ai}; --nrx-x0: 0px")
    } else {
        // nach Ampel, Seitenleiste, „Neue Aufgabe“, „Bots“ und „Deep Research“
        format!("--nrx-ai: {ai}; --nrx-x0: calc(var(--nrx-lead) + 144px)")
    }
}

/// Knopf neben der Ampel: Aufgabenleiste ein- bzw. ausklappen (steht in beiden Fällen gleich).
#[component]
pub fn SidebarToggle() -> Element {
    rsx! {
        button {
            class: "grid size-7 shrink-0 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.09] hover:text-foreground [&_svg]:size-[17px]",
            title: if LEFT() { tr("Seitenleiste einklappen") } else { tr("Seitenleiste zeigen") },
            onclick: move |_| toggle(&LEFT, "nrz-left"),
            icons::PanelLeft {}
        }
    }
}

/// Rechts neben dem Knopf der Seitenleiste, wenn sie eingeklappt ist: neue Aufgabe (Stift, damit es
/// nicht mit dem „+“ der Reiter verwechselt wird).
#[component]
pub fn NewTaskButton() -> Element {
    let nav = use_navigator();
    rsx! {
        button {
            class: "grid size-7 shrink-0 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.09] hover:text-foreground [&_svg]:size-[17px]",
            title: tr("Neue Aufgabe"),
            onclick: move |_| {
                nav.push(Route::Home {});
            },
            icons::SquarePen {}
        }
    }
}

/// Bot-Modus (Übersicht der Bots bzw. Gespräch mit einem Bot)?
pub fn bot_mode(route: &Route) -> bool {
    matches!(route, Route::Bots {} | Route::BotPage { .. })
}

/// Knopf „Bots“ neben dem Knopf der Seitenleiste: in den Bot-Modus bzw. zurück zu den Aufgaben.
#[component]
pub fn BotButton() -> Element {
    let route = use_route::<Route>();
    let nav = use_navigator();
    let on = bot_mode(&route);
    rsx! {
        button {
            class: if on { "grid size-7 shrink-0 place-items-center rounded-lg bg-white/[0.12] text-foreground [&_svg]:size-[17px]" } else { "grid size-7 shrink-0 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.09] hover:text-foreground [&_svg]:size-[17px]" },
            title: if on { tr("Zurück zu den Aufgaben") } else { tr("Bots") },
            onclick: move |_| {
                if bot_mode(&use_route_peek()) {
                    nav.push(Route::Home {});
                } else {
                    nav.push(super::bots::bot_route());
                }
            },
            icons::Bot {}
        }
    }
}

/// Deep-Research-Modus (Startseite der Recherche bzw. eine Recherche)?
pub fn research_mode(route: &Route) -> bool {
    matches!(route, Route::Research {} | Route::ResearchPage { .. })
}

/// Knopf „Deep Research“ neben „Bots“: in den Recherche-Modus bzw. zurück zu den Aufgaben.
#[component]
pub fn ResearchButton() -> Element {
    let route = use_route::<Route>();
    let nav = use_navigator();
    let on = research_mode(&route);
    rsx! {
        button {
            class: if on { "grid size-7 shrink-0 place-items-center rounded-lg bg-white/[0.12] text-foreground [&_svg]:size-[17px]" } else { "grid size-7 shrink-0 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.09] hover:text-foreground [&_svg]:size-[17px]" },
            title: if on { tr("Zurück zu den Aufgaben") } else { tr("Deep Research") },
            onclick: move |_| {
                if research_mode(&use_route_peek()) {
                    nav.push(Route::Home {});
                } else {
                    nav.push(super::research::research_route());
                }
            },
            icons::Telescope {}
        }
    }
}

/// Aktuelle Seite ohne Abo (in Klick-Handlern).
fn use_route_peek() -> Route {
    dioxus::router::router().current::<Route>()
}

/// Reiter „Neue Aufgabe“ (im Bot-Modus „Neuer Bot“, bei Deep Research „Neue Recherche“): hängt
/// rechts bündig an der Seitenleiste.
#[component]
pub fn TaskTab() -> Element {
    let route = use_route::<Route>();
    let nav = use_navigator();
    let research = research_mode(&route);
    let home = matches!(route, Route::Home {} | Route::Research {});
    let bots = bot_mode(&route);
    rsx! {
        div { class: "nrx-tab nrx-tab-t nrx-fl px-1.5",
            span { class: "nrx-line" }
            button {
                class: if home { "flex h-8 flex-1 items-center gap-2 rounded-lg px-2.5 text-[14px] font-medium text-foreground" } else { "flex h-8 flex-1 items-center gap-2 rounded-lg px-2.5 text-[14px] text-muted-foreground transition-colors hover:text-foreground" },
                onclick: move |_| {
                    *MOBILE_NAV.write() = false;
                    if bots {
                        super::bots::new_bot(None);
                    } else if research {
                        nav.push(Route::Research {});
                    } else {
                        nav.push(Route::Home {});
                    }
                },
                icons::Plus { class: "size-4" }
                if bots { {tr("Neuer Bot")} } else if research { {tr("Neue Recherche")} } else { {tr("Neue Aufgabe")} }
            }
        }
    }
}

/// Name eines Chats: allein „Chat“, mit weiteren in der Reihe der Titel seines Threads.
fn chat_label(id: i64, t: i64) -> String {
    if super::views::chats(id).len() < 2 {
        return tr("Chat");
    }
    THREADS.read().get(&t).map(|t| t.title.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| tr("Chat"))
}

/// Zeichen und Name einer Ansicht.
fn view_label(id: i64, v: &View) -> (Element, String) {
    match v {
        View::Chat => (rsx! { icons::MessageSquare {} }, chat_label(id, id)),
        View::Other(t) => (rsx! { icons::MessageSquare {} }, chat_label(id, *t)),
        View::NewChat => (rsx! { icons::MessageSquarePlus {} }, tr("Neuer Chat")),
        View::Cli => {
            let agent = THREADS.read().get(&id).map(|t| t.agent.name().to_string()).unwrap_or_default();
            (rsx! { icons::CodeXml {} }, if agent.is_empty() { tr("CLI") } else { format!("{agent} CLI") })
        }
        View::Browser => (rsx! { icons::Globe {} }, tr("Browser")),
        View::Term(t) => {
            let terms = super::views::TERMINALS.read().get(&id).cloned().unwrap_or_default();
            let n = terms.iter().position(|x| x == t).map(|i| i + 1).unwrap_or(1);
            (rsx! { icons::SquareTerminal {} }, if terms.len() < 2 { tr("Terminal") } else { format!("{} {n}", tr("Terminal")) })
        }
    }
}

/// Ein Reiter: der gewählte hängt am Paneel (an dessen linker Kante nur mit `edge`), die übrigen
/// stehen frei und zeigen ihr X erst beim Darüberfahren. `spin`: Kreis statt Zeichen (arbeitet).
#[allow(clippy::too_many_arguments)]
fn tab_item(key: String, chosen: bool, edge: bool, icon: Element, label: String, hint: String, spin: bool, closable: bool, activate: Callback<()>, close: Callback<()>) -> Element {
    let icon = if spin {
        rsx! { span { class: "size-3.5 shrink-0 rounded-full border-[1.5px] border-foreground/70 border-t-transparent animate-spin" } }
    } else {
        icon
    };
    let close_btn = move |extra: &'static str| {
        rsx! {
            button {
                class: "grid size-6 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-white/10 hover:text-foreground {extra}",
                title: tr("Reiter schliessen"),
                onclick: move |e: MouseEvent| {
                    e.stop_propagation();
                    close.call(());
                },
                icons::X { class: "size-3.5" }
            }
        }
    };
    if chosen {
        rsx! {
            div {
                key: "{key}",
                title: "{hint}",
                class: if edge { "nrx-tab nrx-tab-a nrx-fr gap-2 pr-1.5 pl-3.5 text-[13.5px] font-medium text-foreground [&>svg]:size-4 [&>svg]:shrink-0" } else { "nrx-tab nrx-tab-a nrx-fl nrx-fr gap-2 pr-1.5 pl-3.5 text-[13.5px] font-medium text-foreground [&>svg]:size-4 [&>svg]:shrink-0" },
                span { class: "nrx-line" }
                {icon}
                span { class: "min-w-0 flex-1 truncate", "{label}" }
                if closable {
                    {close_btn("")}
                }
            }
        }
    } else {
        rsx! {
            div { key: "{key}", class: "nrx-tab-i group/tab",
                button {
                    class: "flex h-[30px] min-w-0 flex-1 items-center gap-2 rounded-lg pr-1 pl-2.5 text-left text-[13.5px] text-muted-foreground transition-colors hover:bg-white/[0.07] hover:text-foreground [&>svg]:size-4 [&>svg]:shrink-0",
                    title: "{hint}",
                    onclick: move |_| activate.call(()),
                    {icon}
                    span { class: "min-w-0 flex-1 truncate", "{label}" }
                    if closable {
                        {close_btn("opacity-0 group-hover/tab:opacity-100")}
                    }
                }
            }
        }
    }
}

/// Reiter oben über dem Paneel: beim Thread seine Ansichten, beim Bot seine offenen Gespräche; der
/// gewählte hängt am Paneel, dahinter „+“ (neuer Chat bzw. neues Gespräch).
#[component]
pub fn ThreadTabs() -> Element {
    let route = use_route::<Route>();
    let drag = rsx! { div { class: "min-w-6 flex-1 self-stretch", "data-drag": "1" } };
    // Nur der Chat bzw. ein Gespräch: keine Reiter (das „+“ steht dann oben im Paneel)
    if !route_has_tabs(&route) {
        return drag;
    }
    match route {
        Route::ThreadPage { id } => rsx! { ViewTabs { id } },
        Route::BotPage { id } => rsx! { BotTabs { bot: id } },
        _ => drag,
    }
}

/// Reiter eines Threads: Chats seiner Reihe, neuer Chat, Browser, Terminals.
#[component]
fn ViewTabs(id: i64) -> Element {
    let views = super::views::list(id);
    let ai = super::views::active_index(id);
    // An der linken Kante des Paneels nur mit Aufgabenleiste (eingeklappt stehen Knöpfe davor)
    let edge = ai == 0 && LEFT();
    // Arbeitet der Thread dieses Chats gerade? (Kreis statt Zeichen)
    let working = |t: i64| THREADS.read().get(&t).is_some_and(|t| t.status.is_active());
    rsx! {
        for (i , v) in views.into_iter().enumerate() {
            {
                let (icon, label) = view_label(id, &v);
                let spin = match v {
                    View::Chat => working(id),
                    View::Other(t) => working(t),
                    _ => false,
                };
                let closable = super::views::closable(id, &v);
                let (va, vc) = (v.clone(), v.clone());
                tab_item(
                    format!("{v:?}"),
                    i == ai,
                    edge,
                    icon,
                    label.clone(),
                    label,
                    spin,
                    closable,
                    Callback::new(move |_| super::views::activate(id, va.clone())),
                    Callback::new(move |_| super::views::close(id, vc.clone())),
                )
            }
        }
        // Neuer Chat
        div { class: "mb-1 ml-2 flex shrink-0 items-center", NewChatButton { thread_id: id, top: false } }
        div { class: "min-w-6 flex-1 self-stretch", "data-drag": "1" }
    }
}

/// Reiter eines Bots: seine offenen Gespräche (Beginn als Uhrzeit), das aktuelle hängt am Paneel.
#[component]
fn BotTabs(bot: i64) -> Element {
    let list = super::bots::conversations(bot);
    let ai = super::bots::active_index(bot);
    let edge = ai == 0 && LEFT();
    let closable = list.len() > 1;
    rsx! {
        for (i , tid) in list.into_iter().enumerate() {
            {
                let t = THREADS.read().get(&tid).cloned();
                let label = t.as_ref().map(|t| util::clock(t.created_at)).unwrap_or_else(|| tr("Gespräch"));
                let hint = t.as_ref().map(|t| trf("Gespräch vom {0}", &[&util::datetime(t.created_at)])).unwrap_or_default();
                let spin = t.as_ref().is_some_and(|t| t.status.is_active());
                tab_item(
                    format!("{tid}"),
                    i == ai,
                    edge,
                    rsx! { icons::MessageSquare {} },
                    label,
                    hint,
                    spin,
                    closable,
                    Callback::new(move |_| super::bots::open_conversation(bot, tid)),
                    Callback::new(move |_| super::bots::close_conversation(bot, tid)),
                )
            }
        }
        // Neues Gespräch
        div { class: "mb-1 ml-2 flex shrink-0 items-center", NewConversationButton { bot, top: false } }
        div { class: "min-w-6 flex-1 self-stretch", "data-drag": "1" }
    }
}

/// „+“ beim Bot: neues Gespräch als weiterer Reiter (das bisherige bleibt offen, das Gedächtnis
/// sowieso).
#[component]
pub fn NewConversationButton(bot: i64, top: bool) -> Element {
    let mut busy = use_signal(|| false);
    rsx! {
        button {
            class: if top { "grid size-8 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.09] hover:text-foreground" } else { "grid size-7 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.09] hover:text-foreground" },
            title: tr("Neues Gespräch – das Gedächtnis bleibt"),
            disabled: busy(),
            onclick: move |_| {
                if *busy.peek() {
                    return;
                }
                busy.set(true);
                spawn(async move {
                    super::bots::new_conversation(bot).await;
                    busy.set(false);
                });
            },
            if busy() {
                icons::LoaderCircle { class: "size-4 animate-spin" }
            } else {
                icons::Plus { class: "size-4" }
            }
        }
    }
}

/// „+“: öffnet einen neuen Chat als Reiter (Eingabefeld für einen neuen Thread im selben Projekt).
/// In der Reihe der Reiter bzw. (ohne Reiter) oben rechts im Chat.
#[component]
pub fn NewChatButton(thread_id: i64, top: bool) -> Element {
    rsx! {
        button {
            class: if top { "grid size-8 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.09] hover:text-foreground" } else { "grid size-7 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.09] hover:text-foreground" },
            title: tr("Neuer Chat"),
            onclick: move |_| super::views::new_chat(thread_id),
            icons::Plus { class: "size-4" }
        }
    }
}

/// Oben im Chat neben dem CLI-Knopf: neues Terminal im Arbeitsordner als weiterer Reiter.
#[component]
pub fn TerminalButton(thread_id: i64) -> Element {
    let mut opening = use_signal(|| false);
    rsx! {
        button {
            class: "grid size-8 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.09] hover:text-foreground",
            title: tr("Neues Terminal"),
            onclick: move |_| {
                if *opening.peek() {
                    return;
                }
                opening.set(true);
                spawn(async move {
                    if let Err(e) = super::views::new_terminal(thread_id).await {
                        toast_error(e);
                    }
                    opening.set(false);
                });
            },
            if opening() {
                icons::LoaderCircle { class: "size-4 animate-spin" }
            } else {
                icons::SquareTerminal { class: "size-4" }
            }
        }
    }
}
