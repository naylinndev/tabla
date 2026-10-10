import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, Code2, GripVertical, Info, KeyRound, Plus, RotateCcw, ShieldAlert, Trash2, Undo2, Wand2 } from "lucide-react";
import { api, errorMessage, type AlterColumn, type AlterRequest, type Driver, type TableStructure } from "../lib/api";
import { useApp, type Session } from "../lib/store";
import { Modal } from "./ui";

interface Draft {
  key: string;
  original: AlterColumn | null;
  name: string;
  dataType: string;
  nullable: boolean;
  defaultValue: string;
  dropped: boolean;
  pk: boolean;
}

const TYPES: Record<Driver, string[]> = {
  postgres: ["text", "varchar(255)", "integer", "bigint", "serial", "bigserial", "boolean", "numeric(12,2)", "real", "double precision", "date", "timestamp", "timestamptz", "jsonb", "uuid", "bytea", "text[]"],
  mysql: ["varchar(255)", "text", "int", "bigint", "tinyint(1)", "decimal(10,2)", "double", "date", "datetime", "timestamp", "json", "blob", "char(36)"],
  sqlite: ["TEXT", "INTEGER", "REAL", "NUMERIC", "BLOB"],
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
    return { key: c.name, original, ...original, defaultValue: c.defaultValue ?? "", dropped: false, pk: c.primaryKey };
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
}) {
  const toast = useApp((s) => s.toast);
  const driver = session.connection.driver;
  const prod = session.connection.tag === "production";
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [dragKey, setDragKey] = useState<string | null>(null);
  const [plan, setPlan] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
  const orderChanged = drafts.filter((d) => d.original && !d.dropped).map((d) => d.key).join() !==
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

  const review = async () => {
    setError(null);
    if (!changes.length && orderChanged && driver !== "mysql") {
      onViewOrder(liveNames);
      toast("Column order saved for this table's grid view", "success");
      return;
    }
    setBusy(true);
    try {
      const stmts = await api.planAlter(session.id, { schema, table, changes, order: driver === "mysql" && orderChanged ? liveNames : [] });
      setPlan(stmts);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const apply = async () => {
    setBusy(true);
    try {
      const ran = await api.applyAlter(session.id, { schema, table, changes, order: driver === "mysql" && orderChanged ? liveNames : [] });
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

  return (
    <div className="structure">
      <div className="st-head">
        <h3>Columns · {drafts.filter((d) => !d.dropped).length}</h3>
        <div className="spacer" />
        {editable && (
          <button
            className="btn ghost"
            onClick={() => {
              const key = `new-${crypto.randomUUID()}`;
              setDrafts((ds) => [...ds, { key, original: null, name: "", dataType: TYPES[driver][0], nullable: true, defaultValue: "", dropped: false, pk: false }]);
            }}
          >
            <Plus /> Add column
          </button>
        )}
      </div>
      {driver !== "mysql" && editable && (
        <p className="hint" style={{ marginTop: -4 }}>
          <Info /> Drag rows to reorder. {driver === "postgres" ? "PostgreSQL" : "SQLite"} can't reorder stored columns, so
          this sets the order Tabla shows in the grid. Header drag in the grid does the same.
        </p>
      )}
      <datalist id={`types-${driver}`}>
        {TYPES[driver].map((t) => (
          <option key={t} value={t} />
        ))}
      </datalist>
      <div className="st-table" role="table">
        <div className="st-row st-header" role="row">
          <span />
          <span>Name</span>
          <span>Type</span>
          <span>Nullable</span>
          <span>Default</span>
          <span />
        </div>
        {drafts.map((d) => {
          const changed = d.original ? !same(d.original, toDef(d)) : true;
          return (
            <div
              key={d.key}
              role="row"
              className={`st-row ${d.dropped ? "dropped" : ""} ${!d.original ? "added" : changed ? "changed" : ""} ${dragKey === d.key ? "dragging" : ""}`}
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
                title="Drag to reorder"
              >
                {d.pk ? <KeyRound className="pk-ico" /> : <GripVertical />}
              </span>
              <input
                className="input sm mono"
                value={d.name}
                placeholder="column_name"
                disabled={!editable || d.dropped}
                onChange={(e) => patch(d.key, { name: e.target.value })}
                spellCheck={false}
                autoFocus={!d.original && !d.name}
              />
              <input
                className="input sm mono"
                list={`types-${driver}`}
                value={d.dataType}
                disabled={!editable || d.dropped}
                onChange={(e) => patch(d.key, { dataType: e.target.value })}
                spellCheck={false}
              />
              <label className="st-check">
                <input type="checkbox" checked={d.nullable} disabled={!editable || d.dropped || d.pk} onChange={(e) => patch(d.key, { nullable: e.target.checked })} />
                {d.nullable ? "null" : "not null"}
              </label>
              <input
                className="input sm mono"
                value={d.defaultValue}
                placeholder={editable ? "e.g. now(), 'draft', 0" : ""}
                disabled={!editable || d.dropped}
                onChange={(e) => patch(d.key, { defaultValue: e.target.value })}
                spellCheck={false}
              />
              <span className="st-acts">
                {editable &&
                  (d.dropped ? (
                    <button className="icon-btn" title="Keep column" onClick={() => patch(d.key, { dropped: false })}>
                      <RotateCcw />
                    </button>
                  ) : d.original && changed ? (
                    <button className="icon-btn" title="Undo changes" onClick={() => patch(d.key, { ...d.original!, defaultValue: d.original!.defaultValue ?? "" })}>
                      <Undo2 />
                    </button>
                  ) : null)}
                {editable && !d.dropped && (
                  <button
                    className="icon-btn danger"
                    title={d.original ? "Drop column" : "Remove"}
                    onClick={() => (d.original ? patch(d.key, { dropped: true }) : setDrafts((ds) => ds.filter((x) => x.key !== d.key)))}
                  >
                    <Trash2 />
                  </button>
                )}
              </span>
            </div>
          );
        })}
      </div>

      {error && (
        <div className="q-error" style={{ margin: "0 0 20px" }}>
          <AlertTriangle />
          <div>{error}</div>
        </div>
      )}

      {dirty && editable && (
        <div className="st-bar">
          <span className="dot" />
          <span>
            {changes.length > 0 && `${changes.length} column change${changes.length > 1 ? "s" : ""}`}
            {changes.length > 0 && orderChanged && " · "}
            {orderChanged && "new order"}
          </span>
          <div className="spacer" />
          <button className="btn ghost" onClick={() => structure && setDrafts(fromStructure(structure, viewOrder))}>
            Discard
          </button>
          <button className="btn primary" onClick={review} disabled={busy}>
            {busy ? <span className="spinner" style={{ borderTopColor: "#fff" }} /> : <Wand2 />}
            {changes.length || driver === "mysql" ? "Review SQL" : "Save order"}
          </button>
        </div>
      )}

      <h3 style={{ marginTop: 28 }}>Indexes · {structure.indexes.length}</h3>
      {structure.indexes.length ? (
        <table className="s-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Definition</th>
            </tr>
          </thead>
          <tbody>
            {structure.indexes.map((i) => (
              <tr key={i.name}>
                <td className="mono" style={{ fontWeight: 600, whiteSpace: "nowrap" }}>
                  {i.name}
                </td>
                <td className="mono muted">
                  {i.definition}
                  <button className="icon-btn" style={{ width: 22, height: 22, marginLeft: 6, verticalAlign: "middle" }} title="Open in query editor" onClick={() => onQuery(i.definition)}>
                    <Code2 size={12} />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p className="dim">No indexes.</p>
      )}

      {plan && (
        <Modal onClose={() => setPlan(null)} className="modal wide">
          <div className="modal-head">
            {prod ? <ShieldAlert color="var(--danger)" /> : <Wand2 color="var(--accent-2)" />}
            <div>
              <h2>{prod ? "Change structure on production?" : "Review structure changes"}</h2>
              <p>
                {plan.length} statement{plan.length === 1 ? "" : "s"} will run on <b>{table}</b>
                {driver === "mysql" ? ". MySQL applies them one by one and can't roll back structure changes." : " in a single transaction."}
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
