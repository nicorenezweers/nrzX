//! Verbindung zu nrzGit: Adresse und Zugriffstoken je Benutzer (`nrzgit_connections`), Prüfung
//! über `GET <url>/api/v1/me`, Repositories und Besitzer auflisten, Repositories und Pull
//! Requests anlegen. Das Token verlässt den Server nie (nur als Hinweis „nrzg_ab…9f“).
//!
//! Routen: `GET/PUT/DELETE /nrzgit`, `GET /nrzgit/connections`, `POST /nrzgit/verify`,
//! `GET /nrzgit/repos?q=`, `GET /nrzgit/owners`, `GET /nrzgit/candidates`. nrzX verbindet sich
//! nur mit nrzGit (`?kind=` ist für ältere Oberflächen noch erlaubt, aber nur `nrzgit`).

use super::NrzGitAccess;
use crate::auth::CurrentUser;
use crate::db::{self, now};
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::workspace::{NrzGitCandidate, NrzGitConnect, NrzGitOwner, NrzGitRepo, NrzGitStatus};
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::{delete, get, post, put, routes, Route, State};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub fn routes() -> Vec<Route> {
    routes![status, connections, connect, verify, disconnect, repos, owners, candidates]
}

/// Adresse vereinheitlichen: Schema ergänzen, `/` am Ende entfernen.
pub fn normalize_url(url: &str) -> AppResult<String> {
    let mut u = url.trim().trim_end_matches('/').to_string();
    // Auch die MCP- oder API-Adresse annehmen (aus nrzGit → Einstellungen → MCP kopiert)
    for suffix in ["/mcp", "/api/v1", "/api"] {
        if let Some(s) = u.strip_suffix(suffix) {
            u = s.trim_end_matches('/').to_string();
        }
    }
    if u.is_empty() {
        return Err(AppError::bad("Bitte die Adresse von nrzGit angeben."));
    }
    if !u.starts_with("http://") && !u.starts_with("https://") {
        u = format!("http://{u}");
    }
    let rest = u.split_once("://").map(|(_, r)| r).unwrap_or("");
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() || u.chars().any(|c| c.is_whitespace() || c.is_control()) || rest.contains('@') {
        return Err(AppError::bad("Die Adresse von nrzGit ist ungültig."));
    }
    Ok(u)
}

/// Host (mit Port) einer Adresse, z. B. `umbrel.local:8390`.
pub fn host_of(url: &str) -> String {
    url.split_once("://").map(|(_, r)| r).unwrap_or(url).split('/').next().unwrap_or("").to_string()
}

/// Art der Plattform: nrzGit (Vorgabe).
pub const KIND_NRZGIT: &str = "nrzgit";
/// Alle Plattformen (nrzX kennt nur nrzGit).
pub const KINDS: [&str; 1] = [KIND_NRZGIT];

/// Name der Plattform für Meldungen.
pub fn forge_name(_kind: &str) -> &'static str {
    "nrzGit"
}

/// Seite, auf der man ein Token erstellt (nrzGit: Einstellungen → MCP).
pub fn token_url(url: &str, _kind: &str) -> String {
    if url.is_empty() {
        String::new()
    } else {
        format!("{url}/user/settings/mcp")
    }
}

/// Basis der REST-API (ohne `/` am Ende).
pub fn api_base(url: &str, _kind: &str) -> String {
    format!("{}/api/v1", url.trim_end_matches('/'))
}

/// Host der Container-Registry: der Host von nrzGit.
pub fn registry_host(url: &str, _kind: &str) -> String {
    host_of(url)
}

/// Adresse eines Pull Requests im Browser.
pub fn pull_url(public_url: &str, _kind: &str, repo: &str, number: i64) -> String {
    format!("{public_url}/{repo}/pulls/{number}")
}

/// Angabe `kind` einer Anfrage prüfen (`None` = keine Angabe).
pub fn parse_kind(kind: Option<&str>) -> AppResult<Option<&'static str>> {
    match kind.map(str::trim).unwrap_or("") {
        "" => Ok(None),
        KIND_NRZGIT => Ok(Some(KIND_NRZGIT)),
        _ => Err(AppError::bad("nrzX verbindet sich nur mit nrzGit.")),
    }
}

/// Vorgabe-Adresse der Instanz: Einstellung der Verwaltung, sonst Konfiguration.
pub async fn default_url(state: &AppState) -> AppResult<String> {
    let v = db::setting(&state.db, "nrzgit_url", "").await?;
    Ok(if v.trim().is_empty() { state.config.nrzgit_url.trim().trim_end_matches('/').to_string() } else { v })
}

fn hint(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 10 {
        return "…".into();
    }
    let start: String = chars[..7].iter().collect();
    let end: String = chars[chars.len() - 2..].iter().collect();
    format!("{start}…{end}")
}

/// Verständliche Ursache eines Verbindungsfehlers (statt „error sending request“) mit Hinweis.
pub fn describe_error(e: &reqwest::Error) -> String {
    let mut chain = Vec::new();
    let mut cur: Option<&dyn std::error::Error> = Some(e);
    while let Some(err) = cur {
        chain.push(err.to_string().to_lowercase());
        cur = err.source();
    }
    let all = chain.join(" / ");
    let reason = if e.is_timeout() || all.contains("timed out") {
        "keine Antwort (Zeitüberschreitung)"
    } else if all.contains("dns") || all.contains("lookup") || all.contains("name or service not known") || all.contains("nodename nor servname") {
        "der Name lässt sich hier nicht auflösen"
    } else if all.contains("refused") {
        "Verbindung abgelehnt – auf diesem Port läuft nichts"
    } else if all.contains("unreachable") || all.contains("no route") {
        "Netz nicht erreichbar"
    } else if all.contains("certificate") || all.contains("tls") {
        "Problem mit dem HTTPS-Zertifikat"
    } else {
        "Verbindung fehlgeschlagen"
    };
    format!(
        "{reason}. Der Server von nrzX verbindet sich selbst – im Docker-Container ist meist eine interne Adresse nötig, z. B. http://nrz-git_app_1:8380 (nrzGit zeigt alle unter Einstellungen → MCP). „Automatisch suchen“ probiert sie durch."
    )
}

// ------------------------------------------------------------------ Client

/// HTTP-Zugriff auf die API von nrzGit mit dem Token des Benutzers.
pub struct Client {
    url: String,
    token: String,
    kind: String,
    http: reqwest::Client,
}

/// Meldung aus einer Fehlerantwort: `{ "error": { "message" } }` (sonst `{ "message" }`).
fn error_message(text: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return String::new();
    };
    if let Some(m) = v["error"]["message"].as_str() {
        return m.to_string();
    }
    let main = v["message"].as_str().unwrap_or("").to_string();
    let detail = v["errors"].as_array().and_then(|l| l.iter().find_map(|e| e["message"].as_str())).unwrap_or("");
    if detail.is_empty() || main.is_empty() {
        format!("{main}{detail}")
    } else {
        format!("{}: {detail}", main.trim_end_matches('.'))
    }
}

impl Client {
    pub fn new(url: &str, token: &str, kind: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Client { url: url.trim_end_matches('/').to_string(), token: token.to_string(), kind: kind.to_string(), http }
    }

    pub fn of(access: &NrzGitAccess) -> Self {
        Self::new(&access.url, &access.token, &access.kind)
    }

    async fn send<T: DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> AppResult<T> {
        let forge = forge_name(&self.kind);
        let req = req.header("Authorization", format!("token {}", self.token)).header("Accept", "application/json");
        let res = req
            .send()
            .await
            .map_err(|e| AppError::bad(format!("{forge} ist unter {} nicht erreichbar: {}", self.url, describe_error(&e))))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if status.is_success() {
            return serde_json::from_str(&text)
                .map_err(|_| AppError::bad(format!("{forge} hat unerwartet geantwortet – stimmt die Adresse?")));
        }
        if status.as_u16() == 401 {
            return Err(AppError::bad(format!("{forge} hat das Token abgelehnt.")));
        }
        let message = error_message(&text);
        let message = if message.is_empty() { format!("Fehler {}", status.as_u16()) } else { message };
        Err(match status.as_u16() {
            404 => AppError::bad(format!("{forge}: {message}")),
            409 => AppError::conflict(format!("{forge}: {message}")),
            _ => AppError::bad(format!("{forge}: {message}")),
        })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> AppResult<T> {
        self.send(self.http.get(format!("{}{path}", api_base(&self.url, &self.kind)))).await
    }

    pub async fn post<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> AppResult<T> {
        self.send(self.http.post(format!("{}{path}", api_base(&self.url, &self.kind))).json(body)).await
    }
}

/// Teil von `Me` in nrzGit.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteMe {
    pub username: String,
    pub full_name: String,
    pub organizations: Vec<RemoteUser>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteUser {
    pub username: String,
    pub full_name: String,
    pub is_org: bool,
}

/// Teil von `Repo` in nrzGit.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteRepo {
    pub id: i64,
    pub owner: RemoteUser,
    pub name: String,
    pub full_name: String,
    pub description: String,
    pub default_branch: String,
    pub private: bool,
    pub empty: bool,
    pub clone_url: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RemotePaged<T> {
    items: Vec<T>,
    total: i64,
}

/// Neues Repository in nrzGit (`POST /api/v1/repos`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCreateRepo {
    pub owner: String,
    pub name: String,
    pub description: String,
    pub private: Option<bool>,
    pub auto_init: bool,
}

/// Neuer Pull Request (`POST /api/v1/repos/<o>/<r>/pulls`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCreatePull {
    pub base: String,
    pub head: String,
    pub title: String,
    pub body: String,
    pub draft: bool,
}

/// Teil der Antwort auf einen neuen Pull Request (ein Issue).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteIssue {
    pub number: i64,
    pub title: String,
}

impl Client {
    pub async fn me(&self) -> AppResult<RemoteMe> {
        self.get("/me").await
    }

    /// Alle Repositories, an denen der Benutzer mitarbeitet (inkl. Organisationen), höchstens 1000.
    pub async fn repos(&self, q: &str) -> AppResult<Vec<RemoteRepo>> {
        let mut all = Vec::new();
        for page in 1..=10 {
            let p: RemotePaged<RemoteRepo> = self.get(&format!("/me/repos?limit=100&page={page}&q={}", urlencoding::encode(q))).await?;
            let n = p.items.len();
            all.extend(p.items);
            if n < 100 || all.len() as i64 >= p.total {
                break;
            }
        }
        Ok(all)
    }

    pub async fn repo(&self, full_name: &str) -> AppResult<RemoteRepo> {
        let (owner, name) = split_repo(full_name)?;
        self.get(&format!("/repos/{}/{}", urlencoding::encode(owner), urlencoding::encode(name))).await
    }

    pub async fn create_repo(&self, body: &RemoteCreateRepo) -> AppResult<RemoteRepo> {
        self.post("/repos", body).await
    }

    pub async fn create_pull(&self, full_name: &str, body: &RemoteCreatePull) -> AppResult<RemoteIssue> {
        let (owner, name) = split_repo(full_name)?;
        self.post(&format!("/repos/{}/{}/pulls", urlencoding::encode(owner), urlencoding::encode(name)), body).await
    }
}

/// `owner/name` zerlegen und prüfen.
pub fn split_repo(full_name: &str) -> AppResult<(&str, &str)> {
    let ok = |s: &str| {
        !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && !s.starts_with('.')
    };
    match full_name.trim().split_once('/') {
        Some((o, n)) if ok(o) && ok(n) => Ok((o, n)),
        _ => Err(AppError::bad("Repository bitte als „besitzer/name“ angeben.")),
    }
}

/// Adresse zum Klonen: die von nrzGit gemeldete, wenn sie auf derselben Instanz liegt, sonst
/// `<url>/<owner>/<name>.git`. Nie eine andere Adresse – sonst ginge das Token woandershin.
pub fn clone_url(access_url: &str, repo: &RemoteRepo) -> String {
    let reported = repo.clone_url.trim();
    let same_origin = reported.starts_with(&format!("{access_url}/"));
    if same_origin && !reported.contains('@') {
        reported.to_string()
    } else {
        format!("{access_url}/{}.git", repo.full_name)
    }
}

/// Zugang eines Benutzers, nur wenn die Verbindung geprüft ist – sonst eine verständliche Meldung.
/// `kind`: `Some("nrzgit")` oder `None` (gleich).
pub async fn require(state: &AppState, user_id: i64, kind: Option<&str>) -> AppResult<NrzGitAccess> {
    let found = match kind {
        Some(k) => super::forge_access(state, user_id, k).await?,
        None => super::nrzgit_access(state, user_id).await?,
    };
    match (found, kind) {
        (Some(access), _) => Ok(access),
        (None, Some(k)) => Err(AppError::bad(format!("Bitte zuerst {} verbinden (Einstellungen → nrzGit).", forge_name(k)))),
        (None, None) => Err(AppError::bad("Bitte zuerst nrzGit verbinden (Einstellungen → nrzGit).")),
    }
}

// ------------------------------------------------------------------ Routen

type ConnRow = (String, String, String, Option<i64>, String, String, String, bool);

/// Gespeicherte Verbindung des Benutzers zu nrzGit (auch ungeprüft).
async fn rows(state: &AppState, user_id: i64) -> AppResult<Vec<ConnRow>> {
    Ok(sqlx::query_as(
        "SELECT url, username, token, verified_at, last_error, public_url, kind, has_mcp FROM nrzgit_connections WHERE user_id = ? AND kind = 'nrzgit'",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?)
}

fn status_from(row: ConnRow, default_url: &str) -> NrzGitStatus {
    let (url, username, token, verified_at, error, public_url, kind, has_mcp) = row;
    NrzGitStatus {
        connected: verified_at.is_some(),
        token_url: token_url(if public_url.is_empty() { &url } else { &public_url }, &kind),
        has_mcp: has_mcp || kind == KIND_NRZGIT,
        default_url: default_url.to_string(),
        kind,
        public_url,
        url,
        username,
        token_hint: hint(&token),
        verified_at,
        error,
    }
}

/// Leerer Stand einer nicht verbundenen Plattform.
fn status_empty(kind: &str, default_url: &str) -> NrzGitStatus {
    let default_url = default_url.to_string();
    NrzGitStatus { token_url: token_url(&default_url, kind), kind: kind.into(), has_mcp: true, default_url, ..Default::default() }
}

/// Stand der Verbindung zu nrzGit.
async fn status_of(state: &AppState, user_id: i64, kind: Option<&str>) -> AppResult<NrzGitStatus> {
    let default_url = default_url(state).await?;
    let row = rows(state, user_id).await?.into_iter().find(|r| kind.is_none_or(|k| r.6 == k));
    Ok(match row {
        Some(row) => status_from(row, &default_url),
        None => status_empty(kind.unwrap_or(KIND_NRZGIT), &default_url),
    })
}

/// `GET /nrzgit?kind=` – Stand der Verbindung (ohne Token).
#[get("/nrzgit?<kind>")]
async fn status(state: &State<AppState>, user: CurrentUser, kind: Option<&str>) -> AppResult<Json<NrzGitStatus>> {
    Ok(Json(status_of(state, user.id(), parse_kind(kind)?).await?))
}

/// `GET /nrzgit/connections` – die Verbindung zu nrzGit (als Liste), verbunden oder nicht.
#[get("/nrzgit/connections")]
async fn connections(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<Vec<NrzGitStatus>>> {
    let default_url = default_url(state).await?;
    let mut rows = rows(state, user.id()).await?;
    let mut out = Vec::new();
    for kind in KINDS {
        out.push(match rows.iter().position(|r| r.6 == kind) {
            Some(i) => status_from(rows.remove(i), &default_url),
            None => status_empty(kind, &default_url),
        });
    }
    Ok(Json(out))
}

/// `PUT /nrzgit` – Adresse und Token speichern; wird sofort geprüft und nur gespeichert, wenn
/// nrzGit das Token annimmt.
#[put("/nrzgit", data = "<body>")]
async fn connect(state: &State<AppState>, user: CurrentUser, body: Json<NrzGitConnect>) -> AppResult<Json<NrzGitStatus>> {
    parse_kind(Some(&body.kind))?;
    let url = normalize_url(&body.url)?;
    let kind = KIND_NRZGIT;
    let mut token = body.token.trim().to_string();
    if token.is_empty() {
        // Gespeichertes Token nur für dieselbe Adresse weiterverwenden
        let old: Option<(String, String)> = sqlx::query_as("SELECT url, token FROM nrzgit_connections WHERE user_id = ? AND kind = ?")
            .bind(user.id())
            .bind(kind)
            .fetch_optional(&state.db)
            .await?;
        match old {
            Some((old_url, old_token)) if old_url == url => token = old_token,
            _ => return Err(AppError::bad(format!("Bitte ein Zugriffstoken von {} einfügen.", forge_name(kind)))),
        }
    }
    if token.len() > 500 || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AppError::bad("Das Token ist ungültig."));
    }
    let me = Client::new(&url, &token, kind).me().await?;
    if me.username.is_empty() {
        return Err(AppError::bad(format!("{} hat unerwartet geantwortet – stimmt die Adresse?", forge_name(kind))));
    }
    let has_mcp = true;
    let public_url = if body.public_url.trim().is_empty() { String::new() } else { normalize_url(&body.public_url)? };
    let ts = now();
    sqlx::query(
        "INSERT INTO nrzgit_connections (user_id, kind, url, token, username, verified_at, last_error, public_url, has_mcp, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, '', ?, ?, ?, ?) ON CONFLICT (user_id, kind) DO UPDATE SET url = excluded.url, token = excluded.token, \
         username = excluded.username, verified_at = excluded.verified_at, last_error = '', public_url = excluded.public_url, \
         has_mcp = excluded.has_mcp, updated_at = excluded.updated_at",
    )
    .bind(user.id())
    .bind(kind)
    .bind(&url)
    .bind(&token)
    .bind(&me.username)
    .bind(ts)
    .bind(&public_url)
    .bind(has_mcp)
    .bind(ts)
    .bind(ts)
    .execute(&state.db)
    .await?;
    super::gitops::write_docker_config(state, user.id()).await?;
    Ok(Json(status_of(state, user.id(), Some(kind)).await?))
}

/// `POST /nrzgit/verify` – gespeicherte Verbindung erneut prüfen.
#[post("/nrzgit/verify?<kind>")]
async fn verify(state: &State<AppState>, user: CurrentUser, kind: Option<&str>) -> AppResult<Json<NrzGitStatus>> {
    let wanted = parse_kind(kind)?;
    let row = rows(state, user.id()).await?.into_iter().find(|r| wanted.is_none_or(|k| r.6 == k));
    let Some((url, _, token, _, _, _, kind, _)) = row else {
        return Err(AppError::bad(format!("Es ist keine Verbindung zu {} gespeichert.", forge_name(wanted.unwrap_or(KIND_NRZGIT)))));
    };
    match Client::new(&url, &token, &kind).me().await {
        Ok(me) => {
            let has_mcp = true;
            sqlx::query("UPDATE nrzgit_connections SET username = ?, has_mcp = ?, verified_at = ?, last_error = '', updated_at = ? WHERE user_id = ? AND kind = ?")
                .bind(&me.username)
                .bind(has_mcp)
                .bind(now())
                .bind(now())
                .bind(user.id())
                .bind(&kind)
                .execute(&state.db)
                .await?;
        }
        Err(e) => {
            sqlx::query("UPDATE nrzgit_connections SET verified_at = NULL, last_error = ?, updated_at = ? WHERE user_id = ? AND kind = ?")
                .bind(e.public_message())
                .bind(now())
                .bind(user.id())
                .bind(&kind)
                .execute(&state.db)
                .await?;
        }
    }
    super::gitops::write_docker_config(state, user.id()).await?;
    Ok(Json(status_of(state, user.id(), Some(&kind)).await?))
}

/// `DELETE /nrzgit` – Verbindung trennen (Token löschen).
#[delete("/nrzgit?<kind>")]
async fn disconnect(state: &State<AppState>, user: CurrentUser, kind: Option<&str>) -> AppResult<Status> {
    let wanted = parse_kind(kind)?;
    let found = rows(state, user.id()).await?.into_iter().find(|r| wanted.is_none_or(|k| r.6 == k));
    if let Some(row) = found {
        sqlx::query("DELETE FROM nrzgit_connections WHERE user_id = ? AND kind = ?").bind(user.id()).bind(&row.6).execute(&state.db).await?;
    }
    super::gitops::write_docker_config(state, user.id()).await?;
    Ok(Status::NoContent)
}

/// `GET /nrzgit/repos?q=` – Repositories in nrzGit (eigene, Organisationen, Mitarbeit)
/// mit Verweis auf schon übernommene Projekte.
#[get("/nrzgit/repos?<q>&<kind>")]
async fn repos(state: &State<AppState>, user: CurrentUser, q: Option<&str>, kind: Option<&str>) -> AppResult<Json<Vec<NrzGitRepo>>> {
    let access = require(state, user.id(), parse_kind(kind)?).await?;
    let list = Client::of(&access).repos(q.unwrap_or("").trim()).await?;
    let projects: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, lower(repo) FROM projects WHERE user_id = ? AND repo <> '' AND (CASE forge WHEN '' THEN 'nrzgit' ELSE forge END) = ?",
    )
    .bind(user.id())
    .bind(&access.kind)
    .fetch_all(&state.db)
    .await?;
    let mut out: Vec<NrzGitRepo> = list
        .into_iter()
        .map(|r| NrzGitRepo {
            project_id: projects.iter().find(|(_, n)| *n == r.full_name.to_lowercase()).map(|(id, _)| *id),
            web_url: format!("{}/{}", access.public_url, r.full_name),
            forge: access.kind.clone(),
            id: r.id,
            owner: r.owner.username,
            owner_is_org: r.owner.is_org,
            full_name: r.full_name,
            name: r.name,
            description: r.description,
            private: r.private,
            empty: r.empty,
            default_branch: r.default_branch,
            updated_at: r.updated_at,
        })
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    Ok(Json(out))
}

/// `GET /nrzgit/owners?kind=` – ich und meine Organisationen (Besitzer neuer Repositories).
#[get("/nrzgit/owners?<kind>")]
async fn owners(state: &State<AppState>, user: CurrentUser, kind: Option<&str>) -> AppResult<Json<Vec<NrzGitOwner>>> {
    let access = require(state, user.id(), parse_kind(kind)?).await?;
    let me = Client::of(&access).me().await?;
    let mut out = vec![NrzGitOwner { username: me.username.clone(), full_name: me.full_name.clone(), is_org: false }];
    out.extend(me.organizations.into_iter().map(|o| NrzGitOwner { username: o.username, full_name: o.full_name, is_org: true }));
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(normalize_url(" umbrel.local:8390/ ").unwrap(), "http://umbrel.local:8390");
        assert_eq!(normalize_url("https://git.example.com").unwrap(), "https://git.example.com");
        assert!(normalize_url("http://user:pw@host").is_err());
        assert!(normalize_url("").is_err());
        assert_eq!(host_of("http://umbrel.local:8390/x"), "umbrel.local:8390");
        assert_eq!(hint("nrzg_abcdefghijklmnop"), "nrzg_ab…op");
    }

    #[test]
    fn clone_urls_stay_on_the_instance() {
        let mut r = RemoteRepo { full_name: "anna/demo".into(), clone_url: "http://git:8390/anna/demo.git".into(), ..Default::default() };
        assert_eq!(clone_url("http://git:8390", &r), "http://git:8390/anna/demo.git");
        r.clone_url = "/etc/passwd".into();
        assert_eq!(clone_url("http://git:8390", &r), "http://git:8390/anna/demo.git");
        r.clone_url = "http://git:8390.evil.com/x.git".into();
        assert_eq!(clone_url("http://git:8390", &r), "http://git:8390/anna/demo.git");
    }

    #[test]
    fn urls_and_messages() {
        assert_eq!(token_url("http://git:8390", KIND_NRZGIT), "http://git:8390/user/settings/mcp");
        assert_eq!(token_url("", KIND_NRZGIT), "");
        assert_eq!(forge_name(KIND_NRZGIT), "nrzGit");
        assert_eq!(api_base("http://git:8390/", KIND_NRZGIT), "http://git:8390/api/v1");
        assert_eq!(registry_host("http://git:8390", KIND_NRZGIT), "git:8390");
        assert_eq!(pull_url("http://git:8390", KIND_NRZGIT, "anna/demo", 7), "http://git:8390/anna/demo/pulls/7");
        assert_eq!(error_message(r#"{"error":{"message":"Kein Zugriff."}}"#), "Kein Zugriff.");
        assert!(parse_kind(Some("lugit")).is_err());
        assert!(parse_kind(Some("github")).is_err());
        assert_eq!(parse_kind(Some("nrzgit")).unwrap(), Some(KIND_NRZGIT));
    }

    #[test]
    fn repo_names() {
        assert!(split_repo("anna/demo").is_ok());
        assert!(split_repo("anna/../x").is_err());
        assert!(split_repo("anna").is_err());
    }
}

/// Gateway des Docker-Netzes (Linux `/proc/net/route`) – dort ist der Host erreichbar.
fn default_gateway() -> Option<std::net::Ipv4Addr> {
    let table = std::fs::read_to_string("/proc/net/route").ok()?;
    table.lines().skip(1).find_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() > 2 && cols[1] == "00000000" {
            let raw = u32::from_str_radix(cols[2], 16).ok()?;
            Some(std::net::Ipv4Addr::from(raw.to_le_bytes())).filter(|ip| !ip.is_unspecified())
        } else {
            None
        }
    })
}

/// Antwortet unter `url` ein nrzGit? (Art und Version oder verständlicher Fehler)
async fn probe(http: &reqwest::Client, url: &str) -> Result<(&'static str, String), String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Health {
        name: String,
        version: String,
        ok: bool,
    }
    let res = http.get(format!("{url}/api/v1/health")).send().await.map_err(|e| {
        let full = describe_error(&e);
        full.split(". ").next().unwrap_or(&full).to_string()
    })?;
    if let Ok(h) = res.json::<Health>().await {
        if h.name == "nrzGit" {
            return Ok((KIND_NRZGIT, h.version));
        }
        if !h.name.is_empty() {
            return Err(format!("antwortet als {}", h.name));
        }
    }
    Err("antwortet, ist aber kein nrzGit".to_string())
}

/// `GET /nrzgit/candidates?url=` – übliche Adressen von nrzGit vom Server aus durchprobieren
/// (eingegebene Adresse, Vorgabe, Docker-Name auf Umbrel, Gateway des Docker-Netzes, Host).
#[get("/nrzgit/candidates?<url>")]
async fn candidates(state: &State<AppState>, user: CurrentUser, url: Option<String>) -> AppResult<Json<Vec<NrzGitCandidate>>> {
    let mut list: Vec<(String, String)> = Vec::new();
    let mut add = |u: String, label: &str| {
        if let Ok(u) = normalize_url(&u) {
            if !list.iter().any(|(x, _)| *x == u) {
                list.push((u, label.to_string()));
            }
        }
    };
    if let Some(u) = url.filter(|u| !u.trim().is_empty()) {
        add(u, "Eingegeben");
    }
    for a in super::forge_accesses(state, user.id()).await? {
        add(a.url, "Gespeichert");
    }
    let default = default_url(state).await?;
    if !default.is_empty() {
        add(default, "Vorgabe der Instanz");
    }
    add("http://nrz-git_app_1:8380".into(), "Docker-Name auf Umbrel (empfohlen)");
    if let Some(gw) = default_gateway() {
        add(format!("http://{gw}:8390"), "Host über das Docker-Gateway (App-Proxy)");
        add(format!("http://{gw}:8380"), "Host über das Docker-Gateway");
    }
    add("http://host.docker.internal:8390".into(), "Host (Docker Desktop)");
    add("http://umbrel.local:8390".into(), "umbrel.local");
    add("http://localhost:8390".into(), "Gleicher Rechner");
    add("http://localhost:8380".into(), "Gleicher Rechner (direkt)");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .connect_timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default();
    let checks = list.iter().map(|(u, _)| probe(&http, u));
    let results = futures::future::join_all(checks).await;
    let mut out: Vec<NrzGitCandidate> = list
        .into_iter()
        .zip(results)
        .map(|((url, label), r)| match r {
            Ok((kind, version)) => NrzGitCandidate { url, label, reachable: true, version, error: String::new(), kind: kind.into() },
            Err(error) => NrzGitCandidate { url, label, reachable: false, version: String::new(), error, kind: String::new() },
        })
        .collect();
    // Erreichbare zuerst, sonst Reihenfolge wie oben
    out.sort_by_key(|c| !c.reachable);
    Ok(Json(out))
}
