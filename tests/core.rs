//! Kern: Gesundheit und Hilfen der Tests.

mod common;

use common::TestApp;
use rocket::http::Status;

#[rocket::async_test]
async fn health_and_helpers() {
    let t = TestApp::new().await;
    let res = t.client.get("/api/v1/health").dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    let anna = t.user("anna", true).await;
    let (project, repo) = t.project(anna.id, "demo").await;
    assert!(repo.join(".git").exists());
    let thread = t.thread(anna.id, Some(project), "Erster Thread").await;
    assert!(thread > 0);
    // Ohne Anmeldung keine Ereignisse
    let res = t.client.get("/api/v1/events").dispatch().await;
    assert_eq!(res.status(), Status::Unauthorized);
}

/// Desktop-Modus: Der Server antwortet nur dem WebView der App (Merkmal im User-Agent),
/// ein Browser auf localhost bekommt nur die Hinweisseite. `/mcp` bleibt für die CLIs offen.
#[rocket::async_test]
async fn desktop_mode_only_answers_the_app() {
    let dir = tempfile::tempdir().expect("tempdir");
    let token = "a".repeat(64);
    let figment = nrzx::config::figment()
        .merge(("data_dir", dir.path().to_string_lossy().to_string()))
        .merge(("background", false))
        .merge(("preview_port", 0))
        .merge(("log_level", "off"))
        .merge(("desktop_token", token.clone()));
    let rocket = nrzx::build_with(figment).await.expect("build");
    let client = rocket::local::asynchronous::Client::untracked(rocket).await.expect("client");
    let browser = "Mozilla/5.0 (Macintosh) AppleWebKit/605.1.15 Safari/605.1.15";
    let app = format!("{browser} nrzXApp/{}", nrzx::desktop::app_marker(&token));

    // Browser: Hinweisseite statt Oberfläche, Login oder API
    for path in ["/", "/api/v1/auth/status", &format!("/__desktop/login?token={token}")] {
        let res = client.get(path).header(rocket::http::Header::new("User-Agent", browser)).dispatch().await;
        assert_eq!(res.status(), rocket::http::Status::Forbidden, "{path}");
        assert!(res.into_string().await.unwrap_or_default().contains("nrzX läuft in der App"));
    }
    // Auch POST wird nicht ausgeführt
    let res = client.post("/api/v1/auth/login").header(rocket::http::Header::new("User-Agent", browser)).dispatch().await;
    assert_eq!(res.status(), rocket::http::Status::Forbidden);
    // Falsches Merkmal
    let res = client.get("/").header(rocket::http::Header::new("User-Agent", format!("{browser} nrzXApp/falsch"))).dispatch().await;
    assert_eq!(res.status(), rocket::http::Status::Forbidden);

    // App: normale Antworten, Anmeldung über das Token
    let res = client.get("/api/v1/auth/status").header(rocket::http::Header::new("User-Agent", app.clone())).dispatch().await;
    assert_eq!(res.status(), rocket::http::Status::Ok);
    let res = client.get(format!("/__desktop/login?token={token}")).header(rocket::http::Header::new("User-Agent", app.clone())).dispatch().await;
    assert_eq!(res.status(), rocket::http::Status::SeeOther);

    // MCP der Agenten-CLIs: nicht gesperrt (eigene Tokens – ohne Token 401)
    let res = client.post("/mcp").header(rocket::http::ContentType::JSON).body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).dispatch().await;
    assert_eq!(res.status(), rocket::http::Status::Unauthorized);
}
