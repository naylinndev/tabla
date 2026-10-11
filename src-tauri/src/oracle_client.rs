//! Oracle Instant Client, which Oracle connections need at runtime.
//!
//! Tabla first uses an Instant Client it downloaded itself (into its data
//! directory), then one installed on the system (PATH / LD_LIBRARY_PATH /
//! ORACLE_HOME, or the default search on macOS). The download comes straight
//! from oracle.com, only when the user asks for it.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientStatus {
    /// The client library loaded.
    pub ready: bool,
    pub version: Option<String>,
    /// Why it didn't load, if it didn't.
    pub error: Option<String>,
    /// Tabla can download Instant Client for this system.
    pub can_install: bool,
    /// The client was installed by Tabla but needs a restart to load.
    pub restart_needed: bool,
    pub license_url: &'static str,
}

pub const LICENSE_URL: &str = "https://www.oracle.com/downloads/licenses/instant-client-lic.html";

/// Basic Light package for this system, if Oracle publishes one.
fn download_url() -> Option<String> {
    let path = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux/instantclient/instantclient-basiclite-linuxx64.zip",
        ("linux", "aarch64") => "linux/instantclient/instantclient-basiclite-linux-arm64.zip",
        ("windows", "x86_64") => "nt/instantclient/instantclient-basiclite-windows.zip",
        ("macos", "aarch64") => "mac/instantclient/instantclient-basiclite-macos-arm64.dmg",
        ("macos", "x86_64") => "mac/instantclient/instantclient-basiclite-macos.dmg",
        _ => return None,
    };
    Some(format!("https://download.oracle.com/otn_software/{path}"))
}

/// Where Tabla keeps its own Instant Client.
pub fn managed_root() -> AppResult<PathBuf> {
    if let Some(dir) = std::env::var_os("TABLA_CONFIG_DIR") {
        return Ok(PathBuf::from(dir).join("oracle-client"));
    }
    Ok(dirs::data_local_dir().ok_or_else(|| AppError::msg("No data directory on this system"))?.join(crate::config::APP_ID).join("oracle-client"))
}

const LIB_NAME: &str = if cfg!(target_os = "windows") {
    "oci.dll"
} else if cfg!(target_os = "macos") {
    "libclntsh.dylib"
} else {
    "libclntsh.so"
};

/// The directory holding the client library inside Tabla's own install, if any.
pub fn managed_lib_dir() -> Option<PathBuf> {
    let root = managed_root().ok()?;
    if root.join(LIB_NAME).exists() {
        return Some(root);
    }
    // Archives unpack into instantclient_<version>/.
    std::fs::read_dir(&root).ok()?.flatten().map(|e| e.path()).filter(|p| p.join(LIB_NAME).exists()).max()
}

static INIT_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Loads the client library. Safe to call again after a failure (for example
/// once the user has installed Instant Client).
pub fn init() -> AppResult<()> {
    if oracle::InitParams::is_initialized() {
        return Ok(());
    }
    let mut p = oracle::InitParams::new();
    p.default_driver_name("Tabla").map_err(crate::ora::err)?;
    // On Linux the library path is set before the process starts (see `prepare_process`).
    if !cfg!(target_os = "linux") {
        if let Some(dir) = managed_lib_dir() {
            p.oracle_client_lib_dir(dir.to_string_lossy().as_ref()).map_err(crate::ora::err)?;
        }
    }
    match p.init() {
        Ok(_) => {
            *INIT_ERROR.lock().unwrap() = None;
            Ok(())
        }
        Err(e) => {
            let m = crate::ora::message(&e);
            *INIT_ERROR.lock().unwrap() = Some(m.clone());
            Err(AppError::msg(missing_client_message(&m)))
        }
    }
}

fn missing_client_message(m: &str) -> String {
    if m.contains("DPI-1047") || m.contains("DPI-1072") {
        "Oracle connections need Oracle Instant Client, which isn't installed. \
         Open the connection's settings to install it (about 40 MB from oracle.com)."
            .into()
    } else {
        m.to_string()
    }
}

pub fn status() -> ClientStatus {
    let loaded = init();
    let installed_but_not_loaded = loaded.is_err() && managed_lib_dir().is_some();
    ClientStatus {
        ready: loaded.is_ok(),
        version: loaded.as_ref().ok().and_then(|_| oracle::Version::client().ok()).map(|v| v.to_string()),
        error: loaded.err().map(|e| e.to_string()),
        can_install: download_url().is_some(),
        restart_needed: installed_but_not_loaded,
        license_url: LICENSE_URL,
    }
}

/// Downloads and unpacks Instant Client into Tabla's data directory.
/// `progress` receives (downloaded, total) byte counts.
pub async fn install(mut progress: impl FnMut(u64, Option<u64>)) -> AppResult<PathBuf> {
    use futures_util::StreamExt;
    let url = download_url().ok_or_else(|| AppError::msg("Oracle doesn't publish Instant Client for this system"))?;
    let root = managed_root()?;
    std::fs::create_dir_all(&root)?;
    let archive = root.join(if url.ends_with(".dmg") { "download.dmg" } else { "download.zip" });
    let res = reqwest::Client::builder()
        .user_agent("Tabla")
        .build()
        .map_err(|e| AppError::msg(e.to_string()))?
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| AppError::msg(format!("Couldn't download Instant Client: {e}")))?;
    let total = res.content_length();
    let mut file = tokio::fs::File::create(&archive).await?;
    let mut stream = res.bytes_stream();
    let mut done = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| AppError::msg(format!("Download interrupted: {e}")))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);
    let root2 = root.clone();
    let archive2 = archive.clone();
    let unpacked = tokio::task::spawn_blocking(move || unpack(&archive2, &root2)).await.map_err(|e| AppError::msg(e.to_string()))?;
    let _ = std::fs::remove_file(&archive);
    unpacked?;
    let dir = managed_lib_dir().ok_or_else(|| AppError::msg("The Instant Client download didn't contain the client library"))?;
    #[cfg(target_os = "linux")]
    link_libaio(&dir);
    Ok(dir)
}

fn unpack(archive: &Path, root: &Path) -> AppResult<()> {
    if archive.extension().is_some_and(|e| e == "dmg") {
        return unpack_dmg(archive, root);
    }
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| AppError::msg(format!("Unreadable download: {e}")))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| AppError::msg(e.to_string()))?;
        // enclosed_name rejects absolute paths and `..`.
        let Some(rel) = entry.enclosed_name() else { continue };
        if rel.starts_with("META-INF") {
            continue;
        }
        let out = root.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            // Symbolic links (libclntsh.so -> libclntsh.so.23.1).
            if mode & 0o170000 == 0o120000 {
                let mut target = String::new();
                std::io::Read::read_to_string(&mut entry, &mut target)?;
                let _ = std::fs::remove_file(&out);
                std::os::unix::fs::symlink(target, &out)?;
                continue;
            }
        }
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn unpack_dmg(archive: &Path, root: &Path) -> AppResult<()> {
    use std::process::Command;
    let mount = root.join("mount");
    let _ = std::fs::create_dir_all(&mount);
    let ok = Command::new("hdiutil").args(["attach", "-nobrowse", "-readonly", "-noverify", "-mountpoint"]).arg(&mount).arg(archive).status()?.success();
    if !ok {
        return Err(AppError::msg("Couldn't open the Instant Client disk image"));
    }
    let copied = (|| -> AppResult<()> {
        for entry in std::fs::read_dir(&mount)?.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join(LIB_NAME).exists() {
                // ditto keeps the library symlinks and code signatures intact.
                let dest = root.join(entry.file_name());
                if !Command::new("ditto").arg(&p).arg(&dest).status()?.success() {
                    return Err(AppError::msg("Couldn't copy Instant Client"));
                }
            }
        }
        Ok(())
    })();
    let _ = Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount).status();
    let _ = std::fs::remove_dir(&mount);
    copied
}

#[cfg(not(target_os = "macos"))]
fn unpack_dmg(_: &Path, _: &Path) -> AppResult<()> {
    Err(AppError::msg("Disk images can only be opened on macOS"))
}

/// Ubuntu 24.04+ renamed libaio.so.1 to libaio.so.1t64; Instant Client still
/// asks for the old name, so point it at the new one.
#[cfg(target_os = "linux")]
fn link_libaio(dir: &Path) {
    let link = dir.join("libaio.so.1");
    if link.exists() || ["/lib64/libaio.so.1", "/usr/lib64/libaio.so.1", "/lib/x86_64-linux-gnu/libaio.so.1", "/usr/lib/x86_64-linux-gnu/libaio.so.1", "/lib/aarch64-linux-gnu/libaio.so.1", "/usr/lib/aarch64-linux-gnu/libaio.so.1"].iter().any(|p| Path::new(p).exists()) {
        return;
    }
    for p in ["/usr/lib/x86_64-linux-gnu/libaio.so.1t64", "/usr/lib/aarch64-linux-gnu/libaio.so.1t64", "/lib/x86_64-linux-gnu/libaio.so.1t64"] {
        if Path::new(p).exists() {
            let _ = std::os::unix::fs::symlink(p, &link);
            return;
        }
    }
}

/// Linux only: the dynamic loader reads LD_LIBRARY_PATH once at startup, and
/// Instant Client's libraries find each other only through it. When Tabla has
/// its own Instant Client, restart the process once with the path set. Called
/// first thing in `main`, before any threads exist.
pub fn prepare_process() {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        const MARK: &str = "TABLA_ORACLE_CLIENT_PATH_SET";
        if std::env::var_os(MARK).is_some() {
            return;
        }
        let Some(dir) = managed_lib_dir() else { return };
        let current = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let dir = dir.to_string_lossy().into_owned();
        if current.split(':').any(|p| p == dir) {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let path = if current.is_empty() { dir } else { format!("{dir}:{current}") };
        // exec only returns on failure; then carry on without the client path.
        let _ = std::process::Command::new(exe).args(std::env::args_os().skip(1)).env("LD_LIBRARY_PATH", path).env(MARK, "1").exec();
    }
}

#[cfg(test)]
mod install_probe {
    #[tokio::test]
    #[ignore]
    async fn downloads_and_unpacks() {
        let dir = super::install(|_, _| {}).await.unwrap();
        assert!(dir.join("libclntsh.so").exists());
        eprintln!("installed at {}", dir.display());
    }
}
