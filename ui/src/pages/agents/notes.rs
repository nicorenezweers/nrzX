//! Notizblock an der Seite: Prompts vorbereiten und Ideen sammeln, ohne sie gleich in ein
//! Gespräch zu schicken. Jede Notiz lässt sich bearbeiten, in den Composer übernehmen, direkt an
//! den offenen Thread senden, kopieren oder löschen. Gespeichert auf dem Server (je Benutzer,
//! optional einem Projekt zugeordnet). Bilder hängen über das Anhang-System des Composers an
//! (Büroklammer, Ziehen & Ablegen, Einfügen) und gehen beim Übernehmen in den Composer mit.

use super::composer::{pasted_files, upload_all};
use super::data::{reload_notes, ComposerDraft, COMPOSER_DRAFT, NOTES};
use crate::api;
use crate::components::overlay::toast_error;
use crate::i18n::{tr, trf};
use crate::state::{self, PROJECTS, THREADS};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::agents::{NoteInput, PostMessage, PromptNote};
use nrzx_shared::{Attachment, ThreadSummary};

/// Nur Bilder behalten (andere Dateien gehören nicht in Notizen).
fn only_images(list: Vec<web_sys::File>) -> Vec<web_sys::File> {
    let total = list.len();
    let images: Vec<web_sys::File> = list.into_iter().filter(|f| f.type_().starts_with("image/")).collect();
    if images.len() < total {
        toast_error(tr("Nur Bilder lassen sich an Notizen hängen."));
    }
    images
}

/// Anhänge einer Notiz setzen (Reihenfolge = Liste) und den Notizblock aktualisieren.
async fn set_attachments(id: i64, uuids: Vec<String>) {
    match api::patch::<PromptNote, _>(&format!("/notes/{id}"), &NoteInput { attachments: Some(uuids), ..Default::default() }).await {
        Ok(n) => replace_note(n),
        Err(e) => toast_error(e),
    }
}

fn replace_note(n: PromptNote) {
    if let Some(list) = NOTES.write().as_mut() {
        if let Some(x) = list.iter_mut().find(|x| x.id == n.id) {
            *x = n;
        }
    }
}

/// Bilder hochladen und an eine bestehende Notiz hängen.
async fn add_images(id: i64, list: Vec<web_sys::File>, mut uploading: Signal<bool>) {
    let list = only_images(list);
    if list.is_empty() {
        return;
    }
    uploading.set(true);
    let mut added = Vec::new();
    for f in list {
        match api::upload::<Attachment>("/attachments", &[f]).await {
            Ok(a) => added.push(a.uuid),
            Err(e) => toast_error(e),
        }
    }
    if !added.is_empty() {
        let mut uuids: Vec<String> = NOTES.peek().as_ref().and_then(|l| l.iter().find(|x| x.id == id).map(|n| n.attachments.iter().map(|a| a.uuid.clone()).collect())).unwrap_or_default();
        uuids.extend(added);
        set_attachments(id, uuids).await;
    }
    uploading.set(false);
}

/// Vorschaubilder; Klick vergrößert, das Kreuz entfernt das Bild (`onremove`).
#[component]
fn Thumbs(attachments: Vec<Attachment>, size: &'static str, onremove: EventHandler<String>) -> Element {
    // Alle Bilder der Notiz: im Betrachter mit ← / → blättern
    let all: Vec<(String, String)> = attachments.iter().map(|x| (x.url.clone(), x.name.clone())).collect();
    rsx! {
        div { class: "flex flex-wrap gap-2",
            for (idx, a) in attachments.into_iter().enumerate() {
                div { key: "{a.uuid}", class: "group/thumb relative",
                    button {
                        class: "block overflow-hidden rounded-lg border hover:border-ring/60",
                        title: tr("Vergrößern"),
                        onclick: {
                            let all = all.clone();
                            move |_| crate::components::overlay::view_images(all.clone(), idx)
                        },
                        img { class: "{size} object-cover", src: "{a.url}", alt: "{a.name}", loading: "lazy" }
                    }
                    button {
                        class: "absolute -top-1.5 -right-1.5 hidden size-5 place-items-center rounded-full border bg-background text-muted-foreground group-hover/thumb:grid hover:text-foreground",
                        title: tr("Entfernen"),
                        onclick: {
                            let uuid = a.uuid.clone();
                            move |_| onremove.call(uuid.clone())
                        },
                        icons::X { class: "size-3" }
                    }
                }
            }
        }
    }
}

/// Bild groß ansehen; Klick daneben oder Escape schließt.
/// Notizblock. `thread_id`: offener Thread (für „Senden“ und „In Composer“), sonst der
/// Composer für einen neuen Thread; `project_id`: Projekt für neue Notizen und den Filter.
#[component]
pub fn NotesPanel(thread_id: Option<i64>, project_id: Option<i64>) -> Element {
    let mut draft = use_signal(String::new);
    let mut only_project = use_signal(|| util::storage_get("nrz-notes-project").as_deref() == Some("1"));
    let mut busy = use_signal(|| false);
    let mut draft_files = use_signal(Vec::<Attachment>::new);
    let uploading = use_signal(|| false);
    use_hook(|| {
        if NOTES.peek().is_none() {
            spawn(reload_notes());
        }
    });
    let add = move || {
        let text = draft().trim().to_string();
        let images: Vec<String> = draft_files().iter().map(|a| a.uuid.clone()).collect();
        if (text.is_empty() && images.is_empty()) || busy() || uploading() {
            return;
        }
        busy.set(true);
        spawn(async move {
            match api::post::<PromptNote, _>("/notes", &NoteInput { text: Some(text), project_id, clear_project: false, attachments: Some(images) }).await {
                Ok(n) => {
                    NOTES.write().get_or_insert_with(Vec::new).insert(0, n);
                    draft.set(String::new());
                    draft_files.set(Vec::new());
                }
                Err(e) => toast_error(e),
            }
            busy.set(false);
        });
    };
    let mut add2 = add;
    let list: Vec<PromptNote> = NOTES
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|n| !only_project() || project_id.is_none() || n.project_id == project_id)
        .collect();
    let project_name = project_id.and_then(|p| PROJECTS.read().get(&p).map(|p| p.name.clone()));
    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col",
            div { class: "shrink-0 p-3 pb-2",
                div {
                    class: "rounded-xl border bg-card focus-within:border-ring/60",
                    ondragover: move |e| e.prevent_default(),
                    ondrop: move |e| {
                        e.prevent_default();
                        spawn(upload_all(only_images(util::dropped_files(&e)), draft_files, uploading));
                    },
                    if !draft_files().is_empty() {
                        div { class: "px-3 pt-3",
                            Thumbs {
                                attachments: draft_files(),
                                size: "size-14",
                                onremove: move |uuid: String| draft_files.write().retain(|x| x.uuid != uuid),
                            }
                        }
                    }
                    textarea {
                        class: "block max-h-60 min-h-[4.5rem] w-full resize-y bg-transparent px-3 pt-2.5 pb-1 text-[13.5px] leading-relaxed outline-none placeholder:text-muted-foreground",
                        rows: "3",
                        placeholder: tr("Idee oder Prompt vorbereiten … (Strg+Enter merkt)"),
                        value: "{draft}",
                        oninput: move |e| draft.set(e.value()),
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter && (e.modifiers().ctrl() || e.modifiers().meta()) {
                                e.prevent_default();
                                add2();
                            }
                        },
                        onpaste: move |e: ClipboardEvent| {
                            let list = pasted_files(&e);
                            if !list.is_empty() {
                                e.prevent_default();
                                spawn(upload_all(only_images(list), draft_files, uploading));
                            }
                        },
                    }
                    div { class: "flex items-center gap-2 px-2 pb-2",
                        label { class: "grid size-7 shrink-0 cursor-pointer place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground",
                            title: tr("Bilder anhängen"),
                            if uploading() {
                                icons::LoaderCircle { class: "size-3.5 animate-spin" }
                            } else {
                                icons::Paperclip { class: "size-3.5" }
                            }
                            input {
                                r#type: "file",
                                accept: "image/*",
                                multiple: true,
                                class: "hidden",
                                onchange: move |e| {
                                    spawn(upload_all(only_images(util::input_files(&e)), draft_files, uploading));
                                },
                            }
                        }
                    }
                }
                // Speichern unter dem Eingabefeld, nicht darin
                div { class: "mt-2 flex items-center gap-2 px-1",
                    if let Some(n) = project_name.clone() {
                        span { class: "min-w-0 truncate text-[11px] text-muted-foreground", {trf("für {0}", &[&n])} }
                    }
                    button {
                        class: "ml-auto flex h-7 items-center gap-1.5 rounded-full bg-primary px-3 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-40",
                        disabled: (draft().trim().is_empty() && draft_files().is_empty()) || busy() || uploading(),
                        onclick: move |_| {
                            let mut a = add;
                            a();
                        },
                        icons::Plus { class: "size-3.5" }
                        {tr("Merken")}
                    }
                }
                if project_id.is_some() {
                    label { class: "mt-2 flex cursor-pointer items-center gap-2 px-1 text-xs text-muted-foreground",
                        input {
                            r#type: "checkbox",
                            class: "accent-primary",
                            checked: only_project(),
                            onchange: move |e| {
                                let v = e.checked();
                                only_project.set(v);
                                util::storage_set("nrz-notes-project", if v { "1" } else { "0" });
                            },
                        }
                        {tr("Nur Notizen dieses Projekts")}
                    }
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto px-3 pb-3",
                if NOTES.read().is_none() {
                    p { class: "py-4 text-center text-xs text-muted-foreground", {tr("Lädt …")} }
                } else if list.is_empty() {
                    div { class: "py-8 text-center text-xs text-muted-foreground",
                        icons::StickyNote { class: "mx-auto mb-2 size-6 opacity-60" }
                        p { {tr("Noch keine Notizen. Sammle hier Ideen und Prompts – gesendet wird erst, wenn du willst.")} }
                    }
                }
                div { class: "grid gap-2",
                    for n in list {
                        NoteCard { key: "{n.id}", note: n, thread_id }
                    }
                }
            }
        }
    }
}

#[component]
fn NoteCard(note: PromptNote, thread_id: Option<i64>) -> Element {
    let id = note.id;
    let mut editing = use_signal(|| false);
    let mut text = use_signal(|| note.text.clone());
    let mut expanded = use_signal(|| false);
    let uploading = use_signal(|| false);
    let thread = thread_id.and_then(|t| THREADS.read().get(&t).cloned());
    let can_send = thread.as_ref().is_some_and(|t| !t.status.is_active()) && !(note.text.trim().is_empty() && note.attachments.is_empty());
    let project = note.project_id.and_then(|p| PROJECTS.read().get(&p).map(|p| p.name.clone()));
    let save = move || {
        let body = text().clone();
        editing.set(false);
        spawn(async move {
            match api::patch::<PromptNote, _>(&format!("/notes/{id}"), &NoteInput { text: Some(body), ..Default::default() }).await {
                Ok(n) => replace_note(n),
                Err(e) => toast_error(e),
            }
        });
    };
    let to_composer = {
        let t = note.text.clone();
        let atts = note.attachments.clone();
        move |_| {
            *COMPOSER_DRAFT.write() = Some(ComposerDraft { thread_id, text: t.clone(), attachments: atts.clone() });
        }
    };
    let send = {
        let t = note.text.clone();
        let uuids: Vec<String> = note.attachments.iter().map(|a| a.uuid.clone()).collect();
        move |_| {
            let Some(tid) = thread_id else { return };
            let body = t.clone();
            let attachments = uuids.clone();
            spawn(async move {
                match api::post::<ThreadSummary, _>(&format!("/threads/{tid}/messages"), &PostMessage { text: body, attachments, interrupt: false }).await {
                    Ok(s) => state::upsert_thread(s),
                    Err(e) => toast_error(e),
                }
            });
        }
    };
    let copy = {
        let t = note.text.clone();
        move |_| {
            util::copy(&t);
        }
    };
    let remove = move |_| {
        spawn(async move {
            match api::delete(&format!("/notes/{id}")).await {
                Ok(()) => {
                    if let Some(list) = NOTES.write().as_mut() {
                        list.retain(|x| x.id != id);
                    }
                }
                Err(e) => toast_error(e),
            }
        });
    };
    const ACT: &str = "grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-30 [&_svg]:size-3.5";
    let long = note.text.lines().count() > 6 || note.text.len() > 400;
    let remove_image = {
        let current: Vec<String> = note.attachments.iter().map(|a| a.uuid.clone()).collect();
        move |uuid: String| {
            let rest: Vec<String> = current.iter().filter(|u| **u != uuid).cloned().collect();
            spawn(set_attachments(id, rest));
        }
    };
    rsx! {
        div {
            class: "group rounded-xl border bg-card/60 px-3 py-2.5",
            ondragover: move |e| e.prevent_default(),
            ondrop: move |e| {
                e.prevent_default();
                spawn(add_images(id, util::dropped_files(&e), uploading));
            },
            if !note.attachments.is_empty() {
                div { class: "mb-2",
                    Thumbs { attachments: note.attachments.clone(), size: "size-12", onremove: remove_image }
                }
            }
            if editing() {
                textarea {
                    class: "block max-h-80 min-h-24 w-full resize-y rounded-md bg-background/50 p-2 text-[13px] leading-relaxed outline-none ring-1 ring-ring/50",
                    autofocus: true,
                    value: "{text}",
                    oninput: move |e| text.set(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter && (e.modifiers().ctrl() || e.modifiers().meta()) {
                            e.prevent_default();
                            let mut s = save;
                            s();
                        } else if e.key() == Key::Escape {
                            editing.set(false);
                        }
                    },
                    onpaste: move |e: ClipboardEvent| {
                        let list = pasted_files(&e);
                        if !list.is_empty() {
                            e.prevent_default();
                            spawn(add_images(id, list, uploading));
                        }
                    },
                }
                div { class: "mt-2 flex justify-end gap-2 text-xs",
                    button { class: "text-muted-foreground hover:text-foreground", onclick: move |_| editing.set(false), {tr("Abbrechen")} }
                    button {
                        class: "rounded-full bg-primary px-3 py-1 font-medium text-primary-foreground hover:bg-primary/90",
                        onclick: move |_| {
                            let mut s = save;
                            s();
                        },
                        {tr("Speichern")}
                    }
                }
            } else {
                button {
                    class: if expanded() || !long { "block w-full whitespace-pre-wrap break-words text-left text-[13px] leading-relaxed" } else { "line-clamp-6 block w-full whitespace-pre-wrap break-words text-left text-[13px] leading-relaxed" },
                    title: tr("Klicken zum Bearbeiten"),
                    onclick: move |_| {
                        text.set(note.text.clone());
                        editing.set(true);
                    },
                    "{note.text}"
                }
                if long {
                    button { class: "mt-1 text-[11px] text-muted-foreground hover:text-foreground", onclick: move |_| expanded.toggle(),
                        if expanded() { {tr("weniger")} } else { {tr("mehr")} }
                    }
                }
                div { class: "mt-1.5 flex items-center gap-0.5",
                    span { class: "min-w-0 flex-1 truncate text-[11px] text-muted-foreground/80",
                        {util::ago(note.updated_at)}
                        if let Some(p) = project {
                            " · {p}"
                        }
                    }
                    label { class: "{ACT} cursor-pointer", title: tr("Bilder anhängen"),
                        if uploading() {
                            icons::LoaderCircle { class: "animate-spin" }
                        } else {
                            icons::Paperclip {}
                        }
                        input {
                            r#type: "file",
                            accept: "image/*",
                            multiple: true,
                            class: "hidden",
                            onchange: move |e| {
                                spawn(add_images(id, util::input_files(&e), uploading));
                            },
                        }
                    }
                    button { class: ACT, title: tr("In den Composer übernehmen"), onclick: to_composer, icons::CornerDownLeft {} }
                    if thread_id.is_some() {
                        button { class: ACT, title: if can_send { tr("An diesen Thread senden") } else { tr("Erst wenn der Agent fertig ist") }, disabled: !can_send, onclick: send, icons::Send {} }
                    }
                    button { class: ACT, title: tr("Kopieren"), onclick: copy, icons::Copy {} }
                    button { class: "{ACT} hover:text-destructive", title: tr("Löschen"), onclick: remove, icons::Trash2 {} }
                }
            }
        }
    }
}
