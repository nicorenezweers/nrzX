//! Bereich workspace: Anmeldung und Benutzer, Verbindung zu nrzGit (gegen einen kleinen
//! Fake-nrzGit mit API und Git über HTTP via `git http-backend`), Projekte übernehmen/anlegen,
//! Worktrees je Thread, Änderungen, Diff, Commit, Push, Pull Request, Branches, Dateien, Rechte.

mod common;

use base64::Engine;
use common::{TestApp, TestUser, PASSWORD};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response};
use nrzx_shared::workspace::*;
use nrzx_shared::{AuthStatus, Diff, HubEvent, Me, ProjectSummary};
use rocket::http::{ContentType, Cookie, Method, Status};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const TOKEN: &str = "nrzg_testtoken_0123456789abcdef";

// ------------------------------------------------------------------ Hilfen

/// Git synchron ausführen (nur lokale Pfade, kein HTTP).
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Anfrage an nrzX; Antwort als (Status, Text).
async fn call(t: &TestApp, user: Option<&TestUser>, method: Method, path: &str, body: Option<Value>) -> (Status, String) {
    let mut req = t.client.req(method, path.to_string());
    if let Some(u) = user {
        req = req.private_cookie(u.cookie.clone());
    }
    if let Some(b) = body {
        req = req.header(ContentType::JSON).body(b.to_string());
    }
    let res = req.dispatch().await;
    let status = res.status();
    (status, res.into_string().await.unwrap_or_default())
}

async fn ok<T: DeserializeOwned>(t: &TestApp, user: &TestUser, method: Method, path: &str, body: Option<Value>) -> T {
    let (status, text) = call(t, Some(user), method, path, body).await;
    assert!(status.code < 300, "{path}: {status} {text}");
    serde_json::from_str(if text.is_empty() { "null" } else { &text }).unwrap_or_else(|e| panic!("{path}: {e}: {text}"))
}

async fn wait_ready(t: &TestApp, id: i64) -> (String, String) {
    for _ in 0..400 {
        let (s, m): (String, String) = sqlx::query_as("SELECT status, status_message FROM projects WHERE id = ?").bind(id).fetch_one(t.db()).await.unwrap();
        if s != "cloning" {
            return (s, m);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("Projekt {id} wird nicht fertig");
}

fn events(rx: &mut tokio::sync::broadcast::Receiver<(i64, HubEvent)>) -> Vec<(i64, HubEvent)> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

// ------------------------------------------------------------------ Fake-nrzGit

#[derive(Default)]
struct Fake {
    root: PathBuf,
    base: String,
    /// `Authorization` der API-Aufrufe
    api_auth: Vec<String>,
    /// `Authorization` der Git-Aufrufe
    git_auth: Vec<String>,
    /// (Pfad, Inhalt) neuer Pull Requests
    pulls: Vec<(String, Value)>,
}

type Shared = Arc<Mutex<Fake>>;

fn json_response(status: u16, v: Value) -> Response<Body> {
    Response::builder().status(status).header("Content-Type", "application/json").body(Body::from(v.to_string())).unwrap()
}

fn repo_json(base: &str, owner: &str, name: &str, empty: bool) -> Value {
    json!({
        "id": 1, "owner": { "id": 1, "username": owner, "fullName": "", "avatarUrl": "", "isOrg": owner == "team" },
        "name": name, "fullName": format!("{owner}/{name}"), "description": "Demo", "defaultBranch": "main",
        "private": false, "empty": empty, "cloneUrl": format!("{base}/{owner}/{name}.git"), "updatedAt": 1_700_000_000,
        "permission": { "admin": true }
    })
}

/// Bare-Repository `root/<owner>/<name>.git` mit README und src/main.rs (oder leer).
fn seed_repo(root: &Path, owner: &str, name: &str, with_content: bool) -> PathBuf {
    let bare = root.join(owner).join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    if with_content {
        let work = root.join(format!("seed-{owner}-{name}"));
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::write(work.join("README.md"), format!("# {name}\n\nZeile 1\n")).unwrap();
        std::fs::write(work.join("src/main.rs"), "fn main() {\n    println!(\"hallo\");\n}\n").unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "Erster Commit"]);
        git(&work, &["push", "-q", bare.to_str().unwrap(), "main"]);
        std::fs::remove_dir_all(&work).unwrap();
    }
    bare
}

/// `git http-backend` als CGI.
async fn git_backend(root: &Path, path: &str, query: &str, method: &str, ctype: &str, enc: &str, body: Vec<u8>) -> Response<Body> {
    use tokio::io::AsyncWriteExt;
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("http-backend")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path)
        .env("REQUEST_METHOD", method)
        .env("QUERY_STRING", query)
        .env("CONTENT_TYPE", ctype)
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("REMOTE_USER", "anna")
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if !enc.is_empty() {
        cmd.env("HTTP_CONTENT_ENCODING", enc);
    }
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    tokio::spawn(async move {
        let _ = stdin.write_all(&body).await;
        let _ = stdin.shutdown().await;
    });
    let out = child.wait_with_output().await.unwrap();
    let raw = out.stdout;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| (i, 4)).or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)));
    let Some((i, n)) = split else {
        return Response::builder().status(500).body(Body::from(out.stderr)).unwrap();
    };
    let head = String::from_utf8_lossy(&raw[..i]).to_string();
    let mut res = Response::builder().status(200);
    for line in head.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        if k.eq_ignore_ascii_case("status") {
            let code: u16 = v.trim().split(' ').next().unwrap_or("200").parse().unwrap_or(200);
            res = res.status(code);
        } else {
            res = res.header(k.trim(), v.trim());
        }
    }
    res.body(Body::from(raw[i + n..].to_vec())).unwrap()
}

async fn fake_handle(f: Shared, req: Request<Body>) -> Response<Body> {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let method = req.method().to_string();
    let header = |name: &str| req.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let (auth, ctype, enc) = (header("authorization"), header("content-type"), header("content-encoding"));
    let body = hyper::body::to_bytes(req.into_body()).await.unwrap_or_default().to_vec();
    let (root, base) = {
        let g = f.lock().unwrap();
        (g.root.clone(), g.base.clone())
    };
    let token = TOKEN;
    if path.contains(".git/") {
        f.lock().unwrap().git_auth.push(auth.clone());
        let expected = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("anna:{token}")));
        if auth != expected {
            return Response::builder().status(401).header("WWW-Authenticate", "Basic realm=\"nrzGit\"").body(Body::empty()).unwrap();
        }
        return git_backend(&root, &path, &query, &method, &ctype, &enc, body).await;
    }
    let Some(api) = path.strip_prefix("/api/v1") else {
        return json_response(404, json!({}));
    };
    f.lock().unwrap().api_auth.push(auth.clone());
    if auth != format!("token {token}") {
        return json_response(401, json!({ "error": { "code": "unauthorized", "message": "Bitte anmelden." } }));
    }
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    match (method.as_str(), api) {
        ("GET", "/me") => json_response(
            200,
            json!({ "id": 1, "username": "anna", "fullName": "Anna Beispiel",
                    "organizations": [{ "id": 2, "username": "team", "fullName": "Team", "avatarUrl": "", "isOrg": true }] }),
        ),
        ("GET", "/me/repos") => {
            let mut items = Vec::new();
            for owner in std::fs::read_dir(&root).unwrap().flatten().filter(|e| e.path().is_dir()) {
                let o = owner.file_name().to_string_lossy().to_string();
                for r in std::fs::read_dir(owner.path()).unwrap().flatten() {
                    if let Some(n) = r.file_name().to_string_lossy().strip_suffix(".git") {
                        items.push(repo_json(&base, &o, n, false));
                    }
                }
            }
            let total = items.len();
            json_response(200, json!({ "items": items, "total": total, "page": 1, "perPage": 200 }))
        }
        ("POST", "/repos") => {
            let owner = body["owner"].as_str().filter(|s| !s.is_empty()).unwrap_or("anna").to_string();
            let name = body["name"].as_str().unwrap_or("").to_string();
            let readme = body["autoInit"].as_bool().unwrap_or(false);
            seed_repo(&root, &owner, &name, readme);
            json_response(200, repo_json(&base, &owner, &name, !readme))
        }
        ("POST", p) if p.ends_with("/pulls") => {
            f.lock().unwrap().pulls.push((p.to_string(), body.clone()));
            json_response(200, json!({ "id": 3, "number": 7, "title": body["title"], "state": "open" }))
        }
        ("GET", p) if p.starts_with("/repos/") => {
            let mut it = p.trim_start_matches("/repos/").split('/');
            let (o, n) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            if root.join(o).join(format!("{n}.git")).exists() {
                json_response(200, repo_json(&base, o, n, false))
            } else {
                json_response(404, json!({ "error": { "code": "not_found", "message": "Nicht gefunden." } }))
            }
        }
        _ => json_response(404, json!({ "error": { "code": "not_found", "message": "Nicht gefunden." } })),
    }
}

/// Fake-nrzGit auf einem freien Port starten.
fn start_fake(root: PathBuf) -> Shared {
    std::fs::create_dir_all(&root).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let fake = Arc::new(Mutex::new(Fake { root, base: format!("http://127.0.0.1:{port}"), ..Default::default() }));
    let f2 = fake.clone();
    let make = make_service_fn(move |_| {
        let f = f2.clone();
        async move {
            Ok::<_, Infallible>(service_fn(move |req| {
                let f = f.clone();
                async move { Ok::<_, Infallible>(fake_handle(f, req).await) }
            }))
        }
    });
    tokio::spawn(hyper::Server::from_tcp(listener).unwrap().serve(make));
    fake
}

fn base(f: &Shared) -> String {
    f.lock().unwrap().base.clone()
}

async fn connect(t: &TestApp, u: &TestUser, f: &Shared) {
    let s: NrzGitStatus = ok(t, u, Method::Put, "/api/v1/nrzgit", Some(json!({ "url": base(f), "token": TOKEN }))).await;
    assert!(s.connected);
}

/// Projekt „anna/demo“ aus dem Fake übernehmen und warten, bis es geklont ist.
async fn import_demo(t: &TestApp, u: &TestUser, f: &Shared) -> i64 {
    let root = f.lock().unwrap().root.clone();
    if !root.join("anna/demo.git").exists() {
        seed_repo(&root, "anna", "demo", true);
    }
    let p: ProjectSummary = ok(t, u, Method::Post, "/api/v1/projects/import", Some(json!({ "repo": "anna/demo" }))).await;
    assert_eq!(p.status, "cloning");
    let (status, msg) = wait_ready(t, p.id).await;
    assert_eq!(status, "ready", "{msg}");
    p.id
}

// ------------------------------------------------------------------ Anmeldung und Benutzer

#[rocket::async_test]
async fn setup_login_register_and_users() {
    let t = TestApp::new().await;
    let (s, text) = call(&t, None, Method::Get, "/api/v1/auth/status", None).await;
    assert_eq!(s, Status::Ok);
    let st: AuthStatus = serde_json::from_str(&text).unwrap();
    assert!(st.needs_setup && !st.registration_open);

    // Ersteinrichtung: erstes Konto wird Administrator und ist angemeldet
    let res = t
        .client
        .post("/api/v1/auth/setup")
        .header(ContentType::JSON)
        .body(json!({ "username": "Anna", "email": "anna@example.com", "password": PASSWORD, "fullName": "Anna B." }).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    let cookie = res.cookies().get_private(nrzx::auth::session::COOKIE).expect("Sitzung");
    let me: Me = serde_json::from_str(&res.into_string().await.unwrap()).unwrap();
    assert!(me.is_admin && !me.nrzgit_connected);
    let anna = TestUser { id: me.id, name: "Anna".into(), cookie: Cookie::new(nrzx::auth::session::COOKIE, cookie.value().to_string()) };
    let me: Me = ok(&t, &anna, Method::Get, "/api/v1/me", None).await;
    assert_eq!(me.username, "Anna");

    // Zweite Einrichtung geht nicht
    let (s, _) = call(&t, None, Method::Post, "/api/v1/auth/setup", Some(json!({ "username": "x", "email": "x@example.com", "password": PASSWORD }))).await;
    assert_eq!(s, Status::Conflict);

    // Registrierung zu → 403; Verwaltung öffnet sie
    let reg = json!({ "username": "bert", "email": "bert@example.com", "password": PASSWORD });
    let (s, _) = call(&t, None, Method::Post, "/api/v1/auth/register", Some(reg.clone())).await;
    assert_eq!(s, Status::Forbidden);
    let inst: InstanceSettings = ok(&t, &anna, Method::Put, "/api/v1/settings/instance", Some(json!({ "registrationOpen": true, "nrzgitUrl": "git.local:8390/" }))).await;
    assert!(inst.registration_open);
    assert_eq!(inst.nrzgit_url, "http://git.local:8390");
    let (s, text) = call(&t, None, Method::Post, "/api/v1/auth/register", Some(reg)).await;
    assert_eq!(s, Status::Created, "{text}");
    let bert: Me = serde_json::from_str(&text).unwrap();
    assert!(!bert.is_admin);

    // Anmelden: falsches Passwort, richtiges (mit E-Mail)
    let (s, _) = call(&t, None, Method::Post, "/api/v1/auth/login", Some(json!({ "login": "bert", "password": "falsch-falsch" }))).await;
    assert_eq!(s, Status::BadRequest);
    let res = t.client.post("/api/v1/auth/login").header(ContentType::JSON).body(json!({ "login": "BERT@example.com", "password": PASSWORD }).to_string()).dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    let c = res.cookies().get_private(nrzx::auth::session::COOKIE).expect("Sitzung");
    let bert = TestUser { id: bert.id, name: "bert".into(), cookie: Cookie::new(nrzx::auth::session::COOKIE, c.value().to_string()) };

    // Verwaltung nur für Administratoren
    let (s, _) = call(&t, Some(&bert), Method::Get, "/api/v1/users", None).await;
    assert_eq!(s, Status::Forbidden);
    let users: Vec<UserInfo> = ok(&t, &anna, Method::Get, "/api/v1/users", None).await;
    assert_eq!(users.len(), 2);
    let carl: UserInfo = ok(&t, &anna, Method::Post, "/api/v1/users", Some(json!({ "username": "carl", "email": "carl@example.com", "password": PASSWORD, "isAdmin": true }))).await;
    assert!(carl.is_admin);
    // Sperren beendet die Sitzung
    let _: UserInfo = ok(&t, &anna, Method::Patch, &format!("/api/v1/users/{}", bert.id), Some(json!({ "isActive": false }))).await;
    let (s, _) = call(&t, Some(&bert), Method::Get, "/api/v1/me", None).await;
    assert_eq!(s, Status::Unauthorized);
    let (s, _) = call(&t, None, Method::Post, "/api/v1/auth/login", Some(json!({ "login": "bert", "password": PASSWORD }))).await;
    assert_eq!(s, Status::Forbidden);
    // Eigenes Konto nicht löschen; andere schon
    let (s, _) = call(&t, Some(&anna), Method::Delete, &format!("/api/v1/users/{}", anna.id), None).await;
    assert_eq!(s, Status::Conflict);
    let (s, _) = call(&t, Some(&anna), Method::Delete, &format!("/api/v1/users/{}", carl.id), None).await;
    assert_eq!(s, Status::NoContent);
    // Abmelden
    let (s, _) = call(&t, Some(&anna), Method::Post, "/api/v1/auth/logout", None).await;
    assert_eq!(s, Status::NoContent);
    let (s, _) = call(&t, Some(&anna), Method::Get, "/api/v1/me", None).await;
    assert_eq!(s, Status::Unauthorized);
    // Avatar ohne Anmeldung
    let res = t.client.get("/api/v1/users/anna/avatar?s=40").dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    assert_eq!(res.content_type(), Some(ContentType::SVG));
}

#[rocket::async_test]
async fn profile_password_appearance_and_stats() {
    let t = TestApp::new().await;
    let anna = t.user("anna", false).await;
    let me: Me = ok(&t, &anna, Method::Put, "/api/v1/me/profile", Some(json!({ "fullName": "Anna Beispiel", "email": "a@example.org" }))).await;
    assert_eq!(me.full_name, "Anna Beispiel");
    let (s, _) = call(&t, Some(&anna), Method::Put, "/api/v1/me/profile", Some(json!({ "fullName": "x", "email": "kein-mail" }))).await;
    assert_eq!(s, Status::BadRequest);
    let a: Appearance = ok(&t, &anna, Method::Put, "/api/v1/me/appearance", Some(json!({ "theme": "light", "language": "en" }))).await;
    assert_eq!(a.theme, "light");
    let me: Me = ok(&t, &anna, Method::Get, "/api/v1/me", None).await;
    assert_eq!((me.theme.as_str(), me.language.as_str()), ("light", "en"));
    let a: Appearance = ok(&t, &anna, Method::Put, "/api/v1/me/appearance", Some(json!({ "theme": "mondnacht", "language": "de" }))).await;
    assert_eq!(a.theme, "mondnacht");
    let (s, _) = call(&t, Some(&anna), Method::Put, "/api/v1/me/appearance", Some(json!({ "theme": "pink", "language": "de" }))).await;
    assert_eq!(s, Status::BadRequest);
    let (s, _) = call(&t, Some(&anna), Method::Put, "/api/v1/me/password", Some(json!({ "currentPassword": "falsch", "newPassword": "neues-passwort-1" }))).await;
    assert_eq!(s, Status::BadRequest);
    let res = t
        .client
        .put("/api/v1/me/password")
        .private_cookie(anna.cookie.clone())
        .header(ContentType::JSON)
        .body(json!({ "currentPassword": PASSWORD, "newPassword": "neues-passwort-1" }).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::NoContent);
    assert!(res.cookies().get_private(nrzx::auth::session::COOKIE).is_some(), "neue Sitzung");
    // Alte Sitzung ist beendet
    let (s, _) = call(&t, Some(&anna), Method::Get, "/api/v1/me", None).await;
    assert_eq!(s, Status::Unauthorized);

    let bert = t.user("bert", false).await;
    let (p, _) = t.project(bert.id, "demo").await;
    t.thread(bert.id, Some(p), "Eins").await;
    let st: Stats = ok(&t, &bert, Method::Get, "/api/v1/me/stats", None).await;
    assert_eq!((st.projects, st.threads), (1, 1));
}

// ------------------------------------------------------------------ nrzGit

#[rocket::async_test]
async fn nrzgit_connection_uses_token_header() {
    let t = TestApp::new().await;
    let f = start_fake(t.dir.path().join("fake"));
    let anna = t.user("anna", false).await;

    // Vorher: nicht verbunden, Repos verlangen eine Verbindung
    let s: NrzGitStatus = ok(&t, &anna, Method::Get, "/api/v1/nrzgit", None).await;
    assert!(!s.connected && s.url.is_empty());
    let (st, _) = call(&t, Some(&anna), Method::Get, "/api/v1/nrzgit/repos", None).await;
    assert_eq!(st, Status::BadRequest);

    // Falsches Token: abgelehnt, nichts gespeichert
    let (st, text) = call(&t, Some(&anna), Method::Put, "/api/v1/nrzgit", Some(json!({ "url": base(&f), "token": "nrzg_falsch" }))).await;
    assert_eq!(st, Status::BadRequest, "{text}");
    assert!(text.contains("Token"));
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nrzgit_connections").fetch_one(t.db()).await.unwrap();
    assert_eq!(n, 0);

    // Richtiges Token
    let (st, text) = call(&t, Some(&anna), Method::Put, "/api/v1/nrzgit", Some(json!({ "url": format!("{}/", base(&f)), "token": TOKEN }))).await;
    assert_eq!(st, Status::Ok, "{text}");
    assert!(!text.contains(TOKEN), "Token darf nie ausgeliefert werden");
    let s: NrzGitStatus = serde_json::from_str(&text).unwrap();
    assert!(s.connected);
    assert_eq!(s.username, "anna");
    assert_eq!(s.url, base(&f));
    assert_eq!(s.token_url, format!("{}/user/settings/mcp", base(&f)));
    assert!(s.token_hint.starts_with("nrzg_te") && s.token_hint.contains('…'));
    assert!(f.lock().unwrap().api_auth.iter().any(|a| a == &format!("token {TOKEN}")));
    let me: Me = ok(&t, &anna, Method::Get, "/api/v1/me", None).await;
    assert!(me.nrzgit_connected);
    assert_eq!(me.nrzgit_url, base(&f));

    // Ohne Token bei gleicher Adresse: gespeichertes weiterverwenden; andere Adresse braucht ein Token
    let _: NrzGitStatus = ok(&t, &anna, Method::Put, "/api/v1/nrzgit", Some(json!({ "url": base(&f) }))).await;
    let (st, _) = call(&t, Some(&anna), Method::Put, "/api/v1/nrzgit", Some(json!({ "url": "http://anderswo.example" }))).await;
    assert_eq!(st, Status::BadRequest);

    // Repositories und Besitzer
    seed_repo(&f.lock().unwrap().root.clone(), "anna", "demo", true);
    seed_repo(&f.lock().unwrap().root.clone(), "team", "web", true);
    let repos: Vec<NrzGitRepo> = ok(&t, &anna, Method::Get, "/api/v1/nrzgit/repos?q=", None).await;
    assert_eq!(repos.len(), 2);
    assert!(repos.iter().any(|r| r.full_name == "team/web" && r.owner_is_org));
    assert!(repos.iter().all(|r| r.project_id.is_none()));
    let owners: Vec<NrzGitOwner> = ok(&t, &anna, Method::Get, "/api/v1/nrzgit/owners", None).await;
    assert_eq!(owners.iter().map(|o| o.username.as_str()).collect::<Vec<_>>(), vec!["anna", "team"]);

    // Umgebung der Agenten: Git-Anmeldung, Identität, nrzGit, Docker-Registry
    let env = nrzx::workspace::agent_env(&t.state, anna.id).await.unwrap();
    let get = |k: &str| env.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone()).unwrap_or_default();
    assert_eq!(get("GIT_CONFIG_COUNT"), "1");
    assert_eq!(get("GIT_CONFIG_KEY_0"), format!("http.{}/.extraHeader", base(&f)));
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("anna:{TOKEN}"));
    assert_eq!(get("GIT_CONFIG_VALUE_0"), format!("Authorization: Basic {basic}"));
    assert_eq!(get("NRZGIT_URL"), base(&f));
    assert_eq!(get("NRZGIT_TOKEN"), TOKEN);
    assert_eq!(get("GIT_AUTHOR_EMAIL"), "anna@example.com");
    let docker: Value = serde_json::from_slice(&std::fs::read(Path::new(&get("DOCKER_CONFIG")).join("config.json")).unwrap()).unwrap();
    let host = base(&f).trim_start_matches("http://").to_string();
    assert_eq!(docker["auths"][&host]["auth"], basic);

    // Prüfen, trennen
    let s: NrzGitStatus = ok(&t, &anna, Method::Post, "/api/v1/nrzgit/verify", None).await;
    assert!(s.connected);
    let (st, _) = call(&t, Some(&anna), Method::Delete, "/api/v1/nrzgit", None).await;
    assert_eq!(st, Status::NoContent);
    let me: Me = ok(&t, &anna, Method::Get, "/api/v1/me", None).await;
    assert!(!me.nrzgit_connected);
    let env = nrzx::workspace::agent_env(&t.state, anna.id).await.unwrap();
    assert!(!env.iter().any(|(k, _)| k == "NRZGIT_TOKEN"));
}

#[rocket::async_test]
async fn import_worktrees_changes_diff_commit_push_and_pull_request() {
    let t = TestApp::new().await;
    let f = start_fake(t.dir.path().join("fake"));
    let anna = t.user("anna", false).await;
    connect(&t, &anna, &f).await;
    let mut rx = t.state.hub.subscribe();
    let pid = import_demo(&t, &anna, &f).await;
    assert!(events(&mut rx).iter().any(|(u, e)| *u == anna.id && matches!(e, HubEvent::ProjectUpdated { project } if project.id == pid && project.status == "ready")));

    // Geklont mit Token im Kopf, nicht in der Adresse
    let repo = t.state.config.workspaces_dir().join(pid.to_string()).join("repo");
    assert!(repo.join("README.md").exists());
    let remote = git(&repo, &["remote", "get-url", "origin"]);
    assert_eq!(remote.trim(), format!("{}/anna/demo.git", base(&f)));
    assert!(!std::fs::read_to_string(repo.join(".git/config")).unwrap().contains(TOKEN));
    let basic = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("anna:{TOKEN}")));
    assert!(!f.lock().unwrap().git_auth.is_empty());
    assert!(f.lock().unwrap().git_auth.iter().all(|a| a == &basic));

    // Doppelt übernehmen gibt das bestehende Projekt zurück; die Liste kennt das Projekt
    let (s, again) = call(&t, Some(&anna), Method::Post, "/api/v1/projects/import", Some(json!({ "repo": "anna/demo" }))).await;
    assert_eq!(s, Status::Ok);
    assert_eq!(serde_json::from_str::<ProjectSummary>(&again).unwrap().id, pid);
    let repos: Vec<NrzGitRepo> = ok(&t, &anna, Method::Get, "/api/v1/nrzgit/repos", None).await;
    assert_eq!(repos[0].project_id, Some(pid));
    let list: Vec<ProjectSummary> = ok(&t, &anna, Method::Get, "/api/v1/projects", None).await;
    assert_eq!(list.len(), 1);
    let detail: ProjectDetail = ok(&t, &anna, Method::Get, &format!("/api/v1/projects/{pid}"), None).await;
    assert_eq!(detail.web_url, format!("{}/anna/demo", base(&f)));
    assert_eq!(detail.branch, "main");
    assert_eq!(detail.commits.len(), 1);

    // Worktree je Thread mit eigenem Branch
    let t1 = t.thread(anna.id, Some(pid), "Login-Seite bauen").await;
    let t2 = t.thread(anna.id, Some(pid), "Doku").await;
    let w1 = nrzx::workspace::thread_workdir(&t.state, t1).await.unwrap();
    let w2 = nrzx::workspace::thread_workdir(&t.state, t2).await.unwrap();
    assert_eq!(w1, t.state.config.workspaces_dir().join(format!("{pid}/worktrees/{t1}")));
    assert_ne!(w1, w2);
    let b1 = git(&w1, &["branch", "--show-current"]);
    assert_eq!(b1.trim(), format!("nrz/login-seite-bauen-{t1}"));
    assert_eq!(git(&w2, &["branch", "--show-current"]).trim(), format!("nrz/doku-{t2}"));
    let stored: String = sqlx::query_scalar("SELECT branch FROM threads WHERE id = ?").bind(t1).fetch_one(t.db()).await.unwrap();
    assert_eq!(stored, b1.trim());
    // Zweiter Aufruf: derselbe Ordner
    assert_eq!(nrzx::workspace::thread_workdir(&t.state, t1).await.unwrap(), w1);
    // Ohne Projekt: Scratch-Ordner
    let t3 = t.thread(anna.id, None, "Frage").await;
    assert_eq!(nrzx::workspace::thread_workdir(&t.state, t3).await.unwrap(), t.state.config.scratch_dir().join(t3.to_string()));

    // Änderungen: geänderte und neue (unversionierte) Datei
    std::fs::write(w1.join("README.md"), "# demo\n\nZeile 1\nZeile 2\n").unwrap();
    std::fs::write(w1.join("src/login.rs"), "pub fn login() -> bool {\n    true\n}\n").unwrap();
    events(&mut rx);
    nrzx::workspace::after_run(&t.state, t1).await.unwrap();
    let ev = events(&mut rx);
    assert!(
        ev.iter().any(|(_, e)| matches!(e, HubEvent::ChangesUpdated { thread_id, additions: 4, deletions: 0, files: 2 } if *thread_id == t1)),
        "{ev:?}"
    );
    // Der echte Index des Worktrees bleibt unberührt
    assert!(git(&w1, &["diff", "--cached", "--name-only"]).trim().is_empty());
    let st: WorkspaceStatus = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{t1}/status"), None).await;
    assert!(st.is_git && st.worktree && st.use_worktree && st.has_remote);
    assert_eq!(st.branch, b1.trim());
    assert_eq!((st.additions, st.deletions, st.files.len()), (4, 0, 2));
    assert_eq!(st.files.iter().find(|f| f.path == "src/login.rs").unwrap().status, "added");
    assert_eq!(st.repo_url, format!("{}/anna/demo", base(&f)));
    assert_eq!(st.uncommitted.len(), 2);
    assert_eq!(st.title, "Login-Seite bauen");
    assert!(st.upstream.is_empty());

    // Diff mit Syntaxfarben
    let d: Diff = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{t1}/diff"), None).await;
    assert_eq!(d.files.len(), 2);
    let rs = d.files.iter().find(|f| f.new_path == "src/login.rs").unwrap();
    assert_eq!(rs.status, "added");
    assert!(rs.hunks[0].lines[0].html.as_deref().unwrap_or("").contains("hl-"), "{:?}", rs.hunks[0].lines[0]);
    let one: Diff = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{t1}/diff?path=README.md"), None).await;
    assert_eq!(one.files.len(), 1);

    // Commit & Push über HTTP (Token im Kopf)
    let c: CommitResult = ok(&t, &anna, Method::Post, &format!("/api/v1/workspace/{t1}/commit"), Some(json!({ "message": "Login-Seite", "push": true }))).await;
    assert!(c.pushed);
    assert_eq!((c.files, c.summary.as_str()), (2, "Login-Seite"));
    let bare = f.lock().unwrap().root.join("anna/demo.git");
    assert_eq!(git(&bare, &["rev-parse", b1.trim()]).trim(), c.sha);
    assert_eq!(git(&bare, &["log", "-1", "--format=%an <%ae>", b1.trim()]).trim(), "anna <anna@example.com>");
    let st: WorkspaceStatus = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{t1}/status"), None).await;
    assert_eq!(st.upstream, format!("origin/{}", b1.trim()));
    assert_eq!((st.ahead, st.behind), (0, 0));
    // Änderungen gegen den Standard-Branch bleiben sichtbar (der Branch ist noch nicht gemergt)
    assert_eq!(st.files.len(), 2);
    assert!(st.uncommitted.is_empty());
    let (s, _) = call(&t, Some(&anna), Method::Post, &format!("/api/v1/workspace/{t1}/commit"), Some(json!({ "message": "leer" }))).await;
    assert_eq!(s, Status::BadRequest);

    // Pull Request in nrzGit
    let pr: PullRequestRef = ok(&t, &anna, Method::Post, &format!("/api/v1/workspace/{t1}/pull-request"), Some(json!({ "title": "Login-Seite", "body": "Neu" }))).await;
    assert_eq!(pr.number, 7);
    assert_eq!(pr.url, format!("{}/anna/demo/pulls/7", base(&f)));
    let pulls = f.lock().unwrap().pulls.clone();
    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0].0, "/repos/anna/demo/pulls");
    assert_eq!(pulls[0].1["base"], "main");
    assert_eq!(pulls[0].1["head"], b1.trim());
    assert_eq!(pulls[0].1["title"], "Login-Seite");
    let st: WorkspaceStatus = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{t1}/status"), None).await;
    assert_eq!(st.pull_request.unwrap().number, 7);
    // Auf dem Standard-Branch gibt es keinen Pull Request
    sqlx::query("UPDATE threads SET use_worktree = 0 WHERE id = ?").bind(t3).execute(t.db()).await.unwrap();
    let t4 = t.thread(anna.id, Some(pid), "Hauptcheckout").await;
    sqlx::query("UPDATE threads SET use_worktree = 0 WHERE id = ?").bind(t4).execute(t.db()).await.unwrap();
    assert_eq!(nrzx::workspace::thread_workdir(&t.state, t4).await.unwrap(), repo);
    let (s, _) = call(&t, Some(&anna), Method::Post, &format!("/api/v1/workspace/{t4}/pull-request"), Some(json!({ "title": "x" }))).await;
    assert_eq!(s, Status::BadRequest);

    // Pull im Hauptcheckout: neuer Commit auf main im Server
    let work = t.dir.path().join("other");
    git(t.dir.path(), &["clone", "-q", bare.to_str().unwrap(), work.to_str().unwrap()]);
    std::fs::write(work.join("NEU.md"), "neu\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "Neu auf main"]);
    git(&work, &["push", "-q", "origin", "main"]);
    let r: GitResult = ok(&t, &anna, Method::Post, &format!("/api/v1/workspace/{t4}/pull"), None).await;
    assert_eq!((r.branch.as_str(), r.behind), ("main", 0));
    assert!(repo.join("NEU.md").exists());
    let r: GitResult = ok(&t, &anna, Method::Post, &format!("/api/v1/projects/{pid}/refresh"), None).await;
    assert_eq!(r.branch, "main");
}

#[rocket::async_test]
async fn create_repo_in_nrzgit_and_local_project() {
    let t = TestApp::new().await;
    let f = start_fake(t.dir.path().join("fake"));
    let anna = t.user("anna", false).await;
    // Ohne Verbindung kein Anlegen in nrzGit
    let (s, _) = call(&t, Some(&anna), Method::Post, "/api/v1/projects/create", Some(json!({ "name": "neu" }))).await;
    assert_eq!(s, Status::BadRequest);
    connect(&t, &anna, &f).await;
    let p: ProjectSummary = ok(&t, &anna, Method::Post, "/api/v1/projects/create", Some(json!({ "owner": "team", "name": "neu", "readme": true }))).await;
    assert_eq!(p.repo, "team/neu");
    assert_eq!(wait_ready(&t, p.id).await.0, "ready");
    assert!(t.state.config.workspaces_dir().join(format!("{}/repo/README.md", p.id)).exists());
    // Name, den es in nrzGit schon gibt: übernehmen statt anlegen
    seed_repo(&f.lock().unwrap().root.clone(), "anna", "vorhanden", true);
    let v: ProjectSummary = ok(&t, &anna, Method::Post, "/api/v1/projects/create", Some(json!({ "name": "vorhanden" }))).await;
    assert_eq!(v.repo, "anna/vorhanden");
    assert_eq!(wait_ready(&t, v.id).await.0, "ready");
    // Leeres Repository: geklont, HEAD auf main, Worktree erst mit dem ersten Commit
    let e: ProjectSummary = ok(&t, &anna, Method::Post, "/api/v1/projects/create", Some(json!({ "name": "leer" }))).await;
    assert_eq!(e.repo, "anna/leer");
    assert_eq!(wait_ready(&t, e.id).await.0, "ready");
    let th = t.thread(anna.id, Some(e.id), "Start").await;
    let dir = nrzx::workspace::thread_workdir(&t.state, th).await.unwrap();
    assert_eq!(dir, t.state.config.workspaces_dir().join(format!("{}/repo", e.id)));

    // Lokales Projekt
    let l: ProjectSummary = ok(&t, &anna, Method::Post, "/api/v1/projects/local", Some(json!({ "name": "Spielwiese" }))).await;
    assert_eq!((l.status.as_str(), l.repo.as_str()), ("ready", ""));
    let repo = t.state.config.workspaces_dir().join(format!("{}/repo", l.id));
    assert_eq!(git(&repo, &["log", "--format=%s"]).trim(), "Projekt angelegt");
    let r: GitResult = ok(&t, &anna, Method::Post, &format!("/api/v1/projects/{}/refresh", l.id), None).await;
    assert!(r.message.contains("Lokal"));
    // Umbenennen
    let r: ProjectSummary = ok(&t, &anna, Method::Patch, &format!("/api/v1/projects/{}", l.id), Some(json!({ "name": "Sandkasten" }))).await;
    assert_eq!(r.name, "Sandkasten");
    // Push ohne Remote geht nicht
    let th = t.thread(anna.id, Some(l.id), "Etwas").await;
    let (s, text) = call(&t, Some(&anna), Method::Post, &format!("/api/v1/workspace/{th}/push"), None).await;
    assert_eq!(s, Status::Conflict, "{text}");
}

#[rocket::async_test]
async fn commit_selected_files_and_push_to_local_bare_repo() {
    let t = TestApp::new().await;
    let anna = t.user("anna", false).await;
    let (pid, repo) = t.project(anna.id, "demo").await;
    let bare = t.dir.path().join("remote.git");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["fetch", "-q", "origin"]);

    let th = t.thread(anna.id, Some(pid), "Zwei Dateien").await;
    let w = nrzx::workspace::thread_workdir(&t.state, th).await.unwrap();
    std::fs::write(w.join("a.txt"), "a\n").unwrap();
    std::fs::write(w.join("b.txt"), "b\n").unwrap();
    git(&w, &["mv", "README.md", "LIESMICH.md"]);

    // Nur a.txt und die Umbenennung
    let c: CommitResult = ok(&t, &anna, Method::Post, &format!("/api/v1/workspace/{th}/commit"), Some(json!({ "message": "Teil", "files": ["a.txt", "LIESMICH.md"] }))).await;
    // Umbenennung zählt als eine Datei
    assert_eq!(c.files, 2);
    let shown = git(&w, &["show", "--name-status", "--format=", "HEAD"]);
    assert!(shown.contains("a.txt") && shown.contains("LIESMICH.md") && shown.contains("README.md") && !shown.contains("b.txt"), "{shown}");
    let st: WorkspaceStatus = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{th}/status"), None).await;
    assert_eq!(st.ahead, 1);
    // b.txt bleibt als Änderung
    assert!(git(&w, &["status", "--porcelain"]).contains("b.txt"));
    let (s, _) = call(&t, Some(&anna), Method::Post, &format!("/api/v1/workspace/{th}/commit"), Some(json!({ "message": "x", "files": ["../aussen"] }))).await;
    assert_eq!(s, Status::BadRequest);

    let r: GitResult = ok(&t, &anna, Method::Post, &format!("/api/v1/workspace/{th}/push"), None).await;
    assert_eq!((r.ahead, r.behind), (0, 0));
    let branch = st.branch.clone();
    assert_eq!(git(&bare, &["rev-parse", &branch]).trim(), c.sha);
    let log = git(&bare, &["log", "--format=%s", &branch]);
    assert_eq!(log.lines().collect::<Vec<_>>(), vec!["Teil", "Erster Commit"]);
}

#[rocket::async_test]
async fn branches_and_worktree_setup() {
    let t = TestApp::new().await;
    let anna = t.user("anna", false).await;
    let (pid, repo) = t.project(anna.id, "demo").await;
    let th = t.thread(anna.id, Some(pid), "Branches").await;
    sqlx::query("UPDATE threads SET use_worktree = 0 WHERE id = ?").bind(th).execute(t.db()).await.unwrap();
    let other = t.thread(anna.id, Some(pid), "Nachbar").await;
    sqlx::query("UPDATE threads SET use_worktree = 0 WHERE id = ?").bind(other).execute(t.db()).await.unwrap();

    let st: WorkspaceStatus = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{th}/status"), None).await;
    assert!(!st.worktree && !st.use_worktree);
    assert_eq!(st.path, repo.to_string_lossy());

    // Neuer Branch im Hauptcheckout gilt für alle Threads dort
    let st: WorkspaceStatus = ok(&t, &anna, Method::Post, &format!("/api/v1/workspace/{th}/branch"), Some(json!({ "name": "feature/x", "create": true }))).await;
    assert_eq!(st.branch, "feature/x");
    let b: String = sqlx::query_scalar("SELECT branch FROM threads WHERE id = ?").bind(other).fetch_one(t.db()).await.unwrap();
    assert_eq!(b, "feature/x");
    let list: BranchList = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{th}/branches"), None).await;
    assert_eq!(list.current, "feature/x");
    assert!(list.branches.iter().any(|b| b.name == "main" && !b.current));
    let st: WorkspaceStatus = ok(&t, &anna, Method::Post, &format!("/api/v1/workspace/{th}/branch"), Some(json!({ "name": "main" }))).await;
    assert_eq!(st.branch, "main");
    let (s, _) = call(&t, Some(&anna), Method::Post, &format!("/api/v1/workspace/{th}/branch"), Some(json!({ "name": "gibt-es-nicht" }))).await;
    assert_eq!(s, Status::BadRequest);
    let (s, _) = call(&t, Some(&anna), Method::Post, &format!("/api/v1/workspace/{th}/branch"), Some(json!({ "name": "-x", "create": true }))).await;
    assert_eq!(s, Status::BadRequest);

    // Worktree einrichten und wieder entfernen (Branch bleibt)
    let st: WorkspaceStatus = ok(&t, &anna, Method::Post, &format!("/api/v1/workspace/{th}/worktree"), None).await;
    assert!(st.worktree && st.use_worktree);
    assert_eq!(st.branch, format!("nrz/branches-{th}"));
    let list: BranchList = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{other}/branches"), None).await;
    assert!(list.branches.iter().any(|b| b.name == st.branch && b.checked_out_elsewhere));
    // Während der Agent arbeitet: kein Wechsel
    sqlx::query("UPDATE threads SET status = 'working' WHERE id = ?").bind(th).execute(t.db()).await.unwrap();
    let (s, _) = call(&t, Some(&anna), Method::Delete, &format!("/api/v1/workspace/{th}/worktree"), None).await;
    assert_eq!(s, Status::Conflict);
    sqlx::query("UPDATE threads SET status = 'done' WHERE id = ?").bind(th).execute(t.db()).await.unwrap();
    let st: WorkspaceStatus = ok(&t, &anna, Method::Delete, &format!("/api/v1/workspace/{th}/worktree"), None).await;
    assert!(!st.worktree && !st.use_worktree);
    assert_eq!(st.branch, "main");
    assert!(!t.state.config.workspaces_dir().join(format!("{pid}/worktrees/{th}")).exists());
    assert!(git(&repo, &["branch", "--list", &format!("nrz/branches-{th}")]).contains("nrz/"));
    // Wieder einrichten: derselbe Branch
    let st: WorkspaceStatus = ok(&t, &anna, Method::Post, &format!("/api/v1/workspace/{th}/worktree"), None).await;
    assert_eq!(st.branch, format!("nrz/branches-{th}"));
}

#[rocket::async_test]
async fn files_read_write_and_traversal() {
    let t = TestApp::new().await;
    let anna = t.user("anna", false).await;
    let (pid, _) = t.project(anna.id, "demo").await;
    let th = t.thread(anna.id, Some(pid), "Dateien").await;
    let list: Vec<FileEntry> = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{th}/files"), None).await;
    assert_eq!(list.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["README.md"]);
    let (s, _) = call(&t, Some(&anna), Method::Put, &format!("/api/v1/workspace/{th}/file"), Some(json!({ "path": "src/neu.rs", "content": "fn x() {}\n" }))).await;
    assert_eq!(s, Status::NoContent);
    let list: Vec<FileEntry> = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{th}/files"), None).await;
    assert!(list[0].dir && list[0].name == "src");
    let c: FileContent = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{th}/file?path=src/neu.rs"), None).await;
    assert_eq!(c.content, "fn x() {}\n");
    let all: Vec<String> = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{th}/files/all"), None).await;
    assert_eq!(all, vec!["README.md", "src/neu.rs"]);
    // Nicht hinaus, nicht in .git
    for p in ["../../etc/passwd", "/etc/passwd", ".git/config"] {
        let (s, _) = call(&t, Some(&anna), Method::Get, &format!("/api/v1/workspace/{th}/file?path={}", urlencoding::encode(p)), None).await;
        assert!(s == Status::BadRequest || s == Status::Forbidden, "{p}: {s}");
    }
    let (s, _) = call(&t, Some(&anna), Method::Put, &format!("/api/v1/workspace/{th}/file"), Some(json!({ "path": "../x", "content": "" }))).await;
    assert_eq!(s, Status::BadRequest);
    // Symlink nach draussen
    let w = nrzx::workspace::thread_workdir(&t.state, th).await.unwrap();
    std::os::unix::fs::symlink(t.dir.path(), w.join("raus")).unwrap();
    let (s, _) = call(&t, Some(&anna), Method::Get, &format!("/api/v1/workspace/{th}/files?path=raus"), None).await;
    assert_eq!(s, Status::Forbidden);
    // Binärdatei ohne Inhalt
    std::fs::write(w.join("bild.bin"), [0u8, 1, 2, 3]).unwrap();
    let c: FileContent = ok(&t, &anna, Method::Get, &format!("/api/v1/workspace/{th}/file?path=bild.bin"), None).await;
    assert!(c.binary && c.content.is_empty());
}

#[rocket::async_test]
async fn foreign_projects_and_threads_are_hidden() {
    let t = TestApp::new().await;
    let anna = t.user("anna", false).await;
    let bert = t.user("bert", true).await;
    let (pid, _) = t.project(anna.id, "geheim").await;
    let th = t.thread(anna.id, Some(pid), "Privat").await;
    let list: Vec<ProjectSummary> = ok(&t, &bert, Method::Get, "/api/v1/projects", None).await;
    assert!(list.is_empty());
    for (m, path, body) in [
        (Method::Get, format!("/api/v1/projects/{pid}"), None),
        (Method::Patch, format!("/api/v1/projects/{pid}"), Some(json!({ "name": "meins" }))),
        (Method::Post, format!("/api/v1/projects/{pid}/refresh"), None),
        (Method::Delete, format!("/api/v1/projects/{pid}"), None),
        (Method::Get, format!("/api/v1/workspace/{th}/status"), None),
        (Method::Get, format!("/api/v1/workspace/{th}/diff"), None),
        (Method::Post, format!("/api/v1/workspace/{th}/commit"), Some(json!({ "message": "x" }))),
        (Method::Post, format!("/api/v1/workspace/{th}/push"), None),
        (Method::Get, format!("/api/v1/workspace/{th}/branches"), None),
        (Method::Post, format!("/api/v1/workspace/{th}/worktree"), None),
        (Method::Get, format!("/api/v1/workspace/{th}/files"), None),
        (Method::Get, format!("/api/v1/workspace/{th}/file?path=README.md"), None),
        (Method::Put, format!("/api/v1/workspace/{th}/file"), Some(json!({ "path": "x", "content": "y" }))),
        (Method::Get, format!("/api/v1/previews?threadId={th}"), None),
        (Method::Post, "/api/v1/previews/token".to_string(), Some(json!({ "threadId": th, "port": 5173 }))),
    ] {
        let (s, _) = call(&t, Some(&bert), m, &path, body).await;
        assert_eq!(s, Status::NotFound, "{m} {path}");
    }
    // Ohne Anmeldung: 401
    let (s, _) = call(&t, None, Method::Get, "/api/v1/projects", None).await;
    assert_eq!(s, Status::Unauthorized);
    // Das Projekt steht noch
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects").fetch_one(t.db()).await.unwrap();
    assert_eq!(n, 1);
}

#[rocket::async_test]
async fn delete_project_removes_threads_and_folders() {
    let t = TestApp::new().await;
    let anna = t.user("anna", false).await;
    let (pid, _) = t.project(anna.id, "weg").await;
    let th = t.thread(anna.id, Some(pid), "Weg damit").await;
    let w = nrzx::workspace::thread_workdir(&t.state, th).await.unwrap();
    assert!(w.exists());
    let mut rx = t.state.hub.subscribe();
    let (s, _) = call(&t, Some(&anna), Method::Delete, &format!("/api/v1/projects/{pid}"), None).await;
    assert_eq!(s, Status::NoContent);
    assert!(!t.state.config.workspaces_dir().join(pid.to_string()).exists());
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM threads").fetch_one(t.db()).await.unwrap();
    assert_eq!(n, 0);
    let ev = events(&mut rx);
    assert!(ev.iter().any(|(_, e)| matches!(e, HubEvent::ThreadDeleted { id } if *id == th)));
    assert!(ev.iter().any(|(_, e)| matches!(e, HubEvent::ProjectDeleted { id } if *id == pid)));
}

#[rocket::async_test]
async fn profile_picture_upload_and_remove() {
    let t = TestApp::new().await;
    let anna = t.user("anna", false).await;
    let me: Me = ok(&t, &anna, Method::Get, "/api/v1/me", None).await;
    assert!(!me.custom_avatar);
    assert_eq!(me.avatar_url, "/api/v1/users/anna/avatar");

    // Kleinstes gültiges PNG (1×1)
    let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==").unwrap();
    let data_url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png));
    let me: Me = ok(&t, &anna, Method::Put, "/api/v1/me/avatar", Some(json!({"image": data_url}))).await;
    assert!(me.custom_avatar);
    assert!(me.avatar_url.starts_with("/api/v1/users/anna/avatar?v="), "{}", me.avatar_url);
    // Ausliefern: das Bild, mit Version lange zwischengespeichert
    let res = t.client.get(me.avatar_url.clone()).dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    assert_eq!(res.content_type(), Some(ContentType::PNG));
    assert!(res.headers().get_one("Cache-Control").unwrap_or("").contains("immutable"));
    assert_eq!(res.into_bytes().await.unwrap(), png);
    // Ohne Version: jedes Mal nachfragen
    let res = t.client.get("/api/v1/users/anna/avatar").dispatch().await;
    assert_eq!(res.headers().get_one("Cache-Control"), Some("no-cache"));
    // Neues Bild → neue Adresse
    let again: Me = ok(&t, &anna, Method::Put, "/api/v1/me/avatar", Some(json!({"image": base64::engine::general_purpose::STANDARD.encode(&png)}))).await;
    assert_ne!(again.avatar_url, me.avatar_url);

    // Kein Bild bzw. zu gross: abgelehnt
    let (st, _) = call(&t, Some(&anna), Method::Put, "/api/v1/me/avatar", Some(json!({"image": "data:image/svg+xml;base64,PHN2Zy8+"}))).await;
    assert_eq!(st, Status::BadRequest);
    let big = base64::engine::general_purpose::STANDARD.encode(vec![0x89u8; 3 * 1024 * 1024]);
    let (st, _) = call(&t, Some(&anna), Method::Put, "/api/v1/me/avatar", Some(json!({"image": big}))).await;
    assert_eq!(st, Status::BadRequest);
    // Ohne Anmeldung nicht
    let (st, _) = call(&t, None, Method::Put, "/api/v1/me/avatar", Some(json!({"image": data_url}))).await;
    assert_eq!(st, Status::Unauthorized);

    // Entfernen → wieder das Identicon
    let me: Me = ok(&t, &anna, Method::Delete, "/api/v1/me/avatar", None).await;
    assert!(!me.custom_avatar);
    assert_eq!(me.avatar_url, "/api/v1/users/anna/avatar");
    let res = t.client.get("/api/v1/users/anna/avatar").dispatch().await;
    assert_eq!(res.content_type(), Some(ContentType::SVG));
    assert!(!t.dir.path().join("avatars").join(anna.id.to_string()).exists());
}
