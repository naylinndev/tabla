//! Saved connection profiles.
//!
//! Profiles live in the OS app-config directory as `connections.json`.
//! Passwords are NEVER written to that file: they are stored in the OS
//! keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service).

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Matches the Tauri bundle identifier, so the GUI and `tabla mcp` share
/// the same config directory and keychain entries.
pub const APP_ID: &str = "dev.tabla.desktop";
const KEYRING_SERVICE: &str = APP_ID;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    Postgres,
    Mysql,
    Sqlite,
    Redis,
    Mssql,
}

impl Driver {
    pub fn default_port(self) -> u16 {
        match self {
            Driver::Postgres => 5432,
            Driver::Mysql => 3306,
            Driver::Sqlite => 0,
            Driver::Redis => 6379,
            Driver::Mssql => 1433,
        }
    }

    pub fn quote_ident(self, s: &str) -> String {
        match self {
            Driver::Mysql => format!("`{}`", s.replace('`', "``")),
            Driver::Mssql => format!("[{}]", s.replace(']', "]]")),
            _ => format!("\"{}\"", s.replace('"', "\"\"")),
        }
    }

    pub fn quote_literal(self, s: &str) -> String {
        let mut v = s.replace('\'', "''");
        if self == Driver::Mysql {
            v = v.replace('\\', "\\\\");
        }
        // N'' keeps non-ASCII text intact when compared with nvarchar columns.
        if self == Driver::Mssql { format!("N'{v}'") } else { format!("'{v}'") }
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshAuth {
    Password,
    #[default]
    Key,
    Agent,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SshConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    /// Private key file, e.g. ~/.ssh/id_ed25519 (the key itself is never copied).
    pub key_path: String,
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
    /// TLS certificate files (paths only): CA, client certificate, client key.
    #[serde(default)]
    pub ssl_ca: String,
    #[serde(default)]
    pub ssl_cert: String,
    #[serde(default)]
    pub ssl_key: String,
    #[serde(default)]
    pub ssh: SshConfig,
    /// SSH password or key passphrase. Same rules as `password`: UI -> backend
    /// only, stored in the keychain, never serialized.
    #[serde(default, skip_serializing)]
    pub ssh_secret: Option<String>,
    #[serde(default)]
    pub color: String,
    /// "local" | "development" | "staging" | "production"
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub read_only: bool,
    /// Expose this connection (read-only) to AI assistants via `tabla mcp`.
    #[serde(default)]
    pub ai_access: bool,
}

impl ConnectionConfig {
    pub fn port_or_default(&self) -> u16 {
        if self.port == 0 { self.driver.default_port() } else { self.port }
    }
}

/// Same location Tauri uses for `app_config_dir()`.
/// `TABLA_CONFIG_DIR` overrides it (used by tests and the dev bridge so they
/// never touch the user's real profiles).
pub fn config_dir() -> AppResult<PathBuf> {
    if let Some(dir) = std::env::var_os("TABLA_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(dirs::config_dir().ok_or_else(|| AppError::msg("No config directory on this system"))?.join(APP_ID))
}

fn store_path() -> AppResult<PathBuf> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    Ok(dir.join("connections.json"))
}

pub fn load() -> AppResult<Vec<ConnectionConfig>> {
    let path = store_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn write(list: &[ConnectionConfig]) -> AppResult<()> {
    let path = store_path()?;
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

pub fn upsert(mut cfg: ConnectionConfig) -> AppResult<ConnectionConfig> {
    if cfg.name.trim().is_empty() {
        return Err(AppError::msg("Connection name is required"));
    }
    match cfg.password.take() {
        Some(p) if p.is_empty() => delete_secret(&cfg.id)?,
        Some(p) => entry(&cfg.id)?.set_password(&p)?,
        None => {}
    }
    let ssh_account = ssh_account(&cfg.id);
    match cfg.ssh_secret.take() {
        Some(p) if p.is_empty() => delete_secret(&ssh_account)?,
        Some(p) => entry(&ssh_account)?.set_password(&p)?,
        None => {}
    }
    let mut list = load()?;
    match list.iter_mut().find(|c| c.id == cfg.id) {
        Some(existing) => *existing = cfg.clone(),
        None => list.push(cfg.clone()),
    }
    write(&list)?;
    Ok(cfg)
}

pub fn remove(id: &str) -> AppResult<()> {
    let mut list = load()?;
    list.retain(|c| c.id != id);
    write(&list)?;
    delete_secret(id)?;
    delete_secret(&ssh_account(id))
}

fn ssh_account(id: &str) -> String {
    format!("{id}:ssh")
}

fn entry(id: &str) -> AppResult<keyring::Entry> {
    Ok(keyring::Entry::new(KEYRING_SERVICE, id)?)
}

fn delete_secret(account: &str) -> AppResult<()> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn stored(account: &str) -> AppResult<Option<String>> {
    match entry(account)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        // Typically macOS denying access after the app binary changed (unsigned or
        // updated builds), or a locked keyring on Linux.
        Err(keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_)) => Err(AppError::msg(
            "Your system keychain didn't allow Tabla to read the saved password. Edit this connection, \
             re-enter the password (or SSH passphrase) and save. If your keychain is locked, unlock it first.",
        )),
        Err(e) => Err(e.into()),
    }
}

/// Password from the request if provided, otherwise from the keychain.
pub fn resolve_password(cfg: &ConnectionConfig) -> AppResult<Option<String>> {
    match &cfg.password {
        Some(p) => Ok(Some(p.clone())),
        None => stored(&cfg.id),
    }
}

/// SSH password / key passphrase from the request, otherwise from the keychain.
pub fn resolve_ssh_secret(cfg: &ConnectionConfig) -> AppResult<Option<String>> {
    match &cfg.ssh_secret {
        Some(p) => Ok(Some(p.clone())),
        None => stored(&ssh_account(&cfg.id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Touches the real OS keychain, so it only runs on request:
    /// `cargo test keychain_roundtrip -- --ignored`
    #[test]
    #[ignore]
    fn keychain_roundtrip() {
        let account = "tabla-selftest";
        entry(account).unwrap().set_password("roundtrip-value").unwrap();
        assert_eq!(stored(account).unwrap().as_deref(), Some("roundtrip-value"));
        delete_secret(account).unwrap();
        assert_eq!(stored(account).unwrap(), None);
    }
}
