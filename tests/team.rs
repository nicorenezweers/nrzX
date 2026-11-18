//! Team: zwei Leute mit je eigenem Projekt am selben Repository – Mitglieder und Nachrichten
//! gehen über `refs/nrzx/team`, Agenten schreiben und lesen über die MCP-Werkzeuge.

mod common;

use common::{json, TestApp, TestUser};
use nrzx_shared::team::TeamView;
use rocket::http::{Header, Status};
use serde_json::{json as j, Value};
use std::path::Path;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Zwei Benutzer, zwei Projekte, ein gemeinsames (leeres) Repository als „origin“.
async fn setup() -> (TestApp, TestUser, i64, TestUser, i64, std::path::PathBuf) {
    let t = TestApp::new().await;
    let alice = t.user("Alice", false).await;
    let bob = t.user("bob", false).await;
    let bare = t.dir.path().join("shared.git");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare"]);
    let (pa, ra) = t.project(alice.id, "Shop").await;
    let (pb, rb) = t.project(bob.id, "Shop").await;
    for r in [&ra, &rb] {
        git(r, &["remote", "add", "origin", &bare.to_string_lossy()]);
    }
    (t, alice, pa, bob, pb, bare)
}

async fn call(t: &TestApp, u: &TestUser, method: &str, path: &str, body: Value) -> (Status, String) {
    let req = match method {
        "GET" => t.client.get(path),
        "PUT" => t.client.put(path),
        _ => t.client.post(path),
    };
    let res = req.private_cookie(u.cookie.clone()).header(json()).body(body.to_string()).dispatch().await;
    (res.status(), res.into_string().await.unwrap_or_default())
}

async fn view(t: &TestApp, u: &TestUser, method: &str, path: &str, body: Value) -> TeamView {
    let (status, text) = call(t, u, method, path, body).await;
    assert_eq!(status, Status::Ok, "{method} {path}: {text}");
    serde_json::from_str(&text).expect("TeamView")
}

/// Lauf mit Token für den MCP-Server anlegen.
async fn run_token(t: &TestApp, thread_id: i64) -> String {
    let now = nrzx::db::now();
    let run: i64 = sqlx::query_scalar("INSERT INTO runs (thread_id, agent_kind, status, started_at) VALUES (?, 'claude', 'running', ?) RETURNING id")
        .bind(thread_id)
        .bind(now)
        .fetch_one(t.db())
        .await
        .unwrap();
    let token = format!("tok-{thread_id}-{run}");
    sqlx::query("INSERT INTO run_tokens (token_hash, run_id, thread_id, created_at) VALUES (?, ?, ?, ?)")
        .bind(nrzx::auth::session::hash_token(&token))
        .bind(run)
        .bind(thread_id)
        .bind(now)
        .execute(t.db())
        .await
        .unwrap();
    token
}

async fn mcp(t: &TestApp, token: &str, method: &str, params: Value) -> Value {
    let res = t
        .client
        .post("/mcp")
        .header(json())
        .header(Header::new("Authorization", format!("Bearer {token}")))
        .body(j!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    res.into_json().await.expect("json")
}

async fn tool(t: &TestApp, token: &str, name: &str, args: Value) -> (bool, Value) {
    let r = mcp(t, token, "tools/call", j!({"name": name, "arguments": args})).await;
    let error = r["result"]["isError"].as_bool().unwrap_or(true);
    let body = if error { r["result"]["content"][0]["text"].clone() } else { r["result"]["structuredContent"].clone() };
    (error, body)
}

#[rocket::async_test]
async fn members_and_messages_travel_through_the_repository() {
    let (t, alice, pa, bob, pb, bare) = setup().await;

    // Aus: nichts zu sehen, aber einschaltbar
    let v = view(&t, &alice, "GET", &format!("/api/v1/projects/{pa}/team"), j!({})).await;
    assert!(v.available && !v.enabled && v.members.is_empty());

    let v = view(&t, &alice, "PUT", &format!("/api/v1/projects/{pa}/team"), j!({"enabled": true})).await;
    assert!(v.enabled, "{v:?}");
    assert_eq!(v.error, "");
    assert_eq!(v.me, "alice");
    assert_eq!(v.members.len(), 1);
    assert_eq!(v.members[0].name, "Alice");
    // Die Referenz steht im gemeinsamen Repository
    assert!(!git(&bare, &["rev-parse", "refs/nrzx/team"]).is_empty());

    let v = view(&t, &bob, "PUT", &format!("/api/v1/projects/{pb}/team"), j!({"enabled": true})).await;
    assert_eq!(v.error, "");
    assert_eq!(v.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["bob", "Alice"]);

    // Alice schreibt an alle, Bob sieht es nach dem Abgleich
    let v = view(&t, &alice, "POST", &format!("/api/v1/projects/{pa}/team/messages"), j!({"to": "all", "text": "Ich baue die Kasse um."})).await;
    assert_eq!(v.pending, 0);
    assert_eq!(v.members.len(), 2);
    let v = view(&t, &bob, "POST", &format!("/api/v1/projects/{pb}/team/sync"), j!({})).await;
    assert_eq!(v.messages.len(), 1);
    assert_eq!(v.messages[0].text, "Ich baue die Kasse um.");
    assert_eq!(v.messages[0].from, "alice");

    // Übersicht aller Teams (Handy)
    let (status, text) = call(&t, &bob, "GET", "/api/v1/projects/teams", j!({})).await;
    assert_eq!(status, Status::Ok, "{text}");
    let all: Vec<TeamView> = serde_json::from_str(&text).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].project_id, all[0].project_name.as_str()), (pb, "Shop"));

    // Unbekannter Empfänger
    let (status, text) = call(&t, &bob, "POST", &format!("/api/v1/projects/{pb}/team/messages"), j!({"to": "carol", "text": "Hallo"})).await;
    assert_eq!(status, Status::BadRequest, "{text}");

    // Fremde Projekte gibt es nicht
    let (status, _) = call(&t, &bob, "GET", &format!("/api/v1/projects/{pa}/team"), j!({})).await;
    assert_eq!(status, Status::NotFound);

    // Ausschalten trägt aus
    let v = view(&t, &alice, "PUT", &format!("/api/v1/projects/{pa}/team"), j!({"enabled": false})).await;
    assert!(!v.enabled);
    let v = view(&t, &bob, "POST", &format!("/api/v1/projects/{pb}/team/sync"), j!({})).await;
    assert_eq!(v.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["bob"]);
    // Der Verlauf bleibt
    assert_eq!(v.messages.len(), 1);
}

#[rocket::async_test]
async fn agents_talk_to_each_other() {
    let (t, alice, pa, bob, pb, _bare) = setup().await;
    let ta = t.thread(alice.id, Some(pa), "Kasse umbauen").await;
    let tb = t.thread(bob.id, Some(pb), "Warenkorb").await;
    let tok_a = run_token(&t, ta).await;
    let tok_b = run_token(&t, tb).await;

    // Ohne Team keine Werkzeuge
    let list = mcp(&t, &tok_a, "tools/list", j!({})).await;
    assert!(!list.to_string().contains("team_send"));
    let (error, _) = tool(&t, &tok_a, "team_status", j!({})).await;
    assert!(error);

    view(&t, &alice, "PUT", &format!("/api/v1/projects/{pa}/team"), j!({"enabled": true})).await;
    view(&t, &bob, "PUT", &format!("/api/v1/projects/{pb}/team"), j!({"enabled": true})).await;
    let list = mcp(&t, &tok_a, "tools/list", j!({})).await;
    for name in ["team_status", "team_send", "team_read"] {
        assert!(list.to_string().contains(name), "{name}");
    }

    // Alice' Agent sieht Bobs Agenten
    let (error, status) = tool(&t, &tok_a, "team_status", j!({})).await;
    assert!(!error, "{status}");
    assert_eq!(status["you"], format!("alice#{ta}"));
    let text = status.to_string();
    assert!(text.contains(&format!("bob#{tb}")) && text.contains("Warenkorb"), "{text}");

    // … und fragt ihn etwas
    let (error, sent) = tool(&t, &tok_a, "team_send", j!({"to": format!("bob#{tb}"), "text": "Welche Felder hat der Warenkorb?"})).await;
    assert!(!error, "{sent}");
    assert_eq!(sent["sent"], true);
    let (error, _) = tool(&t, &tok_a, "team_send", j!({"to": "bob#99999", "text": "?"})).await;
    assert!(error);

    // Bobs Agent liest (einmal) und antwortet
    let (error, got) = tool(&t, &tok_b, "team_read", j!({})).await;
    assert!(!error, "{got}");
    assert_eq!(got["messages"].as_array().map(Vec::len), Some(1), "{got}");
    assert_eq!(got["messages"][0]["from"], format!("alice#{ta}"));
    let id = got["messages"][0]["id"].as_str().unwrap().to_string();
    let (_, again) = tool(&t, &tok_b, "team_read", j!({})).await;
    assert_eq!(again["messages"].as_array().map(Vec::len), Some(0));
    let (error, _) = tool(&t, &tok_b, "team_send", j!({"to": format!("alice#{ta}"), "text": "id, menge, preis", "replyTo": id})).await;
    assert!(!error);

    // Alice' Agent wartet auf die Antwort
    let (error, got) = tool(&t, &tok_a, "team_read", j!({"waitSeconds": 20})).await;
    assert!(!error, "{got}");
    assert_eq!(got["messages"][0]["text"], "id, menge, preis");
    assert_eq!(got["messages"][0]["replyTo"], id);

    // Ein anderer Thread von Alice bekommt die an den einen Agenten gerichtete Antwort nicht
    let other = t.thread(alice.id, Some(pa), "Doku").await;
    let tok_o = run_token(&t, other).await;
    let (_, got) = tool(&t, &tok_o, "team_read", j!({})).await;
    assert_eq!(got["messages"].as_array().map(Vec::len), Some(0), "{got}");

    // Der nächste Lauf bekommt Kollegen und Neues in den Hinweis
    view(&t, &bob, "POST", &format!("/api/v1/projects/{pb}/team/messages"), j!({"to": "all", "text": "Bitte main nicht anfassen."})).await;
    view(&t, &alice, "POST", &format!("/api/v1/projects/{pa}/team/sync"), j!({})).await;
    let row = nrzx::agents::threads::row(&t.state, ta).await.unwrap();
    let hint = nrzx::team::hint(&t.state, &row).await;
    assert!(hint.contains(&format!("alice#{ta}")) && hint.contains("Bitte main nicht anfassen."), "{hint}");
    let hint = nrzx::team::hint(&t.state, &row).await;
    assert!(!hint.contains("Bitte main nicht anfassen."), "nur einmal zustellen: {hint}");
}

#[rocket::async_test]
async fn needs_a_shared_repository() {
    let t = TestApp::new().await;
    let u = t.user("solo", false).await;
    let (p, _) = t.project(u.id, "Lokal").await;
    let v = view(&t, &u, "GET", &format!("/api/v1/projects/{p}/team"), j!({})).await;
    assert!(!v.available);
    let (status, _) = call(&t, &u, "PUT", &format!("/api/v1/projects/{p}/team"), j!({"enabled": true})).await;
    assert_eq!(status, Status::Conflict);
}
