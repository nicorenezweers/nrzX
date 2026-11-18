//! Nutzung der Agent-Konten (`GET /api/v1/usage`): Verbrauch aus den Läufen in nrzX
//! (`usage_log`, bleibt beim Löschen von Threads erhalten; 5 Stunden, heute, 7 und 30 Tage)
//! und – wo der Anbieter sie verrät – die Limits des Abos:
//!
//! * Claude Code (Anmeldung im CLI): `GET https://api.anthropic.com/api/oauth/usage` mit dem
//!   OAuth-Token aus `<home>/.claude/.credentials.json` (5-Stunden-Fenster, Woche, Woche Opus …)
//! * Codex: letzte `rate_limits` aus den Sitzungsprotokollen des CLIs
//!   (`<home>/.codex/sessions/**/rollout-*.jsonl`, primäres und sekundäres Fenster)
//!
//! Gemini CLI, OpenCode, Kilo Code und Grok CLI melden keine Limits – dort zählt nrzX Läufe,
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
}
