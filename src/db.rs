//! Die Datenbank: eine SQLite-Datei im Datenordner (WAL), Migrationen in `crate::migrations`.

use crate::config::Config;
use crate::error::AppError;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{FromRow, SqlitePool};
use std::time::Duration;

pub async fn connect(config: &Config) -> Result<SqlitePool, AppError> {
    std::fs::create_dir_all(&config.data_dir)?;
    let options = SqliteConnectOptions::new()
        .filename(config.database_path())
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(10));
    let pool = SqlitePoolOptions::new().max_connections(8).connect_with(options).await?;
    migrator().run(&pool).await.map_err(|e| AppError::Config(format!("Migration: {e}")))?;
    Ok(pool)
}

/// Die Migrationen aus `crate::migrations` für sqlx.
fn migrator() -> sqlx::migrate::Migrator {
    use sqlx::migrate::{Migration, MigrationType, Migrator};
    use sqlx::SqlSafeStr;
    let list = crate::migrations::MIGRATIONS
        .iter()
        .map(|(v, desc, sql)| Migration::new(*v, (*desc).into(), MigrationType::Simple, sqlx::AssertSqlSafe(*sql).into_sql_str(), false))
        .collect();
    Migrator::with_migrations(list)
}

/// Jetzt als Unix-Sekunden.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Zeile aus `users`.
#[derive(Debug, Clone, Default, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub lower_name: String,
    pub full_name: String,
    pub email: String,
    pub password_hash: String,
    pub is_admin: bool,
    pub is_active: bool,
    pub language: String,
    pub theme: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_login_at: Option<i64>,
    /// Version des eigenen Profilbilds (0 = Identicon)
    #[sqlx(default)]
    pub avatar_at: i64,
}

impl User {
    pub fn to_ref(&self) -> nrzx_shared::UserRef {
        nrzx_shared::UserRef {
            id: self.id,
            username: self.username.clone(),
            full_name: self.full_name.clone(),
            avatar_url: avatar_url(&self.username, self.avatar_at),
            is_org: false,
        }
    }
}

/// Adresse des Profilbilds: eigenes Bild mit Version (`?v=`, neue Adresse bei jedem Wechsel),
/// sonst das Identicon.
pub fn avatar_url(username: &str, avatar_at: i64) -> String {
    if avatar_at > 0 {
        format!("/api/v1/users/{}/avatar?v={avatar_at}", urlencoding::encode(username))
    } else {
        format!("/api/v1/users/{}/avatar", urlencoding::encode(username))
    }
}

pub async fn user_by_id(db: &SqlitePool, id: i64) -> Result<Option<User>, AppError> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE id = ?").bind(id).fetch_optional(db).await?)
}

/// Instanz-Einstellung lesen (`settings`), mit Vorgabe.
pub async fn setting(db: &SqlitePool, key: &str, default: &str) -> Result<String, AppError> {
    let v: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(db)
        .await?;
    Ok(v.map(|(v,)| v).unwrap_or_else(|| default.to_string()))
}

pub async fn set_setting(db: &SqlitePool, key: &str, value: &str) -> Result<(), AppError> {
    sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(db)
        .await?;
    Ok(())
}
