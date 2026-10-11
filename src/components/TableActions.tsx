import { useState } from "react";
import { Copy, CopyPlus, Download, Eraser, ExternalLink, FileCode2, Info, PenLine, ShieldAlert, Trash2 } from "lucide-react";
import { api, errorMessage, isLocal, type Driver, type TableInfo } from "../lib/api";
import { quoteIdent } from "../lib/sql";
import { toCSV } from "../lib/sql";
import { copyWhenReady } from "../lib/clipboard";
import { isSystemSchema, useApp, type Session } from "../lib/store";
import { ContextMenu, Modal, type MenuItem } from "./ui";
import { Checkbox } from "./controls";

type Action = "rename" | "duplicate" | "truncate" | "drop";

const q = (d: Driver, schema: string, name: string) => `${quoteIdent(d, schema)}.${quoteIdent(d, name)}`;

function buildSql(d: Driver, schema: string, t: TableInfo, action: Action, target: string, withData: boolean): string[] {
  const from = q(d, schema, t.name);
  const to = q(d, schema, target);
  switch (action) {
    case "rename":
      if (d === "mssql") return [`EXEC sp_rename N'${from.replace(/'/g, "''")}', N'${target.replace(/'/g, "''")}'`];
      return d === "mysql" || d === "clickhouse" ? [`RENAME TABLE ${from} TO ${to}`] : [`ALTER TABLE ${from} RENAME TO ${quoteIdent(d, target)}`];
    case "duplicate":
      if (d === "postgres") return [`CREATE TABLE ${to} (LIKE ${from} INCLUDING ALL)`, ...(withData ? [`INSERT INTO ${to} SELECT * FROM ${from}`] : [])];
      if (d === "clickhouse") return [`CREATE TABLE ${to} AS ${from}`, ...(withData ? [`INSERT INTO ${to} SELECT * FROM ${from}`] : [])];
      if (d === "mysql") return [`CREATE TABLE ${to} LIKE ${from}`, ...(withData ? [`INSERT INTO ${to} SELECT * FROM ${from}`] : [])];
      // Copies columns and identity; keys and indexes are not copied.
      if (d === "mssql") return [`SELECT * INTO ${to} FROM ${from}${withData ? "" : " WHERE 1 = 0"}`];
      return [`CREATE TABLE ${to} AS SELECT * FROM ${from}${withData ? "" : d === "duckdb" ? " WHERE false" : d === "oracle" ? " WHERE 1 = 0" : " WHERE 0"}`];
    case "truncate":
      return d === "sqlite" ? [`DELETE FROM ${from}`] : [`TRUNCATE TABLE ${from}`];
    case "drop":
      return [`DROP ${t.kind === "view" ? "VIEW" : t.kind === "materialized" ? "MATERIALIZED VIEW" : "TABLE"} ${from}`];
  }
}

const TITLES: Record<Action, string> = {
  rename: "Rename table",
  duplicate: "Duplicate table",
  truncate: "Truncate table",
  drop: "Delete table",
};

export function useTableMenu(session: Session) {
  const [menu, setMenu] = useState<{ x: number; y: number; table: TableInfo } | null>(null);
  const [action, setAction] = useState<{ kind: Action; table: TableInfo } | null>(null);
  const toast = useApp((s) => s.toast);
  const st = useApp.getState;
  const d = session.connection.driver;
  const ro = session.connection.readOnly || isSystemSchema(session, session.schema);

  const items = (t: TableInfo): (MenuItem | "sep")[] => [
    { label: "Copy table name", icon: <Copy />, onClick: () => navigator.clipboard.writeText(t.name) },
    { label: "Open in new tab", icon: <ExternalLink />, onClick: () => st().openTable(session.id, session.schema, t.name, "data", true) },
    "sep",
    { label: "Show create syntax", icon: <Info />, onClick: () => st().openTable(session.id, session.schema, t.name, "info") },
    {
      label: "Copy create syntax",
      icon: <FileCode2 />,
      onClick: async () => {
        try {
          await copyWhenReady(api.tableDetails(session.id, session.schema, t.name).then((info) => info.ddl ?? ""));
          toast("Create syntax copied", "success");
        } catch (e) {
          toast(errorMessage(e), "error");
        }
      },
    },
    {
      label: "Export as CSV…",
      icon: <Download />,
      onClick: async () => {
        try {
          const res = await api.runQuery(session.id, `SELECT * FROM ${q(d, session.schema, t.name)}`);
          const r = res.results[0];
          const path = await api.exportFile(`${t.name}.csv`, toCSV(r.columns, r.rows));
          if (path) toast(`Exported ${r.rows.length.toLocaleString()} rows${r.truncated ? " (first 10,000)" : ""}`, "success");
        } catch (e) {
          toast(errorMessage(e), "error");
        }
      },
    },
    ...(ro
      ? []
      : ([
          "sep",
          { label: "Rename…", icon: <PenLine />, onClick: () => setAction({ kind: "rename", table: t }) },
          ...(t.kind === "table" ? [{ label: "Duplicate…", icon: <CopyPlus />, onClick: () => setAction({ kind: "duplicate", table: t }) }] : []),
          "sep",
          ...(t.kind === "table" ? [{ label: "Truncate…", icon: <Eraser />, danger: true, onClick: () => setAction({ kind: "truncate", table: t }) }] : []),
          { label: "Delete…", icon: <Trash2 />, danger: true, onClick: () => setAction({ kind: "drop", table: t }) },
        ] as (MenuItem | "sep")[])),
  ];

  const element = (
    <>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={items(menu.table)} onClose={() => setMenu(null)} />}
      {action && <TableActionDialog session={session} action={action.kind} table={action.table} onClose={() => setAction(null)} />}
    </>
  );
  return { element, open: (e: React.MouseEvent, table: TableInfo) => setMenu({ x: e.clientX, y: e.clientY, table }) };
}

function TableActionDialog({ session, action, table, onClose }: { session: Session; action: Action; table: TableInfo; onClose(): void }) {
  const toast = useApp((s) => s.toast);
  const d = session.connection.driver;
  const prod = session.connection.tag === "production";
  const [target, setTarget] = useState(action === "duplicate" ? `${table.name}_copy` : table.name);
  const [withData, setWithData] = useState(true);
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const destructive = action === "truncate" || action === "drop";
  const needsName = action === "rename" || action === "duplicate";
  const stmts = buildSql(d, session.schema, table, action, target.trim(), withData);
  const ready = needsName ? !!target.trim() && target.trim() !== table.name : confirm === table.name;

  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      const begin = d === "mssql" ? "BEGIN TRAN" : "BEGIN";
      // Oracle, like MySQL, commits structure changes one by one anyway.
      const script = d === "mysql" || d === "clickhouse" || d === "oracle" || stmts.length === 1 ? stmts.join(";\n") : `${begin};\n${stmts.join(";\n")};\nCOMMIT`;
      await api.runQuery(session.id, script);
      const st = useApp.getState();
      if (action === "rename" || action === "drop") {
        const s = st.sessions.find((x) => x.id === session.id);
        s?.tabs.filter((t) => t.kind === "table" && t.table === table.name).forEach((t) => st.closeTab(session.id, t.id));
      }
      await st.refreshTables(session.id);
      if (action === "rename" || action === "duplicate") st.openTable(session.id, session.schema, target.trim());
      toast(
        action === "rename" ? `Renamed to ${target.trim()}` : action === "duplicate" ? `Created ${target.trim()}` : action === "truncate" ? `Emptied ${table.name}` : `Deleted ${table.name}`,
        "success",
      );
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };

  return (
    <Modal onClose={onClose}>
      <div className="modal-head">
        {destructive || prod ? <ShieldAlert color="var(--danger)" /> : <PenLine color="var(--accent-2)" />}
        <div>
          <h2>
            {TITLES[action]} “{table.name}”{prod ? " on production" : ""}
          </h2>
          <p>
            {action === "drop"
              ? "The table and all its data are deleted permanently."
              : action === "truncate"
                ? "Every row is deleted permanently. The structure is kept."
                : action === "duplicate"
                  ? "Creates a new table with the same structure."
                  : "Queries and code that use the old name will stop working."}
          </p>
        </div>
      </div>
      <div className="modal-body">
        {needsName && (
          <div className="field">
            <label>New table name</label>
            <input className="input mono" autoFocus value={target} onChange={(e) => setTarget(e.target.value)} spellCheck={false} onKeyDown={(e) => e.key === "Enter" && ready && run()} />
          </div>
        )}
        {action === "duplicate" && <Checkbox checked={withData} onChange={setWithData} label="Copy the data too" />}
        {action === "duplicate" && (isLocal(d) || d === "oracle") && (
          <p className="hint">
            <Info /> {d === "duckdb" ? "DuckDB" : d === "oracle" ? "Oracle" : "SQLite"} copies columns and data, but not keys, indexes or constraints.
          </p>
        )}
        {destructive && (
          <div className="field">
            <label>
              Type <b className="mono">{table.name}</b> to confirm
            </label>
            <input className="input mono" autoFocus value={confirm} onChange={(e) => setConfirm(e.target.value)} spellCheck={false} onKeyDown={(e) => e.key === "Enter" && ready && run()} />
          </div>
        )}
        <div className="code-block">
          <div className="code-head">
            <span>SQL</span>
          </div>
          <pre>{stmts.map((s) => s + ";").join("\n")}</pre>
        </div>
        {error && <div className="q-error" style={{ margin: 0 }}>{error}</div>}
      </div>
      <div className="modal-foot">
        <div className="spacer" />
        <button className="btn" onClick={onClose}>
          Cancel
        </button>
        <button className={`btn ${destructive || prod ? "danger solid" : "primary"}`} disabled={!ready || busy} onClick={run}>
          {busy && <span className="spinner" style={{ borderTopColor: "#fff" }} />}
          {TITLES[action].split(" ")[0]}
        </button>
      </div>
    </Modal>
  );
}
