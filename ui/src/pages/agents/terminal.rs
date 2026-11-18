//! Terminal im Browser: Ausgabe über Server-Sent Events (`/api/v1/terminals/<id>/stream`,
//! Base64), dargestellt vom VT100-Interpreter (`vt.rs`) als Raster in einem `pre`, Eingaben
//! (Tasten, Einfügen) gesammelt und der Reihe nach an `…/input` geschickt. Adressen sind
//! anklickbar (für Anmeldungen per OAuth).

use super::vt::{style, Attr, Vt};
use crate::api;
use crate::i18n::{tr, trf};
use dioxus::prelude::*;
use nrzx_shared::agents::{TerminalInput, TerminalSize};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

/// Terminal (PTY) – für die Anmeldung in den CLIs und das Terminal unten auf der Thread-Seite.
/// `id` aus `POST /api/v1/terminals`.
#[component]
pub fn TerminalView(id: String) -> Element {
    rsx! {
        TerminalPane { key: "{id}", id: id.clone() }
    }
}

/// Abschnitt einer Zeile mit gleichem Stil (bzw. eine Adresse).
#[derive(Clone, PartialEq)]
struct Seg {
    text: String,
    style: String,
    link: Option<String>,
    cursor: bool,
}

/// Verbindung zum Ereignisstrom (wird beim Entfernen geschlossen).
struct Stream {
    source: web_sys::EventSource,
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_exit: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_error: Closure<dyn FnMut(web_sys::Event)>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.source.close();
    }
}

/// Terminal mit Rückmeldung beim Ende (`on_exit` mit dem Exit-Code). `themed`: eingefärbt wie
/// die App statt fest dunkel – Fläche, Schrift und Grundfarben folgen dem Thema, im Milchglas
/// scheint es durch (natives CLI statt des Chats).
#[component]
pub fn TerminalPane(
    id: String,
    #[props(optional)] on_exit: Option<EventHandler<i32>>,
    #[props(into, optional)] class: Option<String>,
    #[props(default)] themed: bool,
    // Beim Einblenden die Eingabe fokussieren (aus, solange es unsichtbar im Hintergrund lädt)
    #[props(default = true)] autofocus: bool,
) -> Element {
    let vt = use_hook(|| Rc::new(RefCell::new(Vt::new(100, 30))));
    let mut version = use_signal(|| 0u64);
    let mut exited = use_signal(|| None::<i32>);
    let mut focused = use_signal(|| false);
    let mut cell = use_signal(|| (7.8f64, 17.0f64));
    let mut size = use_signal(|| (0usize, 0usize));
    // Kurze Rückmeldung unten (z. B. nach dem Einfügen – manche Programme zeigen nichts an)
    let mut notice = use_signal(|| None::<(u32, String)>);
    let mut notify = move |text: String| {
        let n = notice.peek().as_ref().map_or(0, |x| x.0) + 1;
        notice.set(Some((n, text)));
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(5000).await;
            if notice.peek().as_ref().is_some_and(|x| x.0 == n) {
                notice.set(None);
            }
        });
    };
    let stream: Rc<RefCell<Option<Stream>>> = use_hook(|| Rc::new(RefCell::new(None)));

    // Eingaben der Reihe nach senden
    let input = use_hook(|| InputQueue::new(&id));

    // Ereignisstrom öffnen
    {
        let input = input.clone();
        let vt = vt.clone();
        let stream = stream.clone();
        let id = id.clone();
        use_hook(move || {
            let Ok(source) = web_sys::EventSource::new(&format!("/api/v1/terminals/{id}/stream")) else { return };
            let vt2 = vt.clone();
            let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                let Some(data) = e.data().as_string() else { return };
                let Some(bytes) = decode_b64(&data) else { return };
                let reply = {
                    let mut v = vt2.borrow_mut();
                    v.write(&bytes);
                    std::mem::take(&mut v.responses)
                };
                if !reply.is_empty() {
                    input.send(reply);
                }
                *version.write() += 1;
            });
            let on_exit = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                let code = e.data().as_string().and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(-1);
                exited.set(Some(code));
            });
            // Terminal gibt es nicht mehr (z. B. geschlossen, während niemand zusah): wie beendet (−1);
            // sonst verbindet sich der Browser von selbst neu
            let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
                let closed = e.target().and_then(|t| t.dyn_into::<web_sys::EventSource>().ok()).is_some_and(|s| s.ready_state() == web_sys::EventSource::CLOSED);
                if closed && exited.peek().is_none() {
                    exited.set(Some(-1));
                }
            });
            source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
            let _ = source.add_event_listener_with_callback("exit", on_exit.as_ref().unchecked_ref());
            *stream.borrow_mut() = Some(Stream { source, _on_message: on_message, _on_exit: on_exit, _on_error: on_error });
        });
    }
    {
        let stream = stream.clone();
        use_drop(move || {
            stream.borrow_mut().take();
        });
    }
    // Ende melden
    use_effect(move || {
        if let (Some(code), Some(h)) = (exited(), on_exit) {
            h.call(code);
        }
    });

    // Grösse an den Server melden, wenn sie sich ändert
    let rid = id.clone();
    let vt_resize = vt.clone();
    let mut apply_size = move |w: f64, h: f64| {
        let (cw, ch) = cell();
        let cols = ((w - 16.0) / cw).floor().max(20.0) as usize;
        let rows = ((h - 16.0) / ch).floor().max(5.0) as usize;
        if (cols, rows) == size() {
            return;
        }
        size.set((cols, rows));
        vt_resize.borrow_mut().resize(cols, rows);
        *version.write() += 1;
        let rid = rid.clone();
        spawn(async move {
            let _: Result<serde_json::Value, _> = api::post(&format!("/terminals/{rid}/resize"), &TerminalSize { cols: cols as u16, rows: rows as u16 }).await;
        });
    };

    let _ = version();
    let v = vt.borrow();
    let lines = v.lines();
    let offset = v.screen_offset();
    let show_cursor = v.cursor_visible && exited().is_none() && focused();
    let (cx, cy) = (v.cx, v.cy);
    let rows: Vec<Vec<Seg>> = render_lines(&lines, &v.links, offset, if show_cursor { Some((cx, cy)) } else { None });
    let links: Vec<String> = {
        let mut seen = Vec::new();
        for segs in rows.iter().rev() {
            for s in segs {
                if let Some(l) = &s.link {
                    if !seen.contains(l) {
                        seen.push(l.clone());
                    }
                }
            }
            if seen.len() >= 2 {
                break;
            }
        }
        seen
    };
    let app_cursor = v.app_cursor;
    let bracketed = v.bracketed_paste;
    drop(v);
    let scroll_id = format!("term-{id}");
    let input_id = format!("term-input-{id}");
    let sid = scroll_id.clone();
    // Nach neuer Ausgabe nach unten scrollen – nicht, solange man hochgescrollt hat
    let mut follow = use_signal(|| true);
    use_effect(move || {
        let _ = version();
        if !*follow.peek() {
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
    let extra = class.unwrap_or_default();
    // Fest dunkel bzw. wie die App: Rahmen, Leisten, gedämpfte Schrift, Knöpfe, Adressen
    let (frame, bar, dim, btn, link) = if themed {
        (
            "nrz-term-themed text-foreground",
            "border-border/70",
            "text-muted-foreground",
            "text-muted-foreground hover:bg-accent hover:text-foreground",
            "text-foreground underline decoration-muted-foreground/60",
        )
    } else {
        ("rounded-lg border bg-[#141414] text-[#e5e5e5]", "border-white/10", "text-white/50", "text-white/70 hover:bg-white/10 hover:text-white", "text-sky-300")
    };

    rsx! {
        div { class: "flex h-full min-h-0 flex-col overflow-hidden {frame} {extra}",
            if !links.is_empty() {
                div { class: "flex flex-wrap items-center gap-2 border-b {bar} px-3 py-1.5 text-xs",
                    icons::Link { class: "size-3.5 shrink-0 {dim}" }
                    for l in links.iter().cloned() {
                        div { key: "{l}", class: "flex min-w-0 flex-[1_1_14rem] items-center gap-1.5",
                            a { class: "min-w-0 max-w-[28rem] truncate {link} hover:underline", href: "{l}", title: "{l}", target: "_blank", rel: "noopener noreferrer", "{l}" }
                            a {
                                class: "inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 {btn}",
                                href: "{l}",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                icons::ExternalLink { class: "size-3.5" }
                                {tr("Öffnen")}
                            }
                            crate::components::widgets::CopyButton { text: l.clone(), class: "rounded p-1 {btn}" }
                        }
                    }
                }
            }
            div {
                id: "{scroll_id}",
                class: if themed { "relative min-h-0 flex-1 cursor-text overflow-auto px-4 py-3" } else { "relative min-h-0 flex-1 cursor-text overflow-auto p-2" },
                onscroll: move |e| {
                    let gap = e.scroll_height() as f64 - (e.scroll_top() + e.client_height() as f64);
                    if gap <= 4.0 && !follow() {
                        follow.set(true);
                    } else if gap > 120.0 && follow() {
                        follow.set(false);
                    }
                },
                onwheel: move |e: WheelEvent| {
                    if e.delta().strip_units().y < 0.0 && *follow.peek() {
                        follow.set(false);
                    }
                },
                // Rechtsklick: Eingabefeld unter den Mauszeiger legen, damit das Kontextmenü
                // des Browsers „Einfügen“ anbietet
                onmousedown: {
                    let input_id = input_id.clone();
                    let scroll_id = scroll_id.clone();
                    move |e: MouseEvent| {
                        if e.trigger_button() == Some(dioxus::html::input_data::MouseButton::Secondary) {
                            // Sonst setzt der Browser den Fokus danach auf den Dialog zurück
                            e.prevent_default();
                            let p = e.client_coordinates();
                            place_input(&input_id, &scroll_id, p.x, p.y);
                        } else {
                            reset_input(&input_id);
                        }
                    }
                },
                onresize: move |e| {
                    if let Ok(s) = e.get_content_box_size() {
                        apply_size(s.width, s.height);
                    }
                },
                // Klick: Eingabe fokussieren (ausser beim Markieren zum Kopieren)
                onclick: {
                    let input_id = input_id.clone();
                    move |_| {
                        if !has_selection() {
                            focus_input(&input_id);
                        }
                    }
                },
                // Unsichtbares Eingabefeld: Tasten, Texteingabe (auch IME/Handy) und Einfügen
                textarea {
                    id: "{input_id}",
                    class: "pointer-events-none absolute top-0 left-0 h-px w-px resize-none opacity-0",
                    autocomplete: "off",
                    autocapitalize: "off",
                    spellcheck: "false",
                    "aria-label": tr("Terminal-Eingabe"),
                    onmounted: move |e| async move {
                        if autofocus {
                            let _ = e.set_focus(true).await;
                        }
                    },
                    onfocus: move |_| focused.set(true),
                    onblur: {
                        let input_id = input_id.clone();
                        move |_| {
                            focused.set(false);
                            reset_input(&input_id);
                        }
                    },
                    onkeydown: {
                        let input = input.clone();
                        move |e: KeyboardEvent| {
                            if exited().is_some() {
                                return;
                            }
                            if let Some(data) = key_input(&e, app_cursor) {
                                e.prevent_default();
                                // Escape usw. gehören dem Programm, nicht dem Dialog drumherum
                                e.stop_propagation();
                                input.send(data);
                            }
                        }
                    },
                    oninput: {
                        let input = input.clone();
                        let input_id = input_id.clone();
                        move |e: FormEvent| {
                            let text = e.value();
                            clear_input(&input_id);
                            reset_input(&input_id);
                            if !text.is_empty() && exited().is_none() {
                                if text.chars().count() > 1 {
                                    notify(trf("{0} Zeichen gesendet.", &[&text.chars().count()]));
                                }
                                input.send(text.replace("\r\n", "\r").replace('\n', "\r"));
                            }
                        }
                    },
                    // Einfügen: Text aus dem Ereignis; fehlt er, landet er im Eingabefeld (oninput)
                    onpaste: {
                        let input = input.clone();
                        let input_id = input_id.clone();
                        move |e: ClipboardEvent| {
                            let Some(text) = paste_text(&e) else { return };
                            e.prevent_default();
                            reset_input(&input_id);
                            if exited().is_some() {
                                return;
                            }
                            notify(trf("{0} Zeichen eingefügt und gesendet.", &[&text.chars().count()]));
                            let text = text.replace("\r\n", "\r").replace('\n', "\r");
                            input.send(if bracketed { format!("\u{1b}[200~{text}\u{1b}[201~") } else { text });
                        }
                    },
                }
                // Zeichengrösse messen
                span {
                    class: "pointer-events-none invisible absolute block w-max font-mono text-[12.5px] leading-[1.35]",
                    "aria-hidden": "true",
                    onmounted: move |e| async move {
                        if let Ok(r) = e.get_client_rect().await {
                            if r.width() > 0.0 {
                                cell.set((r.width() / 10.0, r.height()));
                            }
                        }
                    },
                    "MMMMMMMMMM"
                }
                pre { class: "m-0 whitespace-pre font-mono text-[12.5px] leading-[1.35]",
                    for (i , segs) in rows.into_iter().enumerate() {
                        TermRow { key: "{i}", segs }
                    }
                }
            }
            if let Some(code) = exited() {
                div { class: "border-t {bar} px-3 py-1.5 text-xs {dim}",
                    {trf("Prozess beendet (Code {0}).", &[&code])}
                }
            } else if let Some((_, text)) = notice() {
                div { class: if themed { "flex items-center gap-1.5 border-t border-border/70 px-4 py-1 text-[11px] text-emerald-500" } else { "flex items-center gap-1.5 border-t border-white/10 px-3 py-1 text-[11px] text-emerald-300" },
                    icons::Check { class: "size-3.5" }
                    "{text}"
                }
            } else if !themed {
                div { class: "border-t border-white/10 px-3 py-1 text-[11px] text-white/40",
                    {tr("Klicken und tippen. Einfügen mit Strg/Cmd+V oder Rechtsklick, Abbrechen mit Strg+C.")}
                }
            }
        }
    }
}

#[component]
fn TermRow(segs: Vec<Seg>) -> Element {
    rsx! {
        div { class: "min-h-[1.35em]",
            for (i , s) in segs.into_iter().enumerate() {
                if let Some(href) = s.link.clone() {
                    a { key: "{i}", class: "underline decoration-sky-400/60 hover:text-sky-300", style: "{s.style}", href: "{href}", target: "_blank", rel: "noopener noreferrer", "{s.text}" }
                } else if s.cursor {
                    span { key: "{i}", class: "bg-[var(--term-cursor)] text-[var(--term-cursor-text)]", "{s.text}" }
                } else if s.style.is_empty() {
                    span { key: "{i}", "{s.text}" }
                } else {
                    span { key: "{i}", style: "{s.style}", "{s.text}" }
                }
            }
        }
    }
}

/// Zeilen in Abschnitte gleichen Stils zerlegen; Adressen aus OSC-8-Links und aus dem Text
/// (auch über umgebrochene Zeilen) als Links.
fn render_lines(lines: &[&super::vt::Line], links: &[String], offset: usize, cursor: Option<(usize, usize)>) -> Vec<Vec<Seg>> {
    let (urls, link_of) = super::vt::find_links(lines, links);
    lines
        .iter()
        .enumerate()
        .map(|(li, l)| {
            let cur = cursor.and_then(|(cx, cy)| (li == offset + cy).then_some(cx));
            // Zeilenende ohne Inhalt weglassen (ausser Cursor/Hintergrund)
            let mut end = l.cells.iter().rposition(|c| c.ch != ' ' || c.attr.bg != super::vt::Color::Default).map(|p| p + 1).unwrap_or(0);
            if let Some(cx) = cur {
                end = end.max(cx + 1);
            }
            let mut segs: Vec<Seg> = Vec::new();
            let mut last: Option<(Attr, Option<usize>, bool)> = None;
            for x in 0..end.min(l.cells.len()) {
                let c = l.cells[x];
                let key = (c.attr, link_of[li][x], cur == Some(x));
                if last.as_ref() == Some(&key) {
                    if let Some(s) = segs.last_mut() {
                        s.text.push(c.ch);
                    }
                } else {
                    segs.push(Seg { text: c.ch.to_string(), style: style(&c.attr), link: key.1.map(|u| urls[u].clone()), cursor: key.2 });
                    last = Some(key);
                }
            }
            segs
        })
        .collect()
}

/// Base64 → Bytes (über `atob` des Browsers).
fn decode_b64(data: &str) -> Option<Vec<u8>> {
    let bin = web_sys::window()?.atob(data.trim()).ok()?;
    Some(bin.chars().map(|c| c as u32 as u8).collect())
}

/// Text aus einem Einfüge-Ereignis.
fn paste_text(e: &ClipboardEvent) -> Option<String> {
    let ev = e.data().downcast::<web_sys::Event>()?.clone();
    let ce = ev.dyn_into::<web_sys::ClipboardEvent>().ok()?;
    ce.clipboard_data()?.get_data("text").ok().filter(|t| !t.is_empty())
}

/// Ist im Dokument Text markiert (dann kopiert Strg+C statt abzubrechen)?
fn has_selection() -> bool {
    web_sys::window()
        .and_then(|w| w.get_selection().ok().flatten())
        .and_then(|s| s.to_string().as_string())
        .is_some_and(|s| !s.is_empty())
}

/// Taste → Zeichen für das Programm (None = dem Browser überlassen).
fn key_input(e: &KeyboardEvent, app_cursor: bool) -> Option<String> {
    let m = e.modifiers();
    let ctrl = m.contains(Modifiers::CONTROL);
    let alt = m.contains(Modifiers::ALT);
    let meta = m.contains(Modifiers::META);
    if meta {
        return None; // Cmd+C / Cmd+V usw.
    }
    let arrow = |c: char| if app_cursor { format!("\u{1b}O{c}") } else { format!("\u{1b}[{c}") };
    let s = match e.key() {
        Key::Enter => "\r".to_string(),
        Key::Backspace => if alt { "\u{1b}\u{7f}".into() } else { "\u{7f}".into() },
        Key::Tab => if m.contains(Modifiers::SHIFT) { "\u{1b}[Z".into() } else { "\t".into() },
        Key::Escape => "\u{1b}".into(),
        Key::ArrowUp => arrow('A'),
        Key::ArrowDown => arrow('B'),
        Key::ArrowRight => arrow('C'),
        Key::ArrowLeft => arrow('D'),
        Key::Home => arrow('H'),
        Key::End => arrow('F'),
        Key::Delete => "\u{1b}[3~".into(),
        Key::Insert => "\u{1b}[2~".into(),
        Key::PageUp => "\u{1b}[5~".into(),
        Key::PageDown => "\u{1b}[6~".into(),
        Key::Character(c) => {
            let mut chars = c.chars();
            let first = chars.next()?;
            if ctrl {
                if chars.next().is_some() {
                    return None;
                }
                let lower = first.to_ascii_lowercase();
                // Strg+V: Einfügen über den Browser; Strg+C mit Markierung: kopieren
                if lower == 'v' || (lower == 'c' && has_selection()) {
                    return None;
                }
                let code = match lower {
                    'a'..='z' => (lower as u8 - b'a' + 1) as char,
                    '[' => '\u{1b}',
                    '\\' => '\u{1c}',
                    ']' => '\u{1d}',
                    ' ' | '@' => '\u{0}',
                    _ => return None,
                };
                return Some(if alt { format!("\u{1b}{code}") } else { code.to_string() });
            }
            // Zeichen kommen über das Eingabefeld (Tastaturbelegung, Option-Zeichen, IME)
            return None;
        }
        _ => return None,
    };
    Some(s)
}

/// Eingaben in Reihenfolge an den Server (ein Auftrag zur Zeit, Wartendes wird zusammengefasst).
#[derive(Clone)]
struct InputQueue {
    id: String,
    queue: Rc<RefCell<VecDeque<String>>>,
    busy: Rc<Cell<bool>>,
}

impl InputQueue {
    fn new(id: &str) -> Self {
        InputQueue { id: id.to_string(), queue: Rc::new(RefCell::new(VecDeque::new())), busy: Rc::new(Cell::new(false)) }
    }

    fn send(&self, data: String) {
        self.queue.borrow_mut().push_back(data);
        if self.busy.replace(true) {
            return;
        }
        let me = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                let data: String = me.queue.borrow_mut().drain(..).collect();
                if data.is_empty() {
                    me.busy.set(false);
                    break;
                }
                let _: Result<serde_json::Value, _> = api::post(&format!("/terminals/{}/input", me.id), &TerminalInput { data }).await;
            }
        });
    }
}

/// Eingabefeld des Terminals fokussieren.
fn focus_input(id: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = el.focus();
    }
}

/// Eingabefeld für einen Rechtsklick unter den Mauszeiger legen (sonst bietet das Kontextmenü
/// kein „Einfügen“ an). Wird nach dem Einfügen bzw. beim Verlassen zurückgesetzt.
fn place_input(id: &str, scroll_id: &str, x: f64, y: f64) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    let (Some(el), Some(area)) = (doc.get_element_by_id(id), doc.get_element_by_id(scroll_id)) else { return };
    let r = area.get_bounding_client_rect();
    let left = x - r.left() + area.scroll_left() as f64 - 12.0;
    let top = y - r.top() + area.scroll_top() as f64 - 12.0;
    let _ = el.set_attribute("style", &format!("left:{left}px;top:{top}px;width:24px;height:24px;pointer-events:auto;z-index:10"));
    if let Ok(h) = el.dyn_into::<web_sys::HtmlElement>() {
        let _ = h.focus();
    }
}

/// Eingabefeld wieder klein und unsichtbar oben links.
fn reset_input(id: &str) {
    if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id)) {
        if el.has_attribute("style") {
            let _ = el.remove_attribute("style");
        }
    }
}

/// Eingabefeld leeren (der Text ist schon gesendet).
fn clear_input(id: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
    {
        el.set_value("");
    }
}
