import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CheckCircle2, Copy, Download, Search, Trash2, X, XCircle } from "lucide-react";
import { api, errorMessage } from "../lib/api";
import { useApp, type Session } from "../lib/store";
import { Checkbox } from "./controls";

interface LogEntry {
  id: number;
  at: number;
  sql: string;
  ms: number;
  ok: boolean;
  info: string;
}

/** Live log of every statement Tabla sends for this connection (memory only). */
export function Console({ session, onClose }: { session: Session; onClose(): void }) {
  const toast = useApp((s) => s.toast);
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [q, setQ] = useState("");
  const [hideMeta, setHideMeta] = useState(false);
  const last = useRef(0);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let alive = true;
    const poll = async () => {
      try {
        const fresh = await invoke<LogEntry[]>("query_log", { sessionId: session.id, after: last.current });
        if (!alive || !fresh.length) return;
        last.current = fresh[fresh.length - 1].id;
        setEntries((e) => [...e, ...fresh].slice(-500));
        requestAnimationFrame(() => listRef.current?.scrollTo({ top: listRef.current.scrollHeight }));
      } catch {
        /* session closed */
      }
    };
    poll();
    const t = setInterval(poll, 800);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [session.id]);

  const isMeta = (sql: string) => /information_schema|pg_catalog|pg_class|pg_namespace|pg_attribute|pg_index|sqlite_master|pragma_|duckdb_(tables|views|columns|constraints|indexes|schemas|databases)\(\)|SHOW (server_version|CREATE)|SELECT (VERSION|sqlite_version|current_schema|DATABASE)\(\)/i.test(sql);
  const shown = useMemo(
    () => entries.filter((e) => (!hideMeta || !isMeta(e.sql)) && (!q || e.sql.toLowerCase().includes(q.toLowerCase()))),
    [entries, q, hideMeta],
  );
  const asText = () => shown.map((e) => `-- ${new Date(e.at).toLocaleString()} · ${e.ms} ms · ${e.ok ? e.info : "ERROR: " + e.info}\n${e.sql};`).join("\n\n");

  return (
    <div className="console">
      <div className="console-head">
        <b>Console</b>
        <span className="dim">{session.connection.name}</span>
        <div className="sb-search console-search">
          <Search />
          <input className="input" placeholder="Filter queries" value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} />
        </div>
        <Checkbox checked={hideMeta} onChange={setHideMeta} label="Hide Tabla's metadata queries" />
        <div className="spacer" />
        <button className="icon-btn" title="Copy shown" onClick={() => navigator.clipboard.writeText(asText()).then(() => toast("Console copied", "success"))}>
          <Copy />
        </button>
        <button
          className="icon-btn"
          title="Save as…"
          onClick={async () => {
            try {
              const p = await api.exportFile("tabla-console.sql", asText());
              if (p) toast("Console saved", "success");
            } catch (e) {
              toast(errorMessage(e), "error");
            }
          }}
        >
          <Download />
        </button>
        <button
          className="icon-btn"
          title="Clear"
          onClick={async () => {
            await invoke("clear_query_log", { sessionId: session.id }).catch(() => {});
            setEntries([]);
          }}
        >
          <Trash2 />
        </button>
        <button className="icon-btn" title="Close console" onClick={onClose}>
          <X />
        </button>
      </div>
      <div className="console-list" ref={listRef}>
        <div className="console-row head">
          <span>Time</span>
          <span>Query</span>
          <span>Result</span>
          <span className="r">Duration</span>
        </div>
        {shown.length === 0 && <div className="sg-empty">Queries Tabla sends to the server appear here.</div>}
        {shown.map((e) => (
          <div key={e.id} className={`console-row ${e.ok ? "" : "err"}`} title={e.sql}>
            <span className="dim">{new Date(e.at).toLocaleTimeString()}</span>
            <span className="mono ellipsis">{e.sql.replace(/\s+/g, " ")}</span>
            <span className="ellipsis">
              {e.ok ? <CheckCircle2 className="ok-ico" /> : <XCircle className="err-ico" />} {e.info}
            </span>
            <span className="r dim">{e.ms} ms</span>
          </div>
        ))}
      </div>
    </div>
  );
}
