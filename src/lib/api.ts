import { invoke } from "@tauri-apps/api/core";

export type Driver = "postgres" | "mysql" | "sqlite" | "redis" | "mssql" | "clickhouse" | "duckdb" | "oracle" | "mongodb";

/** Engines that open a local file in-process: no host, port, SSH or SSL. */
export const isLocal = (d: Driver) => d === "sqlite" || d === "duckdb";
/** Drivers that speak SQL (tables, queries, structure). */
type SqlDriver = Exclude<Driver, "redis" | "mongodb">;

/** Engines with tables and SQL (Redis and MongoDB have their own workspaces). */
export const isSql = (d: Driver): d is SqlDriver => d !== "redis" && d !== "mongodb";
export type SslMode = "disable" | "prefer" | "require" | "verify-full";
export type EnvTag = "" | "local" | "development" | "staging" | "production";

type SshAuth = "password" | "key" | "agent";

export interface SshConfig {
  enabled: boolean;
  host: string;
  port: number;
  user: string;
  auth: SshAuth;
  /** Path to a private key file; the key is never copied. */
  keyPath: string;
}

export interface ConnectionConfig {
  id: string;
  name: string;
  driver: Driver;
  /** Preset id from lib/providers (e.g. "supabase"). */
  provider: string;
  host: string;
  port: number;
  user: string;
  /** Only sent to the backend; never returned. undefined = keep stored, "" = clear. */
  password?: string;
  database: string;
  sslMode: SslMode;
  sslCa: string;
  sslCert: string;
  sslKey: string;
  ssh: SshConfig;
  /** SSH password or key passphrase. Same rules as `password`. */
  sshSecret?: string;
  color: string;
  tag: EnvTag;
  readOnly: boolean;
  /** Share (read-only) with AI assistants through `tabla mcp`. */
  aiAccess: boolean;
}

export interface EnvUrl {
  file: string;
  key: string;
  url: string;
}

export interface ColumnMeta {
  name: string;
  typeName: string;
}

export interface ResultSet {
  columns: ColumnMeta[];
  rows: (string | null)[][];
  rowsAffected: number;
  truncated: boolean;
}

export interface TableInfo {
  name: string;
  kind: "table" | "view" | "materialized" | "foreign";
  rowsEstimate: number | null;
}

export interface ColumnInfo {
  name: string;
  dataType: string;
  nullable: boolean;
  defaultValue: string | null;
  primaryKey: boolean;
  /** "PRI" | "UNI" | "MUL" | "" */
  key: string;
  extra: string;
}

interface IndexInfo {
  name: string;
  definition: string;
  unique: boolean;
  primary: boolean;
  method: string;
  columns: string;
}

export interface ForeignKey {
  name: string;
  schema: string;
  table: string;
  columns: string[];
  refSchema: string;
  refTable: string;
  refColumns: string[];
  onUpdate: string;
  onDelete: string;
}

export interface TriggerInfo {
  name: string;
  timing: string;
  event: string;
  enabled: boolean;
  definition: string;
}

export interface TableRelations {
  outgoing: ForeignKey[];
  incoming: ForeignKey[];
  triggers: TriggerInfo[];
}

interface MongoOverview {
  version: string;
  databases: string[];
  current: string;
}
export interface MongoCollection {
  name: string;
  kind: "collection" | "view" | "timeseries";
}
interface MongoFindArgs {
  db: string;
  collection: string;
  filter: string;
  sort: string;
  projection: string;
  skip: number;
  limit: number;
}
export interface MongoPage {
  columns: string[];
  rows: (string | null)[][];
  docs: string[];
  ids: string[];
  total: number | null;
}
export interface MongoIndex {
  name: string;
  keys: string;
  unique: boolean;
  options: string;
}

export interface RedisKey {
  key: string;
  type: string;
  /** ms; -1 = no expiry */
  ttl: number;
}

interface RedisScanPage {
  cursor: number;
  keys: RedisKey[];
}

export interface RedisValue {
  type: string;
  ttl: number;
  length: number;
  rows: string[][];
  truncated: boolean;
}

interface RedisOverview {
  version: string;
  mode: string;
  usedMemory: string;
  databases: { index: number; keys: number }[];
}

export type RedisEdit =
  | { op: "setString"; key: string; value: string }
  | { op: "hashSet"; key: string; field: string; value: string }
  | { op: "hashDel"; key: string; field: string }
  | { op: "listSet"; key: string; index: number; value: string }
  | { op: "listPush"; key: string; value: string }
  | { op: "listRemove"; key: string; value: string }
  | { op: "setAdd"; key: string; member: string }
  | { op: "setRemove"; key: string; member: string }
  | { op: "zAdd"; key: string; member: string; score: number }
  | { op: "zRemove"; key: string; member: string }
  | { op: "delete"; keys: string[] }
  | { op: "expire"; key: string; seconds: number | null }
  | { op: "rename"; key: string; to: string };

export interface UpdateInfo {
  /** This build can download, verify and install the release itself. */
  canInstall: boolean;
  current: string;
  latest: string;
  available: boolean;
  url: string;
  notes: string;
  published: string;
}

export interface OracleClientStatus {
  ready: boolean;
  version: string | null;
  error: string | null;
  canInstall: boolean;
  restartNeeded: boolean;
  licenseUrl: string;
}

export interface UserInfo {
  name: string;
  host: string;
  canLogin: boolean;
  superuser: boolean;
  createDb: boolean;
  createRole: boolean;
  locked: boolean;
  validUntil: string | null;
  memberOf: string[];
}

export interface TableDetails {
  rowsEstimate: number | null;
  totalBytes: number | null;
  engine: string | null;
  collation: string | null;
  comment: string | null;
  created: string | null;
  updated: string | null;
  ddl: string | null;
  stats: { label: string; value: string; kind: "bytes" | "number" | "text" }[];
}

export interface TableStructure {
  columns: ColumnInfo[];
  indexes: IndexInfo[];
}

export interface TablePage {
  result: ResultSet;
  total: number | null;
  sql: string;
}

interface SessionInfo {
  sessionId: string;
  serverVersion: string;
  schemas: string[];
  currentSchema: string | null;
}

export interface QueryOutcome {
  results: ResultSet[];
  elapsedMs: number;
}

export interface CellValue {
  column: string;
  value: string | null;
}

export type RowEdit =
  | { kind: "update"; key: CellValue[]; values: CellValue[] }
  | { kind: "insert"; values: CellValue[] }
  | { kind: "delete"; key: CellValue[] };

export interface AlterColumn {
  name: string;
  dataType: string;
  nullable: boolean;
  /** Raw SQL expression; null = no default. */
  defaultValue: string | null;
}

export interface AlterRequest {
  schema: string;
  table: string;
  changes: { original: AlterColumn | null; column: AlterColumn | null }[];
  /** Physical order (MySQL only); empty = unchanged. */
  order: string[];
}

interface FetchArgs {
  schema: string;
  table: string;
  filter?: string | null;
  sort?: { column: string; desc: boolean } | null;
  limit: number;
  offset: number;
}

export const api = {
  listConnections: () => invoke<ConnectionConfig[]>("list_connections"),
  saveConnection: (config: ConnectionConfig) => invoke<ConnectionConfig>("save_connection", { config }),
  deleteConnection: (id: string) => invoke<void>("delete_connection", { id }),
  testConnection: (config: ConnectionConfig) => invoke<string>("test_connection", { config }),
  openSession: (config: ConnectionConfig) => invoke<SessionInfo>("open_session", { config }),
  closeSession: (sessionId: string) => invoke<void>("close_session", { sessionId }),
  listSchemas: (sessionId: string) => invoke<string[]>("list_schemas", { sessionId }),
  listTables: (sessionId: string, schema: string) => invoke<TableInfo[]>("list_tables", { sessionId, schema }),
  tableStructure: (sessionId: string, schema: string, table: string) =>
    invoke<TableStructure>("table_structure", { sessionId, schema, table }),
  mongoOverview: (sessionId: string) => invoke<MongoOverview>("mongo_overview", { sessionId }),
  mongoCollections: (sessionId: string, db: string) => invoke<MongoCollection[]>("mongo_collections", { sessionId, db }),
  mongoFind: (sessionId: string, args: MongoFindArgs) => invoke<MongoPage>("mongo_find", { sessionId, args }),
  mongoInsert: (sessionId: string, db: string, collection: string, json: string) => invoke<string>("mongo_insert", { sessionId, db, collection, json }),
  mongoReplace: (sessionId: string, db: string, collection: string, id: string, json: string) =>
    invoke<void>("mongo_replace", { sessionId, db, collection, id, json }),
  mongoSetField: (sessionId: string, db: string, collection: string, id: string, field: string, value: string | null) =>
    invoke<void>("mongo_set_field", { sessionId, db, collection, id, field, value }),
  mongoDelete: (sessionId: string, db: string, collection: string, ids: string[]) => invoke<number>("mongo_delete", { sessionId, db, collection, ids }),
  mongoIndexes: (sessionId: string, db: string, collection: string) => invoke<MongoIndex[]>("mongo_indexes", { sessionId, db, collection }),
  mongoShell: (sessionId: string, db: string, line: string, allowWrite: boolean) => invoke<string>("mongo_shell", { sessionId, db, line, allowWrite }),
  redisOverview: (sessionId: string) => invoke<RedisOverview>("redis_overview", { sessionId }),
  redisSelect: (sessionId: string, db: number) => invoke<void>("redis_select", { sessionId, db }),
  redisScan: (sessionId: string, pattern: string, cursor: number) => invoke<RedisScanPage>("redis_scan", { sessionId, pattern, cursor }),
  redisGet: (sessionId: string, key: string) => invoke<RedisValue>("redis_get", { sessionId, key }),
  redisEdit: (sessionId: string, edit: RedisEdit) => invoke<void>("redis_edit", { sessionId, edit }),
  redisCommand: (sessionId: string, line: string, allowWrite: boolean) => invoke<string>("redis_command", { sessionId, line, allowWrite }),
  listUsers: (sessionId: string) => invoke<UserInfo[]>("list_users", { sessionId }),
  userGrants: (sessionId: string, name: string, host: string) => invoke<string[]>("user_grants", { sessionId, name, host }),
  tableRelations: (sessionId: string, schema: string, table: string) =>
    invoke<TableRelations>("table_relations", { sessionId, schema, table }),
  tableDetails: (sessionId: string, schema: string, table: string) =>
    invoke<TableDetails>("table_details", { sessionId, schema, table }),
  fetchRows: (sessionId: string, args: FetchArgs) => invoke<TablePage>("fetch_rows", { sessionId, args }),
  runQuery: (sessionId: string, sql: string, opts?: { queryId?: string; maxRows?: number }) =>
    invoke<QueryOutcome>("run_query", { sessionId, sql, queryId: opts?.queryId ?? null, maxRows: opts?.maxRows ?? null }),
  cancelQuery: (sessionId: string, queryId: string) => invoke<boolean>("cancel_query", { sessionId, queryId }),
  applyEdits: (sessionId: string, schema: string, table: string, edits: RowEdit[]) =>
    invoke<number>("apply_edits", { sessionId, schema, table, edits }),
  planAlter: (sessionId: string, req: AlterRequest) => invoke<string[]>("plan_alter", { sessionId, req }),
  applyAlter: (sessionId: string, req: AlterRequest) => invoke<string[]>("apply_alter", { sessionId, req }),
  pickCsvFile: () => invoke<{ name: string; contents: string } | null>("pick_csv_file"),
  pickSqliteFile: () => invoke<string | null>("pick_sqlite_file"),
  pickDuckdbFile: () => invoke<string | null>("pick_duckdb_file"),
  oracleClientStatus: () => invoke<OracleClientStatus>("oracle_client_status"),
  /** Downloads Instant Client; resolves to true when Tabla must restart to load it. */
  installOracleClient: () => invoke<boolean>("install_oracle_client"),
  restartApp: () => invoke<void>("restart_app"),
  loadAppState: () => invoke<Record<string, unknown>>("load_app_state"),
  saveAppState: (state: Record<string, unknown>) => invoke<void>("save_app_state", { state }),
  pickFile: (kind: "ssh-key" | "cert") => invoke<string | null>("pick_file", { kind }),
  exportFile: (defaultName: string, contents: string) =>
    invoke<string | null>("export_file", { defaultName, contents }),
  scanEnvFiles: () => invoke<EnvUrl[] | null>("scan_env_files"),
  checkUpdate: () => invoke<UpdateInfo>("check_update"),
  /** Installs the latest release and restarts; resolves only if it fails. */
  installUpdate: () => invoke<void>("install_update"),
  openProjectPage: (url: string) => invoke<void>("open_project_page", { url }),
  mcpCommand: () => invoke<{ binary: string; installed: boolean }>("mcp_command"),
};

export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
