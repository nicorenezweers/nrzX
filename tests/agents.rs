//! Bereich agents: Threads, Läufe mit nachgeahmten CLIs (Claude Code, Codex), vereinheitlichte
//! Nachrichten, Fortsetzen mit Sitzungs-ID, Stoppen, Konto-Isolation, MCP-Server mit
//! Subagenten, Terminals und Rechte.
//!
//! Die nachgeahmten CLIs sind kleine Skripte, die der Test in `<data>/tools/npm/bin` schreibt –
//! dort sucht nrzX zuerst. Sie geben die JSON-Zeilen der echten Programme aus und legen
//! Argumente, Umgebung und Prompt im Home des Kontos ab.

mod common;

use base64::Engine;
use common::{json, TestApp, TestUser};
use nrzx_shared::agents::{AgentAccount, AgentInfo, TerminalInfo, ThreadDetail, ToolCall};
use nrzx_shared::{AgentKind, Message, ThreadStatus, ThreadSummary};
use rocket::http::{Header, Status};
use serde_json::{json as j, Value};
use std::path::PathBuf;
use std::time::Duration;

const FAKE_CLAUDE: &str = r#"#!/bin/sh
# Nachgeahmtes Claude Code (Test): stream-json wie `claude -p --output-format stream-json`
case "$1" in
  --version) echo "9.9.9 (Claude Code)"; exit 0 ;;
  auth) echo '{"loggedIn":true,"authMethod":"api_key"}'; exit 0 ;;
esac
printf '%s\n' "$@" > "$HOME/last-args"
pwd > "$HOME/last-cwd"
env > "$HOME/last-env"
PROMPT=$(cat)
printf '%s' "$PROMPT" > "$HOME/last-prompt"
SID="sess-$$"
prev=""
for a in "$@"; do
  [ "$prev" = "--resume" ] && SID="$a"
  [ "$prev" = "--mcp-config" ] && cp "$a" "$HOME/last-mcp.json"
  prev="$a"
done
printf '%s\n' '{"type":"system","subtype":"init","session_id":"'"$SID"'","mcp_servers":[{"name":"nrzx","status":"connected"}]}'
case "$PROMPT" in
  *langsam*)
    printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"Ich arbeite lange."}]},"parent_tool_use_id":null}'
    sleep 30
    exit 0 ;;
  *kaputt*)
    printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Etwas ging schief","session_id":"'"$SID"'"}'
    exit 1 ;;
  *bearbeite*)
    printf 'eins\nzwei\ndrei\nvier\nfünf\nsechs\nsieben\nacht\n' > "$PWD/zahlen.txt"
    printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_e","name":"Edit","input":{"file_path":"'"$PWD"'/zahlen.txt","old_string":"fünf","new_string":"FÜNF\nfünfeinhalb"}}]},"parent_tool_use_id":null}'
    sleep 0.3
    printf 'eins\nzwei\ndrei\nvier\nFÜNF\nfünfeinhalb\nsechs\nsieben\nacht\n' > "$PWD/zahlen.txt"
    printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_e","content":"ok","is_error":false}]},"parent_tool_use_id":null}'
    printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_w","name":"Write","input":{"file_path":"'"$PWD"'/neu.txt","content":"a\nb\n"}}]},"parent_tool_use_id":null}'
    sleep 0.3
    printf 'a\nb\n' > "$PWD/neu.txt"
    printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_w","content":"ok","is_error":false}]},"parent_tool_use_id":null}'
    printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Fertig","session_id":"'"$SID"'"}'
    exit 0 ;;
  *ratelimit*)
    RESET=$(( $(date +%s) + 3600 ))
    printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"result":"Claude AI usage limit reached|'"$RESET"'","session_id":"'"$SID"'"}'
    exit 1 ;;
esac
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hallo "}},"parent_tool_use_id":null}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Welt"}},"parent_tool_use_id":null}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"Hallo Welt"}]},"parent_tool_use_id":null}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Edit","input":{"file_path":"'"$PWD"'/src/main.rs","old_string":"alt","new_string":"neu"}}]},"parent_tool_use_id":null}'
printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"Datei geändert","is_error":false}]},"parent_tool_use_id":null}'
echo 'Warnung: das ist kein JSON'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"Fertig: **main.rs** geändert."}]},"parent_tool_use_id":null}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Fertig","session_id":"'"$SID"'","total_cost_usd":0.25,"usage":{"input_tokens":100,"cache_read_input_tokens":20,"output_tokens":30}}'
"#;

const FAKE_CODEX: &str = r#"#!/bin/sh
# Nachgeahmtes Codex (Test): JSONL wie `codex exec --json`
case "$1" in
  --version) echo "codex-cli 0.0.1"; exit 0 ;;
  login) echo "Logged in using an API key"; exit 0 ;;
esac
printf '%s\n' "$@" > "$HOME/last-args"
env > "$HOME/last-env"
PROMPT=$(cat)
printf '%s' "$PROMPT" > "$HOME/last-prompt"
TID="codex-$$"
[ "$2" = "resume" ] && TID="$3"
printf '%s\n' '{"type":"thread.started","thread_id":"'"$TID"'"}'
printf '%s\n' '{"type":"turn.started"}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"**Plan** erst lesen"}}'
printf '%s\n' '{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","aggregated_output":"","exit_code":null,"status":"in_progress"}}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","aggregated_output":"README.md\n","exit_code":0,"status":"completed"}}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Codex fertig"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":50,"cached_input_tokens":0,"output_tokens":10}}'
"#;

fn install_fake(t: &TestApp, program: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let dir = t.state.config.tools_dir().join("npm").join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(program);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

async fn setup() -> TestApp {
    let t = TestApp::new().await;
    install_fake(&t, "claude", FAKE_CLAUDE);
    install_fake(&t, "codex", FAKE_CODEX);
    t
}

async fn get<T: serde::de::DeserializeOwned + Send + 'static>(t: &TestApp, u: &TestUser, path: &str) -> T {
    let res = t.client.get(path).private_cookie(u.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::Ok, "GET {path}");
    res.into_json().await.expect("json")
}

async fn post<T: serde::de::DeserializeOwned>(t: &TestApp, u: &TestUser, path: &str, body: Value) -> T {
    let res = t.client.post(path).private_cookie(u.cookie.clone()).header(json()).body(body.to_string()).dispatch().await;
    let status = res.status();
    let text = res.into_string().await.unwrap_or_default();
    assert_eq!(status, Status::Ok, "POST {path}: {text}");
    serde_json::from_str(&text).expect("json")
}

async fn account(t: &TestApp, u: &TestUser, kind: &str, label: &str, key: &str) -> AgentAccount {
    post(t, u, "/api/v1/accounts", j!({"kind": kind, "label": label, "authKind": "api_key", "secret": key})).await
}

fn home(t: &TestApp, account: i64) -> PathBuf {
    t.state.config.accounts_dir().join(account.to_string()).join("home")
}

/// Warten, bis der Thread nicht mehr arbeitet.
async fn settle(t: &TestApp, u: &TestUser, id: i64) -> ThreadDetail {
    for _ in 0..300 {
        let d: ThreadDetail = get(t, u, &format!("/api/v1/threads/{id}")).await;
        if !d.thread.status.is_active() && d.runs.iter().all(|r| r.status != "running" && r.status != "queued") {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("Thread {id} wurde nicht fertig");
}

async fn messages(t: &TestApp, u: &TestUser, id: i64) -> Vec<Message> {
    get(t, u, &format!("/api/v1/threads/{id}/messages")).await
}

#[rocket::async_test]
async fn agents_list_and_accounts() {
    let t = setup().await;
    let anna = t.user("anna", true).await;
    let a1 = account(&t, &anna, "claude", "Claude Max privat", "sk-ant-api03-geheim-1234").await;
    let a2 = account(&t, &anna, "claude", "Claude Team", "sk-ant-api03-anders-9876").await;
    assert!(a1.is_default && !a2.is_default, "erstes Konto ist Standard");
    assert_eq!(a1.secret_hint, "sk-…1234");
    assert_eq!(a1.status, "ok");
    assert_eq!(a1.secret_env, "ANTHROPIC_API_KEY");
    assert!(home(&t, a1.id).join(".claude").is_dir(), "eigenes Home angelegt");

    let list: Vec<AgentInfo> = get(&t, &anna, "/api/v1/agents").await;
    assert_eq!(list.len(), 6);
    let claude = list.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
    assert!(claude.installed);
    assert_eq!(claude.version, "9.9.9");
    assert_eq!(claude.accounts.len(), 2);
    assert!(!list.iter().find(|a| a.kind == AgentKind::Gemini).unwrap().installed);
    // Geheimnisse werden nie ausgeliefert
    let raw = t.client.get("/api/v1/accounts").private_cookie(anna.cookie.clone()).dispatch().await.into_string().await.unwrap();
    assert!(!raw.contains("geheim-1234"));

    // Standard wechseln, löschen räumt das Home weg
    let a2: AgentAccount = {
        let res = t
            .client
            .patch(format!("/api/v1/accounts/{}", a2.id))
            .private_cookie(anna.cookie.clone())
            .header(json())
            .body(j!({"isDefault": true, "defaultModel": "opus"}).to_string())
            .dispatch()
            .await;
        res.into_json().await.unwrap()
    };
    assert!(a2.is_default && a2.default_model == "opus");
    let res = t.client.delete(format!("/api/v1/accounts/{}", a1.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NoContent);
    assert!(!home(&t, a1.id).exists());
}

#[rocket::async_test]
async fn claude_run_messages_and_resume() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let acc = account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Ändere die main.rs bitte", "agent": "claude", "model": "opus", "effort": "high"})).await;
    assert_eq!(s.title, "Ändere die main.rs bitte");
    assert_eq!(s.account_id, Some(acc.id));
    let d = settle(&t, &anna, s.id).await;
    assert_eq!(d.thread.status, ThreadStatus::Done);
    assert!(d.thread.unread);
    let run = &d.runs[0];
    assert_eq!(run.status, "done");
    assert_eq!((run.input_tokens, run.output_tokens), (120, 30));
    assert!((run.cost_usd - 0.25).abs() < 1e-9);
    assert!(d.session_id.starts_with("sess-"));

    let msgs = messages(&t, &anna, s.id).await;
    let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, vec!["user", "assistant", "tool", "system", "assistant"], "{msgs:#?}");
    assert_eq!(msgs[1].content, "Hallo Welt");
    let tool: ToolCall = serde_json::from_value(msgs[2].data.clone()).unwrap();
    assert_eq!((tool.kind.as_str(), tool.summary.as_str(), tool.status.as_str()), ("edit", "Edit src/main.rs", "ok"));
    assert_eq!(tool.diffs[0].before, "alt");
    assert_eq!(tool.output, "Datei geändert");
    assert_eq!(msgs[3].content, "Warnung: das ist kein JSON");
    assert!(msgs[4].html.contains("<strong>main.rs</strong>"));

    // Aufruf: nicht-interaktiv, JSON, ohne Rückfragen, Modell, Stufe, MCP, Systemhinweis
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    for a in ["-p", "stream-json", "bypassPermissions", "opus", "--effort", "high", "--mcp-config", "--append-system-prompt"] {
        assert!(args.lines().any(|l| l == a), "{a} fehlt in {args}");
    }
    // Fertige Änderungen ausliefern (Version, Commit, Push, Release, nrzStore), ohne dass man es sagt
    assert!(args.contains("Ship finished code changes without being asked") && args.contains("nrzStore"), "{args}");
    let mcp: Value = serde_json::from_str(&std::fs::read_to_string(home(&t, acc.id).join("last-mcp.json")).unwrap()).unwrap();
    assert!(mcp["mcpServers"]["nrzx"]["url"].as_str().unwrap().starts_with("http://127.0.0.1:"));
    assert!(mcp["mcpServers"]["nrzx"]["headers"]["Authorization"].as_str().unwrap().starts_with("Bearer "));
    assert!(mcp["mcpServers"].get("nrzgit").is_none(), "ohne nrzGit-Verbindung kein nrzgit");
    let prompt = std::fs::read_to_string(home(&t, acc.id).join("last-prompt")).unwrap();
    assert_eq!(prompt, "Ändere die main.rs bitte");

    // Mit nrzGit bekommt der Agent dessen MCP-Server; eine alte luGit-Verbindung zählt nicht mehr
    for (kind, url, token) in [("nrzgit", "http://git.test:8390", "nrzg_test"), ("lugit", "http://lugit.test:8430", "lgt_test")] {
        sqlx::query(
            "INSERT INTO nrzgit_connections (user_id, kind, url, token, username, verified_at, has_mcp, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'anna', 1, 1, 1, 1)",
        )
        .bind(anna.id)
        .bind(kind)
        .bind(url)
        .bind(token)
        .execute(t.db())
        .await
        .unwrap();
    }
    let with: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Mit nrzGit", "agent": "claude"})).await;
    settle(&t, &anna, with.id).await;
    let mcp: Value = serde_json::from_str(&std::fs::read_to_string(home(&t, acc.id).join("last-mcp.json")).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["nrzgit"]["url"], "http://git.test:8390/mcp");
    assert_eq!(mcp["mcpServers"]["nrzgit"]["headers"]["Authorization"], "Bearer nrzg_test");
    assert!(mcp["mcpServers"].get("lugit").is_none() && mcp["mcpServers"].get("github").is_none(), "{mcp}");
    sqlx::query("DELETE FROM nrzgit_connections").execute(t.db()).await.unwrap();

    // Gelesen markieren: fertig → ruhend
    let s2: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/read", s.id), j!({})).await;
    assert_eq!(s2.status, ThreadStatus::Idle);
    assert!(!s2.unread);

    // Fortsetzen mit der Sitzungs-ID des CLIs
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/messages", s.id), j!({"text": "Und noch ein Test"})).await;
    let d2 = settle(&t, &anna, s.id).await;
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    let i = lines.iter().position(|l| *l == "--resume").expect("--resume");
    assert_eq!(lines[i + 1], d.session_id);
    assert_eq!(d2.session_id, d.session_id);
    assert_eq!(d2.runs.len(), 2);

    // Fehler des CLIs → Fehlernachricht, Status error
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/messages", s.id), j!({"text": "mach es kaputt"})).await;
    let d3 = settle(&t, &anna, s.id).await;
    assert_eq!(d3.thread.status, ThreadStatus::Error);
    let msgs = messages(&t, &anna, s.id).await;
    assert!(msgs.iter().any(|m| m.role == "error" && m.content == "Etwas ging schief"));

    // Kontext zurücksetzen: nächster Lauf ohne --resume, mit Verlauf im Prompt
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/clear", s.id), j!({})).await;
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/messages", s.id), j!({"text": "Neu anfangen"})).await;
    settle(&t, &anna, s.id).await;
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    assert!(!args.lines().any(|l| l == "--resume"));
    let prompt = std::fs::read_to_string(home(&t, acc.id).join("last-prompt")).unwrap();
    assert_eq!(prompt, "Neu anfangen", "nach dem Zurücksetzen kein alter Verlauf");
}

#[rocket::async_test]
async fn codex_run_and_resume() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let acc = account(&t, &anna, "codex", "ChatGPT", "sk-proj-abcdefghijkl").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Liste die Dateien", "agent": "codex", "effort": "max"})).await;
    let d = settle(&t, &anna, s.id).await;
    assert_eq!(d.thread.status, ThreadStatus::Done);
    assert!(d.session_id.starts_with("codex-"));
    assert_eq!((d.runs[0].input_tokens, d.runs[0].output_tokens), (50, 10));
    let msgs = messages(&t, &anna, s.id).await;
    let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, vec!["user", "tool", "tool", "assistant"], "{msgs:#?}");
    let think: ToolCall = serde_json::from_value(msgs[1].data.clone()).unwrap();
    assert_eq!(think.kind, "think");
    let cmd: ToolCall = serde_json::from_value(msgs[2].data.clone()).unwrap();
    assert_eq!((cmd.kind.as_str(), cmd.target.as_str(), cmd.status.as_str()), ("command", "ls", "ok"));
    assert_eq!(msgs[3].content, "Codex fertig");
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    for a in ["exec", "--json", "--dangerously-bypass-approvals-and-sandbox", "model_reasoning_effort=\"xhigh\"", "-"] {
        assert!(args.lines().any(|l| l == a), "{a} fehlt in {args}");
    }
    assert!(args.contains("mcp_servers.nrzx.bearer_token_env_var=\"NRZX_MCP_TOKEN\""));
    let env = std::fs::read_to_string(home(&t, acc.id).join("last-env")).unwrap();
    assert!(env.contains("OPENAI_API_KEY=sk-proj-abcdefghijkl"));
    assert!(env.contains(&format!("CODEX_HOME={}", home(&t, acc.id).join(".codex").display())));
    assert!(env.lines().any(|l| l.starts_with("NRZX_MCP_TOKEN=")));

    // Fortsetzen: codex exec resume <id> …
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/messages", s.id), j!({"text": "weiter"})).await;
    settle(&t, &anna, s.id).await;
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert_eq!(&lines[..3], &["exec", "resume", d.session_id.as_str()]);
}

#[rocket::async_test]
async fn stop_and_account_isolation() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let privat = account(&t, &anna, "claude", "Claude privat", "sk-ant-privat-1111").await;
    let team = account(&t, &anna, "claude", "Claude Team", "sk-ant-team-2222").await;

    // Zwei Threads, je mit einem anderen Konto, gleichzeitig
    let a: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "arbeite langsam", "agent": "claude", "accountId": privat.id})).await;
    let b: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "schnell", "agent": "claude", "accountId": team.id})).await;
    assert_eq!(a.status, ThreadStatus::Working);
    settle(&t, &anna, b.id).await;

    // Läuft noch: zweite Nachricht kommt in die Warteschlange
    let q: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/messages", a.id), j!({"text": "noch was"})).await;
    assert_eq!(q.status, ThreadStatus::Working);
    assert_eq!(q.queued.len(), 1);
    assert_eq!(q.queued[0].text, "noch was");

    // Stoppen beendet den Prozess
    let started = std::time::Instant::now();
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/stop", a.id), j!({})).await;
    let d = settle(&t, &anna, a.id).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(d.thread.status, ThreadStatus::Idle);
    assert_eq!(d.runs[0].status, "stopped");
    assert!(messages(&t, &anna, a.id).await.iter().any(|m| m.role == "system" && m.content == "Gestoppt"));
    // Nach einem Stopp bleibt die Warteschlange stehen; verwerfen
    assert_eq!(d.thread.queued.len(), 1, "nach Stopp nicht gesendet");
    let qid = d.thread.queued[0].id.clone();
    let res = t.client.delete(format!("/api/v1/threads/{}/queue/{qid}", a.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    assert!(settle(&t, &anna, a.id).await.thread.queued.is_empty());

    // Jedes Konto hat sein eigenes HOME und seinen eigenen Schlüssel
    let env_a = std::fs::read_to_string(home(&t, privat.id).join("last-env")).unwrap();
    let env_b = std::fs::read_to_string(home(&t, team.id).join("last-env")).unwrap();
    assert!(env_a.lines().any(|l| l == format!("HOME={}", home(&t, privat.id).display())));
    assert!(env_b.lines().any(|l| l == format!("HOME={}", home(&t, team.id).display())));
    assert!(env_a.contains("ANTHROPIC_API_KEY=sk-ant-privat-1111") && !env_a.contains("team-2222"));
    assert!(env_b.contains("ANTHROPIC_API_KEY=sk-ant-team-2222") && !env_b.contains("privat-1111"));
    assert!(env_a.lines().any(|l| l == format!("CLAUDE_CONFIG_DIR={}", home(&t, privat.id).join(".claude").display())));
    assert!(env_a.lines().any(|l| l.starts_with("XDG_CONFIG_HOME=") && l.contains(&privat.id.to_string())));
    // Werkzeuge von nrzX zuerst im PATH
    let path = env_a.lines().find(|l| l.starts_with("PATH=")).unwrap();
    assert!(path.starts_with(&format!("PATH={}", t.state.config.tools_dir().join("bin").display())));
    // macOS: `security`-Ersatz nach den Werkzeugen, vor dem System (kein Dialog „Schlüsselbund
    // nicht gefunden“ im Konto-Home)
    if cfg!(target_os = "macos") {
        let shims = t.state.config.data_dir.join("shims");
        assert!(path.contains(&format!(":{}:", shims.display())), "{path}");
        assert!(shims.join("security").is_file());
        assert!(env_a.lines().any(|l| l == "GEMINI_FORCE_FILE_STORAGE=true"));
    }

    // Konto wechseln: Sitzung verworfen, Systemnachricht
    let s: ThreadSummary = {
        let res = t
            .client
            .patch(format!("/api/v1/threads/{}", b.id))
            .private_cookie(anna.cookie.clone())
            .header(json())
            .body(j!({"accountId": privat.id}).to_string())
            .dispatch()
            .await;
        res.into_json().await.unwrap()
    };
    assert_eq!(s.account_id, Some(privat.id));
    let d: ThreadDetail = get(&t, &anna, &format!("/api/v1/threads/{}", b.id)).await;
    assert!(d.session_id.is_empty());
}

#[rocket::async_test]
async fn mcp_server_and_subagents() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let claude = account(&t, &anna, "claude", "Claude", "sk-ant-api03-xxxxxxxx").await;
    account(&t, &anna, "codex", "Codex", "sk-proj-yyyyyyyyyyyy").await;
    let parent: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Plane und arbeite langsam", "agent": "claude"})).await;

    // Token aus der MCP-Konfiguration, die das CLI bekommen hat
    let cfg = home(&t, claude.id).join("last-mcp.json");
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
            let status = res.status();
            (status, res.into_json::<Value>().await.unwrap_or(Value::Null))
        }
    };

    // Ohne Token: 401
    let res = t.client.post("/mcp").header(json()).body(j!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string()).dispatch().await;
    assert_eq!(res.status(), Status::Unauthorized);

    let (st, init) = rpc(j!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}})).await;
    assert_eq!(st, Status::Ok);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "nrzx");
    let res = t
        .client
        .post("/mcp")
        .header(Header::new("Authorization", auth.clone()))
        .header(json())
        .body(j!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Accepted);
    let (_, tools) = rpc(j!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec!["spawn_subagents", "wait_for_subagents", "subagent_status", "stop_subagent", "open_preview", "notify_user", "ask_user"]
    );

    // Zwei Subagenten mit Codex
    let (_, spawned) = rpc(j!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "spawn_subagents", "arguments": {"tasks": [
        {"title": "Share link API", "prompt": "Baue die API", "agent": "codex"},
        {"title": "Share link tests", "prompt": "Schreibe Tests", "agent": "codex"}
    ]}}})).await;
    assert_eq!(spawned["result"]["isError"], false, "{spawned}");
    let kids = spawned["result"]["structuredContent"]["subagents"].as_array().unwrap().clone();
    assert_eq!(kids.len(), 2);
    let (_, waited) = rpc(j!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "wait_for_subagents", "arguments": {"timeoutSeconds": 20}}})).await;
    let r = &waited["result"]["structuredContent"];
    assert_eq!(r["allDone"], true, "{waited}");
    for s in r["subagents"].as_array().unwrap() {
        assert_eq!(s["status"], "done");
        assert_eq!(s["lastAnswer"], "Codex fertig");
    }
    // Elternthread arbeitet weiter, Kinder hängen am Eltern
    let d: ThreadDetail = get(&t, &anna, &format!("/api/v1/threads/{}", parent.id)).await;
    assert_eq!(d.thread.status, ThreadStatus::Working);
    assert_eq!(d.children.len(), 2);
    assert!(d.children.iter().all(|c| c.parent_id == Some(parent.id) && c.agent == AgentKind::Codex));
    let list: Vec<ThreadSummary> = get(&t, &anna, &format!("/api/v1/threads?parent={}", parent.id)).await;
    assert_eq!(list.len(), 2);
    let msgs = messages(&t, &anna, parent.id).await;
    let card = msgs.iter().find(|m| m.role == "subagents").expect("Subagenten-Karte");
    assert_eq!(card.data["children"].as_array().unwrap().len(), 2);

    // Vorschau, Hinweis und Status
    let (_, prev) = rpc(j!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "open_preview", "arguments": {"port": 5173}}})).await;
    assert_eq!(prev["result"]["structuredContent"]["path"], "/");
    let (_, st) = rpc(j!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "subagent_status", "arguments": {}}})).await;
    assert_eq!(st["result"]["structuredContent"]["subagents"].as_array().unwrap().len(), 2);
    let (_, bad) = rpc(j!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "stop_subagent", "arguments": {"id": parent.id}}})).await;
    assert_eq!(bad["result"]["isError"], true);
    let (_, unknown) = rpc(j!({"jsonrpc": "2.0", "id": 8, "method": "nope"})).await;
    assert_eq!(unknown["error"]["code"], -32601);

    // Elternthread stoppen: das Token ist danach ungültig
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/stop", parent.id), j!({})).await;
    settle(&t, &anna, parent.id).await;
    let (st, _) = rpc(j!({"jsonrpc": "2.0", "id": 9, "method": "tools/list"})).await;
    assert_eq!(st, Status::Unauthorized);

    // Der Benutzer startet Subagenten selbst
    let kids: Vec<ThreadSummary> =
        post(&t, &anna, &format!("/api/v1/threads/{}/subagents", parent.id), j!({"tasks": [{"title": "Docs", "prompt": "Schreibe Doku", "agent": "codex"}]})).await;
    assert_eq!(kids.len(), 1);
    let d = settle(&t, &anna, kids[0].id).await;
    // Vom Benutzer gestartet: bleibt offen (wird nicht automatisch abgelegt)
    assert_ne!(d.thread.status, ThreadStatus::Settled);
}

#[rocket::async_test]
async fn terminal_runs_commands() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let acc = account(&t, &anna, "claude", "Claude", "sk-ant-api03-zzzzzzzz").await;
    let info: TerminalInfo = post(&t, &anna, "/api/v1/terminals", j!({"accountId": acc.id, "command": "read x; echo \"got-$x in $HOME\""})).await;
    assert!(info.running);
    let res = t
        .client
        .post(format!("/api/v1/terminals/{}/input", info.id))
        .private_cookie(anna.cookie.clone())
        .header(json())
        .body(j!({"data": "abc\r"}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::NoContent);
    let res = t
        .client
        .post(format!("/api/v1/terminals/{}/resize", info.id))
        .private_cookie(anna.cookie.clone())
        .header(json())
        .body(j!({"cols": 120, "rows": 40}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::NoContent);
    let body = t.client.get(format!("/api/v1/terminals/{}/stream", info.id)).private_cookie(anna.cookie.clone()).dispatch().await.into_string().await.unwrap();
    let mut out = Vec::new();
    let mut exit = None;
    let mut event = "";
    for line in body.lines() {
        if let Some(e) = line.strip_prefix("event:") {
            event = e.trim();
        } else if let Some(d) = line.strip_prefix("data:") {
            if event == "exit" {
                exit = Some(d.trim().to_string());
            } else {
                out.extend(base64::engine::general_purpose::STANDARD.decode(d.trim()).unwrap());
            }
        } else if line.is_empty() {
            event = "";
        }
    }
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains(&format!("got-abc in {}", home(&t, acc.id).display())), "{text}");
    assert_eq!(exit.as_deref(), Some("0"));

    // Fremde sehen das Terminal nicht
    let bob = t.user("bob", false).await;
    let res = t.client.get(format!("/api/v1/terminals/{}", info.id)).private_cookie(bob.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);
    let res = t.client.delete(format!("/api/v1/terminals/{}", info.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NoContent);
}

#[rocket::async_test]
async fn foreign_threads_are_invisible() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let bob = t.user("bob", false).await;
    let acc = account(&t, &anna, "claude", "Claude", "sk-ant-api03-aaaaaaaa").await;
    account(&t, &bob, "claude", "Claude Bob", "sk-ant-api03-bbbbbbbb").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Hallo", "agent": "claude"})).await;
    settle(&t, &anna, s.id).await;

    for path in [format!("/api/v1/threads/{}", s.id), format!("/api/v1/threads/{}/messages", s.id), format!("/api/v1/threads/{}/files", s.id)] {
        let res = t.client.get(&path).private_cookie(bob.cookie.clone()).dispatch().await;
        assert_eq!(res.status(), Status::NotFound, "{path}");
    }
    for path in [format!("/api/v1/threads/{}/messages", s.id), format!("/api/v1/threads/{}/stop", s.id), format!("/api/v1/threads/{}/subagents", s.id)] {
        let res = t.client.post(&path).private_cookie(bob.cookie.clone()).header(json()).body(j!({"text": "x", "tasks": []}).to_string()).dispatch().await;
        assert_eq!(res.status(), Status::NotFound, "{path}");
    }
    let res = t.client.delete(format!("/api/v1/threads/{}", s.id)).private_cookie(bob.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);
    let list: Vec<ThreadSummary> = get(&t, &bob, "/api/v1/threads").await;
    assert!(list.is_empty());
    // Fremdes Konto nicht benutzbar
    let res = t
        .client
        .post("/api/v1/threads")
        .private_cookie(bob.cookie.clone())
        .header(json())
        .body(j!({"prompt": "x", "agent": "claude", "accountId": acc.id}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::NotFound);
    let res = t.client.post(format!("/api/v1/accounts/{}/check", acc.id)).private_cookie(bob.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);
    // Ohne Anmeldung nichts
    let res = t.client.get("/api/v1/threads").dispatch().await;
    assert_eq!(res.status(), Status::Unauthorized);

    // Löschen durch den Besitzer
    let res = t.client.delete(format!("/api/v1/threads/{}", s.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NoContent);
    let res = t.client.get(format!("/api/v1/threads/{}", s.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);
}

#[rocket::async_test]
async fn files_skills_and_attachments() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    account(&t, &anna, "claude", "Claude", "sk-ant-api03-cccccccc").await;
    let (project, repo) = t.project(anna.id, "demo").await;
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    let files: Vec<nrzx_shared::agents::FileEntry> = get(&t, &anna, &format!("/api/v1/agents/files?project={project}&q=main")).await;
    assert_eq!(files[0].path, "src/main.rs");

    let skill: nrzx_shared::agents::Skill = post(&t, &anna, "/api/v1/agents/skills", j!({"name": "review", "description": "Code prüfen", "prompt": "Prüfe den Code gründlich."})).await;
    assert_eq!(skill.name, "review");
    let res = t
        .client
        .post("/api/v1/agents/skills")
        .private_cookie(anna.cookie.clone())
        .header(json())
        .body(j!({"name": "review", "prompt": "doppelt"}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Conflict);

    // Bild anhängen und mitschicken
    let boundary = "XnrzX";
    let png: &[u8] = b"\x89PNG\r\n\x1a\nrest";
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"bild.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
    body.extend_from_slice(png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let res = t
        .client
        .post("/api/v1/attachments")
        .private_cookie(anna.cookie.clone())
        .header(rocket::http::ContentType::new("multipart", "form-data").with_params(("boundary", boundary)))
        .body(body)
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    let att: nrzx_shared::Attachment = res.into_json().await.unwrap();
    assert_eq!(att.content_type, "image/png");
    let res = t.client.get(&att.url).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"projectId": project, "prompt": "Was zeigt das Bild?", "agent": "claude", "attachments": [att.uuid]})).await;
    settle(&t, &anna, s.id).await;
    let msgs = messages(&t, &anna, s.id).await;
    assert_eq!(msgs[0].data["attachments"][0]["name"], "bild.png");
    // Claude bekommt das Bild als Bild-Eingabe (stream-json über stdin)
    let acc: Vec<AgentAccount> = get(&t, &anna, "/api/v1/accounts?kind=claude").await;
    let prompt = std::fs::read_to_string(home(&t, acc[0].id).join("last-prompt")).unwrap();
    let line: Value = serde_json::from_str(prompt.trim()).unwrap();
    assert_eq!(line["message"]["content"][1]["type"], "image");
    let bob = t.user("bob", false).await;
    let res = t.client.get(&att.url).private_cookie(bob.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);
}

#[rocket::async_test]
async fn freestyle_threads() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let acc = account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Baue etwas", "agent": "claude", "freestyle": true})).await;
    assert!(s.freestyle);
    assert!(s.resume_auto, "Weitermachen nach Rate-Limit standardmässig an");
    settle(&t, &anna, s.id).await;
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    assert!(args.contains("FREESTYLE"), "Hinweis im Systemprompt: {args}");
    assert!(args.lines().any(|l| l == "AskUserQuestion"), "eingebaute Rückfrage aus");
}

#[rocket::async_test]
async fn btw_side_chat() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let acc = account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Baue etwas", "agent": "claude"})).await;
    settle(&t, &anna, s.id).await;
    let e: nrzx_shared::agents::BtwEntry = post(&t, &anna, &format!("/api/v1/threads/{}/btw", s.id), j!({"question": "Wie weit bist du?"})).await;
    assert_eq!(e.status, "done", "{e:?}");
    assert_eq!(e.answer, "Fertig: **main.rs** geändert.", "letzter Textblock ist die Antwort");
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    assert!(args.lines().any(|l| l == "haiku"), "günstiges Modell: {args}");
    assert!(!args.lines().any(|l| l == "bypassPermissions"), "nur lesen: {args}");
    let prompt = std::fs::read_to_string(home(&t, acc.id).join("last-prompt")).unwrap();
    assert!(prompt.contains("Side question: Wie weit bist du?") && prompt.contains("USER: Baue etwas"), "{prompt}");
    // Kein Eintrag im Verlauf des Threads
    let msgs: Vec<nrzx_shared::Message> = get(&t, &anna, &format!("/api/v1/threads/{}/messages", s.id)).await;
    assert!(!msgs.iter().any(|m| m.content.contains("Wie weit bist du")));
}

#[rocket::async_test]
async fn read_only_threads() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let acc = account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Wie funktioniert der Login?", "agent": "claude", "readOnly": true})).await;
    assert!(s.read_only);
    settle(&t, &anna, s.id).await;
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    assert!(!args.lines().any(|l| l == "bypassPermissions"), "kein Freibrief im Nur-Lesen-Modus: {args}");
    for a in ["--allowedTools", "Read", "Grep", "--disallowedTools", "Edit", "Write", "--strict-mcp-config"] {
        assert!(args.lines().any(|l| l == a), "{a} fehlt in {args}");
    }
    assert!(args.contains("READ-ONLY"), "Hinweis im Systemprompt");
    let mcp: Value = serde_json::from_str(&std::fs::read_to_string(home(&t, acc.id).join("last-mcp.json")).unwrap()).unwrap();
    assert!(mcp["mcpServers"].as_object().unwrap().is_empty(), "keine MCP-Server: {mcp}");
    // Subagenten gibt es im Nur-Lesen-Modus nicht
    let res = t
        .client
        .post(format!("/api/v1/threads/{}/subagents", s.id))
        .private_cookie(anna.cookie.clone())
        .header(json())
        .body(j!({"tasks": [{"title": "x", "prompt": "y"}]}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::BadRequest);
}

#[rocket::async_test]
async fn retract_message_returns_text_and_attachments() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Erste Frage", "agent": "claude"})).await;
    settle(&t, &anna, s.id).await;
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/messages", s.id), j!({"text": "Zweite Frage"})).await;
    settle(&t, &anna, s.id).await;
    let msgs = messages(&t, &anna, s.id).await;
    let second = msgs.iter().filter(|m| m.role == "user").nth(1).unwrap().id;
    let r: nrzx_shared::agents::RetractedMessage = post(&t, &anna, &format!("/api/v1/threads/{}/messages/{second}/retract", s.id), j!({})).await;
    assert_eq!(r.text, "Zweite Frage");
    let after = messages(&t, &anna, s.id).await;
    assert!(after.iter().all(|m| m.id < second), "Nachricht und alles danach entfernt");
    assert_eq!(after.iter().filter(|m| m.role == "user").count(), 1);
    let d: ThreadDetail = get(&t, &anna, &format!("/api/v1/threads/{}", s.id)).await;
    assert!(d.session_id.is_empty(), "Sitzung beginnt neu");
    assert_eq!(d.runs.len(), 2, "Läufe bleiben für die Nutzung");
    // Nur Benutzernachrichten
    let res = t.client.post(format!("/api/v1/threads/{}/messages/{}/retract", s.id, after[1].id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::BadRequest);
}

#[rocket::async_test]
async fn rate_limit_schedules_resume_and_usage_counts() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let acc = account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "ratelimit bitte", "agent": "claude"})).await;
    let d = settle(&t, &anna, s.id).await;
    let at = d.thread.resume_at.expect("Weitermachen geplant");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!((at - (now + 3630)).abs() < 30, "Zeitpunkt aus der Meldung: {at} vs {now}");
    // Abbrechen
    let res = t.client.delete(format!("/api/v1/threads/{}/resume", s.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    let s2: ThreadSummary = res.into_json().await.unwrap();
    assert_eq!(s2.resume_at, None);
    // Jetzt weitermachen: automatische Nachricht
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/resume", s.id), j!({})).await;
    settle(&t, &anna, s.id).await;
    let msgs = messages(&t, &anna, s.id).await;
    let last_user = msgs.iter().rfind(|m| m.role == "user").unwrap();
    assert_eq!(last_user.data["autoResume"], true);

    let u: nrzx_shared::agents::UsageOverview = get(&t, &anna, "/api/v1/usage").await;
    let a = u.accounts.iter().find(|a| a.account_id == acc.id).unwrap();
    let today = a.windows.iter().find(|w| w.key == "today").unwrap();
    assert_eq!(today.runs, 2);
    assert!(a.limits.is_empty(), "API-Schlüssel: keine Abo-Limits");
}

#[rocket::async_test]
async fn prompt_notes() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let bert = t.user("bert", false).await;
    let n: nrzx_shared::agents::PromptNote = post(&t, &anna, "/api/v1/notes", j!({"text": "Idee: Dark Mode"})).await;
    assert_eq!(n.text, "Idee: Dark Mode");
    let res = t
        .client
        .patch(format!("/api/v1/notes/{}", n.id))
        .private_cookie(anna.cookie.clone())
        .header(json())
        .body(j!({"text": "Idee: Dark Mode für alle Seiten"}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    let list: Vec<nrzx_shared::agents::PromptNote> = get(&t, &anna, "/api/v1/notes").await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].text, "Idee: Dark Mode für alle Seiten");
    let other: Vec<nrzx_shared::agents::PromptNote> = get(&t, &bert, "/api/v1/notes").await;
    assert!(other.is_empty());
    let res = t.client.delete(format!("/api/v1/notes/{}", n.id)).private_cookie(bert.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);
    let res = t.client.delete(format!("/api/v1/notes/{}", n.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NoContent);
}

/// Bild hochladen (`POST /attachments`) wie der Composer.
async fn upload_png(t: &TestApp, user: &TestUser, name: &str) -> nrzx_shared::Attachment {
    let boundary = "XnrzX";
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
    body.extend_from_slice(b"\x89PNG\r\n\x1a\nrest");
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let res = t
        .client
        .post("/api/v1/attachments")
        .private_cookie(user.cookie.clone())
        .header(rocket::http::ContentType::new("multipart", "form-data").with_params(("boundary", boundary)))
        .body(body)
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    res.into_json().await.unwrap()
}

#[rocket::async_test]
async fn prompt_notes_with_images() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let bert = t.user("bert", false).await;
    let a = upload_png(&t, &anna, "eins.png").await;
    let b = upload_png(&t, &anna, "zwei.png").await;
    let fremd = upload_png(&t, &bert, "fremd.png").await;

    // Notiz nur mit Bildern (ohne Text) anlegen
    let n: nrzx_shared::agents::PromptNote = post(&t, &anna, "/api/v1/notes", j!({"text": "", "attachments": [a.uuid, b.uuid]})).await;
    assert_eq!(n.attachments.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["eins.png", "zwei.png"]);
    assert_eq!(n.attachments[0].url, format!("/attachments/{}", a.uuid));

    // Fremde Anhänge: abgelehnt
    let res = t.client.post("/api/v1/notes").private_cookie(anna.cookie.clone()).header(json()).body(j!({"attachments": [fremd.uuid]}).to_string()).dispatch().await;
    assert_eq!(res.status(), Status::BadRequest);

    // Nur den Text ändern: Bilder bleiben
    let patch = |body: serde_json::Value| {
        let c = anna.cookie.clone();
        let client = &t.client;
        let id = n.id;
        async move { client.patch(format!("/api/v1/notes/{id}")).private_cookie(c).header(json()).body(body.to_string()).dispatch().await }
    };
    let res = patch(j!({"text": "Siehe Bilder"})).await;
    let n2: nrzx_shared::agents::PromptNote = res.into_json().await.unwrap();
    assert_eq!(n2.text, "Siehe Bilder");
    assert_eq!(n2.attachments.len(), 2);

    // Ein Bild entfernen
    let res = patch(j!({"attachments": [b.uuid]})).await;
    let n3: nrzx_shared::agents::PromptNote = res.into_json().await.unwrap();
    assert_eq!(n3.attachments.len(), 1);
    assert_eq!(n3.attachments[0].uuid, b.uuid);
    let res = patch(j!({"attachments": [fremd.uuid]})).await;
    assert_eq!(res.status(), Status::BadRequest);

    let list: Vec<nrzx_shared::agents::PromptNote> = get(&t, &anna, "/api/v1/notes").await;
    assert_eq!(list[0].attachments.len(), 1);
    assert_eq!(list[0].attachments[0].name, "zwei.png");
}

#[rocket::async_test]
async fn sidebar_hide_and_order() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let bob = t.user("bob", false).await;
    account(&t, &anna, "claude", "Claude", "sk-ant-api03-aaaaaaaa").await;
    let mut ids = Vec::new();
    for p in ["eins", "zwei", "drei"] {
        let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": p, "agent": "claude"})).await;
        settle(&t, &anna, s.id).await;
        ids.push(s.id);
    }
    let (a, b, c) = (ids[0], ids[1], ids[2]);

    // Ausblenden und wieder einblenden
    let res = t.client.patch(format!("/api/v1/threads/{b}")).private_cookie(anna.cookie.clone()).header(json()).body(j!({"hidden": true}).to_string()).dispatch().await;
    let s: ThreadSummary = res.into_json().await.unwrap();
    assert!(s.hidden);
    let res = t.client.patch(format!("/api/v1/threads/{b}")).private_cookie(anna.cookie.clone()).header(json()).body(j!({"hidden": false}).to_string()).dispatch().await;
    let s: ThreadSummary = res.into_json().await.unwrap();
    assert!(!s.hidden);

    // Reihenfolge festlegen: a oben, dann c, dann b
    let list: Vec<ThreadSummary> = post(&t, &anna, "/api/v1/threads/order", j!({"ids": [a, c, b]})).await;
    assert_eq!(list.len(), 3);
    assert!(list.iter().all(|x| x.sort_key.is_some()));
    let all: Vec<ThreadSummary> = get(&t, &anna, "/api/v1/threads").await;
    assert_eq!(all.iter().map(|x| x.id).collect::<Vec<_>>(), vec![a, c, b]);

    // Fremde Threads lassen sich nicht sortieren
    let res = t.client.post("/api/v1/threads/order").private_cookie(bob.cookie.clone()).header(json()).body(j!({"ids": [a]}).to_string()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);

    // Ohne Angabe: kein eigener Worktree
    let wt: bool = sqlx::query_scalar("SELECT use_worktree FROM threads WHERE id = ?").bind(a).fetch_one(&t.state.db).await.unwrap();
    assert!(!wt, "Standard ist der Hauptcheckout");
}

#[rocket::async_test]
async fn withdraw_queued_message_returns_text_and_attachments() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let bob = t.user("bob", false).await;
    account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "arbeite langsam", "agent": "claude"})).await;
    assert_eq!(s.status, ThreadStatus::Working);

    // Bild hochladen und mit der wartenden Nachricht schicken
    let boundary = "XnrzX";
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"bild.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
    body.extend_from_slice(b"\x89PNG\r\n\x1a\nrest");
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let res = t
        .client
        .post("/api/v1/attachments")
        .private_cookie(anna.cookie.clone())
        .header(rocket::http::ContentType::new("multipart", "form-data").with_params(("boundary", boundary)))
        .body(body)
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    let att: nrzx_shared::Attachment = res.into_json().await.unwrap();
    let q: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/messages", s.id), j!({"text": "noch was", "attachments": [att.uuid]})).await;
    assert_eq!(q.queued.len(), 1);
    let qid = q.queued[0].id.clone();
    let path = format!("/api/v1/threads/{}/queue/{qid}/withdraw", s.id);

    // Fremde Benutzer sehen den Thread nicht
    let res = t.client.post(&path).private_cookie(bob.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);

    // Zurückziehen: Text und Anhänge kommen zurück, die Warteschlange ist leer
    let r: nrzx_shared::agents::RetractedMessage = post(&t, &anna, &path, j!({})).await;
    assert_eq!(r.text, "noch was");
    assert_eq!(r.attachments.len(), 1);
    assert_eq!(r.attachments[0].uuid, att.uuid);
    assert_eq!(r.attachments[0].name, "bild.png");
    assert!(r.thread.queued.is_empty());

    // Schon weg: 404
    let res = t.client.post(&path).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);

    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/stop", s.id), j!({})).await;
    let d = settle(&t, &anna, s.id).await;
    assert!(d.thread.queued.is_empty());
    assert!(!messages(&t, &anna, s.id).await.iter().any(|m| m.content == "noch was"), "nicht gesendet");
}

/// „Jetzt“ an einer wartenden Nachricht unterbricht den Agenten sofort – nicht erst im nächsten
/// passenden Moment (der nachgeahmte Agent schweigt 30 s, die Schonfrist wären 20 s).
#[rocket::async_test]
async fn queued_message_now_interrupts_at_once() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "arbeite langsam", "agent": "claude"})).await;
    assert_eq!(s.status, ThreadStatus::Working);
    let q: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/messages", s.id), j!({"text": "sofort bitte"})).await;
    assert_eq!(q.queued.len(), 1);
    assert!(!q.queued[0].interrupt);
    let qid = q.queued[0].id.clone();

    let started = std::time::Instant::now();
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/queue/{qid}/now", s.id), j!({})).await;
    // Die Nachricht geht gleich als Zwischenruf raus
    let mut sent = false;
    for _ in 0..200 {
        if messages(&t, &anna, s.id).await.iter().any(|m| m.role == "user" && m.content == "sofort bitte") {
            sent = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(sent, "Zwischenruf wurde nicht gesendet");
    assert!(started.elapsed() < Duration::from_secs(10), "hat auf einen passenden Moment gewartet");
    let d = settle(&t, &anna, s.id).await;
    assert!(d.thread.queued.is_empty());
    assert_eq!(d.runs.iter().filter(|r| r.status == "stopped").count(), 1);
    let ms = messages(&t, &anna, s.id).await;
    assert!(!ms.iter().any(|m| m.role == "system" && m.content == "Gestoppt"), "kein „Gestoppt“ bei einem Zwischenruf");
}

async fn patch_thread(t: &TestApp, u: &TestUser, id: i64, body: Value) -> ThreadSummary {
    let res = t.client.patch(format!("/api/v1/threads/{id}")).private_cookie(u.cookie.clone()).header(json()).body(body.to_string()).dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    res.into_json().await.unwrap()
}

#[rocket::async_test]
async fn thread_mode_and_exact_stats() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let bert = t.user("bert", false).await;
    account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Baue etwas", "agent": "claude", "freestyle": true})).await;
    settle(&t, &anna, s.id).await;

    // Nur lesen nachträglich an: FreeStyle geht aus und lässt sich nicht wieder anschalten
    let s2 = patch_thread(&t, &anna, s.id, j!({"readOnly": true})).await;
    assert!(s2.read_only && !s2.freestyle);
    let s2 = patch_thread(&t, &anna, s.id, j!({"freestyle": true})).await;
    assert!(!s2.freestyle);
    let s2 = patch_thread(&t, &anna, s.id, j!({"readOnly": false, "freestyle": true})).await;
    assert!(!s2.read_only && s2.freestyle);

    // Genaue Zahlen: Cache-Tokens aus dem Ergebnis von Claude
    let st: nrzx_shared::workspace::Stats = get(&t, &anna, "/api/v1/me/stats").await;
    assert_eq!(st.total.runs, 1);
    assert_eq!((st.total.input_tokens, st.total.cache_read_tokens, st.total.output_tokens), (120, 20, 30));
    assert_eq!(st.total.fresh_input(), 100);
    assert_eq!(st.windows.iter().map(|w| w.key.as_str()).collect::<Vec<_>>(), vec!["today", "7d", "30d", "all"]);
    assert_eq!(st.models.len(), 1);
    assert!(st.user_messages >= 1 && st.tool_calls >= 1);
    let ts: nrzx_shared::workspace::ThreadStats = get(&t, &anna, &format!("/api/v1/threads/{}/stats", s.id)).await;
    assert_eq!((ts.own.runs, ts.own.cache_read_tokens), (1, 20));
    assert_eq!(ts.with_subagents.total_tokens(), 150);
    // Verlauf für die Menüleiste: 24 Stunden-Abschnitte, der Lauf liegt im letzten
    let u: nrzx_shared::workspace::UsageReport = get(&t, &anna, "/api/v1/me/usage?range=24h").await;
    assert_eq!((u.sessions, u.total.runs, u.bucket_seconds), (1, 1, 3600));
    assert_eq!(u.series.len(), 1);
    assert_eq!(u.series[0].points.len(), 24);
    assert_eq!(u.series[0].points[23].tokens, 150);
    assert!((u.agents[0].sums.cost_usd - 0.25).abs() < 1e-9);
    let u: nrzx_shared::workspace::UsageReport = get(&t, &anna, "/api/v1/me/usage?range=90d").await;
    assert_eq!(u.series[0].points.len(), 30);
    let res = t.client.get("/api/v1/me/usage?range=1y").private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::BadRequest);
    // Fremde Threads: 404
    let res = t.client.get(format!("/api/v1/threads/{}/stats", s.id)).private_cookie(bert.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);

    // Thread löschen: der Verbrauch bleibt in der Nutzung und Statistik erhalten
    let res = t.client.delete(format!("/api/v1/threads/{}", s.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert!(res.status().code < 300, "{}", res.status());
    let u: nrzx_shared::workspace::UsageReport = get(&t, &anna, "/api/v1/me/usage?range=24h").await;
    assert_eq!((u.sessions, u.total.runs, u.total.total_tokens()), (1, 1, 150));
    let st: nrzx_shared::workspace::Stats = get(&t, &anna, "/api/v1/me/stats").await;
    assert_eq!((st.total.runs, st.total.cache_read_tokens), (1, 20));
    let ov: nrzx_shared::agents::UsageOverview = get(&t, &anna, "/api/v1/usage").await;
    let w = ov.accounts[0].windows.iter().find(|w| w.key == "today").unwrap();
    assert_eq!((w.runs, w.input_tokens, w.output_tokens), (1, 120, 30));
    // Ein neuer Lauf (die ID kann wieder vergeben werden) überschreibt den alten Verbrauch nicht
    let s3: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Noch etwas", "agent": "claude"})).await;
    settle(&t, &anna, s3.id).await;
    let u: nrzx_shared::workspace::UsageReport = get(&t, &anna, "/api/v1/me/usage?range=24h").await;
    assert_eq!((u.sessions, u.total.runs, u.total.total_tokens()), (2, 2, 300));
    // Bert sieht nichts davon
    let u: nrzx_shared::workspace::UsageReport = get(&t, &bert, "/api/v1/me/usage?range=24h").await;
    assert_eq!(u.total.runs, 0);
}

#[rocket::async_test]
async fn ai_chain_switches_account_on_rate_limit() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let bob = t.user("bob", false).await;
    let claude = account(&t, &anna, "claude", "Abo 1", "sk-ant-api03-abcdefgh").await;
    let codex = account(&t, &anna, "codex", "Codex Abo", "sk-proj-yyyyyyyyyyyy").await;
    let foreign = account(&t, &bob, "claude", "Bobs Abo", "sk-ant-api03-zzzzzzzz").await;

    let put = |body: Value| {
        let client = &t.client;
        let cookie = anna.cookie.clone();
        async move {
            let res = client.put("/api/v1/chains").private_cookie(cookie).header(json()).body(body.to_string()).dispatch().await;
            (res.status(), res.into_string().await.unwrap_or_default())
        }
    };
    // Fremde Konten und doppelte Stufen werden abgelehnt
    let (st, _) = put(j!({"chains": [{"name": "X", "enabled": true, "steps": [{"accountId": claude.id}, {"accountId": foreign.id}]}]})).await;
    assert_eq!(st, Status::BadRequest);
    let (st, _) = put(j!({"chains": [{"name": "X", "enabled": true, "steps": [{"accountId": claude.id}, {"accountId": claude.id}]}]})).await;
    assert_eq!(st, Status::BadRequest);
    let (st, body) = put(j!({"chains": [{"id": "main", "name": "Haupt", "enabled": true, "steps": [{"accountId": claude.id}, {"accountId": codex.id}]}]})).await;
    assert_eq!(st, Status::Ok, "{body}");
    let saved: nrzx_shared::agents::ChainSettings = get(&t, &anna, "/api/v1/chains").await;
    assert_eq!(saved.chains.len(), 1);
    assert_eq!(saved.chains[0].steps.len(), 2);

    // Claude läuft ins Limit → Codex übernimmt sofort (mit Verlauf) und wird fertig
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "ratelimit bitte", "agent": "claude", "accountId": claude.id})).await;
    let mut done = None;
    for _ in 0..400 {
        let d: ThreadDetail = get(&t, &anna, &format!("/api/v1/threads/{}", s.id)).await;
        if d.thread.agent == AgentKind::Codex && !d.thread.status.is_active() && d.runs.iter().any(|r| r.agent == AgentKind::Codex && r.status == "done") {
            done = Some(d);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let d = done.expect("Codex hat übernommen");
    assert_eq!(d.thread.account_id, Some(codex.id));
    assert_eq!(d.thread.resume_at, None, "kein Warten auf das Zurücksetzen");
    let msgs = messages(&t, &anna, s.id).await;
    let switch = msgs.iter().find(|m| m.role == "system" && m.data["kind"] == "chain_switch").expect("Hinweis im Verlauf");
    assert_eq!(switch.content, "Haupt");
    assert_eq!(switch.data["detail"], "Codex · Codex Abo");
    assert!(msgs.iter().any(|m| m.role == "assistant" && m.content == "Codex fertig"));
    // Codex bekommt den bisherigen Verlauf (neue Sitzung)
    let prompt = std::fs::read_to_string(home(&t, codex.id).join("last-prompt")).unwrap();
    assert!(prompt.contains("ratelimit bitte"), "{prompt}");
    // Das Konto am Limit steht in der Liste
    let after: nrzx_shared::agents::ChainSettings = get(&t, &anna, "/api/v1/chains").await;
    assert!(after.blocked.iter().any(|b| b.account_id == claude.id), "{:?}", after.blocked);
}

#[rocket::async_test]
async fn edits_show_exact_lines_with_context() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    account(&t, &anna, "claude", "Abo", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "bearbeite bitte die Zahlen", "agent": "claude"})).await;
    settle(&t, &anna, s.id).await;
    let msgs = messages(&t, &anna, s.id).await;
    let tools: Vec<ToolCall> = msgs.iter().filter(|m| m.role == "tool").map(|m| serde_json::from_value(m.data.clone()).unwrap()).collect();
    // Bearbeiten: Abschnitt mit Zeilennummer und drei Zeilen Kontext aus der Datei
    let edit = tools.iter().find(|c| c.name == "Edit").expect("Edit");
    assert_eq!(edit.diffs.len(), 1, "{:?}", edit.diffs);
    let d = &edit.diffs[0];
    assert_eq!(d.path, "zahlen.txt");
    assert_eq!((d.line, d.old_line, d.adds, d.dels), (5, 5, 2, 1));
    assert_eq!((d.before.as_str(), d.after.as_str()), ("fünf", "FÜNF\nfünfeinhalb"));
    assert_eq!(d.context_before, vec!["zwei", "drei", "vier"]);
    assert_eq!(d.context_after, vec!["sechs", "sieben", "acht"]);
    // Neue Datei: alles hinzugefügt ab Zeile 1
    let w = tools.iter().find(|c| c.name == "Write").expect("Write");
    assert_eq!((w.diffs[0].line, w.diffs[0].adds, w.diffs[0].dels), (1, 2, 0), "{:?}", w.diffs);
}

#[rocket::async_test]
async fn repo_references_with_double_at() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let ben = t.user("ben", false).await;
    let acc = account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let (app, _) = t.project(anna.id, "Shop App").await;
    let (_, api_repo) = t.project(anna.id, "payment-api").await;
    let (_, foreign) = t.project(ben.id, "geheim").await;

    // Im Projekt „Shop App“: @@payment-api einbeziehen, das eigene Projekt und fremde nicht
    let s: ThreadSummary = post(
        &t,
        &anna,
        "/api/v1/threads",
        j!({"projectId": app, "prompt": "Binde @@payment-api an. Siehe auch @@shop-app und @@geheim, @@gibtsnicht.", "agent": "claude"}),
    )
    .await;
    settle(&t, &anna, s.id).await;
    let prompt = std::fs::read_to_string(home(&t, acc.id).join("last-prompt")).unwrap();
    assert!(prompt.starts_with("Binde @@payment-api an."), "{prompt}");
    assert!(prompt.contains("Referenced repositories (@@)"), "{prompt}");
    assert!(prompt.contains(&format!("- @@payment-api: {}, branch main", api_repo.display())), "{prompt}");
    assert!(!prompt.contains("@@Shop App:"), "eigenes Projekt nicht: {prompt}");
    assert!(!prompt.contains(&foreign.display().to_string()), "fremdes Projekt nicht: {prompt}");
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert!(lines.windows(2).any(|w| w[0] == "--add-dir" && w[1] == api_repo.to_string_lossy()), "{args}");

    // Ohne @@ kein Abschnitt
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/messages", s.id), j!({"text": "Danke"})).await;
    settle(&t, &anna, s.id).await;
    let prompt = std::fs::read_to_string(home(&t, acc.id).join("last-prompt")).unwrap();
    assert!(!prompt.contains("Referenced repositories"), "{prompt}");
}

/// Seitenleiste „wartet auf deine Antwort“ (gelb): nur, solange eine Rückfrage offen ist. Läuft sie
/// ab – der Agent entscheidet dann selbst und macht weiter –, ist es vorbei; eine Antwort oder
/// eine eigene Nachricht danach beenden es ebenso.
#[rocket::async_test]
async fn asking_only_while_question_open() {
    let t = TestApp::new().await;
    let anna = t.user("anna", false).await;
    let id = t.thread(anna.id, None, "Rückfrage").await;
    let insert = |role: &'static str, data: &'static str| {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO messages (thread_id, run_id, role, content, data, html, created_at, updated_at) VALUES (?, NULL, ?, 'Welche Farbe?', ?, '', 0, 0) RETURNING id",
        )
        .bind(id)
        .bind(role)
        .bind(data)
        .fetch_one(t.db())
    };
    let asking = || async {
        let d: ThreadDetail = get(&t, &anna, &format!("/api/v1/threads/{id}")).await;
        d.thread.asking
    };

    let q = insert("question", r#"{"options":[],"multiple":false,"answer":null,"delegated":false,"expired":false}"#).await.unwrap();
    assert!(asking().await, "offene Frage");

    // Abgelaufen: der Agent macht ohne Antwort weiter
    sqlx::query("UPDATE messages SET data = json_set(data, '$.expired', json('true')) WHERE id = ?").bind(q).execute(t.db()).await.unwrap();
    assert!(!asking().await, "abgelaufene Frage");

    // Neue offene Frage, danach eine eigene Nachricht
    insert("question", r#"{"options":[],"multiple":false,"answer":null,"delegated":false,"expired":false}"#).await.unwrap();
    assert!(asking().await, "zweite offene Frage");
    insert("user", "{}").await.unwrap();
    assert!(!asking().await, "eigene Nachricht danach");
}

/// Deep Research: eigener Ordner (research/<id>) mit eigenem Hinweis statt des Coding-Hinweises,
/// das Paper (paper.md) als Markdown und HTML, das Gespräch darüber arbeitet im selben Ordner und
/// sieht das Paper in seinem Hinweis.
#[rocket::async_test]
async fn deep_research_paper_and_discussion() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    let acc = account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Wie wirkt Koffein?", "agent": "claude", "research": true})).await;
    assert!(s.research && s.project_id.is_none() && s.discuss_of.is_none(), "{s:?}");
    settle(&t, &anna, s.id).await;
    let dir = t.state.config.data_dir.join("research").join(s.id.to_string());
    let cwd = std::fs::read_to_string(home(&t, acc.id).join("last-cwd")).unwrap();
    assert_eq!(std::fs::canonicalize(cwd.trim()).unwrap(), std::fs::canonicalize(&dir).unwrap());
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    assert!(args.contains("Deep Research agent") && args.contains("paper.md"), "{args}");
    assert!(!args.contains("Ship finished code changes"), "kein Coding-Hinweis: {args}");

    // Noch kein Paper: leer; danach Markdown und HTML
    let p: nrzx_shared::agents::ResearchPaper = get(&t, &anna, &format!("/api/v1/threads/{}/paper", s.id)).await;
    assert!(p.markdown.is_empty() && p.updated_at == 0, "{p:?}");
    std::fs::write(dir.join("paper.md"), "# Koffein\n\nBlockiert Adenosin-Rezeptoren [1].\n").unwrap();
    let p: nrzx_shared::agents::ResearchPaper = get(&t, &anna, &format!("/api/v1/threads/{}/paper", s.id)).await;
    assert_eq!(p.research_id, s.id);
    assert!(p.html.contains("<h1") && p.html.contains("Adenosin") && p.updated_at > 0, "{}", p.html);

    // Gespräch: einmal angelegt, danach dasselbe; sieht das Paper, arbeitet im Ordner der Recherche
    let chat: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/discuss", s.id), j!({})).await;
    assert_eq!(chat.discuss_of, Some(s.id));
    assert!(!chat.research);
    let again: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/discuss", s.id), j!({})).await;
    assert_eq!(again.id, chat.id);
    let p: nrzx_shared::agents::ResearchPaper = get(&t, &anna, &format!("/api/v1/threads/{}/paper", chat.id)).await;
    assert_eq!(p.research_id, s.id);
    let _: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/messages", chat.id), j!({"text": "Was blockiert Koffein?"})).await;
    settle(&t, &anna, chat.id).await;
    let args = std::fs::read_to_string(home(&t, acc.id).join("last-args")).unwrap();
    assert!(args.contains("You discuss a research paper") && args.contains("Blockiert Adenosin-Rezeptoren"), "{args}");
    let cwd = std::fs::read_to_string(home(&t, acc.id).join("last-cwd")).unwrap();
    assert_eq!(std::fs::canonicalize(cwd.trim()).unwrap(), std::fs::canonicalize(&dir).unwrap());

    // Keine Recherche: kein Paper, kein Gespräch
    let plain: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Hallo", "agent": "claude"})).await;
    settle(&t, &anna, plain.id).await;
    let res = t.client.get(format!("/api/v1/threads/{}/paper", plain.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NotFound);
    let res = t.client.post(format!("/api/v1/threads/{}/discuss", plain.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::BadRequest);
}

/// Eine Recherche löschen: ihr Gespräch und ihr Ordner (Paper) gehen mit.
#[rocket::async_test]
async fn deleting_research_removes_discussion_and_folder() {
    let t = setup().await;
    let anna = t.user("anna", false).await;
    account(&t, &anna, "claude", "Claude", "sk-ant-api03-abcdefgh").await;
    let s: ThreadSummary = post(&t, &anna, "/api/v1/threads", j!({"prompt": "Was ist Rust?", "agent": "claude", "research": true})).await;
    settle(&t, &anna, s.id).await;
    let dir = t.state.config.data_dir.join("research").join(s.id.to_string());
    assert!(dir.is_dir());
    let chat: ThreadSummary = post(&t, &anna, &format!("/api/v1/threads/{}/discuss", s.id), j!({})).await;
    let res = t.client.delete(format!("/api/v1/threads/{}", s.id)).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::NoContent);
    assert!(!dir.exists(), "Ordner der Recherche entfernt");
    let left: Option<i64> = sqlx::query_scalar("SELECT id FROM threads WHERE id = ?").bind(chat.id).fetch_optional(t.db()).await.unwrap();
    assert!(left.is_none(), "Gespräch gelöscht");
}
