//! Nutzung der Agent-Konten: unten in der Aufgabenleiste (höchster Verbrauch der Limits;
//! ein Klick zeigt die Limits je Konto mit Kosten bzw. Tokens von heute) und ausführlich unter
//! Einstellungen → Nutzung bzw. im Fenster der Menüleiste ([`UsageBoard`]): Kosten und Tokens über 24 Stunden bis 90 Tage mit Verlauf je
//! Agent, die Limits der Abos mit Zeitpunkt des Zurücksetzens und was nrzX je Konto gezählt
//! hat. Lädt jede Minute neu, sobald sich die Zahl der laufenden Agenten ändert und beim Öffnen.

use super::data::{duration, use_clock};
use super::icons::AgentIcon;
use crate::api;
use crate::components::common::*;
use crate::i18n::{tr, trf};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::agents::{AccountUsage, UsageLimit, UsageOverview, UsageWindow};
use nrzx_shared::workspace::{TokenSums, UsagePoint, UsageReport};
use nrzx_shared::AgentKind;
use wasm_bindgen::JsValue;

/// Letzter Stand von `GET /usage`.
pub static USAGE: GlobalSignal<Option<UsageOverview>> = Signal::global(|| None);
/// Erhöhen, damit offene Verläufe (Kosten, Tokens) neu laden.
pub static USAGE_TICK: GlobalSignal<u64> = Signal::global(|| 0);

pub async fn reload_usage(refresh: bool) {
    let url = format!("/usage?refresh={refresh}&tz={}", tz_offset());
    if let Ok(u) = api::get::<UsageOverview>(&url).await {
        *USAGE.write() = Some(u);
    }
}

/// Zeitzone des Browsers für „heute“ (`Date.getTimezoneOffset()`, Minuten).
pub fn tz_offset() -> i64 {
    js_sys::Date::new_0().get_timezone_offset() as i64
}

/// Alles neu laden: Limits (`refresh` = beim Anbieter neu fragen, sonst höchstens zwei Minuten
/// alt) und die offenen Verläufe.
pub fn refresh_usage(refresh: bool) {
    *USAGE_TICK.write() += 1;
    spawn(reload_usage(refresh));
}

/// Konto in der Seitenleiste zeigen? (Limits oder Läufe in den letzten 30 Tagen)

fn window<'a>(a: &'a AccountUsage, key: &str) -> Option<&'a UsageWindow> {
    a.windows.iter().find(|w| w.key == key)
}

/// 917M, 4.2M, 12k, 1.5k
fn short(n: i64) -> String {
    let f = n as f64;
    if f >= 1e9 {
        format!("{:.2}B", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.1}M", f / 1e6)
    } else if f >= 1e4 {
        format!("{:.0}k", f / 1e3)
    } else if f >= 1e3 {
        format!("{:.1}k", f / 1e3)
    } else {
        n.to_string()
    }
}

fn money(c: f64) -> String {
    format!("${c:.2}")
}

/// Verbrauch eines Limits jetzt: nach dem Zurücksetzen wieder 0.
fn used_now(l: &UsageLimit, now: i64) -> f64 {
    if l.resets_at.is_some_and(|r| r <= now) {
        0.0
    } else {
        l.used_percent.clamp(0.0, 100.0)
    }
}

/// Farbe des Balkens nach Verbrauch.
fn bar_color(p: f64) -> &'static str {
    if p >= 90.0 {
        "bg-destructive"
    } else if p >= 70.0 {
        "bg-warning"
    } else {
        "bg-success"
    }
}

fn limit_label(l: &UsageLimit) -> String {
    tr(&l.label)
}

/// Limits älter als zehn Minuten und der Anbieter antwortet gerade nicht: Stand mit Grund.
fn stale_note(a: &AccountUsage, now: i64) -> Option<String> {
    let at = a.limits_at?;
    (!a.limits.is_empty() && !a.limits_error.is_empty() && now - at > 600).then(|| trf("Stand {0} – {1}", &[&util::ago(at), &tr(&a.limits_error)]))
}

/// Feste Farbe je Agent (Farbe folgt dem Agenten, nicht dem Rang), geprüft für den dunklen
/// Hintergrund; neben jedem Punkt steht immer der Name mit den Zahlen.
fn agent_color(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Codex => "#3987e5",
        AgentKind::Claude => "#d95926",
        AgentKind::Gemini => "#199e70",
        AgentKind::Opencode => "#c98500",
        AgentKind::Kilo => "#d55181",
        AgentKind::Grok => "#9085e9",
    }
}


// ------------------------------------------------------------------ Ring oben rechts


/// Farbe des Rings nach Verbrauch (wie die Balken).
fn ring_color(p: f64) -> &'static str {
    if p >= 90.0 {
        "var(--destructive)"
    } else if p >= 70.0 {
        "var(--warning)"
    } else {
        "var(--success)"
    }
}

/// Abschnitt „Nutzung“ (Popup unten in der Aufgabenleiste, rechte Spalte): je Konto Agent und Name,
/// darunter je Limit (5h, Woche, Sonderlimits) ein Balken mit dem, was übrig ist, und wann es
/// zurückgesetzt wird. Das Diagramm-Symbol führt zur Nutzung im Detail.
#[component]
pub fn UsageSection() -> Element {
    let nav = use_navigator();
    use_hook(|| {
        if USAGE.peek().is_none() {
            refresh_usage(false);
        }
    });
    let now = util::now();
    // Alle Konten (wie im Bild auch ohne Limits); mit Limits zuerst
    let mut list: Vec<AccountUsage> = USAGE.read().as_ref().map(|u| u.accounts.clone()).unwrap_or_default();
    list.sort_by_key(|a| a.limits.is_empty());
    rsx! {
        div { class: "flex items-center gap-2 px-2.5 pt-2 pb-1 text-[11.5px] font-medium text-muted-foreground",
            span { class: "flex-1", {tr("Nutzung")} }
            button {
                class: "grid size-6 place-items-center rounded-md text-muted-foreground hover:bg-white/[0.09] hover:text-foreground",
                title: tr("Nutzung im Detail"),
                onclick: move |_| { nav.push(crate::Route::Settings { section: "usage".into() }); },
                icons::ChartSpline { class: "size-3.5" }
            }
        }
        if list.is_empty() {
            p { class: "px-2.5 pb-2 text-[13px] text-muted-foreground", {tr("Noch keine Agent-Konten.")} }
        }
        // Je Konto eine Karte: je Limit Name, wie viel genutzt ist (Balken in der Farbe des Füllstands)
        // und wann es zurückgesetzt wird
        div { class: "grid gap-1.5 px-1 pb-1",
            for a in list {
                {
                    let id = a.account_id;
                    let mut limits: Vec<UsageLimit> = a.limits.clone();
                    // 5h zuerst, dann Woche, dann Sonderlimits
                    limits.sort_by_key(|l| match limit_group(&l.key) { Some(false) => 0, Some(true) => 1, None => 2 });
                    limits.truncate(4);
                    let name = format!("{} · {}", a.kind.name(), short_name(&a));
                    let name = if a.label.trim().is_empty() { a.kind.name().to_string() } else { name };
                    let rows: Vec<(String, f64, Option<i64>)> = limits.iter().map(|l| (limit_name(l), used_now(l, now).clamp(0.0, 100.0), l.resets_at.filter(|r| *r > now))).collect();
                    let note = stale_note(&a, now);
                    rsx! {
                        div { key: "{id}", class: "rounded-xl border border-white/[0.07] bg-white/[0.03] px-3 py-2.5",
                            div { class: "flex min-w-0 items-center gap-2.5 text-[13.5px]",
                                AgentIcon { kind: a.kind, size: 16 }
                                span { class: "min-w-0 flex-1 truncate font-medium", "{name}" }
                                if rows.is_empty() {
                                    span { class: "shrink-0 text-xs text-muted-foreground", title: if a.limits_error.is_empty() { tr("Noch keine Limits gemeldet.") } else { a.limits_error.clone() }, {tr("keine Limits")} }
                                }
                            }
                            if !rows.is_empty() {
                                div { class: "mt-2.5 grid gap-2.5",
                                    for (i , (label , used , reset)) in rows.into_iter().enumerate() {
                                        div { key: "{i}", class: "grid gap-1",
                                            div { class: "flex items-baseline justify-between gap-2 text-[12px]",
                                                span { class: "text-muted-foreground", "{label}" }
                                                span { class: "tabular-nums",
                                                    span { class: "font-semibold", style: "color: {ring_color(used)}", {format!("{used:.0} %")} }
                                                    span { class: "text-muted-foreground", " " {tr("genutzt")} }
                                                }
                                            }
                                            div { class: "h-[5px] overflow-hidden rounded-full bg-white/[0.08]",
                                                div { class: "h-full rounded-full transition-[width] duration-500", style: "width: {used.max(1.5)}%; background: {ring_color(used)}" }
                                            }
                                            if let Some(r) = reset {
                                                div { class: "text-[11px] text-muted-foreground tabular-nums", {trf("Zurück auf 0 in {0} · {1}", &[&reset_in(r - now), &util::clock(r)])} }
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some(n) = note {
                                p { class: "mt-2 text-[11px] text-muted-foreground", "{n}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Für die Zeile unten in der Seitenleiste: 5-Stunden- und Wochen-Limit (genutzt in Prozent) des
/// am stärksten ausgelasteten Kontos.
pub fn headline() -> Option<(Option<f64>, Option<f64>)> {
    let now = util::now();
    let usage = USAGE.read();
    let accounts = &usage.as_ref()?.accounts;
    let peak = |t: &(Option<f64>, Option<f64>)| t.0.unwrap_or(0.0).max(t.1.unwrap_or(0.0));
    accounts
        .iter()
        .filter(|a| !a.limits.is_empty())
        .map(|a| {
            let pick = |week: bool| a.limits.iter().filter(|l| limit_group(&l.key) == Some(week)).map(|l| used_now(l, now).clamp(0.0, 100.0)).max_by(|x, y| x.total_cmp(y));
            (pick(false), pick(true))
        })
        .filter(|t| t.0.is_some() || t.1.is_some())
        .max_by(|x, y| peak(x).total_cmp(&peak(y)))
}

/// Farbe nach Füllstand (grün, gelb, rot).
pub fn level_color(used: f64) -> &'static str {
    ring_color(used)
}

/// Name eines Limits in der Übersicht: „5 Stunden“, „Woche“, sonst der Name des Sonderlimits.
fn limit_name(l: &UsageLimit) -> String {
    match l.key.as_str() {
        "five_hour" | "primary" => tr("5 Stunden"),
        "seven_day" | "secondary" => tr("Woche"),
        // Grok: Woche oder Monat, je nachdem was der Anbieter meldet
        "grok" => {
            let label = l.label.trim();
            if label.is_empty() { tr("Woche") } else { tr(label) }
        }
        _ => limit_short(l),
    }
}

/// Kurzer Name eines Limits für die Zeile: „5h“, „Woche“, sonst der Name des Sonderlimits.
fn limit_short(l: &UsageLimit) -> String {
    match l.key.as_str() {
        "five_hour" | "primary" => "5h".into(),
        "seven_day" | "secondary" => tr("Woche"),
        "grok" => {
            let label = l.label.trim();
            if label.is_empty() { tr("Woche") } else { tr(label) }
        }
        _ => {
            let label = l.label.trim();
            if label.is_empty() { l.key.rsplit('_').next().unwrap_or(&l.key).to_string() } else { label.to_string() }
        }
    }
}

// ------------------------------------------------------------------ Session / Woche

/// Art eines Limits für die Übersicht: kurzes Fenster (Claude 5 Stunden, Codex „primary“) oder
/// Woche (Claude „seven_day“, Codex „secondary“); Sonderlimits (Opus, Sonnet …) nur im Detail.
fn limit_group(key: &str) -> Option<bool> {
    match key {
        "five_hour" | "primary" => Some(false),
        "seven_day" | "secondary" | "grok" => Some(true),
        _ => None,
    }
}

/// Kurzer Name eines Kontos für die Kacheln (Beschriftung des Kontos, sonst der Agent).
fn short_name(a: &AccountUsage) -> String {
    let l = a.label.trim();
    if l.is_empty() { a.kind.name().to_string() } else { l.to_string() }
}

/// „1h 38m“, „6d 14h“, „12m“
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

/// Limits als zwei Karten „Session“ und „Weekly“: oben wie viel im Schnitt übrig ist, darunter
/// je Konto übrig in Prozent, Zeit bis zum Zurücksetzen und ein Balken – voll = übrig, schraffiert
/// = verbraucht, der Strich zeigt, wie viel Zeit des Fensters noch bleibt (liegt die Füllung
/// links davon, wird schneller verbraucht als die Zeit vergeht).
#[component]
pub fn LimitsPanel(list: Vec<AccountUsage>, now: i64, #[props(default = false)] wide: bool) -> Element {
    let nav = use_navigator();
    let groups: Vec<(bool, Vec<(String, UsageLimit)>)> = [false, true]
        .into_iter()
        .map(|weekly| {
            let rows = list
                .iter()
                .flat_map(|a| a.limits.iter().filter(|l| limit_group(&l.key) == Some(weekly)).map(|l| (short_name(a), l.clone())))
                .collect::<Vec<_>>();
            (weekly, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect();
    rsx! {
        if groups.is_empty() {
            p { class: "px-1 py-2 text-xs text-muted-foreground", {tr("Noch keine Limits gemeldet.")} }
        }
        div { class: if wide { "grid gap-3 lg:grid-cols-2" } else { "grid gap-2.5" },
            for (weekly , rows) in groups {
                {
                    let span = if weekly { 7 * 86_400 } else { 5 * 3600 };
                    let left = rows.iter().map(|(_, l)| 100.0 - used_now(l, now)).sum::<f64>() / rows.len() as f64;
                    rsx! {
                        div { key: "{weekly}", class: "rounded-2xl border bg-card/60 p-3",
                            div { class: "mb-2 flex items-center gap-2",
                                span { class: "flex-1 text-[15px] font-semibold text-muted-foreground", if weekly { {tr("Wöchentlich")} } else { {tr("Session")} } }
                                span { class: "text-2xl font-bold tabular-nums tracking-tight", "{left:.0}%" }
                                span { class: "text-[13px] text-muted-foreground", {tr("übrig")} }
                                button {
                                    class: "grid size-6 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                                    title: tr("Nutzung im Detail"),
                                    onclick: move |_| { nav.push(crate::Route::Settings { section: "usage".into() }); },
                                    icons::ChartSpline { class: "size-4" }
                                }
                            }
                            div { class: "grid grid-cols-2 gap-x-3 gap-y-2",
                                for (i , (name , l)) in rows.into_iter().enumerate() {
                                    LimitBar { key: "{i}", name, limit: l, now, span }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Eine Kachel: Name und übrig in Prozent, Zeit bis zum Zurücksetzen, Balken mit Zeitstrich.
#[component]
fn LimitBar(name: String, limit: UsageLimit, now: i64, span: i64) -> Element {
    let left = 100.0 - used_now(&limit, now);
    let reset = limit.resets_at.filter(|r| *r > now);
    // Noch verbleibende Zeit des Fensters (0…100 %)
    let time = reset.map(|r| ((r - now) as f64 / span as f64 * 100.0).clamp(0.0, 100.0));
    let title = match reset {
        Some(r) => trf("{0} · {1}: {2} % übrig, zurückgesetzt {3}", &[&name, &limit_label(&limit), &format!("{left:.0}"), &util::clock(r)]),
        None => trf("{0} · {1}: {2} % übrig", &[&name, &limit_label(&limit), &format!("{left:.0}")]),
    };
    rsx! {
        div { class: "min-w-0", title: "{title}",
            div { class: "flex min-w-0 items-baseline gap-1.5 text-[12.5px]",
                span { class: "min-w-0 truncate font-semibold", "{name}" }
                span { class: "shrink-0 font-semibold tabular-nums", "{left:.0}%" }
                if let Some(r) = reset {
                    span { class: "ml-auto flex min-w-0 shrink items-center gap-0.5 truncate text-[11.5px] tabular-nums text-muted-foreground",
                        icons::RotateCw { class: "size-3 shrink-0 -scale-x-100" }
                        {reset_in(r - now)}
                    }
                }
            }
            div {
                class: "relative mt-1 h-3.5 overflow-hidden rounded-full",
                // Schraffur = verbraucht
                style: "background: repeating-linear-gradient(135deg, color-mix(in oklab, var(--muted-foreground) 26%, transparent) 0 2px, color-mix(in oklab, var(--muted-foreground) 9%, transparent) 2px 5px)",
                div { class: "h-full rounded-full bg-muted-foreground/45 transition-[width] duration-500", style: "width: {left}%" }
                if let Some(t) = time {
                    div { class: "absolute inset-y-0 w-[3px] -translate-x-1/2 rounded-full bg-foreground/85", style: "left: clamp(2px, {t}%, calc(100% - 2px))" }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Board

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Cost,
    Tokens,
    Limits,
}

impl Tab {
    fn key(self) -> &'static str {
        match self {
            Tab::Cost => "cost",
            Tab::Tokens => "tokens",
            Tab::Limits => "limits",
        }
    }
    fn parse(s: &str) -> Option<Tab> {
        match s {
            "cost" => Some(Tab::Cost),
            "tokens" => Some(Tab::Tokens),
            "limits" => Some(Tab::Limits),
            _ => None,
        }
    }
}

/// Nutzung mit Reitern Kosten | Tokens | Limits und Zeitraum 24h–90d. `compact` = Fenster der
/// Menüleiste (schmal, eine Spalte), sonst breit für den Dialog der App.
#[component]
pub fn UsageBoard(compact: bool) -> Element {
    let now = use_clock();
    let mut range = use_signal(|| util::storage_get("nrz-tray-range").unwrap_or_else(|| "24h".into()));
    // Ohne gespeicherte Wahl: Limits zuerst, sobald ein Abo sie meldet
    let mut tab = use_signal(|| {
        util::storage_get("nrz-usage-tab").as_deref().and_then(Tab::parse).unwrap_or_else(|| {
            let limits = USAGE.peek().as_ref().is_some_and(|u| u.accounts.iter().any(|a| !a.limits.is_empty()));
            if limits { Tab::Limits } else { Tab::Cost }
        })
    });
    let mut busy = use_signal(|| false);
    let report = use_resource(move || async move {
        let _ = USAGE_TICK();
        api::get::<UsageReport>(&format!("/me/usage?range={}", range())).await
    });
    // Limits beim Öffnen auffrischen (der Verlauf lädt oben ohnehin)
    use_hook(|| {
        spawn(reload_usage(false));
    });
    let mut pick = move |t: Tab| {
        util::storage_set("nrz-usage-tab", t.key());
        tab.set(t);
    };
    let seg = |on: bool| {
        if on {
            "rounded-md bg-accent px-2.5 py-1 text-xs font-semibold text-foreground"
        } else {
            "rounded-md px-2.5 py-1 text-xs text-muted-foreground hover:text-foreground"
        }
    };
    let tab_cls = |on: bool| {
        if on {
            "flex-1 rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-foreground"
        } else {
            "flex-1 rounded-md px-3 py-1.5 text-sm text-muted-foreground hover:text-foreground"
        }
    };
    let ranges = rsx! {
        div { class: "flex items-center gap-0.5 rounded-lg border p-0.5",
            for r in ["24h", "7d", "30d", "90d"] {
                button {
                    key: "{r}",
                    class: seg(range() == r),
                    onclick: move |_| {
                        util::storage_set("nrz-tray-range", r);
                        range.set(r.to_string());
                    },
                    "{r}"
                }
            }
        }
    };
    let refresh = rsx! {
        button {
            class: "grid size-8 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-50",
            title: tr("Aktualisieren"),
            disabled: busy(),
            onclick: move |_| {
                busy.set(true);
                *USAGE_TICK.write() += 1;
                spawn(async move {
                    reload_usage(true).await;
                    busy.set(false);
                });
            },
            icons::RefreshCw { class: if busy() { "size-4 animate-spin" } else { "size-4" } }
        }
    };
    let body = if tab() == Tab::Limits {
        rsx! { LimitsView { compact, now } }
    } else {
        match report.read().clone() {
            None => rsx! { div { class: "py-10", Loading {} } },
            Some(Err(e)) => rsx! { p { class: "py-6 text-sm text-destructive", "{e.message}" } },
            Some(Ok(r)) => rsx! { Report { report: r, cost: tab() == Tab::Cost, compact } },
        }
    };
    if compact {
        return rsx! {
            div { class: "mx-2 flex shrink-0 items-center gap-1",
                div { class: "flex flex-1 gap-0.5 rounded-lg border p-0.5",
                    button { class: tab_cls(tab() == Tab::Limits), onclick: move |_| pick(Tab::Limits), {tr("Limits")} }
                    button { class: tab_cls(tab() == Tab::Cost), onclick: move |_| pick(Tab::Cost), {tr("Kosten")} }
                    button { class: tab_cls(tab() == Tab::Tokens), onclick: move |_| pick(Tab::Tokens), {tr("Tokens")} }
                }
                {refresh}
            }
            if tab() != Tab::Limits {
                div { class: "mx-2 mt-2 flex shrink-0 justify-end", {ranges} }
            }
            div { class: "mt-2 min-h-0 flex-1 overflow-y-auto border-t px-4 pb-4", {body} }
        };
    }
    rsx! {
        div { class: "flex flex-wrap items-center gap-2",
            div { class: "flex gap-0.5 rounded-lg border p-0.5",
                button { class: tab_cls(tab() == Tab::Limits), onclick: move |_| pick(Tab::Limits), {tr("Limits")} }
                button { class: tab_cls(tab() == Tab::Cost), onclick: move |_| pick(Tab::Cost), {tr("Kosten")} }
                button { class: tab_cls(tab() == Tab::Tokens), onclick: move |_| pick(Tab::Tokens), {tr("Tokens")} }
            }
            div { class: "flex-1" }
            if tab() != Tab::Limits {
                {ranges}
            }
            {refresh}
        }
        div { class: "mt-2", {body} }
    }
}

/// Kosten- oder Token-Ansicht eines Zeitraums.
#[component]
fn Report(report: UsageReport, cost: bool, compact: bool) -> Element {
    let t = report.total.clone();
    let value = |s: &TokenSums| if cost { s.cost_usd } else { s.total_tokens() as f64 };
    let total = value(&t);
    let hero = if cost { money(t.cost_usd) } else { short(t.total_tokens()) };
    let sub = if cost { trf("{0} Sitzungen · von den CLIs gemeldet", &[&report.sessions]) } else { trf("{0} Sitzungen", &[&report.sessions]) };
    let chart_title = match (cost, report.bucket_seconds) {
        (true, 3600) => tr("Kosten je Stunde"),
        (true, 21600) => tr("Kosten je 6 Stunden"),
        (true, 86400) => tr("Kosten je Tag"),
        (true, _) => tr("Kosten je 3 Tage"),
        (false, 3600) => tr("Tokens je Stunde"),
        (false, 21600) => tr("Tokens je 6 Stunden"),
        (false, 86400) => tr("Tokens je Tag"),
        (false, _) => tr("Tokens je 3 Tage"),
    };
    let totals: Vec<(String, String)> = vec![
        (tr("Verarbeitete Tokens"), short(t.total_tokens())),
        (tr("Eingabe aus dem Cache"), short(t.cache_read_tokens)),
        (tr("In den Cache geschrieben"), short(t.cache_write_tokens)),
        (tr("Eingabe ohne Cache"), short(t.fresh_input())),
        (tr("Ausgabe"), short(t.output_tokens)),
        (tr("davon Denken"), short(t.reasoning_tokens)),
        (tr("Kosten"), money(t.cost_usd)),
        (tr("Laufzeit"), duration(t.seconds)),
    ];
    // Je Agent – zugleich die Legende des Diagramms (Punkt, Name, Zahlen)
    let legend = rsx! {
        div { class: "grid content-start gap-3",
            if report.agents.is_empty() {
                p { class: "text-sm text-muted-foreground", {tr("Keine Läufe in diesem Zeitraum.")} }
            }
            for a in report.agents.iter().cloned() {
                {
                    let v = value(&a.sums);
                    let share = if total > 0.0 { v / total * 100.0 } else { 0.0 };
                    let right = if cost { money(a.sums.cost_usd) } else { short(a.sums.total_tokens()) };
                    let detail = if cost {
                        trf("{0} % der Kosten · {1} Tokens", &[&format!("{share:.1}"), &short(a.sums.total_tokens())])
                    } else {
                        trf("{0} % der Tokens · {1}", &[&format!("{share:.1}"), &money(a.sums.cost_usd)])
                    };
                    rsx! {
                        div { key: "{a.agent.key()}", class: "flex items-start gap-2.5",
                            span { class: "mt-[7px] size-2 shrink-0 rounded-full", style: "background: {agent_color(a.agent)}" }
                            div { class: "min-w-0 flex-1",
                                div { class: "flex items-center gap-2",
                                    AgentIcon { kind: a.agent, size: 16 }
                                    span { class: "font-medium", {a.agent.name()} }
                                    span { class: "text-xs text-muted-foreground", {trf("{0} Sitzungen", &[&a.sessions])} }
                                    span { class: "ml-auto font-semibold tabular-nums", "{right}" }
                                }
                                p { class: "mt-0.5 text-xs text-muted-foreground tabular-nums", "{detail}" }
                                if !compact && total > 0.0 {
                                    div { class: "mt-1.5 h-1 overflow-hidden rounded-full bg-muted",
                                        div { class: "h-full rounded-full", style: "width: {share.max(1.0)}%; background: {agent_color(a.agent)}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    let narrow = web_sys::window().and_then(|w| w.inner_width().ok()).and_then(|v| v.as_f64()).is_some_and(|w| w < 768.0);
    if compact {
        return rsx! {
            p { class: "mt-4 text-4xl font-semibold tracking-tight tabular-nums", "{hero}" }
            p { class: "mt-1 text-sm text-muted-foreground", "{sub}" }
            div { class: "mt-4", {legend} }
            if !report.series.is_empty() {
                p { class: "mt-6 mb-2 text-sm font-semibold", "{chart_title}" }
                LineChart { report: report.clone(), cost, width: 328.0, height: 150.0 }
            }
            p { class: "mt-6 mb-1 text-sm font-semibold", {tr("Summen")} }
            div { class: "grid",
                for (label , v) in totals {
                    div { class: "flex items-center justify-between border-b border-border/50 py-1.5 text-sm last:border-0",
                        span { class: "text-muted-foreground", "{label}" }
                        span { class: "tabular-nums", "{v}" }
                    }
                }
            }
        };
    }
    rsx! {
        div { class: "grid gap-6 md:grid-cols-[minmax(0,1fr)_16rem]",
            div { class: "min-w-0",
                p { class: "text-4xl font-semibold tracking-tight tabular-nums", "{hero}" }
                p { class: "mt-1 text-sm text-muted-foreground", "{sub}" }
                if !report.series.is_empty() {
                    p { class: "mt-5 mb-2 text-sm font-semibold", "{chart_title}" }
                    // Schmaler Bildschirm: kleineres Koordinatensystem, sonst wird die Achsenschrift winzig
                    if narrow {
                        LineChart { report: report.clone(), cost, width: 328.0, height: 160.0 }
                    } else {
                        LineChart { report: report.clone(), cost, width: 560.0, height: 210.0 }
                    }
                }
            }
            div { class: "min-w-0 md:border-l md:pl-6",
                p { class: "mb-3 text-sm font-semibold", {tr("Je Agent")} }
                {legend}
            }
        }
        p { class: "mt-6 mb-2 text-sm font-semibold", {tr("Summen")} }
        div { class: "grid grid-cols-2 gap-2 sm:grid-cols-4",
            for (label , v) in totals {
                div { class: "rounded-lg border bg-card px-3 py-2",
                    p { class: "text-[11px] text-muted-foreground", "{label}" }
                    p { class: "mt-0.5 text-sm font-semibold tabular-nums", "{v}" }
                }
            }
        }
    }
}

/// Runde Obergrenze für die Achse (1, 2, 2.5, 5 × 10^n).
fn nice_max(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let p = 10f64.powf(v.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if v <= m * p {
            return m * p;
        }
    }
    10.0 * p
}

/// Zeit eines Abschnitts für die Achse: „23 Uhr“ bzw. „7.11.“
fn bucket_label(ts: i64, bucket: i64) -> String {
    let d = js_sys::Date::new(&JsValue::from_f64(ts as f64 * 1000.0));
    if bucket < 86_400 {
        let opts = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&opts, &"hour".into(), &"numeric".into());
        d.to_locale_time_string_with_options(&crate::i18n::locale(), &opts).as_string().unwrap_or_default()
    } else {
        let opts = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&opts, &"day".into(), &"numeric".into());
        let _ = js_sys::Reflect::set(&opts, &"month".into(), &"short".into());
        d.to_locale_date_string(&crate::i18n::locale(), &opts).as_string().unwrap_or_default()
    }
}

/// Linien je Agent (eine Achse), Fadenkreuz mit Werten aller Agenten beim Überfahren. `width`
/// und `height` in Punkten des Koordinatensystems (Schrift 9.5 bleibt bei voller Breite lesbar).
#[component]
fn LineChart(report: UsageReport, cost: bool, width: f64, height: f64) -> Element {
    let mut hover = use_signal(|| None::<usize>);
    let svg_id = use_hook(|| format!("nrz-usage-chart-{}", (js_sys::Math::random() * 1e9) as u64));
    let (w, h) = (width, height);
    let (left, right, top, bottom) = (40.0, 6.0, 8.0, 20.0);
    let pw = w - left - right;
    let ph = h - top - bottom;
    let n = report.series.first().map(|s| s.points.len()).unwrap_or(0).max(1);
    let val = |p: &UsagePoint| if cost { p.cost_usd } else { p.tokens as f64 };
    let max = report.series.iter().flat_map(|s| s.points.iter().map(val)).fold(0.0, f64::max);
    let top_v = nice_max(max);
    let x = move |i: usize| left + if n > 1 { pw * i as f64 / (n - 1) as f64 } else { pw / 2.0 };
    let y = move |v: f64| top + ph - ph * (v / top_v).min(1.0);
    let fmt = move |v: f64| if cost { if top_v < 1.0 { format!("${v:.2}") } else { format!("${v:.0}") } } else { short(v as i64) };
    let grid = "stroke: var(--border); stroke-width: 1";
    let mid = n / 2;
    let labels = [(0usize, "start"), (mid, "middle"), (n - 1, "end")];
    let lines: Vec<(AgentKind, String, String)> = report
        .series
        .iter()
        .map(|s| {
            let pts: Vec<String> = s.points.iter().enumerate().map(|(i, p)| format!("{:.1},{:.1}", x(i), y(val(p)))).collect();
            let line = format!("M{}", pts.join(" L"));
            let area = format!("{line} L{:.1},{:.1} L{:.1},{:.1} Z", x(n - 1), top + ph, x(0), top + ph);
            (s.agent, line, area)
        })
        .collect();
    let tip = hover().map(|i| {
        let start = report.from + report.bucket_seconds * i as i64;
        let mut rows: Vec<(AgentKind, f64)> = report.series.iter().map(|s| (s.agent, s.points.get(i).map(val).unwrap_or(0.0))).collect();
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        (i, bucket_label(start, report.bucket_seconds), rows)
    });
    let id = svg_id.clone();
    rsx! {
        div { class: "relative",
            svg {
                id: "{svg_id}",
                class: "block w-full overflow-visible",
                view_box: "0 0 {w} {h}",
                role: "img",
                "aria-label": if cost { tr("Kosten im Verlauf je Agent") } else { tr("Tokens im Verlauf je Agent") },
                onmousemove: move |e: MouseEvent| {
                    let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&id)) else { return };
                    let r = el.get_bounding_client_rect();
                    if r.width() <= 0.0 {
                        return;
                    }
                    let px = (e.client_coordinates().x - r.left()) / r.width() * w;
                    let i = if n > 1 { (((px - left) / pw) * (n - 1) as f64).round() } else { 0.0 };
                    hover.set(Some(i.clamp(0.0, (n - 1) as f64) as usize));
                },
                onmouseleave: move |_| hover.set(None),
                // Gitter und Achse (zurückhaltend)
                for k in 0..=4 {
                    {
                        let v = top_v * k as f64 / 4.0;
                        let yy = y(v);
                        rsx! {
                            line { x1: "{left}", x2: "{w - right}", y1: "{yy}", y2: "{yy}", style: if k == 0 { "stroke: var(--muted-foreground); stroke-opacity: .5; stroke-width: 1" } else { grid } }
                            text { x: "{left - 6.0}", y: "{yy + 3.5}", "text-anchor": "end", style: "font-size: 9.5px; fill: var(--muted-foreground)", {fmt(v)} }
                        }
                    }
                }
                for (i , anchor) in labels {
                    text {
                        x: "{x(i)}",
                        y: "{h - 5.0}",
                        "text-anchor": anchor,
                        style: "font-size: 9.5px; fill: var(--muted-foreground)",
                        {bucket_label(report.from + report.bucket_seconds * i as i64, report.bucket_seconds)}
                    }
                }
                // Flächen leicht, Linien 2px
                for (kind , _ , area) in lines.iter().cloned() {
                    path { d: "{area}", style: "fill: {agent_color(kind)}; fill-opacity: .08; stroke: none" }
                }
                for (kind , line , _) in lines.iter().cloned() {
                    path { d: "{line}", style: "fill: none; stroke: {agent_color(kind)}; stroke-width: 2; stroke-linejoin: round; stroke-linecap: round" }
                }
                if let Some((i, _, rows)) = tip.clone() {
                    line { x1: "{x(i)}", x2: "{x(i)}", y1: "{top}", y2: "{top + ph}", style: "stroke: var(--muted-foreground); stroke-opacity: .6; stroke-width: 1; stroke-dasharray: 3 3" }
                    for (kind , v) in rows {
                        circle { cx: "{x(i)}", cy: "{y(v)}", r: "4", style: "fill: {agent_color(kind)}; stroke: var(--background); stroke-width: 2" }
                    }
                }
            }
            if let Some((i, label, rows)) = tip {
                div {
                    class: "pointer-events-none absolute top-0 z-10 min-w-36 rounded-lg border bg-popover px-2.5 py-2 text-xs shadow-lg",
                    style: if x(i) > w / 2.0 { "right: {(w - x(i)) / w * 100.0 + 3.0}%" } else { "left: {x(i) / w * 100.0 + 3.0}%" },
                    p { class: "mb-1 font-medium", "{label}" }
                    for (kind , v) in rows {
                        div { class: "flex items-center gap-2",
                            span { class: "size-2 shrink-0 rounded-full", style: "background: {agent_color(kind)}" }
                            span { class: "flex-1 text-muted-foreground", {kind.name()} }
                            span { class: "tabular-nums", {if cost { format!("${v:.2}") } else { short(v as i64) }} }
                        }
                    }
                }
            }
        }
    }
}

/// Limits der Abos je Konto (aus `GET /usage`): genutzt in Prozent, Balken, Zurücksetzen. Breit
/// zusätzlich, was nrzX je Konto gezählt hat (5 Stunden, heute, 7 und 30 Tage).
#[component]
fn LimitsView(compact: bool, now: i64) -> Element {
    let usage = USAGE();
    let Some(u) = usage else { return rsx! { div { class: "py-10", Loading {} } } };
    let accounts: Vec<AccountUsage> = u
        .accounts
        .into_iter()
        .filter(|a| !a.limits.is_empty() || !a.limits_error.is_empty() || (!compact && a.windows.iter().any(|w| w.runs > 0)))
        .collect();
    let names = [("5h", tr("5 Stunden")), ("today", tr("Heute")), ("7d", tr("7 Tage")), ("30d", tr("30 Tage"))];
    rsx! {
        if accounts.is_empty() {
            p { class: "py-8 text-center text-sm text-muted-foreground", {tr("Keine Limits gemeldet. Abos von Claude, Codex und Grok melden ihre Limits nach der Anmeldung.")} }
        }
        if !accounts.is_empty() {
            LimitsPanel { list: accounts.clone(), now, wide: !compact }
        }
        div { class: if compact { "grid" } else { "mt-4 grid gap-4" },
            for a in accounts {
                div { key: "{a.account_id}", class: if compact { "mt-4" } else { "rounded-xl border bg-card p-4" },
                    div { class: "mb-2 flex items-center gap-2",
                        AgentIcon { kind: a.kind, size: 18 }
                        span { class: "font-semibold", {a.kind.name()} }
                        span { class: "min-w-0 truncate text-xs text-muted-foreground", "{a.label}" }
                        if !compact {
                            if let Some(t) = a.last_used_at {
                                span { class: "ml-auto shrink-0 text-xs text-muted-foreground", {trf("zuletzt {0}", &[&util::ago(t)])} }
                            }
                        }
                    }
                    if a.limits.is_empty() {
                        p { class: "rounded-xl border px-3 py-2 text-xs text-muted-foreground",
                            if a.limits_error.is_empty() { {tr("Dieser Anbieter meldet keine Limits – unten zählt nrzX selbst.")} } else { {tr(&a.limits_error)} }
                        }
                    }
                    div { class: if compact { "grid gap-2" } else { "grid gap-2 sm:grid-cols-2" },
                        // Session und Woche stehen oben in den Karten, hier nur Sonderlimits (Opus …)
                        for l in a.limits.iter().filter(|l| limit_group(&l.key).is_none()).cloned() {
                            LimitCard { key: "{l.key}", limit: l, now, compact }
                        }
                    }
                    if let Some(note) = stale_note(&a, now) {
                        p { class: "mt-1.5 text-[11px] text-muted-foreground", "{note}" }
                    } else if a.limits_source == "session" {
                        if let Some(at) = a.limits_at {
                            p { class: "mt-1.5 text-[11px] text-muted-foreground", {trf("Stand der letzten Sitzung: {0}", &[&util::datetime(at)])} }
                        }
                    }
                    if !compact {
                        div { class: "mt-3 grid grid-cols-2 gap-2 sm:grid-cols-4",
                            for (key , name) in names.iter().cloned() {
                                if let Some(w) = window(&a, key).cloned() {
                                    div { key: "{key}", class: "rounded-lg border bg-background/40 px-3 py-2",
                                        p { class: "text-[11px] text-muted-foreground", "{name}" }
                                        p { class: "mt-0.5 text-sm font-semibold tabular-nums",
                                            if w.cost_usd > 0.0 { "$ {w.cost_usd:.2}" } else { {short(w.input_tokens + w.output_tokens)} }
                                        }
                                        p { class: "text-[11px] text-muted-foreground tabular-nums",
                                            {trf("{0} Läufe · {1} rein · {2} raus", &[&w.runs, &short(w.input_tokens), &short(w.output_tokens)])}
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
}

/// Ein Limit: genutzt in Prozent (wie beim Anbieter), Balken und Zeit bis zum Zurücksetzen.
#[component]
fn LimitCard(limit: UsageLimit, now: i64, compact: bool) -> Element {
    let used = used_now(&limit, now);
    let reset = limit.resets_at.filter(|r| *r > now);
    let done = limit.resets_at.is_some_and(|r| r <= now);
    rsx! {
        div { class: "rounded-xl border bg-background/40 px-3 py-2.5",
            div { class: "flex items-baseline gap-2",
                span { class: "min-w-0 truncate text-sm text-muted-foreground", {limit_label(&limit)} }
                span { class: "ml-auto text-xl font-semibold tabular-nums", "{used:.0}%" }
                span { class: "text-xs text-muted-foreground", {tr("genutzt")} }
            }
            div { class: "mt-2 h-2 overflow-hidden rounded-full bg-muted",
                div { class: "h-full rounded-full {bar_color(used)}", style: "width: {used.max(1.0)}%" }
            }
            if let Some(r) = reset {
                p { class: "mt-1.5 flex items-center gap-1 text-[11px] text-muted-foreground",
                    icons::RotateCw { class: "size-3 shrink-0" }
                    if compact {
                        {trf("Zurückgesetzt in {0}", &[&duration(r - now)])}
                    } else {
                        {trf("Zurückgesetzt in {0} ({1})", &[&duration(r - now), &util::clock(r)])}
                    }
                }
            } else if done {
                p { class: "mt-1.5 flex items-center gap-1 text-[11px] text-muted-foreground",
                    icons::RotateCw { class: "size-3 shrink-0" }
                    {tr("bereits zurückgesetzt")}
                }
            }
        }
    }
}
