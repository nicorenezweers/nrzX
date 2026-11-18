//! KI-Ketten: Hat jemand mehrere Abos, legt er fest, wer übernimmt, wenn eines am Limit ist –
//! z. B. Claude Abo 1 → Claude Abo 2 → Grok Abo 1. Bricht ein Lauf mit einem Rate-Limit ab,
//! wechselt der Thread auf die nächste Stufe, die nicht selbst am Limit ist, und macht sofort
//! weiter (mit Verlauf, denn Sitzungen gehören zu CLI und Konto). Vor jedem Lauf prüft
//! [`preflight`], ob das Konto bekanntermassen am Limit ist, und wechselt dann gleich.
//!
//! Gespeichert je Benutzer in `settings` (`chains:<user>`), Routen `GET/PUT /chains`.
//! „Am Limit“ heisst: zuletzt mit Rate-Limit abgebrochen (bis zum Zurücksetzen bzw. eine Stunde)
//! oder ein Limit des Abos steht laut Anbieter bei 99 % oder mehr (Claude, Codex).

use super::accounts::{self, AccountRow};
use super::{messages, runs, runtime, threads, tools, usage};
use crate::auth::CurrentUser;
use crate::db::{self, now};
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::{AiChain, ChainBlock, ChainSettings, ChainStep, SystemData};
use rocket::serde::json::Json;
use rocket::{get, put, routes, Route, State};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

pub fn routes() -> Vec<Route> {
    routes![get_chains, put_chains]
}

/// Prompt des Laufs nach dem Wechsel.
pub const CHAIN_PROMPT: &str = "The previous account reached its usage limit, so this conversation continues on another account. \
                                Please continue exactly where you left off and finish the task.";

const MAX_CHAINS: usize = 12;
const MAX_STEPS: usize = 8;

/// Konten am Limit: Konto → bis (Unix-Sekunden).
static BLOCKED: LazyLock<Mutex<HashMap<i64, i64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn key(user_id: i64) -> String {
    format!("chains:{user_id}")
}

/// Ketten des Benutzers.
pub async fn load(state: &AppState, user_id: i64) -> AppResult<Vec<AiChain>> {
    let raw = db::setting(&state.db, &key(user_id), "").await?;
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

/// Konto bis `until` als „am Limit“ merken.
pub fn block(account_id: i64, until: i64) {
    let mut map = BLOCKED.lock().unwrap_or_else(|e| e.into_inner());
    let ts = now();
    map.retain(|_, u| *u > ts);
    let e = map.entry(account_id).or_insert(until);
    *e = (*e).max(until);
}

fn blocked_until(account_id: i64) -> Option<i64> {
    let ts = now();
    BLOCKED.lock().unwrap_or_else(|e| e.into_inner()).get(&account_id).copied().filter(|u| *u > ts)
}

fn blocked_list(ids: &[i64]) -> Vec<ChainBlock> {
    ids.iter().filter_map(|id| blocked_until(*id).map(|until| ChainBlock { account_id: *id, until })).collect()
}

/// Ist das Konto am Limit? (gemerkt oder laut den Limits des Anbieters, zwischengespeichert)
async fn exhausted(state: &AppState, acc: &AccountRow) -> bool {
    if blocked_until(acc.id).is_some() {
        return true;
    }
    let ts = now();
    let (limits, _, _, _) = usage::limits(state, acc, false).await;
    limits.iter().any(|l| l.used_percent >= 99.0 && l.resets_at.is_some_and(|r| r > ts))
}

/// Erste aktive Kette mit diesem Konto.
fn chain_of(chains: &[AiChain], account_id: i64) -> Option<AiChain> {
    chains.iter().find(|c| c.enabled && c.steps.len() > 1 && c.steps.iter().any(|s| s.account_id == account_id)).cloned()
}

/// Nächste brauchbare Stufe nach dem Konto (danach von vorn): nicht am Limit, CLI installiert.
async fn next_step(state: &AppState, user_id: i64, chain: &AiChain, current: i64) -> Option<(AccountRow, ChainStep)> {
    let pos = chain.steps.iter().position(|s| s.account_id == current)?;
    let n = chain.steps.len();
    for i in 1..n {
        let step = &chain.steps[(pos + i) % n];
        if step.account_id == current {
            continue;
        }
        let Ok(acc) = accounts::get_row(state, user_id, step.account_id).await else { continue };
        if tools::find_agent(state, acc.kind()).is_none() || exhausted(state, &acc).await {
            continue;
        }
        return Some((acc, step.clone()));
    }
    None
}

/// Geplanter Wechsel: neues Konto und Stufe der Kette.
pub struct Plan {
    chain: AiChain,
    acc: AccountRow,
    step: ChainStep,
}

/// Nach einem Rate-Limit: Konto als „am Limit“ merken und – wenn eine Kette weiterhilft – den
/// Wechsel planen. `reset` = Zurücksetzen laut Meldung (sonst laut Anbieter bzw. in einer Stunde).
pub async fn plan(state: &AppState, t: &threads::ThreadRow, reset: Option<i64>, text: &str) -> AppResult<Option<Plan>> {
    let Some(acc_id) = t.account_id else { return Ok(None) };
    let ts = now();
    let mut until = reset;
    if until.is_none() {
        if let Ok(acc) = accounts::get_row(state, t.user_id, acc_id).await {
            let (limits, _, _, _) = usage::limits(state, &acc, true).await;
            until = limits.iter().filter(|l| l.used_percent >= 99.0).filter_map(|l| l.resets_at).filter(|r| *r > ts).max();
        }
    }
    // Überlastung ist meist schnell vorbei, ein Limit nicht
    let fallback = if text.to_lowercase().contains("overloaded") { 300 } else { 3600 };
    block(acc_id, until.filter(|u| *u > ts).unwrap_or(ts + fallback));
    let chains = load(state, t.user_id).await?;
    let Some(chain) = chain_of(&chains, acc_id) else { return Ok(None) };
    Ok(next_step(state, t.user_id, &chain, acc_id).await.map(|(acc, step)| Plan { chain, acc, step }))
}

/// Thread auf die geplante Stufe umstellen und einen Hinweis in den Verlauf schreiben.
pub async fn apply(state: &AppState, t: &threads::ThreadRow, plan: &Plan) -> AppResult<()> {
    let kind = plan.acc.kind();
    let same = kind == t.kind();
    let model = if !plan.step.model.trim().is_empty() {
        plan.step.model.trim().to_string()
    } else if same {
        t.model.clone()
    } else {
        String::new()
    };
    let effort = if same { t.effort.clone() } else { String::new() };
    let (account_id, model, effort) = threads::resolve_agent(state, t.user_id, kind, Some(plan.acc.id), &model, &effort).await?;
    sqlx::query("UPDATE threads SET agent_kind = ?, account_id = ?, model = ?, effort = ?, provider_session_id = '', updated_at = ? WHERE id = ?")
        .bind(kind.key())
        .bind(account_id)
        .bind(&model)
        .bind(&effort)
        .bind(now())
        .bind(t.id)
        .execute(&state.db)
        .await?;
    let data = messages::jv(&SystemData { kind: "chain_switch".into(), resets_at: None, detail: format!("{} · {}", kind.name(), plan.acc.label) });
    let name = if plan.chain.name.trim().is_empty() { "KI-Kette".to_string() } else { plan.chain.name.trim().to_string() };
    messages::insert(state, t.user_id, t.id, None, "system", &name, &data).await?;
    threads::publish(state, t.id).await
}

/// Gleich weitermachen, sobald der abgebrochene Lauf ganz beendet ist.
pub fn continue_soon(state: &AppState, thread_id: i64) {
    let state = state.clone();
    tokio::spawn(async move {
        for _ in 0..120 {
            if !runtime::get(&state).is_active(thread_id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Err(e) = runs::start_with(&state, thread_id, CHAIN_PROMPT, &[], true).await {
            tracing::warn!("KI-Kette: Thread {thread_id} fortsetzen: {e}");
        }
    });
}

/// Vor einem Lauf: Ist das Konto des Threads bekanntermassen am Limit und hilft eine Kette,
/// gleich auf die nächste Stufe wechseln.
pub async fn preflight(state: &AppState, thread_id: i64) -> AppResult<()> {
    let t = threads::row(state, thread_id).await?;
    let Some(acc_id) = t.account_id else { return Ok(()) };
    let chains = load(state, t.user_id).await?;
    let Some(chain) = chain_of(&chains, acc_id) else { return Ok(()) };
    let Ok(acc) = accounts::get_row(state, t.user_id, acc_id).await else { return Ok(()) };
    if !exhausted(state, &acc).await {
        return Ok(());
    }
    if let Some((acc, step)) = next_step(state, t.user_id, &chain, acc_id).await {
        apply(state, &t, &Plan { chain, acc, step }).await?;
    }
    Ok(())
}

/// `GET /chains`
#[get("/chains")]
async fn get_chains(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<ChainSettings>> {
    let chains = load(state, user.id()).await?;
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM agent_accounts WHERE user_id = ?").bind(user.id()).fetch_all(&state.db).await?;
    Ok(Json(ChainSettings { chains, blocked: blocked_list(&ids) }))
}

/// `PUT /chains` – Ketten speichern (Konten müssen dem Benutzer gehören).
#[put("/chains", data = "<body>")]
async fn put_chains(state: &State<AppState>, user: CurrentUser, body: Json<ChainSettings>) -> AppResult<Json<ChainSettings>> {
    let mut out: Vec<AiChain> = Vec::new();
    let input = body.into_inner().chains;
    if input.len() > MAX_CHAINS {
        return Err(AppError::bad("Zu viele Ketten."));
    }
    for (i, c) in input.into_iter().enumerate() {
        let mut steps: Vec<ChainStep> = Vec::new();
        for s in c.steps {
            if steps.iter().any(|x| x.account_id == s.account_id) {
                return Err(AppError::bad("Ein Konto kommt in einer Kette nur einmal vor."));
            }
            accounts::get_row(state, user.id(), s.account_id).await.map_err(|_| AppError::bad("Unbekanntes Konto in der Kette."))?;
            let model: String = s.model.trim().chars().take(200).collect();
            if model.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err(AppError::bad("Ungültiger Modellname."));
            }
            steps.push(ChainStep { account_id: s.account_id, model });
        }
        if steps.len() > MAX_STEPS {
            return Err(AppError::bad("Zu viele Stufen in einer Kette."));
        }
        let id: String = c.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
        let name: String = c.name.trim().chars().filter(|c| !c.is_control()).take(60).collect();
        out.push(AiChain { id: if id.is_empty() { format!("c{}", i + 1) } else { id }, name, enabled: c.enabled, steps });
    }
    db::set_setting(&state.db, &key(user.id()), &serde_json::to_string(&out).unwrap_or_else(|_| "[]".into())).await?;
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM agent_accounts WHERE user_id = ?").bind(user.id()).fetch_all(&state.db).await?;
    Ok(Json(ChainSettings { chains: out, blocked: blocked_list(&ids) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(enabled: bool, ids: &[i64]) -> AiChain {
        AiChain { id: "c".into(), name: "K".into(), enabled, steps: ids.iter().map(|id| ChainStep { account_id: *id, model: String::new() }).collect() }
    }

    #[test]
    fn finds_active_chain() {
        let list = vec![chain(false, &[1, 2]), chain(true, &[3]), chain(true, &[4, 1, 5])];
        assert_eq!(chain_of(&list, 1).map(|c| c.steps.len()), Some(3));
        assert!(chain_of(&list, 3).is_none(), "eine Stufe allein ist keine Kette");
        assert!(chain_of(&list, 9).is_none());
    }

    #[test]
    fn blocks_until_reset() {
        let ts = now();
        block(777_001, ts + 100);
        block(777_001, ts + 50);
        assert_eq!(blocked_until(777_001), Some(ts + 100));
        block(777_002, ts - 5);
        assert_eq!(blocked_until(777_002), None);
    }
}
