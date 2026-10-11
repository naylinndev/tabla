import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, Check, Code2, Copy, GripVertical, Info, KeyRound, Minus, Plus, RefreshCw, RotateCcw, ShieldAlert, Wand2 } from "lucide-react";
import { api, errorMessage, type AlterColumn, type AlterRequest, type TableStructure } from "../lib/api";
import { useApp, type Session } from "../lib/store";
import { joinType, splitType } from "../lib/format";
import { Modal } from "./ui";
import { Checkbox, Combobox } from "./controls";
import { SqlConfirm } from "./SqlConfirm";
import { quoteIdent } from "../lib/sql";

interface Draft {
  key: string;
  original: AlterColumn | null;
  name: string;
  dataType: string;
  nullable: boolean;
  defaultValue: string;
  dropped: boolean;
  pk: boolean;
  keyKind: string;
  extra: string;
}

/** Base type names (length goes in its own column). */
const TYPES: Record<string, string[]> = {
  postgres: ["text", "character varying", "character", "integer", "bigint", "smallint", "serial", "bigserial", "boolean", "numeric", "real", "double precision", "date", "time without time zone", "timestamp without time zone", "timestamp with time zone", "interval", "jsonb", "json", "uuid", "bytea", "inet", "text[]", "integer[]"],
  mysql: ["int", "bigint", "smallint", "tinyint", "mediumint", "decimal", "float", "double", "bit", "varchar", "char", "text", "mediumtext", "longtext", "date", "datetime", "timestamp", "time", "year", "json", "enum", "blob", "longblob", "binary", "varbinary"],
  sqlite: ["TEXT", "INTEGER", "REAL", "NUMERIC", "BLOB"],
  mssql: ["int", "bigint", "smallint", "tinyint", "bit", "decimal", "numeric", "money", "float", "real", "nvarchar", "varchar", "nchar", "char", "date", "time", "datetime2", "datetimeoffset", "datetime", "uniqueidentifier", "varbinary", "xml"],
};

const toDef = (d: Draft): AlterColumn => ({
  name: d.name.trim(),
  dataType: d.dataType.trim(),
  nullable: d.nullable,
  defaultValue: d.defaultValue.trim() || null,
});

const same = (a: AlterColumn, b: AlterColumn) =>
  a.name === b.name && a.dataType === b.dataType && a.nullable === b.nullable && (a.defaultValue ?? null) === (b.defaultValue ?? null);

function fromStructure(st: TableStructure, order?: string[]): Draft[] {
  const cols = [...st.columns];
  if (order?.length) cols.sort((a, b) => (order.indexOf(a.name) + 1 || 1e9) - (order.indexOf(b.name) + 1 || 1e9));
  return cols.map((c) => {
    const original = { name: c.name, dataType: c.dataType, nullable: c.nullable, defaultValue: c.defaultValue };
    return { key: c.name, original, ...original, defaultValue: c.defaultValue ?? "", dropped: false, pk: c.primaryKey, keyKind: c.key, extra: c.extra };
  });
}

export function StructureEditor({
  session,
  schema,
  table,
  structure,
  editable,
  viewOrder,
  onViewOrder,
  onApplied,
  onQuery,
  onRefresh,
}: {
  session: Session;
  schema: string;
  table: string;
  structure: TableStructure | null;
  editable: boolean;
  viewOrder: string[];
  onViewOrder(names: string[]): void;
  onApplied(): void;
  onQuery(sql: string): void;
  onRefresh(): void;
}) {
  const toast = useApp((s) => s.toast);
  const driver = session.connection.driver;
  const prod = session.connection.tag === "production";
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [dragKey, setDragKey] = useState<string | null>(null);
  const [plan, setPlan] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selIndex, setSelIndex] = useState<string | null>(null);
  const [addingIndex, setAddingIndex] = useState(false);
  const [droppingIndex, setDroppingIndex] = useState<string | null>(null);

  useEffect(() => {
    if (structure) setDrafts(fromStructure(structure, viewOrder));
    // Re-seed only when the structure itself reloads.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [structure]);

  const changes = useMemo(
    () =>
      drafts.flatMap((d): AlterRequest["changes"] => {
        if (!d.original) return d.dropped ? [] : [{ original: null, column: toDef(d) }];
        if (d.dropped) return [{ original: d.original, column: null }];
        return same(d.original, toDef(d)) ? [] : [{ original: d.original, column: toDef(d) }];
      }),
    [drafts],
  );
  const liveNames = drafts.filter((d) => !d.dropped).map((d) => d.name.trim());
  const baseline = useMemo(() => (structure ? fromStructure(structure, viewOrder).map((d) => d.key) : []), [structure, viewOrder]);
  const orderChanged =
    drafts.filter((d) => d.original && !d.dropped).map((d) => d.key).join() !==
    baseline.filter((k) => drafts.some((d) => d.key === k && !d.dropped)).join();
  const dirty = changes.length > 0 || orderChanged;

  if (!structure)
    return (
      <div className="q-message">
        <span className="spinner" />
      </div>
    );

  const patch = (key: string, p: Partial<Draft>) => setDrafts((ds) => ds.map((d) => (d.key === key ? { ...d, ...p } : d)));
  const move = (from: string, to: string) =>
    setDrafts((ds) => {
      const a = [...ds];
      const i = a.findIndex((d) => d.key === from);
      const j = a.findIndex((d) => d.key === to);
      const [m] = a.splice(i, 1);
      a.splice(j, 0, m);
      return a;
    });
  const blank = (over: Partial<Draft> = {}): Draft => ({
    key: `new-${crypto.randomUUID()}`,
    original: null,
    name: "",
    dataType: driver === "mysql" ? "varchar(255)" : TYPES[driver][0],
    nullable: true,
    defaultValue: "",
    dropped: false,
    pk: false,
    keyKind: "",
    extra: "",
    ...over,
  });
  const addColumn = () => {
    const d = blank();
    setDrafts((ds) => {
      const i = selected ? ds.findIndex((x) => x.key === selected) : -1;
      const a = [...ds];
      a.splice(i >= 0 ? i + 1 : a.length, 0, d);
      return a;
    });
    setSelected(d.key);
  };
  const removeSelected = () => {
    const d = drafts.find((x) => x.key === selected);
    if (!d) return;
    if (d.original) patch(d.key, { dropped: !d.dropped });
    else setDrafts((ds) => ds.filter((x) => x.key !== d.key));
  };
  const duplicateSelected = () => {
    const d = drafts.find((x) => x.key === selected);
    if (!d) return;
    const copy = blank({ name: `${d.name}_copy`, dataType: d.dataType, nullable: true, defaultValue: d.defaultValue });
    setDrafts((ds) => {
      const a = [...ds];
      a.splice(a.findIndex((x) => x.key === d.key) + 1, 0, copy);
      return a;
    });
    setSelected(copy.key);
  };

  const req = (): AlterRequest => ({ schema, table, changes, order: driver === "mysql" && orderChanged ? liveNames : [] });
  const review = async () => {
    setError(null);
    if (!changes.length && orderChanged && driver !== "mysql") {
      onViewOrder(liveNames);
      toast("Column order saved for this table's grid view", "success");
      return;
    }
    setBusy(true);
    try {
      setPlan(await api.planAlter(session.id, req()));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const apply = async () => {
    setBusy(true);
    try {
      const ran = await api.applyAlter(session.id, req());
      if (orderChanged) onViewOrder(liveNames);
      setPlan(null);
      toast(`Applied ${ran.length} change${ran.length === 1 ? "" : "s"} to ${table}`, "success");
      onApplied();
    } catch (e) {
      setPlan(null);
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const drops = changes.filter((c) => !c.column).length;
  const sel = drafts.find((d) => d.key === selected);
  const mysql = driver === "mysql";

  return (
    <div className="structure">
      <section className="sg-panel">
        <div className={`sg ${mysql ? "mysql" : ""}`} role="table">
          <div className="sg-row sg-head" role="row">
            <span />
            <span>Field</span>
            <span>Type</span>
            <span>Length</span>
            {mysql && <span className="c">Unsigned</span>}
            <span className="c">Allow Null</span>
            <span>Key</span>
            <span>Default</span>
            <span>Extra</span>
          </div>
          {drafts.map((d) => {
            const changed = d.original ? !same(d.original, toDef(d)) : true;
            const t = splitType(d.dataType);
            const setType = (p: Partial<typeof t>) => patch(d.key, { dataType: joinType({ ...t, ...p }) });
            const off = !editable || d.dropped;
            return (
              <div
                key={d.key}
                role="row"
                className={`sg-row ${selected === d.key ? "sel" : ""} ${d.dropped ? "dropped" : !d.original ? "added" : changed ? "changed" : ""} ${dragKey === d.key ? "dragging" : ""}`}
                onMouseDown={() => setSelected(d.key)}
                onDragOver={(e) => {
                  if (!dragKey || dragKey === d.key) return;
                  e.preventDefault();
                  move(dragKey, d.key);
                }}
              >
                <span
                  className="grip"
                  draggable={editable}
                  onDragStart={(e) => {
                    setDragKey(d.key);
                    e.dataTransfer.effectAllowed = "move";
                    e.dataTransfer.setData("text/plain", d.key);
                  }}
                  onDragEnd={() => setDragKey(null)}
                  title={editable ? "Drag to reorder" : undefined}
                >
                  {d.pk ? <KeyRound className="pk-ico" /> : <GripVertical />}
                </span>
                <input
                  className="sg-in mono strong"
                  value={d.name}
                  placeholder="column_name"
                  disabled={off}
                  onChange={(e) => patch(d.key, { name: e.target.value })}
                  spellCheck={false}
                  autoFocus={!d.original && !d.name}
                />
                <Combobox className="sg-combo" value={t.base} suggestions={TYPES[driver]} disabled={off} onChange={(v) => setType({ base: v })} />
                <input className="sg-in mono" value={t.len} disabled={off} onChange={(e) => setType({ len: e.target.value })} spellCheck={false} />
                {mysql && (
                  <span className="c">
                    <Checkbox checked={t.unsigned} disabled={off} onChange={(v) => setType({ unsigned: v })} />
                  </span>
                )}
                <span className="c">
                  <Checkbox checked={d.nullable} disabled={off || d.pk} onChange={(v) => patch(d.key, { nullable: v })} />
                </span>
                <span className={`sg-key ${d.keyKind.toLowerCase()}`}>{d.keyKind}</span>
                <input
                  className="sg-in mono"
                  value={d.defaultValue}
                  placeholder={d.nullable ? "NULL" : ""}
                  disabled={off}
                  onChange={(e) => patch(d.key, { defaultValue: e.target.value })}
                  spellCheck={false}
                />
                <span className="sg-extra" title={d.extra}>
                  {d.dropped ? "will be dropped" : !d.original ? "new" : d.extra}
                </span>
              </div>
            );
          })}
        </div>
        <div className="sg-bar">
          <button className="icon-btn" title="Add column" onClick={addColumn} disabled={!editable}>
            <Plus />
          </button>
          <button className="icon-btn" title={sel?.dropped ? "Keep column" : "Drop selected column"} onClick={removeSelected} disabled={!editable || !sel}>
            {sel?.dropped ? <RotateCcw /> : <Minus />}
          </button>
          <button className="icon-btn" title="Duplicate selected column" onClick={duplicateSelected} disabled={!editable || !sel}>
            <Copy />
          </button>
          <button className="icon-btn" title="Refresh" onClick={onRefresh}>
            <RefreshCw />
          </button>
          <span className="sg-count">
            {drafts.filter((d) => !d.dropped).length} columns
            {!mysql && editable && (
              <span className="dim" title={`${driver === "postgres" ? "PostgreSQL" : driver === "mssql" ? "SQL Server" : "SQLite"} can't reorder stored columns; dragging sets Tabla's grid order.`}>
                {" "}
                · <Info size={11} style={{ verticalAlign: -1 }} /> drag sets view order
              </span>
            )}
          </span>
          <div className="spacer" />
          {dirty && editable && (
            <>
              <span className="sg-pending">
                <span className="dot" />
                {changes.length > 0 && `${changes.length} change${changes.length > 1 ? "s" : ""}`}
                {changes.length > 0 && orderChanged && " · "}
                {orderChanged && "new order"}
              </span>
              <button className="btn ghost" onClick={() => structure && setDrafts(fromStructure(structure, viewOrder))}>
                Discard
              </button>
              <button className="btn primary" onClick={review} disabled={busy}>
                {busy ? <span className="spinner" style={{ borderTopColor: "#fff" }} /> : <Wand2 />}
                {changes.length || mysql ? "Review SQL" : "Save order"}
              </button>
            </>
          )}
        </div>
      </section>

      {error && (
        <div className="q-error" style={{ margin: "12px 0 0" }}>
          <AlertTriangle />
          <div>{error}</div>
        </div>
      )}

      <div className="sg-section">Indexes</div>
      <section className="sg-panel">
        <div className="sg ix" role="table">
          <div className="sg-row sg-head" role="row">
            <span>Name</span>
            <span>Columns</span>
            <span className="c">Unique</span>
            <span className="c">Primary</span>
            <span>Method</span>
            <span />
          </div>
          {structure.indexes.length === 0 && <div className="sg-empty">No indexes</div>}
          {structure.indexes.map((i) => (
            <div key={i.name} className={`sg-row ${selIndex === i.name ? "sel" : ""}`} role="row" title={i.definition} onMouseDown={() => setSelIndex(i.name)}>
              <span className="mono strong ellipsis">{i.name}</span>
              <span className="mono ellipsis">{i.columns}</span>
              <span className="c">{i.unique && <Check className="ok-ico" />}</span>
              <span className="c">{i.primary && <KeyRound className="pk-ico" />}</span>
              <span className="mono dim">{i.method}</span>
              <span className="c">
                <button className="icon-btn" style={{ width: 24, height: 24 }} title="Open definition in query editor" onClick={() => onQuery(i.definition)}>
                  <Code2 size={13} />
                </button>
              </span>
            </div>
          ))}
        </div>
        <div className="sg-bar">
          <button className="icon-btn" title="Add index" disabled={!editable} onClick={() => setAddingIndex(true)}>
            <Plus />
          </button>
          <button
            className="icon-btn"
            title="Drop selected index"
            disabled={!editable || !selIndex || structure.indexes.find((i) => i.name === selIndex)?.primary}
            onClick={() => setDroppingIndex(selIndex)}
          >
            <Minus />
          </button>
          <span className="sg-count">
            {structure.indexes.length} index{structure.indexes.length === 1 ? "" : "es"}
          </span>
        </div>
      </section>

      {addingIndex && (
        <AddIndexDialog session={session} schema={schema} table={table} columns={structure.columns.map((c) => c.name)} onClose={() => setAddingIndex(false)} onDone={onApplied} />
      )}
      {droppingIndex && (
        <SqlConfirm
          session={session}
          title={`Drop index ${droppingIndex}`}
          description="Queries that relied on it may get slower. Data is not changed."
          statements={[
            driver === "mysql" || driver === "mssql"
              ? `DROP INDEX ${quoteIdent(driver, droppingIndex)} ON ${quoteIdent(driver, schema)}.${quoteIdent(driver, table)}`
              : `DROP INDEX ${quoteIdent(driver, schema)}.${quoteIdent(driver, droppingIndex)}`,
          ]}
          danger
          confirmLabel="Drop index"
          onDone={onApplied}
          onClose={() => setDroppingIndex(null)}
        />
      )}
      {plan && (
        <Modal onClose={() => setPlan(null)} className="modal wide">
          <div className="modal-head">
            {prod ? <ShieldAlert color="var(--danger)" /> : <Wand2 color="var(--accent-2)" />}
            <div>
              <h2>{prod ? "Change structure on production?" : "Review structure changes"}</h2>
              <p>
                {plan.length} statement{plan.length === 1 ? "" : "s"} will run on <b>{table}</b>
                {mysql ? ". MySQL applies them one by one and can't roll back structure changes." : " in a single transaction."}
              </p>
            </div>
          </div>
          <div className="modal-body">
            {drops > 0 && (
              <div className="toggle-row" style={{ borderColor: "color-mix(in srgb, var(--danger) 35%, transparent)" }}>
                <AlertTriangle color="var(--danger)" />
                <div>
                  <b>
                    {drops} column{drops > 1 ? "s" : ""} will be dropped
                  </b>
                  <small>Their data is deleted permanently.</small>
                </div>
              </div>
            )}
            <div className="code-block">
              <div className="code-head">
                <span>SQL</span>
                <button className="btn ghost" onClick={() => navigator.clipboard.writeText(plan.join(";\n") + ";")}>
                  Copy
                </button>
              </div>
              <pre>{plan.map((s) => s + ";").join("\n")}</pre>
            </div>
          </div>
          <div className="modal-foot">
            <div className="spacer" />
            <button className="btn" onClick={() => setPlan(null)}>
              Cancel
            </button>
            <button className={`btn ${prod || drops ? "danger solid" : "primary"}`} onClick={apply} disabled={busy}>
              {busy && <span className="spinner" style={{ borderTopColor: "#fff" }} />}
              {prod ? "Apply to production" : "Apply changes"}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function AddIndexDialog({
  session,
  schema,
  table,
  columns,
  onClose,
  onDone,
}: {
  session: Session;
  schema: string;
  table: string;
  columns: string[];
  onClose(): void;
  onDone(): void;
}) {
  const d = session.connection.driver;
  const [picked, setPicked] = useState<string[]>([]);
  const [unique, setUnique] = useState(false);
  const [name, setName] = useState("");
  const idxName = name.trim() || `${unique ? "uq" : "idx"}_${table}_${picked.join("_")}`.slice(0, 60);
  const q = (x: string) => quoteIdent(d, x);
  // SQLite qualifies the index name with the schema; the others qualify the table.
  const target = d === "sqlite" ? `${q(schema)}.${q(idxName)} ON ${q(table)}` : `${q(idxName)} ON ${q(schema)}.${q(table)}`;
  const stmt = picked.length ? `CREATE ${unique ? "UNIQUE " : ""}INDEX ${target} (${picked.map(q).join(", ")})` : "";
  return (
    <SqlConfirm
      session={session}
      title="Add index"
      description="Speeds up lookups and sorting on these columns. Unique also forbids duplicate values."
      statements={stmt ? [stmt] : []}
      displayStatements={stmt ? [stmt] : ["-- Pick at least one column"]}
      confirmLabel="Create index"
      onDone={onDone}
      onClose={onClose}
    >
      <div className="field">
        <label>Columns, in order (click to add)</label>
        <div className="idx-cols">
          {columns.map((c) => {
            const pos = picked.indexOf(c);
            return (
              <button key={c} type="button" className={`idx-col ${pos >= 0 ? "on" : ""}`} onClick={() => setPicked((p) => (pos >= 0 ? p.filter((x) => x !== c) : [...p, c]))}>
                {pos >= 0 && <span className="idx-pos">{pos + 1}</span>}
                {c}
              </button>
            );
          })}
        </div>
      </div>
      <div className="grid-2b">
        <div className="field">
          <label>Index name</label>
          <input className="input mono" placeholder={idxName} value={name} onChange={(e) => setName(e.target.value)} spellCheck={false} />
        </div>
        <div className="field" style={{ justifyContent: "flex-end" }}>
          <Checkbox checked={unique} onChange={setUnique} label="Unique" />
        </div>
      </div>
    </SqlConfirm>
  );
}
