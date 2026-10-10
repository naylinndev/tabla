import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import CodeMirror, { type ReactCodeMirrorRef } from "@uiw/react-codemirror";
import { EditorView, keymap } from "@codemirror/view";
import { Prec } from "@codemirror/state";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { MySQL, PostgreSQL, SQLite, sql } from "@codemirror/lang-sql";
import { format as formatSql } from "sql-formatter";
import {
  AlertTriangle,
  CheckCircle2,
  Clock,
  Copy,
  Download,
  History,
  Play,
  PlayCircle,
  Sparkles,
  TerminalSquare,
  X,
  XCircle,
} from "lucide-react";
import { api, errorMessage, type QueryOutcome } from "../lib/api";
import { pushHistory, readHistory, useApp, type QueryTab, type Session } from "../lib/store";
import { resultSummary, splitStatements, statementAt, toCSV, toJSON, toTSV } from "../lib/sql";
import { DataGrid, type Cursor } from "./DataGrid";
import { Kbd, useDrag } from "./ui";

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.operatorKeyword, t.modifier], color: "var(--syn-keyword)", fontWeight: "500" },
  { tag: [t.string, t.special(t.string)], color: "var(--syn-string)" },
  { tag: [t.number, t.bool, t.null], color: "var(--syn-number)" },
  { tag: [t.lineComment, t.blockComment, t.comment], color: "var(--syn-comment)", fontStyle: "italic" },
  { tag: [t.function(t.variableName), t.standard(t.name), t.typeName], color: "var(--syn-fn)" },
  { tag: [t.operator, t.punctuation, t.bracket], color: "var(--syn-op)" },
  { tag: [t.special(t.name), t.name, t.propertyName], color: "var(--text)" },
]);

const baseTheme = EditorView.theme({
  "&": { height: "100%", fontSize: "13px" },
  ".cm-content": { padding: "12px 0" },
  ".cm-line": { padding: "0 14px 0 6px" },
  ".cm-gutterElement": { padding: "0 6px 0 14px !important" },
});

const DIALECT = { postgres: PostgreSQL, mysql: MySQL, sqlite: SQLite };

export function QueryView({ session, tab, active }: { session: Session; tab: QueryTab; active: boolean }) {
  const updateTab = useApp((s) => s.updateTab);
  const toast = useApp((s) => s.toast);
  const setColumns = useApp((s) => s.setColumns);
  const cm = useRef<ReactCodeMirrorRef>(null);
  const wrapRef = useRef<HTMLDivElement>(null);

  const [running, setRunning] = useState(false);
  const [outcome, setOutcome] = useState<QueryOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [resultIdx, setResultIdx] = useState(0);
  const [editorH, setEditorH] = useState(260);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [cursor, setCursor] = useState<Cursor | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const startH = useRef(editorH);
  const drag = useDrag(
    (_, dy) => {
      const max = (wrapRef.current?.clientHeight ?? 800) - 120;
      setEditorH(Math.max(90, Math.min(max, startH.current + dy)));
    },
    () => (startH.current = editorH),
  );

  // Prefetch column names for autocomplete on small schemas.
  useEffect(() => {
    if (!active || session.tables.length > 80) return;
    const missing = session.tables.filter((tb) => !session.columns[tb.name]).slice(0, 80);
    let cancelled = false;
    (async () => {
      for (const tb of missing) {
        if (cancelled) return;
        try {
          const st = await api.tableStructure(session.id, session.schema, tb.name);
          setColumns(session.id, tb.name, st.columns.map((c) => c.name));
        } catch {
          /* autocomplete is best effort */
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, session.tables, session.schema]);

  const schemaMap = useMemo(() => {
    const m: Record<string, string[]> = {};
    for (const tb of session.tables) m[tb.name] = session.columns[tb.name] ?? [];
    return m;
  }, [session.tables, session.columns]);

  const run = useCallback(
    async (mode: "current" | "all") => {
      const view = cm.current?.view;
      if (!view || running) return;
      const doc = view.state.doc.toString();
      const sel = view.state.selection.main;
      const text =
        mode === "all"
          ? doc
          : !sel.empty
            ? view.state.sliceDoc(sel.from, sel.to)
            : statementAt(doc, sel.head);
      if (!text.trim()) return;
      setRunning(true);
      setError(null);
      const started = performance.now();
      try {
        const res = await api.runQuery(session.id, text);
        setOutcome(res);
        setResultIdx(Math.max(0, res.results.findIndex((r) => r.columns.length)));
        setCursor(null);
        setSelected(new Set());
        pushHistory(session.connection.id, { sql: text.trim(), at: Date.now(), ms: res.elapsedMs, ok: true });
        // DDL may have changed the table list.
        if (/^\s*(create|drop|alter|rename)\b/im.test(text)) useApp.getState().refreshTables(session.id);
      } catch (e) {
        setError(errorMessage(e));
        setOutcome(null);
        pushHistory(session.connection.id, {
          sql: text.trim(),
          at: Date.now(),
          ms: Math.round(performance.now() - started),
          ok: false,
        });
      } finally {
        setRunning(false);
      }
    },
    [running, session.id, session.connection.id],
  );

  const doFormat = useCallback(() => {
    const view = cm.current?.view;
    if (!view) return;
    try {
      const lang = session.connection.driver === "postgres" ? "postgresql" : session.connection.driver;
      const out = formatSql(view.state.doc.toString(), { language: lang, keywordCase: "upper", tabWidth: 2 });
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: out } });
    } catch (e) {
      toast(`Couldn't format: ${errorMessage(e)}`, "error");
    }
  }, [session.connection.driver, toast]);

  const runRef = useRef(run);
  runRef.current = run;
  const formatRef = useRef(doFormat);
  formatRef.current = doFormat;

  const extensions = useMemo(
    () => [
      sql({ dialect: DIALECT[session.connection.driver], schema: schemaMap, upperCaseKeywords: true }),
      syntaxHighlighting(highlight),
      baseTheme,
      EditorView.lineWrapping,
      Prec.highest(
        keymap.of([
          { key: "Mod-Enter", run: () => (runRef.current("current"), true) },
          { key: "Shift-Mod-Enter", run: () => (runRef.current("all"), true) },
          { key: "Shift-Mod-f", run: () => (formatRef.current(), true) },
        ]),
      ),
    ],
    [session.connection.driver, schemaMap],
  );

  useEffect(() => {
    if (active) setTimeout(() => cm.current?.view?.focus(), 0);
  }, [active]);

  const result = outcome?.results[resultIdx];
  const statementCount = useMemo(() => splitStatements(tab.sql).length, [tab.sql]);
  const columns = useMemo(
    () => result?.columns.map((c) => ({ name: c.name, typeName: c.typeName })) ?? [],
    [result],
  );
  const getCell = useCallback((r: number, c: number) => result?.rows[r]?.[c] ?? null, [result]);

  const exportAs = async (fmt: "csv" | "json") => {
    if (!result) return;
    const contents = fmt === "csv" ? toCSV(result.columns, result.rows) : toJSON(result.columns, result.rows);
    try {
      const path = await api.exportFile(`query-result.${fmt}`, contents);
      if (path) toast(`Exported ${result.rows.length.toLocaleString()} rows`, "success");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  const history = historyOpen ? readHistory(session.connection.id) : [];

  return (
    <div className="view" hidden={!active} ref={wrapRef}>
      <div className="toolbar">
        <button className="btn primary" onClick={() => run("current")} disabled={running} title="Run statement at cursor or selection">
          {running ? <span className="spinner" style={{ borderTopColor: "#fff" }} /> : <Play />}
          Run <Kbd keys="Mod ↵" />
        </button>
        <button className="btn" onClick={() => run("all")} disabled={running || statementCount === 0} title="Run every statement">
          <PlayCircle /> Run all{statementCount > 1 ? ` (${statementCount})` : ""}
        </button>
        <div className="divider" />
        <button className="btn ghost" onClick={doFormat} title="Format SQL (Mod+Shift+F)">
          <Sparkles /> Format
        </button>
        <div className="spacer" />
        <button className={`btn ghost ${historyOpen ? "on" : ""}`} onClick={() => setHistoryOpen(!historyOpen)}>
          <History /> History
        </button>
      </div>

      {historyOpen && (
        <div className="popover history">
          <div className="insp-head">
            <History size={14} /> Recent queries
            <div className="spacer" />
            <button className="icon-btn" onClick={() => setHistoryOpen(false)}>
              <X />
            </button>
          </div>
          <div className="history-list">
            {history.length === 0 && <div className="sb-empty">Queries you run will appear here.</div>}
            {history.map((h) => (
              <button
                key={h.at}
                className="history-item"
                onClick={() => {
                  updateTab(session.id, tab.id, { sql: h.sql });
                  setHistoryOpen(false);
                }}
              >
                <code>{h.sql.replace(/\s+/g, " ")}</code>
                <small>
                  {h.ok ? <CheckCircle2 size={11} color="var(--ok)" /> : <XCircle size={11} color="var(--danger)" />}
                  {new Date(h.at).toLocaleString()} · {h.ms} ms
                </small>
              </button>
            ))}
          </div>
        </div>
      )}

      <div className="q-editor" style={{ height: editorH }}>
        <CodeMirror
          ref={cm}
          value={tab.sql}
          onChange={(v) => updateTab(session.id, tab.id, { sql: v })}
          extensions={extensions}
          theme="none"
          placeholder={`-- Write SQL here. ${navigator.platform.includes("Mac") ? "⌘" : "Ctrl"}+Enter runs the statement under the cursor.`}
          basicSetup={{ highlightActiveLine: true, foldGutter: false, autocompletion: true, bracketMatching: true }}
          height="100%"
        />
      </div>
      <div
        className={`q-split ${drag.active ? "on" : ""}`}
        onMouseDown={(e) => {
          startH.current = editorH;
          drag.start(e);
        }}
      />

      <div className="q-results">
        <div className="q-result-tabs">
          {outcome && outcome.results.length > 1 && (
            <div className="segmented">
              {outcome.results.map((r, i) => (
                <button key={i} className={i === resultIdx ? "on" : ""} onClick={() => setResultIdx(i)}>
                  {r.columns.length ? `Result ${i + 1}` : `Statement ${i + 1}`}
                </button>
              ))}
            </div>
          )}
          <div className="q-meta">
            {running && (
              <>
                <span className="spinner" /> Running…
              </>
            )}
            {!running && outcome && result && (
              <>
                <span className="ok">
                  <CheckCircle2 size={12} style={{ verticalAlign: -2 }} /> {resultSummary(result)}
                </span>
                <span>
                  <Clock size={12} style={{ verticalAlign: -2 }} /> {outcome.elapsedMs} ms
                </span>
                {result.truncated && (
                  <span className="warn">
                    <AlertTriangle size={12} style={{ verticalAlign: -2 }} /> Showing first {result.rows.length.toLocaleString()} rows
                  </span>
                )}
              </>
            )}
          </div>
          <div className="spacer" />
          {result && result.columns.length > 0 && (
            <>
              <button
                className="icon-btn"
                title="Copy all as TSV"
                onClick={async () => {
                  await navigator.clipboard.writeText(
                    toTSV([result.columns.map((c) => c.name), ...result.rows]),
                  );
                  toast("Copied result", "success");
                }}
              >
                <Copy />
              </button>
              <button className="btn ghost" onClick={() => exportAs("csv")}>
                <Download /> CSV
              </button>
              <button className="btn ghost" onClick={() => exportAs("json")}>
                <Download /> JSON
              </button>
            </>
          )}
        </div>
        {error ? (
          <div className="q-error">
            <XCircle />
            {error}
          </div>
        ) : result && result.columns.length ? (
          <DataGrid
            columns={columns}
            rowCount={result.rows.length}
            getCell={getCell}
            cursor={cursor}
            onCursor={setCursor}
            selectedRows={selected}
            onSelectRows={setSelected}
            emptyText="Query returned no rows"
            onCopied={(w) => toast(`Copied ${w}`, "success")}
          />
        ) : result ? (
          <div className="q-message">
            <div className="card">
              <CheckCircle2 color="var(--ok)" />
              <div>
                Statement executed · <b>{result.rowsAffected.toLocaleString()}</b> row{result.rowsAffected === 1 ? "" : "s"}{" "}
                affected
              </div>
            </div>
          </div>
        ) : (
          <div className="q-message">
            <div className="card dim">
              <TerminalSquare />
              <div>
                Run a query with <Kbd keys="Mod ↵" /> · run everything with <Kbd keys="Mod Shift ↵" />
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
