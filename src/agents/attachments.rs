//! Anhänge aus dem Composer (Bilder einfügen/ziehen, Dateien): `POST /api/v1/attachments`
//! (multipart, Feld `file`) legt sie unter `<data>/attachments/<uuid>` ab; ausgeliefert werden
//! sie unter `/attachments/<uuid>` nur an ihren Besitzer. Beim Lauf bekommt der Agent sie als
//! Pfade im Prompt bzw. Claude Code Bilder als Bild-Eingabe.

use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::Attachment;
use rocket::form::Form;
use rocket::fs::TempFile;
use rocket::http::{ContentType, Header};
use rocket::response::{self, Responder, Response};
use rocket::serde::json::Json;
use rocket::{get, post, routes, FromForm, Request, Route, State};
use serde::Serialize;
use std::path::PathBuf;

pub fn api_routes() -> Vec<Route> {
    routes![upload]
}

pub fn routes() -> Vec<Route> {
    routes![serve]
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AttachmentRow {
    pub uuid: String,
    pub user_id: i64,
    pub name: String,
    pub size: i64,
    pub content_type: String,
    pub download_count: i64,
    pub created_at: i64,
}

impl AttachmentRow {
    pub fn to_json(&self) -> Attachment {
        Attachment {
            uuid: self.uuid.clone(),
            name: self.name.clone(),
            size: self.size,
            content_type: self.content_type.clone(),
            download_count: self.download_count,
            url: format!("/attachments/{}", self.uuid),
            created_at: self.created_at,
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self.content_type.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp")
    }
}

/// Pfad der Datei (nur gültige UUIDs).
pub fn file_path(state: &AppState, uuid: &str) -> AppResult<PathBuf> {
    let parsed = uuid::Uuid::parse_str(uuid).map_err(|_| AppError::NotFound)?;
    Ok(state.config.attachments_dir().join(parsed.hyphenated().to_string()))
}

/// Anhänge des Benutzers in der gegebenen Reihenfolge (unbekannte: 400).
pub async fn load(state: &AppState, user_id: i64, uuids: &[String]) -> AppResult<Vec<AttachmentRow>> {
    let mut out = Vec::new();
    for u in uuids.iter().take(20) {
        let row: Option<AttachmentRow> = sqlx::query_as("SELECT * FROM attachments WHERE uuid = ? AND user_id = ?")
            .bind(u)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
        out.push(row.ok_or_else(|| AppError::bad("Unbekannter Anhang."))?);
    }
    Ok(out)
}

#[derive(FromForm)]
pub struct Upload<'r> {
    #[field(name = "file")]
    pub files: Vec<TempFile<'r>>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum Uploaded {
    One(Attachment),
    Many(Vec<Attachment>),
}

fn clean_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last.chars().filter(|c| !c.is_control()).collect::<String>().trim().trim_start_matches('.').chars().take(200).collect();
    if cleaned.is_empty() {
        "datei".into()
    } else {
        cleaned
    }
}

fn detect(name: &str, head: &[u8]) -> String {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let by_ext = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" | "csv" | "rs" | "toml" | "yaml" | "yml" | "json" | "js" | "ts" | "py" | "go" | "html" | "css" | "sql" | "sh" => "text/plain",
        _ => "",
    };
    if !by_ext.is_empty() {
        return by_ext.into();
    }
    for (sig, t) in [(&b"\x89PNG\r\n\x1a\n"[..], "image/png"), (b"\xff\xd8\xff", "image/jpeg"), (b"GIF8", "image/gif"), (b"%PDF-", "application/pdf")] {
        if head.starts_with(sig) {
            return t.into();
        }
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return "image/webp".into();
    }
    "application/octet-stream".into()
}

/// `POST /attachments` – eine oder mehrere Dateien (Feld `file`).
#[post("/attachments", data = "<form>")]
async fn upload(state: &State<AppState>, user: CurrentUser, mut form: Form<Upload<'_>>) -> AppResult<Json<Uploaded>> {
    let mut out = Vec::new();
    for f in form.files.iter_mut() {
        if f.len() == 0 {
            continue;
        }
        let name = clean_name(f.raw_name().map(|n| n.dangerous_unsafe_unsanitized_raw().as_str()).unwrap_or("datei"));
        let uuid = uuid::Uuid::new_v4().hyphenated().to_string();
        let path = file_path(state, &uuid)?;
        f.move_copy_to(&path).await?;
        let mut head = vec![0u8; 16];
        if let Ok(mut file) = tokio::fs::File::open(&path).await {
            use tokio::io::AsyncReadExt;
            let n = file.read(&mut head).await.unwrap_or(0);
            head.truncate(n);
        }
        let size = tokio::fs::metadata(&path).await.map(|m| m.len() as i64).unwrap_or(0);
        let ct = detect(&name, &head);
        let ts = now();
        sqlx::query("INSERT INTO attachments (uuid, user_id, name, size, content_type, created_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(&uuid)
            .bind(user.id())
            .bind(&name)
            .bind(size)
            .bind(&ct)
            .bind(ts)
            .execute(&state.db)
            .await?;
        out.push(AttachmentRow { uuid, user_id: user.id(), name, size, content_type: ct, download_count: 0, created_at: ts }.to_json());
    }
    if out.is_empty() {
        return Err(AppError::bad("Keine Datei erhalten."));
    }
    Ok(Json(if out.len() == 1 { Uploaded::One(out.remove(0)) } else { Uploaded::Many(out) }))
}

pub struct FileResponse {
    body: Vec<u8>,
    content_type: String,
    name: String,
    inline: bool,
}

impl<'r> Responder<'r, 'static> for FileResponse {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        let disposition = if self.inline { "inline" } else { "attachment" };
        let ct = ContentType::parse_flexible(&self.content_type).unwrap_or(ContentType::Binary);
        Response::build()
            .header(ct)
            .header(Header::new("Content-Disposition", format!("{disposition}; filename*=UTF-8''{}", urlencoding::encode(&self.name))))
            .header(Header::new("Cache-Control", "private, max-age=86400"))
            .sized_body(self.body.len(), std::io::Cursor::new(self.body))
            .ok()
    }
}

/// `GET /attachments/<uuid>` – nur für den Besitzer; Bilder inline, alles andere als Download.
#[get("/attachments/<uuid>")]
async fn serve(state: &State<AppState>, user: CurrentUser, uuid: &str) -> AppResult<FileResponse> {
    let row: AttachmentRow = sqlx::query_as("SELECT * FROM attachments WHERE uuid = ? AND user_id = ?")
        .bind(uuid)
        .bind(user.id())
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    let body = tokio::fs::read(file_path(state, uuid)?).await.map_err(|_| AppError::NotFound)?;
    sqlx::query("UPDATE attachments SET download_count = download_count + 1 WHERE uuid = ?").bind(uuid).execute(&state.db).await?;
    let inline = row.is_image() || row.content_type == "application/pdf" || row.content_type == "text/plain";
    let content_type = if row.content_type == "text/plain" { "text/plain; charset=utf-8".to_string() } else { row.content_type.clone() };
    Ok(FileResponse { body, content_type, name: row.name, inline })
}
