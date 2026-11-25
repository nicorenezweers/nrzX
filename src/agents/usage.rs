//! Nutzung der Agent-Konten (`GET /api/v1/usage`): Verbrauch aus den Läufen in nrzX
//! (`usage_log`, bleibt beim Löschen von Threads erhalten; 5 Stunden, heute, 7 und 30 Tage)
//! und – wo der Anbieter sie verrät – die Limits des Abos:
//!
//! * Claude Code (Anmeldung im CLI): `GET https://api.anthropic.com/api/oauth/usage` mit dem
//!   OAuth-Token aus `<home>/.claude/.credentials.json` (5-Stunden-Fenster, Woche, Woche Opus …)
//! * Codex: letzte `rate_limits` aus den Sitzungsprotokollen des CLIs
//!   (`<home>/.codex/sessions/**/rollout-*.jsonl`, primäres und sekundäres Fenster)
//! * Grok CLI: `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` mit der
//!   Anmeldung aus `<home>/.grok/auth.json` (Wochenpool, je Produkt, Zusatzverbrauch).
//!   Das ist dieselbe Abfrage wie `/usage` im CLI. Ein API-Schlüssel allein meldet das nicht.
//!   Abgelaufene Tokens erneuert nrzX nicht – das CLI rotiert sie, ein zweites Erneuern
//!   würde die Anmeldung ungültig machen.
//!
//! Gemini CLI, OpenCode und Kilo Code melden keine Limits – dort zählt nrzX Läufe,
//! Tokens und Kosten selbst. Abfragen beim Anbieter werden zwei Minuten zwischengespeichert.

use super::accounts::{self, AccountRow};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::AppResult;
use crate::AppState;
use nrzx_shared::agents::{AccountUsage, UsageLimit, UsageOverview, UsageWindow};
use nrzx_shared::AgentKind;
use rocket::serde::json::Json;
use rocket::{get, routes, Route, State};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub fn routes() -> Vec<Route> {
    routes![overview]
}

/// Zwischenspeicher der Limits je Konto: (abgefragt, Limits, Quelle, Fehler, Zeitpunkt).
type Cached = (Instant, Vec<UsageLimit>, String, String, Option<i64>);

fn cache() -> &'static Mutex<HashMap<i64, Cached>> {
    static C: OnceLock<Mutex<HashMap<i64, Cached>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

const CACHE_FOR: Duration = Duration::from_secs(120);

/// So lange (Sekunden) bleibt der letzte Stand der Limits sichtbar, wenn der Anbieter nicht antwortet.
const STALE_FOR: i64 = 6 * 3600;

/// Beginn des heutigen Tages in der Zeitzone des Browsers (`tz` wie `Date.getTimezoneOffset()`:
/// UTC minus Ortszeit in Minuten, ohne Angabe UTC).
pub fn start_of_day(ts: i64, tz: Option<i64>) -> i64 {
    let offset = tz.unwrap_or(0).clamp(-16 * 60, 16 * 60) * 60;
    let local = ts - offset;
    local - local.rem_euclid(86_400) + offset
}

/// `GET /usage?refresh=&tz=` – Nutzung aller Konten des Benutzers.
#[get("/usage?<refresh>&<tz>")]
async fn overview(state: &State<AppState>, user: CurrentUser, refresh: Option<bool>, tz: Option<i64>) -> AppResult<Json<UsageOverview>> {
    let rows: Vec<AccountRow> = sqlx::query_as("SELECT * FROM agent_accounts WHERE user_id = ? ORDER BY kind, is_default DESC, label")
        .bind(user.id())
        .fetch_all(&state.db)
        .await?;
    let ts = now();
    let windows = [("5h", ts - 5 * 3600), ("today", start_of_day(ts, tz)), ("7d", ts - 7 * 86_400), ("30d", ts - 30 * 86_400)];
    let mut out = Vec::new();
    for r in rows {
        let mut list = Vec::new();
        for (key, since) in windows {
            let (runs, input, output, cost): (i64, i64, i64, f64) = sqlx::query_as(
                "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cost_usd), 0.0) \
                 FROM usage_log WHERE account_id = ? AND user_id = ? AND started_at >= ?",
            )
            .bind(r.id)
            .bind(user.id())
            .bind(since)
            .fetch_one(&state.db)
            .await?;
            list.push(UsageWindow { key: key.into(), runs, input_tokens: input, output_tokens: output, cost_usd: cost });
        }
        let (limits, source, error, at) = limits(state, &r, refresh.unwrap_or(false)).await;
        out.push(AccountUsage {
            account_id: r.id,
            kind: r.kind(),
            label: r.label.clone(),
            auth_kind: r.auth_kind.clone(),
            windows: list,
            limits,
            limits_source: source,
            limits_error: error,
            limits_at: at,
            last_used_at: r.last_used_at,
        });
    }
    Ok(Json(UsageOverview { accounts: out, generated_at: ts }))
}

/// Limits eines Kontos (zwischengespeichert).
pub(crate) async fn limits(state: &AppState, r: &AccountRow, refresh: bool) -> (Vec<UsageLimit>, String, String, Option<i64>) {
    let previous = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&r.id).cloned();
    if !refresh {
        if let Some(c) = &previous {
            if c.0.elapsed() < CACHE_FOR {
                return (c.1.clone(), c.2.clone(), c.3.clone(), c.4);
            }
        }
    }
    let home = accounts::home(state, r.id);
    // Mit nrzCode verknüpfte Anmeldung: das neueste Token nehmen
    accounts::sync_login(state, r.id).await;
    let (list, source, error, at) = match r.kind() {
        AgentKind::Claude => match claude_limits(&home, r).await {
            Ok(l) => (l, "api".to_string(), String::new(), Some(now())),
            Err(e) => (Vec::new(), String::new(), e, None),
        },
        AgentKind::Codex => match codex_limits(&home.join(".codex").join("sessions")).await {
            Some((l, at)) => (l, "session".to_string(), String::new(), Some(at)),
            None => (Vec::new(), String::new(), "Noch keine Limits – Codex meldet sie nach dem ersten Lauf.".to_string(), None),
        },
        AgentKind::Grok => match grok_limits(&home, r).await {
            Ok(l) => (l, "api".to_string(), String::new(), Some(now())),
            Err(e) => (Vec::new(), String::new(), e, None),
        },
        _ => (Vec::new(), String::new(), String::new(), None),
    };
    // Anbieter gerade nicht erreichbar (abgelaufene Anmeldung, zu viele Abfragen …): den letzten
    // Stand behalten statt die Limits verschwinden zu lassen – mit Zeitpunkt und Fehler
    let (list, source, error, at) = match previous {
        Some(p) if list.is_empty() && !error.is_empty() && !p.1.is_empty() && p.4.is_some_and(|t| now() - t < STALE_FOR) => (p.1, p.2, error, p.4),
        _ => (list, source, error, at),
    };
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(r.id, (Instant::now(), list.clone(), source.clone(), error.clone(), at));
    (list, source, error, at)
}

// ------------------------------------------------------------------ Claude Code

/// OAuth-Token des Kontos: Anmeldung im CLI (Datei) oder hinterlegtes OAuth-Token.
async fn claude_token(home: &Path, r: &AccountRow) -> Option<String> {
    if r.auth_kind == "oauth_token" && !r.secret.trim().is_empty() {
        return Some(r.secret.trim().to_string());
    }
    let raw = tokio::fs::read_to_string(home.join(".claude").join(".credentials.json")).await.ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    v["claudeAiOauth"]["accessToken"].as_str().filter(|s| !s.is_empty()).map(String::from)
}

async fn claude_limits(home: &Path, r: &AccountRow) -> Result<Vec<UsageLimit>, String> {
    if r.auth_kind == "api_key" {
        return Err("API-Schlüssel: keine Abo-Limits, nur Kosten.".into());
    }
    let Some(token) = claude_token(home, r).await else {
        return Err("Nicht angemeldet – keine Limits verfügbar.".into());
    };
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
    let resp = http
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "nrzx")
        .send()
        .await
        .map_err(|e| format!("Anthropic nicht erreichbar: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("Anmeldung abgelaufen oder ohne Berechtigung – sie wird beim nächsten Lauf erneuert.".into());
    }
    if !status.is_success() {
        return Err(format!("Anthropic antwortet mit {status}."));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parse_claude(&v))
}

/// `{"five_hour": {"utilization": 12.0, "resets_at": "2026-…"}, "seven_day": {…}, …}`
fn parse_claude(v: &Value) -> Vec<UsageLimit> {
    let Some(obj) = v.as_object() else { return Vec::new() };
    let order = ["five_hour", "seven_day", "seven_day_opus", "seven_day_sonnet", "seven_day_oauth_apps"];
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort_by_key(|k| order.iter().position(|o| o == k).unwrap_or(order.len()));
    let mut out = Vec::new();
    for k in keys {
        let w = &obj[k];
        let Some(util) = w.get("utilization").and_then(Value::as_f64) else { continue };
        let resets_at = w.get("resets_at").and_then(Value::as_str).and_then(parse_rfc3339);
        out.push(UsageLimit { key: k.clone(), label: claude_label(k), used_percent: util.clamp(0.0, 100.0), resets_at });
    }
    out
}

fn claude_label(key: &str) -> String {
    match key {
        "five_hour" => "5 Stunden".into(),
        "seven_day" => "Woche".into(),
        "seven_day_opus" => "Woche (Opus)".into(),
        "seven_day_sonnet" => "Woche (Sonnet)".into(),
        "seven_day_oauth_apps" => "Woche (Apps)".into(),
        other => other.replace('_', " "),
    }
}

/// „2026-11-01T12:30:00.123+00:00“ / „…Z“ → Unix-Sekunden (ohne Zusatz-Crate).
fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // Zeitzone am Ende: Z oder ±hh:mm
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = if rest.is_empty() || rest.eq_ignore_ascii_case("z") {
        0
    } else {
        let sign = if rest.starts_with('-') { -1 } else { 1 };
        let hh = rest.get(1..3)?.parse::<i64>().ok()?;
        let mm = rest.get(4..6).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        sign * (hh * 3600 + mm * 60)
    };
    // Tage seit 1970 (Algorithmus von Howard Hinnant)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se - offset)
}

// ------------------------------------------------------------------ Codex

/// Neueste Sitzungsdatei unter `sessions/` (rekursiv, höchstens ein paar tausend Einträge).
fn newest_jsonl(dir: &Path) -> Option<(PathBuf, std::time::SystemTime)> {
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = 0;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 20_000 {
                return best;
            }
            let p = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                let m = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(_, bm)| m > *bm) {
                    best = Some((p, m));
                }
            }
        }
    }
    best
}

async fn codex_limits(sessions: &Path) -> Option<(Vec<UsageLimit>, i64)> {
    let dir = sessions.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let (file, mtime) = newest_jsonl(&dir)?;
        let text = std::fs::read_to_string(&file).ok()?;
        let at = mtime.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or_else(|_| now());
        text.lines().rev().filter(|l| l.contains("rate_limits")).find_map(|l| {
            let v: Value = serde_json::from_str(l).ok()?;
            let rl = find_key(&v, "rate_limits")?;
            let list = parse_codex(rl, at);
            (!list.is_empty()).then_some((list, at))
        })
    })
    .await
    .ok()
    .flatten()
}

fn find_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(o) => o.get(key).filter(|x| !x.is_null()).or_else(|| o.values().find_map(|x| find_key(x, key))),
        _ => None,
    }
}

/// `{"primary": {"used_percent": 12.0, "window_minutes": 300, "resets_at": 1730000000 | "resets_in_seconds": 3600}, "secondary": {…}}`
fn parse_codex(rl: &Value, at: i64) -> Vec<UsageLimit> {
    let mut out = Vec::new();
    for key in ["primary", "secondary"] {
        let Some(w) = rl.get(key).filter(|w| w.is_object()) else { continue };
        let Some(used) = w.get("used_percent").and_then(Value::as_f64) else { continue };
        let minutes = w.get("window_minutes").and_then(Value::as_i64).unwrap_or(0);
        let resets_at = w
            .get("resets_at")
            .and_then(Value::as_i64)
            .or_else(|| w.get("resets_in_seconds").and_then(Value::as_i64).map(|s| at + s));
        let label = match minutes {
            0 => if key == "primary" { "Kurzes Fenster".to_string() } else { "Langes Fenster".to_string() },
            m if m % 10_080 == 0 => if m == 10_080 { "Woche".to_string() } else { format!("{} Wochen", m / 10_080) },
            m if m % 1440 == 0 => format!("{} Tage", m / 1440),
            m if m % 60 == 0 => format!("{} Stunden", m / 60),
            m => format!("{m} Minuten"),
        };
        out.push(UsageLimit { key: key.into(), label, used_percent: used.clamp(0.0, 100.0), resets_at });
    }
    out
}

// ------------------------------------------------------------------ Grok CLI

const GROK_BILLING: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const GROK_BILLING_MONTH: &str = "https://cli-chat-proxy.grok.com/v1/billing";

/// Anmeldung des Grok-CLIs (`~/.grok/auth.json`) und Abfrage des Abo-Verbrauchs.
async fn grok_limits(home: &Path, r: &AccountRow) -> Result<Vec<UsageLimit>, String> {
    let path = home.join(".grok").join("auth.json");
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(_) if r.auth_kind == "api_key" => {
            return Err("API-Schlüssel: keine Abo-Limits. Mit „grok login“ anmelden.".into());
        }
        Err(_) => return Err("Nicht angemeldet – keine Limits verfügbar.".into()),
    };
    let v: Value = serde_json::from_str(&raw).map_err(|_| "Grok-Anmeldung ist unlesbar.".to_string())?;
    let token = grok_session(&v)?;
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
    let body = grok_get(&http, &token, GROK_BILLING).await?;
    let mut list = parse_grok(&body);
    if list.is_empty() {
        // Ältere Konten: Monatsguthaben ohne Wochenprozent
        let monthly = grok_get(&http, &token, GROK_BILLING_MONTH).await?;
        list = parse_grok(&monthly);
    }
    if list.is_empty() {
        return Err("Grok meldet für dieses Konto keine Limits.".into());
    }
    Ok(list)
}

async fn grok_get(http: &reqwest::Client, token: &str, url: &str) -> Result<Value, String> {
    let resp = http
        .get(url)
        .bearer_auth(token)
        .header("x-xai-token-auth", "xai-grok-cli")
        .header("Accept", "application/json")
        .header("User-Agent", "nrzx")
        .send()
        .await
        .map_err(|e| format!("Grok nicht erreichbar: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("Anmeldung abgelaufen oder ohne Berechtigung – einmal „grok“ starten.".into());
    }
    if !status.is_success() {
        return Err(format!("Grok antwortet mit {status}."));
    }
    resp.json().await.map_err(|_| "Grok antwortet nicht mit Limits.".into())
}

/// Sitzungs-Token aus `auth.json` (Objekt je Bereich, oder ein einzelner Eintrag).
/// API-Schlüssel (`xai-…`) gelten hier nicht: die Abrechnung verlangt die Anmeldung des CLIs.
fn grok_session(v: &Value) -> Result<String, String> {
    let mut sessions: Vec<(bool, i64, String)> = Vec::new();
    let mut saw_api = false;
    let mut consider = |scope: &str, entry: &Value| {
        let Some(token) = entry
            .get("key")
            .or_else(|| entry.get("access_token"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return;
        };
        if scope.contains("api_key") || token.starts_with("xai-") {
            saw_api = true;
            return;
        }
        let exp = entry.get("expires_at").and_then(Value::as_str).and_then(parse_rfc3339).unwrap_or(0);
        let fresh = exp == 0 || exp > now();
        sessions.push((fresh, exp, token.to_string()));
    };
    if let Some(obj) = v.as_object() {
        if obj.contains_key("key") || obj.contains_key("access_token") {
            consider("", v);
        } else {
            for (k, entry) in obj {
                if entry.is_object() {
                    consider(k, entry);
                }
            }
        }
    }
    if let Some((_, _, token)) = sessions.iter().filter(|s| s.0).max_by_key(|s| s.1) {
        return Ok(token.clone());
    }
    if !sessions.is_empty() {
        return Err("Anmeldung abgelaufen – einmal „grok“ starten, dann werden die Limits gelesen.".into());
    }
    if saw_api {
        return Err("API-Schlüssel: keine Abo-Limits. Mit „grok login“ anmelden.".into());
    }
    Err("Nicht angemeldet – keine Limits verfügbar.".into())
}

/// `{"config": {"creditUsagePercent": 42.5, "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "…"},
/// "productUsage": [{"product": "GrokBuild", "usagePercent": 30}], "onDemandCap": {"val": 5000}, …}}`
fn parse_grok(v: &Value) -> Vec<UsageLimit> {
    let cfg = v.get("config").filter(|c| c.is_object()).unwrap_or(v);
    if !cfg.is_object() {
        return Vec::new();
    }
    let period = cfg.get("currentPeriod").filter(|p| p.is_object());
    let resets_at = period
        .and_then(|p| p.get("end"))
        .and_then(Value::as_str)
        .and_then(parse_rfc3339)
        .or_else(|| cfg.get("billingPeriodEnd").and_then(Value::as_str).and_then(parse_rfc3339));
    let kind = period.and_then(|p| p.get("type")).and_then(Value::as_str).unwrap_or("");
    let weekly = kind.contains("WEEK");
    let mut out = Vec::new();
    if let Some(used) = cfg.get("creditUsagePercent").and_then(Value::as_f64) {
        let label = if weekly || kind.is_empty() { "Woche" } else { "Monat" };
        out.push(UsageLimit { key: "grok".into(), label: label.into(), used_percent: used.clamp(0.0, 100.0), resets_at });
    } else if let (Some(limit), Some(used)) = (money(cfg.get("monthlyLimit")), money(cfg.get("used"))) {
        if limit > 0.0 {
            out.push(UsageLimit {
                key: "grok".into(),
                label: "Monat".into(),
                used_percent: (used / limit * 100.0).clamp(0.0, 100.0),
                resets_at,
            });
        }
    }
    if let Some(products) = cfg.get("productUsage").and_then(Value::as_array) {
        for p in products {
            let Some(used) = p.get("usagePercent").and_then(Value::as_f64) else { continue };
            let raw = p.get("product").and_then(Value::as_str).unwrap_or("");
            let label = grok_product(raw);
            if label.is_empty() {
                continue;
            }
            // Ein einziges Produkt mit demselben Prozent wie der Pool: der Wochenbalken reicht
            if out.len() == 1 && out[0].key == "grok" && (out[0].used_percent - used).abs() < 0.05 && products.len() == 1 {
                continue;
            }
            let key = format!("product_{}", raw.to_ascii_lowercase().replace('-', "_"));
            out.push(UsageLimit { key, label, used_percent: used.clamp(0.0, 100.0), resets_at });
        }
    }
    if out.iter().all(|l| l.key != "grok") {
        let products: Vec<f64> = out.iter().filter(|l| l.key.starts_with("product_")).map(|l| l.used_percent).collect();
        if let Some(used) = products.iter().copied().max_by(f64::total_cmp) {
            // Ein Produkt ist der Pool selbst – kein zweiter, gleicher Balken
            if products.len() == 1 {
                out.retain(|l| !l.key.starts_with("product_"));
            }
            out.insert(0, UsageLimit { key: "grok".into(), label: if weekly || kind.is_empty() { "Woche" } else { "Monat" }.into(), used_percent: used, resets_at });
        }
    }
    if let (Some(cap), Some(used)) = (money(cfg.get("onDemandCap")), money(cfg.get("onDemandUsed"))) {
        if cap > 0.0 {
            out.push(UsageLimit {
                key: "on_demand".into(),
                label: format!("Zusatz {} / {}", usd(used), usd(cap)),
                used_percent: (used / cap * 100.0).clamp(0.0, 100.0),
                resets_at,
            });
        }
    }
    out
}

/// Betrag in US-Cent (`{"val": 5000}` oder eine nackte Zahl).
fn money(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    v.get("val").and_then(Value::as_f64).or_else(|| v.as_f64())
}

fn usd(cents: f64) -> String {
    let d = cents / 100.0;
    if (d - d.round()).abs() < 0.005 {
        format!("${:.0}", d.round())
    } else {
        format!("${d:.2}")
    }
}

/// `GrokBuild` / `PRODUCT_GROK_BUILD` → „Grok Build“.
fn grok_product(name: &str) -> String {
    let name = name.trim().strip_prefix("PRODUCT_").unwrap_or(name.trim());
    if name.is_empty() || name.eq_ignore_ascii_case("UNSPECIFIED") {
        return String::new();
    }
    if name.contains('_') {
        return name
            .split('_')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let mut c = p.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn local_day() {
        // 2026-11-01 00:30 UTC = 01:30 in Zürich (UTC+1, getTimezoneOffset -60): Tag beginnt 23:00 UTC
        let ts = 1_793_493_000;
        assert_eq!(start_of_day(ts, None), 1_793_491_200);
        assert_eq!(start_of_day(ts, Some(-60)), 1_793_491_200 - 3600);
        // UTC-5 (+300): 2026-10-31 19:30 Ortszeit, der Tag begann am 31. um 05:00 UTC
        assert_eq!(start_of_day(ts, Some(300)), 1_793_491_200 - 86_400 + 5 * 3600);
    }

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-11-01T12:00:00.500+00:00"), Some(1_793_534_400));
        assert_eq!(parse_rfc3339("2026-11-01T14:00:00+02:00"), Some(1_793_534_400));
    }

    #[test]
    fn claude() {
        let v = json!({"seven_day": {"utilization": 40.0, "resets_at": "1970-01-02T00:00:00Z"}, "five_hour": {"utilization": 12.5, "resets_at": null}, "seven_day_opus": null});
        let l = parse_claude(&v);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].key, "five_hour");
        assert_eq!(l[1].resets_at, Some(86_400));
    }

    #[test]
    fn codex() {
        let line = json!({"type": "event_msg", "payload": {"type": "token_count", "rate_limits": {
            "primary": {"used_percent": 7.0, "window_minutes": 300, "resets_in_seconds": 60},
            "secondary": {"used_percent": 30.0, "window_minutes": 10080, "resets_at": 99}}}});
        let rl = find_key(&line, "rate_limits").unwrap();
        let l = parse_codex(rl, 1000);
        assert_eq!(l[0].label, "5 Stunden");
        assert_eq!(l[0].resets_at, Some(1060));
        assert_eq!(l[1].label, "Woche");
    }

    #[test]
    fn grok_picks_the_session_not_the_api_key() {
        let v = json!({
            "xai::api_key": {"key": "xai-secret"},
            "oidc": {"key": "eyJ.old", "expires_at": "2000-01-01T00:00:00Z"},
            "grok-shell": {"key": "eyJ.live", "refresh_token": "rt", "expires_at": "2099-01-01T00:00:00Z"}
        });
        assert_eq!(grok_session(&v).unwrap(), "eyJ.live");
        assert!(grok_session(&json!({"xai::api_key": {"key": "xai-secret"}})).is_err());
        assert!(grok_session(&json!({"oidc": {"key": "eyJ.old", "expires_at": "2000-01-01T00:00:00Z"}})).unwrap_err().contains("abgelaufen"));
    }

    #[test]
    fn grok_credits() {
        let v = json!({"config": {
            "creditUsagePercent": 42.5,
            "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "start": "2026-06-01T00:00:00Z", "end": "2026-06-08T00:00:00Z"},
            "productUsage": [
                {"product": "GrokBuild", "usagePercent": 61.25},
                {"product": "GrokChat", "usagePercent": 7.0},
                {"product": "PRODUCT_UNSPECIFIED"}
            ],
            "onDemandCap": {"val": 5000},
            "onDemandUsed": {"val": 300}
        }});
        let l = parse_grok(&v);
        assert_eq!(l[0].key, "grok");
        assert_eq!(l[0].label, "Woche");
        assert_eq!(l[0].used_percent, 42.5);
        assert_eq!(l[0].resets_at, Some(1_780_876_800));
        assert_eq!(l[1].label, "Grok Build");
        assert_eq!(l[2].label, "Grok Chat");
        assert_eq!(l[3].label, "Zusatz $3 / $50");
        assert!((l[3].used_percent - 6.0).abs() < 0.01);
        // Ein Produkt mit demselben Prozent fällt weg
        let one = parse_grok(&json!({"config": {"creditUsagePercent": 8.0, "productUsage": [{"product": "GrokBuild", "usagePercent": 8.0}]}}));
        assert_eq!(one.len(), 1);
        // Monatsguthaben in Cent, ohne Wochenprozent
        let month = parse_grok(&json!({"config": {"monthlyLimit": {"val": 15000}, "used": {"val": 7500}, "billingPeriodEnd": "1970-01-02T00:00:00Z"}}));
        assert_eq!(month[0].label, "Monat");
        assert_eq!(month[0].used_percent, 50.0);
        assert_eq!(month[0].resets_at, Some(86_400));
    }
}
