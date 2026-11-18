//! Dialog, Bestätigung, Menü und Meldungen – Aussehen von rust-ui (Dialog, Alert Dialog,
//! Dropdown Menu, Toast), der Zustand liegt hier in Dioxus-Signalen statt in eingeschleustem
//! JavaScript. Ersetzt überall die Browser-Popups (`confirm`, `alert`, `prompt`).

use dioxus::prelude::*;
use futures_channel::oneshot;
use std::cell::RefCell;
use std::rc::Rc;
use tw_merge::tw_merge;
use crate::i18n::tr;

// ------------------------------------------------------------------ Dialog

/// Modales Fenster. Schliesst mit Esc, Klick daneben (abschaltbar) oder dem X.
#[component]
pub fn Modal(
    open: Signal<bool>,
    #[props(into)] title: String,
    #[props(into, optional)] description: Option<String>,
    #[props(into, optional)] class: Option<String>,
    #[props(default = true)] dismissible: bool,
    children: Element,
) -> Element {
    if !open() {
        return rsx! {};
    }
    let content = tw_merge!(
        "nrx-pop nrx-dialog relative p-6 w-full max-w-lg max-h-[85vh] overflow-y-auto animate-in",
        class.as_deref().unwrap_or("")
    );
    rsx! {
        div {
            class: "fixed inset-0 z-[60] flex items-center justify-center bg-black/35 p-4 backdrop-blur-[1.5px]",
            tabindex: "-1",
            // Erstes Eingabefeld des Dialogs fokussieren, sonst die Fläche selbst (für Escape)
            onmounted: move |_| {
                spawn(async {
                    gloo_timers::future::TimeoutFuture::new(0).await;
                    crate::util::focus_dialog();
                });
            },
            onkeydown: move |e| if e.key() == Key::Escape && dismissible { open.set(false) },
            onclick: move |_| if dismissible { open.set(false) },
            div {
                class: "{content}",
                role: "dialog",
                "aria-modal": "true",
                onclick: move |e| e.stop_propagation(),
                button {
                    class: "absolute top-4 right-4 rounded-sm p-1 text-muted-foreground hover:text-foreground focus:outline-none focus:ring-2 focus:ring-ring",
                    title: tr("Schliessen"),
                    onclick: move |_| open.set(false),
                    icons::X { class: "size-4" }
                }
                div { class: "flex flex-col gap-2 pr-6",
                    h3 { class: "text-lg leading-none font-semibold", "{title}" }
                    if let Some(d) = description {
                        p { class: "text-muted-foreground text-sm", "{d}" }
                    }
                }
                div { class: "mt-5", {children} }
            }
        }
    }
}

// ------------------------------------------------------------------ Bestätigung

#[derive(Clone)]
pub struct Ask {
    pub title: String,
    pub description: String,
    pub confirm: String,
    pub destructive: bool,
}

impl Ask {
    pub fn new(title: impl Into<String>, description: impl Into<String>) -> Self {
        Ask { title: title.into(), description: description.into(), confirm: {tr("Bestätigen")}.into(), destructive: false }
    }
    pub fn confirm(mut self, label: impl Into<String>) -> Self {
        self.confirm = label.into();
        self
    }
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
}

type Pending = Rc<RefCell<Option<oneshot::Sender<bool>>>>;

static ASK: GlobalSignal<Option<(Ask, Pending)>> = Signal::global(|| None);

/// Eigene Rückfrage statt `window.confirm` – `true`, wenn bestätigt.
pub async fn confirm(ask: Ask) -> bool {
    let (tx, rx) = oneshot::channel();
    if let Some((_, old)) = ASK.write().replace((ask, Rc::new(RefCell::new(Some(tx))))) {
        if let Some(s) = old.borrow_mut().take() {
            let _ = s.send(false);
        }
    }
    rx.await.unwrap_or(false)
}

fn answer(yes: bool) {
    if let Some((_, pending)) = ASK.write().take() {
        if let Some(s) = pending.borrow_mut().take() {
            let _ = s.send(yes);
        }
    }
}

/// Einmal in der App einbinden.
#[component]
pub fn ConfirmHost() -> Element {
    let Some((ask, _)) = ASK.read().clone() else { return rsx! {} };
    let action = if ask.destructive { "nrx-btn-danger" } else { "nrx-btn-main" };
    rsx! {
        div {
            class: "fixed inset-0 z-[70] flex items-center justify-center bg-black/35 p-4 backdrop-blur-[1.5px]",
            tabindex: "-1",
            onmounted: move |e| async move { let _ = e.set_focus(true).await; },
            onkeydown: move |e| match e.key() {
                Key::Escape => answer(false),
                Key::Enter => answer(true),
                _ => {}
            },
            div { class: "nrx-pop nrx-dialog w-full max-w-md p-6 animate-in", role: "alertdialog",
                div { class: "flex gap-4",
                    if ask.destructive {
                        div { class: "grid size-10 shrink-0 place-items-center rounded-full bg-destructive/15 text-destructive",
                            icons::TriangleAlert { class: "size-5" }
                        }
                    }
                    div { class: "flex flex-col gap-2",
                        h3 { class: "text-[16px] leading-snug font-semibold", "{ask.title}" }
                        p { class: "text-muted-foreground text-sm", "{ask.description}" }
                    }
                }
                footer { class: "mt-6 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
                    button { class: "nrx-btn", onclick: move |_| answer(false),
                        {tr("Abbrechen")}
                    }
                    button { class: "nrx-btn {action}", onclick: move |_| answer(true),
                        "{ask.confirm}"
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Meldungen

/// Fehlermeldung unten rechts – Hinweise und Bestätigungen gibt es dort keine (mehr).
#[derive(Clone, PartialEq)]
struct Toast {
    id: u64,
    text: String,
    /// Wird gerade ausgeblendet (Animation läuft, danach entfernt)
    leaving: bool,
}

static TOASTS: GlobalSignal<Vec<Toast>> = Signal::global(Vec::new);
static NEXT: GlobalSignal<u64> = Signal::global(|| 0);

/// Dauer der Ausblende-Animation (muss zu `toast-out` in tailwind.css passen).
const LEAVE_MS: u32 = 220;

fn push(text: String) {
    let id = {
        let mut n = NEXT.write();
        *n += 1;
        *n
    };
    TOASTS.write().push(Toast { id, text, leaving: false });
    // Im Wurzel-Bereich: Meldungen kommen auch aus dem Ereignisstrom des Servers (ausserhalb jeder
    // Komponente) – `spawn` bräche dort ab und die Oberfläche lüde neu
    dioxus::core::spawn_forever(async move {
        gloo_timers::future::TimeoutFuture::new(7000).await;
        dismiss(id);
    });
}

/// Meldung sanft ausblenden und danach entfernen.
fn dismiss(id: u64) {
    let mut found = false;
    for t in TOASTS.write().iter_mut().filter(|t| t.id == id && !t.leaving) {
        t.leaving = true;
        found = true;
    }
    if found {
        dioxus::core::spawn_forever(async move {
            gloo_timers::future::TimeoutFuture::new(LEAVE_MS).await;
            TOASTS.write().retain(|t| t.id != id);
        });
    }
}

/// Fehlermeldung unten rechts (das Einzige, was dort noch erscheint).
pub fn toast_error(text: impl std::fmt::Display) {
    push(text.to_string());
}

#[component]
pub fn Toaster() -> Element {
    rsx! {
        div { class: "pointer-events-none fixed bottom-20 right-4 z-[80] flex w-full max-w-sm flex-col gap-2",
            for t in TOASTS.read().iter().cloned() {
                div {
                    key: "{t.id}",
                    role: "alert",
                    class: format!(
                        "nrx-pop pointer-events-auto flex items-start gap-3 border-destructive/40 px-4 py-3 text-sm {}",
                        if t.leaving { "animate-toast-out" } else { "animate-toast-in" },
                    ),
                    icons::CircleAlert { class: "mt-0.5 size-4 shrink-0 text-destructive" }
                    span { class: "flex-1", "{t.text}" }
                    button {
                        class: "text-muted-foreground hover:text-foreground",
                        onclick: move |_| dismiss(t.id),
                        icons::X { class: "size-3.5" }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Menü

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Align {
    #[default]
    Start,
    End,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Side {
    #[default]
    Bottom,
    Top,
}

/// Aufklappmenü (rust-ui Dropdown Menu). `trigger` ist der sichtbare Knopf.
#[component]
pub fn Dropdown(
    trigger: Element,
    #[props(default)] align: Align,
    #[props(default)] side: Side,
    #[props(into, optional)] class: Option<String>,
    #[props(into, optional)] trigger_class: Option<String>,
    children: Element,
) -> Element {
    let mut open = use_signal(|| false);
    provide_context(MenuOpen(open));
    let pos = match (align, side) {
        (Align::Start, Side::Bottom) => "left-0 top-full mt-1.5",
        (Align::End, Side::Bottom) => "right-0 top-full mt-1.5",
        (Align::Start, Side::Top) => "left-0 bottom-full mb-1.5",
        (Align::End, Side::Top) => "right-0 bottom-full mb-1.5",
    };
    let menu = tw_merge!(
        "nrx-pop absolute z-50 min-w-[13rem] overflow-hidden p-1.5 animate-in",
        pos,
        class.as_deref().unwrap_or("")
    );
    rsx! {
        div { class: "relative",
            button {
                class: trigger_class.unwrap_or_default(),
                "aria-expanded": "{open}",
                onclick: move |_| open.toggle(),
                {trigger}
            }
            if open() {
                div { class: "fixed inset-0 z-40", onclick: move |_| open.set(false) }
                div { class: "{menu}", role: "menu", "data-popup": "1", {children} }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct MenuOpen(Signal<bool>);

/// Eintrag im Menü; schliesst es nach dem Klick.
#[component]
pub fn MenuItem(
    onclick: EventHandler<MouseEvent>,
    #[props(default)] destructive: bool,
    #[props(default)] active: bool,
    children: Element,
) -> Element {
    let ctx = try_consume_context::<MenuOpen>();
    let color = if destructive { "text-destructive hover:bg-destructive/15 [&_svg]:text-destructive" } else { "hover:bg-white/[0.09] [&_svg]:text-muted-foreground hover:[&_svg]:text-foreground" };
    let act = if active { "bg-white/[0.07]" } else { "" };
    rsx! {
        button {
            class: "relative flex min-h-8 w-full cursor-default select-none items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-left text-[13.5px] outline-none transition-colors [&_svg]:size-4 [&_svg]:shrink-0 {color} {act}",
            role: "menuitem",
            onclick: move |e| {
                if let Some(MenuOpen(mut o)) = ctx {
                    o.set(false);
                }
                onclick.call(e);
            },
            {children}
            // Gewählter Eintrag: Haken rechts
            if active {
                icons::Check { class: "ml-auto !size-3.5 !text-foreground" }
            }
        }
    }
}

#[component]
pub fn MenuLabel(children: Element) -> Element {
    rsx! { div { class: "px-2.5 pt-2 pb-1 text-[11.5px] font-medium text-muted-foreground", {children} } }
}

#[component]
pub fn MenuSeparator() -> Element {
    rsx! { div { class: "mx-1.5 my-1.5 h-px bg-white/[0.08]" } }
}

/// Suchfeld oben in einem Menü: bekommt beim Öffnen den Fokus, bleibt beim Scrollen stehen,
/// Enter wählt (`onenter`, z. B. den ersten Treffer) und schliesst, Esc schliesst. Beim
/// Schliessen wird die Suche geleert.
#[component]
pub fn MenuSearch(value: Signal<String>, #[props(into)] placeholder: String, #[props(optional)] onenter: Option<EventHandler<()>>) -> Element {
    let ctx = try_consume_context::<MenuOpen>();
    use_drop(move || {
        if !value.peek().is_empty() {
            value.set(String::new());
        }
    });
    rsx! {
        div { class: "nrx-pop-head sticky -top-1.5 z-10 -mx-1.5 -mt-1.5 mb-1 p-1.5",
            div { class: "relative flex items-center",
                icons::Search { class: "pointer-events-none absolute left-2.5 size-3.5 text-muted-foreground" }
                input {
                    class: "h-8 w-full rounded-lg bg-white/[0.06] pl-8 pr-2 text-[13.5px] outline-none placeholder:text-muted-foreground focus:bg-white/[0.09]",
                    r#type: "search",
                    placeholder: "{placeholder}",
                    value: "{value}",
                    onmounted: move |e| async move {
                        let _ = e.set_focus(true).await;
                    },
                    oninput: move |e| value.set(e.value()),
                    onkeydown: move |e: KeyboardEvent| match e.key() {
                        Key::Enter => {
                            e.prevent_default();
                            if let Some(h) = onenter {
                                h.call(());
                            }
                            if let Some(MenuOpen(mut o)) = ctx {
                                o.set(false);
                            }
                        }
                        Key::Escape => {
                            if let Some(MenuOpen(mut o)) = ctx {
                                o.set(false);
                            }
                        }
                        _ => {}
                    },
                }
            }
        }
    }
}

/// Trifft die Suche (ohne Gross-/Kleinschreibung) einen der Texte?
pub fn search_hit(needle: &str, texts: &[&str]) -> bool {
    let n = needle.trim().to_lowercase();
    n.is_empty() || texts.iter().any(|t| t.to_lowercase().contains(&n))
}

// ------------------------------------------------------------------ Bildbetrachter

/// Bilder im Betrachter (Adresse, Name) und das gezeigte.
static IMAGES: GlobalSignal<Option<(Vec<(String, String)>, usize)>> = Signal::global(|| None);

/// Bild in der App groß zeigen (Popup statt Browser-Tab oder Bildprogramm des Systems).
/// `list`: alle Bilder der Gruppe (mit ← / → blättern), `index`: das angeklickte.
pub fn view_images(list: Vec<(String, String)>, index: usize) {
    if list.is_empty() {
        return;
    }
    let i = index.min(list.len() - 1);
    *IMAGES.write() = Some((list, i));
}

/// Klicks auf Bilder in Antworten (Markdown) öffnen den Betrachter – einmal beim Start.
pub fn install_image_clicks() {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    let open = Closure::<dyn FnMut(JsValue, JsValue)>::new(|src: JsValue, alt: JsValue| {
        if let Some(src) = src.as_string() {
            view_images(vec![(src, alt.as_string().unwrap_or_default())], 0);
        }
    });
    if let Some(w) = web_sys::window() {
        let _ = js_sys::Reflect::set(&w, &JsValue::from_str("__nrzViewImage"), open.as_ref().unchecked_ref());
    }
    open.forget();
    let _ = js_sys::eval(
        r#"document.addEventListener('click', function (e) {
  var img = e.target && e.target.closest && e.target.closest('.markdown img');
  if (!img || !window.__nrzViewImage) return;
  e.preventDefault(); e.stopPropagation();
  window.__nrzViewImage(img.currentSrc || img.src, img.alt || '');
}, true);"#,
    );
}

/// Betrachter – einmal im Rahmen eingebunden.
#[component]
pub fn ImageViewerHost() -> Element {
    let Some((list, i)) = IMAGES() else { return rsx! {} };
    let n = list.len();
    let (url, name) = list[i].clone();
    let close = move || *IMAGES.write() = None;
    let go = move |d: isize| {
        if let Some((l, i)) = IMAGES.write().as_mut() {
            *i = ((*i as isize + d).rem_euclid(l.len() as isize)) as usize;
        }
    };
    let nav_btn = "absolute top-1/2 grid size-10 -translate-y-1/2 place-items-center rounded-full bg-black/50 text-white/80 hover:bg-black/70 hover:text-white";
    rsx! {
        div {
            class: "fixed inset-0 z-[90] flex flex-col items-center justify-center gap-3 bg-black/85 p-8 outline-none backdrop-blur-[2px] animate-in",
            tabindex: "-1",
            role: "dialog",
            "aria-modal": "true",
            onmounted: move |e| async move {
                let _ = e.set_focus(true).await;
            },
            onclick: move |_| close(),
            onkeydown: move |e: KeyboardEvent| match e.key() {
                Key::Escape => {
                    e.stop_propagation();
                    close();
                }
                Key::ArrowLeft if n > 1 => go(-1),
                Key::ArrowRight if n > 1 => go(1),
                _ => {}
            },
            img {
                class: "max-h-[calc(100%-2.5rem)] max-w-full rounded-lg object-contain shadow-2xl",
                src: "{url}",
                alt: "{name}",
                onclick: move |e| e.stop_propagation(),
            }
            div { class: "flex max-w-full items-center gap-2 text-xs text-white/70",
                span { class: "truncate", "{name}" }
                if n > 1 {
                    span { class: "tabular-nums", "{i + 1} / {n}" }
                }
            }
            if n > 1 {
                button {
                    class: "{nav_btn} left-4",
                    title: tr("Vorheriges Bild"),
                    onclick: move |e| {
                        e.stop_propagation();
                        go(-1);
                    },
                    icons::ChevronLeft { class: "size-5" }
                }
                button {
                    class: "{nav_btn} right-4",
                    title: tr("Nächstes Bild"),
                    onclick: move |e| {
                        e.stop_propagation();
                        go(1);
                    },
                    icons::ChevronRight { class: "size-5" }
                }
            }
            button {
                class: "absolute top-4 right-4 grid size-9 place-items-center rounded-full bg-black/50 text-white/80 hover:bg-black/70 hover:text-white",
                title: tr("Schließen"),
                onclick: move |_| close(),
                icons::X { class: "size-4" }
            }
        }
    }
}
