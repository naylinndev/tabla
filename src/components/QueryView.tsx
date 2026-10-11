import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import CodeMirror, { type ReactCodeMirrorRef } from "@uiw/react-codemirror";
import { EditorView, keymap } from "@codemirror/view";
import { Prec } from "@codemirror/state";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { MSSQL, MySQL, PostgreSQL, SQLite, StandardSQL, sql } from "@codemirror/lang-sql";
import { format as formatSql } from "sql-formatter";
import {
  AlertTriangle,
  BookMarked,
  Square,
  Save,
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
import { local, pushHistory, readHistory, useApp, type QueryTab, type Session } from "../lib/store";
import { resultSummary, splitStatements, statementAt, toCSV, toJSON, toTSV } from "../lib/sql";
import { DataGrid, type Cursor } from "./DataGrid";
import { snippetsFor } from "../lib/snippets";
import { Select } from "./controls";
import { Modal } from "./ui";
import { engineLabel, schemaLines, tablesIn } from "../lib/ai";
import { aiFixPrompt } from "../lib/errorHelp";
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

const DIALECT: Record<string, typeof PostgreSQL> = { postgres: PostgreSQL, mysql: MySQL, sqlite: SQLite, redis: PostgreSQL, mssql: MSSQL, clickhouse: StandardSQL };

export function QueryView({ session, tab, active }: { session: Session; tab: QueryTab; active: boolean }) {
  const updateTab = useApp((s) => s.updateTab);
  const toast = useApp((s) => s.toast);
  const setColumns = useApp((s) => s.setColumns);
  const cm = useRef<ReactCodeMirrorRef>(null);
  const lastSql = useRef("");
  const wrapRef = useRef<HTMLDivElement>(null);

  const [running, setRunning] = useState(false);
  const queryIdRef = useRef<string | null>(null);
  const [progress, setProgress] = useState(0);
  const [elapsed, setElapsed] = useState(0);
  const [cancelling, setCancelling] = useState(false);
  const [maxRows, setMaxRows] = useState<number>(() => local.get("tabla.maxRows", 10000));
  useEffect(() => {
    if (!running) return;
    const start = Date.now();
    setElapsed(0);
    const t = setInterval(() => setElapsed(Date.now() - start), 100);
    let unlisten: (() => void) | undefined;
    // Progress events only exist inside the desktop app.
    if ("__TAURI_EVENT_PLUGIN_INTERNALS__" in window || (window as unknown as { __TAURI_INTERNALS__?: { transformCallback?: unknown } }).__TAURI_INTERNALS__?.transformCallback) {
      import("@tauri-apps/api/event")
        .then(({ listen }) =>
          listen<{ queryId: string; rows: number }>("query-progress", (e) => {
            if (e.payload.queryId === queryIdRef.current) setProgress(e.payload.rows);
          }),
        )
        .then((u) => (unlisten = u))
        .catch(() => {});
    }
    return () => {
      clearInterval(t);
      unlisten?.();
    };
  }, [running]);
  const cancel = async () => {
    const qid = queryIdRef.current;
    if (!qid) return;
    setCancelling(true);
    try {
      const stopped = await api.cancelQuery(session.id, qid);
      if (!stopped) toast(session.connection.driver === "sqlite" ? "SQLite queries run locally and can't be cancelled." : "The query had already finished.", "info");
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setCancelling(false);
    }
  };
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
            : statementAt(doc, sel.head, session.connection.driver);
      if (!text.trim()) return;
      lastSql.current = text;
      setRunning(true);
      setError(null);
      const started = performance.now();
      try {
        const qid = crypto.randomUUID();
        queryIdRef.current = qid;
        setProgress(0);
        const res = await api.runQuery(session.id, text, { queryId: qid, maxRows: maxRows });
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
      const d = session.connection.driver;
      const lang = d === "postgres" ? "postgresql" : d === "mysql" ? "mysql" : d === "mssql" ? "transactsql" : d === "clickhouse" ? "clickhouse" : "sqlite";
      const out = formatSql(view.state.doc.toString(), { language: lang, keywordCase: "upper", tabWidth: 2 });
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: out } });
    } catch (e) {
      toast(`Couldn't format: ${errorMessage(e)}`, "error");
    }
  }, [session.connection.driver, toast]);

  const runRef = useRef(run);
  runRef.current = run;
  const [snippetsOpen, setSnippetsOpen] = useState(false);
  const [naming, setNaming] = useState<string | null>(null);
  const saveQuery = useApp((s) => s.saveQuery);
  const saved = useApp((s) => s.saved[session.connection.id]);
  const saveCurrent = () => {
    const sql = cm.current?.view?.state.doc.toString() ?? tab.sql;
    const existing = tab.savedId && saved?.find((q) => q.id === tab.savedId);
    if (existing) {
      saveQuery(session.connection.id, { id: existing.id, name: existing.name, sql });
      toast(`Saved “${existing.name}”`, "success");
    } else setNaming(tab.title.startsWith("Query ") ? "" : tab.title);
  };
  useEffect(() => {
    if (!snippetsOpen) return;
    const close = () => setSnippetsOpen(false);
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [snippetsOpen]);
  const saveRef = useRef(saveCurrent);
  saveRef.current = saveCurrent;
  const insertAtCursor = (text: string) => {
    const view = cm.current?.view;
    if (!view) return;
    const { from, to } = view.state.selection.main;
    const prefix = from > 0 && view.state.sliceDoc(from - 1, from) !== "\n" ? "\n\n" : "";
    view.dispatch({ changes: { from, to, insert: prefix + text }, selection: { anchor: from + prefix.length + text.length } });
    view.focus();
  };
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
          { key: "Mod-s", run: () => (saveRef.current(), true) },
        ]),
      ),
    ],
    [session.connection.driver, schemaMap],
  );

  useEffect(() => {
    if (active) setTimeout(() => cm.current?.view?.focus(), 0);
  }, [active]);

  const result = outcome?.results[resultIdx];
  const statementCount = useMemo(() => splitStatements(tab.sql, session.connection.driver).length, [tab.sql, session.connection.driver]);
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

  const copyForAi = async () => {
    if (!error) return;
    const sqlText = lastSql.current || tab.sql;
    let schema = "";
    try {
      const names = tablesIn(sqlText, session);
      schema = names.length ? await schemaLines(session, names, 10) : "";
    } catch {
      /* schema is optional context */
    }
    await navigator.clipboard.writeText(aiFixPrompt({ engine: engineLabel(session), sql: sqlText, error, schema }));
    toast("Copied. Paste it into your AI assistant.", "success");
  };

  return (
    <div className="view" hidden={!active} ref={wrapRef}>
      <div className="toolbar">
        {running ? (
          <button className="btn danger solid" onClick={cancel} disabled={cancelling} title="Stop the query on the server">
            <Square /> {cancelling ? "Cancelling…" : "Cancel"} <span className="mono" style={{ opacity: 0.8 }}>{(elapsed / 1000).toFixed(1)}s</span>
          </button>
        ) : (
          <button className="btn primary" onClick={() => run("current")} title="Run statement at cursor or selection">
            <Play /> Run <Kbd keys="Mod ↵" />
          </button>
        )}
        <button className="btn" onClick={() => run("all")} disabled={running || statementCount === 0} title="Run every statement">
          <PlayCircle /> <span className="lbl">Run all{statementCount > 1 ? ` (${statementCount})` : ""}</span>
        </button>
        <div className="divider" />
        <button className="btn ghost" onClick={doFormat} title="Format SQL (Mod+Shift+F)">
          <Sparkles /> <span className="lbl">Format</span>
        </button>
        <div style={{ position: "relative" }}>
          <button className={`btn ghost ${snippetsOpen ? "on" : ""}`} onMouseDown={(e) => e.stopPropagation()} onClick={() => setSnippetsOpen(!snippetsOpen)} title="Insert a ready-made query">
            <BookMarked /> <span className="lbl">Snippets</span>
          </button>
          {snippetsOpen && (
            <div className="popover snippets" onMouseDown={(e) => e.stopPropagation()}>
              {snippetsFor(session.connection.driver).map((sn) => (
                <button
                  key={sn.name}
                  className="history-item"
                  onClick={() => {
                    insertAtCursor(sn.sql);
                    setSnippetsOpen(false);
                  }}
                >
                  <b className="sn-name">{sn.name}</b>
                  <small>{sn.hint}</small>
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="spacer" />
        <button className="btn ghost" onClick={saveCurrent} title={tab.savedId ? "Save changes to this saved query (Mod+S)" : "Save this query (Mod+S)"}>
          <Save /> <span className="lbl">{tab.savedId ? "Save" : "Save as…"}</span>
        </button>
        <button className={`btn ghost ${historyOpen ? "on" : ""}`} onClick={() => setHistoryOpen(!historyOpen)}>
          <History /> <span className="lbl">History</span>
        </button>
      </div>

      {naming !== null && (
        <Modal onClose={() => setNaming(null)}>
          <div className="modal-head">
            <Save color="var(--accent-2)" />
            <div>
              <h2>Save query</h2>
              <p>Saved queries appear in the sidebar for {session.connection.name}.</p>
            </div>
          </div>
          <form
            className="modal-body"
            onSubmit={(e) => {
              e.preventDefault();
              const item = saveQuery(session.connection.id, { name: naming, sql: cm.current?.view?.state.doc.toString() ?? tab.sql });
              updateTab(session.id, tab.id, { savedId: item.id, title: item.name });
              setNaming(null);
              toast(`Saved “${item.name}”`, "success");
            }}
          >
            <input className="input" autoFocus placeholder="e.g. Weekly signups" value={naming} onChange={(e) => setNaming(e.target.value)} />
            <button type="submit" hidden />
          </form>
          <div className="modal-foot">
            <div className="spacer" />
            <button className="btn" onClick={() => setNaming(null)}>
              Cancel
            </button>
            <button
              className="btn primary"
              disabled={!naming.trim()}
              onClick={() => {
                const item = saveQuery(session.connection.id, { name: naming, sql: cm.current?.view?.state.doc.toString() ?? tab.sql });
                updateTab(session.id, tab.id, { savedId: item.id, title: item.name });
                setNaming(null);
                toast(`Saved “${item.name}”`, "success");
              }}
            >
              <Save /> Save
            </button>
          </div>
        </Modal>
      )}

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
                <span className="spinner" /> Running… {progress > 0 && `${progress.toLocaleString()} rows so far`}
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
          <Select
            size="sm"
            className="w-limit"
            value={maxRows}
            title="Maximum rows to load per result"
            options={[1000, 10000, 50000, 200000].map((n) => ({ value: n, label: `${n.toLocaleString()} rows`, hint: "max" }))}
            onChange={(v) => {
              const n = Number(v);
              local.set("tabla.maxRows", n);
              setMaxRows(n);
            }}
          />
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
            <div style={{ flex: 1 }}>{error}</div>
            {error !== "Query cancelled" && (
              <button className="btn" style={{ fontFamily: "var(--font-ui)" }} onClick={copyForAi} title="Copy the query, error and related tables as a prompt">
                <Sparkles /> Copy for AI
              </button>
            )}
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
