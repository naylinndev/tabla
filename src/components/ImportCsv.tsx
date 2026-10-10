import { useMemo, useState } from "react";
import { FileUp, ShieldAlert } from "lucide-react";
import { api, errorMessage, type ColumnInfo, type RowEdit } from "../lib/api";
import { parseCSV } from "../lib/sql";
import { useApp, type Session } from "../lib/store";
import { Modal } from "./ui";
import { Checkbox, Select } from "./controls";

const SKIP = "__skip__";

/** CSV → table import: auto-maps columns by header name, previews, inserts in one transaction. */
export function ImportCsv({
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
  columns: ColumnInfo[];
  onClose(): void;
  onDone(): void;
}) {
  const toast = useApp((s) => s.toast);
  const prod = session.connection.tag === "production";
  const [file, setFile] = useState<{ name: string; contents: string } | null>(null);
  const [header, setHeader] = useState(true);
  const [emptyNull, setEmptyNull] = useState(true);
  const [mapping, setMapping] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const parsed = useMemo(() => (file ? parseCSV(file.contents) : null), [file]);
  const width = parsed ? Math.max(0, ...parsed.rows.slice(0, 50).map((r) => r.length)) : 0;
  const head = parsed && header ? parsed.rows[0] : Array.from({ length: width }, (_, i) => `Column ${i + 1}`);
  const data = parsed ? (header ? parsed.rows.slice(1) : parsed.rows) : [];

  const pick = async () => {
    setError(null);
    try {
      const f = await api.pickCsvFile();
      if (!f) return;
      setFile(f);
      const p = parseCSV(f.contents);
      const first = p.rows[0] ?? [];
      // Auto-map by name (case/space/underscore-insensitive).
      const norm = (x: string) => x.toLowerCase().replace(/[\\s_-]/g, "");
      setMapping(first.map((h) => columns.find((c) => norm(c.name) === norm(h))?.name ?? SKIP));
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const mapped = mapping.map((m, i) => [i, m] as const).filter(([, m]) => m !== SKIP);
  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      const edits: RowEdit[] = data.map((r) => ({
        kind: "insert",
        values: mapped.map(([i, column]) => {
          const v = r[i] ?? "";
          return { column, value: emptyNull && v === "" ? null : v };
        }),
      }));
      const n = await api.applyEdits(session.id, schema, table, edits);
      toast(`Imported ${n.toLocaleString()} rows into ${table}`, "success");
      onDone();
      onClose();
    } catch (e) {
      setError(`Nothing was imported (the whole file runs in one transaction). ${errorMessage(e)}`);
      setBusy(false);
    }
  };

  return (
    <Modal onClose={onClose} className="modal wide">
      <div className="modal-head">
        {prod ? <ShieldAlert color="var(--danger)" /> : <FileUp color="var(--accent-2)" />}
        <div>
          <h2>
            Import CSV into {table}
            {prod ? " on production" : ""}
          </h2>
          <p>Columns are matched by header name. Every row is inserted in one transaction: all or nothing.</p>
        </div>
      </div>
      <div className="modal-body">
        <div className="url-row">
          <button className="btn" onClick={pick}>
            <FileUp /> {file ? "Choose another file" : "Choose CSV file"}
          </button>
          {file && parsed && (
            <span className="hint" style={{ alignSelf: "center" }}>
              {file.name} · {data.length.toLocaleString()} rows · delimiter “{parsed.delimiter === "\\t" ? "tab" : parsed.delimiter}”
            </span>
          )}
        </div>
        {parsed && (
          <>
            <div style={{ display: "flex", gap: 20 }}>
              <Checkbox checked={header} onChange={setHeader} label="First row is the header" />
              <Checkbox checked={emptyNull} onChange={setEmptyNull} label="Empty values become NULL" />
            </div>
            <div className="sg-panel">
              <div className="csv-map">
                {head.map((h, i) => (
                  <div key={i} className="csv-col">
                    <div className="csv-src mono ellipsis" title={h}>
                      {h || <span className="dim">(blank)</span>}
                    </div>
                    <Select
                      size="sm"
                      mono
                      className="w-full"
                      value={mapping[i] ?? SKIP}
                      options={[{ value: SKIP, label: "skip", hint: "" }, ...columns.map((c) => ({ value: c.name, label: c.name, hint: c.dataType }))]}
                      onChange={(v) => setMapping((m) => Object.assign([...m], { [i]: String(v) }))}
                    />
                    {data.slice(0, 4).map((r, j) => (
                      <div key={j} className="csv-cell mono ellipsis">
                        {r[i] === "" || r[i] === undefined ? <span className="null">{emptyNull ? "NULL" : "''"}</span> : r[i]}
                      </div>
                    ))}
                  </div>
                ))}
              </div>
            </div>
            {mapped.length === 0 && <p className="hint">Map at least one column.</p>}
            {data.length > 50000 && <p className="hint">Large file: this can take a while, and the table is locked while it runs.</p>}
          </>
        )}
        {error && (
          <div className="q-error" style={{ margin: 0 }}>
            {error}
          </div>
        )}
      </div>
      <div className="modal-foot">
        <div className="spacer" />
        <button className="btn" onClick={onClose}>
          Cancel
        </button>
        <button className={`btn ${prod ? "danger solid" : "primary"}`} disabled={!parsed || !mapped.length || !data.length || busy} onClick={run}>
          {busy && <span className="spinner" style={{ borderTopColor: "#fff" }} />}
          Import {data.length ? data.length.toLocaleString() : ""} rows
        </button>
      </div>
    </Modal>
  );
}
