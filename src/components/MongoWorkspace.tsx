import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { json } from "@codemirror/lang-json";
import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import {
  Braces,
  ChevronLeft,
  ChevronRight,
  Copy,
  Eye,
  FileJson,
  Folder,
  KeyRound,
  ListTree,
  Lock,
  PenLine,
  Plus,
  Power,
  RefreshCw,
  Search,
  ShieldAlert,
  Table2,
  TerminalSquare,
  Trash2,
  X,
} from "lucide-react";
import { api, errorMessage, type MongoCollection, type MongoIndex, type MongoPage } from "../lib/api";
import { isSystemSchema, useApp, local, type Session } from "../lib/store";
import { DriverIcon } from "./icons";
import { Modal, TagBadge, useDrag } from "./ui";
import { Checkbox, Select } from "./controls";
import { Console } from "./Console";

const highlight = HighlightStyle.define([
  { tag: [t.propertyName], color: "var(--syn-keyword)" },
  { tag: [t.string], color: "var(--syn-string)" },
  { tag: [t.number, t.bool, t.null], color: "var(--syn-number)" },
]);
const PAGE_SIZES = [25, 50, 100, 250, 500];

export function MongoWorkspace({ session, visible }: { session: Session; visible: boolean }) {
  const toast = useApp((s) => s.toast);
  const c = session.connection;
  const prod = c.tag === "production";
  const [db, setDb] = useState(session.schema || session.schemas[0] || "test");
  const [collections, setCollections] = useState<MongoCollection[]>([]);
  const [loading, setLoading] = useState(false);
  const [q, setQ] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<"docs" | "shell">("docs");
  const [logOpen, setLogOpen] = useState(false);
  const [creating, setCreating] = useState<"collection" | "database" | null>(null);
  const system = isSystemSchema(session, db);
  const [width, setWidth] = useState(() => local.get("tabla.sidebarWidth", 300));
  const startW = useRef(width);
  const resize = useDrag((dx) => {
    const w = Math.max(220, Math.min(560, startW.current + dx));
    setWidth(w);
    local.set("tabla.sidebarWidth", w);
  });

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setCollections(await api.mongoCollections(session.id, db));
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setLoading(false);
    }
  }, [session.id, db, toast]);

  // A collection to open once the database switch below has happened (new database).
  const openAfterSwitch = useRef<string | null>(null);
  useEffect(() => {
    setSelected(openAfterSwitch.current);
    openAfterSwitch.current = null;
    load();
  }, [load]);

  const shown = useMemo(() => collections.filter((x) => !q || x.name.toLowerCase().includes(q.toLowerCase())), [collections, q]);

  return (
    <div className="shell" style={{ display: visible ? "flex" : "none", flex: 1, minWidth: 0 }}>
      <aside className="sidebar" style={{ width }}>
        <div className={`sb-resize ${resize.active ? "on" : ""}`} onMouseDown={(e) => { startW.current = width; resize.start(e); }} />
        <div className="titlebar-space" data-tauri-drag-region />
        <div className="sb-head">
          <div className="sb-conn">
            <DriverIcon driver={c.driver} provider={c.provider} size={30} />
            <div style={{ minWidth: 0, flex: 1 }}>
              <div className="sb-conn-name">{c.name}</div>
              <div className="sb-conn-meta">
                {c.host}
                {c.port ? `:${c.port}` : ""} · v{session.serverVersion}
              </div>
            </div>
            <TagBadge tag={c.tag} />
            <button className="icon-btn" title="Disconnect" onClick={() => useApp.getState().requestDisconnect(session.id)}>
              <Power />
            </button>
          </div>
          <div className="schema-pick">
            <Select
              size="sm"
              className="w-full"
              mono
              value={db}
              title="Database"
              options={[
                ...session.schemas.map((d) => ({ value: d, label: d })),
                ...session.systemSchemas.map((d) => ({ value: d, label: d, group: "System" })),
              ]}
              onChange={(v) => setDb(String(v))}
            />
            {!c.readOnly && (
              <button className="icon-btn" title="New database" onClick={() => setCreating("database")}>
                <Plus size={14} />
              </button>
            )}
          </div>
          {system && (
            <div className="schema-note">
              <Lock size={11} /> Built-in database: read-only here
            </div>
          )}
          <div className="sb-search">
            <Search />
            <input className="input" placeholder="Filter collections" value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} />
          </div>
        </div>
        <div className="sb-section" style={{ padding: "6px 22px 4px" }}>
          Collections <span className="count">{collections.length}</span>
          <div className="spacer" />
          <button className="icon-btn" style={{ width: 22, height: 22 }} title="Refresh" onClick={load}>
            {loading ? <span className="spinner" style={{ width: 11, height: 11 }} /> : <RefreshCw size={12} />}
          </button>
        </div>
        <div className="sb-list">
          {shown.map((x) => (
            <button
              key={x.name}
              className={`sb-item ${selected === x.name && view === "docs" ? "active" : ""}`}
              onClick={() => {
                setSelected(x.name);
                setView("docs");
              }}
              title={x.name}
            >
              {x.kind === "view" ? <Eye /> : <Folder />}
              <span className="name">{x.name}</span>
              {x.kind !== "collection" && <span className="rows">{x.kind}</span>}
            </button>
          ))}
          {!loading && shown.length === 0 && <div className="sb-empty">{q ? "No collections match" : "No collections in this database"}</div>}
        </div>
        <div className="sb-foot">
          <button className="btn" onClick={() => setCreating("collection")} disabled={c.readOnly || system}>
            <Plus /> Collection
          </button>
          <button className={`btn ${view === "shell" ? "primary" : ""}`} onClick={() => setView(view === "shell" ? "docs" : "shell")}>
            <TerminalSquare /> Shell
          </button>
        </div>
      </aside>

      <main className={`main ${prod ? "production" : ""}`}>
        <div className="tabbar" data-tauri-drag-region>
          <div className="tabs-scroll">
            <div className={`tab ${view === "docs" ? "active" : ""}`} onMouseDown={() => setView("docs")}>
              <Folder />
              <span className="title">{selected ?? "Documents"}</span>
            </div>
            <div className={`tab ${view === "shell" ? "active" : ""}`} onMouseDown={() => setView("shell")}>
              <TerminalSquare />
              <span className="title">Shell</span>
            </div>
          </div>
          <div className="spacer" data-tauri-drag-region style={{ height: "100%" }} />
          <button className={`icon-btn ${logOpen ? "on" : ""}`} onClick={() => setLogOpen(!logOpen)} title="Console: every command Tabla sends">
            <TerminalSquare />
          </button>
        </div>
        {view === "shell" ? (
          <MongoShell session={session} db={db} onChanged={load} />
        ) : selected ? (
          <CollectionView key={`${db}.${selected}`} session={session} db={db} collection={selected} readOnlyKind={collections.find((x) => x.name === selected)?.kind === "view"} />
        ) : (
          <div className="q-message">
            <div className="card dim">
              <Folder />
              <div>
                Pick a collection on the left, or open the <b>Shell</b> to run <span className="mono">db.collection.find()</span> and other commands.
              </div>
            </div>
          </div>
        )}
        {logOpen && <Console session={session} onClose={() => setLogOpen(false)} />}
      </main>

      {creating && (
        <NewCollection
          session={session}
          db={creating === "database" ? null : db}
          onClose={() => setCreating(null)}
          onCreated={(name, inDb) => {
            setCreating(null);
            if (inDb !== db) {
              // MongoDB creates a database with its first collection; list it and switch to it.
              useApp.setState((st) => ({
                sessions: st.sessions.map((s) => (s.id === session.id && !s.schemas.includes(inDb) ? { ...s, schemas: [...s.schemas, inDb].sort() } : s)),
              }));
              openAfterSwitch.current = name;
              setDb(inDb);
            } else load();
            setSelected(name);
            setView("docs");
          }}
        />
      )}
    </div>
  );
}

/** New collection in `db`, or (db = null) a new database with its first collection. */
function NewCollection({ session, db, onClose, onCreated }: { session: Session; db: string | null; onClose(): void; onCreated(name: string, db: string): void }) {
  const [name, setName] = useState("");
  const [dbName, setDbName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const target = db ?? dbName.trim();
  const taken = db === null && [...session.schemas, ...session.systemSchemas].includes(target);
  const ready = !!name.trim() && !!target && !taken;
  const create = async () => {
    try {
      await api.mongoShell(session.id, target, JSON.stringify({ create: name.trim() }), true);
      onCreated(name.trim(), target);
    } catch (e) {
      setError(errorMessage(e));
    }
  };
  return (
    <Modal onClose={onClose}>
      <div className="modal-head">
        <Folder />
        <div>
          <h2>{db === null ? "New database" : "New collection"}</h2>
          <p>
            {db === null
              ? "MongoDB creates a database together with its first collection."
              : `In database ${db}. Collections are also created automatically by the first insert.`}
          </p>
        </div>
      </div>
      <div className="modal-body">
        {db === null && (
          <div className="field">
            <label>Database name</label>
            <input className="input mono" autoFocus placeholder="shop" value={dbName} onChange={(e) => setDbName(e.target.value)} spellCheck={false} />
            {taken && <small className="dim">A database named {target} already exists.</small>}
          </div>
        )}
        <div className="field">
          <label>{db === null ? "First collection" : "Name"}</label>
          <input
            className="input mono"
            autoFocus={db !== null}
            placeholder="orders"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && ready && create()}
            spellCheck={false}
          />
        </div>
        {error && <div className="q-error">{error}</div>}
      </div>
      <div className="modal-foot">
        <div className="spacer" />
        <button className="btn" onClick={onClose}>
          Cancel
        </button>
        <button className="btn primary" disabled={!ready} onClick={create}>
          Create
        </button>
      </div>
    </Modal>
  );
}

function CollectionView({ session, db, collection, readOnlyKind }: { session: Session; db: string; collection: string; readOnlyKind: boolean }) {
  const toast = useApp((s) => s.toast);
  const ro = session.connection.readOnly || readOnlyKind || isSystemSchema(session, db);
  const prod = session.connection.tag === "production";
  const key = `tabla.mongo.${session.connection.id}.${db}.${collection}`;
  const [filter, setFilter] = useState(() => local.get(`${key}.filter`, ""));
  const [sort, setSort] = useState(() => local.get(`${key}.sort`, ""));
  const [applied, setApplied] = useState({ filter, sort });
  const [limit, setLimit] = useState(() => local.get("tabla.mongo.limit", 50));
  const [skip, setSkip] = useState(0);
  const [page, setPage] = useState<MongoPage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [mode, setMode] = useState<"table" | "json" | "indexes">(() => local.get("tabla.mongo.mode", "table"));
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [editing, setEditing] = useState<{ index: number | null; text: string } | null>(null);
  const [cell, setCell] = useState<{ row: number; col: number; value: string } | null>(null);
  const [confirm, setConfirm] = useState<{ title: string; body: string; run(): Promise<void> } | null>(null);

  const load = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const p = await api.mongoFind(session.id, { db, collection, filter: applied.filter, sort: applied.sort, projection: "", skip, limit });
      setPage(p);
      setPicked(new Set());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }, [session.id, db, collection, applied, skip, limit]);

  useEffect(() => {
    load();
  }, [load]);

  const apply = () => {
    local.set(`${key}.filter`, filter);
    local.set(`${key}.sort`, sort);
    setSkip(0);
    setApplied({ filter, sort });
  };

  // Production changes always ask first.
  const guarded = (title: string, body: string, run: () => Promise<void>) => (prod ? setConfirm({ title, body, run }) : run());

  const saveCell = async (row: number, col: number, value: string | null) => {
    if (!page) return;
    const field = page.columns[col];
    await guarded(`Update ${field}`, `Set ${field} on this document${prod ? " in production" : ""}.`, async () => {
      try {
        await api.mongoSetField(session.id, db, collection, page.ids[row], field, value);
        setCell(null);
        load();
      } catch (e) {
        toast(errorMessage(e), "error");
      }
    });
  };

  const del = () => {
    if (!page || !picked.size) return;
    const ids = [...picked].map((i) => page.ids[i]);
    setConfirm({
      title: `Delete ${ids.length} document${ids.length === 1 ? "" : "s"}${prod ? " in production" : ""}?`,
      body: "They are removed immediately and can't be restored.",
      run: async () => {
        try {
          await api.mongoDelete(session.id, db, collection, ids);
          toast(`Deleted ${ids.length} document${ids.length === 1 ? "" : "s"}`, "success");
          load();
        } catch (e) {
          toast(errorMessage(e), "error");
        }
      },
    });
  };

  const total = page?.total ?? null;
  const to = skip + (page?.rows.length ?? 0);

  return (
    <div className="view mongo">
      <div className="mongo-bar">
        <div className="mongo-q">
          <Braces size={13} className="dim" />
          <input
            className="input mono"
            placeholder='Filter, e.g. { status: "active", age: { $gt: 30 } }'
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && apply()}
            spellCheck={false}
          />
        </div>
        <div className="mongo-q" style={{ maxWidth: 220 }}>
          <span className="dim mono" style={{ fontSize: 11 }}>
            sort
          </span>
          <input className="input mono" placeholder="{ _id: -1 }" value={sort} onChange={(e) => setSort(e.target.value)} onKeyDown={(e) => e.key === "Enter" && apply()} spellCheck={false} />
        </div>
        <button className="btn primary" onClick={apply} disabled={busy}>
          Find
        </button>
        {(applied.filter || applied.sort) && (
          <button
            className="btn ghost"
            onClick={() => {
              setFilter("");
              setSort("");
              local.set(`${key}.filter`, "");
              local.set(`${key}.sort`, "");
              setSkip(0);
              setApplied({ filter: "", sort: "" });
            }}
          >
            <X /> Reset
          </button>
        )}
      </div>
      <div className="sg-bar mongo-tools">
        <div className="segmented">
          {(
            [
              ["table", "Table", <Table2 key="t" />],
              ["json", "JSON", <FileJson key="j" />],
              ["indexes", "Indexes", <ListTree key="i" />],
            ] as const
          ).map(([id, label, icon]) => (
            <button
              key={id}
              className={mode === id ? "on" : ""}
              onClick={() => {
                setMode(id);
                local.set("tabla.mongo.mode", id);
              }}
            >
              {icon} {label}
            </button>
          ))}
        </div>
        <button className="icon-btn" title="Refresh" onClick={load}>
          {busy ? <span className="spinner" style={{ width: 12, height: 12 }} /> : <RefreshCw />}
        </button>
        <div className="spacer" />
        {mode !== "indexes" && (
          <>
            <button className="btn ghost" disabled={ro} onClick={() => setEditing({ index: null, text: "{\n  \n}" })}>
              <Plus /> Document
            </button>
            <button className="btn ghost danger" disabled={ro || !picked.size} onClick={del}>
              <Trash2 /> Delete{picked.size ? ` (${picked.size})` : ""}
            </button>
          </>
        )}
      </div>

      {error && <div className="q-error" style={{ margin: 12 }}>{error}</div>}
      {mode === "indexes" ? (
        <IndexesView session={session} db={db} collection={collection} ro={ro} />
      ) : !page ? (
        !error && (
          <div className="q-message">
            <span className="spinner" />
          </div>
        )
      ) : page.rows.length === 0 ? (
        <div className="q-message">
          <div className="card dim">{applied.filter ? "No documents match this filter." : "This collection is empty."}</div>
        </div>
      ) : mode === "table" ? (
        <div className="mongo-grid">
          <table>
            <thead>
              <tr>
                <th className="pick">
                  <Checkbox checked={picked.size === page.rows.length} onChange={(v) => setPicked(v ? new Set(page.rows.map((_, i) => i)) : new Set())} />
                </th>
                {page.columns.map((col) => (
                  <th key={col} className="mono">
                    {col === "_id" && <KeyRound size={11} className="dim" style={{ verticalAlign: -1, marginRight: 4 }} />}
                    {col}
                  </th>
                ))}
                <th />
              </tr>
            </thead>
            <tbody>
              {page.rows.map((r, ri) => (
                <tr key={page.ids[ri] || ri} className={picked.has(ri) ? "sel" : ""}>
                  <td className="pick">
                    <Checkbox
                      checked={picked.has(ri)}
                      onChange={(v) =>
                        setPicked((p) => {
                          const n = new Set(p);
                          if (v) n.add(ri);
                          else n.delete(ri);
                          return n;
                        })
                      }
                    />
                  </td>
                  {r.map((v, ci) =>
                    cell && cell.row === ri && cell.col === ci ? (
                      <td key={ci} className="editing">
                        <input
                          className="mono"
                          autoFocus
                          value={cell.value}
                          onChange={(e) => setCell({ ...cell, value: e.target.value })}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") saveCell(ri, ci, cell.value);
                            if (e.key === "Escape") setCell(null);
                          }}
                          onBlur={() => setCell(null)}
                          spellCheck={false}
                        />
                      </td>
                    ) : (
                      <td
                        key={ci}
                        className={`mono ${v === null ? "null" : ""} ${/^[[{]/.test(v ?? "") ? "nested" : ""}`}
                        title={v ?? "missing / null"}
                        onDoubleClick={() => !ro && page.columns[ci] !== "_id" && setCell({ row: ri, col: ci, value: v ?? "" })}
                      >
                        {v === null ? "—" : v.length > 200 ? v.slice(0, 200) + "…" : v}
                      </td>
                    ),
                  )}
                  <td className="row-acts">
                    <button className="icon-btn" title="Edit document" disabled={ro} onClick={() => setEditing({ index: ri, text: page.docs[ri] })}>
                      <PenLine size={13} />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="mongo-docs">
          {page.docs.map((d, i) => (
            <div key={page.ids[i] || i} className={`mongo-doc ${picked.has(i) ? "sel" : ""}`}>
              <div className="mongo-doc-acts">
                <Checkbox
                  checked={picked.has(i)}
                  onChange={(v) =>
                    setPicked((p) => {
                      const n = new Set(p);
                      if (v) n.add(i);
                      else n.delete(i);
                      return n;
                    })
                  }
                />
                <div className="spacer" />
                <button className="icon-btn" title="Copy JSON" onClick={() => navigator.clipboard.writeText(d)}>
                  <Copy size={13} />
                </button>
                <button className="icon-btn" title="Edit document" disabled={ro} onClick={() => setEditing({ index: i, text: d })}>
                  <PenLine size={13} />
                </button>
              </div>
              <pre className="mono">{d}</pre>
            </div>
          ))}
        </div>
      )}

      {mode !== "indexes" && page && (
        <div className="sg-bar mongo-pager">
          <span className="dim">
            {page.rows.length ? `${(skip + 1).toLocaleString()}–${to.toLocaleString()}` : "0"}
            {total !== null ? ` of ${total.toLocaleString()}` : ""}
          </span>
          <div className="spacer" />
          <Select
            size="sm"
            value={limit}
            options={PAGE_SIZES.map((n) => ({ value: n, label: `${n} per page` }))}
            onChange={(v) => {
              setLimit(Number(v));
              local.set("tabla.mongo.limit", Number(v));
              setSkip(0);
            }}
          />
          <button className="icon-btn" title="Previous page" disabled={skip === 0 || busy} onClick={() => setSkip(Math.max(0, skip - limit))}>
            <ChevronLeft />
          </button>
          <button className="icon-btn" title="Next page" disabled={busy || (total !== null ? to >= total : page.rows.length < limit)} onClick={() => setSkip(skip + limit)}>
            <ChevronRight />
          </button>
        </div>
      )}

      {editing && (
        <DocEditor
          title={editing.index === null ? `Insert into ${collection}` : "Edit document"}
          initial={editing.text}
          onClose={() => setEditing(null)}
          onSave={async (text) => {
            const run = async () => {
              if (editing.index === null) await api.mongoInsert(session.id, db, collection, text);
              else await api.mongoReplace(session.id, db, collection, page!.ids[editing.index], text);
              setEditing(null);
              toast(editing.index === null ? "Document inserted" : "Document saved", "success");
              load();
            };
            if (prod) setConfirm({ title: editing.index === null ? "Insert document in production?" : "Replace document in production?", body: "The change is written immediately.", run: async () => { try { await run(); } catch (e) { toast(errorMessage(e), "error"); } } });
            else await run();
          }}
        />
      )}
      {confirm && (
        <Modal onClose={() => setConfirm(null)}>
          <div className="modal-head">
            <ShieldAlert color="var(--danger)" />
            <div>
              <h2>{confirm.title}</h2>
              <p>{confirm.body}</p>
            </div>
          </div>
          <div className="modal-foot">
            <div className="spacer" />
            <button className="btn" onClick={() => setConfirm(null)}>
              Cancel
            </button>
            <button
              className="btn danger solid"
              autoFocus
              onClick={async () => {
                const c = confirm;
                setConfirm(null);
                await c.run();
              }}
            >
              Continue
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function DocEditor({ title, initial, onClose, onSave }: { title: string; initial: string; onClose(): void; onSave(text: string): Promise<void> }) {
  const [text, setText] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const extensions = useMemo(() => [json(), syntaxHighlighting(highlight), EditorView.lineWrapping], []);
  return (
    <Modal onClose={onClose} className="modal wide">
      <div className="modal-head">
        <FileJson />
        <div>
          <h2>{title}</h2>
          <p>
            Extended JSON or mongosh syntax: <span className="mono">ObjectId("…")</span>, <span className="mono">ISODate("…")</span>, unquoted keys.
          </p>
        </div>
      </div>
      <div className="modal-body">
        <div className="mongo-editor">
          <CodeMirror value={text} onChange={setText} extensions={extensions} theme="none" height="420px" autoFocus basicSetup={{ foldGutter: true, highlightActiveLine: false }} />
        </div>
        {error && <div className="q-error">{error}</div>}
      </div>
      <div className="modal-foot">
        <div className="spacer" />
        <button className="btn" onClick={onClose}>
          Cancel
        </button>
        <button
          className="btn primary"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError(null);
            try {
              await onSave(text);
            } catch (e) {
              setError(errorMessage(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          Save
        </button>
      </div>
    </Modal>
  );
}

function IndexesView({ session, db, collection, ro }: { session: Session; db: string; collection: string; ro: boolean }) {
  const toast = useApp((s) => s.toast);
  const [list, setList] = useState<MongoIndex[] | null>(null);
  const [keys, setKeys] = useState("");
  const [unique, setUnique] = useState(false);
  const load = useCallback(() => {
    api.mongoIndexes(session.id, db, collection).then(setList, (e) => toast(errorMessage(e), "error"));
  }, [session.id, db, collection, toast]);
  useEffect(load, [load]);
  const run = async (line: string) => {
    try {
      await api.mongoShell(session.id, db, line, true);
      load();
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  const coll = `db.getCollection(${JSON.stringify(collection)})`;
  return (
    <div className="structure">
      <section className="sg-panel">
        <div className="sg mongo-ix" role="table">
          <div className="sg-row sg-head">
            <span>Name</span>
            <span>Keys</span>
            <span className="c">Unique</span>
            <span>Options</span>
            <span />
          </div>
          {list?.map((ix) => (
            <div key={ix.name} className="sg-row">
              <span className="mono strong ellipsis">{ix.name}</span>
              <span className="mono ellipsis">{ix.keys}</span>
              <span className="c">{ix.unique ? <span className="badge local">unique</span> : ""}</span>
              <span className="mono ellipsis dim">{ix.options}</span>
              <span className="c">
                {ix.name !== "_id_" && (
                  <button className="icon-btn danger" style={{ width: 24, height: 24 }} title="Drop index" disabled={ro} onClick={() => run(`${coll}.dropIndex(${JSON.stringify(ix.name)})`)}>
                    <Trash2 size={13} />
                  </button>
                )}
              </span>
            </div>
          ))}
        </div>
        <div className="sg-bar">
          <input className="input mono" style={{ flex: 1 }} placeholder="{ email: 1 } or { name: 'text' }" value={keys} onChange={(e) => setKeys(e.target.value)} disabled={ro} spellCheck={false} />
          <Checkbox checked={unique} onChange={setUnique} label="Unique" disabled={ro} />
          <button className="btn" disabled={ro || !keys.trim()} onClick={() => run(`${coll}.createIndex(${keys.trim()}${unique ? ", {unique: true}" : ""})`).then(() => setKeys(""))}>
            <Plus /> Add index
          </button>
        </div>
      </section>
    </div>
  );
}

interface ShellLine {
  id: number;
  cmd: string;
  out: string;
  error?: boolean;
  pending?: boolean;
}

function MongoShell({ session, db, onChanged }: { session: Session; db: string; onChanged(): void }) {
  const prod = session.connection.tag === "production";
  const [lines, setLines] = useState<ShellLine[]>([]);
  const [input, setInput] = useState("");
  const [hist, setHist] = useState<string[]>(() => local.get(`tabla.mongo.hist.${session.connection.id}`, []));
  const [hi, setHi] = useState(-1);
  const outRef = useRef<HTMLDivElement>(null);
  const inRef = useRef<HTMLInputElement>(null);
  const nextId = useRef(1);

  useEffect(() => {
    outRef.current?.scrollTo({ top: outRef.current.scrollHeight });
  }, [lines]);
  useEffect(() => inRef.current?.focus(), []);

  const exec = async (cmd: string, allow: boolean, replaceId?: number) => {
    const id = replaceId ?? nextId.current++;
    try {
      const out = await api.mongoShell(session.id, db, cmd, allow);
      setLines((ls) => [...ls.filter((l) => l.id !== id), { id, cmd, out }]);
      if (allow) onChanged();
    } catch (e) {
      const msg = errorMessage(e);
      setLines((ls) => [...ls.filter((l) => l.id !== id), msg.startsWith("CONFIRM_WRITE") ? { id, cmd, out: "", pending: true } : { id, cmd, out: msg, error: true }]);
    }
  };

  const submit = () => {
    const cmd = input.trim();
    if (!cmd) return;
    if (cmd === "clear" || cmd === "cls") {
      setLines([]);
      setInput("");
      return;
    }
    const h = [cmd, ...hist.filter((x) => x !== cmd)].slice(0, 200);
    setHist(h);
    local.set(`tabla.mongo.hist.${session.connection.id}`, h);
    setHi(-1);
    setInput("");
    // Writes run straight away except on production, where they ask first.
    exec(cmd, !prod);
  };

  const examples = ["show collections", "db.users.find({}).limit(5)", "db.orders.countDocuments({status: 'paid'})", "{ dbStats: 1 }"];

  return (
    <div className="view rcli">
      <div className="rcli-out mono" ref={outRef} onClick={() => inRef.current?.focus()}>
        {lines.length === 0 && (
          <div className="dim" style={{ fontFamily: "var(--font-ui)", fontSize: 13 }}>
            Run mongosh-style commands against <b>{db}</b>. Try:{" "}
            {examples.map((e) => (
              <button key={e} className="kbd rcli-ex" onClick={() => setInput(e)}>
                {e}
              </button>
            ))}
            <br />
            <span style={{ fontSize: 12 }}>
              find, aggregate, countDocuments, distinct, insertOne/Many, updateOne/Many, deleteOne/Many, createIndex, or any {"{command}"} document. Writes ask first on production. Type <b>clear</b> to clear.
            </span>
          </div>
        )}
        {lines.map((l) => (
          <div key={l.id} className="rcli-entry">
            <div className="rcli-cmd">
              <span className="dim">{db}&gt;</span> {l.cmd}
            </div>
            {l.pending ? (
              <div className="rcli-confirm">
                <ShieldAlert size={14} color="var(--warn)" />
                <span>This command changes data{prod ? " on production" : ""}. Run it?</span>
                <button className="btn danger solid" onClick={() => exec(l.cmd, true, l.id)}>
                  Run
                </button>
                <button className="btn ghost" onClick={() => setLines((ls) => ls.filter((x) => x.id !== l.id))}>
                  <X /> Cancel
                </button>
              </div>
            ) : (
              <pre className={`rcli-reply ${l.error ? "err" : ""}`}>{l.out}</pre>
            )}
          </div>
        ))}
      </div>
      <div className="rcli-in">
        <span className="mono dim">{db}&gt;</span>
        <input
          ref={inRef}
          className="mono"
          value={input}
          placeholder="db.collection.find({})"
          spellCheck={false}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
            else if (e.key === "ArrowUp") {
              e.preventDefault();
              const n = Math.min(hist.length - 1, hi + 1);
              if (n >= 0) {
                setHi(n);
                setInput(hist[n]);
              }
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              const n = hi - 1;
              setHi(n);
              setInput(n >= 0 ? hist[n] : "");
            }
          }}
        />
      </div>
    </div>
  );
}
