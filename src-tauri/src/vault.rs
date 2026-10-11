//! Encryption at rest for everything Tabla keeps on disk: connection
//! profiles, query history, open query tabs, saved queries and filters.
//!
//! A random 256-bit key lives in the OS keychain (macOS Keychain, Windows
//! Credential Manager, Secret Service on Linux); files are AES-256-GCM
//! encrypted with it and written owner-only. Other programs then can't read
//! Tabla's data by opening its files: they would also need the key, which on
//! macOS the keychain only hands to Tabla itself.
//!
//! When no keychain is available (some minimal Linux setups) files stay
//! readable JSON, still owner-only. A file that is encrypted but can't be
//! decrypted is never overwritten, so a locked or denied keychain can't
//! destroy data.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};

use crate::error::{AppError, AppResult};

const MAGIC: &[u8] = b"TABLA-ENC1\n";
const KEY_ACCOUNT: &str = "tabla:data-key";

/// The key, once read or created in this process.
static KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);
/// Set when this system has no usable keychain at all.
static NO_KEYCHAIN: AtomicBool = AtomicBool::new(false);

fn keychain_entry() -> AppResult<keyring::Entry> {
    Ok(keyring::Entry::new(crate::config::APP_ID, KEY_ACCOUNT)?)
}

fn locked() -> AppError {
    AppError::msg(
        "Tabla's data is encrypted with a key in your system keychain, and the keychain didn't allow Tabla to read it. \
         Unlock your keychain (or allow Tabla when asked) and try again.",
    )
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Reads the key, creating it on first use. `create` is false when only
/// decrypting, so a missing key is reported instead of replaced.
fn key(create: bool) -> AppResult<Option<[u8; 32]>> {
    if std::env::var_os("TABLA_PLAIN_DATA").is_some() {
        // Tests and the dev bridge, which may run without a keychain.
        return Ok(None);
    }
    let mut cached = KEY.lock().unwrap();
    if let Some(k) = *cached {
        return Ok(Some(k));
    }
    if NO_KEYCHAIN.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let Ok(entry) = keychain_entry() else {
        NO_KEYCHAIN.store(true, Ordering::Relaxed);
        return Ok(None);
    };
    match entry.get_password() {
        Ok(v) => {
            let k = unhex(&v).ok_or_else(|| AppError::msg("Tabla's data key in the keychain is damaged"))?;
            *cached = Some(k);
            Ok(Some(k))
        }
        Err(keyring::Error::NoEntry) if !create => Ok(None),
        Err(keyring::Error::NoEntry) => {
            let mut k = [0u8; 32];
            SystemRandom::new().fill(&mut k).map_err(|_| AppError::msg("No secure random source"))?;
            if entry.set_password(&hex(&k)).is_err() {
                // Can't store a key: stay unencrypted rather than lose data next run.
                NO_KEYCHAIN.store(true, Ordering::Relaxed);
                return Ok(None);
            }
            *cached = Some(k);
            Ok(Some(k))
        }
        // Locked or denied (or no keychain service): reading encrypted data
        // must fail; writing falls back to plain, which `write` refuses
        // whenever that would replace encrypted data.
        Err(keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_)) if !create => Err(locked()),
        Err(keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn cipher(k: &[u8; 32]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, k).expect("32-byte key"))
}

/// Encrypts `plain` when a keychain key is available.
pub fn seal(plain: &[u8]) -> AppResult<Vec<u8>> {
    let Some(k) = key(true)? else { return Ok(plain.to_vec()) };
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).map_err(|_| AppError::msg("No secure random source"))?;
    let mut buf = plain.to_vec();
    cipher(&k).seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(MAGIC), &mut buf).map_err(|_| AppError::msg("Encryption failed"))?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + buf.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&buf);
    Ok(out)
}

pub fn is_sealed(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

/// Decrypts data written by `seal`; plain (older or keyless) data passes through.
pub fn open(data: &[u8]) -> AppResult<Vec<u8>> {
    let Some(rest) = data.strip_prefix(MAGIC) else { return Ok(data.to_vec()) };
    let k = key(false)?.ok_or_else(locked)?;
    if rest.len() < NONCE_LEN {
        return Err(AppError::msg("Tabla's data file is damaged"));
    }
    let (nonce, body) = rest.split_at(NONCE_LEN);
    let mut buf = body.to_vec();
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| AppError::msg("Tabla's data file is damaged"))?;
    let plain = cipher(&k).open_in_place(nonce, Aad::from(MAGIC), &mut buf).map_err(|_| AppError::msg("Tabla's data file is damaged or was encrypted with another key"))?;
    Ok(plain.to_vec())
}

/// Creates a directory only the current user can enter.
pub fn private_dir(dir: &Path) -> AppResult<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes a file atomically, readable and writable by the current user only
/// from the moment it exists. (On Windows, files under the user profile are
/// already private to that user.)
pub fn write_private(path: &Path, bytes: &[u8]) -> AppResult<()> {
    if let Some(dir) = path.parent() {
        private_dir(dir)?;
    }
    let tmp: PathBuf = path.with_extension(format!("{}.tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
    let mut o = fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(tmp, path)?;
    Ok(())
}

/// Reads and decrypts a file; `None` if it doesn't exist.
pub fn read(path: &Path) -> AppResult<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(data) => Ok(Some(open(&data)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Encrypts and writes a file. Refuses to replace an encrypted file with
/// plain text (when the keychain went away), so nothing is silently downgraded.
pub fn write(path: &Path, plain: &[u8]) -> AppResult<()> {
    let sealed = seal(plain)?;
    if !is_sealed(&sealed) && fs::read(path).map(|d| is_sealed(&d)).unwrap_or(false) {
        return Err(locked());
    }
    write_private(path, &sealed)
}

/// The app's own state (query history, open query tabs, saved queries,
/// filters, layout) as one JSON object, kept in `state.bin`.
pub fn load_state() -> AppResult<serde_json::Map<String, serde_json::Value>> {
    let path = crate::config::config_dir()?.join("state.bin");
    match read(&path)? {
        Some(raw) => Ok(serde_json::from_slice(&raw).unwrap_or_default()),
        None => Ok(Default::default()),
    }
}

pub fn save_state(state: &serde_json::Map<String, serde_json::Value>) -> AppResult<()> {
    write(&crate::config::config_dir()?.join("state.bin"), &serde_json::to_vec(state)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_and_opens_with_a_key() {
        let k = [7u8; 32];
        *KEY.lock().unwrap() = Some(k);
        let sealed = seal(b"{\"q\":\"SELECT secret\"}").unwrap();
        assert!(is_sealed(&sealed));
        assert!(!sealed.windows(6).any(|w| w == b"SELECT"), "plain text leaked");
        assert_eq!(open(&sealed).unwrap(), b"{\"q\":\"SELECT secret\"}");
        // Tampering is detected.
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open(&bad).is_err());
        // Plain JSON from older versions still reads.
        assert_eq!(open(b"[]").unwrap(), b"[]");
    }

    #[test]
    #[cfg(unix)]
    fn writes_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("tabla-vault-{}", std::process::id()));
        let path = dir.join("x.json");
        write_private(&path, b"{}").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let _ = fs::remove_dir_all(dir);
    }
}
