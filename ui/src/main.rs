//! nrzX – Oberfläche (Dioxus 0.7, WebAssembly) mit Komponenten von rust-ui.
//! Aufbau wie ein Coding-Agent-Client: links Projekte und Threads, in der Mitte die
//! Unterhaltung mit dem Agenten und der Composer, rechts Umgebung, Git, Vorschau und Subagenten.
//!
//! Seiten je Bereich: `pages::workspace` (Rahmen, Anmeldung, Projekte, Thread-Seite mit rechter
//! Spalte, Einstellungen), `pages::agents` (Unterhaltung, Composer, Subagenten, Agent-Konten,
//! Werkzeuge, Terminal).

mod api;
mod components;
mod events;
mod i18n;
mod mic;
mod notify;
mod popup;
mod pages;
mod sound;
mod state;
mod ui;
mod util;

use components::overlay::{ConfirmHost, ImageViewerHost, Toaster};
use dioxus::prelude::*;
use pages::workspace::*;

static CSS: Asset = asset!("/assets/tailwind.css");
static ICON: Asset = asset!("/assets/icon.png");
static FAVICON: Asset = asset!("/assets/favicon-32.png");

#[derive(Routable, Clone, PartialEq, Debug)]
#[rustfmt::skip]
pub enum Route {
    #[route("/user/login?:next")]
    Login { next: String },
    #[route("/install")]
    Setup {},
    #[layout(AppLayout)]
        // Startseite: Projekt wählen und neuen Thread beginnen
        #[route("/")]
        Home {},
        // Thread: Unterhaltung (agents::ChatView) und rechte Spalte (Umgebung, Git, Vorschau,
        // Subagenten über agents::Lineage)
        #[route("/t/:id")]
        ThreadPage { id: i64 },
        // Bot-Modus: Übersicht der Bots und Gespräch mit einem Bot
        #[route("/bots")]
        Bots {},
        #[route("/bots/:id")]
        BotPage { id: i64 },
        // Deep Research: Frage eingeben, Research Paper mit Gespräch darüber
        #[route("/research")]
        Research {},
        #[route("/research/:id")]
        ResearchPage { id: i64 },
        #[route("/projects")]
        Projects {},
        #[route("/projects/:id")]
        ProjectPage { id: i64 },
        // section: profile | usage | nrzgit | agents | chains | tools | dieter | appearance | app |
        // users | instance
        #[route("/settings/:section")]
        Settings { section: String },
    #[end_layout]
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

fn main() {
    install_panic_hook();
    dioxus::launch(App);
}

/// Nur wenn die Oberfläche in einer Minute dreimal abstürzt: kleiner Hinweis im Stil der App mit
/// „Neu starten“ statt einer Schleife aus Neuladen (reines JavaScript – Dioxus reagiert nach einem
/// Absturz nicht mehr).
const CRASH_NOTICE: &str = "(function(){if(document.getElementById('nrz-crash'))return;var d=document.createElement('div');d.id='nrz-crash';\
d.style.cssText='position:fixed;left:50%;top:16px;transform:translateX(-50%);z-index:2147483647;display:flex;gap:12px;align-items:center;background:rgba(36,36,38,.92);-webkit-backdrop-filter:blur(32px) saturate(140%);backdrop-filter:blur(32px) saturate(140%);color:#f4f4f5;border:1px solid rgba(255,255,255,.13);border-radius:14px;padding:8px 8px 8px 16px;font:13px -apple-system,system-ui,sans-serif;box-shadow:0 12px 40px rgba(0,0,0,.45)';\
d.textContent='Die Oberfläche hängt.';var b=document.createElement('button');b.textContent='Neu starten';\
b.style.cssText='font:inherit;cursor:default;border-radius:9px;padding:5px 12px;border:1px solid rgba(255,255,255,.14);background:rgba(255,255,255,.08);color:#f4f4f5';\
b.onclick=function(){try{sessionStorage.removeItem('nrz-crashes')}catch(e){}location.reload()};d.appendChild(b);document.body.appendChild(d)})()";

/// Nach einem Absturz (Rust-Panic) reagiert die Oberfläche nicht mehr – wasm bricht ab. Die Meldung
/// geht ins Protokoll der App (`POST /client/error`, `~/Library/Logs/nrzX/nrzx.log`), und die
/// Oberfläche startet still auf derselben Seite neu (der Stand liegt auf dem Server) – ohne
/// Hinweis. Nur bei drei Abstürzen in einer Minute bleibt sie stehen ([`CRASH_NOTICE`]).
fn install_panic_hook() {
    static CRASHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{info}");
        web_sys::console::error_1(&format!("nrzX: {msg}").into());
        // Folgefehler desselben Absturzes zählen nicht noch einmal
        if CRASHED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        // Reines JavaScript und erst nach dem Absturz: nichts davon braucht den (kaputten) Zustand
        let report = js_sys::Function::new_with_args("m", "try{navigator.sendBeacon('/api/v1/client/error',new Blob([m],{type:'text/plain'}))}catch(e){}");
        let _ = report.call1(&wasm_bindgen::JsValue::NULL, &wasm_bindgen::JsValue::from_str(&msg));
        let Some(w) = web_sys::window() else { return };
        let now = js_sys::Date::now();
        let store = w.session_storage().ok().flatten();
        let mut recent: Vec<f64> = store
            .as_ref()
            .and_then(|s| s.get_item("nrz-crashes").ok().flatten())
            .unwrap_or_default()
            .split(',')
            .filter_map(|v| v.parse::<f64>().ok())
            .filter(|t| now - t < 60_000.0)
            .collect();
        recent.push(now);
        if let Some(s) = &store {
            let _ = s.set_item("nrz-crashes", &recent.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(","));
        }
        let script = if recent.len() < 3 { "location.reload()" } else { CRASH_NOTICE };
        let f = js_sys::Function::new_no_args(script);
        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&f, 30);
    }));
}

#[component]
fn App() -> Element {
    let mut ready = use_signal(|| false);
    use_hook(notify::install);
    use_hook(util::install_file_drop);
    use_hook(util::install_spin_sync);
    use_hook(popup::install);
    use_hook(sound::install);
    use_hook(components::overlay::install_image_clicks);
    // Milchglas der App, ⌘, / Strg+, für die Einstellungen, Update-Stand der App für den Knopf
    // unten in der Seitenleiste; ein festes, dunkles Thema
    use_hook(state::apply_glass);
    use_hook(state::install_settings_shortcut);
    use_hook(pages::workspace::update::install);
    use_hook(state::apply_theme);
    use_future(move || async move {
        i18n::init().await;
        ready.set(true);
    });
    rsx! {
        document::Stylesheet { href: CSS }
        document::Stylesheet { href: concat!("/static/highlight.css?v=", env!("CARGO_PKG_VERSION")) }
        document::Link { rel: "icon", r#type: "image/png", href: FAVICON }
        document::Link { rel: "apple-touch-icon", href: ICON }
        document::Meta { name: "theme-color", content: "#1f1f1f" }
        if ready() {
            Router::<Route> {}
        }
        ConfirmHost {}
        ImageViewerHost {}
        Toaster {}
    }
}
