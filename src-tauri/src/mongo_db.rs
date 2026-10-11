//! MongoDB sessions: database and collection browser, document grid with
//! filters, document editing and a small mongosh-style shell.
//!
//! Input accepts what people paste from mongosh and docs: relaxed JSON
//! (unquoted keys, single quotes) plus ObjectId(), ISODate(), NumberLong()…
//! Every read is bounded, and writes are refused on read-only connections.

use std::time::{Duration, Instant};

use futures_util::TryStreamExt;
use mongodb::{
    Client,
    bson::{Bson, Document, doc},
    options::{ClientOptions, Credential, Tls, TlsOptions},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    config::{ConnectionConfig, SslMode},
    error::{AppError, AppResult},
};

const MAX_DOCS: i64 = 1000;
const SHELL_DOCS: usize = 200;

pub struct MongoSession {
    client: Client,
    pub read_only: bool,
    pub default_db: String,
    pub tunnel: Option<crate::ssh::Tunnel>,
    log: std::sync::Mutex<std::collections::VecDeque<crate::db::LogEntry>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MongoOverview {
    pub version: String,
    pub databases: Vec<String>,
    pub current: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionInfo {
    pub name: String,
    /// "collection" | "view" | "timeseries"
    pub kind: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindArgs {
    pub db: String,
    pub collection: String,
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub projection: String,
    pub skip: u64,
    pub limit: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindPage {
    /// Top-level fields across the page, `_id` first.
    pub columns: Vec<String>,
    /// One display string per column (None = field missing or null).
    pub rows: Vec<Vec<Option<String>>>,
    /// Each document as pretty relaxed Extended JSON.
    pub docs: Vec<String>,
    /// Each document's `_id` as Extended JSON, used as the key for edits.
    pub ids: Vec<String>,
    /// Matching documents, when the count finished quickly.
    pub total: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub keys: String,
    pub unique: bool,
    pub options: String,
}

impl MongoSession {
    pub async fn open(cfg: &ConnectionConfig, password: Option<String>) -> AppResult<Self> {
        let host = if cfg.host.trim().is_empty() { "localhost".to_string() } else { cfg.host.trim().to_string() };
        // Port left empty on a real host name means an Atlas-style SRV record.
        let srv = cfg.port == 0 && host.contains('.') && host.parse::<std::net::IpAddr>().is_err() && !cfg.ssh.enabled;
        if cfg.ssh.enabled {
            let secret = crate::config::resolve_ssh_secret(cfg)?;
            let tunnel = crate::ssh::open(&cfg.ssh, secret, &host, cfg.port_or_default()).await?;
            let mut s = Self::connect(cfg, password, "127.0.0.1", tunnel.local_port, false).await?;
            s.tunnel = Some(tunnel);
            return Ok(s);
        }
        Self::connect(cfg, password, &host, cfg.port_or_default(), srv).await
    }

    async fn connect(cfg: &ConnectionConfig, password: Option<String>, host: &str, port: u16, srv: bool) -> AppResult<Self> {
        let host_part = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
        let uri = if srv { format!("mongodb+srv://{host_part}/") } else { format!("mongodb://{host_part}:{port}/") };
        let mut o = ClientOptions::parse(uri).await.map_err(m)?;
        o.app_name = Some("Tabla".into());
        o.connect_timeout = Some(Duration::from_secs(10));
        o.server_selection_timeout = Some(Duration::from_secs(10));
        if !srv {
            // One address: talk to it directly (also required through SSH tunnels).
            o.direct_connection = Some(true);
        }
        if !cfg.user.trim().is_empty() {
            let mut c = Credential::default();
            c.username = Some(cfg.user.trim().to_string());
            c.password = password;
            c.source = Some(o.credential.as_ref().and_then(|c| c.source.clone()).unwrap_or_else(|| "admin".into()));
            o.credential = Some(c);
        }
        let tls = cfg.ssl_mode != SslMode::Disable || srv;
        if tls {
            let mut t = TlsOptions::default();
            let has_ca = !cfg.ssl_ca.trim().is_empty();
            if matches!(cfg.ssl_mode, SslMode::Prefer | SslMode::Require) && !has_ca {
                t.allow_invalid_certificates = Some(true);
            }
            if has_ca {
                t.ca_file_path = Some(cfg.ssl_ca.trim().into());
            }
            // MongoDB expects the client certificate and key in one PEM file.
            if !cfg.ssl_cert.trim().is_empty() {
                t.cert_key_file_path = Some(cfg.ssl_cert.trim().into());
            }
            o.tls = Some(Tls::Enabled(t));
        } else {
            o.tls = Some(Tls::Disabled);
        }
        let client = Client::with_options(o).map_err(m)?;
        client.database("admin").run_command(doc! { "ping": 1 }).await.map_err(m)?;
        let default_db = if cfg.database.trim().is_empty() { "test".into() } else { cfg.database.trim().to_string() };
        Ok(Self { client, read_only: cfg.read_only, default_db, tunnel: None, log: Default::default() })
    }

    pub async fn close(&self) {
        self.client.clone().shutdown().await;
    }

    fn record(&self, line: &str, started: Instant, outcome: Result<String, String>) {
        let mut log = self.log.lock().unwrap();
        let id = log.back().map(|e| e.id + 1).unwrap_or(1);
        let mut text = redact_pwd(line);
        if text.len() > 4000 {
            let cut = (0..=4000).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
            text.truncate(cut);
            text.push('…');
        }
        let (ok, info) = match outcome {
            Ok(i) => (true, i),
            Err(e) => (false, e),
        };
        log.push_back(crate::db::LogEntry {
            id,
            at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0),
            sql: text,
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

    fn writable(&self) -> AppResult<()> {
        if self.read_only { Err(AppError::msg("This connection is read-only")) } else { Ok(()) }
    }

    pub async fn overview(&self) -> AppResult<MongoOverview> {
        let info = self.client.database("admin").run_command(doc! { "buildInfo": 1 }).await.map_err(m)?;
        let version = info.get_str("version").unwrap_or_default().to_string();
        // Users limited to one database may not be allowed to list them all.
        let mut databases = match self.client.list_database_names().authorized_databases(true).await {
            Ok(d) => d,
            Err(_) => vec![],
        };
        if !databases.contains(&self.default_db) {
            databases.push(self.default_db.clone());
        }
        databases.retain(|d| !matches!(d.as_str(), "admin" | "local" | "config") || *d == self.default_db);
        databases.sort();
        Ok(MongoOverview { version, databases, current: self.default_db.clone() })
    }

    pub async fn collections(&self, db: &str) -> AppResult<Vec<CollectionInfo>> {
        let started = Instant::now();
        let res = self.client.database(db).list_collections().await;
        let mut cur = res.map_err(m)?;
        let mut out = Vec::new();
        while let Some(spec) = cur.try_next().await.map_err(m)? {
            let kind = match spec.collection_type {
                mongodb::results::CollectionType::View => "view",
                mongodb::results::CollectionType::Timeseries => "timeseries",
                _ => "collection",
            };
            if spec.name.starts_with("system.") {
                continue;
            }
            out.push(CollectionInfo { name: spec.name, kind: kind.into() });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        self.record(&format!("db.getSiblingDB({}).getCollectionInfos()", js_str(db)), started, Ok(format!("{} collections", out.len())));
        Ok(out)
    }

    pub async fn find(&self, a: &FindArgs) -> AppResult<FindPage> {
        let filter = parse_doc(&a.filter)?;
        let sort = parse_doc(&a.sort)?;
        let projection = parse_doc(&a.projection)?;
        let coll = self.client.database(&a.db).collection::<Document>(&a.collection);
        let started = Instant::now();
        let label = format!(
            "db.{}.find({}){}{}.skip({}).limit({})",
            a.collection,
            if a.filter.trim().is_empty() { "{}" } else { a.filter.trim() },
            if projection.is_empty() { String::new() } else { format!(".projection({})", a.projection.trim()) },
            if sort.is_empty() { String::new() } else { format!(".sort({})", a.sort.trim()) },
            a.skip,
            a.limit
        );
        let mut q = coll.find(filter.clone()).skip(a.skip).limit(a.limit.clamp(1, MAX_DOCS)).max_time(Duration::from_secs(30));
        if !sort.is_empty() {
            q = q.sort(sort);
        }
        if !projection.is_empty() {
            q = q.projection(projection);
        }
        let res: Result<Vec<Document>, _> = async { q.await?.try_collect().await }.await;
        self.record(&label, started, res.as_ref().map(|d| format!("{} documents", d.len())).map_err(|e| m(e.clone()).to_string()));
        let docs = res.map_err(m)?;
        let total = if filter.is_empty() {
            coll.estimated_document_count().await.ok()
        } else {
            coll.count_documents(filter).max_time(Duration::from_secs(3)).await.ok()
        };
        Ok(page(docs, total))
    }

    pub async fn insert(&self, db: &str, collection: &str, json: &str) -> AppResult<String> {
        self.writable()?;
        let d = parse_doc(json)?;
        let started = Instant::now();
        let label = format!("db.{collection}.insertOne({})", compact(&d));
        let res = self.client.database(db).collection::<Document>(collection).insert_one(d).await;
        self.record(&label, started, res.as_ref().map(|_| "inserted".into()).map_err(|e| m(e.clone()).to_string()));
        Ok(ext_json(res.map_err(m)?.inserted_id, false))
    }

    /// Replaces a whole document, matched by `_id`.
    pub async fn replace(&self, db: &str, collection: &str, id: &str, json: &str) -> AppResult<()> {
        self.writable()?;
        let id = parse_value(id)?;
        let mut d = parse_doc(json)?;
        match d.get("_id") {
            Some(v) if *v != id => return Err(AppError::msg("The _id can't be changed. Insert a new document instead.")),
            _ => {
                d.insert("_id", id.clone());
            }
        }
        let started = Instant::now();
        let label = format!("db.{collection}.replaceOne({{_id: {}}}, …)", compact_bson(&id));
        let res = self.client.database(db).collection::<Document>(collection).replace_one(doc! { "_id": id }, d).await;
        self.record(&label, started, res.as_ref().map(|r| format!("{} modified", r.modified_count)).map_err(|e| m(e.clone()).to_string()));
        if res.map_err(m)?.matched_count != 1 {
            return Err(gone());
        }
        Ok(())
    }

    /// Sets (or with `value: None`, unsets) one top-level field.
    pub async fn set_field(&self, db: &str, collection: &str, id: &str, field: &str, value: Option<&str>) -> AppResult<()> {
        self.writable()?;
        if field == "_id" {
            return Err(AppError::msg("The _id can't be changed"));
        }
        let id = parse_value(id)?;
        let update = match value {
            Some(v) => doc! { "$set": { field: parse_cell(v) } },
            None => doc! { "$unset": { field: "" } },
        };
        let started = Instant::now();
        let label = format!("db.{collection}.updateOne({{_id: {}}}, {})", compact_bson(&id), compact(&update));
        let res = self.client.database(db).collection::<Document>(collection).update_one(doc! { "_id": id }, update).await;
        self.record(&label, started, res.as_ref().map(|r| format!("{} modified", r.modified_count)).map_err(|e| m(e.clone()).to_string()));
        if res.map_err(m)?.matched_count != 1 {
            return Err(gone());
        }
        Ok(())
    }

    pub async fn delete(&self, db: &str, collection: &str, ids: &[String]) -> AppResult<u64> {
        self.writable()?;
        let ids: Vec<Bson> = ids.iter().map(|i| parse_value(i)).collect::<AppResult<_>>()?;
        let started = Instant::now();
        let label = format!("db.{collection}.deleteMany({{_id: {{$in: [{} ids]}}}})", ids.len());
        let n = ids.len() as u64;
        let res = self.client.database(db).collection::<Document>(collection).delete_many(doc! { "_id": { "$in": ids } }).await;
        self.record(&label, started, res.as_ref().map(|r| format!("{} deleted", r.deleted_count)).map_err(|e| m(e.clone()).to_string()));
        let deleted = res.map_err(m)?.deleted_count;
        if deleted != n {
            return Err(AppError::msg(format!("Deleted {deleted} of {n} documents; the others no longer exist. Refresh to see the current data.")));
        }
        Ok(deleted)
    }

    pub async fn indexes(&self, db: &str, collection: &str) -> AppResult<Vec<IndexInfo>> {
        let mut cur = self.client.database(db).collection::<Document>(collection).list_indexes().await.map_err(m)?;
        let mut out = Vec::new();
        while let Some(ix) = cur.try_next().await.map_err(m)? {
            let o = ix.options.unwrap_or_default();
            let name = o.name.clone().unwrap_or_default();
            let unique = o.unique.unwrap_or(false);
            let mut extra = serde_json::to_value(&o).unwrap_or(Value::Null);
            if let Value::Object(map) = &mut extra {
                map.retain(|k, v| !v.is_null() && k != "name" && k != "unique" && k != "v");
            }
            out.push(IndexInfo {
                name,
                keys: compact(&ix.keys),
                unique,
                options: if extra.as_object().is_some_and(|m| m.is_empty()) { String::new() } else { extra.to_string() },
            });
        }
        Ok(out)
    }

    /// Runs one mongosh-style line. Writes need `allow_write` (the UI asks first).
    pub async fn shell(&self, db: &str, line: &str, allow_write: bool) -> AppResult<String> {
        let started = Instant::now();
        let res = self.shell_inner(db, line.trim(), allow_write).await;
        self.record(line.trim(), started, res.as_ref().map(|_| "ok".into()).map_err(|e| e.to_string()));
        res
    }

    async fn shell_inner(&self, db: &str, line: &str, allow_write: bool) -> AppResult<String> {
        let line = line.trim_end_matches(';').trim();
        if line.is_empty() {
            return Ok(String::new());
        }
        let lower = line.to_ascii_lowercase();
        if lower == "show dbs" || lower == "show databases" {
            return Ok(self.overview().await?.databases.join("\n"));
        }
        if lower == "show collections" || lower == "show tables" {
            return Ok(self.collections(db).await?.into_iter().map(|c| c.name).collect::<Vec<_>>().join("\n"));
        }
        let database = self.client.database(db);
        // A bare document is a database command: {"dbStats": 1}
        if line.starts_with('{') {
            let cmd = parse_doc(line)?;
            let name = cmd.keys().next().cloned().unwrap_or_default();
            if !is_read_command(&name, &cmd) {
                self.writable()?;
                if !allow_write {
                    return Err(needs_confirm());
                }
            }
            let out = database.run_command(cmd).await.map_err(m)?;
            return Ok(pretty(Bson::Document(out)));
        }
        let call = parse_call(line)?;
        let coll = database.collection::<Document>(&call.collection);
        let arg = |i: usize| -> AppResult<Document> {
            match call.args.get(i) {
                None => Ok(Document::new()),
                Some(Bson::Document(d)) => Ok(d.clone()),
                Some(other) => Err(AppError::msg(format!("Expected an object, got {}", compact_bson(other)))),
            }
        };
        let write = matches!(
            call.method.as_str(),
            "insertOne" | "insertMany" | "updateOne" | "updateMany" | "replaceOne" | "deleteOne" | "deleteMany" | "drop" | "createIndex" | "dropIndex"
        ) || (call.method == "aggregate" && pipeline_writes(&call.args));
        if write {
            self.writable()?;
            if !allow_write {
                return Err(needs_confirm());
            }
        }
        let out: Bson = match call.method.as_str() {
            "find" | "findOne" => {
                let mut q = coll.find(arg(0)?).max_time(Duration::from_secs(30));
                let proj = arg(1)?;
                if !proj.is_empty() {
                    q = q.projection(proj);
                }
                for (name, v) in &call.chain {
                    q = match (name.as_str(), v) {
                        ("sort", Bson::Document(d)) => q.sort(d.clone()),
                        ("limit", v) => q.limit(as_i64(v)?),
                        ("skip", v) => q.skip(as_i64(v)? as u64),
                        ("projection", Bson::Document(d)) => q.projection(d.clone()),
                        _ => return Err(AppError::msg(format!("Unsupported cursor method .{name}()"))),
                    };
                }
                let limit = if call.method == "findOne" { 1 } else { SHELL_DOCS as i64 };
                let docs: Vec<Document> = q.limit(limit.min(call.chain.iter().find(|c| c.0 == "limit").map(|c| as_i64(&c.1).unwrap_or(limit)).unwrap_or(limit)))
                    .await
                    .map_err(m)?
                    .try_collect()
                    .await
                    .map_err(m)?;
                if call.method == "findOne" {
                    docs.into_iter().next().map(Bson::Document).unwrap_or(Bson::Null)
                } else {
                    Bson::Array(docs.into_iter().map(Bson::Document).collect())
                }
            }
            "aggregate" => {
                let pipeline: Vec<Document> = match call.args.first() {
                    Some(Bson::Array(a)) => a.iter().filter_map(|s| s.as_document().cloned()).collect(),
                    _ => return Err(AppError::msg("aggregate takes an array of stages")),
                };
                let docs: Vec<Document> = coll.aggregate(pipeline).max_time(Duration::from_secs(30)).await.map_err(m)?.try_collect().await.map_err(m)?;
                Bson::Array(docs.into_iter().take(SHELL_DOCS).map(Bson::Document).collect())
            }
            "countDocuments" => Bson::Int64(coll.count_documents(arg(0)?).await.map_err(m)? as i64),
            "estimatedDocumentCount" => Bson::Int64(coll.estimated_document_count().await.map_err(m)? as i64),
            "distinct" => {
                let field = call.args.first().and_then(Bson::as_str).ok_or_else(|| AppError::msg("distinct takes a field name"))?;
                Bson::Array(coll.distinct(field, arg(1)?).await.map_err(m)?)
            }
            "getIndexes" => Bson::Array(
                self.indexes(db, &call.collection)
                    .await?
                    .into_iter()
                    .map(|i| Bson::Document(doc! { "name": i.name, "key": i.keys, "unique": i.unique }))
                    .collect(),
            ),
            "stats" => Bson::Document(database.run_command(doc! { "collStats": &call.collection }).await.map_err(m)?),
            "insertOne" => Bson::Document(doc! { "insertedId": coll.insert_one(arg(0)?).await.map_err(m)?.inserted_id }),
            "insertMany" => {
                let docs: Vec<Document> = match call.args.first() {
                    Some(Bson::Array(a)) => a.iter().filter_map(|s| s.as_document().cloned()).collect(),
                    _ => return Err(AppError::msg("insertMany takes an array of documents")),
                };
                Bson::Document(doc! { "insertedCount": coll.insert_many(docs).await.map_err(m)?.inserted_ids.len() as i64 })
            }
            "updateOne" | "updateMany" => {
                let (f, u) = (arg(0)?, arg(1)?);
                let r = if call.method == "updateOne" { coll.update_one(f, u).await } else { coll.update_many(f, u).await }.map_err(m)?;
                Bson::Document(doc! { "matchedCount": r.matched_count as i64, "modifiedCount": r.modified_count as i64 })
            }
            "replaceOne" => {
                let r = coll.replace_one(arg(0)?, arg(1)?).await.map_err(m)?;
                Bson::Document(doc! { "matchedCount": r.matched_count as i64, "modifiedCount": r.modified_count as i64 })
            }
            "deleteOne" | "deleteMany" => {
                let f = arg(0)?;
                let r = if call.method == "deleteOne" { coll.delete_one(f).await } else { coll.delete_many(f).await }.map_err(m)?;
                Bson::Document(doc! { "deletedCount": r.deleted_count as i64 })
            }
            "createIndex" => {
                let mut spec = doc! { "key": arg(0)? };
                spec.extend(arg(1)?);
                if !spec.contains_key("name") {
                    let name: Vec<String> = arg(0)?.iter().map(|(k, v)| format!("{k}_{}", compact_bson(v))).collect();
                    spec.insert("name", name.join("_"));
                }
                let r = database.run_command(doc! { "createIndexes": &call.collection, "indexes": [spec] }).await.map_err(m)?;
                Bson::Document(r)
            }
            "dropIndex" => {
                let name = call.args.first().and_then(Bson::as_str).ok_or_else(|| AppError::msg("dropIndex takes the index name"))?;
                Bson::Document(database.run_command(doc! { "dropIndexes": &call.collection, "index": name }).await.map_err(m)?)
            }
            "drop" => {
                coll.drop().await.map_err(m)?;
                Bson::Boolean(true)
            }
            other => return Err(AppError::msg(format!("Unsupported method .{other}(). Try find, aggregate, countDocuments, distinct, insertOne, updateOne, deleteOne, or a {{command}} document."))),
        };
        Ok(pretty(out))
    }
}

fn m(e: mongodb::error::Error) -> AppError {
    use mongodb::error::ErrorKind;
    let msg = match e.kind.as_ref() {
        ErrorKind::Command(c) => c.message.clone(),
        ErrorKind::Authentication { message, .. } => format!("Authentication failed: {message}"),
        ErrorKind::ServerSelection { message, .. } => {
            // The driver's message lists every server; the first error is what matters.
            let first = message.split("Error: ").nth(1).unwrap_or(message).split(", labels").next().unwrap_or(message);
            format!("Couldn't reach the server: {}", first.trim_end_matches(['}', ' ', ']']))
        }
        ErrorKind::Write(mongodb::error::WriteFailure::WriteError(w)) => w.message.clone(),
        _ => e.to_string(),
    };
    AppError::msg(msg)
}

fn gone() -> AppError {
    AppError::msg("That document no longer exists. Refresh to see the current data.")
}

fn needs_confirm() -> AppError {
    AppError::msg("CONFIRM_WRITE: this command changes data")
}

const READ_COMMANDS: &[&str] = &[
    "ping", "buildinfo", "hello", "ismaster", "serverstatus", "dbstats", "collstats", "listcollections", "listindexes",
    "listdatabases", "count", "distinct", "find", "explain", "connectionstatus", "getparameter", "hostinfo", "getlog",
    "currentop", "top", "validate", "datasize", "usersinfo", "rolesinfo", "getcmdlineopts", "replsetgetstatus",
];

fn is_read_command(name: &str, cmd: &Document) -> bool {
    let n = name.to_ascii_lowercase();
    READ_COMMANDS.contains(&n.as_str())
        || (n == "aggregate" && !cmd.get_array("pipeline").map(|p| pipeline_writes(&[Bson::Array(p.clone())])).unwrap_or(false))
}

/// $out and $merge stages write.
fn pipeline_writes(args: &[Bson]) -> bool {
    matches!(args.first(), Some(Bson::Array(stages)) if stages.iter().any(|s| s.as_document().is_some_and(|d| d.contains_key("$out") || d.contains_key("$merge"))))
}

fn as_i64(v: &Bson) -> AppResult<i64> {
    match v {
        Bson::Int32(n) => Ok(*n as i64),
        Bson::Int64(n) => Ok(*n),
        Bson::Double(f) => Ok(*f as i64),
        _ => Err(AppError::msg("Expected a number")),
    }
}

struct Call {
    collection: String,
    method: String,
    args: Vec<Bson>,
    chain: Vec<(String, Bson)>,
}

/// `db.users.find({age: {$gt: 30}}).sort({age: -1}).limit(5)`
/// and `db.getCollection("weird name").aggregate([...])`.
fn parse_call(line: &str) -> AppResult<Call> {
    let rest = line.strip_prefix("db.").ok_or_else(|| AppError::msg("Start with db.<collection>.<method>(…) or a {command} document"))?;
    let (collection, rest) = if let Some(r) = rest.strip_prefix("getCollection(") {
        let end = r.find(')').ok_or_else(|| AppError::msg("Missing ) after getCollection"))?;
        let name: String = json5::from_str(&r[..end]).map_err(|e| AppError::msg(format!("Collection name: {e}")))?;
        (name, r[end + 1..].strip_prefix('.').unwrap_or(&r[end + 1..]))
    } else {
        let dot = rest.find('.').ok_or_else(|| AppError::msg("Expected db.<collection>.<method>(…)"))?;
        (rest[..dot].to_string(), &rest[dot + 1..])
    };
    let mut calls = Vec::new();
    let mut s = rest;
    while !s.is_empty() {
        let open = s.find('(').ok_or_else(|| AppError::msg("Expected ( after the method name"))?;
        let name = s[..open].trim().trim_start_matches('.').to_string();
        let close = matching_paren(s, open).ok_or_else(|| AppError::msg(format!("Missing ) for .{name}(")))?;
        let args = parse_args(&s[open + 1..close])?;
        calls.push((name, args));
        s = s[close + 1..].trim();
    }
    let mut it = calls.into_iter();
    let (method, args) = it.next().ok_or_else(|| AppError::msg("Expected a method call"))?;
    let chain = it.map(|(n, mut a)| (n, if a.is_empty() { Bson::Null } else { a.remove(0) })).collect();
    Ok(Call { collection, method, args, chain })
}

fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    let (mut depth, mut i, mut quote) = (0i32, open, None::<u8>);
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'"' | b'\'' => quote = Some(c),
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

fn parse_args(inner: &str) -> AppResult<Vec<Bson>> {
    if inner.trim().is_empty() {
        return Ok(vec![]);
    }
    match parse_value(&format!("[{inner}]"))? {
        Bson::Array(a) => Ok(a),
        _ => unreachable!(),
    }
}

/// Rewrites mongosh helpers into Extended JSON: ObjectId("…") → {"$oid": "…"}.
fn shell_helpers(s: &str) -> String {
    const HELPERS: &[(&str, &str, bool)] = &[
        ("ObjectId(", "$oid", true),
        ("ISODate(", "$date", true),
        ("new Date(", "$date", true),
        ("NumberLong(", "$numberLong", true),
        ("NumberInt(", "$numberInt", true),
        ("NumberDecimal(", "$numberDecimal", true),
    ];
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut quote: Option<char> = None;
    'outer: while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        if let Some(q) = quote {
            out.push(c);
            if c == '\\' {
                if let Some(n) = s[i + 1..].chars().next() {
                    out.push(n);
                    i += 1 + n.len_utf8();
                    continue;
                }
            } else if c == q {
                quote = None;
            }
            i += c.len_utf8();
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            out.push(c);
            i += 1;
            continue;
        }
        for (name, key, _) in HELPERS {
            if s[i..].starts_with(name) {
                let open = i + name.len() - 1;
                if let Some(close) = matching_paren(s, open) {
                    let inner = s[open + 1..close].trim().trim_matches(|c| c == '"' || c == '\'');
                    out.push_str(&format!("{{\"{key}\": \"{}\"}}", inner.replace('"', "\\\"")));
                    i = close + 1;
                    continue 'outer;
                }
            }
        }
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn parse_value(text: &str) -> AppResult<Bson> {
    let v: Value = json5::from_str(&shell_helpers(text)).map_err(|e| AppError::msg(format!("Couldn't read that as JSON: {e}")))?;
    Bson::try_from(v).map_err(|e| AppError::msg(format!("Invalid Extended JSON: {e}")))
}

pub fn parse_doc(text: &str) -> AppResult<Document> {
    if text.trim().is_empty() {
        return Ok(Document::new());
    }
    match parse_value(text)? {
        Bson::Document(d) => Ok(d),
        other => Err(AppError::msg(format!("Expected an object like {{\"field\": 1}}, got {}", compact_bson(&other)))),
    }
}

/// A grid cell edit: JSON when it parses (numbers, true, {…}, ObjectId(…)), else a plain string.
fn parse_cell(text: &str) -> Bson {
    let t = text.trim();
    let looks_json = t.starts_with(['{', '[', '"']) || t == "true" || t == "false" || t == "null" || t.parse::<f64>().is_ok() || t.contains('(');
    if looks_json {
        if let Ok(b) = parse_value(t) {
            return b;
        }
    }
    Bson::String(text.to_string())
}

fn ext_json(b: Bson, pretty_print: bool) -> String {
    let v = b.into_relaxed_extjson();
    if pretty_print { serde_json::to_string_pretty(&v).unwrap_or_default() } else { v.to_string() }
}

fn pretty(b: Bson) -> String {
    ext_json(b, true)
}

fn compact(d: &Document) -> String {
    ext_json(Bson::Document(d.clone()), false)
}

fn compact_bson(b: &Bson) -> String {
    ext_json(b.clone(), false)
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// Grid text for one value: scalars as plain text, nested values as compact JSON.
fn display(b: &Bson) -> Option<String> {
    Some(match b {
        Bson::Null | Bson::Undefined => return None,
        Bson::String(s) => s.clone(),
        Bson::ObjectId(o) => o.to_hex(),
        Bson::DateTime(d) => d.try_to_rfc3339_string().unwrap_or_else(|_| d.to_string()),
        Bson::Int32(n) => n.to_string(),
        Bson::Int64(n) => n.to_string(),
        Bson::Double(f) => f.to_string(),
        Bson::Boolean(v) => v.to_string(),
        Bson::Decimal128(d) => d.to_string(),
        other => compact_bson(other),
    })
}

fn page(docs: Vec<Document>, total: Option<u64>) -> FindPage {
    let mut columns: Vec<String> = vec!["_id".into()];
    for d in &docs {
        for k in d.keys() {
            if !columns.contains(k) {
                columns.push(k.clone());
            }
        }
    }
    if docs.iter().all(|d| !d.contains_key("_id")) {
        columns.remove(0);
    }
    let rows = docs.iter().map(|d| columns.iter().map(|c| d.get(c).and_then(display)).collect()).collect();
    let ids = docs.iter().map(|d| d.get("_id").map(|v| compact_bson(v)).unwrap_or_default()).collect();
    let docs = docs.into_iter().map(|d| pretty(Bson::Document(d))).collect();
    FindPage { columns, rows, docs, ids, total }
}

/// Masks `pwd` values (createUser, updateUser…) so passwords never reach the Console log.
fn redact_pwd(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(p) = rest.find("pwd") {
        let (head, tail) = rest.split_at(p + 3);
        out.push_str(head);
        // pwd, "pwd" or 'pwd', then a colon and a quoted value.
        let after_key = tail.strip_prefix(['"', '\'']).unwrap_or(tail);
        let Some(value) = after_key.trim_start().strip_prefix(':').map(str::trim_start) else {
            rest = tail;
            continue;
        };
        let Some(q) = value.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            rest = tail;
            continue;
        };
        let end = value[1..].find(q).map(|i| i + 2).unwrap_or(value.len());
        out.push_str(&tail[..tail.len() - value.len()]);
        out.push(q);
        out.push_str("•••");
        out.push(q);
        rest = &value[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn masks_passwords() {
        assert_eq!(redact_pwd("{createUser: 'bob', pwd: 's3cr:et', roles: []}"), "{createUser: 'bob', pwd: '•••', roles: []}");
        assert_eq!(redact_pwd("{\"updateUser\": \"bob\", \"pwd\": \"x\"}"), "{\"updateUser\": \"bob\", \"pwd\": \"•••\"}");
        assert_eq!(redact_pwd("db.pwds.find({})"), "db.pwds.find({})");
    }

    #[test]
    fn reads_shell_syntax() {
        let d = parse_doc("{_id: ObjectId('65a1b2c3d4e5f60718293a4b'), name: 'Ann', at: ISODate(\"2024-01-02T03:04:05Z\"), n: NumberLong(5), s: 'ObjectId(x)'}").unwrap();
        assert_eq!(d.get_object_id("_id").unwrap().to_hex(), "65a1b2c3d4e5f60718293a4b");
        assert_eq!(d.get_str("name").unwrap(), "Ann");
        assert!(d.get_datetime("at").is_ok());
        assert_eq!(d.get_i64("n").unwrap(), 5);
        assert_eq!(d.get_str("s").unwrap(), "ObjectId(x)");
        assert!(parse_doc("").unwrap().is_empty());
        assert!(parse_doc("[1]").is_err());
    }

    #[test]
    fn parses_calls() {
        let c = parse_call("db.users.find({age: {$gt: 30}}, {name: 1}).sort({age: -1}).limit(5)").unwrap();
        assert_eq!((c.collection.as_str(), c.method.as_str(), c.args.len(), c.chain.len()), ("users", "find", 2, 2));
        assert_eq!(c.chain[1], ("limit".into(), Bson::Int32(5)));
        let c = parse_call("db.getCollection('my coll').countDocuments()").unwrap();
        assert_eq!((c.collection.as_str(), c.method.as_str()), ("my coll", "countDocuments"));
        let c = parse_call("db.o.aggregate([{$match: {s: 'a)b'}}, {$out: 'x'}])").unwrap();
        assert!(pipeline_writes(&c.args));
        assert!(parse_call("users.find()").is_err());
    }

    #[test]
    fn cell_values() {
        assert_eq!(parse_cell("42"), Bson::Int32(42));
        assert_eq!(parse_cell("true"), Bson::Boolean(true));
        assert_eq!(parse_cell("hello world"), Bson::String("hello world".into()));
        assert_eq!(parse_cell("{a: 1}"), Bson::Document(doc! { "a": 1 }));
        assert_eq!(parse_cell("(not json"), Bson::String("(not json".into()));
    }

    #[test]
    fn builds_pages() {
        let p = page(vec![doc! { "_id": 1, "a": "x" }, doc! { "_id": 2, "b": { "c": 1 } }], Some(2));
        assert_eq!(p.columns, vec!["_id", "a", "b"]);
        assert_eq!(p.rows[1], vec![Some("2".into()), None, Some("{\"c\":1}".into())]);
        assert_eq!(p.ids, vec!["1", "2"]);
    }
}

#[cfg(test)]
mod integration {
    //! Runs against a real MongoDB when TABLA_TEST_MONGO="host:port:user:database"
    //! is set (password in TABLA_TEST_MONGO_PASSWORD).
    use super::*;
    use crate::config::Driver;

    fn cfg() -> Option<(ConnectionConfig, Option<String>)> {
        let v = std::env::var("TABLA_TEST_MONGO").ok()?;
        let pw = std::env::var("TABLA_TEST_MONGO_PASSWORD").ok().filter(|p| !p.is_empty());
        let p: Vec<&str> = v.split(':').collect();
        let c = serde_json::from_value(serde_json::json!({
            "id": "mongo-test", "name": "m", "driver": Driver::Mongodb, "sslMode": "disable",
            "host": p[0], "port": p[1].parse::<u16>().unwrap(), "user": p[2], "database": p[3]
        }))
        .unwrap();
        Some((c, pw))
    }

    #[test]
    fn mongo_end_to_end() {
        crate::test_lock::big_stack(end_to_end());
    }

    async fn end_to_end() {
        let Some((c, pw)) = cfg() else { return };
        let s = MongoSession::open(&c, pw.clone()).await.unwrap();
        let db = "tabla_test";
        let _ = s.shell(db, "db.people.drop()", true).await;
        assert!(s.shell(db, "db.people.insertOne({a: 1})", false).await.unwrap_err().to_string().starts_with("CONFIRM_WRITE"));
        s.shell(db, "db.people.insertMany([{name: 'Ann', age: 31, tags: ['a']}, {name: 'Bob', age: 25}, {name: 'Cy', age: 40, at: ISODate('2024-01-02T03:04:05Z')}])", true)
            .await
            .unwrap();
        s.shell(db, "db.people.createIndex({age: 1}, {unique: false})", true).await.unwrap();

        let ov = s.overview().await.unwrap();
        assert!(!ov.version.is_empty());
        assert!(ov.databases.iter().any(|d| d == db));
        assert!(s.collections(db).await.unwrap().iter().any(|c| c.name == "people"));
        assert!(s.indexes(db, "people").await.unwrap().iter().any(|i| i.keys == "{\"age\":1}"));

        let find = |filter: &str, sort: &str| FindArgs { db: db.into(), collection: "people".into(), filter: filter.into(), sort: sort.into(), projection: String::new(), skip: 0, limit: 50 };
        let p = s.find(&find("{age: {$gte: 30}}", "{age: -1}")).await.unwrap();
        assert_eq!(p.total, Some(2));
        let name_col = p.columns.iter().position(|c| c == "name").unwrap();
        assert_eq!(p.rows[0][name_col].as_deref(), Some("Cy"));
        assert!(p.docs[0].contains("\"$date\""));
        let all = s.find(&find("", "")).await.unwrap();
        assert_eq!(all.total, Some(3));

        // Field edit, full replace, delete — all by _id.
        let ann = s.find(&find("{name: 'Ann'}", "")).await.unwrap();
        let id = ann.ids[0].clone();
        s.set_field(db, "people", &id, "age", Some("32")).await.unwrap();
        s.set_field(db, "people", &id, "nick", Some("annie")).await.unwrap();
        s.set_field(db, "people", &id, "tags", None).await.unwrap();
        let back = s.shell(db, "db.people.findOne({name: 'Ann'})", false).await.unwrap();
        assert!(back.contains("\"age\": 32") && back.contains("annie") && !back.contains("tags"), "{back}");
        s.replace(db, "people", &id, "{name: 'Ann B', age: 33}").await.unwrap();
        assert!(s.replace(db, "people", &id, "{_id: 5, name: 'x'}").await.is_err());
        let new_id = s.insert(db, "people", "{name: 'Dee'}").await.unwrap();
        assert_eq!(s.delete(db, "people", &[new_id.clone()]).await.unwrap(), 1);
        assert!(s.delete(db, "people", &[new_id]).await.is_err());
        assert!(s.set_field(db, "people", "{\"$oid\": \"000000000000000000000000\"}", "a", Some("1")).await.is_err());

        // Shell reads
        assert_eq!(s.shell(db, "db.people.countDocuments({age: {$lt: 30}})", false).await.unwrap(), "1");
        assert!(s.shell(db, "db.people.aggregate([{$group: {_id: null, n: {$sum: 1}}}])", false).await.unwrap().contains("\"n\": 3"));
        assert!(s.shell(db, "{dbStats: 1}", false).await.unwrap().contains("collections"));
        assert!(s.shell(db, "db.people.find().sort({age: 1}).limit(1)", false).await.unwrap().contains("Bob"));
        assert!(s.shell(db, "show collections", false).await.unwrap().contains("people"));
        let err = s.shell(db, "{nope: 1}", true).await.unwrap_err().to_string();
        assert!(err.to_lowercase().contains("no such command"), "{err}");

        // Read-only connections refuse writes before they reach the server.
        let mut ro = c.clone();
        ro.read_only = true;
        let r = MongoSession::open(&ro, pw).await.unwrap();
        assert!(r.insert(db, "people", "{a: 1}").await.is_err());
        assert!(r.shell(db, "db.people.deleteMany({})", true).await.is_err());
        assert!(r.shell(db, "db.people.aggregate([{$out: 'copy'}])", true).await.is_err());
        assert_eq!(r.find(&find("", "")).await.unwrap().total, Some(3));
        assert!(!s.log_since(0).is_empty());
        s.shell(db, "db.people.drop()", true).await.unwrap();
    }
}
