//! Kleine Helfer: Datum, Grösse, Browser-Speicher.

/// Unix-Sekunden → "24.10.2026, 14:05" in der Zeitzone des Browsers.
pub fn datetime(ts: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"dateStyle".into(), &"medium".into());
    let _ = js_sys::Reflect::set(&opts, &"timeStyle".into(), &"short".into());
    d.to_locale_string(crate::i18n::locale(), &opts).into()
}

/// Unix-Sekunden → "20:00" (heute) bzw. "Sa., 20:00" (anderer Tag).
pub fn clock(ts: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"timeStyle".into(), &"short".into());
    let today = js_sys::Date::new_0().to_date_string() == d.to_date_string();
    let time: String = d.to_locale_time_string_with_options(crate::i18n::locale(), &opts).into();
    if today {
        return time;
    }
    let wd = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&wd, &"weekday".into(), &"short".into());
    let day: String = d.to_locale_date_string(crate::i18n::locale(), &wd).into();
    format!("{day}, {time}")
}

/// Unix-Sekunden → "24.10.2026".
pub fn date(ts: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"dateStyle".into(), &"medium".into());
    d.to_locale_date_string(crate::i18n::locale(), &opts).into()
}

pub fn now() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

/// 812 B, 14,2 KB, 3,1 MB (Dezimalzeichen der gewählten Sprache)
pub fn size(bytes: i64) -> String {
    let b = bytes as f64;
    let num = |x: f64| {
        let opts = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&opts, &"minimumFractionDigits".into(), &1.into());
        let _ = js_sys::Reflect::set(&opts, &"maximumFractionDigits".into(), &1.into());
        js_sys::Intl::NumberFormat::new(&js_sys::Array::of1(&crate::i18n::locale().into()), &opts)
            .format()
            .call1(&wasm_bindgen::JsValue::NULL, &x.into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| format!("{x:.1}"))
    };
    if b >= 1_048_576.0 {
        format!("{} MB", num(b / 1_048_576.0))
    } else if b >= 1024.0 {
        format!("{} KB", num(b / 1024.0))
    } else {
        format!("{bytes} B")
    }
}

pub fn storage_get(key: &str) -> Option<String> {
    web_sys::window()?.local_storage().ok()??.get_item(key).ok()?
}

pub fn storage_remove(key: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.remove_item(key);
    }
}

pub fn storage_set(key: &str, value: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(key, value);
    }
}

/// Text in die Zwischenablage – direkt im Klick-Handler aufrufen (Safari erlaubt das Kopieren
/// nur dort). `navigator.clipboard` gibt es nur in sicheren Kontexten (HTTPS, localhost); über
/// `http://umbrel.local` fehlt es, dann der alte Weg über ein markiertes Textfeld.
pub fn copy(text: &str) -> bool {
    use wasm_bindgen::JsCast;
    let Some(w) = web_sys::window() else { return false };
    let nav = w.navigator();
    let api = w.is_secure_context() && js_sys::Reflect::get(&nav, &"clipboard".into()).is_ok_and(|c| !c.is_undefined() && !c.is_null());
    if api {
        let p = nav.clipboard().write_text(text);
        wasm_bindgen_futures::spawn_local(async move {
            let _ = wasm_bindgen_futures::JsFuture::from(p).await;
        });
        return true;
    }
    let Some(doc) = w.document() else { return false };
    let Some(body) = doc.body() else { return false };
    let Ok(el) = doc.create_element("textarea") else { return false };
    let ta: web_sys::HtmlTextAreaElement = el.unchecked_into();
    ta.set_value(text);
    let _ = ta.set_attribute("readonly", "");
    let _ = ta.set_attribute("style", "position:fixed;top:0;left:0;width:1px;height:1px;opacity:0");
    let active = doc.active_element();
    if body.append_child(&ta).is_err() {
        return false;
    }
    ta.select();
    let ok = doc.unchecked_ref::<web_sys::HtmlDocument>().exec_command("copy").unwrap_or(false);
    ta.remove();
    if let Some(a) = active.and_then(|a| a.dyn_into::<web_sys::HtmlElement>().ok()) {
        let _ = a.focus();
    }
    ok
}

/// Ganze Adresse zu einem Pfad (für Freigabelinks).
pub fn absolute(path: &str) -> String {
    let origin = web_sys::window().and_then(|w| w.location().origin().ok()).unwrap_or_default();
    format!("{origin}{path}")
}

/// Dateien aus einem <input type=file> oder Drop-Ereignis.
pub fn files_of(list: Option<web_sys::FileList>) -> Vec<web_sys::File> {
    let Some(list) = list else { return Vec::new() };
    (0..list.length()).filter_map(|i| list.get(i)).collect()
}

/// Dateien eines Ziehvorgangs: die Dateiliste, sonst die Einträge (manche Quellen – etwa
/// Bildschirmfotos aus der Vorschau oder Bilder aus Browsern – füllen nur diese).
pub fn transfer_files(dt: &web_sys::DataTransfer) -> Vec<web_sys::File> {
    let files = files_of(dt.files());
    if !files.is_empty() {
        return files;
    }
    let items = dt.items();
    (0..items.length())
        .filter_map(|i| items.get(i))
        .filter(|it| it.kind() == "file")
        .filter_map(|it| it.get_as_file().ok().flatten())
        .collect()
}

/// Werden Dateien gezogen (nicht Text oder ein Eintrag der Oberfläche)?
pub fn drags_files(dt: &web_sys::DataTransfer) -> bool {
    dt.types().iter().any(|t| t.as_string().as_deref() == Some("Files"))
}

/// Dateien aus einem Drop-Ereignis (Drag & Drop vom Desktop). Dioxus legt das Ereignis als
/// `web_sys::DragEvent` ab – ein Cast auf `web_sys::Event` liefert nichts.
pub fn dropped_files(e: &dioxus::prelude::DragEvent) -> Vec<web_sys::File> {
    e.data()
        .downcast::<web_sys::DragEvent>()
        .and_then(|d| d.data_transfer())
        .map(|dt| transfer_files(&dt))
        .unwrap_or_default()
}

/// Rohes Drag-Ereignis (für `dataTransfer`).
pub fn drag_event(e: &dioxus::prelude::DragEvent) -> Option<web_sys::DragEvent> {
    e.data().downcast::<web_sys::DragEvent>().cloned()
}

/// Neben einem Eingabefeld abgelegte Dateien: (Element-ID des Composers, Dateien).
pub static FILE_DROP: dioxus::prelude::GlobalSignal<Option<(String, Vec<web_sys::File>)>> = dioxus::prelude::Signal::global(|| None);
/// Werden gerade Dateien über das Fenster gezogen? (Eingabefelder heben sich hervor.)
pub static DRAGGING_FILES: dioxus::prelude::GlobalSignal<bool> = dioxus::prelude::Signal::global(|| false);

/// Tasten-Listener am Fenster, die mit ihrem Besitzer wieder gehen ([`WindowKeys::remove`], spätestens
/// beim Fallenlassen). Ein Listener, der ein geschlossenes Feld überlebt, griffe bei jedem
/// Tastendruck auf dessen Signale zu – ein Absturz der Oberfläche.
pub struct WindowKeys {
    list: Vec<(&'static str, wasm_bindgen::closure::Closure<dyn FnMut(web_sys::KeyboardEvent)>)>,
    gone: std::cell::Cell<bool>,
}

impl WindowKeys {
    pub fn listen(list: Vec<(&'static str, wasm_bindgen::closure::Closure<dyn FnMut(web_sys::KeyboardEvent)>)>) -> Self {
        use wasm_bindgen::JsCast;
        if let Some(w) = web_sys::window() {
            for (ev, cb) in &list {
                let _ = w.add_event_listener_with_callback(ev, cb.as_ref().unchecked_ref());
            }
        }
        Self { list, gone: std::cell::Cell::new(false) }
    }

    pub fn remove(&self) {
        use wasm_bindgen::JsCast;
        if self.gone.replace(true) {
            return;
        }
        if let Some(w) = web_sys::window() {
            for (ev, cb) in &self.list {
                let _ = w.remove_event_listener_with_callback(ev, cb.as_ref().unchecked_ref());
            }
        }
    }
}

impl Drop for WindowKeys {
    fn drop(&mut self) {
        self.remove();
    }
}

thread_local! {
    static DRAG_END: std::cell::RefCell<Option<gloo_timers::callback::Timeout>> = const { std::cell::RefCell::new(None) };
}

/// Dateien überall im Fenster annehmen (einmal beim Start): Das WebView öffnet eine daneben
/// abgelegte Datei nie selbst (WebView2 zeigte sonst das Bild statt der App), sie geht an das
/// nächste sichtbare Eingabefeld. Eigene Ablageflächen (Composer, Notizen) behandeln
/// ihre Drops vorher selbst.
pub fn install_file_drop() {
    use dioxus::prelude::*;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;
    let Some(win) = web_sys::window() else { return };
    let over = Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |e: web_sys::DragEvent| {
        let Some(dt) = e.data_transfer() else { return };
        if !drags_files(&dt) {
            return;
        }
        e.prevent_default();
        dt.set_drop_effect("copy");
        if !*DRAGGING_FILES.peek() {
            *DRAGGING_FILES.write() = true;
        }
        // `dragover` kommt laufend, solange gezogen wird – bleibt es aus, ist der Zug vorbei
        // (verlassen oder abgebrochen; `dragleave` ist dafür in WebKit zu unzuverlässig)
        let t = gloo_timers::callback::Timeout::new(350, || {
            if *DRAGGING_FILES.peek() {
                *DRAGGING_FILES.write() = false;
            }
        });
        DRAG_END.with(|d| *d.borrow_mut() = Some(t));
    });
    let drop = Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |e: web_sys::DragEvent| {
        DRAG_END.with(|d| *d.borrow_mut() = None);
        if *DRAGGING_FILES.peek() {
            *DRAGGING_FILES.write() = false;
        }
        let Some(dt) = e.data_transfer() else { return };
        if !drags_files(&dt) {
            return;
        }
        let handled = e.default_prevented();
        e.prevent_default();
        if handled {
            return;
        }
        let files = transfer_files(&dt);
        if files.is_empty() {
            return;
        }
        if let Some(id) = nearest_composer(e.client_x() as f64, e.client_y() as f64) {
            *FILE_DROP.write() = Some((id, files));
        }
    });
    let _ = win.add_event_listener_with_callback("dragover", over.as_ref().unchecked_ref());
    let _ = win.add_event_listener_with_callback("drop", drop.as_ref().unchecked_ref());
    over.forget();
    drop.forget();
}

/// Sichtbares Eingabefeld (`data-composer`) am nächsten zum Punkt.
fn nearest_composer(x: f64, y: f64) -> Option<String> {
    use wasm_bindgen::JsCast;
    let doc = web_sys::window()?.document()?;
    let list = doc.query_selector_all("[data-composer]").ok()?;
    let mut best: Option<(f64, String)> = None;
    for i in 0..list.length() {
        let Some(el) = list.get(i).and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok()) else { continue };
        if el.offset_parent().is_none() {
            continue;
        }
        let r = el.get_bounding_client_rect();
        let dx = (r.left() - x).max(x - r.right()).max(0.0);
        let dy = (r.top() - y).max(y - r.bottom()).max(0.0);
        let d = dx * dx + dy * dy;
        if best.as_ref().is_none_or(|(b, _)| d < *b) {
            if let Some(id) = el.get_attribute("data-composer") {
                best = Some((d, id));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// Dateien aus einem <input type=file> (onchange); leert das Feld danach.
pub fn input_files(e: &dioxus::prelude::FormEvent) -> Vec<web_sys::File> {
    use wasm_bindgen::JsCast;
    let input = e
        .data()
        .downcast::<web_sys::Event>()
        .and_then(|ev| ev.target())
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
    match input {
        Some(i) => {
            let files = files_of(i.files());
            i.set_value("");
            files
        }
        None => Vec::new(),
    }
}

/// Im obersten Dialog das erste Eingabefeld fokussieren (sonst die Fläche dahinter, für Esc).
pub fn focus_dialog() {
    use wasm_bindgen::JsCast;
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    let Ok(list) = doc.query_selector_all("[role=dialog]") else { return };
    if list.length() == 0 {
        return;
    }
    let Some(dialog) = list.item(list.length() - 1).and_then(|n| n.dyn_into::<web_sys::Element>().ok()) else { return };
    let target = dialog
        .query_selector("[autofocus], input:not([type=hidden]):not([readonly]):not([disabled]), textarea:not([readonly]), select:not([disabled])")
        .ok()
        .flatten()
        .or_else(|| dialog.parent_element());
    if let Some(el) = target.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
        let _ = el.focus();
    }
}

/// "vor 3 Minuten", "vor 2 Tagen", ältere Daten als Datum.
pub fn ago(ts: i64) -> String {
    use crate::i18n::{tr, trf};
    let d = now() - ts;
    if d < 60 {
        tr("gerade eben")
    } else if d < 3600 {
        let m = d / 60;
        if m == 1 { tr("vor 1 Minute") } else { trf("vor {0} Minuten", &[&m]) }
    } else if d < 86_400 {
        let h = d / 3600;
        if h == 1 { tr("vor 1 Stunde") } else { trf("vor {0} Stunden", &[&h]) }
    } else if d < 30 * 86_400 {
        let t = d / 86_400;
        if t == 1 { tr("gestern") } else { trf("vor {0} Tagen", &[&t]) }
    } else {
        date(ts)
    }
}

/// Zahl mit Tausendertrennzeichen der Sprache (1’234 / 1,234).
pub fn number(n: i64) -> String {
    let f = js_sys::Intl::NumberFormat::new(&js_sys::Array::of1(&crate::i18n::locale().into()), &js_sys::Object::new());
    f.format()
        .call1(&wasm_bindgen::JsValue::NULL, &(n as f64).into())
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| n.to_string())
}

/// Aktuelle Adresse (Pfad + Suche) des Browsers.
pub fn current_path() -> String {
    web_sys::window()
        .map(|w| {
            let l = w.location();
            format!("{}{}", l.pathname().unwrap_or_default(), l.search().unwrap_or_default())
        })
        .unwrap_or_default()
}

/// Reichhaltig kopieren (im Klick-Handler aufrufen): Klartext, HTML (mit Bildern als `<img>`)
/// und optional eigene Daten (`application/x-nrzx`, z. B. Anhänge für den Composer). Läuft
/// über ein einmaliges `copy`-Ereignis – geht auch ohne HTTPS.
pub fn copy_rich(text: &str, html: &str, custom: Option<&str>) -> bool {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return false };
    let (text, html, custom) = (text.to_string(), html.to_string(), custom.map(String::from));
    let handler = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
        if let Ok(ce) = e.clone().dyn_into::<web_sys::ClipboardEvent>() {
            if let Some(dt) = ce.clipboard_data() {
                let _ = dt.set_data("text/plain", &text);
                let _ = dt.set_data("text/html", &html);
                if let Some(c) = &custom {
                    let _ = dt.set_data("application/x-nrzx", c);
                }
                e.prevent_default();
            }
        }
    });
    let _ = doc.add_event_listener_with_callback("copy", handler.as_ref().unchecked_ref());
    let ok = doc.unchecked_ref::<web_sys::HtmlDocument>().exec_command("copy").unwrap_or(false);
    let _ = doc.remove_event_listener_with_callback("copy", handler.as_ref().unchecked_ref());
    drop(handler);
    ok
}

/// HTML-Zeichen maskieren.
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Nachricht an die native App (macOS/Windows: `window.ipc`); `false` im Browser.
pub fn native(msg: &str) -> bool {
    use wasm_bindgen::JsCast;
    let Some(w) = web_sys::window() else { return false };
    let Ok(ipc) = js_sys::Reflect::get(&w, &"ipc".into()) else { return false };
    if ipc.is_undefined() || ipc.is_null() {
        return false;
    }
    let Ok(f) = js_sys::Reflect::get(&ipc, &"postMessage".into()).and_then(|f| f.dyn_into::<js_sys::Function>().map_err(Into::into)) else {
        return false;
    };
    f.call1(&ipc, &msg.into()).is_ok()
}

/// Liegt Milchglas hinter dem Fenster der App? (`window.__nrzGlass`: macOS Vibrancy bzw.
/// unter Windows Acryl oder Weichzeichnen)
pub fn glass_available() -> bool {
    web_sys::window().and_then(|w| js_sys::Reflect::get(&w, &"__nrzGlass".into()).ok()).is_some_and(|v| v.is_truthy())
}

/// Mac (für Tastenkürzel wie ⌘,)?
pub fn is_mac() -> bool {
    web_sys::window().and_then(|w| w.navigator().user_agent().ok()).is_some_and(|ua| ua.contains("Mac"))
}

/// Tastenkürzel zum Anzeigen: am Mac wie geschrieben (`⌘Z`, `⇧⌘Z`), sonst mit Strg (`Strg+Z`,
/// `Strg+Umschalt+Z`) – unter Windows und Linux gibt es keine ⌘-Taste.
pub fn keys(s: &str) -> String {
    if is_mac() {
        return s.to_string();
    }
    s.replace("⇧⌘", "Strg+Umschalt+").replace('⌘', "Strg+")
}

/// Läuft die Oberfläche in der App (nicht im Browser)?
pub fn in_app() -> bool {
    web_sys::window().and_then(|w| js_sys::Reflect::get(&w, &"ipc".into()).ok()).is_some_and(|v| !v.is_undefined() && !v.is_null())
}

/// Ladekreise ruckelfrei: Ein Kreis, der neu ins DOM kommt (Seitenleiste sortiert um, Liste
/// rendert neu), begänne sonst wieder bei 0° – sichtbares Springen, solange ein Agent arbeitet.
/// Darum bekommt jeder neue Kreis eine negative Verzögerung passend zur Seitenzeit: alle drehen im
/// Gleichtakt weiter, egal wann sie eingehängt wurden. (Einmal beim Start.)
pub fn install_spin_sync() {
    let _ = js_sys::eval(
        r#"(function () {
  if (window.__nrzSpin) return; window.__nrzSpin = true;
  var SEL = '[class*="animate-spin"],[class*="animate-[spin"]';
  function sync(el) {
    var d = parseFloat(getComputedStyle(el).animationDuration) || 1;
    el.style.animationDelay = '-' + ((performance.now() / 1000) % d).toFixed(3) + 's';
  }
  function scan(n) {
    if (n.nodeType !== 1) return;
    if (n.matches && n.matches(SEL)) sync(n);
    if (n.querySelectorAll) { var l = n.querySelectorAll(SEL); for (var i = 0; i < l.length; i++) sync(l[i]); }
  }
  new MutationObserver(function (records) {
    for (var r = 0; r < records.length; r++) {
      var rec = records[r];
      if (rec.type === 'attributes') { var t = rec.target; if (t.matches && t.matches(SEL) && !t.style.animationDelay) sync(t); continue; }
      for (var i = 0; i < rec.addedNodes.length; i++) scan(rec.addedNodes[i]);
    }
  }).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['class'] });
  scan(document.documentElement);
})();"#,
    );
}
