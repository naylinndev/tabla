//! Update check against GitHub Releases. Sends a plain GET with a generic
//! user agent: no identifiers, no telemetry. The UI rate-limits it and the
//! user can turn it off.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub const REPO: &str = "naylinndev/tabla";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// The in-app updater can install this release (signed release build, supported install).
    pub can_install: bool,
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub url: String,
    pub notes: String,
    pub published: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Compares dotted versions numerically ("0.10.0" > "0.9.3"); ignores a leading "v".
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v').split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect()
    };
    parse(latest) > parse(current)
}

pub fn check() -> AppResult<UpdateInfo> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let mut res = ureq::get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("User-Agent", "Tabla-update-check")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => AppError::msg("No releases have been published yet."),
            other => AppError::msg(format!("Couldn't check for updates: {other}")),
        })?;
    let text = res.body_mut().read_to_string().map_err(|e| AppError::msg(e.to_string()))?;
    let r: Release = serde_json::from_str(&text)?;
    if r.draft || r.prerelease {
        return Err(AppError::msg("No stable release yet."));
    }
    let latest = r.tag_name.trim_start_matches('v').to_string();
    Ok(UpdateInfo {
        can_install: false,
        available: is_newer(&latest, &current),
        current,
        latest,
        url: r.html_url,
        notes: r.body.chars().take(1200).collect(),
        published: r.published_at,
    })
}

/// Signed update manifest that release builds attach to each GitHub release.
pub const MANIFEST_URL: &str = "https://github.com/naylinndev/tabla/releases/latest/download/latest.json";

/// Whether this build was configured with an updater public key, which only
/// release builds signed with the project's updater key are.
pub fn updater_configured(config: &tauri::Config) -> bool {
    config.plugins.0.get("updater").and_then(|u| u.get("pubkey")).and_then(|k| k.as_str()).is_some_and(|k| !k.trim().is_empty())
}

/// Whether this copy of Tabla can replace itself. Linux packages (.deb) are
/// updated by the package manager; only the AppImage updates in place.
pub fn install_supported() -> bool {
    if cfg!(target_os = "linux") { std::env::var_os("APPIMAGE").is_some() } else { true }
}

/// Only the project's own pages may be opened from the app.
pub fn is_allowed_url(url: &str) -> bool {
    url.starts_with(&format!("https://github.com/{REPO}/")) || url == format!("https://github.com/{REPO}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn updater_needs_a_public_key() {
        let cfg = |plugins: serde_json::Value| -> tauri::Config {
            serde_json::from_value(serde_json::json!({ "identifier": "dev.tabla.test", "plugins": plugins })).unwrap()
        };
        assert!(!updater_configured(&cfg(serde_json::json!({}))));
        assert!(!updater_configured(&cfg(serde_json::json!({ "updater": { "pubkey": " " } }))));
        assert!(updater_configured(&cfg(serde_json::json!({ "updater": { "pubkey": "dW50cnVzdGVk", "endpoints": [MANIFEST_URL] } }))));
        // The app's own config ships without one; release builds add it.
        let shipped: tauri::Config = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert!(!updater_configured(&shipped));
    }

    #[test]
    fn only_project_urls() {
        assert!(is_allowed_url("https://github.com/naylinndev/tabla/releases/tag/v0.2.0"));
        assert!(!is_allowed_url("https://github.com/naylinndev/tabla.evil.com/x"));
        assert!(!is_allowed_url("https://evil.example/https://github.com/naylinndev/tabla/"));
    }
}
