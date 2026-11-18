//! Bot-Modus: Bots anlegen, Gespräche (Threads mit bot_id) im eigenen Arbeitsordner,
//! Systemhinweis mit Persönlichkeit und Gedächtnis, Werkzeug `remember`, neue Gespräche,
//! Routinen, Rechte und Löschen.

mod common;

use common::{json, TestApp, TestUser};
use nrzx_shared::agents::{AgentAccount, Bot, ThreadDetail};
use nrzx_shared::ThreadSummary;
use rocket::http::{Header, Status};
use serde_json::{json as j, Value};
use std::path::PathBuf;
use std::time::Duration;

const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "9.9.9 (Claude Code)"; exit 0 ;;
  auth) echo '{"loggedIn":true,"authMethod":"api_key"}'; exit 0 ;;
esac
printf '%s\n' "$@" > "$HOME/last-args"
pwd > "$HOME/last-cwd"
PROMPT=$(cat)
SID="sess-$$"
prev=""
for a in "$@"; do
  [ "$prev" = "--mcp-config" ] && cp "$a" "$HOME/last-mcp.json"
  prev="$a"
done
printf '%s\n' '{"type":"system","subtype":"init","session_id":"'"$SID"'"}'
case "$PROMPT" in
  *langsam*) sleep 30; exit 0 ;;
esac
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"Hallo!"}]},"parent_tool_use_id":null}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Hallo!","session_id":"'"$SID"'"}'
"#;

async fn setup() -> TestApp {
    use std::os::unix::fs::PermissionsExt;
    let t = TestApp::new().await;
    let dir = t.state.config.tools_dir().join("npm").join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("claude");
    std::fs::write(&path, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    t
}

async fn get<T: serde::de::DeserializeOwned + Send + 'static>(t: &TestApp, u: &TestUser, path: &str) -> T {
    let res = t.client.get(path).private_cookie(u.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::Ok, "GET {path}");
    res.into_json().await.expect("json")
}

async fn send(t: &TestApp, u: &TestUser, method: &str, path: &str, body: Value) -> (Status, String) {
    let req = match method {
        "POST" => t.client.post(path),
        "PATCH" => t.client.patch(path),
        _ => t.client.delete(path),
    };
    let res = req.private_cookie(u.cookie.clone()).header(json()).body(body.to_string()).dispatch().await;
    let status = res.status();
    (status, res.into_string().await.unwrap_or_default())
}

async fn post<T: serde::de::DeserializeOwned>(t: &TestApp, u: &TestUser, path: &str, body: Value) -> T {
    let (status, text) = send(t, u, "POST", path, body).await;
    assert_eq!(status, Status::Ok, "POST {path}: {text}");
    serde_json::from_str(&text).expect("json")
}

fn home(t: &TestApp, account: i64) -> PathBuf {
    t.state.config.accounts_dir().join(account.to_string()).join("home")
}

async fn settle(t: &TestApp, u: &TestUser, id: i64) {
    for _ in 0..300 {
        let d: ThreadDetail = get(t, u, &format!("/api/v1/threads/{id}")).await;
        if !d.thread.status.is_active() && d.runs.iter().all(|r| r.status != "running" && r.status != "queued") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("Thread {id} wurde nicht fertig");
}

#[rocket::async_test]
async fn bots_conversations_memory_and_routines() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let ben = t.user("ben", false).await;
    let acc: AgentAccount = post(&t, &anna, "/api/v1/accounts", j!({"kind": "claude", "label": "Claude", "authKind": "api_key", "secret": "sk-ant-api03-abcdefgh"})).await;

    // Ohne Namen kein Bot
    let (st, _) = send(&t, &anna, "POST", "/api/v1/bots", j!({"name": "  "})).await;
    assert_eq!(st, Status::BadRequest);

    let bot: Bot = post(&t, &anna, "/api/v1/bots", j!({"name": "Lena", "emoji": "🦊", "role": "Recherche", "instructions": "Antworte immer kurz."})).await;
    assert_eq!((bot.name.as_str(), bot.emoji.as_str(), bot.account_id), ("Lena", "🦊", Some(acc.id)));
    let tid = bot.thread_id.expect("erstes Gespräch");
    assert!(t.state.config.data_dir.join("bots").join(bot.id.to_string()).is_dir(), "eigener Arbeitsordner");

    // Das Gespräch ist ein Thread mit bot_id
    let list: Vec<ThreadSummary> = get(&t, &anna, "/api/v1/threads?parent=none").await;
    let conv = list.iter().find(|x| x.id == tid).expect("Gespräch in der Liste");
    assert_eq!(conv.bot_id, Some(bot.id));
    assert_eq!(conv.title, "Lena");

    // Lauf: im Arbeitsordner des Bots, Hinweis mit Persönlichkeit und Gedächtnis
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{tid}/messages"), j!({"text": "Hallo Lena"})).await;
    settle(&t, &anna, tid).await;
    let cwd = std::fs::read_to_string(home(&t, acc.id).join("last-cwd")).unwrap();
    assert!(cwd.trim().ends_with(&format!("bots/{}", bot.id)), "{cwd}");
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    assert!(args.contains("You are \"Lena\" (Recherche)"), "{args}");
    assert!(args.contains("Antworte immer kurz."), "{args}");
    assert!(args.contains("(empty)"), "{args}");
    assert!(!args.contains("coding agents). cwd"), "kein Coding-Hinweis: {args}");

    // Langsamer Lauf: über MCP etwas merken
    let _ = std::fs::remove_file(home(&t, acc.id).join("last-mcp.json"));
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{tid}/messages"), j!({"text": "Arbeite langsam"})).await;
    let cfg = home(&t, acc.id).join("last-mcp.json");
    for _ in 0..200 {
        if cfg.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let mcp: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let auth = mcp["mcpServers"]["nrzx"]["headers"]["Authorization"].as_str().unwrap().to_string();
    let rpc = |body: Value| {
        let auth = auth.clone();
        let client = &t.client;
        async move {
            let res = client.post("/mcp").header(Header::new("Authorization", auth)).header(json()).body(body.to_string()).dispatch().await;
            res.into_json::<Value>().await.unwrap_or(Value::Null)
        }
    };
    let tools = rpc(j!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"remember") && names.contains(&"rewrite_memory"), "{names:?}");
    let r = rpc(j!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "remember", "arguments": {"fact": "Anna trinkt gern Tee"}}})).await;
    assert_eq!(r["result"]["isError"], false, "{r}");
    rpc(j!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "remember", "arguments": {"fact": "- Arbeitet an nrzX"}}})).await;
    let b: Bot = get(&t, &anna, &format!("/api/v1/bots/{}", bot.id)).await;
    assert_eq!(b.memory, "- Anna trinkt gern Tee\n- Arbeitet an nrzX");
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{tid}/stop"), j!({})).await;
    settle(&t, &anna, tid).await;

    // Gewöhnliche Threads haben kein Gedächtnis-Werkzeug (siehe tests/agents.rs) – hier: Ändern
    let (st, text) = send(&t, &anna, "PATCH", &format!("/api/v1/bots/{}", bot.id), j!({"name": "Lena Fuchs", "memory": "- neu"})).await;
    assert_eq!(st, Status::Ok, "{text}");
    let b: Bot = serde_json::from_str(&text).unwrap();
    assert_eq!((b.name.as_str(), b.memory.as_str()), ("Lena Fuchs", "- neu"));

    // Neues Gespräch: anderer Thread, Gedächtnis bleibt; das alte lässt sich wieder öffnen
    let b2: Bot = post(&t, &anna, &format!("/api/v1/bots/{}/conversation", bot.id), j!({})).await;
    let tid2 = b2.thread_id.unwrap();
    assert_ne!(tid2, tid);
    assert_eq!(b2.memory, "- neu");
    let b3: Bot = post(&t, &anna, &format!("/api/v1/bots/{}/conversation/{tid}", bot.id), j!({})).await;
    assert_eq!(b3.thread_id, Some(tid));

    // Routinen
    let (st, _) = send(&t, &anna, "POST", &format!("/api/v1/bots/{}/routines", bot.id), j!({"prompt": "x", "timeMin": 480, "days": 0})).await;
    assert_eq!(st, Status::BadRequest, "ohne Wochentag");
    let b4: Bot = post(&t, &anna, &format!("/api/v1/bots/{}/routines", bot.id), j!({"title": "Morgen", "prompt": "Fasse die News zusammen", "timeMin": 450, "days": 31, "tzOffset": 60, "enabled": true})).await;
    assert_eq!(b4.routines.len(), 1);
    assert_eq!((b4.routines[0].time_min, b4.routines[0].days, b4.routines[0].tz_offset), (450, 31, 60));
    let rid = b4.routines[0].id;
    let (st, text) = send(&t, &anna, "PATCH", &format!("/api/v1/bots/{}/routines/{rid}", bot.id), j!({"title": "Morgen", "prompt": "Wetter", "timeMin": 420, "days": 127, "tzOffset": 60, "enabled": false})).await;
    assert_eq!(st, Status::Ok, "{text}");
    assert!(!serde_json::from_str::<Bot>(&text).unwrap().routines[0].enabled);

    // Fremde Bots gibt es nicht
    let res = t.client.get(format!("/api/v1/bots/{}", bot.id)).private_cookie(ben.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);
    let ben_bots: Vec<Bot> = get(&t, &ben, "/api/v1/bots").await;
    assert!(ben_bots.is_empty());
    let (st, _) = send(&t, &ben, "POST", &format!("/api/v1/bots/{}/conversation/{tid}", bot.id), j!({})).await;
    assert_eq!(st, Status::NotFound);

    // Löschen: Gespräche, Routinen und Arbeitsordner sind weg
    let (st, _) = send(&t, &anna, "DELETE", &format!("/api/v1/bots/{}", bot.id), j!({})).await;
    assert_eq!(st, Status::NoContent);
    let list: Vec<ThreadSummary> = get(&t, &anna, "/api/v1/threads").await;
    assert!(list.iter().all(|x| x.bot_id.is_none()), "{list:?}");
    assert!(!t.state.config.data_dir.join("bots").join(bot.id.to_string()).exists());
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bot_routines").fetch_one(t.db()).await.unwrap();
    assert_eq!(n, 0);
}
