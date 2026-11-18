//! Einstellungen der Spracherkennung und des Vorlesens für „Sprechen“ im Chat (`call`): Mikrofon,
//! Whisper-Modell (lokal), Sprache und Stimme. Dazu die Hilfen `speech_lang` und `mic_error`.

use crate::api;
use crate::components::common::*;
use crate::components::overlay::toast_error;
use crate::i18n::{tr, trf};
use crate::mic;
use crate::state::ME;
use crate::util;
use dioxus::prelude::*;
use futures_util::StreamExt;
use nrzx_shared::voice::{SttModel, VoiceSettings};

/// Sprache für das Vorlesen (`de-DE` …).
pub(crate) fn speech_lang(language: &str) -> String {
    match language {
        "de" => "de-DE".into(),
        "en" => "en-US".into(),
        "" => match crate::i18n::LANG.read().as_str() {
            "en" => "en-US".into(),
            _ => "de-DE".into(),
        },
        l => l.to_string(),
    }
}

/// Fehler des Mikrofons als Satz. Fehlt die Erlaubnis, zeigt die App gleich die Rechte von
/// nrzX (dort führt „Einstellungen“ direkt zu Datenschutz & Sicherheit → Mikrofon).
pub(crate) fn mic_error(e: &str) -> String {
    if e == "denied" {
        util::native("perm:hint");
    }
    match e {
        "denied" if util::in_app() => tr("Kein Zugriff auf das Mikrofon. Erlaube es in den Rechten von nrzX (öffnet sich gleich) oder unter Systemeinstellungen → Datenschutz & Sicherheit → Mikrofon."),
        "denied" => tr("Kein Zugriff auf das Mikrofon – bitte im Browser erlauben."),
        "nodevice" => tr("Kein Mikrofon gefunden."),
        "unsupported" => tr("Mikrofon ist hier nicht verfügbar (nur über https oder in der App)."),
        other => trf("Mikrofon: {0}", &[&other]),
    }
}

// ------------------------------------------------------------------ Einstellungen

/// Einstellungen → Sprechen: Mikrofon, Spracherkennung (Whisper-Modell, Sprache) und Vorlesen –
/// für „Sprechen“ oben im Chat.
#[component]
pub fn DieterSettings() -> Element {
    let mut settings = use_signal(|| None::<VoiceSettings>);
    let mut models = use_signal(|| None::<Vec<SttModel>>);
    let admin = ME.read().as_ref().is_some_and(|m| m.is_admin);
    use_future(move || async move {
        match api::get::<VoiceSettings>("/voice/settings").await {
            Ok(s) => settings.set(Some(s)),
            Err(e) => toast_error(e),
        }
        // Liste laufend halten, solange ein Modell geladen wird
        loop {
            if let Ok(m) = api::get::<Vec<SttModel>>("/voice/models").await {
                let busy = m.iter().any(|x| x.downloading);
                models.set(Some(m));
                if !busy {
                    gloo_timers::future::TimeoutFuture::new(5000).await;
                    continue;
                }
            }
            gloo_timers::future::TimeoutFuture::new(1000).await;
        }
    });
    let mut save = move |s: VoiceSettings| {
        settings.set(Some(s.clone()));
        spawn(async move {
            match api::put::<VoiceSettings, _>("/voice/settings", &s).await {
                Ok(s) => settings.set(Some(s)),
                Err(e) => toast_error(e),
            }
        });
    };
    let Some(s) = settings() else {
        return rsx! { Loading {} };
    };
    rsx! {
        Section { title: tr("Mikrofon"), description: tr("Welches Mikrofon „Sprechen“ im Chat benutzt – gespeichert für dieses Gerät. Mit „Testen“ siehst du, ob es dich hört."),
            MicPicker {}
        }
        Section { title: tr("Spracherkennung"), description: tr("Whisper läuft lokal auf diesem Rechner (einmal laden, danach ohne Internet). Kleinere Modelle sind schneller, grössere genauer."),
            div { class: "grid max-w-2xl gap-2",
                for m in models().unwrap_or_default() {
                    div {
                        key: "{m.id}",
                        class: if m.id == s.stt_model { "flex items-center gap-3 rounded-lg border border-ring bg-accent/60 px-4 py-2.5" } else { "flex items-center gap-3 rounded-lg border px-4 py-2.5" },
                        button {
                            class: "flex min-w-0 flex-1 items-start gap-2.5 text-left",
                            onclick: {
                                let mut n = s.clone();
                                let id = m.id.clone();
                                move |_| {
                                    n.stt_model = id.clone();
                                    save(n.clone());
                                }
                            },
                            span { class: if m.id == s.stt_model { "mt-1 size-3 shrink-0 rounded-full border-4 border-primary" } else { "mt-1 size-3 shrink-0 rounded-full border" } }
                            span { class: "min-w-0",
                                span { class: "block text-sm font-medium", "{m.label} · " {util::size(m.size)} }
                                span { class: "block text-xs text-muted-foreground", {tr(&m.detail)} }
                                if !m.error.is_empty() {
                                    span { class: "block text-xs text-destructive", "{m.error}" }
                                }
                            }
                        }
                        if m.installed {
                            span { class: "inline-flex items-center gap-1 text-xs text-success", icons::Check { class: "size-3.5" } {tr("Geladen")} }
                            if admin {
                                button {
                                    class: "grid size-7 place-items-center rounded text-muted-foreground hover:bg-accent hover:text-destructive",
                                    title: tr("Modell löschen"),
                                    onclick: {
                                        let id = m.id.clone();
                                        move |_| {
                                            let id = id.clone();
                                            spawn(async move {
                                                match api::send::<Vec<SttModel>, _>("DELETE", &format!("/voice/models/{id}"), &()).await {
                                                    Ok(l) => models.set(Some(l)),
                                                    Err(e) => toast_error(e),
                                                }
                                            });
                                        }
                                    },
                                    icons::Trash2 { class: "size-3.5" }
                                }
                            }
                        } else if m.downloading {
                            div { class: "flex w-32 flex-col gap-1",
                                div { class: "h-1.5 overflow-hidden rounded-full bg-muted",
                                    div { class: "h-full rounded-full bg-primary transition-all", style: "width: {(m.progress * 100.0) as u32}%" }
                                }
                                span { class: "text-right text-[11px] text-muted-foreground", "{(m.progress * 100.0) as u32} %" }
                            }
                        } else {
                            button {
                                class: "inline-flex h-8 items-center gap-1.5 rounded-md border px-2.5 text-xs hover:bg-accent",
                                onclick: {
                                    let id = m.id.clone();
                                    move |_| {
                                        let id = id.clone();
                                        spawn(async move {
                                            match api::action::<Vec<SttModel>>(&format!("/voice/models/{id}/download")).await {
                                                Ok(l) => models.set(Some(l)),
                                                Err(e) => toast_error(e),
                                            }
                                        });
                                    }
                                },
                                icons::Download { class: "size-3.5" }
                                {tr("Laden")}
                            }
                        }
                    }
                }
            }
            div { class: "mt-4 flex max-w-2xl flex-wrap items-center gap-2",
                span { class: "mr-1 text-sm text-muted-foreground", {tr("Sprache:")} }
                for (code , label) in [("de", tr("Deutsch")), ("en", tr("Englisch")), ("", tr("Automatisch"))] {
                    button {
                        key: "{code}",
                        class: if s.language == code { "rounded-full bg-primary px-3 py-1 text-xs text-primary-foreground" } else { "rounded-full border px-3 py-1 text-xs hover:bg-accent" },
                        onclick: {
                            let mut n = s.clone();
                            move |_| {
                                n.language = code.to_string();
                                save(n.clone());
                            }
                        },
                        "{label}"
                    }
                }
            }
        }
        Section { title: tr("Stimme"), description: tr("Beim Sprechen im Chat liest nrzX die Antworten des Agenten mit der Sprachausgabe des Systems vor. Solange vorgelesen wird, hört das Mikrofon nicht zu."),
            div { class: "grid max-w-2xl gap-3",
                Toggle {
                    checked: s.speak,
                    onchange: {
                        let s = s.clone();
                        move |v: bool| {
                            let mut n = s.clone();
                            n.speak = v;
                            save(n);
                        }
                    },
                    label: tr("Antworten vorlesen"),
                }
                VoicePicker { settings: s.clone(), onsave: move |n: VoiceSettings| save(n) }
            }
        }
    }
}

/// Mikrofon für Dieter (je Gerät): Standard des Systems oder ein bestimmtes; Test mit Pegel.
/// An- und abgesteckte Geräte erscheinen von selbst.
#[component]
fn MicPicker() -> Element {
    // None = lädt noch
    let mut list = use_signal(|| None::<Vec<mic::Input>>);
    let mut chosen = use_signal(mic::chosen);
    let mut note = use_signal(String::new);
    let mut testing = use_signal(|| false);
    let mut level = use_signal(|| 0f32);
    let mut device = use_signal(String::new);
    let refresh = move |unlock: bool| async move {
        match mic::inputs(unlock).await {
            Ok(l) => {
                list.set(Some(l));
                note.set(String::new());
            }
            Err(e) => {
                list.set(Some(Vec::new()));
                note.set(mic_error(&e));
            }
        }
    };
    use_future(move || async move {
        let watch = mic::watch_devices();
        refresh(false).await;
        if let Some((_watch, mut changes)) = watch {
            while changes.next().await.is_some() {
                refresh(false).await;
            }
        }
    });
    use_drop(mic::probe_stop);
    // Test: gewähltes Mikrofon öffnen und den Pegel zeigen, bis „Test beenden“
    let start_test = move || {
        spawn(async move {
            match mic::probe_start().await {
                Ok(name) => {
                    device.set(name);
                    note.set(String::new());
                    if !*testing.peek() {
                        testing.set(true);
                        // Erst jetzt kennt der Browser die Namen
                        refresh(false).await;
                        let mut smooth = 0f32;
                        while *testing.peek() {
                            smooth = smooth * 0.6 + mic::probe_level() * 0.4;
                            let v = (smooth * 12.0).min(1.0);
                            if (v - *level.peek()).abs() > 0.02 {
                                level.set(v);
                            }
                            gloo_timers::future::TimeoutFuture::new(60).await;
                        }
                    }
                }
                Err(e) => {
                    mic::probe_stop();
                    testing.set(false);
                    note.set(mic_error(&e));
                }
            }
        });
    };
    let toggle = move |_| {
        if testing() {
            mic::probe_stop();
            testing.set(false);
            level.set(0.0);
        } else {
            start_test();
        }
    };
    let pick = move |e: FormEvent| {
        let id = e.value();
        let label = list.peek().as_ref().and_then(|l| l.iter().find(|d| d.id == id).map(|d| d.label.clone()));
        let label = label.unwrap_or_else(|| if chosen.peek().0 == id { chosen.peek().1.clone() } else { String::new() });
        chosen.set((id.clone(), label.clone()));
        spawn(async move {
            // Hört Dieter gerade zu, wechselt er sofort
            if let Err(e) = mic::choose(&id, &label).await {
                note.set(mic_error(&e));
            }
            if *testing.peek() {
                start_test();
            }
        });
    };
    let (cur, cur_label) = chosen();
    let loading = list().is_none();
    let devices = list().unwrap_or_default();
    // Gewähltes Gerät nicht in der Liste: abgesteckt (Liste da) oder Namen noch unbekannt
    let extra = !cur.is_empty() && !devices.iter().any(|d| d.id == cur);
    let missing = extra && !devices.is_empty();
    let extra_label = if cur_label.is_empty() { tr("Gewähltes Mikrofon") } else { cur_label.clone() };
    let unnamed = !loading && (devices.is_empty() || devices.iter().any(|d| d.label.is_empty()));
    let bar = format!("width: {:.0}%", level() * 100.0);
    rsx! {
        div { class: "grid max-w-2xl gap-2",
            div { class: "flex flex-wrap items-end gap-2",
                label { class: "grid min-w-64 flex-1 gap-1 text-xs text-muted-foreground",
                    {tr("Mikrofon")}
                    select { class: "{INPUT} h-9 text-sm text-foreground", disabled: loading, onchange: pick,
                        option { value: "", selected: cur.is_empty(), {tr("Standard des Systems")} }
                        for (i , d) in devices.iter().enumerate() {
                            option { key: "{d.id}", value: "{d.id}", selected: d.id == cur,
                                if d.label.is_empty() {
                                    {trf("Mikrofon {0}", &[&(i + 1)])}
                                } else {
                                    "{d.label}"
                                }
                            }
                        }
                        if extra {
                            option { value: "{cur}", selected: true,
                                if missing {
                                    {trf("{0} (nicht verbunden)", &[&extra_label])}
                                } else {
                                    "{extra_label}"
                                }
                            }
                        }
                    }
                }
                button { class: "inline-flex h-9 items-center gap-1.5 rounded-md border px-3 text-sm hover:bg-accent", onclick: toggle,
                    if testing() {
                        icons::MicOff { class: "size-4" }
                        {tr("Test beenden")}
                    } else {
                        icons::Mic { class: "size-4" }
                        {tr("Testen")}
                    }
                }
            }
            if testing() {
                div { class: "grid gap-1.5",
                    div { class: "h-2 overflow-hidden rounded-full bg-muted",
                        div { class: "h-full rounded-full bg-success transition-[width] duration-75", style: "{bar}" }
                    }
                    p { class: "text-xs text-muted-foreground",
                        if device().is_empty() {
                            {tr("Sprich etwas – der Balken sollte ausschlagen.")}
                        } else {
                            {trf("Sprich etwas – der Balken sollte ausschlagen. Gerät: {0}", &[&device()])}
                        }
                    }
                }
            }
            if missing {
                p { class: "text-xs text-muted-foreground", {tr("Nicht verbunden – solange nimmt Dieter den Standard des Systems.")} }
            }
            if unnamed {
                div { class: "flex flex-wrap items-center gap-2 text-xs text-muted-foreground",
                    {tr("Die Namen der Mikrofone zeigt der Browser erst nach der Erlaubnis.")}
                    button {
                        class: "rounded-md border px-2 py-1 text-foreground hover:bg-accent",
                        onclick: move |_| {
                            spawn(refresh(true));
                        },
                        {tr("Mikrofone anzeigen")}
                    }
                }
            }
            if !note().is_empty() {
                p { class: "text-xs text-destructive", "{note}" }
            }
        }
    }
}

/// Stimme fürs Vorlesen: automatisch eine männliche oder eine bestimmte Systemstimme; Probe.
#[component]
fn VoicePicker(settings: VoiceSettings, onsave: EventHandler<VoiceSettings>) -> Element {
    let lang = speech_lang(&settings.language);
    let base = lang.split('-').next().unwrap_or("de").to_string();
    // Stimmen lädt der Browser manchmal erst nach – kurz nachfragen
    let mut list = use_signal(Vec::<(String, String, bool)>::new);
    {
        let base = base.clone();
        use_future(move || {
            let base = base.clone();
            async move {
                for _ in 0..20 {
                    let l = mic::voices(&base);
                    if !l.is_empty() {
                        list.set(l);
                        break;
                    }
                    gloo_timers::future::TimeoutFuture::new(250).await;
                }
            }
        });
    }
    let auto = mic::auto_voice(&lang);
    let current = settings.voice.clone();
    let l = list();
    let probe = {
        let lang = lang.clone();
        let current = current.clone();
        move |_| {
            let text = if lang.starts_with("de") { "Hallo, ich bin Dieter. Sag mir, was die Fenster tun sollen." } else { "Hi, I am Dieter. Tell me what the windows should do." };
            mic::speak(text, &lang, &current, || {});
        }
    };
    rsx! {
        div { class: "flex flex-wrap items-end gap-2",
            label { class: "grid min-w-64 flex-1 gap-1 text-xs text-muted-foreground",
                {tr("Stimme")}
                select {
                    class: "{INPUT} h-9 text-sm text-foreground",
                    onchange: {
                        let s = settings.clone();
                        move |e: FormEvent| {
                            let mut n = s.clone();
                            n.voice = e.value();
                            onsave.call(n);
                        }
                    },
                    option { value: "", selected: current.is_empty(),
                        match auto.clone() {
                            Some(v) => trf("Automatisch – männlich ({0})", &[&v]),
                            None => tr("Automatisch – tiefer gestimmt (keine männliche Stimme installiert)"),
                        }
                    }
                    for (name , code , male) in l.iter().cloned() {
                        option { key: "{name}", value: "{name}", selected: current == name,
                            if male { "♂ {name} · {code}" } else { "{name} · {code}" }
                        }
                    }
                }
            }
            button { class: "inline-flex h-9 items-center gap-1.5 rounded-md border px-3 text-sm hover:bg-accent", onclick: probe,
                icons::Volume2 { class: "size-4" }
                {tr("Probe hören")}
            }
        }
        if auto.is_none() && current.is_empty() && !l.is_empty() {
            p { class: "mt-2 max-w-2xl text-xs text-muted-foreground",
                {tr("Tipp (macOS): Unter Systemeinstellungen → Bedienungshilfen → Gesprochene Inhalte → Systemstimme → Stimmen verwalten eine männliche Stimme wie „Markus“ oder „Yannick“ laden.")}
            }
        }
    }
}
