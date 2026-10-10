//! Saved connection profiles.
//!
//! Profiles live in the OS app-config directory as `connections.json`.
//! Passwords are NEVER written to that file: they are stored in the OS
//! keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service).

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};

const KEYRING_SERVICE: &str = "dev.tabla.desktop";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    Postgres,
    Mysql,
    Sqlite,
}

impl Driver {
    pub fn default_port(self) -> u16 {
        match self {
            Driver::Postgres => 5432,
            Driver::Mysql => 3306,
            Driver::Sqlite => 0,
        }
    }

    pub fn quote_ident(self, s: &str) -> String {
        match self {
            Driver::Mysql => format!("`{}`", s.replace('`', "``")),
            _ => format!("\"{}\"", s.replace('"', "\"\"")),
        }
    }

    pub fn quote_literal(self, s: &str) -> String {
        let mut v = s.replace('\'', "''");
        if self == Driver::Mysql {
            v = v.replace('\\', "\\\\");
        }
        format!("'{v}'")
    }

    pub fn qualified(self, schema: &str, table: &str) -> String {
        if schema.is_empty() {
            self.quote_ident(table)
        } else {
            format!("{}.{}", self.quote_ident(schema), self.quote_ident(table))
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyFull,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub driver: Driver,
    /// UI preset id (e.g. "supabase", "cockroach"); purely informational.
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    /// Only ever travels UI -> backend. `None` keeps the stored keychain
    /// value, `Some("")` clears it. Never serialized back out.
    #[serde(default, skip_serializing)]
    pub password: Option<String>,
    /// Database name, or the file path for SQLite.
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub ssl_mode: SslMode,
    #[serde(default)]
    pub color: String,
    /// "local" | "development" | "staging" | "production"
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub read_only: bool,
}

impl ConnectionConfig {
    pub fn port_or_default(&self) -> u16 {
        if self.port == 0 { self.driver.default_port() } else { self.port }
    }
}

fn store_path(app: &AppHandle) -> AppResult<PathBuf> {
    let dir = app.path().app_config_dir()?;
    fs::create_dir_all(&dir)?;
    Ok(dir.join("connections.json"))
}

pub fn load(app: &AppHandle) -> AppResult<Vec<ConnectionConfig>> {
    let path = store_path(app)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn write(app: &AppHandle, list: &[ConnectionConfig]) -> AppResult<()> {
    let path = store_path(app)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(list)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(tmp, path)?;
    Ok(())
}

pub fn upsert(app: &AppHandle, mut cfg: ConnectionConfig) -> AppResult<ConnectionConfig> {
    if cfg.name.trim().is_empty() {
        return Err(AppError::msg("Connection name is required"));
    }
    match cfg.password.take() {
        Some(p) if p.is_empty() => delete_password(&cfg.id)?,
        Some(p) => entry(&cfg.id)?.set_password(&p)?,
        None => {}
    }
    let mut list = load(app)?;
    match list.iter_mut().find(|c| c.id == cfg.id) {
        Some(existing) => *existing = cfg.clone(),
        None => list.push(cfg.clone()),
    }
    write(app, &list)?;
    Ok(cfg)
}

pub fn remove(app: &AppHandle, id: &str) -> AppResult<()> {
    let mut list = load(app)?;
    list.retain(|c| c.id != id);
    write(app, &list)?;
    delete_password(id)
}

fn entry(id: &str) -> AppResult<keyring::Entry> {
    Ok(keyring::Entry::new(KEYRING_SERVICE, id)?)
}

fn delete_password(id: &str) -> AppResult<()> {
    match entry(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Password from the request if provided, otherwise from the keychain.
pub fn resolve_password(cfg: &ConnectionConfig) -> AppResult<Option<String>> {
    if let Some(p) = &cfg.password {
        return Ok(Some(p.clone()));
    }
    match entry(&cfg.id)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
