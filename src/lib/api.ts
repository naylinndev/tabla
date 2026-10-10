import { invoke } from "@tauri-apps/api/core";

export type Driver = "postgres" | "mysql" | "sqlite";
export type SslMode = "disable" | "prefer" | "require" | "verify-full";
export type EnvTag = "" | "local" | "development" | "staging" | "production";

export type SshAuth = "password" | "key" | "agent";

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
}

export interface IndexInfo {
  name: string;
  definition: string;
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

export interface SessionInfo {
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

export interface FetchArgs {
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
  fetchRows: (sessionId: string, args: FetchArgs) => invoke<TablePage>("fetch_rows", { sessionId, args }),
  runQuery: (sessionId: string, sql: string) => invoke<QueryOutcome>("run_query", { sessionId, sql }),
  applyEdits: (sessionId: string, schema: string, table: string, edits: RowEdit[]) =>
    invoke<number>("apply_edits", { sessionId, schema, table, edits }),
  pickSqliteFile: () => invoke<string | null>("pick_sqlite_file"),
  pickFile: (kind: "ssh-key" | "cert") => invoke<string | null>("pick_file", { kind }),
  exportFile: (defaultName: string, contents: string) =>
    invoke<string | null>("export_file", { defaultName, contents }),
  scanEnvFiles: () => invoke<EnvUrl[] | null>("scan_env_files"),
  mcpCommand: () => invoke<{ binary: string; installed: boolean }>("mcp_command"),
};

export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
