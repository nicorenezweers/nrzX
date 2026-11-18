//! Übernahme aus nrzCode: Konten mit ihren Anmeldedateien, Verbindung zu nrzGit und Projekte
//! aus einer nachgebauten nrzCode-Datenbank.

mod common;

use common::TestApp;
use nrzx_shared::workspace::{NrzCodeImportPreview, NrzCodeImportResult};
use rocket::http::{ContentType, Status};
use serde_json::json;
use sqlx::sqlite::SqliteConnectOptions;

/// nrzCode-Daten: Datenbank mit Schema (gleiche Migrationen), Benutzer, zwei Konten, nrzGit,
/// ein lokales und ein nrzGit-Projekt.
async fn fake_nrzcode(dir: &std::path::Path) {
    let opts = SqliteConnectOptions::new().filename(dir.join("nrzcode.db")).create_if_missing(true);
    let db = sqlx::SqlitePool::connect_with(opts).await.unwrap();
    for (_, _, sql) in nrzx::migrations::MIGRATIONS {
        sqlx::raw_sql(sqlx::AssertSqlSafe(*sql)).execute(&db).await.unwrap();
    }
    let q = |s: &'static str| sqlx::raw_sql(s);
    q("INSERT INTO users (id, username, lower_name, email, password_hash, is_admin, created_at, updated_at) VALUES (1, 'anna', 'anna', 'a@x', 'x', 1, 1, 1)")
        .execute(&db)
        .await
        .unwrap();
    q("INSERT INTO agent_accounts (id, user_id, kind, label, auth_kind, secret, created_at, updated_at) VALUES \
       (7, 1, 'claude', 'Claude Anna', 'login', '', 1, 1), (8, 1, 'opencode', 'OpenRouter', 'api_key', 'sk-or-geheim', 1, 1)")
        .execute(&db)
        .await
        .unwrap();
    q("INSERT INTO nrzgit_connections (user_id, kind, url, token, username, verified_at, created_at, updated_at) VALUES \
       (1, 'nrzgit', 'http://127.0.0.1:9', 'nrzg_token', 'anna', 1, 1, 1)")
        .execute(&db)
        .await
        .unwrap();
    q("INSERT INTO projects (id, user_id, name, repo, clone_url, status, created_at, updated_at, last_activity_at) VALUES \
       (3, 1, 'Notizen', '', '', 'ready', 1, 1, 2), (4, 1, 'shop', 'anna/shop', 'http://127.0.0.1:9/anna/shop.git', 'ready', 1, 1, 1)")
        .execute(&db)
        .await
        .unwrap();
    db.close().await;
    // Home des Claude-Kontos: Anmeldung plus Dinge, die nicht mitkommen sollen
    let home = dir.join("accounts/7/home");
    std::fs::create_dir_all(home.join(".claude/projects")).unwrap();
    std::fs::create_dir_all(home.join(".rustup")).unwrap();
    std::fs::write(home.join(".claude/.credentials.json"), r#"{"claudeAiOauth":{}}"#).unwrap();
    std::fs::write(home.join(".claude/projects/log.jsonl"), "x").unwrap();
    std::fs::write(home.join(".rustup/toolchain"), "x").unwrap();
    // Lokales Projekt
    let repo = dir.join("workspaces/3/repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "# Notizen\n").unwrap();
}

#[rocket::async_test]
async fn takes_over_accounts_nrzgit_and_projects() {
    let src = tempfile::tempdir().unwrap();
    fake_nrzcode(src.path()).await;
    std::env::set_var("NRZX_NRZCODE_DIR", src.path());
    let t = TestApp::new().await;
    let anna = t.user("anna", true).await;

    let res = t.client.get("/api/v1/import/nrzcode").private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    let p: NrzCodeImportPreview = serde_json::from_str(&res.into_string().await.unwrap()).unwrap();
    assert!(p.found);
    assert_eq!(p.accounts.len(), 2);
    assert_eq!(p.nrzgit.as_deref(), Some("http://127.0.0.1:9"));
    assert_eq!(p.projects.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["Notizen", "shop"]);

    let res = t
        .client
        .post("/api/v1/import/nrzcode")
        .header(ContentType::JSON)
        .private_cookie(anna.cookie.clone())
        .body(json!({"accounts": true, "nrzgit": true, "projects": true}).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    let r: NrzCodeImportResult = serde_json::from_str(&res.into_string().await.unwrap()).unwrap();
    assert_eq!((r.accounts, r.nrzgit, r.projects), (2, true, 2), "{:?}", r.skipped);

    // Konten: Schlüssel aus der Datenbank, Anmeldung als Datei – ohne Logs und Toolchains
    let (id, secret): (i64, String) = sqlx::query_as("SELECT id, secret FROM agent_accounts WHERE label = 'OpenRouter'").fetch_one(t.db()).await.unwrap();
    assert!(id > 0 && secret == "sk-or-geheim");
    let claude: i64 = sqlx::query_scalar("SELECT id FROM agent_accounts WHERE label = 'Claude Anna'").fetch_one(t.db()).await.unwrap();
    let home = nrzx::agents::accounts::home(&t.state, claude);
    assert!(home.join(".claude/.credentials.json").is_file());
    assert!(!home.join(".claude/projects").exists() && !home.join(".rustup").exists());
    // nrzGit verbunden, zweites Mal: alles übersprungen
    let token: String = sqlx::query_scalar("SELECT token FROM nrzgit_connections WHERE user_id = ?").bind(anna.id).fetch_one(t.db()).await.unwrap();
    assert_eq!(token, "nrzg_token");
    let res = t
        .client
        .post("/api/v1/import/nrzcode")
        .header(ContentType::JSON)
        .private_cookie(anna.cookie.clone())
        .body(json!({"accounts": true, "nrzgit": true, "projects": true}).to_string())
        .dispatch()
        .await;
    let again: NrzCodeImportResult = serde_json::from_str(&res.into_string().await.unwrap()).unwrap();
    assert_eq!((again.accounts, again.nrzgit, again.projects), (0, false, 0));
    // Vorhandene Konten: Anmeldung frisch übernommen und mit nrzCode verknüpft
    assert_eq!(again.refreshed, 2);
    assert_eq!(again.skipped.len(), 3, "{:?}", again.skipped);
    let linked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_accounts WHERE user_id = ? AND linked_home != ''").bind(anna.id).fetch_one(t.db()).await.unwrap();
    assert_eq!(linked, 2, "übernommene Konten sind verknüpft");

    // Lokales Projekt wird kopiert (im Hintergrund)
    let pid: i64 = sqlx::query_scalar("SELECT id FROM projects WHERE name = 'Notizen'").fetch_one(t.db()).await.unwrap();
    for _ in 0..100 {
        let status: String = sqlx::query_scalar("SELECT status FROM projects WHERE id = ?").bind(pid).fetch_one(t.db()).await.unwrap();
        if status == "ready" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(t.state.config.workspaces_dir().join(format!("{pid}/repo/README.md")).is_file());
}
