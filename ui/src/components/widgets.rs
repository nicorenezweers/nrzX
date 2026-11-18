//! Bausteine für alle Seiten von nrzGit: Avatar, relative Zeit, Kopierknopf, Seitennavigation,
//! Zustands- und Label-Abzeichen, Reiter, Markdown-Anzeige und Markdown-Editor mit Vorschau.

use crate::api;
use crate::components::common::*;
use crate::components::overlay::toast_error;
use crate::i18n::{tr, trf};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::{Attachment, MarkdownRequest, MarkdownResponse, UserRef};

/// Rundes Avatar-Bild (Organisationen eckig wie in Forgejo).
#[component]
pub fn UserAvatar(user: UserRef, #[props(default = 20)] size: u32, #[props(into, optional)] class: Option<String>) -> Element {
    let shape = if user.is_org { "rounded-md" } else { "rounded-full" };
    let extra = class.unwrap_or_default();
    // Eigenes Profilbild hat schon `?v=` (Version)
    let sep = if user.avatar_url.contains('?') { "&" } else { "?" };
    rsx! {
        img {
            class: "inline-block shrink-0 bg-muted object-cover {shape} {extra}",
            style: "width: {size}px; height: {size}px",
            src: "{user.avatar_url}{sep}s={size * 2}",
            alt: "{user.username}",
            loading: "lazy",
        }
    }
}

/// "vor 3 Tagen" mit genauem Datum im Tooltip.
#[component]
pub fn RelTime(ts: i64, #[props(into, optional)] class: Option<String>) -> Element {
    rsx! {
        time { class: class.unwrap_or_default(), title: util::datetime(ts), {util::ago(ts)} }
    }
}

/// Knopf "kopieren" mit kurzer Bestätigung.
#[component]
pub fn CopyButton(#[props(into)] text: String, #[props(into, optional)] class: Option<String>, #[props(into, optional)] label: Option<String>) -> Element {
    let mut copied = use_signal(|| false);
    let c = class.unwrap_or_else(|| format!("{} {}", btn(BTN_OUTLINE), BTN_ICON));
    rsx! {
        button {
            r#type: "button",
            class: "{c}",
            title: tr("Kopieren"),
            onclick: move |_| {
                if util::copy(&text) {
                    copied.set(true);
                    spawn(async move {
                        gloo_timers::future::TimeoutFuture::new(1500).await;
                        copied.set(false);
                    });
                } else {
                    crate::components::overlay::toast_error(tr("Kopieren ging nicht – bitte markieren und mit Strg/Cmd+C kopieren."));
                }
            },
            if copied() {
                icons::Check { class: "size-4" }
            } else {
                icons::Copy { class: "size-4" }
            }
            if let Some(l) = label {
                span { "{l}" }
            }
        }
    }
}

/// Seitennavigation: « 1 … 4 5 6 … 20 ».
#[component]
pub fn Pagination(page: i64, total: i64, per_page: i64, onchange: EventHandler<i64>) -> Element {
    let pages = ((total + per_page - 1) / per_page.max(1)).max(1);
    if pages <= 1 {
        return rsx! {};
    }
    let page = page.clamp(1, pages);
    let mut nums: Vec<i64> = vec![1, pages];
    for p in page - 2..=page + 2 {
        if p >= 1 && p <= pages {
            nums.push(p);
        }
    }
    nums.sort_unstable();
    nums.dedup();
    let base = "inline-flex h-8 min-w-8 items-center justify-center rounded-md px-2 text-sm";
    rsx! {
        nav { class: "mt-6 flex items-center justify-center gap-1",
            button { class: "{base} hover:bg-accent disabled:opacity-40", disabled: page <= 1, onclick: move |_| onchange.call(page - 1), title: tr("Zurück"),
                icons::ChevronLeft { class: "size-4" }
            }
            for (i , p) in nums.iter().copied().enumerate() {
                if i > 0 && p - nums[i - 1] > 1 {
                    span { class: "px-1 text-muted-foreground", "…" }
                }
                button {
                    class: if p == page { format!("{base} bg-primary font-medium text-primary-foreground") } else { format!("{base} hover:bg-accent") },
                    onclick: move |_| onchange.call(p),
                    "{p}"
                }
            }
            button { class: "{base} hover:bg-accent disabled:opacity-40", disabled: page >= pages, onclick: move |_| onchange.call(page + 1), title: tr("Weiter"),
                icons::ChevronRight { class: "size-4" }
            }
        }
    }
}

/// Zustand eines Issues/PRs: offen, geschlossen, gemergt, Entwurf.
#[component]
pub fn StateBadge(#[props(into)] state: String, #[props(default)] small: bool) -> Element {
    let (cls, label) = match state.as_str() {
        "open" => ("bg-success text-success-foreground", tr("Offen")),
        "merged" => ("bg-merged text-white", tr("Gemergt")),
        "draft" => ("bg-muted text-muted-foreground", tr("Entwurf")),
        _ => ("bg-destructive text-white", tr("Geschlossen")),
    };
    let size = if small { "px-2 py-0.5 text-[11px]" } else { "px-2.5 py-1 text-xs" };
    rsx! {
        span { class: "inline-flex items-center gap-1 rounded-full font-medium {cls} {size}",
            match state.as_str() {
                "open" => rsx! { icons::CircleDot { class: "size-3.5" } },
                "merged" => rsx! { icons::GitMerge { class: "size-3.5" } },
                "draft" => rsx! { icons::GitPullRequestDraft { class: "size-3.5" } },
                _ => rsx! { icons::CircleCheck { class: "size-3.5" } },
            }
            "{label}"
        }
    }
}

/// Zähler-Pille (z. B. neben Reitern).
#[component]
pub fn Counter(n: i64) -> Element {
    rsx! {
        span { class: "rounded-full bg-muted px-1.5 text-[11px] font-semibold leading-5 text-muted-foreground", {util::number(n)} }
    }
}

/// Ein Reiter in einer Reiterleiste (`TabBar`).
#[derive(Clone, PartialEq)]
pub struct TabItem {
    pub label: String,
    pub to: crate::Route,
    pub active: bool,
    pub count: Option<i64>,
    pub icon: Option<Element>,
}

/// Reiterleiste mit Links (unterstrichen wie in Forgejo/GitHub).
#[component]
pub fn TabBar(items: Vec<TabItem>, #[props(into, optional)] class: Option<String>) -> Element {
    let extra = class.unwrap_or_default();
    rsx! {
        nav { class: "flex gap-1 overflow-x-auto overflow-y-hidden border-b [scrollbar-width:none] [&::-webkit-scrollbar]:hidden {extra}",
            for t in items {
                Link {
                    to: t.to.clone(),
                    class: if t.active {
                        "relative flex shrink-0 items-center gap-2 px-3 py-2.5 text-sm font-medium text-foreground after:absolute after:inset-x-2 after:bottom-0 after:h-0.5 after:rounded-full after:bg-foreground [&_svg]:size-4"
                    } else {
                        "relative flex shrink-0 items-center gap-2 rounded-md px-3 py-2.5 text-sm text-muted-foreground hover:text-foreground [&_svg]:size-4"
                    },
                    if let Some(i) = t.icon.clone() {
                        {i}
                    }
                    "{t.label}"
                    if let Some(c) = t.count {
                        Counter { n: c }
                    }
                }
            }
        }
    }
}

/// Fertig gerendertes, vom Server bereinigtes Markdown.
#[component]
pub fn Markdown(#[props(into)] html: String, #[props(into, optional)] class: Option<String>) -> Element {
    let extra = class.unwrap_or_default();
    rsx! {
        div { class: "markdown {extra}", dangerous_inner_html: "{html}" }
    }
}

/// Markdown-Editor: Schreiben/Vorschau, Bilder und Dateien per Knopf oder Drag & Drop
/// (lädt als Anhang ins Repository hoch und fügt den Link ein). `repo` = "owner/name".
#[component]
pub fn MarkdownEditor(
    value: Signal<String>,
    #[props(into, optional)] repo: Option<String>,
    #[props(into, optional)] placeholder: Option<String>,
    #[props(default = 8)] rows: u32,
    #[props(default)] autofocus: bool,
) -> Element {
    let mut preview = use_signal(|| false);
    let mut html = use_signal(String::new);
    let mut uploading = use_signal(|| false);
    let repo_for_preview = repo.clone();
    let repo_for_drop = repo.clone();
    let repo_for_input = repo.clone();
    let render = move |_| {
        preview.set(true);
        let text = value();
        let _ = repo_for_preview.clone();
        spawn(async move {
            match api::post::<MarkdownResponse, _>("/markdown", &MarkdownRequest { text }).await {
                Ok(r) => html.set(r.html),
                Err(e) => toast_error(e),
            }
        });
    };
    let mut upload = move |files: Vec<web_sys::File>, repo: Option<String>| {
        let Some(repo) = repo else { return };
        if files.is_empty() {
            return;
        }
        uploading.set(true);
        spawn(async move {
            for f in files {
                match api::upload::<Attachment>(&format!("/attachments?scope={repo}"), &[f]).await {
                    Ok(a) => {
                        let is_image = a.content_type.starts_with("image/");
                        let link = if is_image { format!("![{}]({})", a.name, a.url) } else { format!("[{}]({})", a.name, a.url) };
                        let mut v = value.write();
                        if !v.is_empty() && !v.ends_with('\n') {
                            v.push('\n');
                        }
                        v.push_str(&link);
                        v.push('\n');
                    }
                    Err(e) => toast_error(e),
                }
            }
            uploading.set(false);
        });
    };
    let tab = |active: bool| {
        if active {
            "rounded-md bg-background px-3 py-1 text-[13px] font-medium shadow-xs"
        } else {
            "rounded-md px-3 py-1 text-[13px] text-muted-foreground hover:text-foreground"
        }
    };
    rsx! {
        div { class: "overflow-hidden rounded-lg border focus-within:border-ring",
            div { class: "flex items-center justify-between gap-2 border-b bg-muted/40 px-2 py-1.5",
                div { class: "inline-flex rounded-md bg-muted p-0.5",
                    button { r#type: "button", class: tab(!preview()), onclick: move |_| preview.set(false), {tr("Schreiben")} }
                    button { r#type: "button", class: tab(preview()), onclick: render, {tr("Vorschau")} }
                }
                if repo.is_some() {
                    label { class: "{btn(BTN_GHOST)} {BTN_SM} cursor-pointer text-muted-foreground",
                        title: tr("Dateien anhängen"),
                        if uploading() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        } else {
                            icons::Paperclip { class: "size-4" }
                        }
                        input {
                            r#type: "file",
                            multiple: true,
                            class: "hidden",
                            onchange: move |e| upload(util::input_files(&e), repo_for_input.clone()),
                        }
                    }
                }
            }
            if preview() {
                div { class: "min-h-32 px-4 py-3",
                    if html().trim().is_empty() {
                        p { class: "text-sm text-muted-foreground", {tr("Nichts zum Anzeigen.")} }
                    } else {
                        Markdown { html: html() }
                    }
                }
            } else {
                textarea {
                    class: "block w-full resize-y bg-transparent px-3 py-2 font-mono text-[13px] outline-none placeholder:text-muted-foreground",
                    rows: "{rows}",
                    autofocus,
                    placeholder: placeholder.unwrap_or_else(|| tr("Markdown wird unterstützt. Bilder hierher ziehen.")),
                    value: "{value}",
                    oninput: move |e| value.set(e.value()),
                    ondragover: move |e| e.prevent_default(),
                    ondrop: move |e| {
                        e.prevent_default();
                        upload(util::dropped_files(&e), repo_for_drop.clone());
                    },
                }
            }
        }
    }
}

/// Text mit Zahl in der Mehrzahl: `plural(n, "1 Commit", "{0} Commits")`.
pub fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        tr(one)
    } else {
        trf(many, &[&util::number(n)])
    }
}
