//! Sprachsteuerung „Dieter“: Einstellungen, Whisper-Modelle, Erkennung und Antworten.
//!
//! * Dieter selbst läuft mit einem nachgeahmten Claude Code (Skript in `<data>/tools/npm/bin`),
//!   das eine JSON-Antwort ausgibt und den Prompt im Home des Kontos ablegt.
//! * Die echte Erkennung läuft nur, wenn `NRZX_TEST_WHISPER` auf einen Modellordner zeigt
//!   (`config.json`, `tokenizer.json`, `model.safetensors` von `openai/whisper-tiny` o. ä.) und
//!   `NRZX_TEST_WAV` auf eine WAV-Datei mit dem Satz „Dieter, schalte dich ab“.

mod common;

use common::{json, TestApp, TestUser};
use nrzx_shared::agents::AgentAccount;
use nrzx_shared::voice::{DieterReply, SttModel, Transcription, VoiceSettings};
use rocket::http::{ContentType, Status};
use serde_json::json as j;

const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "9.9.9 (Claude Code)"; exit 0 ;;
  auth) echo '{"loggedIn":true,"authMethod":"api_key"}'; exit 0 ;;
esac
printf '%s\n' "$@" > "$HOME/dieter-args"
PROMPT=$(cat)
printf '%s' "$PROMPT" > "$HOME/dieter-prompt"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"s1"}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"```json\n{\"reply\":\"Lena baut die Webseite.\",\"actions\":[{\"type\":\"send\",\"to\":\"lehna\",\"prompt\":\"Baue eine Webseite mit Startseite und Kontaktformular.\"},{\"type\":\"new\",\"prompt\":\"x\"}]}\n```"}]},"parent_tool_use_id":null}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"s1"}'
"#;

fn install_fake(t: &TestApp, program: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let dir = t.state.config.tools_dir().join("npm").join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(program);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn wav(samples: &[i16]) -> Vec<u8> {
    let mut v = Vec::new();
    let data_len = (samples.len() * 2) as u32;
    v.extend(b"RIFF");
    v.extend((36 + data_len).to_le_bytes());
    v.extend(b"WAVEfmt ");
    v.extend(16u32.to_le_bytes());
    v.extend(1u16.to_le_bytes());
    v.extend(1u16.to_le_bytes());
    v.extend(16000u32.to_le_bytes());
    v.extend(32000u32.to_le_bytes());
    v.extend(2u16.to_le_bytes());
    v.extend(16u16.to_le_bytes());
    v.extend(b"data");
    v.extend(data_len.to_le_bytes());
    for s in samples {
        v.extend(s.to_le_bytes());
    }
    v
}

async fn get<T: serde::de::DeserializeOwned + Send + 'static>(t: &TestApp, u: &TestUser, path: &str) -> T {
    let res = t.client.get(path).private_cookie(u.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::Ok, "{path}");
    res.into_json().await.expect("json")
}

async fn send(t: &TestApp, u: &TestUser, path: &str, body: &[u8]) -> (Status, String) {
    let res = t.client.post(path).private_cookie(u.cookie.clone()).header(ContentType::new("audio", "wav")).body(body).dispatch().await;
    (res.status(), res.into_string().await.unwrap_or_default())
}

#[rocket::async_test]
async fn settings_models_and_errors() {
    let t = TestApp::new().await;
    let anna = t.user("anna", false).await;

    let s: VoiceSettings = get(&t, &anna, "/api/v1/voice/settings").await;
    assert_eq!(s, VoiceSettings::default());
    assert_eq!(s.stt_model, "base");

    let res = t
        .client
        .put("/api/v1/voice/settings")
        .private_cookie(anna.cookie.clone())
        .header(json())
        .body(j!({"sttModel": "tiny", "language": "EN", "speak": false, "model": "  "}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    let s: VoiceSettings = get(&t, &anna, "/api/v1/voice/settings").await;
    assert_eq!((s.stt_model.as_str(), s.language.as_str(), s.speak, s.model.as_str()), ("tiny", "en", false, ""));

    let res = t
        .client
        .put("/api/v1/voice/settings")
        .private_cookie(anna.cookie.clone())
        .header(json())
        .body(j!({"sttModel": "riesig"}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::BadRequest);

    let models: Vec<SttModel> = get(&t, &anna, "/api/v1/voice/models").await;
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["tiny", "base", "small"]);
    assert!(models.iter().all(|m| !m.installed && !m.downloading && m.size > 100_000_000));

    // Kein Modell geladen: verständlicher Konflikt
    let (status, body) = send(&t, &anna, "/api/v1/voice/transcribe?names=Lena", &wav(&vec![0; 16000])).await;
    assert_eq!(status, Status::Conflict, "{body}");
    assert!(body.contains("Einstellungen → Dieter"), "{body}");
    // Zu kurz: nichts zu erkennen (ohne Modell)
    let (status, body) = send(&t, &anna, "/api/v1/voice/transcribe", &wav(&[0; 100])).await;
    assert_eq!(status, Status::Ok);
    assert_eq!(serde_json::from_str::<Transcription>(&body).unwrap().text, "");
    // Kein WAV
    let (status, _) = send(&t, &anna, "/api/v1/voice/transcribe", b"hallo").await;
    assert_eq!(status, Status::BadRequest);

    // Löschen nur für Administratoren
    let res = t.client.delete("/api/v1/voice/models/tiny").private_cookie(anna.cookie.clone()).dispatch().await;
    assert_ne!(res.status(), Status::Ok);
    // Ohne Anmeldung nichts
    let res = t.client.get("/api/v1/voice/settings").dispatch().await;
    assert_eq!(res.status(), Status::Unauthorized);
}

#[rocket::async_test]
async fn dieter_turns_speech_into_actions() {
    let t = TestApp::new().await;
    install_fake(&t, "claude", FAKE_CLAUDE);
    let anna = t.user("anna", false).await;
    let bernd_thread = t.thread(anna.id, None, "Tests reparieren").await;

    // Ohne Agent-Konto: klare Meldung
    let body = j!({"text": "Sag Lena, sie soll eine Webseite bauen", "panes": [{"name": "Lena"}], "canAdd": false});
    let res = t.client.post("/api/v1/voice/command").private_cookie(anna.cookie.clone()).header(json()).body(body.to_string()).dispatch().await;
    assert_eq!(res.status(), Status::BadRequest);
    assert!(res.into_string().await.unwrap().contains("kein Agent"));

    let res = t
        .client
        .post("/api/v1/accounts")
        .private_cookie(anna.cookie.clone())
        .header(json())
        .body(j!({"kind": "claude", "label": "Privat", "authKind": "api_key", "secret": "sk-test"}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    let acc: AgentAccount = res.into_json().await.unwrap();

    let body = j!({
        "text": "Sag Lena, sie soll eine Webseite bauen",
        "panes": [{"name": "Lena"}, {"name": "Bernd", "threadId": bernd_thread}],
        "focused": "Bernd",
        "canAdd": false,
        "history": [{"role": "user", "text": "Wie ist der Stand?"}, {"role": "dieter", "text": "Bernd arbeitet noch."}]
    });
    let res = t.client.post("/api/v1/voice/command").private_cookie(anna.cookie.clone()).header(json()).body(body.to_string()).dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    let reply: DieterReply = res.into_json().await.unwrap();
    assert_eq!(reply.reply, "Lena baut die Webseite.");
    // Name angepasst („lehna“ → Lena), „new“ verworfen (Raster voll)
    assert_eq!(reply.actions.len(), 1);
    assert_eq!((reply.actions[0].kind.as_str(), reply.actions[0].to.as_str()), ("send", "Lena"));
    assert!(reply.actions[0].prompt.contains("Kontaktformular"));

    // Der Agent sah die Fenster, den Verlauf und den Satz – schnelles Modell, nur lesen
    let home = t.state.config.accounts_dir().join(acc.id.to_string()).join("home");
    let prompt = std::fs::read_to_string(home.join("dieter-prompt")).unwrap();
    assert!(prompt.contains("- Lena [") || prompt.contains("- Lena:"), "{prompt}");
    assert!(prompt.contains("Bernd [currently shown big]") && prompt.contains("Tests reparieren"), "{prompt}");
    assert!(prompt.contains("Bernd arbeitet noch.") && prompt.contains("«Sag Lena, sie soll eine Webseite bauen»"), "{prompt}");
    assert!(prompt.contains("No more windows can be opened"), "{prompt}");
    let args = std::fs::read_to_string(home.join("dieter-args")).unwrap();
    assert!(args.lines().any(|l| l == "haiku"), "{args}");
    assert!(args.contains("--disallowedTools"), "{args}");

    // Fremder Thread bleibt unsichtbar
    let bob = t.user("bob", false).await;
    let _ = t
        .client
        .post("/api/v1/accounts")
        .private_cookie(bob.cookie.clone())
        .header(json())
        .body(j!({"kind": "claude", "label": "Bob", "authKind": "api_key", "secret": "sk-bob"}).to_string())
        .dispatch()
        .await;
    let body = j!({"text": "Status?", "panes": [{"name": "Otto", "threadId": bernd_thread}]});
    let res = t.client.post("/api/v1/voice/command").private_cookie(bob.cookie.clone()).header(json()).body(body.to_string()).dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    let bob_home = t.state.config.accounts_dir();
    let leaked = std::fs::read_dir(bob_home)
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("home").join("dieter-prompt")).ok())
        .any(|p| p.contains("Otto:") && p.contains("Tests reparieren"));
    assert!(!leaked);
}

#[rocket::async_test]
async fn transcribes_with_a_local_model() {
    let (Ok(model), Ok(sample)) = (std::env::var("NRZX_TEST_WHISPER"), std::env::var("NRZX_TEST_WAV")) else {
        return;
    };
    let t = TestApp::new().await;
    let anna = t.user("anna", false).await;
    let dir = t.state.config.models_dir().join("whisper-tiny");
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["config.json", "tokenizer.json", "model.safetensors"] {
        std::os::unix::fs::symlink(std::path::Path::new(&model).join(f), dir.join(f)).unwrap();
    }
    let res = t
        .client
        .put("/api/v1/voice/settings")
        .private_cookie(anna.cookie.clone())
        .header(json())
        .body(j!({"sttModel": "tiny", "language": "de"}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    let models: Vec<SttModel> = get(&t, &anna, "/api/v1/voice/models").await;
    assert!(models.iter().any(|m| m.id == "tiny" && m.installed));
    let (status, body) = send(&t, &anna, "/api/v1/voice/transcribe?names=Lena,Benni", &std::fs::read(sample).unwrap()).await;
    assert_eq!(status, Status::Ok, "{body}");
    let r: Transcription = serde_json::from_str(&body).unwrap();
    assert!(r.text.to_lowercase().contains("dieter"), "{}", r.text);
    assert_eq!(r.language, "de");
}
