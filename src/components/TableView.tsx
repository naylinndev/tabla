import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronLeft,
  ChevronRight,
  Copy,
  Download,
  Eye,
  Filter as FilterIcon,
  KeyRound,
  ListFilter,
  Lock,
  PanelRight,
  Plus,
  RefreshCw,
  RotateCcw,
  Rows3,
  Save,
  Table2,
  TableProperties,
  Trash2,
  Undo2,
  Info as InfoIcon,
  GitFork,
  Zap,
  FileUp,
  Columns3,
  Search,
  CopyPlus,
  X,
  Ban,
  Code2,
  ShieldAlert,
} from "lucide-react";
import { api, errorMessage, type RowEdit, type TablePage, type TableStructure } from "../lib/api";
import { useApp, local, type Session, type TableTab } from "../lib/store";
import { buildFilter, FILTER_OPS, opTakesValue, toCSV, toJSON, type Filter, type FilterOp } from "../lib/sql";
import { DataGrid, type Cursor, type GridColumn } from "./DataGrid";
import { StructureEditor } from "./StructureEditor";
import { ImportCsv } from "./ImportCsv";
import { TableInfoView } from "./TableInfoView";
import { RelationsView, TriggersView, useRelations } from "./RelationsViews";
import { Checkbox, Select } from "./controls";
import { ContextMenu, Kbd, Modal, type MenuItem } from "./ui";

type Edits = Record<number, Record<string, string | null>>;
type NewRow = Record<string, string | null>;

const PAGE_SIZES = [100, 200, 500, 1000];

export function TableView({ session, tab, active }: { session: Session; tab: TableTab; active: boolean }) {
  const toast = useApp((s) => s.toast);
  const updateTab = useApp((s) => s.updateTab);
  const setColumns = useApp((s) => s.setColumns);
  const newQuery = useApp((s) => s.newQuery);
  const conn = session.connection;
  const tableInfo = session.tables.find((t) => t.name === tab.table);
  const isView = tableInfo ? tableInfo.kind !== "table" : false;

  const [structure, setStructure] = useState<TableStructure | null>(null);
  const [structureRev, setStructureRev] = useState(0);
  const orderKey = `tabla.colorder.${conn.id}.${tab.schema}.${tab.table}`;
  const [columnOrder, setColumnOrder] = useState<string[]>(() => local.get(orderKey, []));
  const hideKey = `tabla.colhide.${conn.id}.${tab.schema}.${tab.table}`;
  const [hidden, setHidden] = useState<string[]>(() => local.get(hideKey, []));
  const saveHidden = (names: string[]) => {
    local.set(hideKey, names);
    setHidden(names);
  };
  const [colsOpen, setColsOpen] = useState(false);
  const [importing, setImporting] = useState(false);
  const saveOrder = (names: string[]) => {
    local.set(orderKey, names);
    setColumnOrder(names);
  };
  const [page, setPage] = useState<TablePage | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [limit, setLimit] = useState(() => local.get("tabla.pageSize", 200));
  const [offset, setOffset] = useState(0);
  const [sort, setSort] = useState<{ column: string; desc: boolean } | null>(null);

  const init = tab.initialFilter;
  const [filterOpen, setFilterOpen] = useState(!!init);
  const [filters, setFilters] = useState<Filter[]>(() =>
    init ? [{ id: crypto.randomUUID(), column: init.column, op: init.value === null ? "is null" : "=", value: init.value ?? "", enabled: true }] : [],
  );
  const [rawMode, setRawMode] = useState(false);
  const [rawFilter, setRawFilter] = useState("");
  const [join, setJoin] = useState<"AND" | "OR">("AND");
  const [applied, setApplied] = useState(() =>
    init ? buildFilter(conn.driver, [{ id: "x", column: init.column, op: init.value === null ? "is null" : "=", value: init.value ?? "", enabled: true }]) : "",
  );

  const [edits, setEdits] = useState<Edits>({});
  const [deleted, setDeleted] = useState<Set<number>>(new Set());
  const [inserted, setInserted] = useState<NewRow[]>([]);
  const [saving, setSaving] = useState(false);
  const [confirmProd, setConfirmProd] = useState(false);

  const [cursor, setCursor] = useState<Cursor | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [inspector, setInspector] = useState(() => local.get("tabla.inspector", false));
  const [menu, setMenu] = useState<{ x: number; y: number; row: number; col: number } | null>(null);

  const rows = page?.result.rows ?? [];
  const { rel } = useRelations(session, tab.schema, tab.table);
  const fkByColumn = useMemo(() => {
    const m = new Map<string, { schema: string; table: string; column: string }>();
    for (const f of rel?.outgoing ?? []) if (f.columns.length === 1) m.set(f.columns[0], { schema: f.refSchema || tab.schema, table: f.refTable, column: f.refColumns[0] });
    return m;
  }, [rel, tab.schema]);
  const pkCols = useMemo(() => structure?.columns.filter((c) => c.primaryKey).map((c) => c.name) ?? [], [structure]);
  const editable = !conn.readOnly && !isView && pkCols.length > 0;
  const editBlockReason = conn.readOnly
    ? "Read-only connection"
    : isView
      ? "Views can't be edited"
      : structure && !pkCols.length
        ? "Table has no primary key"
        : null;

  const columns: GridColumn[] = useMemo(() => {
    const src = page?.result.columns ?? [];
    const types = new Map(structure?.columns.map((c) => [c.name, c.dataType]));
    return src.map((c) => ({ name: c.name, typeName: types.get(c.name) ?? c.typeName, pk: pkCols.includes(c.name) }));
  }, [page, structure, pkCols]);

  const changeCount =
    Object.values(edits).reduce((n, r) => n + Object.keys(r).length, 0) + deleted.size + inserted.length;
  const setPending = useApp((s) => s.setPending);
  useEffect(() => {
    setPending(session.id, tab.id, changeCount);
    return () => setPending(session.id, tab.id, 0);
  }, [session.id, tab.id, changeCount, setPending]);

  const load = useCallback(
    async (opts?: { offset?: number }) => {
      setLoading(true);
      setError(null);
      try {
        const res = await api.fetchRows(session.id, {
          schema: tab.schema,
          table: tab.table,
          filter: applied || null,
          sort,
          limit,
          offset: opts?.offset ?? offset,
        });
        setPage(res);
        setEdits({});
        setDeleted(new Set());
        setInserted([]);
        setSelected(new Set());
        setCursor((c) => (c && c.row < res.result.rows.length ? c : null));
      } catch (e) {
        setError(errorMessage(e));
      } finally {
        setLoading(false);
      }
    },
    [session.id, tab.schema, tab.table, applied, sort, limit, offset],
  );

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [applied, sort, limit, offset]);

  useEffect(() => {
    api
      .tableStructure(session.id, tab.schema, tab.table)
      .then((s) => {
        setStructure(s);
        setColumns(session.id, tab.table, s.columns.map((c) => c.name));
      })
      .catch((e) => toast(errorMessage(e), "error"));
  }, [session.id, tab.schema, tab.table, setColumns, toast, structureRev]);

  // ── cell access ─────────────────────────────────────────────
  const colName = (c: number) => columns[c]?.name;
  const getCell = useCallback(
    (r: number, c: number): string | null | undefined => {
      const name = columns[c]?.name;
      if (r >= rows.length) {
        const nr = inserted[r - rows.length];
        return nr && name in nr ? nr[name] : undefined;
      }
      const e = edits[r];
      if (e && name in e) return e[name];
      return rows[r]?.[c] ?? null;
    },
    [columns, rows, edits, inserted],
  );
  const rowState = useCallback(
    (r: number) => (deleted.has(r) ? "deleted" : r >= rows.length ? "inserted" : null),
    [deleted, rows.length],
  );
  const isEdited = useCallback(
    (r: number, c: number) => r < rows.length && !!edits[r] && columns[c]?.name in edits[r],
    [edits, rows.length, columns],
  );

  const onEdit = useCallback(
    (r: number, c: number, v: string | null) => {
      const name = columns[c].name;
      if (r >= rows.length) {
        setInserted((ins) => ins.map((row, i) => (i === r - rows.length ? { ...row, [name]: v } : row)));
        return;
      }
      setEdits((prev) => {
        const row = { ...(prev[r] ?? {}) };
        if (rows[r][c] === v) delete row[name];
        else row[name] = v;
        const next = { ...prev };
        if (Object.keys(row).length) next[r] = row;
        else delete next[r];
        return next;
      });
    },
    [columns, rows],
  );

  const deleteRows = useCallback(
    (rs: number[]) => {
      if (!editable) return;
      const newIdx = rs.filter((r) => r >= rows.length).map((r) => r - rows.length);
      if (newIdx.length) setInserted((ins) => ins.filter((_, i) => !newIdx.includes(i)));
      setDeleted((d) => {
        const next = new Set(d);
        rs.filter((r) => r < rows.length).forEach((r) => (next.has(r) ? next.delete(r) : next.add(r)));
        return next;
      });
      setSelected(new Set());
    },
    [editable, rows.length],
  );

  const addRow = () => {
    setInserted((ins) => [...ins, {}]);
    const r = rows.length + inserted.length;
    setCursor({ row: r, col: 0 });
  };

  /** Copies rows as new pending inserts, leaving keys and auto-generated columns to the database. */
  const duplicateRows = (rs: number[]) => {
    if (!editable || !rs.length) return;
    const auto = new Set(
      structure?.columns.filter((c) => c.primaryKey || /auto|identity|generated/i.test(c.extra) || /nextval\(/i.test(c.defaultValue ?? "")).map((c) => c.name),
    );
    const copies = rs.map((r) => {
      const nr: Record<string, string | null> = {};
      columns.forEach((c, i) => {
        if (!auto.has(c.name)) nr[c.name] = getCell(r, i) ?? null;
      });
      return nr;
    });
    setInserted((ins) => [...ins, ...copies]);
    setSelected(new Set());
    toast(`Duplicated ${copies.length} row${copies.length > 1 ? "s" : ""}. Commit to save.`, "info");
  };

  const discard = () => {
    setEdits({});
    setDeleted(new Set());
    setInserted([]);
  };

  const keyOf = (r: number) =>
    pkCols.map((name) => ({ column: name, value: rows[r][columns.findIndex((c) => c.name === name)] }));

  const commit = async (confirmed = false) => {
    if (!changeCount || saving) return;
    if (conn.tag === "production" && !confirmed) return setConfirmProd(true);
    const list: RowEdit[] = [];
    deleted.forEach((r) => list.push({ kind: "delete", key: keyOf(r) }));
    Object.entries(edits).forEach(([r, vals]) => {
      if (deleted.has(+r)) return;
      list.push({
        kind: "update",
        key: keyOf(+r),
        values: Object.entries(vals).map(([column, value]) => ({ column, value })),
      });
    });
    inserted.forEach((nr) =>
      list.push({ kind: "insert", values: Object.entries(nr).map(([column, value]) => ({ column, value })) }),
    );
    setSaving(true);
    try {
      await api.applyEdits(session.id, tab.schema, tab.table, list);
      toast(`Saved ${changeCount} change${changeCount > 1 ? "s" : ""} to ${tab.table}`, "success");
      await load();
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setSaving(false);
    }
  };

  // ── filters ─────────────────────────────────────────────────
  const addFilter = (column?: string, op: FilterOp = "=", value = "") => {
    setFilterOpen(true);
    setRawMode(false);
    setFilters((f) => [
      ...f,
      { id: crypto.randomUUID(), column: column ?? columns[0]?.name ?? "", op, value, enabled: true },
    ]);
  };
  const applyFilters = (list = filters) => {
    const sql = rawMode ? rawFilter.trim() : buildFilter(conn.driver, list, join);
    setOffset(0);
    setApplied(sql);
  };
  const clearFilters = () => {
    setFilters([]);
    setRawFilter("");
    setApplied("");
    setOffset(0);
    setFilterOpen(false);
  };

  // ── keyboard ────────────────────────────────────────────────
  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      const m = e.metaKey || e.ctrlKey;
      if (!m) return;
      const k = e.key.toLowerCase();
      if (k === "s") {
        e.preventDefault();
        commit();
      } else if (k === "r") {
        e.preventDefault();
        if (!changeCount) load();
      } else if (k === "f") {
        e.preventDefault();
        if (!filterOpen) addFilter();
        else setFilterOpen(false);
      } else if (k === "i") {
        e.preventDefault();
        toggleInspector();
      } else if (k === "d" && tab.view === "data") {
        e.preventDefault();
        duplicateRows(selected.size ? [...selected] : cursor ? [cursor.row] : []);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const toggleInspector = () =>
    setInspector((v: boolean) => {
      local.set("tabla.inspector", !v);
      return !v;
    });

  const exportAs = async (fmt: "csv" | "json") => {
    const cols = page?.result.columns ?? [];
    const contents = fmt === "csv" ? toCSV(cols, rows) : toJSON(cols, rows);
    try {
      const path = await api.exportFile(`${tab.table}.${fmt}`, contents);
      if (path) toast(`Exported ${rows.length.toLocaleString()} rows`, "success");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  const menuItems = (r: number, c: number): (MenuItem | "sep")[] => {
    const v = getCell(r, c);
    const name = colName(c);
    const rowObj = () => Object.fromEntries(columns.map((col, i) => [col.name, getCell(r, i) ?? null]));
    const items: (MenuItem | "sep")[] = [
      { label: "Copy value", icon: <Copy />, kbd: "Mod C", onClick: () => navigator.clipboard.writeText(v ?? "NULL") },
      {
        label: "Copy row as JSON",
        icon: <Code2 />,
        onClick: () => navigator.clipboard.writeText(JSON.stringify(rowObj(), null, 2)),
      },
      { label: "Copy column name", icon: <Copy />, onClick: () => navigator.clipboard.writeText(name) },
      "sep",
      {
        label: v === null ? `Filter: ${name} IS NULL` : `Filter by this value`,
        icon: <FilterIcon />,
        onClick: () => {
          const f: Filter = {
            id: crypto.randomUUID(),
            column: name,
            op: v === null ? "is null" : "=",
            value: v ?? "",
            enabled: true,
          };
          const list = [...filters, f];
          setFilters(list);
          setFilterOpen(true);
          setRawMode(false);
          applyFilters(list);
        },
      },
    ];
    if (editable) {
      items.push(
        "sep",
        { label: "Set NULL", icon: <Ban />, onClick: () => onEdit(r, c, null) },
      );
      if (edits[r])
        items.push({
          label: "Revert row",
          icon: <Undo2 />,
          onClick: () =>
            setEdits((e) => {
              const n = { ...e };
              delete n[r];
              return n;
            }),
        });
      items.push({
        label: selected.size > 1 ? `Duplicate ${selected.size} rows` : "Duplicate row",
        icon: <CopyPlus />,
        kbd: "Mod D",
        onClick: () => duplicateRows(selected.size ? [...selected] : [r]),
      });
      items.push({
        label: deleted.has(r) ? "Restore row" : "Delete row",
        icon: deleted.has(r) ? <RotateCcw /> : <Trash2 />,
        danger: !deleted.has(r),
        kbd: "Delete",
        onClick: () => deleteRows(selected.size ? [...selected] : [r]),
      });
    }
    return items;
  };

  const total = page?.total ?? null;
  const from = rows.length ? offset + 1 : 0;
  const to = offset + rows.length;
  const busy = changeCount > 0;

  return (
    <div className="view" hidden={!active}>
      <div className="toolbar">
        <div className="crumb">
          {isView ? <Eye /> : <Table2 />}
          <span className="schema">{tab.schema}</span>
          <span className="dim sep">/</span>
          <span>{tab.table}</span>
        </div>
        <div className="segmented view-switch" style={{ marginLeft: 8 }}>
          {(
            [
              ["data", "Content", <Rows3 key="d" />],
              ["structure", "Structure", <TableProperties key="s" />],
              ["relations", "Relations", <GitFork key="r" />],
              ["triggers", "Triggers", <Zap key="t" />],
              ["info", "Info", <InfoIcon key="i" />],
            ] as const
          )
            // ClickHouse has no foreign keys or triggers.
            .filter(([id]) => session.connection.driver !== "clickhouse" || (id !== "relations" && id !== "triggers"))
            .map(([id, label, icon]) => (
            <button key={id} className={tab.view === id ? "on" : ""} onClick={() => updateTab(session.id, tab.id, { view: id })} title={label}>
              {icon} <span className="lbl">{label}</span>
            </button>
          ))}
        </div>
        <div className="spacer" />
        {tab.view === "data" && (
          <>
            <button
              className={`btn ghost ${filterOpen || applied ? "on" : ""}`}
              onClick={() => (filterOpen ? setFilterOpen(false) : filters.length ? setFilterOpen(true) : addFilter())}
              disabled={busy}
              title="Filter rows (Mod+F)"
            >
              <ListFilter /> <span className="lbl">Filter</span>
              {applied && <span className="badge development">on</span>}
            </button>
            {editable && (
              <button className="btn ghost" onClick={addRow} title="Insert a new row">
                <Plus /> <span className="lbl">Row</span>
              </button>
            )}
            {editable && (
              <button className="icon-btn" onClick={() => setImporting(true)} title="Import CSV into this table" disabled={busy}>
                <FileUp />
              </button>
            )}
            <div style={{ position: "relative" }}>
              <button className={`icon-btn ${hidden.length ? "on" : ""}`} onClick={() => setColsOpen(!colsOpen)} title="Show or hide columns">
                <Columns3 />
              </button>
              {colsOpen && (
                <ColumnPicker
                  columns={columns.map((c) => c.name)}
                  hidden={hidden}
                  onChange={saveHidden}
                  onClose={() => setColsOpen(false)}
                />
              )}
            </div>
            <button className="icon-btn" onClick={() => exportAs("csv")} title="Export page as CSV" disabled={!rows.length}>
              <Download />
            </button>
            <div className="divider" />
            <div className="pager">
              <button
                className="icon-btn"
                disabled={offset === 0 || busy}
                onClick={() => setOffset(Math.max(0, offset - limit))}
                title="Previous page"
              >
                <ChevronLeft />
              </button>
              <span>
                {from.toLocaleString()}–{to.toLocaleString()}
                {total !== null && <span className="dim"> of {total.toLocaleString()}</span>}
              </span>
              <button
                className="icon-btn"
                disabled={busy || (total !== null ? to >= total : rows.length < limit)}
                onClick={() => setOffset(offset + limit)}
                title="Next page"
              >
                <ChevronRight />
              </button>
              <Select
                size="sm"
                className="w-pagesize"
                value={limit}
                disabled={busy}
                title="Rows per page"
                options={PAGE_SIZES.map((n) => ({ value: n, label: String(n), hint: "rows" }))}
                onChange={(v) => {
                  local.set("tabla.pageSize", v);
                  setOffset(0);
                  setLimit(v);
                }}
              />
            </div>
            <button className="icon-btn" onClick={() => load()} disabled={busy} title="Refresh (Mod+R)">
              <RefreshCw />
            </button>
            <button className={`icon-btn ${inspector ? "on" : ""}`} onClick={toggleInspector} title="Row inspector (Mod+I)">
              <PanelRight />
            </button>
          </>
        )}
      </div>

      {tab.view === "data" && filterOpen && (
        <div className="filters">
          {rawMode ? (
            <div className="filter-row">
              <span className="mono dim">WHERE</span>
              <input
                className="input sm mono raw"
                autoFocus
                placeholder="status = 'active' AND created_at > now() - interval '7 days'"
                value={rawFilter}
                onChange={(e) => setRawFilter(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && applyFilters()}
              />
            </div>
          ) : (
            filters.map((f, i) => (
              <div className="filter-row" key={f.id}>
                <Checkbox
                  checked={f.enabled}
                  onChange={(v) => setFilters(filters.map((x) => (x.id === f.id ? { ...x, enabled: v } : x)))}
                />
                <span className="dim filter-join">{i === 0 ? "Where" : join.toLowerCase()}</span>
                <Select
                  size="sm"
                  mono
                  className="w-col"
                  value={f.column}
                  options={columns.map((c) => ({ value: c.name, label: c.name, hint: c.typeName.toLowerCase() }))}
                  onChange={(v) => setFilters(filters.map((x) => (x.id === f.id ? { ...x, column: v } : x)))}
                />
                <Select
                  size="sm"
                  className="w-op"
                  value={f.op}
                  options={FILTER_OPS.map((op) => ({ value: op, label: op }))}
                  onChange={(v) => setFilters(filters.map((x) => (x.id === f.id ? { ...x, op: v } : x)))}
                />
                {opTakesValue(f.op) ? (
                  <input
                    className="input sm mono"
                    autoFocus={i === filters.length - 1}
                    placeholder={f.op === "in" ? "a, b, c" : "value"}
                    value={f.value}
                    onChange={(e) => setFilters(filters.map((x) => (x.id === f.id ? { ...x, value: e.target.value } : x)))}
                    onKeyDown={(e) => e.key === "Enter" && applyFilters()}
                  />
                ) : (
                  <div className="spacer" />
                )}
                <button
                  className="icon-btn"
                  title="Remove filter"
                  onClick={() => {
                    const list = filters.filter((x) => x.id !== f.id);
                    setFilters(list);
                    if (!list.length) clearFilters();
                    else applyFilters(list);
                  }}
                >
                  <X />
                </button>
              </div>
            ))
          )}
          <div className="filter-row">
            {!rawMode && (
              <button className="btn ghost" onClick={() => addFilter()}>
                <Plus /> Add condition
              </button>
            )}
            <button
              className="btn ghost"
              onClick={() => {
                if (!rawMode && filters.length) setRawFilter(buildFilter(conn.driver, filters, join));
                setRawMode(!rawMode);
              }}
            >
              <Code2 /> {rawMode ? "Builder" : "Raw SQL"}
            </button>
            {!rawMode && (
              <code className="where-preview" title="The WHERE clause Tabla will run">
                {filters.some((f) => f.enabled) ? `WHERE ${buildFilter(conn.driver, filters, join)}` : "No active conditions"}
              </code>
            )}
            <div className="spacer" />
            {!rawMode && filters.length > 1 && (
              <Select
                size="sm"
                className="w-join"
                value={join}
                options={[
                  { value: "AND", label: "AND", hint: "all conditions" },
                  { value: "OR", label: "OR", hint: "any condition" },
                ]}
                onChange={(v) => setJoin(v)}
              />
            )}
            <button className="btn ghost" onClick={clearFilters}>
              Clear
            </button>
            <button className="btn primary" onClick={() => applyFilters()}>
              Apply <Kbd keys="↵" />
            </button>
          </div>
        </div>
      )}

      {tab.view === "info" ? (
        <TableInfoView session={session} schema={tab.schema} table={tab.table} />
      ) : tab.view === "relations" ? (
        <RelationsView session={session} schema={tab.schema} table={tab.table} editable={!conn.readOnly && !isView} />
      ) : tab.view === "triggers" ? (
        <TriggersView session={session} schema={tab.schema} table={tab.table} editable={!conn.readOnly && !isView} />
      ) : tab.view === "structure" ? (
        <StructureEditor
          session={session}
          schema={tab.schema}
          table={tab.table}
          structure={structure}
          editable={!conn.readOnly && !isView}
          viewOrder={columnOrder}
          onViewOrder={saveOrder}
          onApplied={() => {
            setStructureRev((n) => n + 1);
            load();
          }}
          onQuery={(sql) => newQuery(session.id, sql)}
          onRefresh={() => setStructureRev((n) => n + 1)}
        />
      ) : (
        <div className="split-h">
          <div className="grid-wrap">
            {error ? (
              <div className="q-error">
                <ShieldAlert />
                {error}
              </div>
            ) : (
              <DataGrid
                columns={columns}
                rowCount={rows.length + inserted.length}
                getCell={getCell}
                rowState={rowState}
                isEdited={isEdited}
                editable={editable}
                onEdit={onEdit}
                sort={sort}
                onSort={(col) => {
                  if (busy) return;
                  setOffset(0);
                  setSort((s) => (s?.column !== col ? { column: col, desc: false } : !s.desc ? { column: col, desc: true } : null));
                }}
                rowNumberOffset={offset}
                cursor={cursor}
                onCursor={setCursor}
                selectedRows={selected}
                onSelectRows={setSelected}
                onContextMenu={(e, row, col) => setMenu({ x: e.clientX, y: e.clientY, row, col })}
                onDeleteRows={editable ? deleteRows : undefined}
                loading={loading}
                emptyText={applied ? "No rows match this filter" : "This table is empty"}
                columnOrder={columnOrder}
                hiddenColumns={hidden}
                onReorder={saveOrder}
                linkColumns={fkByColumn.size ? new Set(columns.map((c, i) => (fkByColumn.has(c.name) ? i : -1)).filter((i) => i >= 0)) : undefined}
                onFollow={(r, c) => {
                  const fk = fkByColumn.get(columns[c].name);
                  const v = getCell(r, c);
                  if (fk && v != null) useApp.getState().openTable(session.id, fk.schema, fk.table, "data", true, { column: fk.column, value: v });
                }}
                onCopied={(w) => toast(`Copied ${w}`, "success")}
              />
            )}
            {changeCount > 0 && (
              <div className="commit-bar">
                <span className="dot" />
                <span>
                  {changeCount} pending change{changeCount > 1 ? "s" : ""}
                </span>
                <button className="btn ghost" onClick={discard} disabled={saving}>
                  Discard
                </button>
                <button className="btn primary" onClick={() => commit()} disabled={saving}>
                  {saving ? <span className="spinner" /> : <Save />}
                  Commit <Kbd keys="Mod S" />
                </button>
              </div>
            )}
          </div>
          {inspector && (
            <Inspector
              columns={columns}
              row={cursor?.row ?? null}
              getCell={getCell}
              isEdited={isEdited}
              editable={editable && cursor !== null && rowState(cursor.row) !== "deleted"}
              onEdit={onEdit}
              onClose={toggleInspector}
            />
          )}
        </div>
      )}

      <div className="statusbar">
        <span className="item">
          <span className="live" />
          {loading ? "Loading…" : `${(total ?? rows.length).toLocaleString()} rows`}
        </span>
        {page && (
          <span className="item mono" style={{ overflow: "hidden", textOverflow: "ellipsis", minWidth: 0 }} title={page.sql}>
            {page.sql}
          </span>
        )}
        <div className="spacer" />
        {cursor && columns[cursor.col] && (
          <span className="item">
            Row {offset + cursor.row + 1} · {columns[cursor.col].name}
          </span>
        )}
        {editBlockReason && (
          <span className="item" title={editBlockReason}>
            <Lock /> {editBlockReason}
          </span>
        )}
      </div>

      {importing && structure && (
        <ImportCsv session={session} schema={tab.schema} table={tab.table} columns={structure.columns} onClose={() => setImporting(false)} onDone={() => load()} />
      )}
      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.row, menu.col)} onClose={() => setMenu(null)} />}

      {confirmProd && (
        <Modal onClose={() => setConfirmProd(false)}>
          <div className="modal-head">
            <ShieldAlert color="var(--danger)" />
            <div>
              <h2>Write to production?</h2>
              <p>
                {conn.name} is tagged as production. {changeCount} change{changeCount > 1 ? "s" : ""} will be applied to{" "}
                <b>{tab.table}</b>.
              </p>
            </div>
          </div>
          <div className="modal-foot">
            <div className="spacer" />
            <button className="btn" onClick={() => setConfirmProd(false)}>
              Cancel
            </button>
            <button
              className="btn danger solid"
              autoFocus
              onClick={() => {
                setConfirmProd(false);
                commit(true);
              }}
            >
              Commit to production
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function Inspector({
  columns,
  row,
  getCell,
  isEdited,
  editable,
  onEdit,
  onClose,
}: {
  columns: GridColumn[];
  row: number | null;
  getCell(r: number, c: number): string | null | undefined;
  isEdited(r: number, c: number): boolean;
  editable: boolean;
  onEdit(r: number, c: number, v: string | null): void;
  onClose(): void;
}) {
  return (
    <aside className="inspector">
      <div className="insp-head">
        <PanelRight size={14} />
        {row === null ? "Row inspector" : `Row ${row + 1}`}
        <div className="spacer" />
        <button className="icon-btn" onClick={onClose} title="Close">
          <X />
        </button>
      </div>
      <div className="insp-body">
        {row === null ? (
          <div className="sb-empty">Select a cell to inspect its row.</div>
        ) : (
          columns.map((c, ci) => (
            <InspectorField
              key={`${row}-${c.name}`}
              column={c}
              value={getCell(row, ci)}
              edited={isEdited(row, ci)}
              editable={editable}
              onCommit={(v) => onEdit(row, ci, v)}
            />
          ))
        )}
      </div>
    </aside>
  );
}

function prettyJson(v: string) {
  try {
    return JSON.stringify(JSON.parse(v), null, 2);
  } catch {
    return v;
  }
}

function InspectorField({
  column,
  value,
  edited,
  editable,
  onCommit,
}: {
  column: GridColumn;
  value: string | null | undefined;
  edited: boolean;
  editable: boolean;
  onCommit(v: string | null): void;
}) {
  const isJson = /json/i.test(column.typeName) && value;
  const display = value == null ? "" : isJson ? prettyJson(value) : value;
  const [draft, setDraft] = useState(display);
  useEffect(() => setDraft(display), [display]);
  const lines = Math.min(12, Math.max(1, draft.split("\n").length));
  return (
    <div className={`insp-field ${edited ? "edited" : ""}`}>
      <label>
        {column.pk && <KeyRound size={11} color="var(--warn)" />}
        {column.name}
        <span className="ty">{column.typeName.toLowerCase()}</span>
        <span className="acts">
          <button
            className="icon-btn"
            style={{ width: 20, height: 20 }}
            title="Copy"
            onClick={() => navigator.clipboard.writeText(value ?? "NULL")}
          >
            <Copy size={12} />
          </button>
          {editable && value !== null && (
            <button className="icon-btn" style={{ width: 20, height: 20 }} title="Set NULL" onClick={() => onCommit(null)}>
              <Ban size={12} />
            </button>
          )}
        </span>
      </label>
      <textarea
        rows={lines}
        className={value === null ? "is-null" : ""}
        placeholder={value === null ? "NULL" : value === undefined ? "DEFAULT" : ""}
        value={draft}
        readOnly={!editable}
        spellCheck={false}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => {
          if (draft === display) return;
          // Store compact JSON back if it was pretty-printed and is still valid.
          let out = draft;
          if (isJson) {
            try {
              out = JSON.stringify(JSON.parse(draft));
            } catch {
              /* keep as typed */
            }
          }
          onCommit(out);
        }}
      />
    </div>
  );
}

function ColumnPicker({ columns, hidden, onChange, onClose }: { columns: string[]; hidden: string[]; onChange(h: string[]): void; onClose(): void }) {
  const [q, setQ] = useState("");
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const down = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && onClose();
    window.addEventListener("mousedown", down);
    return () => window.removeEventListener("mousedown", down);
  }, [onClose]);
  const shown = columns.filter((c) => c.toLowerCase().includes(q.toLowerCase()));
  return (
    <div ref={ref} className="popover col-picker">
      <div className="sb-search" style={{ margin: 8 }}>
        <Search />
        <input className="input" autoFocus placeholder="Filter columns" value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} />
      </div>
      <div className="col-list">
        {shown.map((c) => (
          <div key={c} className="col-item">
            <Checkbox checked={!hidden.includes(c)} onChange={(v) => onChange(v ? hidden.filter((h) => h !== c) : [...hidden, c])} label={<span className="mono">{c}</span>} />
          </div>
        ))}
      </div>
      <div className="col-foot">
        <button className="btn ghost" onClick={() => onChange([])}>
          Show all
        </button>
        <span className="dim">
          {columns.length - hidden.length} of {columns.length}
        </span>
      </div>
    </div>
  );
}
