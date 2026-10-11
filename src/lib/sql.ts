import type { ColumnMeta, Driver, ResultSet } from "./api";

export function quoteIdent(driver: Driver, s: string): string {
  if (driver === "mysql" || driver === "clickhouse") return "`" + s.replace(/`/g, "``") + "`";
  if (driver === "mssql") return "[" + s.replace(/]/g, "]]") + "]";
  return '"' + s.replace(/"/g, '""') + '"';
}

export function quoteLiteral(driver: Driver, s: string): string {
  let v = s.replace(/'/g, "''");
  if (driver === "mysql" || driver === "clickhouse") v = v.replace(/\\/g, "\\\\");
  // N'' keeps non-ASCII text intact on SQL Server.
  return driver === "mssql" ? `N'${v}'` : `'${v}'`;
}

export type FilterOp =
  | "="
  | "!="
  | "<"
  | ">"
  | "<="
  | ">="
  | "contains"
  | "not contains"
  | "starts with"
  | "in"
  | "is null"
  | "is not null";

export const FILTER_OPS: FilterOp[] = [
  "=", "!=", "<", ">", "<=", ">=", "contains", "not contains", "starts with", "in", "is null", "is not null",
];

export const opTakesValue = (op: FilterOp) => op !== "is null" && op !== "is not null";

export interface Filter {
  id: string;
  column: string;
  op: FilterOp;
  value: string;
  enabled: boolean;
}

function likeEscape(v: string, driver?: Driver) {
  // SQL Server also treats [ as a wildcard (character ranges).
  return v.replace(driver === "mssql" ? /[\\%_[]/g : /[\\%_]/g, (m) => "\\" + m);
}

export function buildFilter(driver: Driver, filters: Filter[], join: "AND" | "OR" = "AND"): string {
  const parts = filters
    .filter((f) => f.enabled && f.column)
    .map((f) => {
      const col = quoteIdent(driver, f.column);
      const asText = driver === "postgres" || driver === "duckdb" ? `${col}::text` : driver === "clickhouse" ? `toString(${col})` : col;
      const like = driver === "postgres" || driver === "clickhouse" || driver === "duckdb" ? "ILIKE" : "LIKE";
      const lit = (v: string) => quoteLiteral(driver, v);
      // MySQL and ClickHouse already treat backslash as the LIKE escape.
      const esc = driver === "mysql" || driver === "clickhouse" ? "" : " ESCAPE '\\'";
      switch (f.op) {
        case "is null":
          return `${col} IS NULL`;
        case "is not null":
          return `${col} IS NOT NULL`;
        case "contains":
          return `${asText} ${like} ${lit(`%${likeEscape(f.value, driver)}%`)}${esc}`;
        case "not contains":
          return `${asText} NOT ${like} ${lit(`%${likeEscape(f.value, driver)}%`)}${esc}`;
        case "starts with":
          return `${asText} ${like} ${lit(`${likeEscape(f.value, driver)}%`)}${esc}`;
        case "in": {
          const items = f.value.split(",").map((s) => s.trim()).filter(Boolean);
          return items.length ? `${col} IN (${items.map(lit).join(", ")})` : "1=0";
        }
        default:
          return `${col} ${f.op === "!=" ? "<>" : f.op} ${lit(f.value)}`;
      }
    });
  return parts.length > 1 && join === "OR" ? parts.map((p) => `(${p})`).join(" OR ") : parts.join(` ${join} `);
}

const NUMERIC = /int|serial|numeric|decimal|float|double|real|money|number|bit\b/i;
export const isNumericType = (t: string) => NUMERIC.test(t) && !/interval|point/i.test(t);
export const isBoolType = (t: string) => /^bool(ean)?$|tinyint\(1\)/i.test(t);
export const isJsonType = (t: string) => /json/i.test(t);

/**
 * Splits a script into statements, respecting quotes, comments and
 * Postgres dollar-quoting. Returns each statement with its offsets.
 */
export function splitStatements(sql: string, driver?: Driver): { text: string; from: number; to: number }[] {
  if (driver === "mssql") return splitBatches(sql);
  const out: { text: string; from: number; to: number }[] = [];
  let start = 0;
  let i = 0;
  const n = sql.length;
  const push = (end: number) => {
    const raw = sql.slice(start, end);
    if (raw.trim()) {
      const lead = raw.length - raw.trimStart().length;
      out.push({ text: raw.trim(), from: start + lead, to: start + lead + raw.trim().length });
    }
  };
  while (i < n) {
    const c = sql[i];
    const next = sql[i + 1];
    if (c === "-" && next === "-") {
      const e = sql.indexOf("\n", i);
      i = e === -1 ? n : e + 1;
    } else if (c === "/" && next === "*") {
      const e = sql.indexOf("*/", i + 2);
      i = e === -1 ? n : e + 2;
    } else if (c === "'" || c === '"' || c === "`") {
      i++;
      while (i < n) {
        if (sql[i] === "\\" && c === "'") i += 2;
        else if (sql[i] === c && sql[i + 1] === c) i += 2;
        else if (sql[i] === c) {
          i++;
          break;
        } else i++;
      }
    } else if (c === "$") {
      const m = /^\$[A-Za-z_]*\$/.exec(sql.slice(i));
      if (m) {
        const e = sql.indexOf(m[0], i + m[0].length);
        i = e === -1 ? n : e + m[0].length;
      } else i++;
    } else if (c === ";") {
      push(i);
      start = i + 1;
      i++;
    } else i++;
  }
  push(n);
  return out;
}

/**
 * T-SQL scripts are split into batches on lines that hold only GO (as in
 * SSMS); semicolons inside BEGIN…END bodies stay part of their batch.
 */
function splitBatches(sql: string): { text: string; from: number; to: number }[] {
  const out: { text: string; from: number; to: number }[] = [];
  const re = /^[ \t]*GO[ \t]*(?:--.*)?$/gim;
  let start = 0;
  const push = (end: number) => {
    const raw = sql.slice(start, end);
    if (raw.trim()) {
      const lead = raw.length - raw.trimStart().length;
      out.push({ text: raw.trim(), from: start + lead, to: start + lead + raw.trim().length });
    }
  };
  for (let m = re.exec(sql); m; m = re.exec(sql)) {
    push(m.index);
    start = m.index + m[0].length;
  }
  push(sql.length);
  return out;
}

export function statementAt(sql: string, pos: number, driver?: Driver): string {
  const stmts = splitStatements(sql, driver);
  if (!stmts.length) return "";
  // Prefer the statement containing the cursor, else the closest one before it.
  const hit = stmts.find((s) => pos >= s.from && pos <= s.to + 1);
  if (hit) return hit.text;
  const before = [...stmts].reverse().find((s) => s.to < pos);
  return (before ?? stmts[0]).text;
}

function csvCell(v: string | null) {
  if (v === null) return "";
  return /[",\n\r]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v;
}

export function toCSV(columns: ColumnMeta[], rows: (string | null)[][]): string {
  const head = columns.map((c) => csvCell(c.name)).join(",");
  return [head, ...rows.map((r) => r.map(csvCell).join(","))].join("\n") + "\n";
}

/** Converts a text cell to its natural JSON type (number, boolean, object). */
function typedValue(v: string | null, type: string): unknown {
  if (v === null) return null;
  if (isBoolType(type)) return v === "t" || v === "true" || v === "1";
  if (isNumericType(type)) {
    const n = Number(v);
    // Keep huge integers/decimals as strings so no precision is lost.
    return Number.isFinite(n) && Math.abs(n) < Number.MAX_SAFE_INTEGER && v.replace(/\.?0+$/, "").length <= 16 ? n : v;
  }
  if (isJsonType(type)) {
    try {
      return JSON.parse(v);
    } catch {
      return v;
    }
  }
  return v;
}

export function toJSON(columns: ColumnMeta[], rows: (string | null)[][]): string {
  return JSON.stringify(
    rows.map((r) => Object.fromEntries(columns.map((c, i) => [c.name, typedValue(r[i], c.typeName)]))),
    null,
    2,
  );
}

export function toTSV(rows: (string | null)[][]): string {
  return rows.map((r) => r.map((v) => (v ?? "NULL").replace(/[\t\n]/g, " ")).join("\t")).join("\n");
}

export function resultSummary(r: ResultSet): string {
  if (r.columns.length) return `${r.rows.length.toLocaleString()} row${r.rows.length === 1 ? "" : "s"}`;
  return `${r.rowsAffected.toLocaleString()} affected`;
}

/** RFC 4180 CSV parsing with quoted fields; the delimiter is auto-detected. */
export function parseCSV(text: string, delimiter?: string): { rows: string[][]; delimiter: string } {
  const firstLine = text.slice(0, text.indexOf("\n") === -1 ? text.length : text.indexOf("\n"));
  const d =
    delimiter ??
    [",", "\t", ";", "|"].map((c) => [c, firstLine.split(c).length] as const).sort((a, b) => b[1] - a[1])[0][0];
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        field += '"';
        i++;
      } else if (c === '"') quoted = false;
      else field += c;
    } else if (c === '"' && field === "") quoted = true;
    else if (c === d) {
      row.push(field);
      field = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(field);
      field = "";
      if (row.length > 1 || row[0] !== "") rows.push(row);
      row = [];
    } else field += c;
  }
  if (field !== "" || row.length) {
    row.push(field);
    rows.push(row);
  }
  return { rows, delimiter: d };
}
