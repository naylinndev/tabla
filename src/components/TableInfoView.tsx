import { useEffect, useMemo, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { MSSQL, MySQL, PostgreSQL, SQLite, sql } from "@codemirror/lang-sql";
import { Check, Copy, RefreshCw } from "lucide-react";
import { api, errorMessage, type TableDetails } from "../lib/api";
import { formatBytes, formatNumber } from "../lib/format";
import type { Session } from "../lib/store";

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.operatorKeyword, t.modifier], color: "var(--syn-keyword)", fontWeight: "500" },
  { tag: [t.string, t.special(t.string)], color: "var(--syn-string)" },
  { tag: [t.number, t.bool, t.null], color: "var(--syn-number)" },
  { tag: [t.comment, t.lineComment, t.blockComment], color: "var(--syn-comment)" },
  { tag: [t.typeName, t.function(t.variableName), t.standard(t.name)], color: "var(--syn-fn)" },
]);
const DIALECT: Record<string, typeof PostgreSQL> = { postgres: PostgreSQL, mysql: MySQL, sqlite: SQLite, redis: PostgreSQL, mssql: MSSQL };

export function TableInfoView({ session, schema, table }: { session: Session; schema: string; table: string }) {
  const [info, setInfo] = useState<TableDetails | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [rev, setRev] = useState(0);

  useEffect(() => {
    setError(null);
    api.tableDetails(session.id, schema, table).then(setInfo, (e) => setError(errorMessage(e)));
  }, [session.id, schema, table, rev]);

  const extensions = useMemo(
    () => [sql({ dialect: DIALECT[session.connection.driver] }), syntaxHighlighting(highlight), EditorView.lineWrapping, EditorView.editable.of(false)],
    [session.connection.driver],
  );

  if (error) return <div className="q-error">{error}</div>;
  if (!info)
    return (
      <div className="q-message">
        <span className="spinner" />
      </div>
    );

  const fmt = (kind: string, v: string) => (kind === "bytes" ? formatBytes(Number(v)) : kind === "number" ? formatNumber(v) : v);
  const head: [string, string][] = [
    ["Engine", info.engine ?? "—"],
    ["Collation", info.collation ?? "—"],
    ["Created", info.created ?? "Not available"],
    ["Updated", info.updated ?? "Not available"],
  ];
  const stats: [string, string][] = [
    ["Number of rows", info.rowsEstimate == null ? "—" : `~${formatNumber(info.rowsEstimate)}`],
    [session.connection.driver === "sqlite" ? "Database file size" : "Total size", formatBytes(info.totalBytes)],
    ...info.stats.map((s): [string, string] => [s.label, fmt(s.kind, s.value)]),
  ];

  return (
    <div className="structure info-view">
      <div className="info-grid">
        {head.map(([k, v]) => (
          <div className="kv" key={k}>
            <span>{k}</span>
            <b className="ellipsis" title={v}>
              {v}
            </b>
          </div>
        ))}
      </div>
      <div className="info-sep" />
      <div className="info-grid">
        {stats.map(([k, v]) => (
          <div className="kv" key={k}>
            <span>{k}</span>
            <b>{v}</b>
          </div>
        ))}
      </div>
      <div className="info-sep" />
      <div className="kv-block">
        <span>Comment</span>
        <div className="info-comment">{info.comment || <span className="dim">No comment</span>}</div>
      </div>
      <div className="kv-block grow">
        <span>Create syntax</span>
        <div className="code-block ddl">
          <div className="code-head">
            <span>{session.connection.driver === "postgres" ? "Reconstructed from the catalog" : "From the server"}</span>
            <div style={{ display: "flex", gap: 2 }}>
              <button className="btn ghost" onClick={() => setRev((r) => r + 1)} title="Refresh">
                <RefreshCw />
              </button>
              <button
                className="btn ghost"
                onClick={async () => {
                  await navigator.clipboard.writeText(info.ddl ?? "");
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1500);
                }}
              >
                {copied ? <Check /> : <Copy />} {copied ? "Copied" : "Copy"}
              </button>
            </div>
          </div>
          <CodeMirror value={info.ddl ?? "-- Not available"} extensions={extensions} theme="none" basicSetup={{ lineNumbers: false, foldGutter: false, highlightActiveLine: false }} />
        </div>
      </div>
    </div>
  );
}
