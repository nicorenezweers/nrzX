//! nrzXMobile – Oberfläche fürs Handy (Dioxus 0.7, WebAssembly), gestaltet wie eine iOS-App:
//! grosser Titel, Liste als Karte mit runden Ecken, Blätter von unten, Kopf- und Eingabeleisten
//! im Milchglas, Systemschrift und sichere Ränder (Notch, Home-Balken).
//!
//! * Anmelden mit eigenem Konto (beim ersten Aufruf: erstes Konto anlegen). Jeder sieht nur seine
//!   PCs und deren Chats.
//! * Chats eines gekoppelten PCs live, Chat öffnen, weiter anweisen (Warteschlange oder
//!   Zwischenruf), stoppen, neuen Chat beginnen; Nutzungslimits; Einstellungen mit PCs, Passwort
//!   und – für Administratoren – Konten.
//!
//! Alles geht an den eigenen Server (`mobile/src/main.rs`): `/api/auth/…`, `/api/users`,
//! `/api/pcs`, `/api/pair` und `/api/pc/<id>/v1/…` – das reicht er an nrzX am PC weiter.

mod api;

use dioxus::prelude::*;
use nrzx_shared::agents::{AccountUsage, AgentInfo, NewThread, PostMessage, UsageLimit, UsageOverview};
use nrzx_shared::team::{address as team_address, TeamPost, TeamView};
use nrzx_shared::{AgentKind, HubEvent, Message, ProjectSummary, ThreadStatus, ThreadSummary};
use serde::Deserialize;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

static CSS: Asset = asset!("/assets/tailwind.css");

/// Ein gekoppelter PC (`GET /api/pcs`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PcInfo {
    pub id: String,
    pub name: String,
    pub online: bool,
    pub last_seen: i64,
}

/// Angemeldetes Konto.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub id: String,
    pub username: String,
    pub admin: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Auth {
    Loading,
    /// Noch kein Konto – erstes anlegen
    Setup,
    Login,
    In(Me),
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Chats,
    Team,
    Limits,
    Settings,
}

static AUTH: GlobalSignal<Auth> = Signal::global(|| Auth::Loading);
/// Gekoppelte PCs (`None` = noch nicht geladen).
static PCS: GlobalSignal<Option<Vec<PcInfo>>> = Signal::global(|| None);
/// Gewählter PC (im Browser gemerkt).
static PC: GlobalSignal<Option<String>> = Signal::global(|| storage_get("nrzm-pc"));
/// Ist der gewählte PC gerade verbunden?
static ONLINE: GlobalSignal<bool> = Signal::global(|| false);
static THREADS: GlobalSignal<HashMap<i64, ThreadSummary>> = Signal::global(HashMap::new);
/// Erste Liste des PCs geladen?
static THREADS_LOADED: GlobalSignal<bool> = Signal::global(|| false);
static MESSAGES: GlobalSignal<HashMap<i64, Vec<Message>>> = Signal::global(HashMap::new);
static OPEN: GlobalSignal<Option<i64>> = Signal::global(|| None);
/// Zählt hoch, wenn sich ein Team am PC ändert (die Team-Ansicht lädt dann neu).
static TEAM_REV: GlobalSignal<u64> = Signal::global(|| 0);
static SCREEN: GlobalSignal<Screen> = Signal::global(|| Screen::Chats);
static PAIRING: GlobalSignal<bool> = Signal::global(|| false);
static NEW_CHAT: GlobalSignal<bool> = Signal::global(|| false);
static TOAST: GlobalSignal<Option<String>> = Signal::global(|| None);
/// Sekundentakt für „Arbeitet 2m“.
static NOW: GlobalSignal<i64> = Signal::global(now);
/// Version dieser Oberfläche (gleich wie die des Servers, mit dem sie gebaut ist).
const UI_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Version, die am Server läuft (aus `/api/info`); weicht sie ab, gibt es „Neu laden“.
static SERVER_VERSION: GlobalSignal<Option<String>> = Signal::global(|| None);

#[derive(Deserialize)]
struct Info {
    version: String,
}

/// Version am Server nachsehen.
async fn load_info() -> Option<String> {
    let v = api::get::<Info>("/api/info").await.ok().map(|i| i.version)?;
    if SERVER_VERSION.peek().as_deref() != Some(v.as_str()) {
        *SERVER_VERSION.write() = Some(v.clone());
    }
    Some(v)
}

/// Version `a` neuer als `b` („1.10.0“ > „1.9.3“)?
fn newer(a: &str, b: &str) -> bool {
    let p = |v: &str| v.trim().split('.').map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    p(a) > p(b)
}

/// Seite neu laden (neue Oberfläche vom Server).
fn reload() {
    if let Some(w) = web_sys::window() {
        let _ = w.location().reload();
    }
}

fn main() {
    dioxus::launch(App);
}

pub fn now() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn storage_get(k: &str) -> Option<String> {
    storage()?.get_item(k).ok().flatten()
}

fn storage_set(k: &str, v: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(k, v);
    }
}

fn storage_remove(k: &str) {
    if let Some(s) = storage() {
        let _ = s.remove_item(k);
    }
}

fn toast(text: impl Into<String>) {
    let t = text.into();
    *TOAST.write() = Some(t.clone());
    spawn_forever_local(async move {
        gloo_timers::future::TimeoutFuture::new(3500).await;
        if TOAST.peek().as_deref() == Some(t.as_str()) {
            *TOAST.write() = None;
        }
    });
}

/// Aufgabe, die nicht an eine Komponente gebunden ist: läuft weiter, auch wenn die Ansicht
/// wechselt (ein `spawn` im Listeneintrag brach beim Öffnen des Chats ab – der Chat lud nie).
fn spawn_forever_local(f: impl std::future::Future<Output = ()> + 'static) {
    dioxus::core::spawn_forever(f);
}

/// Sitzung abgelaufen bzw. abgemeldet: alles vergessen, zur Anmeldung.
pub fn signed_out() {
    if matches!(*AUTH.peek(), Auth::In(_)) {
        *AUTH.write() = Auth::Login;
    }
    close_events();
    *PCS.write() = None;
    THREADS.write().clear();
    MESSAGES.write().clear();
    *THREADS_LOADED.write() = false;
    *OPEN.write() = None;
}

/// „12s“, „4m“, „3h“, „2d“
fn short(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// „1h 38m“, „6d 14h“
fn reset_in(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 86_400 {
        format!("{}d {}h", s / 86_400, (s % 86_400) / 3600)
    } else if s >= 3600 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}m", (s / 60).max(1))
    }
}

/// Uhrzeit „14:05“ in der Zeitzone des Geräts.
fn clock(ts: i64) -> String {
    let d = js_sys::Date::new(&JsValue::from_f64(ts as f64 * 1000.0));
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}

fn copy(text: &str) {
    if let Some(c) = web_sys::window().map(|w| w.navigator().clipboard()) {
        let _ = c.write_text(text);
        toast("Kopiert");
    }
}

/// Immer dunkel (wie nrzX); Statusleiste passend.
fn dark_theme() {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    if let Some(el) = doc.document_element() {
        let _ = el.class_list().remove_1("light");
        let _ = el.set_attribute("style", "color-scheme: dark");
    }
    let meta = doc.query_selector("meta[name=theme-color]").ok().flatten().or_else(|| {
        let m = doc.create_element("meta").ok()?;
        let _ = m.set_attribute("name", "theme-color");
        doc.head()?.append_child(&m).ok()?;
        Some(m)
    });
    if let Some(m) = meta {
        let _ = m.set_attribute("content", "#1b1b1b");
    }
}

// ------------------------------------------------------------------ Laden und live

#[derive(Deserialize)]
struct AuthState {
    setup: bool,
    user: Option<Me>,
}

async fn load_auth() {
    match api::get::<AuthState>("/api/auth/state").await {
        Ok(s) => {
            *AUTH.write() = match (s.user, s.setup) {
                (Some(u), _) => Auth::In(u),
                (None, true) => Auth::Setup,
                (None, false) => Auth::Login,
            };
        }
        Err(e) => {
            *AUTH.write() = Auth::Login;
            toast(e);
        }
    }
}

async fn load_pcs() {
    match api::get::<Vec<PcInfo>>("/api/pcs").await {
        Ok(list) => {
            let cur = PC.peek().clone();
            let valid = cur.as_ref().is_some_and(|c| list.iter().any(|p| &p.id == c));
            if !valid {
                let first = list.iter().find(|p| p.online).or(list.first()).map(|p| p.id.clone());
                select_pc(first);
            }
            if let Some(p) = list.iter().find(|p| Some(&p.id) == PC.peek().as_ref()) {
                if *ONLINE.peek() != p.online {
                    *ONLINE.write() = p.online;
                }
            }
            if PCS.peek().as_ref() != Some(&list) {
                *PCS.write() = Some(list);
            }
        }
        Err(e) => {
            if PCS.peek().is_none() && matches!(*AUTH.peek(), Auth::In(_)) {
                *PCS.write() = Some(Vec::new());
            }
            toast(e);
        }
    }
}

fn select_pc(id: Option<String>) {
    match &id {
        Some(i) => storage_set("nrzm-pc", i),
        None => storage_remove("nrzm-pc"),
    }
    if *PC.peek() == id {
        return;
    }
    *PC.write() = id;
    THREADS.write().clear();
    MESSAGES.write().clear();
    *THREADS_LOADED.write() = false;
    *OPEN.write() = None;
}

/// Pfad der API von nrzX am gewählten PC.
fn pc_path(rest: &str) -> Option<String> {
    PC.peek().as_ref().map(|id| format!("/api/pc/{id}/v1/{}", rest.trim_start_matches('/')))
}

async fn load_threads() {
    let Some(path) = pc_path("threads?parent=none&limit=200") else { return };
    match api::get::<Vec<ThreadSummary>>(&path).await {
        Ok(list) => {
            *THREADS.write() = list.into_iter().map(|t| (t.id, t)).collect();
            *THREADS_LOADED.write() = true;
        }
        Err(e) if e.contains("nicht verbunden") => {}
        Err(e) => toast(e),
    }
}

async fn load_messages(id: i64) {
    let Some(path) = pc_path(&format!("threads/{id}/messages?tail=150")) else { return };
    match api::get::<Vec<Message>>(&path).await {
        Ok(list) => {
            MESSAGES.write().insert(id, list);
        }
        Err(e) => {
            // Ohne Liste bliebe der Ladekreis stehen
            MESSAGES.write().entry(id).or_default();
            toast(e);
        }
    }
}

/// Chat öffnen und seinen Verlauf laden (unabhängig von der Ansicht, die das auslöst).
fn open_thread(id: i64) {
    *OPEN.write() = Some(id);
    spawn_forever_local(load_messages(id));
}

/// Ereignis vom PC: Threads und Nachrichten aktuell halten.
fn apply(ev: HubEvent) {
    match ev {
        HubEvent::ThreadUpdated { thread } => {
            if thread.parent_id.is_none() {
                THREADS.write().insert(thread.id, thread);
            }
        }
        HubEvent::ThreadDeleted { id } => {
            THREADS.write().remove(&id);
            MESSAGES.write().remove(&id);
        }
        HubEvent::Message { message } => {
            let mut all = MESSAGES.write();
            if let Some(list) = all.get_mut(&message.thread_id) {
                match list.iter().position(|m| m.id == message.id) {
                    Some(i) => list[i] = message,
                    None => list.push(message),
                }
            }
        }
        HubEvent::MessagesRemoved { thread_id, from_id } => {
            if let Some(list) = MESSAGES.write().get_mut(&thread_id) {
                list.retain(|m| m.id < from_id);
            }
        }
        HubEvent::TeamUpdated { .. } => *TEAM_REV.write() += 1,
        _ => {}
    }
}

thread_local! {
    static SOURCE: std::cell::RefCell<Option<(web_sys::EventSource, Closure<dyn FnMut(web_sys::MessageEvent)>)>> = const { std::cell::RefCell::new(None) };
}

fn close_events() {
    SOURCE.with(|s| {
        if let Some((es, _)) = s.borrow_mut().take() {
            es.close();
        }
    });
}

/// Server-Sent Events des PCs öffnen (vorherige schliessen).
fn listen(pc: &str) {
    close_events();
    let Ok(es) = web_sys::EventSource::new(&format!("/api/pc/{pc}/events")) else { return };
    let cb = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
        let Some(text) = e.data().as_string() else { return };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return };
        match v["kind"].as_str() {
            Some("online") => {
                *ONLINE.write() = true;
                spawn_forever_local(async {
                    load_threads().await;
                    if let Some(id) = *OPEN.peek() {
                        load_messages(id).await;
                    }
                });
            }
            Some("offline") => *ONLINE.write() = false,
            Some("event") => {
                if let Ok(ev) = serde_json::from_value::<HubEvent>(v["event"].clone()) {
                    apply(ev);
                }
            }
            _ => {}
        }
    });
    es.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    SOURCE.with(|s| *s.borrow_mut() = Some((es, cb)));
}

// ------------------------------------------------------------------ Rahmen

#[component]
fn App() -> Element {
    use_hook(|| {
        dark_theme();
        spawn_forever_local(async {
            load_auth().await;
            load_info().await;
            loop {
                gloo_timers::future::TimeoutFuture::new(1000).await;
                *NOW.write() = now();
                let signed_in = matches!(*AUTH.peek(), Auth::In(_));
                // Alle 20 s nachsehen, welche PCs da sind
                // Jede Minute: läuft am Server eine neuere Version?
                if now() % 60 == 0 {
                    load_info().await;
                }
                if now() % 20 == 0 && signed_in {
                    load_pcs().await;
                }
                // Absicherung: Liste fehlt noch (Netz weg, PC kam später) – alle 5 s neu versuchen
                if now() % 5 == 0 && signed_in && PC.peek().is_some() && !*THREADS_LOADED.peek() {
                    load_threads().await;
                }
            }
        });
    });
    // Angemeldet: PCs laden
    let signed_in = matches!(AUTH(), Auth::In(_));
    use_effect(use_reactive!(|signed_in| {
        if signed_in {
            spawn_forever_local(load_pcs());
        }
    }));
    // Angemeldet und PC bekannt (auch beim erneuten Öffnen mit gemerktem PC und noch gültiger
    // Sitzung – vorher hing das nur am Wechsel des PCs, die Chats luden dann nie): Ereignisse und
    // Threads. Hängt an beidem, nicht nur am PC.
    let pc_now = PC();
    use_effect(use_reactive!(|signed_in, pc_now| {
        if !signed_in {
            return;
        }
        if let Some(pc) = pc_now {
            listen(&pc);
            spawn_forever_local(load_threads());
        }
    }));
    let auth = AUTH();
    let pcs = PCS();
    rsx! {
        document::Stylesheet { href: CSS }
        document::Meta { name: "viewport", content: "width=device-width, initial-scale=1, viewport-fit=cover, interactive-widget=resizes-content" }
        document::Meta { name: "apple-mobile-web-app-capable", content: "yes" }
        document::Meta { name: "mobile-web-app-capable", content: "yes" }
        document::Meta { name: "apple-mobile-web-app-title", content: "nrzX" }
        document::Meta { name: "apple-mobile-web-app-status-bar-style", content: "black" }
        document::Meta { name: "format-detection", content: "telephone=no" }
        document::Link { rel: "manifest", href: "/manifest.webmanifest" }
        document::Link { rel: "apple-touch-icon", href: "/icon.png" }
        document::Link { rel: "icon", href: "/icon.png" }
        div { class: "flex h-dvh flex-col overflow-hidden bg-page text-foreground antialiased",
            match auth {
                Auth::Loading => rsx! { div { class: "grid flex-1 place-items-center", Spinner {} } },
                Auth::Setup => rsx! { AuthScreen { setup: true } },
                Auth::Login => rsx! { AuthScreen { setup: false } },
                Auth::In(me) => match pcs {
                    None => rsx! { div { class: "grid flex-1 place-items-center", Spinner {} } },
                    Some(list) if list.is_empty() && SCREEN() != Screen::Settings => rsx! { Welcome { me } },
                    Some(_) => match (OPEN(), SCREEN()) {
                        (Some(id), _) => rsx! { ChatScreen { key: "{id}", id } },
                        (None, Screen::Settings) => rsx! { Settings { me } },
                        (None, Screen::Limits) => rsx! { Limits {} },
                        (None, Screen::Team) => rsx! { Team {} },
                        (None, Screen::Chats) => rsx! { Home {} },
                    },
                },
            }
            if PAIRING() {
                PairSheet {}
            }
            if NEW_CHAT() {
                NewChatSheet {}
            }
            // Am Server läuft eine neuere Version als diese Oberfläche (nach einem Update): neu laden
            if let Some(v) = SERVER_VERSION().filter(|v| newer(v, UI_VERSION)) {
                div { class: "fixed inset-x-3 bottom-[max(env(safe-area-inset-bottom),0.75rem)] z-[55] flex items-center gap-3 rounded-2xl bg-elevated px-4 py-3 shadow-2xl ring-1 ring-link/40 animate-drop",
                    icons::Sparkles { class: "size-5 shrink-0 text-link" }
                    div { class: "min-w-0 flex-1",
                        p { class: "text-[15px] font-semibold", "Neue Version {v}" }
                        p { class: "text-[13px] text-muted-foreground", "Neu laden, um sie zu benutzen." }
                    }
                    button { class: "h-9 shrink-0 rounded-full bg-link px-4 text-[15px] font-semibold text-white active:opacity-70", onclick: move |_| reload(), "Neu laden" }
                }
            }
            if let Some(t) = TOAST() {
                div { class: "pointer-events-none fixed inset-x-4 top-[max(env(safe-area-inset-top),0.75rem)] z-[60] flex justify-center",
                    div { class: "glass max-w-full rounded-2xl px-4 py-2.5 text-center text-[14px] font-medium shadow-xl animate-drop", "{t}" }
                }
            }
        }
    }
}

#[component]
fn Spinner() -> Element {
    rsx! { icons::LoaderCircle { class: "size-5 animate-spin text-muted-foreground" } }
}

/// Grosser Titel wie in iOS-Apps, rechts Knöpfe.
#[component]
fn LargeHeader(title: String, #[props(default)] subtitle: Option<Element>, #[props(default)] actions: Option<Element>, #[props(default)] back: bool) -> Element {
    rsx! {
        header { class: "shrink-0 px-5 pt-[max(env(safe-area-inset-top),0.75rem)] pb-3",
            if back {
                button { class: "-ml-2 mb-1 flex h-9 items-center gap-0.5 text-[17px] text-link active:opacity-50", onclick: move |_| *SCREEN.write() = Screen::Chats,
                    icons::ChevronLeft { class: "size-6" }
                    "Chats"
                }
            }
            div { class: "flex items-center gap-3",
                div { class: "min-w-0 flex-1",
                    h1 { class: "truncate text-[32px] leading-tight font-bold tracking-tight", "{title}" }
                    if let Some(s) = subtitle {
                        {s}
                    }
                }
                if let Some(a) = actions {
                    div { class: "flex shrink-0 items-center gap-1", {a} }
                }
            }
        }
    }
}

const ICON_BTN: &str = "grid size-10 place-items-center rounded-full text-foreground/90 active:bg-foreground/10 [&_svg]:size-[22px]";

// ------------------------------------------------------------------ Anmeldung

#[component]
fn AuthScreen(setup: bool) -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut repeat = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let submit = move |e: FormEvent| {
        e.prevent_default();
        if busy() {
            return;
        }
        if setup && password() != repeat() {
            error.set(Some("Die Passwörter stimmen nicht überein.".into()));
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_forever_local(async move {
            let path = if setup { "/api/auth/setup" } else { "/api/auth/login" };
            let body = serde_json::json!({ "username": username.peek().trim(), "password": *password.peek() });
            match api::post::<serde_json::Value, _>(path, &body).await {
                Ok(v) => match serde_json::from_value::<Me>(v["user"].clone()) {
                    Ok(me) => *AUTH.write() = Auth::In(me),
                    Err(e) => error.set(Some(e.to_string())),
                },
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };
    rsx! {
        div { class: "flex flex-1 flex-col overflow-y-auto px-6 pt-[max(env(safe-area-inset-top),2rem)] pb-[max(env(safe-area-inset-bottom),1.5rem)]",
            div { class: "flex flex-1 flex-col justify-center",
                div { class: "mb-8 flex flex-col items-center text-center",
                    img { src: "/icon.png", class: "size-20 rounded-[22px] shadow-lg", alt: "" }
                    h1 { class: "mt-5 text-[28px] font-bold tracking-tight", "nrzX" }
                    p { class: "mt-1.5 text-[15px] text-muted-foreground",
                        if setup { "Lege das erste Konto an – es verwaltet weitere Konten. Jedes Konto sieht nur seine eigenen PCs und Chats." } else { "Melde dich an, um deine Chats zu sehen." }
                    }
                }
                form { class: "flex flex-col gap-5", onsubmit: submit,
                    div { class: "group-list",
                        input {
                            class: "field",
                            r#type: "text",
                            name: "username",
                            autocomplete: "username",
                            autocapitalize: "none",
                            autocorrect: "off",
                            spellcheck: "false",
                            placeholder: "Benutzername",
                            value: "{username}",
                            oninput: move |e| username.set(e.value()),
                        }
                        input {
                            class: "field",
                            r#type: "password",
                            name: "password",
                            autocomplete: if setup { "new-password" } else { "current-password" },
                            placeholder: "Passwort",
                            value: "{password}",
                            oninput: move |e| password.set(e.value()),
                        }
                        if setup {
                            input {
                                class: "field",
                                r#type: "password",
                                autocomplete: "new-password",
                                placeholder: "Passwort wiederholen",
                                value: "{repeat}",
                                oninput: move |e| repeat.set(e.value()),
                            }
                        }
                    }
                    if let Some(e) = error() {
                        p { class: "-mt-2 px-1 text-[14px] text-destructive", "{e}" }
                    }
                    button {
                        class: "btn-primary",
                        r#type: "submit",
                        disabled: busy() || username().trim().is_empty() || password().is_empty(),
                        if busy() { icons::LoaderCircle { class: "size-5 animate-spin" } } else if setup { "Konto anlegen" } else { "Anmelden" }
                    }
                    if setup {
                        p { class: "px-1 text-center text-[12.5px] text-muted-foreground", "Mindestens 8 Zeichen. Passwörter werden nur als Argon2-Hash gespeichert." }
                    }
                }
            }
        }
    }
}

/// Noch kein PC gekoppelt.
#[component]
fn Welcome(me: Me) -> Element {
    rsx! {
        div { class: "flex flex-1 flex-col items-center justify-center gap-5 px-8 pb-[env(safe-area-inset-bottom)] text-center",
            img { src: "/icon.png", class: "size-20 rounded-[22px] shadow-lg", alt: "" }
            div {
                h1 { class: "text-[24px] font-bold tracking-tight", "Hallo, {me.username}" }
                p { class: "mt-2 text-[15px] text-muted-foreground", "Verbinde deinen PC: Dann siehst du hier die Chats deiner Coding-Agenten, ihre Limits und kannst unterwegs weiter anweisen." }
            }
            button { class: "btn-primary w-full max-w-xs", onclick: move |_| *PAIRING.write() = true,
                icons::Laptop { class: "size-5" }
                "PC verbinden"
            }
            button { class: "text-[15px] text-link active:opacity-50", onclick: move |_| *SCREEN.write() = Screen::Settings, "Einstellungen" }
        }
    }
}

// ------------------------------------------------------------------ Chats

#[component]
fn Home() -> Element {
    let pcs = PCS().unwrap_or_default();
    let cur = PC();
    let online = ONLINE();
    let pc_name = pcs.iter().find(|p| Some(&p.id) == cur.as_ref()).map(|p| p.name.clone()).unwrap_or_default();
    let mut search = use_signal(|| false);
    let mut query = use_signal(String::new);
    rsx! {
        LargeHeader {
            title: "nrzX",
            subtitle: rsx! {
                div { class: "mt-0.5 flex min-w-0 items-center gap-1.5 text-[13px] text-muted-foreground",
                    span { class: if online { "size-2 shrink-0 rounded-full bg-success" } else { "size-2 shrink-0 rounded-full bg-muted-foreground/50" } }
                    if pcs.len() > 1 {
                        select {
                            class: "min-w-0 truncate bg-transparent outline-none",
                            onchange: move |e| select_pc(Some(e.value())),
                            for p in pcs.iter().cloned() {
                                option { key: "{p.id}", value: "{p.id}", selected: Some(&p.id) == cur.as_ref(), "{p.name}" }
                            }
                        }
                    } else {
                        span { class: "truncate", "{pc_name}" }
                    }
                    if !online {
                        span { class: "shrink-0", "· offline" }
                    }
                }
            },
            actions: rsx! {
                button { class: ICON_BTN, title: "Team", onclick: move |_| *SCREEN.write() = Screen::Team, icons::Users {} }
                button { class: ICON_BTN, title: "Limits", onclick: move |_| *SCREEN.write() = Screen::Limits, icons::Gauge {} }
                button {
                    class: ICON_BTN,
                    title: "Suchen",
                    onclick: move |_| {
                        let v = !search();
                        search.set(v);
                        if !v {
                            query.set(String::new());
                        }
                    },
                    icons::Search {}
                }
                button { class: ICON_BTN, title: "Einstellungen", onclick: move |_| *SCREEN.write() = Screen::Settings, icons::Settings {} }
            },
        }
        if search() {
            div { class: "shrink-0 px-4 pb-3",
                div { class: "flex h-10 items-center gap-2 rounded-xl bg-fill px-3",
                    icons::Search { class: "size-[18px] shrink-0 text-muted-foreground" }
                    input {
                        class: "min-w-0 flex-1 bg-transparent text-[16px] outline-none placeholder:text-muted-foreground",
                        r#type: "search",
                        placeholder: "Chats durchsuchen",
                        autofocus: true,
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                    }
                }
            }
        }
        if !online {
            p { class: "shrink-0 px-5 pb-2 text-[13px] text-muted-foreground", "nrzX am PC ist gerade nicht verbunden – angezeigt wird der letzte Stand." }
        }
        div { class: "sheet relative min-h-0 flex-1 overflow-y-auto overscroll-contain",
            ChatList { query: query() }
            div { class: "h-28" }
        }
        button {
            class: "fab",
            disabled: !online,
            onclick: move |_| *NEW_CHAT.write() = true,
            icons::SquarePen { class: "size-5" }
            "Neuer Chat"
        }
    }
}

fn status_rank(t: &ThreadSummary) -> u8 {
    match t.status {
        ThreadStatus::Working | ThreadStatus::Waiting => 0,
        _ => 1,
    }
}

#[component]
fn ChatList(query: String) -> Element {
    let mut show_settled = use_signal(|| false);
    let q = query.trim().to_lowercase();
    let matches = |t: &ThreadSummary| {
        q.is_empty() || t.title.to_lowercase().contains(&q) || t.project_name.as_deref().unwrap_or("").to_lowercase().contains(&q) || t.branch.to_lowercase().contains(&q)
    };
    let all: Vec<ThreadSummary> = THREADS.read().values().filter(|t| !t.hidden && matches(t)).cloned().collect();
    let mut open: Vec<ThreadSummary> = all.iter().filter(|t| t.status != ThreadStatus::Settled).cloned().collect();
    open.sort_by_key(|t| (status_rank(t), std::cmp::Reverse(t.last_activity_at)));
    let mut settled: Vec<ThreadSummary> = all.iter().filter(|t| t.status == ThreadStatus::Settled).cloned().collect();
    settled.sort_by_key(|t| std::cmp::Reverse(t.last_activity_at));
    let loaded = THREADS_LOADED();
    rsx! {
        if !loaded && all.is_empty() && ONLINE() {
            div { class: "grid place-items-center py-16", Spinner {} }
        } else if all.is_empty() {
            div { class: "px-6 py-16 text-center text-[15px] text-muted-foreground",
                if !q.is_empty() { "Keine Treffer." } else if ONLINE() { "Noch keine Chats." } else { "Warte auf den PC …" }
            }
        }
        div { class: "flex flex-col",
            for t in open {
                ThreadRow { key: "{t.id}", t }
            }
        }
        if !settled.is_empty() {
            button { class: "flex w-full items-center gap-3 px-5 pt-5 pb-2 text-[13px] text-muted-foreground", onclick: move |_| show_settled.toggle(),
                span { "Abgelegt ({settled.len()})" }
                span { class: "h-px flex-1 bg-separator" }
                icons::ChevronDown { class: if show_settled() || !q.is_empty() { "size-4 rotate-180 transition-transform" } else { "size-4 transition-transform" } }
            }
            if show_settled() || !q.is_empty() {
                for t in settled {
                    ThreadRow { key: "{t.id}", t }
                }
            }
        }
    }
}

/// Projekt-Zeichen: Anfangsbuchstabe auf einer Farbe aus dem Namen.
#[component]
fn ProjectMark(name: String) -> Element {
    let hue = name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
    let letter = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "·".into());
    rsx! {
        span { class: "grid size-[18px] shrink-0 place-items-center rounded-[5px] text-[10.5px] font-bold text-white", style: "background: oklch(0.6 0.13 {hue})", "{letter}" }
    }
}

/// Zustand rechts in der Zeile: „Arbeitet 2m“, „Rückfrage“, „Fehler“ oder die Zeit.
fn state_label(t: &ThreadSummary, now: i64) -> (String, &'static str) {
    match t.status {
        ThreadStatus::Working => (format!("Arbeitet {}", short(now - t.run_started_at.unwrap_or(t.last_activity_at))), "text-info"),
        ThreadStatus::Waiting => ("Rückfrage".into(), "text-warning"),
        ThreadStatus::Error if t.resume_at.is_some() => ("Rate-Limit".into(), "text-warning"),
        ThreadStatus::Error => ("Fehler".into(), "text-destructive"),
        ThreadStatus::Done => ("Fertig".into(), "text-success"),
        _ => (short(now - t.last_activity_at), "text-muted-foreground"),
    }
}

fn agent_name(k: AgentKind) -> &'static str {
    k.name()
}

#[component]
fn ThreadRow(t: ThreadSummary) -> Element {
    let now = NOW();
    let id = t.id;
    let project = t.project_name.clone().unwrap_or_else(|| "Ohne Projekt".into());
    let (label, color) = state_label(&t, now);
    rsx! {
        button {
            class: "row flex w-full flex-col gap-0.5 px-5 py-3 text-left",
            onclick: move |_| open_thread(id),
            div { class: "flex items-center gap-2 text-[14px] text-muted-foreground",
                ProjectMark { name: project.clone() }
                span { class: "min-w-0 flex-1 truncate", "{project}" }
                if t.unread && !t.status.is_active() {
                    span { class: "size-2 shrink-0 rounded-full bg-info" }
                }
                span { class: "shrink-0 text-[13.5px] tabular-nums {color}", "{label}" }
            }
            p { class: "truncate text-[17px] leading-snug font-medium", {title_of(&t)} }
            div { class: "flex min-w-0 items-center gap-1.5 text-[13.5px] text-muted-foreground",
                if !t.branch.is_empty() {
                    span { class: "min-w-0 truncate font-mono text-[13px]", "{t.branch}" }
                    span { "·" }
                }
                span { class: "min-w-0 flex-1 truncate", "{agent_name(t.agent)}" }
                if t.status.is_active() {
                    icons::LoaderCircle { class: "size-3.5 shrink-0 animate-spin text-info" }
                }
            }
        }
    }
}

fn title_of(t: &ThreadSummary) -> String {
    if t.title.trim().is_empty() { "Neuer Chat".into() } else { t.title.clone() }
}

// ------------------------------------------------------------------ Chat

#[component]
fn ChatScreen(id: i64) -> Element {
    let thread = THREADS.read().get(&id).cloned();
    let msgs = MESSAGES.read().get(&id).cloned();
    let now = NOW();
    let pc_name = PCS().unwrap_or_default().into_iter().find(|p| Some(&p.id) == PC().as_ref()).map(|p| p.name).unwrap_or_default();
    // Verlauf laden, falls noch nicht da (z. B. nach dem Neuladen)
    use_hook(move || {
        if MESSAGES.peek().get(&id).is_none() {
            spawn_forever_local(load_messages(id));
        }
    });
    // Ans Ende scrollen, wenn Neues kommt
    let count = msgs.as_ref().map(|m| (m.len(), m.last().map(|x| (x.updated_at, x.content.len()))));
    use_effect(use_reactive!(|count| {
        let first = count.is_some_and(|(n, _)| n > 0);
        let _ = count;
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(30).await;
            if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("chat-scroll")) {
                if first || el.scroll_height() - el.scroll_top() - el.client_height() < 500 {
                    el.set_scroll_top(el.scroll_height());
                }
            }
        });
    }));
    let Some(t) = thread else {
        return rsx! {
            div { class: "p-6 pt-[max(env(safe-area-inset-top),1.5rem)]",
                button { class: "text-[17px] text-link", onclick: move |_| *OPEN.write() = None, "‹ Chats" }
            }
        };
    };
    let active = t.status.is_active();
    let stop = move |_| {
        spawn_forever_local(async move {
            if let Some(p) = pc_path(&format!("threads/{id}/stop")) {
                match api::post::<ThreadSummary, _>(&p, &serde_json::json!({})).await {
                    Ok(s) => {
                        THREADS.write().insert(s.id, s);
                        toast("Gestoppt");
                    }
                    Err(e) => toast(e),
                }
            }
        });
    };
    let subtitle = [t.project_name.clone().unwrap_or_else(|| "Ohne Projekt".into()), pc_name].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    rsx! {
        header { class: "glass-bar z-10 flex shrink-0 items-center gap-1 px-1.5 pt-[max(env(safe-area-inset-top),0.5rem)] pb-2",
            button { class: ICON_BTN, title: "Zurück", onclick: move |_| *OPEN.write() = None, icons::ArrowLeft {} }
            div { class: "min-w-0 flex-1 px-1",
                p { class: "truncate text-[17px] leading-tight font-semibold", {title_of(&t)} }
                p { class: "truncate text-[13px] text-muted-foreground", "{subtitle}" }
            }
            if active {
                button { class: "mr-1.5 flex h-8 shrink-0 items-center gap-1.5 rounded-full bg-destructive/15 px-3 text-[14px] font-semibold text-destructive active:bg-destructive/25", onclick: stop,
                    icons::Square { class: "size-3 fill-current" }
                    "Stopp"
                }
            }
        }
        div { id: "chat-scroll", class: "min-h-0 flex-1 overflow-y-auto overscroll-contain bg-background",
            div { class: "flex flex-col gap-4 px-4 pt-4 pb-6",
                match msgs {
                    None => rsx! { div { class: "grid place-items-center py-16", Spinner {} } },
                    Some(list) => rsx! {
                        if list.is_empty() {
                            p { class: "py-16 text-center text-[15px] text-muted-foreground", "Noch keine Nachrichten." }
                        }
                        for b in blocks(&list, active, now) {
                            {b}
                        }
                    },
                }
                if !t.queued.is_empty() {
                    div { class: "rounded-2xl border border-dashed px-3.5 py-2.5 text-[13px] text-muted-foreground",
                        p { class: "mb-1 font-medium", "Wartet auf den Agenten:" }
                        for q in t.queued.iter() {
                            p { key: "{q.id}", class: "truncate", if q.interrupt { "⚡ " } else { "· " } "{q.text}" }
                        }
                    }
                }
            }
        }
        Composer { id, active }
    }
}

/// Nachrichten zu Blöcken: Arbeitsschritte zwischen zwei Antworten als eine Zeile
/// „Arbeitete 2m ›“ (aufklappbar), Benutzer rechts als Blase, Antworten als Text.
fn blocks(list: &[Message], active: bool, now: i64) -> Vec<Element> {
    let mut out = Vec::new();
    let mut tools: Vec<Message> = Vec::new();
    let last_tool_run = list.iter().rposition(|m| m.role == "tool");
    let last_assistant = list.iter().rposition(|m| m.role == "assistant");
    let flush = |tools: &mut Vec<Message>, out: &mut Vec<Element>, live: bool| {
        if !tools.is_empty() {
            let steps = std::mem::take(tools);
            out.push(rsx! { Steps { key: "s{steps[0].id}", steps, live, now } });
        }
    };
    for (i, m) in list.iter().enumerate() {
        if m.role == "tool" {
            tools.push(m.clone());
            continue;
        }
        flush(&mut tools, &mut out, false);
        let m = m.clone();
        out.push(match m.role.as_str() {
            "user" => rsx! { UserBubble { key: "{m.id}", m } },
            "assistant" => {
                let last = Some(i) == last_assistant;
                rsx! { AssistantText { key: "{m.id}", m, meta: last && !active } }
            }
            "error" => rsx! { div { key: "{m.id}", class: "rounded-2xl bg-destructive/12 px-3.5 py-2.5 text-[14px] whitespace-pre-wrap text-destructive", "{m.content}" } },
            "question" => rsx! {
                div { key: "{m.id}", class: "rounded-2xl bg-info/12 px-3.5 py-3 text-[15px]",
                    p { class: "mb-1 flex items-center gap-1.5 text-[14px] font-semibold text-info", icons::MessageCircleQuestion { class: "size-4" } "Rückfrage" }
                    p { class: "whitespace-pre-wrap", {question_text(&m)} }
                    p { class: "mt-1.5 text-[13px] text-muted-foreground", "Antwort unten schreiben und senden." }
                }
            },
            "subagents" => rsx! { p { key: "{m.id}", class: "text-[13px] text-muted-foreground", "Subagenten gestartet" } },
            _ => {
                let kind = m.data.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                let text = if kind == "mcp" { format!("{} MCP-Server nicht verbunden", m.content.split(',').count()) } else { m.content.clone() };
                rsx! { p { key: "{m.id}", class: "text-center text-[12px] text-muted-foreground/70", "{text}" } }
            }
        });
    }
    let live = active && last_tool_run.is_some_and(|i| i + 1 == list.len());
    flush(&mut tools, &mut out, live);
    if active && !live {
        out.push(rsx! {
            div { key: "{WORKING_KEY}", class: "flex items-center gap-2 text-[14px] text-muted-foreground",
                Spinner {}
                "Arbeitet …"
            }
        });
    }
    out
}

const WORKING_KEY: &str = "working";

fn question_text(m: &Message) -> String {
    let q = m.data.get("questions").and_then(|q| q.as_array()).and_then(|a| a.first()).and_then(|q| q.get("question")).and_then(|q| q.as_str());
    q.map(str::to_string).unwrap_or_else(|| m.content.clone())
}

#[component]
fn UserBubble(m: Message) -> Element {
    let interjection = m.data.get("interjection").and_then(|v| v.as_bool()).unwrap_or(false);
    let text = m.content.clone();
    rsx! {
        div { class: "flex flex-col items-end gap-1 pl-10",
            if interjection {
                span { class: "text-[12px] text-muted-foreground", "⚡ Zwischenruf" }
            }
            div { class: "rounded-[20px] bg-bubble px-4 py-2.5 text-[16px] leading-snug whitespace-pre-wrap break-words", "{m.content}" }
            div { class: "flex items-center gap-1 pr-1 text-[12.5px] text-muted-foreground",
                span { class: "tabular-nums", {clock(m.created_at)} }
                button { class: "grid size-7 place-items-center rounded-full active:bg-foreground/10", title: "Kopieren", onclick: move |_| copy(&text), icons::Copy { class: "size-3.5" } }
            }
        }
    }
}

#[component]
fn AssistantText(m: Message, meta: bool) -> Element {
    let text = m.content.clone();
    rsx! {
        div { class: "flex flex-col gap-1",
            div { class: "md", dangerous_inner_html: "{m.html}" }
            if meta {
                div { class: "flex items-center gap-1 text-[12.5px] text-muted-foreground",
                    button { class: "-ml-1.5 grid size-7 place-items-center rounded-full active:bg-foreground/10", title: "Kopieren", onclick: move |_| copy(&text), icons::Copy { class: "size-3.5" } }
                    span { class: "tabular-nums", {clock(m.updated_at)} }
                }
            }
        }
    }
}

/// „Arbeitete 2m ›“ – klappt die einzelnen Schritte auf.
#[component]
fn Steps(steps: Vec<Message>, live: bool, now: i64) -> Element {
    let mut open = use_signal(|| false);
    let start = steps.first().map(|m| m.created_at).unwrap_or(now);
    let end = if live { now } else { steps.iter().map(|m| m.updated_at).max().unwrap_or(start) };
    let took = short(end - start);
    let failed = steps.iter().filter(|m| m.data.get("status").and_then(|s| s.as_str()) == Some("error")).count();
    // „Nachgedacht“ nur, wenn es wirklich nur Denkschritte waren
    let only_thinking = steps.iter().all(|m| m.data.get("kind").and_then(|k| k.as_str()) == Some("think"));
    let label = if live { format!("Arbeitet {took}") } else if only_thinking { "Nachgedacht".into() } else { format!("Arbeitete {}", short((end - start).max(1))) };
    rsx! {
        div { class: "flex flex-col",
            button { class: "flex w-full items-center gap-1.5 border-b pb-2 text-[14.5px] text-muted-foreground active:text-foreground", onclick: move |_| open.toggle(),
                if live { Spinner {} }
                span { "{label}" }
                if failed > 0 {
                    span { class: "text-destructive", "· {failed} fehlgeschlagen" }
                }
                icons::ChevronRight { class: if open() { "size-4 rotate-90 transition-transform" } else { "size-4 transition-transform" } }
            }
            if open() {
                div { class: "mt-2 flex flex-col gap-1.5",
                    for m in steps.iter() {
                        StepRow { key: "{m.id}", m: m.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn StepRow(m: Message) -> Element {
    let mut open = use_signal(|| false);
    let get = |k: &str| m.data.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (kind, target, status, output) = (get("kind"), get("target"), get("status"), get("output"));
    let label = if target.is_empty() { get("name") } else { target };
    let prefix = match kind.as_str() {
        "command" => "$",
        "edit" | "write" => "✎",
        "read" => "↗",
        "search" | "web" => "⌕",
        "think" => "…",
        _ => "·",
    };
    rsx! {
        div {
            button { class: "flex w-full min-w-0 items-center gap-2 text-left text-[13px] text-muted-foreground", onclick: move |_| open.toggle(),
                span { class: "w-3 shrink-0 font-mono", "{prefix}" }
                span { class: if status == "error" { "min-w-0 flex-1 truncate font-mono text-destructive" } else { "min-w-0 flex-1 truncate font-mono" }, "{label}" }
            }
            if open() && !output.trim().is_empty() {
                pre { class: "mt-1 max-h-60 overflow-auto rounded-xl bg-surface px-3 py-2 font-mono text-[11.5px] whitespace-pre-wrap text-muted-foreground", "{output}" }
            }
        }
    }
}

/// Eingabe unten als Pille: senden; arbeitet der Agent, in die Warteschlange oder als Zwischenruf.
#[component]
fn Composer(id: i64, active: bool) -> Element {
    let key = format!("nrzm-draft-{id}");
    let mut text = use_signal({
        let k = key.clone();
        move || storage_get(&k).unwrap_or_default()
    });
    let mut sending = use_signal(|| false);
    use_effect(use_reactive!(|key| {
        let t = text();
        if t.is_empty() { storage_remove(&key) } else { storage_set(&key, &t) }
    }));
    let online = ONLINE();
    let mut send = move |interrupt: bool| {
        let t = text().trim().to_string();
        if t.is_empty() || sending() {
            return;
        }
        sending.set(true);
        spawn_forever_local(async move {
            if let Some(p) = pc_path(&format!("threads/{id}/messages")) {
                match api::post::<ThreadSummary, _>(&p, &PostMessage { text: t, attachments: Vec::new(), interrupt }).await {
                    Ok(s) => {
                        THREADS.write().insert(s.id, s);
                        text.set(String::new());
                        if active {
                            toast(if interrupt { "Zwischenruf – geht im nächsten passenden Moment raus" } else { "In der Warteschlange" });
                        }
                        load_messages(id).await;
                    }
                    Err(e) => toast(e),
                }
            }
            sending.set(false);
        });
    };
    let empty = text().trim().is_empty();
    let rows = text().lines().count().clamp(1, 6);
    rsx! {
        div { class: "glass-bar shrink-0 px-3 pt-2 pb-[max(env(safe-area-inset-bottom),0.625rem)]",
            div { class: "flex items-end gap-2 rounded-[24px] border bg-card py-1.5 pr-1.5 pl-4 shadow-sm",
                textarea {
                    class: "max-h-40 min-h-9 flex-1 resize-none bg-transparent py-[7px] text-[16px] leading-snug outline-none placeholder:text-muted-foreground disabled:opacity-50",
                    rows: "{rows}",
                    enterkeyhint: "send",
                    placeholder: if online { if active { "Weiter anweisen …" } else { "Den Agenten fragen …" } } else { "PC nicht verbunden" },
                    disabled: !online,
                    value: "{text}",
                    oninput: move |e| text.set(e.value()),
                }
                if active && !empty {
                    button { class: "grid size-9 shrink-0 place-items-center rounded-full bg-warning/15 text-warning disabled:opacity-40", title: "Zwischenrufen", disabled: sending() || !online, onclick: move |_| send(true),
                        icons::Zap { class: "size-[18px]" }
                    }
                }
                button { class: "grid size-9 shrink-0 place-items-center rounded-full bg-link text-white transition-opacity disabled:opacity-30", title: if active { "In die Warteschlange" } else { "Senden" }, disabled: empty || sending() || !online, onclick: move |_| send(false),
                    if sending() { icons::LoaderCircle { class: "size-[18px] animate-spin" } } else if active { icons::ListPlus { class: "size-[18px]" } } else { icons::ArrowUp { class: "size-[18px]" } }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Team

/// Team: wer arbeitet mit am Repository, woran sitzen die Agenten der anderen, und der gemeinsame
/// Nachrichtenverlauf – mit einem Feld, um vom Handy aus ans Team oder an einen Agenten zu
/// schreiben. Eingeschaltet wird ein Team am PC (rechte Spalte, Reiter „Team“).
#[component]
fn Team() -> Element {
    let mut teams = use_signal(|| None::<Result<Vec<TeamView>, String>>);
    let mut pick = use_signal(|| None::<i64>);
    let mut text = use_signal(String::new);
    let mut to = use_signal(|| "all".to_string());
    let mut sending = use_signal(|| false);
    use_effect(move || {
        let _ = (TEAM_REV(), PC(), ONLINE());
        spawn(async move {
            if let Some(p) = pc_path("projects/teams") {
                teams.set(Some(api::get::<Vec<TeamView>>(&p).await));
            }
        });
    });
    let online = ONLINE();
    let list = match teams() {
        Some(Ok(l)) => l,
        other => {
            return rsx! {
                LargeHeader { title: "Team", back: true }
                div { class: "min-h-0 flex-1 px-5",
                    match other {
                        Some(Err(e)) => rsx! { p { class: "text-[15px] text-muted-foreground", "{e}" } },
                        _ => rsx! { div { class: "grid place-items-center py-16", Spinner {} } },
                    }
                }
            };
        }
    };
    let cur = pick().and_then(|id| list.iter().find(|t| t.project_id == id)).or(list.first()).cloned();
    let Some(v) = cur else {
        return rsx! {
            LargeHeader { title: "Team", back: true }
            div { class: "min-h-0 flex-1 px-5",
                p { class: "text-[15px] leading-relaxed text-muted-foreground",
                    "Noch kein Team. Am PC ein Projekt öffnen und in der rechten Spalte unter „Team“ einschalten – dann arbeiten mehrere Leute mit ihrem eigenen nrzX am selben Repository, und ihre Agenten sprechen miteinander."
                }
            }
        };
    };
    let me = v.me.clone();
    let project_id = v.project_id;
    let mut targets: Vec<(String, String)> = vec![("all".to_string(), "Alle".to_string())];
    for m in v.members.iter().filter(|m| m.name.to_lowercase() != me) {
        let key = m.name.to_lowercase();
        targets.push((key.clone(), m.name.clone()));
        for a in &m.agents {
            targets.push((team_address(&key, Some(a.id)), format!("{}#{} · {}", m.name, a.id, a.title)));
        }
    }
    let to_now = if targets.iter().any(|(k, _)| *k == to()) { to() } else { "all".to_string() };
    let mut messages = v.messages.clone();
    messages.reverse();
    let now = NOW();
    let send = {
        let to_now = to_now.clone();
        move |_| {
            let t = text().trim().to_string();
            if t.is_empty() || sending() {
                return;
            }
            let to_now = to_now.clone();
            sending.set(true);
            spawn_forever_local(async move {
                if let Some(p) = pc_path(&format!("projects/{project_id}/team/messages")) {
                    match api::post::<TeamView, _>(&p, &TeamPost { to: to_now, text: t, reply_to: None }).await {
                        Ok(_) => {
                            text.set(String::new());
                            *TEAM_REV.write() += 1;
                        }
                        Err(e) => toast(e),
                    }
                }
                sending.set(false);
            });
        }
    };
    let rows = text().lines().count().clamp(1, 5);
    rsx! {
        LargeHeader {
            title: "Team",
            back: true,
            subtitle: rsx! {
                div { class: "mt-0.5 flex min-w-0 items-center gap-1.5 text-[13px] text-muted-foreground",
                    if list.len() > 1 {
                        select {
                            class: "min-w-0 truncate bg-transparent outline-none",
                            onchange: move |e| pick.set(e.value().parse().ok()),
                            for t in list.iter() {
                                option { key: "{t.project_id}", value: "{t.project_id}", selected: t.project_id == project_id, "{t.project_name}" }
                            }
                        }
                    } else {
                        span { class: "truncate", "{v.project_name}" }
                    }
                }
            },
            actions: rsx! {
                button { class: ICON_BTN, title: "Aktualisieren", onclick: move |_| *TEAM_REV.write() += 1, icons::RefreshCw {} }
            },
        }
        div { class: "min-h-0 flex-1 overflow-y-auto overscroll-contain px-4 pb-4",
            if !v.error.is_empty() {
                p { class: "mb-3 rounded-2xl bg-destructive/10 px-4 py-2.5 text-[14px] text-destructive", "{v.error}" }
            }
            div { class: "flex flex-col gap-3",
                for m in v.members.iter() {
                    div { key: "{m.name}", class: "rounded-2xl bg-card px-4 py-3",
                        div { class: "flex items-center gap-2",
                            span { class: "min-w-0 truncate text-[16px] font-semibold", "{m.name}" }
                            if m.name.to_lowercase() == me {
                                span { class: "shrink-0 rounded-full bg-fill px-2 text-[12px] text-muted-foreground", "du" }
                            }
                            span { class: "flex-1" }
                            span { class: "shrink-0 text-[12px] text-muted-foreground", "vor {short((now - m.seen_at).max(0))}" }
                        }
                        if m.agents.is_empty() {
                            p { class: "mt-1 text-[14px] text-muted-foreground", "Gerade kein Agent aktiv." }
                        }
                        for a in m.agents.iter() {
                            div { key: "{a.id}", class: "mt-2 flex min-w-0 items-center gap-2.5",
                                span {
                                    class: match a.status.as_str() {
                                        "working" => "size-2.5 shrink-0 rounded-full bg-success",
                                        "waiting" => "size-2.5 shrink-0 rounded-full bg-warning",
                                        _ => "size-2.5 shrink-0 rounded-full bg-muted-foreground/40",
                                    },
                                }
                                div { class: "min-w-0 flex-1",
                                    p { class: "truncate text-[15px]", if a.title.is_empty() { "Neuer Chat" } else { "{a.title}" } }
                                    p { class: "truncate text-[12.5px] text-muted-foreground", "#{a.id} · {a.agent} · {a.branch}" }
                                }
                            }
                        }
                    }
                }
            }
            p { class: "mb-2 mt-5 px-1 text-[13px] font-semibold uppercase tracking-wide text-muted-foreground", "Nachrichten" }
            if messages.is_empty() {
                p { class: "px-1 text-[15px] text-muted-foreground", "Noch keine Nachrichten." }
            }
            div { class: "flex flex-col gap-2",
                for m in messages {
                    div { key: "{m.id}", class: "rounded-2xl bg-card px-4 py-3",
                        div { class: "flex min-w-0 items-center gap-1.5 text-[12.5px] text-muted-foreground",
                            span { class: "min-w-0 truncate font-semibold text-foreground", "{m.sender()}" }
                            span { class: "shrink-0", "→" }
                            span { class: "min-w-0 truncate", if m.to.is_empty() { "alle" } else { "{m.recipient()}" } }
                            span { class: "flex-1" }
                            span { class: "shrink-0", "{clock(m.at)}" }
                        }
                        if !m.from_title.is_empty() {
                            p { class: "truncate text-[12.5px] text-muted-foreground", "{m.from_title}" }
                        }
                        p { class: "mt-1 whitespace-pre-wrap break-words text-[15px] leading-snug", "{m.text}" }
                    }
                }
            }
        }
        div { class: "glass-bar shrink-0 px-3 pt-2 pb-[max(env(safe-area-inset-bottom),0.625rem)]",
            div { class: "mb-1.5 flex items-center gap-2 px-2 text-[13px] text-muted-foreground",
                "An"
                select {
                    class: "min-w-0 flex-1 truncate bg-transparent text-foreground outline-none",
                    onchange: move |e| to.set(e.value()),
                    for (key , label) in targets {
                        option { key: "{key}", value: "{key}", selected: key == to_now, "{label}" }
                    }
                }
            }
            div { class: "flex items-end gap-2 rounded-[24px] border bg-card py-1.5 pr-1.5 pl-4 shadow-sm",
                textarea {
                    class: "max-h-40 min-h-9 flex-1 resize-none bg-transparent py-[7px] text-[16px] leading-snug outline-none placeholder:text-muted-foreground disabled:opacity-50",
                    rows: "{rows}",
                    enterkeyhint: "send",
                    placeholder: if online { "Ans Team schreiben …" } else { "PC nicht verbunden" },
                    disabled: !online,
                    value: "{text}",
                    oninput: move |e| text.set(e.value()),
                }
                button {
                    class: "grid size-9 shrink-0 place-items-center rounded-full bg-link text-white transition-opacity disabled:opacity-30",
                    title: "Senden",
                    disabled: text().trim().is_empty() || sending() || !online,
                    onclick: send,
                    if sending() {
                        icons::LoaderCircle { class: "size-[18px] animate-spin" }
                    } else {
                        icons::ArrowUp { class: "size-[18px]" }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Neuer Chat

#[component]
fn NewChatSheet() -> Element {
    let mut text = use_signal(|| storage_get("nrzm-new-draft").unwrap_or_default());
    let mut project = use_signal(|| storage_get("nrzm-new-project").and_then(|v| v.parse::<i64>().ok()));
    let mut busy = use_signal(|| false);
    let data = use_resource(|| async move {
        let projects = match pc_path("projects") {
            Some(p) => api::get::<Vec<ProjectSummary>>(&p).await.unwrap_or_default(),
            None => Vec::new(),
        };
        let agents = match pc_path("agents") {
            Some(p) => api::get::<Vec<AgentInfo>>(&p).await.unwrap_or_default(),
            None => Vec::new(),
        };
        (projects, agents)
    });
    use_effect(move || {
        let t = text();
        if t.is_empty() { storage_remove("nrzm-new-draft") } else { storage_set("nrzm-new-draft", &t) }
    });
    let (projects, agents) = data.read().clone().unwrap_or_default();
    let mut projects = projects;
    projects.sort_by_key(|p| std::cmp::Reverse(p.last_activity_at));
    // Agent: zuletzt benutzter, sonst der erste mit Konto
    let remembered = storage_get("nrzm-new-agent");
    let agent = agents
        .iter()
        .filter(|a| !a.accounts.is_empty())
        .find(|a| Some(a.kind.key()) == remembered.as_deref())
        .or_else(|| agents.iter().find(|a| !a.accounts.is_empty()))
        .map(|a| a.kind);
    let start = move |_| {
        let prompt = text().trim().to_string();
        let Some(kind) = agent else {
            toast("Am PC ist noch kein Agent mit Konto eingerichtet.");
            return;
        };
        if prompt.is_empty() || busy() {
            return;
        }
        busy.set(true);
        let pid = project();
        spawn_forever_local(async move {
            let req = NewThread {
                project_id: pid,
                prompt,
                agent: kind,
                account_id: None,
                model: String::new(),
                effort: String::new(),
                use_worktree: false,
                read_only: false,
                freestyle: false,
                title: None,
                attachments: Vec::new(),
                tasks: Vec::new(),
                research: false,
            };
            if let Some(p) = pc_path("threads") {
                match api::post::<ThreadSummary, _>(&p, &req).await {
                    Ok(t) => {
                        storage_remove("nrzm-new-draft");
                        storage_set("nrzm-new-agent", kind.key());
                        match pid {
                            Some(i) => storage_set("nrzm-new-project", &i.to_string()),
                            None => storage_remove("nrzm-new-project"),
                        }
                        let id = t.id;
                        THREADS.write().insert(id, t);
                        *NEW_CHAT.write() = false;
                        open_thread(id);
                    }
                    Err(e) => toast(e),
                }
            }
            busy.set(false);
        });
    };
    rsx! {
        Sheet { title: "Neuer Chat", onclose: move |_| *NEW_CHAT.write() = false,
            div { class: "flex flex-col gap-4",
                div {
                    p { class: "section-label", "Projekt" }
                    div { class: "-mx-1 flex gap-2 overflow-x-auto px-1 pb-1",
                        Chip { on: project().is_none(), onclick: move |_| project.set(None), "Ohne Projekt" }
                        for p in projects.iter().cloned() {
                            Chip { key: "{p.id}", on: project() == Some(p.id), onclick: move |_| project.set(Some(p.id)),
                                ProjectMark { name: p.name.clone() }
                                "{p.name}"
                            }
                        }
                    }
                }
                textarea {
                    class: "min-h-36 w-full resize-none rounded-2xl bg-fill px-4 py-3 text-[16px] leading-snug outline-none placeholder:text-muted-foreground",
                    placeholder: "Woran soll der Agent arbeiten?",
                    autofocus: true,
                    value: "{text}",
                    oninput: move |e| text.set(e.value()),
                }
                p { class: "-mt-2 px-1 text-[12.5px] text-muted-foreground",
                    match agent {
                        Some(k) => rsx! { "Mit {agent_name(k)} – Standardkonto, Modell und Stufe wie am PC." },
                        None if data.read().is_none() => rsx! { "Lädt …" },
                        None => rsx! { "Am PC ist noch kein Agent mit Konto eingerichtet." },
                    }
                }
                button { class: "btn-primary", disabled: text().trim().is_empty() || busy() || agent.is_none(), onclick: start,
                    if busy() { icons::LoaderCircle { class: "size-5 animate-spin" } } else { icons::ArrowUp { class: "size-5" } "Starten" }
                }
            }
        }
    }
}

#[component]
fn Chip(on: bool, onclick: EventHandler<MouseEvent>, children: Element) -> Element {
    rsx! {
        button {
            class: if on { "flex h-9 shrink-0 items-center gap-1.5 rounded-full bg-foreground px-3.5 text-[14px] font-medium text-background" } else { "flex h-9 shrink-0 items-center gap-1.5 rounded-full bg-fill px-3.5 text-[14px]" },
            onclick: move |e| onclick.call(e),
            {children}
        }
    }
}

/// Blatt von unten (wie in iOS), mit Griff und Titelzeile.
#[component]
fn Sheet(title: String, onclose: EventHandler<()>, children: Element) -> Element {
    rsx! {
        div { class: "fixed inset-0 z-40 bg-black/40 animate-fade", onclick: move |_| onclose.call(()) }
        div { class: "fixed inset-x-0 bottom-0 z-50 flex max-h-[92dvh] flex-col rounded-t-[28px] bg-elevated shadow-2xl animate-sheet",
            div { class: "mx-auto mt-2 h-[5px] w-9 shrink-0 rounded-full bg-muted-foreground/35" }
            div { class: "flex shrink-0 items-center px-5 pt-3 pb-2",
                h2 { class: "flex-1 text-[20px] font-bold tracking-tight", "{title}" }
                button { class: "grid size-8 place-items-center rounded-full bg-fill text-muted-foreground active:opacity-60", title: "Schliessen", onclick: move |_| onclose.call(()),
                    icons::X { class: "size-[18px]" }
                }
            }
            div { class: "min-h-0 overflow-y-auto px-5 pt-2 pb-[max(env(safe-area-inset-bottom),1.25rem)]", {children} }
        }
    }
}

// ------------------------------------------------------------------ Limits

fn used_now(l: &UsageLimit, now: i64) -> f64 {
    if l.resets_at.is_some_and(|r| r <= now) { 0.0 } else { l.used_percent.clamp(0.0, 100.0) }
}

#[component]
fn Limits() -> Element {
    let mut usage = use_signal(|| None::<Result<UsageOverview, String>>);
    let mut tick = use_signal(|| 0u32);
    use_effect(move || {
        let _ = (tick(), PC(), ONLINE());
        spawn(async move {
            if let Some(p) = pc_path("usage?refresh=true") {
                usage.set(Some(api::get::<UsageOverview>(&p).await));
            }
        });
    });
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(60_000).await;
            tick += 1;
        }
    });
    let now = NOW();
    rsx! {
        LargeHeader {
            title: "Limits",
            back: true,
            actions: rsx! {
                button { class: ICON_BTN, title: "Aktualisieren", onclick: move |_| tick += 1, icons::RefreshCw {} }
            },
        }
        div { class: "min-h-0 flex-1 overflow-y-auto px-4 pb-[max(env(safe-area-inset-bottom),1rem)]",
            div { class: "flex flex-col gap-3",
                match usage() {
                    None => rsx! { div { class: "grid place-items-center py-16", Spinner {} } },
                    Some(Err(e)) => rsx! { p { class: "px-1 text-[15px] text-muted-foreground", "{e}" } },
                    Some(Ok(u)) => {
                        let accounts: Vec<AccountUsage> = u.accounts.into_iter().filter(|a| !a.limits.is_empty()).collect();
                        rsx! {
                            if accounts.is_empty() {
                                p { class: "py-10 text-center text-[15px] text-muted-foreground", "Noch keine Limits gemeldet – Claude, Codex und Grok melden sie nach der Anmeldung." }
                            }
                            for weekly in [false, true] {
                                LimitCard { key: "{weekly}", weekly, accounts: accounts.clone(), now }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn LimitCard(weekly: bool, accounts: Vec<AccountUsage>, now: i64) -> Element {
    let keys: &[&str] = if weekly { &["seven_day", "secondary", "grok"] } else { &["five_hour", "primary"] };
    let rows: Vec<(String, UsageLimit)> = accounts
        .iter()
        .flat_map(|a| a.limits.iter().filter(|l| keys.contains(&l.key.as_str())).map(move |l| (if a.label.is_empty() { a.kind.name().to_string() } else { a.label.clone() }, l.clone())))
        .collect();
    if rows.is_empty() {
        return rsx! {};
    }
    let span = if weekly { 7 * 86_400 } else { 5 * 3600 };
    let left = rows.iter().map(|(_, l)| 100.0 - used_now(l, now)).sum::<f64>() / rows.len() as f64;
    rsx! {
        div { class: "rounded-[20px] bg-card p-4 shadow-sm",
            div { class: "mb-3 flex items-baseline gap-2",
                span { class: "flex-1 text-[15px] font-semibold text-muted-foreground", if weekly { "Woche" } else { "Session" } }
                span { class: "text-3xl font-bold tabular-nums tracking-tight", "{left:.0}%" }
                span { class: "text-[14px] text-muted-foreground", "übrig" }
            }
            div { class: "grid grid-cols-2 gap-x-4 gap-y-3",
                for (i , (name , l)) in rows.into_iter().enumerate() {
                    {
                        let left = 100.0 - used_now(&l, now);
                        let reset = l.resets_at.filter(|r| *r > now);
                        let time = reset.map(|r| ((r - now) as f64 / span as f64 * 100.0).clamp(0.0, 100.0));
                        rsx! {
                            div { key: "{i}", class: "min-w-0",
                                div { class: "flex items-baseline gap-1.5 text-[13.5px]",
                                    span { class: "min-w-0 truncate font-semibold", "{name}" }
                                    span { class: "shrink-0 font-semibold tabular-nums", "{left:.0}%" }
                                }
                                if let Some(r) = reset {
                                    p { class: "text-[12px] tabular-nums text-muted-foreground", "↻ {reset_in(r - now)}" }
                                }
                                div {
                                    class: "relative mt-1 h-3.5 overflow-hidden rounded-full",
                                    style: "background: repeating-linear-gradient(135deg, color-mix(in oklab, var(--muted-foreground) 26%, transparent) 0 2px, color-mix(in oklab, var(--muted-foreground) 9%, transparent) 2px 5px)",
                                    div { class: "h-full rounded-full bg-muted-foreground/45", style: "width: {left}%" }
                                    if let Some(t) = time {
                                        div { class: "absolute inset-y-0 w-[3px] -translate-x-1/2 rounded-full bg-foreground/85", style: "left: clamp(2px, {t}%, calc(100% - 2px))" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Einstellungen

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserInfo {
    id: String,
    username: String,
    admin: bool,
    created_at: i64,
}

#[component]
fn Settings(me: Me) -> Element {
    let pcs = PCS().unwrap_or_default();
    let cur = PC();
    let now = NOW();
    let mut pw_open = use_signal(|| false);
    let mut users_tick = use_signal(|| 0u32);
    let users = use_resource(move || async move {
        let _ = users_tick();
        api::get::<Vec<UserInfo>>("/api/users").await.ok()
    });
    let mut add_open = use_signal(|| false);
    let logout = move |_| {
        spawn_forever_local(async {
            let _ = api::post_empty("/api/auth/logout", &serde_json::json!({})).await;
            signed_out();
            *AUTH.write() = Auth::Login;
            *SCREEN.write() = Screen::Chats;
        });
    };
    rsx! {
        LargeHeader { title: "Einstellungen", back: true }
        div { class: "min-h-0 flex-1 overflow-y-auto px-4 pb-[max(env(safe-area-inset-bottom),1.5rem)]",
            p { class: "section-label", "PCs" }
            div { class: "group-list",
                for p in pcs.iter().cloned() {
                    div { key: "{p.id}", class: "list-row",
                        button { class: "flex min-w-0 flex-1 items-center gap-3 text-left", onclick: { let id = p.id.clone(); move |_| { select_pc(Some(id.clone())); *SCREEN.write() = Screen::Chats; } },
                            icons::Laptop { class: "size-5 shrink-0 text-muted-foreground" }
                            div { class: "min-w-0 flex-1",
                                p { class: "truncate text-[16px]", "{p.name}" }
                                p { class: "text-[13px] text-muted-foreground",
                                    if p.online { "verbunden" } else { "zuletzt vor {short(now - p.last_seen)}" }
                                }
                            }
                            if Some(&p.id) == cur.as_ref() {
                                icons::Check { class: "size-5 shrink-0 text-link" }
                            }
                        }
                        button {
                            class: "grid size-8 shrink-0 place-items-center rounded-full text-destructive active:bg-destructive/10",
                            title: "Entfernen",
                            onclick: {
                                let (id, name) = (p.id.clone(), p.name.clone());
                                move |_| {
                                    let ok = web_sys::window().and_then(|w| w.confirm_with_message(&format!("„{name}“ entfernen? nrzX am PC muss danach neu gekoppelt werden.")).ok()).unwrap_or(false);
                                    if ok {
                                        let id = id.clone();
                                        spawn_forever_local(async move {
                                            match api::delete(&format!("/api/pcs/{id}")).await {
                                                Ok(()) => load_pcs().await,
                                                Err(e) => toast(e),
                                            }
                                        });
                                    }
                                }
                            },
                            icons::Trash2 { class: "size-[18px]" }
                        }
                    }
                }
                button { class: "list-row text-link", onclick: move |_| *PAIRING.write() = true,
                    icons::Plus { class: "size-5" }
                    span { class: "text-[16px]", "PC verbinden" }
                }
            }

            p { class: "section-label", "Konto" }
            div { class: "group-list",
                div { class: "list-row",
                    icons::CircleUser { class: "size-5 text-muted-foreground" }
                    span { class: "flex-1 text-[16px]", "{me.username}" }
                    if me.admin {
                        span { class: "text-[13px] text-muted-foreground", "Administrator" }
                    }
                }
                button { class: "list-row", onclick: move |_| pw_open.set(true),
                    icons::KeyRound { class: "size-5 text-muted-foreground" }
                    span { class: "flex-1 text-left text-[16px]", "Passwort ändern" }
                    icons::ChevronRight { class: "size-5 text-muted-foreground/60" }
                }
                button { class: "list-row text-destructive", onclick: logout,
                    icons::LogOut { class: "size-5" }
                    span { class: "text-[16px]", "Abmelden" }
                }
            }

            if me.admin {
                p { class: "section-label", "Konten" }
                div { class: "group-list",
                    for u in users.read().clone().flatten().unwrap_or_default() {
                        div { key: "{u.id}", class: "list-row",
                            icons::User { class: "size-5 text-muted-foreground" }
                            div { class: "min-w-0 flex-1",
                                p { class: "truncate text-[16px]", "{u.username}" }
                                p { class: "text-[13px] text-muted-foreground", if u.admin { "Administrator" } else { "Konto" } " · seit {short(now - u.created_at)}" }
                            }
                            if u.id != me.id {
                                button {
                                    class: "grid size-8 shrink-0 place-items-center rounded-full text-destructive active:bg-destructive/10",
                                    title: "Konto löschen",
                                    onclick: {
                                        let (id, name) = (u.id.clone(), u.username.clone());
                                        move |_| {
                                            let ok = web_sys::window().and_then(|w| w.confirm_with_message(&format!("Konto „{name}“ löschen? Seine PCs werden getrennt.")).ok()).unwrap_or(false);
                                            if ok {
                                                let id = id.clone();
                                                spawn_forever_local(async move {
                                                    match api::delete(&format!("/api/users/{id}")).await {
                                                        Ok(()) => {
                                                            users_tick += 1;
                                                            toast("Konto gelöscht");
                                                        }
                                                        Err(e) => toast(e),
                                                    }
                                                });
                                            }
                                        }
                                    },
                                    icons::Trash2 { class: "size-[18px]" }
                                }
                            }
                        }
                    }
                    button { class: "list-row text-link", onclick: move |_| add_open.set(true),
                        icons::UserPlus { class: "size-5" }
                        span { class: "text-[16px]", "Konto hinzufügen" }
                    }
                }
                p { class: "mt-1.5 px-4 text-[12.5px] text-muted-foreground", "Jedes Konto koppelt seine eigenen PCs und sieht nur deren Chats – auch Administratoren sehen keine fremden Chats." }
            }
            p { class: "section-label", "App" }
            Updates { admin: me.admin }
            p { class: "mt-8 text-center text-[12.5px] text-muted-foreground", "nrzX Mobile {UI_VERSION}" }
        }
        if pw_open() {
            PasswordSheet { onclose: move |_| pw_open.set(false) }
        }
        if add_open() {
            AddUserSheet { onclose: move |_| add_open.set(false), ondone: move |_| { add_open.set(false); users_tick += 1; } }
        }
    }
}

#[component]
fn PasswordSheet(onclose: EventHandler<()>) -> Element {
    let mut current = use_signal(String::new);
    let mut new = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn_forever_local(async move {
            let body = serde_json::json!({ "current": *current.peek(), "new": *new.peek() });
            match api::post_empty("/api/auth/password", &body).await {
                Ok(()) => {
                    toast("Passwort geändert – andere Geräte sind abgemeldet");
                    onclose.call(());
                }
                Err(e) => toast(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        Sheet { title: "Passwort ändern", onclose: move |_| onclose.call(()),
            form { class: "flex flex-col gap-4", onsubmit: submit,
                div { class: "group-list",
                    input { class: "field", r#type: "password", autocomplete: "current-password", placeholder: "Aktuelles Passwort", value: "{current}", oninput: move |e| current.set(e.value()) }
                    input { class: "field", r#type: "password", autocomplete: "new-password", placeholder: "Neues Passwort (min. 8 Zeichen)", value: "{new}", oninput: move |e| new.set(e.value()) }
                }
                button { class: "btn-primary", r#type: "submit", disabled: busy() || current().is_empty() || new().chars().count() < 8, "Ändern" }
            }
        }
    }
}

#[component]
fn AddUserSheet(onclose: EventHandler<()>, ondone: EventHandler<()>) -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut admin = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn_forever_local(async move {
            let body = serde_json::json!({ "username": username.peek().trim(), "password": *password.peek(), "admin": admin() });
            match api::post::<serde_json::Value, _>("/api/users", &body).await {
                Ok(_) => {
                    toast("Konto angelegt");
                    ondone.call(());
                }
                Err(e) => toast(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        Sheet { title: "Konto hinzufügen", onclose: move |_| onclose.call(()),
            form { class: "flex flex-col gap-4", onsubmit: submit,
                div { class: "group-list",
                    input { class: "field", r#type: "text", autocapitalize: "none", autocorrect: "off", spellcheck: "false", autocomplete: "off", placeholder: "Benutzername", value: "{username}", oninput: move |e| username.set(e.value()) }
                    input { class: "field", r#type: "password", autocomplete: "new-password", placeholder: "Passwort (min. 8 Zeichen)", value: "{password}", oninput: move |e| password.set(e.value()) }
                    label { class: "list-row",
                        span { class: "flex-1 text-[16px]", "Administrator" }
                        input { r#type: "checkbox", class: "switch", checked: admin(), onchange: move |e| admin.set(e.checked()) }
                    }
                }
                p { class: "-mt-2 px-1 text-[12.5px] text-muted-foreground", "Gib Benutzername und Passwort der Person weiter – sie koppelt danach ihren eigenen PC." }
                button { class: "btn-primary", r#type: "submit", disabled: busy() || username().trim().len() < 2 || password().chars().count() < 8, "Anlegen" }
            }
        }
    }
}

// ------------------------------------------------------------------ Koppeln

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairCode {
    code: String,
    expires_at: i64,
    pc_port: u16,
    #[serde(default)]
    pc_url: String,
}

/// Code erzeugen; nrzX am PC trägt Adresse und Code ein und koppelt sich.
#[component]
fn PairSheet() -> Element {
    let mut code = use_signal(|| None::<PairCode>);
    let before: Vec<String> = PCS.peek().clone().unwrap_or_default().into_iter().map(|p| p.id).collect();
    use_hook(move || {
        spawn(async move {
            match api::post::<PairCode, _>("/api/pair", &serde_json::json!({})).await {
                Ok(c) => code.set(Some(c)),
                Err(e) => toast(e),
            }
        });
    });
    // Warten, bis der neue PC auftaucht
    use_future(move || {
        let before = before.clone();
        async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(2000).await;
                load_pcs().await;
                let new = PCS.peek().clone().unwrap_or_default().into_iter().find(|p| !before.contains(&p.id));
                if let Some(p) = new {
                    select_pc(Some(p.id.clone()));
                    *PAIRING.write() = false;
                    *SCREEN.write() = Screen::Chats;
                    toast(format!("{} verbunden", p.name));
                    break;
                }
            }
        }
    });
    let host = web_sys::window().and_then(|w| w.location().hostname().ok()).unwrap_or_default();
    let proto = web_sys::window().and_then(|w| w.location().protocol().ok()).unwrap_or_else(|| "http:".into());
    let now = NOW();
    rsx! {
        Sheet { title: "PC verbinden", onclose: move |_| *PAIRING.write() = false,
            p { class: "text-[15px] text-muted-foreground", "In nrzX am PC: Einstellungen → Mobil. Dort diese Adresse und den Code eintragen und „Koppeln“ drücken. Der PC gehört danach zu deinem Konto." }
            match code() {
                None => rsx! { div { class: "grid place-items-center py-8", Spinner {} } },
                Some(c) => {
                    let url = if c.pc_url.is_empty() { format!("{proto}//{host}:{}", c.pc_port) } else { c.pc_url.clone() };
                    let left = c.expires_at - now;
                    rsx! {
                        div { class: "mt-4 flex flex-col gap-3",
                            CopyRow { label: "Adresse", value: url }
                            CopyRow { label: "Code", value: c.code.clone(), big: true }
                            p { class: "flex items-center gap-2 text-[13px] text-muted-foreground",
                                Spinner {}
                                if left > 0 { "Warte auf den PC … (Code gilt noch {reset_in(left)})" } else { "Code abgelaufen – neu öffnen." }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CopyRow(label: String, value: String, #[props(default)] big: bool) -> Element {
    let v = value.clone();
    rsx! {
        div { class: "flex items-center gap-3 rounded-2xl bg-fill px-4 py-3",
            div { class: "min-w-0 flex-1",
                p { class: "text-[12px] font-medium uppercase tracking-wide text-muted-foreground", "{label}" }
                p { class: if big { "font-mono text-[26px] font-semibold tracking-[0.18em]" } else { "truncate font-mono text-[15px]" }, "{value}" }
            }
            button { class: "grid size-9 shrink-0 place-items-center rounded-full text-link active:bg-foreground/10", onclick: move |_| copy(&v),
                icons::Copy { class: "size-[18px]" }
            }
        }
    }
}

// ------------------------------------------------------------------ Updates

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCheck {
    current: String,
    latest: String,
    available: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum UpdateState {
    Idle,
    Checking,
    Checked(UpdateCheck),
    Installing(String),
    /// Server startet mit dieser Version neu
    Restarting(String),
    Failed(String),
}

/// Version und – für Administratoren – Updates aus der App: nach neuer Version suchen,
/// installieren (der Server lädt, prüft und startet sich neu), danach lädt die Seite neu.
#[component]
fn Updates(admin: bool) -> Element {
    let mut state = use_signal(|| UpdateState::Idle);
    let mut check = move || {
        state.set(UpdateState::Checking);
        spawn_forever_local(async move {
            match api::get::<UpdateCheck>("/api/update").await {
                Ok(c) => state.set(UpdateState::Checked(c)),
                Err(e) => state.set(UpdateState::Failed(e)),
            }
        });
    };
    // Administratoren: beim Öffnen gleich nachsehen
    use_hook(move || {
        if admin {
            check();
        }
    });
    let mut install = move |version: String| {
        state.set(UpdateState::Installing(version.clone()));
        spawn_forever_local(async move {
            #[derive(Deserialize)]
            struct Installed {
                version: String,
            }
            match api::post::<Installed, _>("/api/update", &serde_json::json!({})).await {
                Ok(r) => {
                    state.set(UpdateState::Restarting(r.version.clone()));
                    // Warten, bis die neue Version antwortet (höchstens zwei Minuten), dann neu laden
                    for _ in 0..80 {
                        gloo_timers::future::TimeoutFuture::new(1500).await;
                        if let Ok(i) = api::get::<Info>("/api/info").await {
                            if i.version == r.version {
                                reload();
                                return;
                            }
                        }
                    }
                    state.set(UpdateState::Failed("Der Server meldet sich nicht mit der neuen Version – in Umbrel nachsehen.".into()));
                }
                Err(e) => state.set(UpdateState::Failed(e)),
            }
        });
    };
    let server = SERVER_VERSION().unwrap_or_else(|| UI_VERSION.to_string());
    rsx! {
        div { class: "group-list",
            div { class: "list-row",
                icons::Smartphone { class: "size-5 text-muted-foreground" }
                span { class: "flex-1 text-[16px]", "Version" }
                span { class: "text-[15px] tabular-nums text-muted-foreground", "{server}" }
            }
            if admin {
                match state() {
                    UpdateState::Idle => rsx! {
                        button { class: "list-row text-link", onclick: move |_| check(),
                            icons::RefreshCw { class: "size-5" }
                            span { class: "text-[16px]", "Nach Updates suchen" }
                        }
                    },
                    UpdateState::Checking => rsx! {
                        div { class: "list-row text-muted-foreground",
                            icons::LoaderCircle { class: "size-5 animate-spin" }
                            span { class: "text-[16px]", "Suche nach Updates …" }
                        }
                    },
                    UpdateState::Checked(c) if c.available => rsx! {
                        div { class: "list-row",
                            icons::Sparkles { class: "size-5 text-link" }
                            div { class: "min-w-0 flex-1",
                                p { class: "text-[16px]", "Version {c.latest} verfügbar" }
                                p { class: "text-[13px] text-muted-foreground", "Installiert ist {c.current}." }
                            }
                        }
                        button { class: "list-row justify-center font-semibold text-link", onclick: { let v = c.latest.clone(); move |_| install(v.clone()) },
                            icons::Download { class: "size-5" }
                            span { class: "text-[16px]", "Jetzt aktualisieren" }
                        }
                    },
                    UpdateState::Checked(c) => rsx! {
                        button { class: "list-row", onclick: move |_| check(),
                            icons::CircleCheck { class: "size-5 text-success" }
                            span { class: "flex-1 text-left text-[16px]", "Aktuell" }
                            span { class: "text-[13px] text-muted-foreground", "neueste: {c.latest}" }
                        }
                    },
                    UpdateState::Installing(v) => rsx! {
                        div { class: "list-row text-muted-foreground",
                            icons::LoaderCircle { class: "size-5 animate-spin" }
                            span { class: "text-[16px]", "Lade und prüfe {v} …" }
                        }
                    },
                    UpdateState::Restarting(v) => rsx! {
                        div { class: "list-row text-muted-foreground",
                            icons::LoaderCircle { class: "size-5 animate-spin" }
                            span { class: "text-[16px]", "Startet mit {v} neu …" }
                        }
                    },
                    UpdateState::Failed(e) => rsx! {
                        button { class: "list-row", onclick: move |_| check(),
                            icons::TriangleAlert { class: "size-5 shrink-0 text-destructive" }
                            span { class: "min-w-0 flex-1 text-left text-[14px] text-destructive", "{e}" }
                            span { class: "shrink-0 text-[14px] text-link", "Erneut" }
                        }
                    },
                }
            }
        }
        if admin {
            p { class: "mt-1.5 px-4 text-[12.5px] text-muted-foreground", "Updates kommen von nrzGit auf deinem Umbrel. Die neue Version wird geprüft (SHA-256) und ersetzt die laufende ohne Neustart der App in Umbrel; danach lädt diese Seite neu." }
        }
    }
}
