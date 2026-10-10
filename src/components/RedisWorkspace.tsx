import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Clock,
  Copy,
  KeyRound,
  Minus,
  PenLine,
  Plus,
  Power,
  RefreshCw,
  Save,
  Search,
  ShieldAlert,
  TerminalSquare,
  Trash2,
  X,
} from "lucide-react";
import { api, errorMessage, type RedisEdit, type RedisKey, type RedisValue } from "../lib/api";
import { useApp, local, type Session } from "../lib/store";
import { DriverIcon } from "./icons";
import { Kbd, Modal, TagBadge, useDrag } from "./ui";
import { Select } from "./controls";
import { Console } from "./Console";

const TYPE_COLORS: Record<string, string> = {
  string: "var(--ok)",
  hash: "var(--accent-2)",
  list: "var(--warn)",
  set: "#22b8cf",
  zset: "#e86bd0",
  stream: "#ff8a4c",
};

function formatTtl(ms: number): string {
  if (ms < 0) return "";
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.round(s / 60)}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  return `${Math.round(s / 86400)}d`;
}

function TypeBadge({ type }: { type: string }) {
  return (
    <span className="rtype" style={{ "--c": TYPE_COLORS[type] ?? "var(--text-3)" } as React.CSSProperties}>
      {type === "zset" ? "zset" : type}
    </span>
  );
}

const DANGER = /^(FLUSHALL|FLUSHDB|SHUTDOWN|DEBUG|CONFIG|SWAPDB|MIGRATE|REPLICAOF|SLAVEOF|FAILOVER|KEYS)$/i;

export function RedisWorkspace({ session, visible }: { session: Session; visible: boolean }) {
  const toast = useApp((s) => s.toast);
  const closeSession = useApp((s) => s.closeSession);
  const c = session.connection;
  const prod = c.tag === "production";
  const [db, setDb] = useState(+(session.schema || 0));
  const [dbCounts, setDbCounts] = useState<{ index: number; keys: number }[]>([]);
  const [pattern, setPattern] = useState("");
  const [keys, setKeys] = useState<RedisKey[]>([]);
  const [cursor, setCursor] = useState(0);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<"key" | "console">("key");
  const [creating, setCreating] = useState(false);
  const [logOpen, setLogOpen] = useState(false);
  const [width, setWidth] = useState(() => local.get("tabla.sidebarWidth", 300));
  const startW = useRef(width);
  const resize = useDrag((dx) => {
    const w = Math.max(220, Math.min(560, startW.current + dx));
    setWidth(w);
    local.set("tabla.sidebarWidth", w);
  });
  const listRef = useRef<HTMLDivElement>(null);

  const loadOverview = useCallback(() => {
    api.redisOverview(session.id).then((o) => setDbCounts(o.databases), () => {});
  }, [session.id]);

  const scan = useCallback(
    async (reset: boolean) => {
      setLoading(true);
      try {
        // SCAN can return few keys per call on big keyspaces; keep going until we have a page.
        let cur = reset ? 0 : cursor;
        let got: RedisKey[] = [];
        do {
          const page = await api.redisScan(session.id, pattern.trim() ? (pattern.includes("*") ? pattern : `*${pattern}*`) : "*", cur);
          got = got.concat(page.keys);
          cur = page.cursor;
        } while (cur !== 0 && got.length < 300);
        setKeys((k) => (reset ? got : [...k, ...got.filter((x) => !k.some((y) => y.key === x.key))]).sort((a, b) => a.key.localeCompare(b.key)));
        setCursor(cur);
      } catch (e) {
        toast(errorMessage(e), "error");
      } finally {
        setLoading(false);
      }
    },
    [session.id, pattern, cursor, toast],
  );

  useEffect(() => {
    scan(true);
    loadOverview();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [db, pattern]);

  const switchDb = async (i: number) => {
    try {
      await api.redisSelect(session.id, i);
      setSelected(null);
      setDb(i);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  const virt = useVirtualizer({ count: keys.length, getScrollElement: () => listRef.current, estimateSize: () => 32, overscan: 12 });

  const removeKey = (k: string) => {
    setKeys((ks) => ks.filter((x) => x.key !== k));
    if (selected === k) setSelected(null);
    loadOverview();
  };

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
                {c.host}:{c.port} · v{session.serverVersion}
              </div>
            </div>
            <TagBadge tag={c.tag} />
            <button className="icon-btn" title="Disconnect" onClick={() => closeSession(session.id)}>
              <Power />
            </button>
          </div>
          <div style={{ marginBottom: 8 }}>
            <Select
              size="sm"
              className="w-full"
              value={db}
              title="Database"
              options={(dbCounts.length ? dbCounts : [{ index: db, keys: 0 }]).map((d) => ({
                value: d.index,
                label: `db${d.index}`,
                hint: d.keys ? `${d.keys.toLocaleString()} keys` : "empty",
              }))}
              onChange={(v) => switchDb(Number(v))}
            />
          </div>
          <div className="sb-search">
            <Search />
            <input
              className="input"
              placeholder="Filter keys (user:* or text)"
              defaultValue={pattern}
              onKeyDown={(e) => e.key === "Enter" && setPattern((e.target as HTMLInputElement).value)}
              onBlur={(e) => setPattern(e.target.value)}
              spellCheck={false}
            />
          </div>
        </div>
        <div className="sb-section" style={{ padding: "6px 22px 4px" }}>
          Keys <span className="count">{keys.length.toLocaleString()}{cursor !== 0 ? "+" : ""}</span>
          <div className="spacer" />
          <button className="icon-btn" style={{ width: 22, height: 22 }} title="Refresh" onClick={() => scan(true)}>
            {loading ? <span className="spinner" style={{ width: 11, height: 11 }} /> : <RefreshCw size={12} />}
          </button>
        </div>
        <div className="sb-list" ref={listRef}>
          <div style={{ height: virt.getTotalSize(), position: "relative" }}>
            {virt.getVirtualItems().map((v) => {
              const k = keys[v.index];
              return (
                <button
                  key={k.key}
                  className={`sb-item rkey ${selected === k.key && view === "key" ? "active" : ""}`}
                  style={{ position: "absolute", top: v.start, left: 0, right: 0 }}
                  onClick={() => {
                    setSelected(k.key);
                    setView("key");
                  }}
                  title={k.key}
                >
                  <TypeBadge type={k.type} />
                  <span className="name">{k.key}</span>
                  {k.ttl >= 0 && (
                    <span className="rows" title="Expires in">
                      <Clock size={10} style={{ verticalAlign: -1 }} /> {formatTtl(k.ttl)}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
          {!loading && keys.length === 0 && <div className="sb-empty">{pattern ? "No keys match" : "This database is empty"}</div>}
          {cursor !== 0 && (
            <button className="btn ghost" style={{ width: "100%", justifyContent: "center", marginTop: 6 }} onClick={() => scan(false)} disabled={loading}>
              Load more keys
            </button>
          )}
        </div>
        <div className="sb-foot">
          <button className="btn" onClick={() => setCreating(true)} disabled={c.readOnly}>
            <Plus /> New key
          </button>
          <button className={`btn ${view === "console" ? "primary" : ""}`} onClick={() => setView(view === "console" ? "key" : "console")}>
            <TerminalSquare /> CLI
          </button>
        </div>
      </aside>

      <main className={`main ${prod ? "production" : ""}`}>
        <div className="tabbar" data-tauri-drag-region>
          <div className="tabs-scroll">
            <div className={`tab ${view === "key" ? "active" : ""}`} onMouseDown={() => setView("key")}>
              <KeyRound />
              <span className="title">{selected ?? "Keys"}</span>
            </div>
            <div className={`tab ${view === "console" ? "active" : ""}`} onMouseDown={() => setView("console")}>
              <TerminalSquare />
              <span className="title">CLI</span>
            </div>
          </div>
          <div className="spacer" data-tauri-drag-region style={{ height: "100%" }} />
          <button className={`icon-btn ${logOpen ? "on" : ""}`} onClick={() => setLogOpen(!logOpen)} title="Console: every command Tabla sends">
            <TerminalSquare />
          </button>
        </div>
        {view === "console" ? (
          <RedisCli session={session} onChanged={() => scan(true)} />
        ) : selected ? (
          <KeyView key={`${db}:${selected}`} session={session} keyName={selected} onDeleted={removeKey} onRenamed={(to) => { removeKey(selected); setSelected(to); scan(true); }} />
        ) : (
          <div className="q-message">
            <div className="card dim">
              <KeyRound />
              <div>
                Pick a key on the left, or open the <b>CLI</b> to run Redis commands.
              </div>
            </div>
          </div>
        )}
        {logOpen && <Console session={session} onClose={() => setLogOpen(false)} />}
      </main>

      {creating && (
        <NewKeyDialog
          session={session}
          onClose={() => setCreating(false)}
          onCreated={(k) => {
            setCreating(false);
            scan(true);
            loadOverview();
            setSelected(k);
            setView("key");
          }}
        />
      )}
    </div>
  );
}

function KeyView({ session, keyName, onDeleted, onRenamed }: { session: Session; keyName: string; onDeleted(k: string): void; onRenamed(to: string): void }) {
  const toast = useApp((s) => s.toast);
  const ro = session.connection.readOnly;
  const [val, setVal] = useState<RedisValue | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [ttlInput, setTtlInput] = useState("");
  const [adding, setAdding] = useState<string[]>(["", ""]);

  const load = useCallback(() => {
    setError(null);
    api.redisGet(session.id, keyName).then(
      (v) => {
        setVal(v);
        if (v.type === "string") setDraft(v.rows[0]?.[0] ?? "");
        setTtlInput(v.ttl >= 0 ? String(Math.round(v.ttl / 1000)) : "");
      },
      (e) => setError(errorMessage(e)),
    );
  }, [session.id, keyName]);
  useEffect(load, [load]);
  const saveRef = useRef<() => void>(() => {});
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        saveRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const edit = async (e: RedisEdit, msg = "Saved") => {
    try {
      await api.redisEdit(session.id, e);
      toast(msg, "success");
      load();
    } catch (err) {
      toast(errorMessage(err), "error");
    }
  };

  if (error)
    return (
      <div className="q-error" style={{ margin: 16 }}>
        {error}
      </div>
    );
  if (!val)
    return (
      <div className="q-message">
        <span className="spinner" />
      </div>
    );

  saveRef.current = () => {
    if (val.type === "string" && !ro && draft !== (val.rows[0]?.[0] ?? "")) edit({ op: "setString", key: keyName, value: draft });
  };
  const isJson = val.type === "string" && /^\s*[[{]/.test(draft) && (() => { try { JSON.parse(draft); return true; } catch { return false; } })();
  const header = (
    <div className="toolbar rkey-bar">
      <TypeBadge type={val.type} />
      <span className="crumb mono ellipsis" title={keyName}>
        {keyName}
      </span>
      <span className="dim" style={{ fontSize: 12 }}>
        {val.type === "string" ? `${val.length.toLocaleString()} chars` : `${val.length.toLocaleString()} ${val.type === "hash" ? "fields" : "items"}`}
      </span>
      <div className="spacer" />
      <span className="ttl-box" title="Seconds until the key expires; empty = never">
        <Clock size={13} />
        <input
          className="input sm mono"
          value={ttlInput}
          placeholder="no expiry"
          disabled={ro}
          onChange={(e) => setTtlInput(e.target.value.replace(/\D/g, ""))}
          onKeyDown={(e) => e.key === "Enter" && edit({ op: "expire", key: keyName, seconds: ttlInput ? +ttlInput : null }, ttlInput ? `Expires in ${ttlInput}s` : "Expiry removed")}
        />
        <span className="dim">s</span>
      </span>
      <button className="icon-btn" title="Copy key name" onClick={() => navigator.clipboard.writeText(keyName)}>
        <Copy />
      </button>
      <button className="icon-btn" title="Rename" disabled={ro} onClick={() => setRenaming(keyName)}>
        <PenLine />
      </button>
      <button className="icon-btn" title="Reload" onClick={load}>
        <RefreshCw />
      </button>
      <button className="icon-btn danger" title="Delete key" disabled={ro} onClick={() => setConfirmDelete(true)}>
        <Trash2 />
      </button>
    </div>
  );

  const cols: Record<string, string[]> = { hash: ["Field", "Value"], list: ["Index", "Value"], set: ["Member"], zset: ["Member", "Score"], stream: ["ID", "Fields"] };

  return (
    <div className="view">
      {header}
      {val.type === "string" ? (
        <div className="rstring">
          <textarea className="rtext mono" value={draft} readOnly={ro} spellCheck={false} onChange={(e) => setDraft(e.target.value)} />
          <div className="sg-bar">
            {isJson && (
              <button className="btn ghost" onClick={() => setDraft(JSON.stringify(JSON.parse(draft), null, 2))}>
                Format JSON
              </button>
            )}
            <div className="spacer" />
            {draft !== (val.rows[0]?.[0] ?? "") && (
              <>
                <button className="btn ghost" onClick={() => setDraft(val.rows[0]?.[0] ?? "")}>
                  Discard
                </button>
                <button className="btn primary" disabled={ro} onClick={() => edit({ op: "setString", key: keyName, value: isJson ? JSON.stringify(JSON.parse(draft)) : draft })}>
                  <Save /> Save <Kbd keys="Mod S" />
                </button>
              </>
            )}
          </div>
        </div>
      ) : (
        <div className="structure">
          {val.truncated && (
            <p className="hint" style={{ marginTop: 0 }}>
              Showing the first {val.rows.length.toLocaleString()} of {val.length.toLocaleString()}. Use the CLI for the rest.
            </p>
          )}
          <section className="sg-panel">
            <div className={`sg rv ${val.type}`} role="table">
              <div className="sg-row sg-head">
                {cols[val.type]?.map((h) => <span key={h}>{h}</span>)}
                <span />
              </div>
              {val.rows.map((row, i) => (
                <RedisRow key={`${row[0]}-${i}`} type={val.type} row={row} ro={ro || val.type === "stream"} keyName={keyName} edit={edit} />
              ))}
              {val.rows.length === 0 && <div className="sg-empty">Empty</div>}
            </div>
            {val.type !== "stream" && !ro && (
              <div className="sg-bar radd">
                {val.type === "list" || val.type === "set" ? (
                  <input className="input sm mono" placeholder={val.type === "list" ? "New item (appended)" : "New member"} value={adding[0]} onChange={(e) => setAdding([e.target.value, adding[1]])} />
                ) : (
                  <>
                    <input className="input sm mono" placeholder={val.type === "hash" ? "Field" : "Member"} value={adding[0]} onChange={(e) => setAdding([e.target.value, adding[1]])} />
                    <input className="input sm mono" placeholder={val.type === "hash" ? "Value" : "Score"} value={adding[1]} onChange={(e) => setAdding([adding[0], e.target.value])} />
                  </>
                )}
                <button
                  className="btn"
                  disabled={!adding[0] || (val.type === "zset" && isNaN(+adding[1]))}
                  onClick={() => {
                    const [a, b] = adding;
                    const e: RedisEdit =
                      val.type === "hash"
                        ? { op: "hashSet", key: keyName, field: a, value: b }
                        : val.type === "list"
                          ? { op: "listPush", key: keyName, value: a }
                          : val.type === "set"
                            ? { op: "setAdd", key: keyName, member: a }
                            : { op: "zAdd", key: keyName, member: a, score: +b || 0 };
                    edit(e, "Added");
                    setAdding(["", ""]);
                  }}
                >
                  <Plus /> Add
                </button>
              </div>
            )}
          </section>
        </div>
      )}

      {confirmDelete && (
        <Modal onClose={() => setConfirmDelete(false)}>
          <div className="modal-head">
            <ShieldAlert color="var(--danger)" />
            <div>
              <h2>Delete “{keyName}”{session.connection.tag === "production" ? " on production" : ""}?</h2>
              <p>The key and its value are removed immediately (UNLINK).</p>
            </div>
          </div>
          <div className="modal-foot">
            <div className="spacer" />
            <button className="btn" onClick={() => setConfirmDelete(false)}>
              Cancel
            </button>
            <button
              className="btn danger solid"
              autoFocus
              onClick={async () => {
                setConfirmDelete(false);
                try {
                  await api.redisEdit(session.id, { op: "delete", keys: [keyName] });
                  toast(`Deleted ${keyName}`, "success");
                  onDeleted(keyName);
                } catch (e) {
                  toast(errorMessage(e), "error");
                }
              }}
            >
              Delete key
            </button>
          </div>
        </Modal>
      )}
      {renaming !== null && (
        <Modal onClose={() => setRenaming(null)}>
          <div className="modal-head">
            <PenLine color="var(--accent-2)" />
            <div>
              <h2>Rename key</h2>
              <p>Fails if a key with the new name already exists, so nothing is overwritten.</p>
            </div>
          </div>
          <div className="modal-body">
            <input className="input mono" autoFocus value={renaming} onChange={(e) => setRenaming(e.target.value)} spellCheck={false} />
          </div>
          <div className="modal-foot">
            <div className="spacer" />
            <button className="btn" onClick={() => setRenaming(null)}>
              Cancel
            </button>
            <button
              className="btn primary"
              disabled={!renaming.trim() || renaming === keyName}
              onClick={async () => {
                const to = renaming.trim();
                try {
                  await api.redisEdit(session.id, { op: "rename", key: keyName, to });
                  setRenaming(null);
                  toast(`Renamed to ${to}`, "success");
                  onRenamed(to);
                } catch (e) {
                  toast(errorMessage(e), "error");
                }
              }}
            >
              Rename
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function RedisRow({ type, row, ro, keyName, edit }: { type: string; row: string[]; ro: boolean; keyName: string; edit(e: RedisEdit, msg?: string): void }) {
  const editable = type === "hash" || type === "list" || type === "zset";
  const valueIdx = type === "set" ? 0 : 1;
  const [draft, setDraft] = useState(row[valueIdx] ?? "");
  useEffect(() => setDraft(row[valueIdx] ?? ""), [row, valueIdx]);
  const changed = draft !== (row[valueIdx] ?? "");
  const sent = useRef<string | null>(null);
  const save = () => {
    if (!changed || sent.current === draft) return;
    sent.current = draft;
    if (type === "hash") edit({ op: "hashSet", key: keyName, field: row[0], value: draft });
    else if (type === "list") edit({ op: "listSet", key: keyName, index: +row[0], value: draft });
    else if (type === "zset" && !isNaN(+draft)) edit({ op: "zAdd", key: keyName, member: row[0], score: +draft });
  };
  const remove = () =>
    edit(
      type === "hash"
        ? { op: "hashDel", key: keyName, field: row[0] }
        : type === "list"
          ? { op: "listRemove", key: keyName, value: row[1] }
          : type === "set"
            ? { op: "setRemove", key: keyName, member: row[0] }
            : { op: "zRemove", key: keyName, member: row[0] },
      "Removed",
    );
  return (
    <div className={`sg-row ${changed ? "changed" : ""}`}>
      {type !== "set" && <span className="mono strong ellipsis" title={row[0]}>{row[0]}</span>}
      {editable && !ro ? (
        <input
          className="sg-in mono"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={save}
          onKeyDown={(e) => {
            if (e.key === "Enter") save();
            else if (e.key === "Escape") setDraft(row[valueIdx] ?? "");
          }}
          spellCheck={false}
        />
      ) : (
        <span className="mono ellipsis" title={row[valueIdx]} style={{ padding: "0 8px", userSelect: "text" }}>
          {row[valueIdx]}
        </span>
      )}
      <span className="c">
        {!ro && (
          <button className="icon-btn danger" style={{ width: 24, height: 24 }} title="Remove" onClick={remove}>
            <Minus size={13} />
          </button>
        )}
      </span>
    </div>
  );
}

function NewKeyDialog({ session, onClose, onCreated }: { session: Session; onClose(): void; onCreated(k: string): void }) {
  const toast = useApp((s) => s.toast);
  const [key, setKey] = useState("");
  const [type, setType] = useState("string");
  const [a, setA] = useState("");
  const [b, setB] = useState("");
  const [ttl, setTtl] = useState("");
  const create = async () => {
    const k = key.trim();
    const e: RedisEdit =
      type === "string"
        ? { op: "setString", key: k, value: a }
        : type === "hash"
          ? { op: "hashSet", key: k, field: a, value: b }
          : type === "list"
            ? { op: "listPush", key: k, value: a }
            : type === "set"
              ? { op: "setAdd", key: k, member: a }
              : { op: "zAdd", key: k, member: a, score: +b || 0 };
    try {
      await api.redisEdit(session.id, e);
      if (ttl) await api.redisEdit(session.id, { op: "expire", key: k, seconds: +ttl });
      toast(`Created ${k}`, "success");
      onCreated(k);
    } catch (err) {
      toast(errorMessage(err), "error");
    }
  };
  const labels: Record<string, [string, string?]> = { string: ["Value"], hash: ["Field", "Value"], list: ["First item"], set: ["First member"], zset: ["Member", "Score"] };
  return (
    <Modal onClose={onClose}>
      <div className="modal-head">
        <Plus color="var(--accent-2)" />
        <div>
          <h2>New key</h2>
          <p>Redis creates the key when its first value is written.</p>
        </div>
      </div>
      <div className="modal-body">
        <div className="grid-2">
          <div className="field">
            <label>Key</label>
            <input className="input mono" autoFocus placeholder="user:42:profile" value={key} onChange={(e) => setKey(e.target.value)} spellCheck={false} />
          </div>
          <div className="field">
            <label>Type</label>
            <Select className="w-full" value={type} options={["string", "hash", "list", "set", "zset"].map((t) => ({ value: t, label: t }))} onChange={(v) => setType(String(v))} />
          </div>
        </div>
        <div className={labels[type][1] ? "grid-2b" : ""}>
          <div className="field">
            <label>{labels[type][0]}</label>
            {type === "string" ? (
              <textarea className="rtext mono" style={{ minHeight: 110 }} value={a} onChange={(e) => setA(e.target.value)} spellCheck={false} />
            ) : (
              <input className="input mono" value={a} onChange={(e) => setA(e.target.value)} spellCheck={false} />
            )}
          </div>
          {labels[type][1] && (
            <div className="field">
              <label>{labels[type][1]}</label>
              <input className="input mono" value={b} onChange={(e) => setB(e.target.value)} spellCheck={false} />
            </div>
          )}
        </div>
        <div className="field">
          <label>Expire after (seconds, optional)</label>
          <input className="input mono" value={ttl} onChange={(e) => setTtl(e.target.value.replace(/\D/g, ""))} placeholder="never" />
        </div>
      </div>
      <div className="modal-foot">
        <div className="spacer" />
        <button className="btn" onClick={onClose}>
          Cancel
        </button>
        <button className="btn primary" disabled={!key.trim() || (type !== "string" && !a)} onClick={create}>
          Create key
        </button>
      </div>
    </Modal>
  );
}

interface CliLine {
  id: number;
  cmd: string;
  out: string;
  error?: boolean;
  pending?: boolean;
}

function RedisCli({ session, onChanged }: { session: Session; onChanged(): void }) {
  const prod = session.connection.tag === "production";
  const [lines, setLines] = useState<CliLine[]>([]);
  const [input, setInput] = useState("");
  const [hist, setHist] = useState<string[]>(() => local.get(`tabla.redis.hist.${session.connection.id}`, []));
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
      const out = await api.redisCommand(session.id, cmd, allow);
      setLines((ls) => [...ls.filter((l) => l.id !== id), { id, cmd, out }]);
      if (allow) onChanged();
    } catch (e) {
      const msg = errorMessage(e);
      setLines((ls) => [...ls.filter((l) => l.id !== id), msg === "needs-confirmation" ? { id, cmd, out: "", pending: true } : { id, cmd, out: msg, error: true }]);
    }
  };

  const submit = () => {
    const cmd = input.trim();
    if (!cmd) return;
    if (cmd.toLowerCase() === "clear") {
      setLines([]);
      setInput("");
      return;
    }
    const h = [cmd, ...hist.filter((x) => x !== cmd)].slice(0, 200);
    setHist(h);
    local.set(`tabla.redis.hist.${session.connection.id}`, h);
    setHi(-1);
    setInput("");
    const name = cmd.split(/\s+/)[0];
    // Writes run straight away except on production; dangerous commands always ask.
    exec(cmd, !prod && !DANGER.test(name));
  };

  const examples = useMemo(() => ["SCAN 0 MATCH user:* COUNT 100", "INFO memory", "TTL session:abc", "HGETALL user:1"], []);

  return (
    <div className="view rcli">
      <div className="rcli-out mono" ref={outRef} onClick={() => inRef.current?.focus()}>
        {lines.length === 0 && (
          <div className="dim" style={{ fontFamily: "var(--font-ui)", fontSize: 13 }}>
            Run any Redis command. Try:{" "}
            {examples.map((e) => (
              <button key={e} className="kbd rcli-ex" onClick={() => setInput(e)}>
                {e}
              </button>
            ))}
            <br />
            <span style={{ fontSize: 12 }}>
              Writes ask for confirmation on production; FLUSHALL, KEYS and other risky commands always do. Type <b>clear</b> to clear.
            </span>
          </div>
        )}
        {lines.map((l) => (
          <div key={l.id} className="rcli-entry">
            <div className="rcli-cmd">
              <span className="dim">{session.connection.name}&gt;</span> {l.cmd}
            </div>
            {l.pending ? (
              <div className="rcli-confirm">
                <ShieldAlert size={14} color="var(--warn)" />
                <span>
                  {DANGER.test(l.cmd.split(/\s+/)[0]) ? "This command can wipe data or stall the server" : "This command changes data"}
                  {prod ? " on production" : ""}. Run it?
                </span>
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
        <span className="mono dim">{session.connection.name}&gt;</span>
        <input
          ref={inRef}
          className="mono"
          value={input}
          placeholder="GET key"
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
