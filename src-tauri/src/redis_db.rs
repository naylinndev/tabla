//! Redis sessions: key browser, typed value editors and a command console.
//!
//! Keys are listed with SCAN (never KEYS), values are read with bounded
//! commands (HSCAN/LRANGE/SSCAN/ZRANGE with limits) so a huge key can't
//! freeze the app, and destructive console commands are flagged for the UI.

use std::time::{Duration, Instant};

use redis::{Value, aio::ConnectionManager};
use serde::{Deserialize, Serialize};

use crate::{
    config::{ConnectionConfig, SslMode},
    error::{AppError, AppResult},
};

const MAX_ITEMS: usize = 1000;

pub struct RedisSession {
    conn: ConnectionManager,
    pub read_only: bool,
    /// Kept alive while connected through SSH.
    pub tunnel: Option<crate::ssh::Tunnel>,
    log: std::sync::Mutex<std::collections::VecDeque<crate::db::LogEntry>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// Milliseconds; -1 = no expiry.
    pub ttl: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanPage {
    pub cursor: u64,
    pub keys: Vec<KeyInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedisValue {
    #[serde(rename = "type")]
    pub kind: String,
    pub ttl: i64,
    /// Total length (characters, fields, items or members).
    pub length: u64,
    /// string -> [[value]]; hash -> [[field, value]]; list -> [[index, value]];
    /// set -> [[member]]; zset -> [[member, score]]; stream -> [[id, "k=v …"]].
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbInfo {
    pub index: u32,
    pub keys: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedisOverview {
    pub version: String,
    pub mode: String,
    pub used_memory: String,
    pub databases: Vec<DbInfo>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum RedisEdit {
    SetString { key: String, value: String },
    HashSet { key: String, field: String, value: String },
    HashDel { key: String, field: String },
    ListSet { key: String, index: i64, value: String },
    ListPush { key: String, value: String },
    ListRemove { key: String, value: String },
    SetAdd { key: String, member: String },
    SetRemove { key: String, member: String },
    ZAdd { key: String, member: String, score: f64 },
    ZRemove { key: String, member: String },
    Delete { keys: Vec<String> },
    Expire { key: String, seconds: Option<i64> },
    Rename { key: String, to: String },
}

fn r(e: redis::RedisError) -> AppError {
    AppError::msg(format!("Redis: {e}"))
}

/// Renders any reply for the console (strings, arrays, maps, errors…).
pub fn render(v: &Value, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    match v {
        Value::Nil => "(nil)".into(),
        Value::Int(i) => format!("(integer) {i}"),
        Value::BulkString(b) => format!("\"{}\"", String::from_utf8_lossy(b)),
        Value::SimpleString(s) => s.clone(),
        Value::Okay => "OK".into(),
        Value::Double(d) => format!("(double) {d}"),
        Value::Boolean(b) => format!("(boolean) {b}"),
        Value::VerbatimString { text, .. } => text.clone(),
        Value::BigNumber(n) => format!("(big number) {n}"),
        Value::ServerError(e) => format!("(error) {}", e.details().unwrap_or(e.code())),
        Value::Array(items) | Value::Set(items) => {
            if items.is_empty() {
                return "(empty array)".into();
            }
            items
                .iter()
                .enumerate()
                .map(|(i, x)| format!("{pad}{}) {}", i + 1, render(x, indent + 1).trim_start()))
                .collect::<Vec<_>>()
                .join("\n")
        }
        Value::Map(pairs) => pairs
            .iter()
            .map(|(k, x)| format!("{pad}{} => {}", render(k, indent + 1), render(x, indent + 1).trim_start()))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Attribute { data, .. } => render(data, indent),
        Value::Push { data, .. } => render(&Value::Array(data.clone()), indent),
        #[allow(unreachable_patterns)]
        other => format!("{other:?}"),
    }
}

fn as_str(v: &Value) -> String {
    match v {
        Value::BulkString(b) => String::from_utf8_lossy(b).into_owned(),
        Value::SimpleString(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        Value::Double(d) => d.to_string(),
        Value::Nil => String::new(),
        other => render(other, 0),
    }
}

/// Splits a console line into arguments, honoring quotes and backslash escapes.
pub fn split_args(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(match n {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if quote.is_some() {
        return Err("Unclosed quote".into());
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Commands that change data or the server; the UI asks before running them on
/// production and read-only connections refuse them.
pub fn is_write_command(cmd: &str) -> bool {
    const READS: &[&str] = &[
        "GET", "MGET", "STRLEN", "GETRANGE", "EXISTS", "TYPE", "TTL", "PTTL", "KEYS", "SCAN", "HGET", "HMGET", "HGETALL",
        "HKEYS", "HVALS", "HLEN", "HSCAN", "HEXISTS", "LRANGE", "LLEN", "LINDEX", "SMEMBERS", "SCARD", "SISMEMBER",
        "SSCAN", "ZRANGE", "ZRANGEBYSCORE", "ZREVRANGE", "ZCARD", "ZSCORE", "ZRANK", "ZSCAN", "XRANGE", "XREVRANGE",
        "XLEN", "XINFO", "INFO", "PING", "ECHO", "DBSIZE", "TIME", "MEMORY", "OBJECT", "DUMP", "RANDOMKEY", "CLIENT",
        "CONFIG", "SLOWLOG", "COMMAND", "LASTSAVE", "ROLE", "PFCOUNT", "GEOPOS", "GEODIST", "GEOSEARCH", "BITCOUNT",
        "GETBIT", "JSON.GET", "JSON.TYPE", "FT.SEARCH", "FT.INFO", "SELECT",
    ];
    !READS.contains(&cmd.to_ascii_uppercase().as_str())
}

/// What AI assistants (MCP) may run: reads of keys and server stats only.
/// No CLIENT, CONFIG, SLOWLOG, KEYS (blocks the server) or SELECT.
pub fn ai_allowed(cmd: &str) -> bool {
    const READS: &[&str] = &[
        "GET", "MGET", "STRLEN", "GETRANGE", "EXISTS", "TYPE", "TTL", "PTTL", "SCAN", "HGET", "HMGET", "HGETALL", "HKEYS",
        "HVALS", "HLEN", "HSCAN", "HEXISTS", "LRANGE", "LLEN", "LINDEX", "SMEMBERS", "SCARD", "SISMEMBER", "SSCAN", "ZRANGE",
        "ZRANGEBYSCORE", "ZREVRANGE", "ZCARD", "ZSCORE", "ZRANK", "ZSCAN", "XRANGE", "XREVRANGE", "XLEN", "XINFO", "INFO",
        "PING", "DBSIZE", "PFCOUNT", "GEOPOS", "GEODIST", "BITCOUNT", "GETBIT", "JSON.GET", "JSON.TYPE", "FT.SEARCH", "FT.INFO",
    ];
    READS.contains(&cmd.to_ascii_uppercase().as_str())
}

pub fn is_dangerous(cmd: &str) -> bool {
    matches!(
        cmd.to_ascii_uppercase().as_str(),
        "FLUSHALL" | "FLUSHDB" | "SHUTDOWN" | "DEBUG" | "CONFIG" | "SWAPDB" | "MIGRATE" | "REPLICAOF" | "SLAVEOF" | "FAILOVER" | "KEYS"
    )
}

impl RedisSession {
    /// Connects directly, or through the configured SSH tunnel.
    pub async fn open(cfg: &ConnectionConfig, password: Option<String>) -> AppResult<Self> {
        let host = if cfg.host.trim().is_empty() { "localhost".to_string() } else { cfg.host.trim().to_string() };
        let port = cfg.port_or_default();
        if cfg.ssh.enabled {
            let secret = crate::config::resolve_ssh_secret(cfg)?;
            let tunnel = crate::ssh::open(&cfg.ssh, secret, &host, port).await?;
            let mut s = Self::connect(cfg, password, "127.0.0.1", tunnel.local_port).await?;
            s.tunnel = Some(tunnel);
            return Ok(s);
        }
        Self::connect(cfg, password, &host, port).await
    }

    pub async fn connect(cfg: &ConnectionConfig, password: Option<String>, host: &str, port: u16) -> AppResult<Self> {
        let db: i64 = cfg.database.trim().parse().unwrap_or(0);
        let tls = matches!(cfg.ssl_mode, SslMode::Require | SslMode::VerifyFull);
        let scheme = if tls { "rediss" } else { "redis" };
        let enc = |s: &str| {
            s.bytes()
                .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
                .collect::<String>()
        };
        let auth = match (cfg.user.trim(), password.as_deref().filter(|p| !p.is_empty())) {
            ("", None) => String::new(),
            ("", Some(p)) => format!(":{}@", enc(p)),
            (u, None) => format!("{}@", enc(u)),
            (u, Some(p)) => format!("{}:{}@", enc(u), enc(p)),
        };
        let insecure = if tls && cfg.ssl_mode != SslMode::VerifyFull { "#insecure" } else { "" };
        let url = format!("{scheme}://{auth}{host}:{port}/{db}{insecure}");
        let client = redis::Client::open(url).map_err(r)?;
        let config = redis::aio::ConnectionManagerConfig::new()
            .set_connection_timeout(Some(Duration::from_secs(10)))
            .set_response_timeout(Some(Duration::from_secs(30)));
        let mut conn = tokio::time::timeout(Duration::from_secs(12), client.get_connection_manager_with_config(config))
            .await
            .map_err(|_| AppError::msg(format!("Redis: timed out connecting to {host}:{port}")))?
            .map_err(r)?;
        let _: String = redis::cmd("PING").query_async(&mut conn).await.map_err(r)?;
        Ok(Self { conn, read_only: cfg.read_only, tunnel: None, log: Default::default() })
    }

    fn record(&self, line: &str, started: Instant, outcome: Result<String, String>) {
        let mut log = self.log.lock().unwrap();
        let id = log.back().map(|e| e.id + 1).unwrap_or(1);
        // AUTH arguments are secrets.
        let line = if line.trim_start().to_ascii_uppercase().starts_with("AUTH") { "AUTH •••".to_string() } else { line.to_string() };
        let (ok, info) = match outcome {
            Ok(i) => (true, i),
            Err(e) => (false, e),
        };
        log.push_back(crate::db::LogEntry {
            id,
            at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0),
            sql: line,
            ms: started.elapsed().as_millis(),
            ok,
            info,
        });
        while log.len() > 500 {
            log.pop_front();
        }
    }

    pub fn log_since(&self, after: u64) -> Vec<crate::db::LogEntry> {
        self.log.lock().unwrap().iter().filter(|e| e.id > after).cloned().collect()
    }

    pub fn clear_log(&self) {
        self.log.lock().unwrap().clear();
    }

    async fn run(&self, c: &mut redis::Cmd, label: &str) -> AppResult<Value> {
        let started = Instant::now();
        let mut conn = self.conn.clone();
        let res: Result<Value, _> = c.query_async(&mut conn).await;
        self.record(label, started, res.as_ref().map(|_| "ok".into()).map_err(|e| e.to_string()));
        res.map_err(r)
    }

    pub async fn overview(&self) -> AppResult<RedisOverview> {
        let info = as_str(&self.run(&mut redis::cmd("INFO"), "INFO").await?);
        let field = |k: &str| {
            info.lines().find_map(|l| l.strip_prefix(&format!("{k}:"))).map(|v| v.trim().to_string()).unwrap_or_default()
        };
        // Keyspace lines look like: db0:keys=12,expires=0,avg_ttl=0
        let mut databases: Vec<DbInfo> = info
            .lines()
            .filter_map(|l| {
                let (db, rest) = l.split_once(':')?;
                let index = db.strip_prefix("db")?.parse().ok()?;
                let keys = rest.split(',').find_map(|p| p.strip_prefix("keys="))?.parse().ok()?;
                Some(DbInfo { index, keys })
            })
            .collect();
        // Managed services often disable CONFIG; fall back to the usual 16.
        let count = match self.run(redis::cmd("CONFIG").arg("GET").arg("databases"), "CONFIG GET databases").await {
            Ok(Value::Array(v)) if v.len() == 2 => as_str(&v[1]).parse().unwrap_or(16),
            Ok(Value::Map(m)) if !m.is_empty() => as_str(&m[0].1).parse().unwrap_or(16),
            _ => 16,
        };
        for i in 0..count {
            if !databases.iter().any(|d| d.index == i) {
                databases.push(DbInfo { index: i, keys: 0 });
            }
        }
        databases.sort_by_key(|d| d.index);
        Ok(RedisOverview {
            version: field("redis_version"),
            mode: field("redis_mode"),
            used_memory: field("used_memory_human"),
            databases,
        })
    }

    pub async fn select(&self, db: u32) -> AppResult<()> {
        self.run(redis::cmd("SELECT").arg(db), &format!("SELECT {db}")).await.map(|_| ())
    }

    pub async fn scan(&self, pattern: &str, cursor: u64, count: usize) -> AppResult<ScanPage> {
        let pattern = if pattern.trim().is_empty() { "*" } else { pattern.trim() };
        let v = self
            .run(redis::cmd("SCAN").arg(cursor).arg("MATCH").arg(pattern).arg("COUNT").arg(count.max(10)), &format!("SCAN {cursor} MATCH {pattern}"))
            .await?;
        let Value::Array(parts) = v else { return Err(AppError::msg("Unexpected SCAN reply")) };
        let next: u64 = parts.first().map(as_str).and_then(|s| s.parse().ok()).unwrap_or(0);
        let names: Vec<String> = match parts.get(1) {
            Some(Value::Array(k)) => k.iter().map(as_str).collect(),
            _ => vec![],
        };
        let mut keys = Vec::with_capacity(names.len());
        if !names.is_empty() {
            let mut pipe = redis::pipe();
            for k in &names {
                pipe.cmd("TYPE").arg(k).cmd("PTTL").arg(k);
            }
            let mut conn = self.conn.clone();
            let started = Instant::now();
            let res: Vec<Value> = pipe.query_async(&mut conn).await.map_err(r)?;
            self.record(&format!("TYPE/PTTL × {}", names.len()), started, Ok(format!("{} keys", names.len())));
            for (i, k) in names.into_iter().enumerate() {
                keys.push(KeyInfo {
                    key: k,
                    kind: res.get(i * 2).map(as_str).unwrap_or_default(),
                    ttl: res.get(i * 2 + 1).map(as_str).and_then(|s| s.parse().ok()).unwrap_or(-1),
                });
            }
        }
        keys.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(ScanPage { cursor: next, keys })
    }

    pub async fn get(&self, key: &str) -> AppResult<RedisValue> {
        let kind = as_str(&self.run(redis::cmd("TYPE").arg(key), &format!("TYPE {key}")).await?);
        let ttl: i64 = as_str(&self.run(redis::cmd("PTTL").arg(key), &format!("PTTL {key}")).await?).parse().unwrap_or(-1);
        let count = |v: Value| as_str(&v).parse::<u64>().unwrap_or(0);
        let pairs = |v: Value| -> Vec<Vec<String>> {
            match v {
                Value::Array(items) => items.chunks(2).map(|c| c.iter().map(as_str).collect()).collect(),
                Value::Map(m) => m.iter().map(|(a, b)| vec![as_str(a), as_str(b)]).collect(),
                _ => vec![],
            }
        };
        let (length, rows) = match kind.as_str() {
            "string" => {
                let v = self.run(redis::cmd("GET").arg(key), &format!("GET {key}")).await?;
                let s = as_str(&v);
                (s.chars().count() as u64, vec![vec![s]])
            }
            "hash" => {
                let len = count(self.run(redis::cmd("HLEN").arg(key), &format!("HLEN {key}")).await?);
                let v = self.run(redis::cmd("HSCAN").arg(key).arg(0).arg("COUNT").arg(MAX_ITEMS), &format!("HSCAN {key}")).await?;
                let rows = match v {
                    Value::Array(p) if p.len() == 2 => pairs(p[1].clone()),
                    _ => vec![],
                };
                (len, rows)
            }
            "list" => {
                let len = count(self.run(redis::cmd("LLEN").arg(key), &format!("LLEN {key}")).await?);
                let v = self.run(redis::cmd("LRANGE").arg(key).arg(0).arg(MAX_ITEMS as i64 - 1), &format!("LRANGE {key} 0 {}", MAX_ITEMS - 1)).await?;
                let rows = match v {
                    Value::Array(items) => items.iter().enumerate().map(|(i, x)| vec![i.to_string(), as_str(x)]).collect(),
                    _ => vec![],
                };
                (len, rows)
            }
            "set" => {
                let len = count(self.run(redis::cmd("SCARD").arg(key), &format!("SCARD {key}")).await?);
                let v = self.run(redis::cmd("SSCAN").arg(key).arg(0).arg("COUNT").arg(MAX_ITEMS), &format!("SSCAN {key}")).await?;
                let mut rows: Vec<Vec<String>> = match v {
                    Value::Array(p) if p.len() == 2 => match &p[1] {
                        Value::Array(m) => m.iter().map(|x| vec![as_str(x)]).collect(),
                        _ => vec![],
                    },
                    _ => vec![],
                };
                rows.sort();
                (len, rows)
            }
            "zset" => {
                let len = count(self.run(redis::cmd("ZCARD").arg(key), &format!("ZCARD {key}")).await?);
                let v = self
                    .run(redis::cmd("ZRANGE").arg(key).arg(0).arg(MAX_ITEMS as i64 - 1).arg("WITHSCORES"), &format!("ZRANGE {key} WITHSCORES"))
                    .await?;
                let rows = match v {
                    // RESP3 returns [[member, score], …]; RESP2 a flat list.
                    Value::Array(items) if items.first().is_some_and(|x| matches!(x, Value::Array(_))) => items
                        .iter()
                        .filter_map(|x| match x {
                            Value::Array(p) => Some(p.iter().map(as_str).collect()),
                            _ => None,
                        })
                        .collect(),
                    other => pairs(other),
                };
                (len, rows)
            }
            "stream" => {
                let len = count(self.run(redis::cmd("XLEN").arg(key), &format!("XLEN {key}")).await?);
                let v = self.run(redis::cmd("XREVRANGE").arg(key).arg("+").arg("-").arg("COUNT").arg(200), &format!("XREVRANGE {key}")).await?;
                let rows = match v {
                    Value::Array(entries) => entries
                        .iter()
                        .filter_map(|e| match e {
                            Value::Array(p) if p.len() == 2 => {
                                let fields = pairs(p[1].clone()).into_iter().map(|kv| kv.join("=")).collect::<Vec<_>>().join("  ");
                                Some(vec![as_str(&p[0]), fields])
                            }
                            _ => None,
                        })
                        .collect(),
                    _ => vec![],
                };
                (len, rows)
            }
            "none" => return Err(AppError::msg(format!("Key \"{key}\" no longer exists"))),
            other => (0, vec![vec![format!("Values of type \"{other}\" can be read in the console")]]),
        };
        let truncated = (rows.len() as u64) < length && kind != "string";
        Ok(RedisValue { kind, ttl, length, rows, truncated })
    }

    pub async fn edit(&self, e: RedisEdit) -> AppResult<()> {
        if self.read_only {
            return Err(AppError::msg("This connection is read-only"));
        }
        let mut c = match &e {
            RedisEdit::SetString { key, value } => {
                // KEEPTTL: editing a value shouldn't silently drop its expiry.
                let mut c = redis::cmd("SET");
                c.arg(key).arg(value).arg("KEEPTTL");
                c
            }
            RedisEdit::HashSet { key, field, value } => redis::cmd("HSET").arg(key).arg(field).arg(value).to_owned(),
            RedisEdit::HashDel { key, field } => redis::cmd("HDEL").arg(key).arg(field).to_owned(),
            RedisEdit::ListSet { key, index, value } => redis::cmd("LSET").arg(key).arg(*index).arg(value).to_owned(),
            RedisEdit::ListPush { key, value } => redis::cmd("RPUSH").arg(key).arg(value).to_owned(),
            RedisEdit::ListRemove { key, value } => redis::cmd("LREM").arg(key).arg(1).arg(value).to_owned(),
            RedisEdit::SetAdd { key, member } => redis::cmd("SADD").arg(key).arg(member).to_owned(),
            RedisEdit::SetRemove { key, member } => redis::cmd("SREM").arg(key).arg(member).to_owned(),
            RedisEdit::ZAdd { key, member, score } => redis::cmd("ZADD").arg(key).arg(*score).arg(member).to_owned(),
            RedisEdit::ZRemove { key, member } => redis::cmd("ZREM").arg(key).arg(member).to_owned(),
            RedisEdit::Delete { keys } => {
                let mut c = redis::cmd("UNLINK");
                for k in keys {
                    c.arg(k);
                }
                c
            }
            RedisEdit::Expire { key, seconds } => match seconds {
                Some(s) => redis::cmd("EXPIRE").arg(key).arg(*s).to_owned(),
                None => redis::cmd("PERSIST").arg(key).to_owned(),
            },
            RedisEdit::Rename { key, to } => redis::cmd("RENAMENX").arg(key).arg(to).to_owned(),
        };
        let label = match &e {
            RedisEdit::Delete { keys } => format!("UNLINK {}", keys.join(" ")),
            RedisEdit::Rename { key, to } => format!("RENAMENX {key} {to}"),
            _ => "edit".into(),
        };
        let v = self.run(&mut c, &label).await?;
        if let (RedisEdit::Rename { to, .. }, Value::Int(0)) = (&e, &v) {
            return Err(AppError::msg(format!("A key named \"{to}\" already exists")));
        }
        Ok(())
    }

    /// Runs one console line. `allow_write` must be true for writes.
    pub async fn command(&self, line: &str, allow_write: bool) -> AppResult<String> {
        let args = split_args(line).map_err(AppError::Msg)?;
        let Some(name) = args.first() else { return Ok(String::new()) };
        if is_write_command(name) && self.read_only {
            return Err(AppError::msg(format!("{} is blocked: this connection is read-only", name.to_uppercase())));
        }
        // Writes, and anything that can wipe data or stall the server (FLUSHALL, KEYS…), need an explicit go-ahead.
        if (is_write_command(name) || is_dangerous(name)) && !allow_write {
            return Err(AppError::msg("needs-confirmation"));
        }
        let mut c = redis::cmd(name);
        for a in &args[1..] {
            c.arg(a);
        }
        let mut conn = self.conn.clone();
        let started = Instant::now();
        let res: Result<Value, _> = c.query_async(&mut conn).await;
        self.record(line, started, res.as_ref().map(|_| "ok".into()).map_err(|e| e.to_string()));
        Ok(match res {
            Ok(v) => render(&v, 0),
            Err(e) => format!("(error) {e}"),
        })
    }

    /// The connection manager closes when the last clone is dropped.
    pub async fn close(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_console_args() {
        assert_eq!(split_args(r#"SET "my key" 'a b' plain"#).unwrap(), ["SET", "my key", "a b", "plain"]);
        assert_eq!(split_args(r#"SET k "line\nbreak""#).unwrap(), ["SET", "k", "line\nbreak"]);
        assert_eq!(split_args(r#"SET k """#).unwrap(), ["SET", "k", ""]);
        assert!(split_args(r#"SET "oops"#).is_err());
    }

    #[test]
    fn classifies_commands() {
        assert!(!is_write_command("get"));
        assert!(is_write_command("SET"));
        assert!(is_write_command("flushall"));
        assert!(is_dangerous("FLUSHDB"));
        assert!(!is_dangerous("GET"));
        assert!(ai_allowed("hgetall"));
        for c in ["SET", "DEL", "FLUSHALL", "CONFIG", "CLIENT", "KEYS", "SELECT", "EVAL", "SLOWLOG"] {
            assert!(!ai_allowed(c), "{c}");
        }
    }

    #[tokio::test]
    async fn redis_roundtrip() {
        // TABLA_TEST_REDIS="host:port:db" — uses a throwaway database index.
        let Ok(v) = std::env::var("TABLA_TEST_REDIS") else { return };
        let p: Vec<&str> = v.split(':').collect();
        let cfg: ConnectionConfig = serde_json::from_value(serde_json::json!({
            "id": "redis-test", "name": "r", "driver": "redis", "sslMode": "disable", "database": p[2]
        }))
        .unwrap();
        let s = RedisSession::connect(&cfg, None, p[0], p[1].parse().unwrap()).await.unwrap();
        s.command("FLUSHDB", true).await.unwrap();
        s.command(r#"SET greeting "hello world""#, true).await.unwrap();
        s.command("HSET user:1 name Ana plan pro", true).await.unwrap();
        s.command("RPUSH queue a b c", true).await.unwrap();
        s.command("SADD tags x y", true).await.unwrap();
        s.command("ZADD board 10 ana 20 ben", true).await.unwrap();
        s.command("XADD events * kind signup user 1", true).await.unwrap();
        s.command("EXPIRE greeting 300", true).await.unwrap();

        let page = s.scan("*", 0, 100).await.unwrap();
        let kinds: Vec<(String, String)> = page.keys.iter().map(|k| (k.key.clone(), k.kind.clone())).collect();
        assert_eq!(kinds.len(), 6, "{kinds:?}");
        assert!(page.keys.iter().find(|k| k.key == "greeting").unwrap().ttl > 0);
        assert_eq!(s.scan("user:*", 0, 100).await.unwrap().keys.len(), 1);

        assert_eq!(s.get("greeting").await.unwrap().rows, vec![vec!["hello world".to_string()]]);
        let h = s.get("user:1").await.unwrap();
        assert_eq!((h.kind.as_str(), h.length), ("hash", 2));
        assert_eq!(s.get("queue").await.unwrap().rows[2], ["2", "c"]);
        assert_eq!(s.get("board").await.unwrap().rows[1][0], "ben");
        assert_eq!(s.get("events").await.unwrap().rows.len(), 1);

        s.edit(RedisEdit::SetString { key: "greeting".into(), value: "hi".into() }).await.unwrap();
        let g = s.get("greeting").await.unwrap();
        assert_eq!(g.rows[0][0], "hi");
        assert!(g.ttl > 0, "KEEPTTL keeps the expiry");
        s.edit(RedisEdit::HashSet { key: "user:1".into(), field: "plan".into(), value: "free".into() }).await.unwrap();
        s.edit(RedisEdit::ListSet { key: "queue".into(), index: 0, value: "z".into() }).await.unwrap();
        s.edit(RedisEdit::ZAdd { key: "board".into(), member: "cy".into(), score: 5.0 }).await.unwrap();
        assert_eq!(s.get("board").await.unwrap().rows[0][0], "cy");
        assert!(s.edit(RedisEdit::Rename { key: "tags".into(), to: "queue".into() }).await.is_err(), "won't overwrite");
        s.edit(RedisEdit::Rename { key: "tags".into(), to: "labels".into() }).await.unwrap();
        s.edit(RedisEdit::Delete { keys: vec!["labels".into(), "events".into()] }).await.unwrap();
        assert_eq!(s.scan("*", 0, 100).await.unwrap().keys.len(), 4);

        assert!(matches!(s.command("SET x 1", false).await, Err(AppError::Msg(m)) if m == "needs-confirmation"));
        assert!(s.command("GET greeting", false).await.unwrap().contains("hi"));
        let ov = s.overview().await.unwrap();
        assert!(!ov.version.is_empty());
        s.command("FLUSHDB", true).await.unwrap();
    }
}
