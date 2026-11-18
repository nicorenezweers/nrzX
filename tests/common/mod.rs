//! Hilfen für die Integrationstests: Server mit leerem Datenordner, Benutzer, Projekte
//! (lokales Git-Repository mit erstem Commit) und Threads direkt in der Datenbank anlegen.

#![allow(dead_code)]

use nrzx::AppState;
use rocket::http::{ContentType, Cookie};
use rocket::local::asynchronous::Client;
use std::path::PathBuf;

pub struct TestApp {
    pub client: Client,
    pub state: AppState,
    pub dir: tempfile::TempDir,
}

#[derive(Clone)]
pub struct TestUser {
    pub id: i64,
    pub name: String,
    /// Privates Sitzungs-Cookie (mit `.private_cookie(...)` mitschicken)
    pub cookie: Cookie<'static>,
}

pub const PASSWORD: &str = "geheim-passwort-123";

impl TestApp {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let figment = nrzx::config::figment()
            .merge(("data_dir", dir.path().to_string_lossy().to_string()))
            .merge(("background", false))
            .merge(("preview_port", 0))
            .merge(("log_level", "off"));
        let rocket = nrzx::build_with(figment).await.expect("build");
        let client = Client::untracked(rocket).await.expect("client");
        let state = client.rocket().state::<AppState>().expect("state").clone();
        TestApp { client, state, dir }
    }

    pub fn db(&self) -> &sqlx::SqlitePool {
        &self.state.db
    }

    /// Benutzer direkt anlegen (Passwort [`PASSWORD`]) und eine Sitzung öffnen.
    pub async fn user(&self, name: &str, admin: bool) -> TestUser {
        let hash = nrzx::auth::password::hash(PASSWORD.into()).await.expect("hash");
        let now = nrzx::db::now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, lower_name, email, password_hash, is_admin, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(name)
        .bind(name.to_lowercase())
        .bind(format!("{}@example.com", name.to_lowercase()))
        .bind(hash)
        .bind(admin)
        .bind(now)
        .bind(now)
        .fetch_one(self.db())
        .await
        .expect("user");
        let token = nrzx::auth::session::create(self.db(), id, "test").await.expect("session");
        TestUser { id, name: name.into(), cookie: Cookie::new(nrzx::auth::session::COOKIE, token) }
    }

    /// Projekt mit lokalem Repository (`<data>/workspaces/<id>/repo`, Branch `main`, ein Commit
    /// mit `README.md`) direkt anlegen.
    pub async fn project(&self, user_id: i64, name: &str) -> (i64, PathBuf) {
        let now = nrzx::db::now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO projects (user_id, name, status, created_at, updated_at, last_activity_at) VALUES (?, ?, 'ready', ?, ?, ?) RETURNING id",
        )
        .bind(user_id)
        .bind(name)
        .bind(now)
        .bind(now)
        .bind(now)
        .fetch_one(self.db())
        .await
        .expect("project");
        let repo = self.state.config.workspaces_dir().join(id.to_string()).join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("README.md"), format!("# {name}\n")).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["add", "."],
            vec!["-c", "user.name=test", "-c", "user.email=test@example.com", "commit", "-q", "-m", "Erster Commit"],
        ] {
            let ok = std::process::Command::new("git").args(&args).current_dir(&repo).status().unwrap().success();
            assert!(ok, "git {args:?}");
        }
        (id, repo)
    }

    /// Thread direkt anlegen (ohne Lauf).
    pub async fn thread(&self, user_id: i64, project_id: Option<i64>, title: &str) -> i64 {
        let now = nrzx::db::now();
        sqlx::query_scalar(
            "INSERT INTO threads (user_id, project_id, title, branch, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, 'main', ?, ?, ?) RETURNING id",
        )
        .bind(user_id)
        .bind(project_id)
        .bind(title)
        .bind(now)
        .bind(now)
        .bind(now)
        .fetch_one(self.db())
        .await
        .expect("thread")
    }
}

pub fn json() -> ContentType {
    ContentType::JSON
}
