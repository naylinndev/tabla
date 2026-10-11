use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{}", db_message(.0))]
    Db(#[from] sqlx::Error),
    #[error("Keychain error: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Tauri(#[from] tauri::Error),
    #[error("{}", ms_message(.0))]
    Mssql(#[from] tiberius::error::Error),
    #[error("{0}")]
    Msg(String),
}

impl AppError {
    pub fn msg(s: impl Into<String>) -> Self {
        AppError::Msg(s.into())
    }
}

// Commands return errors to the UI as plain strings.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// The server's own message plus DETAIL/HINT, without sqlx's prefix or the
/// Postgres source-file location ("at line 1452") that confuses users.
pub fn db_message(e: &sqlx::Error) -> String {
    let Some(db) = e.as_database_error() else { return e.to_string() };
    let mut out = db.message().to_string();
    if let Some(pg) = db.try_downcast_ref::<sqlx::postgres::PgDatabaseError>() {
        if let Some(d) = pg.detail() {
            out.push_str(&format!("\nDetail: {d}"));
        }
        if let Some(h) = pg.hint() {
            out.push_str(&format!("\nHint: {h}"));
        }
    }
    out
}

/// SQL Server's own message without tiberius' "Token error: '…' on server …" wrapper.
pub fn ms_message(e: &tiberius::error::Error) -> String {
    match e {
        tiberius::error::Error::Server(t) => t.message().to_string(),
        tiberius::error::Error::Io { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

/// Returned by SQL-only code paths if they are ever reached for a Redis session.
pub fn not_sql() -> AppError {
    AppError::msg("This isn't available for Redis connections")
}
