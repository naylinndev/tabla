import { useEffect, useMemo, useState } from "react";
import { api, type Driver } from "../lib/api";
import { quoteIdent, quoteLiteral } from "../lib/sql";
import { useApp, type Session } from "../lib/store";
import { Select } from "./controls";
import { SqlConfirm } from "./SqlConfirm";

/** MySQL and ClickHouse call the level above tables a database; the rest call it a schema. */
export const schemaNoun = (d: Driver) => (d === "mysql" || d === "clickhouse" || d === "mongodb" ? "database" : "schema");

/** Engines where Tabla can create that level. Oracle schemas are users (see Users), SQLite has one file. */
export const canCreateSchema = (d: Driver) => ["mysql", "clickhouse", "postgres", "mssql", "duckdb"].includes(d);

const DEFAULT = "";

/** "New database" (MySQL, ClickHouse) or "New schema" (Postgres, SQL Server, DuckDB). */
export function NewDatabase({ session, onClose }: { session: Session; onClose(): void }) {
  const d = session.connection.driver;
  const noun = schemaNoun(d);
  const [name, setName] = useState("");
  const [charset, setCharset] = useState(DEFAULT);
  const [collation, setCollation] = useState(DEFAULT);
  const [charsets, setCharsets] = useState<{ name: string; collation: string; description: string }[]>([]);
  const [collations, setCollations] = useState<string[]>([]);
  const [server, setServer] = useState<{ charset: string; collation: string } | null>(null);

  // MySQL: offer the server's character sets and collations, defaulting to the server's own.
  useEffect(() => {
    if (d !== "mysql") return;
    const rows = async (sql: string) => (await api.runQuery(session.id, sql)).results[0]?.rows ?? [];
    rows("SELECT @@character_set_server, @@collation_server")
      .then(([r]) => r && setServer({ charset: r[0] ?? "", collation: r[1] ?? "" }))
      .catch(() => {});
    rows("SELECT CHARACTER_SET_NAME, DEFAULT_COLLATE_NAME, DESCRIPTION FROM information_schema.CHARACTER_SETS ORDER BY DESCRIPTION")
      .then((r) => setCharsets(r.map((x) => ({ name: x[0] ?? "", collation: x[1] ?? "", description: x[2] ?? "" }))))
      .catch(() => {});
  }, [d, session.id]);

  const effectiveCharset = charset || server?.charset || "";
  useEffect(() => {
    if (d !== "mysql" || !effectiveCharset) return;
    api
      .runQuery(session.id, `SELECT COLLATION_NAME FROM information_schema.COLLATIONS WHERE CHARACTER_SET_NAME = ${quoteLiteral(d, effectiveCharset)} ORDER BY COLLATION_NAME`)
      .then((o) => setCollations((o.results[0]?.rows ?? []).map((x) => x[0] ?? "")))
      .catch(() => {});
  }, [d, session.id, effectiveCharset]);

  const trimmed = name.trim();
  const taken = [...session.schemas, ...session.systemSchemas].some((s) => s.toLowerCase() === trimmed.toLowerCase());
  const sql = useMemo(() => {
    if (!trimmed || taken) return "";
    const id = quoteIdent(d, trimmed);
    if (d === "mysql") {
      return [`CREATE DATABASE ${id}`, charset && `CHARACTER SET ${charset}`, collation && `COLLATE ${collation}`].filter(Boolean).join(" ");
    }
    if (d === "clickhouse") return `CREATE DATABASE ${id}`;
    return `CREATE SCHEMA ${id}`;
  }, [d, trimmed, taken, charset, collation]);

  return (
    <SqlConfirm
      session={session}
      title={`Create ${noun}`}
      description={`On ${session.connection.name}`}
      statements={sql ? [sql] : []}
      displayStatements={sql ? [sql] : [taken ? `-- A ${noun} named ${trimmed} already exists` : `-- Enter a name for the new ${noun}`]}
      confirmLabel={`Create ${noun}`}
      onDone={() => useApp.getState().reloadSchemas(session.id, trimmed)}
      onClose={onClose}
    >
      <div className="field">
        <label>{noun === "database" ? "Database name" : "Schema name"}</label>
        <input className="input mono" autoFocus placeholder={noun === "database" ? "shop" : "billing"} value={name} onChange={(e) => setName(e.target.value)} spellCheck={false} />
      </div>
      {d === "mysql" && (
        <div className="grid-2b">
          <div className="field">
            <label>Encoding</label>
            <Select
              className="w-full"
              value={charset}
              options={[
                { value: DEFAULT, label: `Server Default${server ? ` (${server.charset})` : ""}` },
                // UTF-8 first: what almost everyone wants. Then the rest, by name.
                ...[...charsets.filter((c) => c.name.startsWith("utf8")), ...charsets.filter((c) => !c.name.startsWith("utf8"))].map((c) => ({
                  value: c.name,
                  label: `${c.description} (${c.name})`,
                  group: c.name.startsWith("utf8") ? "Unicode" : "Other",
                })),
              ]}
              onChange={(v) => {
                setCharset(String(v));
                setCollation(DEFAULT);
              }}
            />
          </div>
          <div className="field">
            <label>Collation</label>
            <Select
              className="w-full"
              value={collation}
              options={[
                {
                  value: DEFAULT,
                  label: charset
                    ? `Default (${charsets.find((c) => c.name === charset)?.collation ?? ""})`
                    : `Server Default${server ? ` (${server.collation})` : ""}`,
                },
                ...collations.map((c) => ({ value: c, label: c, group: effectiveCharset })),
              ]}
              onChange={(v) => setCollation(String(v))}
            />
          </div>
        </div>
      )}
    </SqlConfirm>
  );
}
