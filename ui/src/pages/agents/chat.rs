//! Unterhaltung eines Threads: Benutzernachrichten rechts als graue Blase, Antworten der
//! Agenten als Markdown, die Arbeitsschritte eines Laufs („Arbeitete 20s“) mit Code-Blöcken –
//! Bearbeitungen rot/grün mit Zeilennummern, Befehle mit Ausgabe, Lesen/Suchen in einer Zeile –,
//! Subagenten-Karte, Statuszeile („Working 25s · Stop“) und unten der Composer.

use super::composer::Composer;
use super::data::{duration, running_for, subagents_text, use_agents, ComposerDraft, COMPOSER_DRAFT};
use super::icons::{AgentIcon, AgentStack};
use crate::api;
use crate::components::common::*;
use crate::components::overlay::{confirm, toast_error, Ask};
use crate::components::widgets::Markdown;
use crate::i18n::{tr, trf};
use crate::state::{self, MESSAGES, THREADS};
use crate::Route;
use dioxus::prelude::*;
use nrzx_shared::agents::{AnswerQuestion, EditDiff, QuestionData, RetractedMessage, SubagentsData, SystemData, ThreadDetail, ThreadPatch, ToolCall, UserMessageData};
use crate::util;
use nrzx_shared::diff::{line_ops, Op};
use nrzx_shared::{Message, ThreadStatus, ThreadSummary};

/// Unterhaltung eines Threads mit Composer unten (füllt die Mitte der Thread-Seite).
#[component]
pub fn ChatView(thread_id: i64) -> Element {
    rsx! {
        // In einer Schleife, damit der Schlüssel greift: neuer Thread → neue Komponente (frische
        // Hooks, Nachrichten werden geladen); ein einzelner Knoten behielte seine Hooks
        for id in std::iter::once(thread_id) {
            ChatBody { key: "{id}", thread_id: id }
        }
    }
}

/// Nachrichten zusammenführen (gleiche ID: die neuere gewinnt).
fn merge(into: &mut Vec<Message>, list: Vec<Message>) {
    for m in list {
        match into.iter_mut().find(|x| x.id == m.id) {
            Some(x) if x.updated_at <= m.updated_at => *x = m,
            Some(_) => {}
            None => into.push(m),
        }
    }
    into.sort_by_key(|m| m.id);
}

/// Verlauf eines Threads laden und mit den Live-Nachrichten zusammenführen. Auch im
/// Hintergrund für Threads, die gerade nicht offen sind (Wechsel ohne Warten).
pub async fn load_messages(thread_id: i64) -> Result<(), api::ApiError> {
    // Leere Liste vorab: Live-Nachrichten während des Ladens gehen nicht verloren
    MESSAGES.write().entry(thread_id).or_default();
    let list = api::get::<Vec<Message>>(&format!("/threads/{thread_id}/messages")).await?;
    {
        let mut all = MESSAGES.write();
        merge(all.entry(thread_id).or_default(), list);
    }
    state::LOADED.write().insert(thread_id);
    Ok(())
}

/// Verläufe mehrerer Threads im Hintergrund laden (noch nicht geladene zuerst). Läuft im
/// Wurzel-Scope: Aufrufe kommen auch aus den Live-Ereignissen (ausserhalb jeder Komponente –
/// `spawn` brach dort ab, etwa bei der ersten Nachricht eines eben angelegten Threads).
pub fn prefetch_messages(ids: Vec<i64>) {
    dioxus::core::spawn_forever(async move {
        for id in ids {
            if state::LOADED.peek().contains(&id) {
                continue;
            }
            let _ = load_messages(id).await;
        }
    });
}

/// Thread mit Eltern und Subagenten laden und in den globalen Zustand legen.
pub async fn load_thread(id: i64) -> Result<ThreadDetail, api::ApiError> {
    let d = api::get::<ThreadDetail>(&format!("/threads/{id}")).await?;
    state::upsert_thread(d.thread.clone());
    for c in d.children.iter().cloned() {
        state::upsert_thread(c);
    }
    if let Some(p) = d.parent.clone() {
        state::upsert_thread(p);
    }
    Ok(d)
}

/// Neuer Thread, solange der Server ihn anlegt: sieht aus wie der Chat (gesendete Nachricht,
/// „Startet …“, Eingabe unten), damit der Wechsel in den echten Chat nicht springt.
#[component]
pub fn PendingChat(pending: state::PendingThread) -> Element {
    let m = Message {
        id: 0,
        thread_id: 0,
        run_id: None,
        role: "user".into(),
        content: pending.text.clone(),
        html: pending.text.lines().map(|l| format!("<p>{}</p>", util::escape_html(l))).collect(),
        data: serde_json::to_value(UserMessageData { attachments: pending.attachments.clone(), ..Default::default() }).unwrap_or_default(),
        created_at: pending.created_at,
        updated_at: pending.created_at,
    };
    rsx! {
        div { class: "flex h-full min-h-0 flex-col",
            div { class: "min-h-0 flex-1 overflow-y-auto",
                // Ohne Zeiger: Kopieren/Zurückziehen gibt es erst im echten Thread
                div { class: "pointer-events-none mx-auto flex w-full max-w-3xl flex-col gap-5 px-4 pt-14 pb-10",
                    UserBubble { m, last: true, active: true }
                }
            }
            div { class: "relative mx-auto w-full max-w-3xl shrink-0 px-4 pb-4",
                div { class: "flex items-center gap-2 text-[13px] text-muted-foreground",
                    icons::LoaderCircle { class: "size-4 animate-spin" }
                    span { class: "text-foreground/80", {tr("Startet …")} }
                }
                div { class: "rounded-2xl border bg-card shadow-sm",
                    div { class: "px-4 pt-3.5 pb-2 text-[15px] leading-relaxed text-muted-foreground", {tr("Nachricht für danach … Enter: Warteschlange · ⌘/Strg+Enter: zwischenrufen")} }
                    div { class: "h-11" }
                }
            }
        }
    }
}

#[component]
fn ChatBody(thread_id: i64) -> Element {
    let _agents = use_agents();
    // Kein Sekundentakt hier: sonst rendert der ganze Chat jede Sekunde neu (bei langen Chats
    // spürbar – der Ladekreis ruckelte). Nur Statuszeile, laufende Subagenten und der
    // Rate-Limit-Countdown lesen den Takt selbst ([`tick`]).
    let mut error = use_signal(|| None::<String>);
    let mut loaded = use_signal(|| state::LOADED.peek().contains(&thread_id));
    let mut at_bottom = use_signal(|| true);
    let mut touch_y = use_signal(|| 0.0f64);
    let mut marking = use_signal(|| false);

    use_hook(move || {
        MESSAGES.write().entry(thread_id).or_default();
        spawn(async move {
            if let Err(e) = load_messages(thread_id).await {
                error.set(Some(e.message));
            }
            loaded.set(true);
            if let Err(e) = load_thread(thread_id).await {
                error.set(Some(e.message));
            }
            // Subagenten gleich mitladen – der Wechsel dorthin zeigt sofort den Verlauf
            let kids: Vec<i64> = THREADS.peek().values().filter(|t| t.parent_id == Some(thread_id)).map(|t| t.id).collect();
            prefetch_messages(kids);
        });
    });

    // Fertig und angezeigt → gelesen (nur der gezeigte Thread – die übrigen sind nur eingehängt)
    use_effect(move || {
        let t = THREADS.read().get(&thread_id).cloned();
        let shown = state::SHOWN_THREAD() == Some(thread_id);
        if let Some(t) = t {
            if shown && t.unread && !t.status.is_active() && !marking() {
                marking.set(true);
                spawn(async move {
                    if let Ok(s) = api::action::<ThreadSummary>(&format!("/threads/{thread_id}/read")).await {
                        state::upsert_thread(s);
                    }
                    marking.set(false);
                });
            }
        }
    });

    // Nach unten scrollen, solange man unten ist – nur bei neuen Nachrichten dieses Threads (die
    // Nachrichten anderer Threads lösen über die Kennung nichts aus)
    let scroll_id = format!("chat-{thread_id}");
    let sid = scroll_id.clone();
    let mark = use_memo(move || MESSAGES.read().get(&thread_id).map(|l| (l.len(), l.last().map(|m| (m.updated_at, m.content.len())))));
    use_effect(move || {
        let _ = mark();
        let _ = loaded();
        if !*at_bottom.peek() {
            return;
        }
        let sid = sid.clone();
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&sid)) {
                el.set_scroll_top(el.scroll_height());
            }
        });
    });
    // Zu diesem Chat gewechselt: ganz nach unten zur neuesten Nachricht und wieder mitlaufen – der
    // Thread blieb verborgen eingehängt, seine Stelle von vorhin zählt nicht
    let sid = scroll_id.clone();
    use_effect(move || {
        if state::SHOWN_THREAD() != Some(thread_id) {
            return;
        }
        at_bottom.set(true);
        let sid = sid.clone();
        spawn(async move {
            // erst nach dem Einblenden – verborgen hat die Liste keine Höhe
            gloo_timers::future::TimeoutFuture::new(0).await;
            if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&sid)) {
                el.set_scroll_top(el.scroll_height());
            }
        });
    });

    let thread = THREADS.read().get(&thread_id).cloned();
    let msgs = MESSAGES.read().get(&thread_id).cloned().unwrap_or_default();
    let blocks = group(&msgs);
    // Letzte Arbeitsgruppe – solange der Agent arbeitet, steht sie offen („3 Schritte bisher“)
    let last_work = blocks.iter().rposition(|b| matches!(b, Block::Work(_)));
    let active = thread.as_ref().is_some_and(|t| t.status.is_active());
    let last_user = msgs.iter().rfind(|m| m.role == "user").map(|m| m.id);
    // Nur die letzte Rate-Limit-Meldung zeigt den Countdown (solange ein Weitermachen geplant ist)
    let last_limit = msgs
        .iter()
        .rfind(|m| m.role == "system" && m.data.get("kind").and_then(|k| k.as_str()) == Some("rate_limit"))
        .filter(|_| thread.as_ref().is_some_and(|t| t.resume_at.is_some() && !t.status.is_active()))
        .map(|m| m.id);

    rsx! {
        div { class: "flex h-full min-h-0 flex-col",
            div {
                id: "{scroll_id}",
                class: "min-h-0 flex-1 overflow-y-auto",
                // Stelle bleibt beim Wachsen des Eingabefelds (`composer::grow`)
                "data-chat-scroll": "1",
                // Mitlaufen: nur ganz unten wieder an, weit oben aus; dazwischen bleibt es, wie es
                // ist – sonst zieht jede neue Zeile eines Agenten beim Hochscrollen zurück nach unten
                onscroll: move |e| {
                    let gap = e.scroll_height() as f64 - (e.scroll_top() + e.client_height() as f64);
                    if gap <= 4.0 && !at_bottom() {
                        at_bottom.set(true);
                    } else if gap > 120.0 && at_bottom() {
                        at_bottom.set(false);
                    }
                },
                // Rad bzw. Trackpad nach oben: sofort nicht mehr mitlaufen
                onwheel: move |e: WheelEvent| {
                    if e.delta().strip_units().y < 0.0 && *at_bottom.peek() {
                        at_bottom.set(false);
                    }
                },
                // Touch: Finger nach unten gezogen (= Inhalt nach oben) ebenso
                ontouchstart: move |e: TouchEvent| {
                    if let Some(t) = e.touches().first() {
                        touch_y.set(t.client_coordinates().y);
                    }
                },
                ontouchmove: move |e: TouchEvent| {
                    if let Some(t) = e.touches().first() {
                        if t.client_coordinates().y > *touch_y.peek() + 6.0 && *at_bottom.peek() {
                            at_bottom.set(false);
                        }
                    }
                },
                // Oben deutlich Luft bis zur ersten Nachricht
                div { class: "mx-auto flex w-full max-w-3xl flex-col gap-6 px-6 pt-14 pb-10",
                    if let Some(e) = error() {
                        Callout { text: e, error: true }
                    }
                    if !loaded() && msgs.is_empty() {
                        Loading {}
                    }
                    if let Some(t) = thread.as_ref() {
                        if let Some(pid) = t.parent_id {
                            ParentNote { parent_id: pid }
                        }
                        if t.read_only {
                            div { class: "flex w-fit items-center gap-2 rounded-lg border border-info/30 bg-info/10 px-3 py-1.5 text-xs text-info",
                                icons::Eye { class: "size-3.5" }
                                {tr("Nur lesen – der Agent beantwortet Fragen zur Codebasis und ändert nichts.")}
                            }
                        }
                    }
                    for (bi , b) in blocks.into_iter().enumerate() {
                        match b {
                            Block::User(m) => rsx! { UserBubble { key: "{m.id}", last: last_user == Some(m.id), active, m } },
                            Block::Assistant(m) => rsx! { AssistantText { key: "{m.id}", m } },
                            Block::Work(steps) => rsx! { WorkGroup { key: "w{steps[0].id}", steps, live: active && Some(bi) == last_work, thread_id } },
                            Block::Subagents(m) => rsx! { SubagentCard { key: "{m.id}", m } },
                            Block::System(m) => {
                                let latest = last_limit == Some(m.id);
                                rsx! { SystemLine { key: "{m.id}", m, latest } }
                            }
                            Block::Question(m) => rsx! { QuestionCard { key: "{m.id}", m } },
                            Block::Error(m) => rsx! { ErrorBox { key: "{m.id}", m } },
                        }
                    }
                    if loaded() && msgs.is_empty() && !active {
                        p { class: "py-10 text-center text-sm text-muted-foreground", {tr("Noch keine Nachrichten.")} }
                    }
                }
            }
            div { class: "relative mx-auto w-full max-w-3xl shrink-0 px-4 pb-4",
                // Hochgescrollt: ein Klick springt ans Ende (und folgt dann wieder)
                if !at_bottom() && !msgs.is_empty() {
                    button {
                        class: "absolute -top-12 left-1/2 z-10 flex h-9 -translate-x-1/2 items-center gap-1.5 rounded-xl border bg-card/80 px-3.5 text-[14px] font-medium shadow-md backdrop-blur transition-colors hover:bg-accent",
                        title: tr("Nach unten"),
                        onclick: {
                            let sid = scroll_id.clone();
                            move |_| {
                                if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&sid)) {
                                    let o = web_sys::ScrollToOptions::new();
                                    o.set_top(el.scroll_height() as f64);
                                    o.set_behavior(web_sys::ScrollBehavior::Smooth);
                                    el.scroll_to_with_scroll_to_options(&o);
                                }
                                at_bottom.set(true);
                            }
                        },
                        icons::ArrowDown { class: "size-4" }
                        {tr("Neueste")}
                    }
                }
                if let Some(t) = thread.clone() {
                    TodoBar { thread_id, active }
                    Composer { project_id: thread.as_ref().and_then(|t| t.project_id), thread_id: Some(thread_id), status: rsx! { StatusLine { thread: t } } }
                }
            }
        }
    }
}

/// Hinweis oben in einem Subagent-Thread.
#[component]
fn ParentNote(parent_id: i64) -> Element {
    let parent = THREADS.read().get(&parent_id).cloned();
    let title = parent.map(|p| p.title).unwrap_or_else(|| format!("#{parent_id}"));
    rsx! {
        Link {
            to: Route::ThreadPage { id: parent_id },
            class: "flex w-fit items-center gap-2 rounded-lg border bg-card px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground",
            icons::CornerLeftUp { class: "size-3.5" }
            {trf("Subagent von „{0}“", &[&title])}
        }
    }
}

enum Block {
    User(Message),
    Assistant(Message),
    Work(Vec<Message>),
    Subagents(Message),
    System(Message),
    Error(Message),
    Question(Message),
}

/// Werkzeug-Nachrichten eines Laufs zu Gruppen zusammenfassen.
fn group(msgs: &[Message]) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    for m in msgs {
        match m.role.as_str() {
            "tool" => {
                if let Some(Block::Work(steps)) = out.last_mut() {
                    if steps.last().map(|s| s.run_id) == Some(m.run_id) {
                        steps.push(m.clone());
                        continue;
                    }
                }
                out.push(Block::Work(vec![m.clone()]));
            }
            "user" => out.push(Block::User(m.clone())),
            "assistant" => out.push(Block::Assistant(m.clone())),
            "subagents" => out.push(Block::Subagents(m.clone())),
            "error" => out.push(Block::Error(m.clone())),
            "question" => out.push(Block::Question(m.clone())),
            _ => out.push(Block::System(m.clone())),
        }
    }
    out
}

/// Benutzernachricht als graue Blase; beim Überfahren: kopieren (Text und Bilder) und
/// zurückziehen (Nachricht samt allem danach entfernen, Text und Bilder kommen in den Composer).
#[component]
fn UserBubble(m: Message, last: bool, active: bool) -> Element {
    let data: UserMessageData = serde_json::from_value(m.data.clone()).unwrap_or_default();
    let thread_id = m.thread_id;
    let mid = m.id;
    let copy = {
        let text = m.content.clone();
        let atts = data.attachments.clone();
        move |_| {
            let mut html = String::new();
            for a in atts.iter().filter(|a| a.content_type.starts_with("image/")) {
                html.push_str(&format!("<p><img src=\"{}\" alt=\"{}\"></p>", util::escape_html(&util::absolute(&a.url)), util::escape_html(&a.name)));
            }
            for line in text.lines() {
                html.push_str(&format!("<p>{}</p>", util::escape_html(line)));
            }
            let mut plain = text.clone();
            for a in atts.iter() {
                plain.push_str(&format!("\n{}", util::absolute(&a.url)));
            }
            let custom = serde_json::json!({"text": text, "attachments": atts}).to_string();
            if !util::copy_rich(&plain, &html, Some(&custom)) {
                util::copy(&plain);
            }
        }
    };
    let retract = move |_| {
        spawn(async move {
            if !last || active {
                let desc = if active {
                    tr("Der Agent wird gestoppt. Die Nachricht und alles danach verschwinden aus dem Verlauf; Text und Bilder kommen zum Bearbeiten in den Composer. Änderungen an Dateien bleiben.")
                } else {
                    tr("Die Nachricht und alles danach verschwinden aus dem Verlauf; Text und Bilder kommen zum Bearbeiten in den Composer. Änderungen an Dateien bleiben.")
                };
                if !confirm(Ask::new(tr("Nachricht zurückziehen?"), desc).confirm(tr("Zurückziehen"))).await {
                    return;
                }
            }
            match api::action::<RetractedMessage>(&format!("/threads/{thread_id}/messages/{mid}/retract")).await {
                Ok(r) => {
                    if let Some(list) = MESSAGES.write().get_mut(&thread_id) {
                        list.retain(|x| x.id < mid);
                    }
                    state::upsert_thread(r.thread.clone());
                    *COMPOSER_DRAFT.write() = Some(ComposerDraft { thread_id: Some(thread_id), text: r.text, attachments: r.attachments });
                }
                Err(e) => toast_error(e),
            }
        });
    };
    let edit_copy = {
        let text = m.content.clone();
        let atts = data.attachments.clone();
        move |_| {
            *COMPOSER_DRAFT.write() = Some(ComposerDraft { thread_id: Some(thread_id), text: text.clone(), attachments: atts.clone() });
        }
    };
    const ACT: &str = "grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground [&_svg]:size-3.5";
    rsx! {
        div { class: "group flex flex-col items-end gap-2",
            if data.interjection {
                div { class: "-mb-1 flex items-center gap-1 text-[11px] text-muted-foreground/80",
                    icons::Zap { class: "size-3" }
                    {tr("Zwischenruf")}
                }
            }
            if data.auto_resume {
                div { class: "flex items-center gap-1.5 text-xs text-muted-foreground",
                    icons::Hourglass { class: "size-3.5" }
                    {tr("Nach dem Rate-Limit automatisch fortgesetzt")}
                }
            } else {
                if !data.attachments.is_empty() {
                    div { class: "flex max-w-[80%] flex-wrap justify-end gap-2",
                        for a in data.attachments.clone() {
                            if a.content_type.starts_with("image/") {
                                // In der App groß zeigen (Popup), nicht im Browser oder Bildprogramm
                                button {
                                    key: "{a.uuid}",
                                    class: "cursor-zoom-in overflow-hidden rounded-xl",
                                    title: tr("Vergrößern"),
                                    onclick: {
                                        let all: Vec<(String, String)> = data.attachments.iter().filter(|x| x.content_type.starts_with("image/")).map(|x| (x.url.clone(), x.name.clone())).collect();
                                        let idx = all.iter().position(|(u, _)| *u == a.url).unwrap_or(0);
                                        move |_| crate::components::overlay::view_images(all.clone(), idx)
                                    },
                                    img { class: "max-h-40 rounded-xl border object-cover", src: "{a.url}", alt: "{a.name}" }
                                }
                            } else {
                                a { key: "{a.uuid}", href: "{a.url}", target: "_blank", class: "flex items-center gap-1.5 rounded-lg border bg-card px-2.5 py-1.5 text-xs",
                                    icons::Paperclip { class: "size-3.5" }
                                    "{a.name}"
                                }
                            }
                        }
                    }
                }
                if !m.content.trim().is_empty() {
                    div { class: "max-w-[80%] rounded-2xl border border-white/5 bg-foreground/[0.07] px-5 py-3.5 text-[16px] leading-relaxed",
                        Markdown { html: m.html.clone(), class: "[&_p]:my-1" }
                    }
                }
                div { class: "-mt-1 flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100",
                    button { class: ACT, title: tr("Kopieren (Text und Bilder)"), onclick: copy, icons::Copy {} }
                    button { class: ACT, title: tr("Als neue Nachricht in den Composer übernehmen"), onclick: edit_copy, icons::Pencil {} }
                    button { class: ACT, title: tr("Zurückziehen und bearbeiten"), onclick: retract, icons::Undo2 {} }
                }
            }
        }
    }
}

#[component]
fn AssistantText(m: Message) -> Element {
    rsx! {
        div { class: "text-[16px] leading-relaxed",
            if m.html.is_empty() {
                p { class: "whitespace-pre-wrap", "{m.content}" }
            } else {
                Markdown { html: m.html.clone(), class: "text-[16px]" }
            }
        }
    }
}

#[component]
fn SystemLine(m: Message, latest: bool) -> Element {
    let data: SystemData = serde_json::from_value(m.data.clone()).unwrap_or_default();
    let mut open = use_signal(|| false);
    if data.kind == "rate_limit" {
        if latest {
            return rsx! { RateLimitCard { thread_id: m.thread_id, data } };
        }
        let text = match data.resets_at {
            Some(at) => trf("Nutzungslimit erreicht · zurückgesetzt um {0}", &[&util::clock(at)]),
            None => tr("Nutzungslimit erreicht"),
        };
        return rsx! {
            div { class: "flex items-center gap-3 text-xs text-muted-foreground",
                div { class: "h-px flex-1 bg-border" }
                span { class: "flex max-w-[80%] items-center gap-1.5", title: "{data.detail}",
                    icons::Hourglass { class: "size-3.5" }
                    "{text}"
                }
                div { class: "h-px flex-1 bg-border" }
            }
        };
    }
    if data.kind == "chain_switch" {
        // KI-Kette: Inhalt = Name der Kette, detail = neues Konto („Grok · Abo 1“)
        let text = trf("Limit erreicht – KI-Kette „{0}“ macht mit {1} weiter", &[&m.content, &data.detail]);
        return rsx! {
            div { class: "flex items-center gap-3 text-xs text-muted-foreground",
                div { class: "h-px flex-1 bg-border" }
                span { class: "flex max-w-[80%] items-center gap-1.5 text-center",
                    icons::Workflow { class: "size-3.5 shrink-0" }
                    "{text}"
                }
                div { class: "h-px flex-1 bg-border" }
            }
        };
    }
    if data.kind == "raw" {
        let first = m.content.lines().next().unwrap_or("").to_string();
        let lines = m.content.lines().count();
        return rsx! {
            div { class: "rounded-lg border bg-card/50 text-xs",
                button { class: "flex w-full items-center gap-2 px-3 py-1.5 text-left text-muted-foreground hover:text-foreground",
                    onclick: move |_| open.toggle(),
                    icons::ScrollText { class: "size-3.5 shrink-0" }
                    span { class: "truncate font-mono", "{first}" }
                    if lines > 1 {
                        span { class: "ml-auto shrink-0", {trf("{0} Zeilen", &[&lines])} }
                    }
                }
                if open() {
                    pre { class: "max-h-80 overflow-auto border-t px-3 py-2 font-mono text-[12px] whitespace-pre-wrap", "{m.content}" }
                }
            }
        };
    }
    if data.kind == "mcp" {
        // Klein und unaufdringlich: Namen nur beim Überfahren
        let n = m.content.split(',').filter(|s| !s.trim().is_empty()).count();
        let text = if n == 1 { tr("1 MCP-Server nicht verbunden") } else { trf("{0} MCP-Server nicht verbunden", &[&n]) };
        return rsx! {
            div { class: "flex justify-center",
                span { class: "flex items-center gap-1 text-[11px] text-muted-foreground/60", title: "{m.content}",
                    icons::Unplug { class: "size-3" }
                    "{text}"
                }
            }
        };
    }
    let text = tr(&m.content);
    rsx! {
        div { class: "flex items-center gap-3 text-xs text-muted-foreground",
            div { class: "h-px flex-1 bg-border" }
            span { class: "max-w-[80%] text-center", "{text}" }
            div { class: "h-px flex-1 bg-border" }
        }
    }
}

#[component]
fn ErrorBox(m: Message) -> Element {
    rsx! {
        div { class: "flex items-start gap-2.5 rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm",
            icons::CircleAlert { class: "mt-0.5 size-4 shrink-0 text-destructive" }
            pre { class: "min-w-0 flex-1 font-sans whitespace-pre-wrap break-words", {tr(&m.content)} }
        }
    }
}

// ------------------------------------------------------------------ Werkzeuge

/// Icon je Art des Werkzeugs.
fn step_icon(kind: &str) -> Element {
    match kind {
        "read" => rsx! { icons::FileText { class: "size-3.5" } },
        "edit" => rsx! { icons::Pencil { class: "size-3.5" } },
        "write" => rsx! { icons::FilePlus { class: "size-3.5" } },
        "command" => rsx! { icons::SquareTerminal { class: "size-3.5" } },
        "search" => rsx! { icons::Search { class: "size-3.5" } },
        "web" => rsx! { icons::Globe { class: "size-3.5" } },
        "mcp" => rsx! { icons::Plug { class: "size-3.5" } },
        "task" => rsx! { icons::Bot { class: "size-3.5" } },
        "todo" => rsx! { icons::ListTodo { class: "size-3.5" } },
        "think" => rsx! { icons::Brain { class: "size-3.5" } },
        _ => rsx! { icons::Wrench { class: "size-3.5" } },
    }
}

fn step_verb(kind: &str) -> String {
    match kind {
        "read" => tr("Lesen"),
        "edit" => tr("Bearbeiten"),
        "write" => tr("Schreiben"),
        "command" => tr("Befehl"),
        "search" => tr("Suche"),
        "web" => tr("Web"),
        "mcp" => "MCP".into(),
        "task" => tr("Aufgabe"),
        "todo" => tr("Aufgabenliste"),
        "think" => tr("Gedanke"),
        _ => String::new(),
    }
}

/// Teil einer Arbeitsgruppe, wie er im Chat steht.
enum Seg {
    /// Lesen, Suchen, Nachdenken, Werkzeuge – zusammengefasst in einer Zeile
    Explore(Vec<Message>),
    /// Bearbeitung als Code-Block
    Edit(Message),
    /// Befehle nacheinander – eine Zeile „3 Befehle ausgeführt“, aufklappbar
    Commands(Vec<Message>),
}

fn segments(steps: &[Message]) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    for m in steps {
        let kind = m.data["kind"].as_str().unwrap_or("");
        let has_diffs = m.data["diffs"].as_array().is_some_and(|d| !d.is_empty());
        let running = m.data["status"] == "running";
        match kind {
            "edit" | "write" if has_diffs || running => out.push(Seg::Edit(m.clone())),
            "command" => match out.last_mut() {
                Some(Seg::Commands(list)) => list.push(m.clone()),
                _ => out.push(Seg::Commands(vec![m.clone()])),
            },
            _ => match out.last_mut() {
                Some(Seg::Explore(list)) => list.push(m.clone()),
                _ => out.push(Seg::Explore(vec![m.clone()])),
            },
        }
    }
    out
}

/// Arbeitsschritte zwischen zwei Antworten, eingeklappt hinter einer unterstrichenen Zeile:
/// „3 Schritte bisher“, solange der Agent daran arbeitet (dann offen), danach „Hat 12 s
/// gearbeitet“. Aufgeklappt ruhige Zeilen („>_ 3 Befehle ausgeführt“, „2 Dateien gelesen“) und
/// Bearbeitungen als Code-Block. Darunter Chips für erstellte Seiten (`.html`) – ein Klick zeigt
/// sie im integrierten Browser.
#[component]
fn WorkGroup(steps: Vec<Message>, live: bool, thread_id: i64) -> Element {
    let mut open = use_signal(|| None::<bool>);
    let shown = open().unwrap_or(live);
    let first = steps.first().map(|m| m.created_at).unwrap_or(0);
    let last = steps.iter().map(|m| m.updated_at.max(m.created_at)).max().unwrap_or(first);
    let n = steps.len();
    let label = if live {
        if n == 1 { tr("1 Schritt bisher") } else { trf("{0} Schritte bisher", &[&n]) }
    } else {
        trf("Hat {0} gearbeitet", &[&duration((last - first).max(1))])
    };
    // Erstellte bzw. geschriebene Seiten
    let mut pages: Vec<(String, usize)> = Vec::new();
    for m in &steps {
        let call: ToolCall = serde_json::from_value(m.data.clone()).unwrap_or_default();
        if !matches!(call.kind.as_str(), "write" | "edit") || call.status == "error" {
            continue;
        }
        let path = if call.target.is_empty() { call.diffs.first().map(|d| d.path.clone()).unwrap_or_default() } else { call.target.clone() };
        let lower = path.to_lowercase();
        if !(lower.ends_with(".html") || lower.ends_with(".htm")) {
            continue;
        }
        let size: usize = call.diffs.iter().map(|d| d.after.len()).sum();
        match pages.iter_mut().find(|(p, _)| *p == path) {
            Some((_, s)) => *s = (*s).max(size),
            None => pages.push((path, size)),
        }
    }
    rsx! {
        div { class: "flex flex-col gap-2",
            button {
                class: "w-fit text-[14px] text-muted-foreground underline decoration-muted-foreground/50 underline-offset-4 transition-colors hover:text-foreground",
                onclick: move |_| open.set(Some(!shown)),
                "{label}"
            }
            if shown {
                div { class: "flex flex-col gap-1.5",
                    for seg in segments(&steps) {
                        match seg {
                            Seg::Explore(list) => rsx! {
                                ExploreLine { key: "{list[0].id}", steps: list }
                            },
                            Seg::Edit(m) => rsx! {
                                EditCard { key: "{m.id}", m }
                            },
                            Seg::Commands(list) => rsx! {
                                CommandsLine { key: "{list[0].id}", steps: list }
                            },
                        }
                    }
                }
            }
            if !pages.is_empty() {
                div { class: "flex flex-wrap gap-2",
                    for (path , size) in pages {
                        button {
                            key: "{path}",
                            class: "inline-flex items-center gap-2 rounded-lg border bg-card/50 px-3 py-1.5 font-mono text-[13px] transition-colors hover:bg-accent/50",
                            title: tr("Im integrierten Browser zeigen"),
                            onclick: {
                                let path = path.clone();
                                move |_| crate::pages::workspace::browser::open(thread_id, crate::pages::workspace::browser::Target::File(path.clone()))
                            },
                            span { {path.rsplit('/').next().unwrap_or(&path).to_string()} }
                            if size > 0 {
                                span { class: "text-[11.5px] text-muted-foreground", {kb(size)} }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// „6 KB“, „812 B“
fn kb(bytes: usize) -> String {
    if bytes >= 1024 { format!("{} KB", bytes.div_ceil(1024)) } else { format!("{bytes} B") }
}

/// Kopf einer aufklappbaren Zeile: Zeichen, Text, Zusatz; der Pfeil erscheint beim Überfahren.
#[component]
fn FoldHead(icon: Element, label: String, open: bool, onclick: EventHandler<()>, children: Element) -> Element {
    rsx! {
        button {
            class: "group/fold flex w-fit max-w-full min-w-0 items-center gap-2 rounded-md py-0.5 text-left text-[13px] text-muted-foreground transition-colors hover:text-foreground",
            onclick: move |_| onclick.call(()),
            span { class: "grid size-4 shrink-0 place-items-center text-muted-foreground/80 group-hover/fold:text-foreground", {icon} }
            span { class: "shrink-0", "{label}" }
            {children}
            icons::ChevronRight {
                class: if open { "size-3.5 shrink-0 rotate-90 transition-transform" } else { "size-3.5 shrink-0 opacity-0 transition group-hover/fold:opacity-100" },
            }
        }
    }
}

/// „>_ 3 Befehle ausgeführt“ – aufgeklappt jeder Befehl mit seiner Ausgabe. Läuft gerade einer,
/// steht er dahinter.
#[component]
fn CommandsLine(steps: Vec<Message>) -> Element {
    let mut open = use_signal(|| false);
    let calls: Vec<ToolCall> = steps.iter().map(|m| serde_json::from_value(m.data.clone()).unwrap_or_default()).collect();
    let n = calls.len();
    let current = calls.iter().rev().find(|c| c.status == "running").map(|c| c.target.clone());
    let failed = calls.iter().filter(|c| c.status == "error").count();
    let label = match (&current, n) {
        (Some(_), 1) => tr("Führt einen Befehl aus"),
        (Some(_), k) => trf("Führt Befehle aus ({0})", &[&k]),
        (None, 1) => tr("Einen Befehl ausgeführt"),
        (None, k) => trf("{0} Befehle ausgeführt", &[&k]),
    };
    rsx! {
        div { class: "flex flex-col",
            FoldHead {
                icon: rsx! { icons::Terminal { class: "size-3.5" } },
                label,
                open: open(),
                onclick: move |_| open.toggle(),
                if failed > 0 {
                    span { class: "shrink-0 text-xs text-destructive",
                        if failed == 1 { {tr("· 1 fehlgeschlagen")} } else { {trf("· {0} fehlgeschlagen", &[&failed])} }
                    }
                }
                if let Some(c) = current {
                    icons::LoaderCircle { class: "size-3.5 shrink-0 animate-spin" }
                    span { class: "min-w-0 truncate font-mono text-[12px] text-foreground/70", "{c}" }
                }
            }
            if open() {
                div { class: "mt-1.5 mb-1 ml-2 flex flex-col gap-1.5 border-l pl-3.5",
                    for m in steps {
                        CommandCard { key: "{m.id}", m }
                    }
                }
            }
        }
    }
}

/// „2 Dateien gelesen · 1 Suche“ bzw. „Nachgedacht“ – aufklappbar zu den einzelnen Schritten;
/// läuft gerade einer, steht er dahinter.
#[component]
fn ExploreLine(steps: Vec<Message>) -> Element {
    let mut open = use_signal(|| false);
    let calls: Vec<ToolCall> = steps.iter().map(|m| serde_json::from_value(m.data.clone()).unwrap_or_default()).collect();
    let n = |kinds: &[&str]| calls.iter().filter(|c| kinds.contains(&c.kind.as_str())).count();
    let mut parts: Vec<String> = Vec::new();
    match n(&["read"]) {
        0 => {}
        1 => parts.push(tr("1 Datei gelesen")),
        k => parts.push(trf("{0} Dateien gelesen", &[&k])),
    }
    match n(&["search"]) {
        0 => {}
        1 => parts.push(tr("1 Suche")),
        k => parts.push(trf("{0} Suchen", &[&k])),
    }
    match n(&["web"]) {
        0 => {}
        1 => parts.push(tr("1 Web-Abfrage")),
        k => parts.push(trf("{0} Web-Abfragen", &[&k])),
    }
    match n(&["edit", "write", "mcp", "task", "todo", "other"]) {
        0 => {}
        1 => parts.push(tr("1 Werkzeug")),
        k => parts.push(trf("{0} Werkzeuge", &[&k])),
    }
    let thought = n(&["think"]) > 0;
    let only_thought = thought && parts.is_empty();
    if thought && !only_thought {
        parts.push(tr("nachgedacht"));
    }
    let current = calls.iter().rev().find(|c| c.status == "running").map(|c| {
        let verb = step_verb(&c.kind);
        let target = if c.target.is_empty() { c.name.clone() } else { c.target.clone() };
        if verb.is_empty() { target } else { format!("{verb} {target}") }
    });
    let errors = calls.iter().filter(|c| c.status == "error").count();
    let label = if only_thought {
        if current.is_some() { tr("Denkt nach") } else { tr("Nachgedacht") }
    } else if parts.is_empty() {
        tr("Erkundet")
    } else {
        parts.join(" · ")
    };
    let icon = if only_thought {
        rsx! { icons::Brain { class: "size-3.5" } }
    } else if n(&["read"]) > 0 && n(&["search", "web"]) == 0 {
        rsx! { icons::FileText { class: "size-3.5" } }
    } else {
        rsx! { icons::Search { class: "size-3.5" } }
    };
    let current = current.filter(|_| !only_thought);
    rsx! {
        div { class: "flex flex-col",
            FoldHead {
                icon,
                label,
                open: open(),
                onclick: move |_| open.toggle(),
                if errors > 0 {
                    span { class: "shrink-0 text-xs text-destructive", "· {errors} ✕" }
                }
                if let Some(c) = current {
                    icons::LoaderCircle { class: "size-3.5 shrink-0 animate-spin" }
                    span { class: "min-w-0 truncate font-mono text-[12px] text-foreground/70", "{c}" }
                }
            }
            if open() {
                div { class: "mt-1 mb-1 ml-2 flex flex-col gap-0.5 border-l pl-2.5",
                    for m in steps {
                        Step { key: "{m.id}", m }
                    }
                }
            }
        }
    }
}

/// Zeile eines Code-Blocks: Zeilennummer (falls bekannt), Zeichen (`+`, `-`, ` `, `…`) und Text.
#[derive(Clone, PartialEq)]
struct CodeRow {
    no: Option<i64>,
    sign: char,
    text: String,
}

/// Zeilen einer Bearbeitung: Kontext davor, die Änderungen (gleiche Zeilen dazwischen als
/// Kontext), Kontext danach. Ältere Nachrichten haben keine Zeilennummern.
fn code_rows(d: &EditDiff) -> Vec<CodeRow> {
    let mut rows = Vec::new();
    if d.before.is_empty() && d.after.is_empty() {
        if d.truncated {
            rows.push(CodeRow { no: None, sign: '…', text: trf("weitere Änderungen (+{0} −{1})", &[&d.adds, &d.dels]) });
        }
        return rows;
    }
    let known = d.line > 0;
    let new0 = d.line;
    let old0 = if d.old_line > 0 { d.old_line } else { d.line };
    let ctx = d.context_before.len() as i64;
    for (k, l) in d.context_before.iter().enumerate() {
        rows.push(CodeRow { no: known.then_some(new0 - ctx + k as i64), sign: ' ', text: l.clone() });
    }
    let a: Vec<&str> = d.before.lines().collect();
    let b: Vec<&str> = d.after.lines().collect();
    let (mut on, mut nn) = (old0, new0);
    for op in line_ops(&a, &b) {
        match op {
            Op::Same(_, j) => {
                rows.push(CodeRow { no: known.then_some(nn), sign: ' ', text: b[j].to_string() });
                on += 1;
                nn += 1;
            }
            Op::Del(i) => {
                rows.push(CodeRow { no: known.then_some(on), sign: '-', text: a[i].to_string() });
                on += 1;
            }
            Op::Add(j) => {
                rows.push(CodeRow { no: known.then_some(nn), sign: '+', text: b[j].to_string() });
                nn += 1;
            }
        }
    }
    if d.truncated {
        rows.push(CodeRow { no: None, sign: '…', text: tr("gekürzt") });
    } else {
        for (k, l) in d.context_after.iter().enumerate() {
            rows.push(CodeRow { no: known.then_some(nn + k as i64), sign: ' ', text: l.clone() });
        }
    }
    rows
}

/// So viele Zeilen zeigt ein Code-Block, bevor „weitere Zeilen“ kommt.
const CODE_ROWS: usize = 28;

/// Bearbeitung als Code-Block: Kopf „← Bearbeitet src/cart.ts +2 −2“, darunter die Abschnitte mit
/// Zeilennummern, rot entfernt und grün hinzugefügt, mit Syntax-Farben.
#[component]
fn EditCard(m: Message) -> Element {
    let call: ToolCall = serde_json::from_value(m.data.clone()).unwrap_or_default();
    let mut open = use_signal(|| true);
    let mut full = use_signal(|| false);
    let running = call.status == "running";
    let failed = call.status == "error";
    // Je Datei ihre Abschnitte (Reihenfolge wie geliefert), dazwischen „⋯“
    let mut files: Vec<(String, Vec<EditDiff>)> = Vec::new();
    for d in &call.diffs {
        match files.iter_mut().find(|(p, _)| *p == d.path) {
            Some((_, l)) => l.push(d.clone()),
            None => files.push((d.path.clone(), vec![d.clone()])),
        }
    }
    let several = files.len() > 1;
    let target = if call.target.is_empty() { files.first().map(|f| f.0.clone()).unwrap_or_default() } else { call.target.clone() };
    let created = call.kind == "write" && call.diffs.iter().all(|d| d.before.is_empty() && d.dels == 0 && d.line <= 1);
    let verb = if created { tr("Erstellt") } else if call.kind == "write" { tr("Geschrieben") } else { tr("Bearbeitet") };
    let (adds, dels) = call.diffs.iter().fold((0, 0), |(a, r), d| {
        let (x, y) = d.counts();
        (a + x, r + y)
    });
    // Alle Zeilen (mit Datei-Köpfen und Trennern), dann ggf. gekürzt
    let mut rows: Vec<(Option<String>, CodeRow)> = Vec::new();
    for (path, list) in &files {
        if several {
            rows.push((Some(path.clone()), CodeRow { no: None, sign: '#', text: path.clone() }));
        }
        for (i, d) in list.iter().enumerate() {
            if i > 0 {
                rows.push((None, CodeRow { no: None, sign: '⋯', text: String::new() }));
            }
            rows.extend(code_rows(d).into_iter().map(|r| (Some(path.clone()), r)));
        }
    }
    let total = rows.len();
    let hidden = if full() { 0 } else { total.saturating_sub(CODE_ROWS) };
    rows.truncate(total - hidden);
    rsx! {
        div { class: "overflow-hidden rounded-xl border bg-surface/60",
            button {
                class: "flex w-full min-w-0 items-center gap-2 px-3 py-1.5 text-left text-[13px] hover:bg-accent/30",
                onclick: move |_| open.toggle(),
                span { class: "shrink-0 text-muted-foreground", if created { icons::FilePlus { class: "size-3.5" } } else { icons::Pencil { class: "size-3.5" } } }
                span { class: "shrink-0 text-muted-foreground", "{verb}" }
                span { class: "min-w-0 truncate font-mono text-[12.5px] text-foreground", title: "{target}", "{target}" }
                LineStat { adds, dels, class: "shrink-0 text-xs" }
                span { class: "ml-auto flex shrink-0 items-center gap-1.5",
                    if running {
                        icons::LoaderCircle { class: "size-3.5 animate-spin text-muted-foreground" }
                    }
                    if failed {
                        icons::CircleX { class: "size-3.5 text-destructive" }
                    }
                    if open() {
                        icons::ChevronDown { class: "size-3.5 text-muted-foreground" }
                    } else {
                        icons::ChevronRight { class: "size-3.5 text-muted-foreground" }
                    }
                }
            }
            if open() && !rows.is_empty() {
                div { class: "work-code overflow-x-auto border-t py-1.5 font-mono text-[12.5px] leading-[1.65]",
                    for (i , (path , r)) in rows.into_iter().enumerate() {
                        {
                            let lang = crate::components::highlight::lang_of(path.as_deref().unwrap_or(""));
                            let class = match r.sign {
                                '+' => "row add",
                                '-' => "row del",
                                _ => "row",
                            };
                            rsx! {
                                match r.sign {
                                    '#' => rsx! {
                                        div { key: "{i}", class: "row mt-1 px-3 pb-0.5 font-sans text-[11.5px] text-muted-foreground first:mt-0", "{r.text}" }
                                    },
                                    '⋯' | '…' => rsx! {
                                        div { key: "{i}", class: "row text-muted-foreground/70",
                                            span { class: "no", "⋯" }
                                            span { class: "sign" }
                                            span { class: "font-sans text-[11.5px]", "{r.text}" }
                                        }
                                    },
                                    _ => rsx! {
                                        div { key: "{i}", class: "{class}",
                                            span { class: "no", if let Some(n) = r.no { "{n}" } }
                                            span { class: "sign",
                                                match r.sign {
                                                    '+' => "+",
                                                    '-' => "−",
                                                    _ => "",
                                                }
                                            }
                                            span { class: "whitespace-pre pr-6",
                                                for (k , (cls , t)) in crate::components::highlight::highlight(&r.text, lang).into_iter().enumerate() {
                                                    span { key: "{k}", class: cls, "{t}" }
                                                }
                                            }
                                        }
                                    },
                                }
                            }
                        }
                    }
                    if hidden > 0 {
                        button {
                            class: "mt-1 ml-11 font-sans text-xs text-muted-foreground hover:text-foreground",
                            onclick: move |_| full.set(true),
                            {trf("{0} weitere Zeilen anzeigen", &[&hidden])}
                        }
                    }
                }
            }
            if failed && !call.output.trim().is_empty() {
                pre { class: "max-h-48 overflow-auto border-t px-3 py-2 font-mono text-[12px] whitespace-pre-wrap text-destructive", "{call.output}" }
            }
        }
    }
}

/// Befehl als Block: „$ bun test“ mit Zustand; die Ausgabe klappt auf (bei Fehlern von selbst).
#[component]
fn CommandCard(m: Message) -> Element {
    let call: ToolCall = serde_json::from_value(m.data.clone()).unwrap_or_default();
    let mut open = use_signal(|| None::<bool>);
    let running = call.status == "running";
    let failed = call.status == "error";
    let output = call.output.trim_end().to_string();
    let has_output = !output.is_empty();
    let show = has_output && open().unwrap_or(failed);
    let full = call.input["command"].as_str().or(call.input["cmd"].as_str()).map(str::to_string).filter(|c| !c.is_empty()).unwrap_or_else(|| call.target.clone());
    rsx! {
        div { class: "overflow-hidden rounded-xl border bg-surface/60",
            button {
                class: "flex w-full min-w-0 items-center gap-2 px-3 py-1.5 text-left hover:bg-accent/30",
                onclick: move |_| open.set(Some(!show)),
                span { class: "shrink-0 font-mono text-[12.5px] text-muted-foreground", "$" }
                span { class: "min-w-0 flex-1 truncate font-mono text-[12.5px]", title: "{full}", "{call.target}" }
                span { class: "flex shrink-0 items-center gap-1.5",
                    if running {
                        icons::LoaderCircle { class: "size-3.5 animate-spin text-muted-foreground" }
                    } else if failed {
                        icons::CircleX { class: "size-3.5 text-destructive" }
                    } else {
                        icons::Check { class: "size-3.5 text-success" }
                    }
                    if has_output {
                        if show {
                            icons::ChevronDown { class: "size-3.5 text-muted-foreground" }
                        } else {
                            icons::ChevronRight { class: "size-3.5 text-muted-foreground" }
                        }
                    }
                }
            }
            if show {
                pre { class: "max-h-72 overflow-auto border-t px-3 py-2 font-mono text-[12px] leading-5 whitespace-pre-wrap text-muted-foreground",
                    if full.contains('\n') {
                        span { class: "text-foreground/80", "$ {full}\n" }
                    }
                    "{output}"
                }
            }
        }
    }
}

#[component]
fn Step(m: Message) -> Element {
    let call: ToolCall = serde_json::from_value(m.data.clone()).unwrap_or_default();
    let mut open = use_signal(|| false);
    let verb = step_verb(&call.kind);
    let target = if call.target.is_empty() { call.name.clone() } else { call.target.clone() };
    let label = if verb.is_empty() { call.summary.clone() } else { verb };
    let has_detail = !call.output.is_empty() || !call.diffs.is_empty() || !call.input.is_null();
    rsx! {
        div { class: "text-[13px]",
            button {
                class: "flex w-full min-w-0 items-center gap-2 rounded-md px-1.5 py-1 text-left text-muted-foreground hover:bg-accent/40 hover:text-foreground",
                onclick: move |_| if has_detail { open.toggle() },
                span { class: "shrink-0", {step_icon(&call.kind)} }
                span { class: "shrink-0 font-medium text-foreground/80", "{label}" }
                if call.kind != "other" || !call.target.is_empty() {
                    span { class: "min-w-0 truncate font-mono text-[12px]", "{target}" }
                }
                span { class: "ml-auto shrink-0",
                    match call.status.as_str() {
                        "running" => rsx! { icons::LoaderCircle { class: "size-3.5 animate-spin" } },
                        "error" => rsx! { icons::CircleX { class: "size-3.5 text-destructive" } },
                        _ => rsx! {},
                    }
                }
            }
            if open() {
                div { class: "mb-2 ml-6 flex flex-col gap-2",
                    for d in call.diffs.iter().cloned() {
                        MiniDiff { d }
                    }
                    if call.kind == "command" {
                        pre { class: "max-h-72 overflow-auto rounded-lg border bg-surface px-3 py-2 font-mono text-[12px] whitespace-pre-wrap",
                            span { class: "text-muted-foreground", "$ " }
                            "{call.target}\n"
                            "{call.output}"
                        }
                    } else if call.kind == "think" {
                        p { class: "whitespace-pre-wrap text-muted-foreground", "{call.output}" }
                    } else {
                        if call.diffs.is_empty() && !call.input.is_null() {
                            pre { class: "max-h-48 overflow-auto rounded-lg border bg-surface px-3 py-2 font-mono text-[11.5px] whitespace-pre-wrap",
                                {serde_json::to_string_pretty(&call.input).unwrap_or_default()}
                            }
                        }
                        if !call.output.is_empty() {
                            pre { class: "max-h-72 overflow-auto rounded-lg border bg-surface px-3 py-2 font-mono text-[12px] whitespace-pre-wrap", "{call.output}" }
                        }
                    }
                }
            }
        }
    }
}

/// Bearbeitung in den Einzelschritten (aufgeklappte Liste): dieselben Zeilen wie im Code-Block.
#[component]
fn MiniDiff(d: EditDiff) -> Element {
    let (adds, dels) = d.counts();
    let lang = crate::components::highlight::lang_of(&d.path);
    let rows = code_rows(&d);
    rsx! {
        div { class: "overflow-hidden rounded-lg border",
            div { class: "flex items-center gap-2 border-b bg-muted/40 px-3 py-1 font-mono text-[11.5px]",
                span { class: "truncate", "{d.path}" }
                span { class: "ml-auto text-success", "+{adds}" }
                span { class: "text-destructive", "−{dels}" }
            }
            div { class: "work-code max-h-72 overflow-auto py-1 font-mono text-[12px] leading-5",
                for (i , r) in rows.into_iter().enumerate() {
                    div {
                        key: "{i}",
                        class: match r.sign {
                            '+' => "row add",
                            '-' => "row del",
                            _ => "row",
                        },
                        span { class: "no", if let Some(n) = r.no { "{n}" } }
                        span { class: "sign",
                            match r.sign {
                                '+' => "+",
                                '-' => "−",
                                '…' => "⋯",
                                _ => "",
                            }
                        }
                        span { class: "whitespace-pre pr-6",
                            for (k , (cls , t)) in crate::components::highlight::highlight(&r.text, lang).into_iter().enumerate() {
                                span { key: "{k}", class: cls, "{t}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Subagenten

/// Sekundentakt (abonniert: die Komponente rendert jede Sekunde neu).
fn tick() -> i64 {
    crate::pages::workspace::parts::NOW()
}

/// Aktuelle Zeit ohne Abo.
fn peek_now() -> i64 {
    *crate::pages::workspace::parts::NOW.peek()
}

#[component]
fn SubagentCard(m: Message) -> Element {
    let data: SubagentsData = serde_json::from_value(m.data.clone()).unwrap_or_default();
    let mut open = use_signal(|| false);
    let all = THREADS.read();
    let kids: Vec<ThreadSummary> = data.children.iter().filter_map(|id| all.get(id).cloned()).collect();
    drop(all);
    let working = kids.iter().filter(|k| k.status.is_active()).count();
    // Takt nur, solange noch einer arbeitet
    let now = if working > 0 { tick() } else { peek_now() };
    let n = data.children.len();
    let kinds: Vec<_> = kids.iter().map(|k| k.agent).collect();
    let end = if working > 0 { now } else { kids.iter().map(|k| k.last_activity_at).max().unwrap_or(m.created_at) };
    let elapsed = duration(end - m.created_at);
    rsx! {
        div { class: "rounded-xl border bg-card",
            button { class: "flex w-full items-center gap-3 px-3 py-2.5 text-left", onclick: move |_| open.toggle(),
                AgentStack { kinds, size: 18 }
                div { class: "min-w-0 flex-1",
                    div { class: "text-sm font-medium", {subagents_text(n)} }
                    if working > 0 {
                        div { class: "text-xs text-info", {trf("{0} arbeiten", &[&working])} }
                    } else {
                        div { class: "text-xs text-muted-foreground", {tr("alle fertig")} }
                    }
                }
                span { class: "text-xs text-muted-foreground tabular-nums", "{elapsed}" }
                if open() {
                    icons::ChevronUp { class: "size-4 text-muted-foreground" }
                } else {
                    icons::ChevronDown { class: "size-4 text-muted-foreground" }
                }
            }
            if open() {
                div { class: "border-t px-2 py-1.5",
                    for k in kids {
                        SubagentRow { key: "{k.id}", t: k, now }
                    }
                }
            }
        }
    }
}

/// Zeile eines Subagenten (Karte im Chat und Lineage).
#[component]
pub fn SubagentRow(t: ThreadSummary, now: i64) -> Element {
    let time = if t.status.is_active() { duration(running_for(&t, now)) } else { duration((t.last_activity_at - t.created_at).max(0)) };
    rsx! {
        Link {
            to: Route::ThreadPage { id: t.id },
            class: "flex items-center gap-2.5 rounded-md px-2 py-1.5 text-sm hover:bg-accent/50",
            AgentIcon { kind: t.agent, size: 16 }
            span { class: "min-w-0 flex-1 truncate", "{t.title}" }
            crate::components::common::ChangeStat { thread: t.clone() }
            match t.status {
                ThreadStatus::Working | ThreadStatus::Waiting => rsx! { icons::LoaderCircle { class: "size-3.5 animate-spin text-muted-foreground" } },
                ThreadStatus::Error => rsx! { icons::CircleX { class: "size-3.5 text-destructive" } },
                ThreadStatus::Done | ThreadStatus::Idle | ThreadStatus::Settled => rsx! { icons::CircleCheck { class: "size-3.5 text-success" } },
            }
            span { class: "w-12 text-right text-xs text-muted-foreground tabular-nums", "{time}" }
        }
    }
}

// ------------------------------------------------------------------ Statuszeile

#[component]
fn StatusLine(thread: ThreadSummary) -> Element {
    let id = thread.id;
    let now = if thread.status.is_active() { tick() } else { peek_now() };
    let stop = move |_| {
        spawn(async move {
            match api::action::<ThreadSummary>(&format!("/threads/{id}/stop")).await {
                Ok(s) => state::upsert_thread(s),
                Err(e) => toast_error(e),
            }
        });
    };
    match thread.status {
        ThreadStatus::Working => rsx! {
            div { class: "flex items-center gap-2 text-[13px] text-muted-foreground",
                icons::LoaderCircle { class: "size-4 animate-spin" }
                span { {trf("Arbeitet · {0}", &[&duration(running_for(&thread, now))])} }
                button { class: "font-medium text-foreground hover:underline", onclick: stop, {tr("Stopp")} }
            }
        },
        ThreadStatus::Waiting if open_question(id) => rsx! {
            div { class: "flex items-center gap-2 text-[13px] text-muted-foreground",
                icons::MessageCircleQuestion { class: "size-4 text-info" }
                span { class: "text-foreground/80", {tr("Wartet auf deine Antwort")} }
                button { class: "font-medium text-foreground hover:underline", onclick: stop, {tr("Stopp")} }
            }
        },
        ThreadStatus::Waiting => {
            let kids: Vec<ThreadSummary> = THREADS.read().values().filter(|t| t.parent_id == Some(id) && t.status.is_active()).cloned().collect();
            let titles = kids.iter().map(|k| k.title.clone()).collect::<Vec<_>>().join(", ");
            rsx! {
                div { class: "flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground",
                    icons::LoaderCircle { class: "size-4 shrink-0 animate-spin [animation-duration:2.5s]" }
                    span { class: "shrink-0 text-foreground/80", {trf("Wartet auf {0}", &[&subagents_text(kids.len())])} }
                    span { class: "min-w-0 truncate", "{titles}" }
                    button { class: "shrink-0 font-medium text-foreground hover:underline", onclick: stop, {tr("Stopp")} }
                }
            }
        }
        _ if thread.children_running > 0 => {
            // Vom Benutzer gestartete Subagenten arbeiten noch
            let kids: Vec<ThreadSummary> = THREADS.read().values().filter(|t| t.parent_id == Some(id) && t.status.is_active()).cloned().collect();
            let titles = kids.iter().map(|k| k.title.clone()).collect::<Vec<_>>().join(", ");
            rsx! {
                div { class: "flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground",
                    icons::LoaderCircle { class: "size-4 shrink-0 animate-spin [animation-duration:2.5s]" }
                    span { class: "shrink-0 text-foreground/80", {trf("{0} arbeiten", &[&subagents_text(kids.len())])} }
                    span { class: "min-w-0 truncate", "{titles}" }
                    button { class: "shrink-0 font-medium text-foreground hover:underline", onclick: stop, {tr("Stopp")} }
                }
            }
        }
        _ => rsx! {},
    }
}

#[cfg(test)]
mod tests {
    use nrzx_shared::agents::EditDiff;

    fn signs(rows: &[super::CodeRow]) -> String {
        rows.iter().map(|r| r.sign).collect()
    }

    #[test]
    fn code_rows_with_line_numbers_and_context() {
        // Vom Server: Zeile 8, je zwei Zeilen Kontext
        let d = EditDiff {
            path: "src/cart.ts".into(),
            before: "  const taxed = subtotal(items) * (1 + taxRate)\n  return round(taxed - discount)".into(),
            after: "  const discounted = Math.max(0, subtotal(items) - discount)\n  return round(discounted * (1 + taxRate))".into(),
            line: 8,
            old_line: 8,
            context_before: vec!["".into(), "export function total(items: Item[], discount = 0, taxRate = 0.08) {".into()],
            context_after: vec!["}".into()],
            ..Default::default()
        };
        let rows = super::code_rows(&d);
        assert_eq!(signs(&rows), "  --++ ");
        let nos: Vec<Option<i64>> = rows.iter().map(|r| r.no).collect();
        assert_eq!(nos, vec![Some(6), Some(7), Some(8), Some(9), Some(8), Some(9), Some(10)]);
        // Ältere Nachricht (nur alter/neuer Text): gleiche Zeilen als Kontext, ohne Nummern
        let d = EditDiff { before: "a\nb\nc".into(), after: "a\nB\nc".into(), ..Default::default() };
        let rows = super::code_rows(&d);
        assert_eq!(signs(&rows), " -+ ");
        assert!(rows.iter().all(|r| r.no.is_none()));
    }
}

/// Hat der Thread eine offene Rückfrage?
fn open_question(thread_id: i64) -> bool {
    MESSAGES.read().get(&thread_id).is_some_and(|l| {
        l.iter().rev().take(30).any(|m| {
            m.role == "question" && serde_json::from_value::<QuestionData>(m.data.clone()).is_ok_and(|d| d.answer.is_none() && !d.expired)
        })
    })
}

/// "1:05:09" bzw. "4:09".
fn countdown(secs: i64) -> String {
    let s = secs.max(0);
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
}

/// Rate-Limit: übersetzte Meldung, Countdown bis zum Zurücksetzen, „Automatisch weitermachen“.
#[component]
fn RateLimitCard(thread_id: i64, data: SystemData) -> Element {
    let now = tick();
    let thread = THREADS.read().get(&thread_id).cloned();
    let Some(t) = thread else { return rsx! {} };
    let at = t.resume_at.or(data.resets_at).unwrap_or(now);
    let left = at - now;
    let auto = t.resume_auto;
    let set_auto = move |v: bool| {
        spawn(async move {
            let patch = ThreadPatch { resume_auto: Some(v), ..Default::default() };
            match api::patch::<ThreadSummary, _>(&format!("/threads/{thread_id}"), &patch).await {
                Ok(s) => state::upsert_thread(s),
                Err(e) => toast_error(e),
            }
        });
    };
    let resume = move |_| {
        spawn(async move {
            match api::action::<ThreadSummary>(&format!("/threads/{thread_id}/resume")).await {
                Ok(s) => state::upsert_thread(s),
                Err(e) => toast_error(e),
            }
        });
    };
    let dismiss = move |_| {
        spawn(async move {
            if let Err(e) = api::delete(&format!("/threads/{thread_id}/resume")).await {
                toast_error(e);
            } else if let Some(t) = THREADS.write().get_mut(&thread_id) {
                t.resume_at = None;
            }
        });
    };
    let over = left <= 0;
    rsx! {
        div { class: "rounded-xl border border-warning/40 bg-warning/5 p-4",
            div { class: "flex items-start gap-3",
                div { class: "grid size-9 shrink-0 place-items-center rounded-lg bg-warning/15 text-warning",
                    icons::Hourglass { class: "size-4" }
                }
                div { class: "min-w-0 flex-1",
                    p { class: "text-sm font-semibold", {tr("Nutzungslimit erreicht")} }
                    p { class: "mt-0.5 text-sm text-muted-foreground",
                        if over {
                            {tr("Das Limit ist zurückgesetzt.")}
                        } else {
                            {trf("Das Limit wird um {0} zurückgesetzt.", &[&util::clock(at)])}
                        }
                    }
                }
                if !over {
                    div { class: "shrink-0 text-right",
                        p { class: "font-mono text-2xl font-semibold tabular-nums leading-none", "{countdown(left)}" }
                        p { class: "mt-1 text-[11px] text-muted-foreground", {tr("bis zum Reset")} }
                    }
                }
            }
            div { class: "mt-4 flex flex-wrap items-center gap-x-4 gap-y-2 border-t border-warning/20 pt-3",
                Toggle { checked: auto, onchange: set_auto, label: tr("Automatisch weitermachen") }
                span { class: "ml-auto flex items-center gap-3 text-sm",
                    button { class: "font-medium hover:underline", onclick: resume, {if over || !auto { tr("Jetzt weitermachen") } else { tr("Jetzt versuchen") }} }
                    button { class: "text-muted-foreground hover:text-foreground hover:underline", onclick: dismiss, {tr("Verwerfen")} }
                }
            }
            if !data.detail.is_empty() {
                p { class: "mt-2 truncate text-[11px] text-muted-foreground/70", title: "{data.detail}", "{data.detail}" }
            }
        }
    }
}

/// Rückfrage des Agenten: Optionen, eigene Antwort oder „Selbst entscheiden lassen“.
#[component]
fn QuestionCard(m: Message) -> Element {
    let data: QuestionData = serde_json::from_value(m.data.clone()).unwrap_or_default();
    let thread_id = m.thread_id;
    let mid = m.id;
    let mut text = use_signal(String::new);
    let mut chosen = use_signal(Vec::<usize>::new);
    let mut busy = use_signal(|| false);
    let mut submit = move |answer: String, delegate: bool| {
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            let body = AnswerQuestion { answer, delegate };
            match api::post::<ThreadSummary, _>(&format!("/threads/{thread_id}/questions/{mid}"), &body).await {
                Ok(s) => state::upsert_thread(s),
                Err(e) => toast_error(e),
            }
            busy.set(false);
        });
    };

    // Beantwortet oder abgelaufen: kompakt
    if data.answer.is_some() || data.expired {
        let answer = match (&data.answer, data.delegated) {
            (_, true) => tr("Dem Agenten überlassen"),
            (Some(a), _) => a.clone(),
            (None, _) => tr("Nicht beantwortet – der Agent hat selbst entschieden"),
        };
        return rsx! {
            div { class: "rounded-xl border bg-card/50 px-4 py-3 text-sm",
                div { class: "flex items-start gap-2 text-muted-foreground",
                    icons::MessageCircleQuestion { class: "mt-0.5 size-4 shrink-0" }
                    p { class: "min-w-0 whitespace-pre-wrap", "{m.content}" }
                }
                div { class: "mt-1.5 flex items-start gap-2 pl-6",
                    icons::CornerDownRight { class: "mt-0.5 size-3.5 shrink-0 text-muted-foreground" }
                    p { class: "min-w-0 font-medium whitespace-pre-wrap", "{answer}" }
                }
            }
        };
    }

    let options = data.options.clone();
    let multiple = data.multiple;
    rsx! {
        div { class: "rounded-xl border border-info/40 bg-card p-4 shadow-sm",
            div { class: "mb-1 flex items-center gap-2 text-xs font-medium text-info",
                icons::MessageCircleQuestion { class: "size-4" }
                {tr("Rückfrage")}
            }
            p { class: "text-[15px] leading-relaxed whitespace-pre-wrap", "{m.content}" }
            if !options.is_empty() {
                div { class: "mt-3 flex flex-col gap-1.5",
                    for (i , o) in options.iter().cloned().enumerate() {
                        button {
                            key: "{i}",
                            class: if chosen().contains(&i) { "flex w-full items-start gap-3 rounded-lg border border-primary bg-accent px-3 py-2 text-left" } else { "flex w-full items-start gap-3 rounded-lg border px-3 py-2 text-left hover:bg-accent disabled:opacity-50" },
                            disabled: busy(),
                            onclick: {
                                let label = o.label.clone();
                                move |_| {
                                    if multiple {
                                        let mut c = chosen();
                                        if let Some(p) = c.iter().position(|x| *x == i) { c.remove(p); } else { c.push(i); }
                                        chosen.set(c);
                                    } else {
                                        submit(label.clone(), false);
                                    }
                                }
                            },
                            span { class: "mt-px grid size-5 shrink-0 place-items-center rounded-md border text-[11px] font-medium text-muted-foreground", "{i + 1}" }
                            span { class: "min-w-0",
                                span { class: "block text-sm font-medium", "{o.label}" }
                                if !o.description.is_empty() {
                                    span { class: "block text-xs text-muted-foreground", "{o.description}" }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "mt-3 flex items-center gap-2",
                input {
                    class: "h-9 min-w-0 flex-1 rounded-lg border bg-transparent px-3 text-sm outline-none focus:border-ring",
                    placeholder: if options.is_empty() { tr("Antwort …") } else { tr("Eigene Antwort …") },
                    value: "{text}",
                    disabled: busy(),
                    oninput: move |e| text.set(e.value()),
                    onkeydown: {
                        let options = options.clone();
                        move |e: KeyboardEvent| {
                            if e.key() == Key::Enter {
                                let t = text().trim().to_string();
                                let picked: Vec<String> = chosen().iter().filter_map(|i| options.get(*i).map(|o| o.label.clone())).collect();
                                let answer = [picked.join(", "), t].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" – ");
                                if !answer.is_empty() {
                                    submit(answer, false);
                                }
                            }
                        }
                    },
                }
                button {
                    class: "h-9 shrink-0 rounded-lg bg-primary px-3 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-40",
                    disabled: busy() || (text().trim().is_empty() && chosen().is_empty()),
                    onclick: {
                        let options = options.clone();
                        move |_| {
                            let picked: Vec<String> = chosen().iter().filter_map(|i| options.get(*i).map(|o| o.label.clone())).collect();
                            let answer = [picked.join(", "), text().trim().to_string()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" – ");
                            submit(answer, false);
                        }
                    },
                    {tr("Antworten")}
                }
            }
            div { class: "mt-2 flex justify-end",
                button {
                    class: "flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground disabled:opacity-50",
                    disabled: busy(),
                    title: tr("Der Agent wählt selbst die sinnvollste Lösung"),
                    onclick: move |_| submit(String::new(), true),
                    icons::Sparkles { class: "size-3.5" }
                    {tr("Selbst entscheiden lassen")}
                }
            }
        }
    }
}

/// Ein Eintrag der Todo-Liste des Agenten.
#[derive(Clone, PartialEq)]
struct Todo {
    text: String,
    /// 0 offen, 1 in Arbeit, 2 erledigt
    state: u8,
}

/// Neueste Todo-Liste des letzten Laufs (Claude TodoWrite, OpenCode todowrite, Gemini
/// write_todos, Codex todo_list/update_plan).
fn latest_todos(msgs: &[Message]) -> Option<Vec<Todo>> {
    let last_run = msgs.iter().rev().find_map(|m| m.run_id)?;
    let m = msgs.iter().rev().filter(|m| m.run_id == Some(last_run)).find(|m| {
        m.role == "tool" && serde_json::from_value::<ToolCall>(m.data.clone()).is_ok_and(|c| c.kind == "todo" || c.name == "update_plan")
    })?;
    let call: ToolCall = serde_json::from_value(m.data.clone()).ok()?;
    let input = &call.input;
    let items = input["todos"].as_array().or(input["items"].as_array()).or(input["plan"].as_array())?;
    let list: Vec<Todo> = items
        .iter()
        .filter_map(|i| {
            let status = i["status"].as_str().unwrap_or("");
            let state = if i["completed"].as_bool() == Some(true) || status == "completed" || status == "done" {
                2
            } else if status == "in_progress" || status == "active" {
                1
            } else {
                0
            };
            // Für die laufende Aufgabe die Verlaufsform („Baue …“), falls vorhanden
            let keys: &[&str] = if state == 1 { &["activeForm", "content", "text", "description", "step", "title"] } else { &["content", "text", "description", "step", "title"] };
            let text = keys.iter().find_map(|k| i[*k].as_str()).unwrap_or("").trim().to_string();
            (!text.is_empty()).then_some(Todo { text, state })
        })
        .collect();
    (!list.is_empty()).then_some(list)
}

/// Todo-Liste des Agenten über dem Composer: Fortschritt und aktuelle Aufgabe, aufklappbar.
#[component]
fn TodoBar(thread_id: i64, active: bool) -> Element {
    let mut open = use_signal(|| util::storage_get("nrz-todos-open").as_deref() != Some("0"));
    let todos = MESSAGES.read().get(&thread_id).and_then(|l| latest_todos(l));
    let Some(todos) = todos else { return rsx! {} };
    let done = todos.iter().filter(|t| t.state == 2).count();
    let total = todos.len();
    if done == total && !active {
        return rsx! {};
    }
    let current = todos.iter().find(|t| t.state == 1).or_else(|| todos.iter().find(|t| t.state == 0)).map(|t| t.text.clone()).unwrap_or_default();
    let pct = done * 100 / total.max(1);
    rsx! {
        div { class: "mb-2 overflow-hidden rounded-xl border bg-card/60",
            button {
                class: "flex w-full items-center gap-2.5 px-3 py-2 text-left text-sm",
                onclick: move |_| {
                    let v = !open();
                    open.set(v);
                    util::storage_set("nrz-todos-open", if v { "1" } else { "0" });
                },
                icons::ListChecks { class: "size-4 shrink-0 text-muted-foreground" }
                span { class: "shrink-0 font-medium tabular-nums", {trf("Aufgaben {0}/{1}", &[&done, &total])} }
                div { class: "h-1.5 w-16 shrink-0 overflow-hidden rounded-full bg-muted",
                    div { class: "h-full rounded-full bg-success transition-all", style: "width: {pct}%" }
                }
                if !open() && !current.is_empty() {
                    span { class: "min-w-0 truncate text-muted-foreground", "{current}" }
                }
                span { class: "ml-auto shrink-0 text-muted-foreground",
                    if open() {
                        icons::ChevronDown { class: "size-4" }
                    } else {
                        icons::ChevronUp { class: "size-4" }
                    }
                }
            }
            if open() {
                ul { class: "max-h-56 overflow-y-auto border-t px-3 py-2",
                    for (i , t) in todos.into_iter().enumerate() {
                        li { key: "{i}", class: "flex items-start gap-2.5 py-1 text-sm",
                            span { class: "mt-0.5 shrink-0",
                                match t.state {
                                    2 => rsx! { icons::CircleCheck { class: "size-4 text-success" } },
                                    1 => rsx! { icons::LoaderCircle { class: if active { "size-4 animate-spin text-info" } else { "size-4 text-info" } } },
                                    _ => rsx! { icons::Circle { class: "size-4 text-muted-foreground/60" } },
                                }
                            }
                            span {
                                class: match t.state {
                                    2 => "min-w-0 text-muted-foreground line-through decoration-muted-foreground/40",
                                    1 => "min-w-0 font-medium",
                                    _ => "min-w-0 text-foreground/80",
                                },
                                "{t.text}"
                            }
                        }
                    }
                }
            }
        }
    }
}
