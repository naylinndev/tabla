import { useEffect, useMemo, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { MySQL, PostgreSQL, SQLite, sql } from "@codemirror/lang-sql";
import { ArrowRight, Code2, ExternalLink, Info, Minus, Plus, Power, RefreshCw } from "lucide-react";
import { api, errorMessage, type Driver, type ForeignKey, type TableRelations, type TriggerInfo } from "../lib/api";
import { quoteIdent } from "../lib/sql";
import { useApp, type Session } from "../lib/store";
import { Select } from "./controls";
import { SqlConfirm } from "./SqlConfirm";

const ACTIONS = ["NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"];

export function useRelations(session: Session, schema: string, table: string) {
  const [rel, setRel] = useState<TableRelations | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [rev, setRev] = useState(0);
  useEffect(() => {
    setError(null);
    api.tableRelations(session.id, schema, table).then(setRel, (e) => setError(errorMessage(e)));
  }, [session.id, schema, table, rev]);
  return { rel, error, reload: () => setRev((r) => r + 1) };
}

const qt = (d: Driver, schema: string, table: string) => `${quoteIdent(d, schema)}.${quoteIdent(d, table)}`;

export function RelationsView({ session, schema, table, editable }: { session: Session; schema: string; table: string; editable: boolean }) {
  const { rel, error, reload } = useRelations(session, schema, table);
  const openTable = useApp((s) => s.openTable);
  const d = session.connection.driver;
  const [selected, setSelected] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [dropping, setDropping] = useState<ForeignKey | null>(null);

  if (error) return <div className="q-error">{error}</div>;
  if (!rel)
    return (
      <div className="q-message">
        <span className="spinner" />
      </div>
    );
  const sel = rel.outgoing.find((f) => f.name === selected);
  const canAlter = editable && d !== "sqlite";

  return (
    <div className="structure">
      <div className="sg-section" style={{ marginTop: 0 }}>
        Foreign keys on {table}
      </div>
      <section className="sg-panel">
        <div className="sg fk" role="table">
          <div className="sg-row sg-head">
            <span>Name</span>
            <span>Columns</span>
            <span>References</span>
            <span>On update</span>
            <span>On delete</span>
            <span />
          </div>
          {rel.outgoing.length === 0 && <div className="sg-empty">This table has no foreign keys.</div>}
          {rel.outgoing.map((f) => (
            <div key={f.name} className={`sg-row ${selected === f.name ? "sel" : ""}`} onMouseDown={() => setSelected(f.name)}>
              <span className="mono strong ellipsis">{f.name}</span>
              <span className="mono ellipsis">{f.columns.join(", ")}</span>
              <span className="mono ellipsis">
                <ArrowRight size={12} className="dim" style={{ verticalAlign: -1 }} /> {f.refTable}({f.refColumns.join(", ")})
              </span>
              <span className="fk-act">{f.onUpdate}</span>
              <span className={`fk-act ${f.onDelete === "CASCADE" ? "warn" : ""}`}>{f.onDelete}</span>
              <span className="c">
                <button className="icon-btn" style={{ width: 24, height: 24 }} title={`Open ${f.refTable}`} onClick={() => openTable(session.id, f.refSchema || schema, f.refTable)}>
                  <ExternalLink size={13} />
                </button>
              </span>
            </div>
          ))}
        </div>
        <div className="sg-bar">
          <button className="icon-btn" title="Add foreign key" onClick={() => setAdding(true)} disabled={!canAlter}>
            <Plus />
          </button>
          <button className="icon-btn" title="Drop selected foreign key" onClick={() => sel && setDropping(sel)} disabled={!canAlter || !sel}>
            <Minus />
          </button>
          <button className="icon-btn" title="Refresh" onClick={reload}>
            <RefreshCw />
          </button>
          <span className="sg-count">
            {rel.outgoing.length} key{rel.outgoing.length === 1 ? "" : "s"}
            {d === "sqlite" && editable && (
              <span className="dim">
                {" "}
                · <Info size={11} style={{ verticalAlign: -1 }} /> SQLite can only define keys when a table is created
              </span>
            )}
          </span>
        </div>
      </section>

      <div className="sg-section">Referenced by</div>
      <section className="sg-panel">
        <div className="sg fk-in" role="table">
          <div className="sg-row sg-head">
            <span>Table</span>
            <span>Columns</span>
            <span>Points to</span>
            <span>On delete</span>
            <span />
          </div>
          {rel.incoming.length === 0 && <div className="sg-empty">No other table points at {table}.</div>}
          {rel.incoming.map((f) => (
            <div key={`${f.table}.${f.name}`} className="sg-row">
              <span className="mono strong ellipsis">{f.table}</span>
              <span className="mono ellipsis">{f.columns.join(", ")}</span>
              <span className="mono ellipsis">{f.refColumns.join(", ")}</span>
              <span className={`fk-act ${f.onDelete === "CASCADE" ? "warn" : ""}`}>{f.onDelete}</span>
              <span className="c">
                <button className="icon-btn" style={{ width: 24, height: 24 }} title={`Open ${f.table}`} onClick={() => openTable(session.id, f.schema || schema, f.table)}>
                  <ExternalLink size={13} />
                </button>
              </span>
            </div>
          ))}
        </div>
      </section>

      {adding && <AddForeignKey session={session} schema={schema} table={table} onClose={() => setAdding(false)} onDone={reload} />}
      {dropping && (
        <SqlConfirm
          session={session}
          title={`Drop foreign key ${dropping.name}`}
          description={`${table} will no longer be checked against ${dropping.refTable}. Data is not changed.`}
          statements={[
            d === "mysql"
              ? `ALTER TABLE ${qt(d, schema, table)} DROP FOREIGN KEY ${quoteIdent(d, dropping.name)}`
              : `ALTER TABLE ${qt(d, schema, table)} DROP CONSTRAINT ${quoteIdent(d, dropping.name)}`,
          ]}
          danger
          confirmLabel="Drop key"
          onDone={reload}
          onClose={() => setDropping(null)}
        />
      )}
    </div>
  );
}

function AddForeignKey({ session, schema, table, onClose, onDone }: { session: Session; schema: string; table: string; onClose(): void; onDone(): void }) {
  const d = session.connection.driver;
  const [cols, setCols] = useState<string[]>([]);
  const [column, setColumn] = useState("");
  const [refTable, setRefTable] = useState(session.tables.find((x) => x.kind === "table" && x.name !== table)?.name ?? "");
  const [refCols, setRefCols] = useState<string[]>([]);
  const [refColumn, setRefColumn] = useState("");
  const [onUpdate, setOnUpdate] = useState("NO ACTION");
  const [onDelete, setOnDelete] = useState("NO ACTION");
  const [name, setName] = useState("");

  useEffect(() => {
    api.tableStructure(session.id, schema, table).then((s) => {
      setCols(s.columns.map((c) => c.name));
      setColumn((c) => c || s.columns.find((x) => !x.primaryKey)?.name || s.columns[0]?.name || "");
    });
  }, [session.id, schema, table]);
  useEffect(() => {
    if (!refTable) return;
    api.tableStructure(session.id, schema, refTable).then((s) => {
      setRefCols(s.columns.map((c) => c.name));
      setRefColumn(s.columns.find((c) => c.primaryKey)?.name ?? s.columns[0]?.name ?? "");
    });
  }, [session.id, schema, refTable]);

  const fkName = name.trim() || `fk_${table}_${column}`.slice(0, 60);
  const stmt = useMemo(
    () =>
      column && refTable && refColumn
        ? `ALTER TABLE ${qt(d, schema, table)} ADD CONSTRAINT ${quoteIdent(d, fkName)} FOREIGN KEY (${quoteIdent(d, column)}) REFERENCES ${qt(d, schema, refTable)} (${quoteIdent(d, refColumn)}) ON UPDATE ${onUpdate} ON DELETE ${onDelete}`
        : "",
    [d, schema, table, fkName, column, refTable, refColumn, onUpdate, onDelete],
  );

  return (
    <SqlConfirm
      session={session}
      title="Add foreign key"
      description="Existing rows must already match, or the server will refuse the key."
      statements={stmt ? [stmt] : []}
      confirmLabel="Add key"
      onDone={onDone}
      onClose={onClose}
    >
      <div className="grid-2b">
        <div className="field">
          <label>Column in {table}</label>
          <Select className="w-full" mono value={column} options={cols.map((c) => ({ value: c, label: c }))} onChange={(v) => setColumn(String(v))} />
        </div>
        <div className="field">
          <label>Key name</label>
          <input className="input mono" placeholder={fkName} value={name} onChange={(e) => setName(e.target.value)} spellCheck={false} />
        </div>
      </div>
      <div className="grid-2b">
        <div className="field">
          <label>References table</label>
          <Select
            className="w-full"
            mono
            value={refTable}
            options={session.tables.filter((x) => x.kind === "table").map((x) => ({ value: x.name, label: x.name }))}
            onChange={(v) => setRefTable(String(v))}
          />
        </div>
        <div className="field">
          <label>References column</label>
          <Select className="w-full" mono value={refColumn} options={refCols.map((c) => ({ value: c, label: c }))} onChange={(v) => setRefColumn(String(v))} />
        </div>
      </div>
      <div className="grid-2b">
        <div className="field">
          <label>On update</label>
          <Select className="w-full" value={onUpdate} options={ACTIONS.map((a) => ({ value: a, label: a }))} onChange={(v) => setOnUpdate(String(v))} />
        </div>
        <div className="field">
          <label>On delete</label>
          <Select className="w-full" value={onDelete} options={ACTIONS.map((a) => ({ value: a, label: a }))} onChange={(v) => setOnDelete(String(v))} />
        </div>
      </div>
    </SqlConfirm>
  );
}

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.operatorKeyword, t.modifier], color: "var(--syn-keyword)", fontWeight: "500" },
  { tag: [t.string], color: "var(--syn-string)" },
  { tag: [t.number, t.bool, t.null], color: "var(--syn-number)" },
  { tag: [t.comment, t.lineComment, t.blockComment], color: "var(--syn-comment)" },
]);
const DIALECT: Record<string, typeof PostgreSQL> = { postgres: PostgreSQL, mysql: MySQL, sqlite: SQLite, redis: PostgreSQL };

function triggerTemplate(d: Driver, schema: string, table: string): string {
  const tq = `${quoteIdent(d, schema)}.${quoteIdent(d, table)}`;
  if (d === "postgres")
    return `-- 1. The function the trigger runs\nCREATE OR REPLACE FUNCTION ${table}_before_write() RETURNS trigger\nLANGUAGE plpgsql AS $$\nBEGIN\n  -- NEW is the row being written, e.g. NEW.updated_at := now();\n  RETURN NEW;\nEND $$;\n\n-- 2. The trigger\nCREATE TRIGGER ${table}_before_write\nBEFORE INSERT OR UPDATE ON ${tq}\nFOR EACH ROW EXECUTE FUNCTION ${table}_before_write();`;
  if (d === "mysql")
    return `CREATE TRIGGER \`${table}_before_insert\`\nBEFORE INSERT ON ${tq}\nFOR EACH ROW\nSET NEW.created_at = IFNULL(NEW.created_at, NOW());`;
  return `CREATE TRIGGER ${table}_after_insert\nAFTER INSERT ON ${tq}\nBEGIN\n  -- e.g. INSERT INTO audit_log (table_name, row_id) VALUES ('${table}', NEW.rowid);\n  SELECT 1;\nEND;`;
}

export function TriggersView({ session, schema, table, editable }: { session: Session; schema: string; table: string; editable: boolean }) {
  const { rel, error, reload } = useRelations(session, schema, table);
  const newQuery = useApp((s) => s.newQuery);
  const d = session.connection.driver;
  const [selected, setSelected] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ title: string; statements: string[]; danger?: boolean; label: string } | null>(null);
  const extensions = useMemo(() => [sql({ dialect: DIALECT[d] }), syntaxHighlighting(highlight), EditorView.lineWrapping, EditorView.editable.of(false)], [d]);

  if (error) return <div className="q-error">{error}</div>;
  if (!rel)
    return (
      <div className="q-message">
        <span className="spinner" />
      </div>
    );
  const sel: TriggerInfo | undefined = rel.triggers.find((x) => x.name === selected) ?? rel.triggers[0];
  const tq = qt(d, schema, table);
  const dropSql = (tr: TriggerInfo) =>
    d === "postgres" ? `DROP TRIGGER ${quoteIdent(d, tr.name)} ON ${tq}` : `DROP TRIGGER ${quoteIdent(d, schema)}.${quoteIdent(d, tr.name)}`;

  return (
    <div className="structure">
      <section className="sg-panel">
        <div className="sg trg" role="table">
          <div className="sg-row sg-head">
            <span>Name</span>
            <span>Timing</span>
            <span>Event</span>
            <span className="c">Enabled</span>
          </div>
          {rel.triggers.length === 0 && <div className="sg-empty">No triggers on {table}.</div>}
          {rel.triggers.map((tr) => (
            <div key={tr.name} className={`sg-row ${sel?.name === tr.name ? "sel" : ""}`} onMouseDown={() => setSelected(tr.name)}>
              <span className="mono strong ellipsis">{tr.name}</span>
              <span className="fk-act">{tr.timing}</span>
              <span className="fk-act">{tr.event}</span>
              <span className="c">{tr.enabled ? <span className="badge local">on</span> : <span className="badge">off</span>}</span>
            </div>
          ))}
        </div>
        <div className="sg-bar">
          <button className="icon-btn" title="New trigger (opens a template in the query editor)" onClick={() => newQuery(session.id, triggerTemplate(d, schema, table))} disabled={!editable}>
            <Plus />
          </button>
          <button
            className="icon-btn"
            title="Drop selected trigger"
            disabled={!editable || !sel}
            onClick={() => sel && setConfirm({ title: `Drop trigger ${sel.name}`, statements: [dropSql(sel)], danger: true, label: "Drop trigger" })}
          >
            <Minus />
          </button>
          {d === "postgres" && (
            <button
              className="icon-btn"
              title={sel?.enabled ? "Disable trigger" : "Enable trigger"}
              disabled={!editable || !sel}
              onClick={() =>
                sel &&
                setConfirm({
                  title: `${sel.enabled ? "Disable" : "Enable"} trigger ${sel.name}`,
                  statements: [`ALTER TABLE ${tq} ${sel.enabled ? "DISABLE" : "ENABLE"} TRIGGER ${quoteIdent(d, sel.name)}`],
                  label: sel.enabled ? "Disable" : "Enable",
                })
              }
            >
              <Power />
            </button>
          )}
          <button
            className="icon-btn"
            title="Edit in query editor"
            disabled={!editable || !sel}
            onClick={() =>
              sel &&
              newQuery(
                session.id,
                d === "postgres"
                  ? `-- Recreate the trigger with your changes.\n-- The function it calls can be edited with CREATE OR REPLACE FUNCTION.\n${dropSql(sel)};\n${sel.definition};`
                  : `-- Triggers can't be altered in place: drop and recreate.\n${dropSql(sel)};\n${sel.definition.trim().replace(/;?$/, ";")}`,
              )
            }
          >
            <Code2 />
          </button>
          <button className="icon-btn" title="Refresh" onClick={reload}>
            <RefreshCw />
          </button>
          <span className="sg-count">
            {rel.triggers.length} trigger{rel.triggers.length === 1 ? "" : "s"}
          </span>
        </div>
      </section>

      {sel && (
        <>
          <div className="sg-section">Definition · {sel.name}</div>
          <div className="code-block ddl">
            <CodeMirror value={sel.definition} extensions={extensions} theme="none" basicSetup={{ lineNumbers: false, foldGutter: false, highlightActiveLine: false }} />
          </div>
        </>
      )}

      {confirm && (
        <SqlConfirm
          session={session}
          title={confirm.title}
          statements={confirm.statements}
          danger={confirm.danger}
          confirmLabel={confirm.label}
          onDone={reload}
          onClose={() => setConfirm(null)}
        />
      )}
    </div>
  );
}
