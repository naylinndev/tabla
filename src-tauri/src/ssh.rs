//! SSH tunnels (local port forwarding) for databases that are only reachable
//! from inside a server, e.g. a VPS where Postgres listens on localhost.
//!
//! Auth: password, private key file (optionally passphrase-protected), or the
//! running SSH agent. Host keys are verified against `~/.ssh/known_hosts`
//! (read-only) and Tabla's own `known_hosts`; unknown hosts are trusted on
//! first use and recorded, and a changed key is always refused.

use std::{path::PathBuf, sync::Arc, time::Duration};

use russh::{
    client,
    keys::{self, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate, ssh_key},
};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{
    config::{SshAuth, SshConfig, config_dir},
    error::{AppError, AppResult},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A live tunnel. Dropping it stops forwarding and closes the SSH session.
pub struct Tunnel {
    pub local_port: u16,
    task: JoinHandle<()>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Client {
    host: String,
    port: u16,
    rejected: Arc<std::sync::Mutex<Option<String>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let pk = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(c) => PublicKey::new(c.public_key().clone(), ""),
        };
        match verify_host(&self.host, self.port, &pk) {
            Ok(()) => Ok(true),
            Err(msg) => {
                *self.rejected.lock().unwrap() = Some(msg);
                Ok(false)
            }
        }
    }
}

fn tabla_known_hosts() -> Option<PathBuf> {
    // Created owner-only, like the rest of Tabla's files: it lists the hosts you connect to.
    config_dir().ok().filter(|d| crate::vault::private_dir(d).is_ok()).map(|d| d.join("known_hosts"))
}

fn changed_msg(host: &str, file: &str) -> String {
    format!(
        "The SSH host key for {host} has CHANGED since you last connected. This can mean someone is \
         intercepting the connection, or the server was reinstalled. Tabla refused to connect. \
         If you trust the change, remove the old entry for {host} from {file} and try again."
    )
}

fn verify_host(host: &str, port: u16, pk: &PublicKey) -> Result<(), String> {
    // 1. The user's own known_hosts (never modified by Tabla).
    match keys::check_known_hosts(host, port, pk) {
        Ok(true) => return Ok(()),
        Err(keys::Error::KeyChanged { .. }) => return Err(changed_msg(host, "~/.ssh/known_hosts")),
        _ => {}
    }
    // 2. Tabla's own file: trust on first use. Serialized so concurrent
    // connects don't record the same host twice.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    let Some(path) = tabla_known_hosts() else { return Ok(()) };
    match keys::check_known_hosts_path(host, port, pk, &path) {
        Ok(true) => Ok(()),
        Err(keys::Error::KeyChanged { .. }) => Err(changed_msg(host, &path.to_string_lossy())),
        _ => {
            let _ = keys::known_hosts::learn_known_hosts_path(host, port, pk, &path);
            eprintln!(
                "tabla: trusted new SSH host {host}:{port} ({})",
                pk.fingerprint(ssh_key::HashAlg::Sha256)
            );
            Ok(())
        }
    }
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(p)),
        None => PathBuf::from(p),
    }
}

fn ssh_err(e: impl std::fmt::Display) -> AppError {
    AppError::msg(format!("SSH: {e}"))
}

/// The UI recognizes these two messages and asks for the passphrase in a dialog.
pub const PASSPHRASE_NEEDED: &str = "This SSH key is protected by a passphrase. Enter it in the Passphrase field.";
pub const PASSPHRASE_WRONG: &str = "That passphrase didn't unlock the SSH key.";

/// Why the SSH server couldn't open a forward to the database, in plain words.
fn forward_err(e: russh::Error, ssh_host: &str, target: &str, port: u16) -> AppError {
    use russh::ChannelOpenFailure as F;
    let why = match e {
        russh::Error::ChannelOpenFailure(F::ConnectFailed) => format!(
            "nothing is accepting connections on {target}:{port} there. Check the database's port and that it listens on {target} \
             (try 127.0.0.1 instead of localhost)"
        ),
        russh::Error::ChannelOpenFailure(F::AdministrativelyProhibited) => {
            "the server doesn't allow port forwarding (AllowTcpForwarding in sshd_config) for this user".into()
        }
        other => other.to_string(),
    };
    AppError::msg(format!("Signed in to {ssh_host} over SSH, but it couldn't reach the database: {why}."))
}

async fn auth_agent(session: &mut client::Handle<Client>, user: &str) -> AppResult<bool> {
    #[cfg(unix)]
    let mut agent = keys::agent::client::AgentClient::connect_env()
        .await
        .map_err(|_| AppError::msg("SSH agent not found. Start ssh-agent (or 1Password/Secretive) and add your key with ssh-add."))?;
    #[cfg(windows)]
    let mut agent = keys::agent::client::AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
        .await
        .map_err(|_| AppError::msg("OpenSSH Authentication Agent is not running. Start it in Services and add your key with ssh-add."))?;
    let identities = agent.request_identities().await.map_err(ssh_err)?;
    if identities.is_empty() {
        return Err(AppError::msg("Your SSH agent has no keys loaded. Run ssh-add first."));
    }
    for id in identities {
        let pk = id.public_key().into_owned();
        let hash = session.best_supported_rsa_hash().await.map_err(ssh_err)?.flatten();
        if session.authenticate_publickey_with(user, pk, hash, &mut agent).await.map_err(ssh_err)?.success() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Opens an SSH session and forwards a random local port to `target_host:target_port`
/// as seen from the SSH server. `secret` is the SSH password or key passphrase.
pub async fn open(ssh: &SshConfig, secret: Option<String>, target_host: &str, target_port: u16) -> AppResult<Tunnel> {
    let host = ssh.host.trim().to_string();
    if host.is_empty() {
        return Err(AppError::msg("Enter the SSH server's host name or IP"));
    }
    let port = if ssh.port == 0 { 22 } else { ssh.port };
    let user = if ssh.user.trim().is_empty() { "root".to_string() } else { ssh.user.trim().to_string() };

    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    let rejected = Arc::new(std::sync::Mutex::new(None));
    let handler = Client { host: host.clone(), port, rejected: rejected.clone() };

    let connect = client::connect(config, (host.as_str(), port), handler);
    let mut session = match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Err(_) => return Err(AppError::msg(format!("SSH: timed out connecting to {host}:{port}"))),
        Ok(Err(e)) => {
            if let Some(msg) = rejected.lock().unwrap().take() {
                return Err(AppError::Msg(msg));
            }
            return Err(ssh_err(format!("{e} ({host}:{port})")));
        }
        Ok(Ok(s)) => s,
    };

    let ok = match ssh.auth {
        SshAuth::Password => session
            .authenticate_password(&user, secret.unwrap_or_default())
            .await
            .map_err(ssh_err)?
            .success(),
        SshAuth::Key => {
            if ssh.key_path.trim().is_empty() {
                return Err(AppError::msg("Choose your SSH private key file (for example ~/.ssh/id_ed25519)"));
            }
            let path = expand_home(ssh.key_path.trim());
            let passphrase = secret.as_deref().filter(|s| !s.is_empty());
            let key = keys::load_secret_key(&path, passphrase).map_err(|e| match e {
                keys::Error::KeyIsEncrypted if passphrase.is_none() => AppError::msg(PASSPHRASE_NEEDED),
                keys::Error::IO(io) => AppError::msg(format!("Couldn't read the SSH key {}: {io}", path.display())),
                // With a passphrase given, a key that won't decode almost always means the passphrase is wrong.
                _ if passphrase.is_some() => AppError::msg(PASSPHRASE_WRONG),
                e => AppError::msg(format!("Couldn't read the SSH key {}: {e}", path.display())),
            })?;
            let hash = session.best_supported_rsa_hash().await.map_err(ssh_err)?.flatten();
            session
                .authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(ssh_err)?
                .success()
        }
        SshAuth::Agent => auth_agent(&mut session, &user).await?,
    };
    if !ok {
        return Err(AppError::msg(format!(
            "SSH login failed for {user}@{host}. Check the user name and that this key or password is allowed on the server."
        )));
    }

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let local_port = listener.local_addr()?.port();
    // Reach the database once now: if the server can't, say why here instead of
    // handing the driver a socket that just closes ("connection reset by peer").
    // The channel is kept for the driver's first connection.
    let first = session
        .channel_open_direct_tcpip(target_host, target_port as u32, "127.0.0.1", local_port as u32)
        .await
        .map_err(|e| forward_err(e, &host, target_host, target_port))?;
    let first = Arc::new(tokio::sync::Mutex::new(Some(first)));
    let session = Arc::new(session);
    let target_host = target_host.to_string();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut tcp, _)) = listener.accept().await else { break };
            let session = session.clone();
            let target = target_host.clone();
            let first = first.clone();
            tokio::spawn(async move {
                let ready = first.lock().await.take();
                let ch = match ready {
                    Some(ch) => Ok(ch),
                    None => session.channel_open_direct_tcpip(target.as_str(), target_port as u32, "127.0.0.1", local_port as u32).await,
                };
                match ch {
                    Ok(ch) => {
                        let mut stream = ch.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                    }
                    Err(e) => eprintln!("tabla: SSH forward to {target}:{target_port} failed: {e}"),
                }
            });
        }
    });
    Ok(Tunnel { local_port, task })
}

#[cfg(test)]
mod tests {
    //! Needs a real sshd. Skipped unless TABLA_TEST_SSH="host:port:user:/dir/with/keys"
    //! and TABLA_TEST_PG (see db.rs) are set; TABLA_CONFIG_DIR should point at a temp dir.
    //! The key dir must contain user_key, user_key_pp (passphrase "s3cret-phrase"),
    //! user_rsa and host_ed25519.pub.
    use super::*;
    use crate::{
        config::{ConnectionConfig, Driver, SslMode},
        db::Session,
    };

    struct Env {
        host: String,
        port: u16,
        user: String,
        keys: String,
        pg_user: String,
    }

    fn env() -> Option<Env> {
        let v = std::env::var("TABLA_TEST_SSH").ok()?;
        let pg = std::env::var("TABLA_TEST_PG").ok()?;
        let p: Vec<&str> = v.splitn(4, ':').collect();
        Some(Env {
            host: p[0].into(),
            port: p[1].parse().unwrap(),
            user: p[2].into(),
            keys: p[3].into(),
            pg_user: pg.split(':').nth(2).unwrap().into(),
        })
    }

    fn cfg(e: &Env, ssh_host: &str, auth: SshAuth, key: &str, secret: Option<&str>) -> ConnectionConfig {
        ConnectionConfig {
            id: format!("ssh-test-{key}-{ssh_host}"),
            name: "ssh test".into(),
            driver: Driver::Postgres,
            provider: String::new(),
            // Resolved on the SSH server, like a VPS-local database.
            host: "127.0.0.1".into(),
            port: 5432,
            user: e.pg_user.clone(),
            password: None,
            database: "tabla_test".into(),
            ssl_mode: SslMode::Disable,
            ssl_ca: String::new(),
            ssl_cert: String::new(),
            ssl_key: String::new(),
            ssh: SshConfig {
                enabled: true,
                host: ssh_host.into(),
                port: e.port,
                user: e.user.clone(),
                auth,
                key_path: if key.is_empty() { String::new() } else { format!("{}/{key}", e.keys) },
            },
            ssh_secret: secret.map(Into::into),
            color: String::new(),
            tag: String::new(),
            read_only: false,
            ai_access: false,
        }
    }

    async fn count_users(c: &ConnectionConfig) -> Result<String, String> {
        let s = Session::connect(c, None).await.map_err(|e| e.to_string())?;
        let rs = s.run_script("SELECT count(*) FROM users", 10).await.map_err(|e| e.to_string())?;
        Ok(rs[0].rows[0][0].clone().unwrap())
    }

    #[tokio::test]
    async fn key_auth_tunnels_to_postgres() {
        let Some(e) = env() else { return };
        assert_eq!(count_users(&cfg(&e, &e.host, SshAuth::Key, "user_key", None)).await.unwrap(), "1200");
    }

    #[tokio::test]
    async fn rsa_key_works() {
        let Some(e) = env() else { return };
        assert_eq!(count_users(&cfg(&e, &e.host, SshAuth::Key, "user_rsa", None)).await.unwrap(), "1200");
    }

    #[tokio::test]
    async fn passphrase_key_needs_passphrase() {
        let Some(e) = env() else { return };
        let err = count_users(&cfg(&e, &e.host, SshAuth::Key, "user_key_pp", None)).await.unwrap_err();
        assert!(err.contains(PASSPHRASE_NEEDED), "{err}");
        let bad = count_users(&cfg(&e, &e.host, SshAuth::Key, "user_key_pp", Some("wrong"))).await.unwrap_err();
        assert!(bad.contains(PASSPHRASE_WRONG), "{bad}");
        assert_eq!(
            count_users(&cfg(&e, &e.host, SshAuth::Key, "user_key_pp", Some("s3cret-phrase"))).await.unwrap(),
            "1200"
        );
    }

    #[tokio::test]
    async fn unreachable_database_is_explained() {
        let Some(e) = env() else { return };
        // Signed in fine, but nothing listens on this port on the SSH server.
        let mut c = cfg(&e, &e.host, SshAuth::Key, "user_key", None);
        c.port = 5999;
        let err = count_users(&c).await.unwrap_err();
        assert!(err.contains("couldn't reach the database") && err.contains("5999"), "{err}");
    }

    #[tokio::test]
    async fn unauthorized_key_is_rejected() {
        let Some(e) = env() else { return };
        // The host key is a valid key file but not in authorized_keys.
        let err = count_users(&cfg(&e, &e.host, SshAuth::Key, "host_ed25519", None)).await.unwrap_err();
        assert!(err.contains("SSH login failed"), "{err}");
    }

    #[tokio::test]
    async fn agent_auth() {
        let Some(e) = env() else { return };
        // Opt-in: needs an agent holding a test key, never the user's personal agent.
        if std::env::var("TABLA_TEST_SSH_AGENT").is_err() {
            return;
        }
        assert_eq!(count_users(&cfg(&e, &e.host, SshAuth::Agent, "", None)).await.unwrap(), "1200");
    }

    #[tokio::test]
    async fn changed_host_key_is_refused() {
        let Some(e) = env() else { return };
        // Record a *different* key for "localhost" in Tabla's known_hosts, then connect via that name.
        let other = keys::load_public_key(format!("{}/user_key.pub", e.keys)).unwrap();
        let path = tabla_known_hosts().unwrap();
        keys::known_hosts::learn_known_hosts_path("localhost", e.port, &other, &path).unwrap();
        let err = count_users(&cfg(&e, "localhost", SshAuth::Key, "user_key", None)).await.unwrap_err();
        assert!(err.contains("CHANGED"), "{err}");
    }
}
