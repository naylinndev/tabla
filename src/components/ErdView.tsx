import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Download, KeyRound, LayoutGrid, Link2, Maximize, Minus, Plus, RefreshCw, Search } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { api, errorMessage } from "../lib/api";
import { useApp, local, type Session } from "../lib/store";
import { Checkbox } from "./controls";

interface GraphColumn {
  name: string;
  dataType: string;
  primaryKey: boolean;
}
interface GraphTable {
  name: string;
  kind: string;
  columns: GraphColumn[];
}
interface Fk {
  name: string;
  table: string;
  columns: string[];
  refTable: string;
  refColumns: string[];
}
interface Graph {
  tables: GraphTable[];
  foreignKeys: Fk[];
}
type Pos = Record<string, { x: number; y: number }>;

const BOX_W = 240;
const HEAD_H = 34;
const ROW_H = 22;
const MAX_ROWS = 16;
const GAP_X = 90;
const GAP_Y = 28;
const MAX_COL_H = 1500;

function shownColumns(t: GraphTable, keysOnly: boolean, fkCols: Set<string>) {
  const cols = keysOnly ? t.columns.filter((c) => c.primaryKey || fkCols.has(`${t.name}.${c.name}`)) : t.columns;
  return { cols: cols.slice(0, MAX_ROWS), more: cols.length - Math.min(cols.length, MAX_ROWS) };
}

function boxHeight(rows: number, more: number) {
  return HEAD_H + rows * ROW_H + (more ? ROW_H : 0) + 6;
}

/** Layered layout: referenced tables on the left, tables that point at them to the right. */
function autoLayout(g: Graph, height: (t: GraphTable) => number): Pos {
  const parents = new Map<string, Set<string>>();
  for (const f of g.foreignKeys) if (f.refTable !== f.table) (parents.get(f.table) ?? parents.set(f.table, new Set()).get(f.table)!).add(f.refTable);
  const level = new Map<string, number>(g.tables.map((t) => [t.name, 0]));
  for (let pass = 0; pass < g.tables.length; pass++) {
    let changed = false;
    for (const t of g.tables) {
      const ps = [...(parents.get(t.name) ?? [])].filter((p) => level.has(p));
      const l = ps.length ? Math.max(...ps.map((p) => level.get(p)!)) + 1 : 0;
      if (l !== level.get(t.name) && l < 30) {
        level.set(t.name, l);
        changed = true;
      }
    }
    if (!changed) break;
  }
  // Unconnected tables go in their own trailing columns so the graph stays readable.
  const connected = new Set(g.foreignKeys.flatMap((f) => [f.table, f.refTable]));
  const maxLevel = Math.max(0, ...[...level.entries()].filter(([t]) => connected.has(t)).map(([, l]) => l));
  const groups: string[][] = [];
  for (const t of [...g.tables].sort((a, b) => a.name.localeCompare(b.name))) {
    const l = connected.has(t.name) ? level.get(t.name)! : maxLevel + 1;
    (groups[l] ??= []).push(t.name);
  }
  const pos: Pos = {};
  let x = 40;
  const byName = new Map(g.tables.map((t) => [t.name, t]));
  for (const group of groups) {
    if (!group) continue;
    let y = 40;
    for (const name of group) {
      const h = height(byName.get(name)!);
      if (y + h > MAX_COL_H && y > 40) {
        x += BOX_W + GAP_X;
        y = 40;
      }
      pos[name] = { x, y };
      y += h + GAP_Y;
    }
    x += BOX_W + GAP_X;
  }
  return pos;
}

export function ErdView({ session, schema, active }: { session: Session; schema: string; active: boolean }) {
  const toast = useApp((s) => s.toast);
  const openTable = useApp((s) => s.openTable);
  const [graph, setGraph] = useState<Graph | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [rev, setRev] = useState(0);
  const posKey = `tabla.erd.${session.connection.id}.${schema}`;
  const [pos, setPos] = useState<Pos>(() => local.get(posKey, {}));
  const [view, setView] = useState({ x: 0, y: 0, z: 1 });
  const [keysOnly, setKeysOnly] = useState(false);
  const [hover, setHover] = useState<string | null>(null);
  const [q, setQ] = useState("");
  const wrap = useRef<HTMLDivElement>(null);
  const drag = useRef<{ kind: "pan" | "box"; name?: string; sx: number; sy: number; ox: number; oy: number } | null>(null);

  useEffect(() => {
    if (!active) return;
    setError(null);
    invoke<Graph>("schema_graph", { sessionId: session.id, schema }).then(setGraph, (e) => setError(errorMessage(e)));
  }, [session.id, schema, rev, active]);

  const fkCols = useMemo(() => new Set((graph?.foreignKeys ?? []).flatMap((f) => [...f.columns.map((c) => `${f.table}.${c}`), ...f.refColumns.map((c) => `${f.refTable}.${c}`)])), [graph]);
  const heightOf = useCallback(
    (t: GraphTable) => {
      const { cols, more } = shownColumns(t, keysOnly, fkCols);
      return boxHeight(cols.length, more);
    },
    [keysOnly, fkCols],
  );

  // Lay out tables that don't have a saved position yet.
  useEffect(() => {
    if (!graph) return;
    const missing = graph.tables.some((t) => !pos[t.name]);
    if (missing) {
      const auto = autoLayout(graph, heightOf);
      setPos((p) => ({ ...auto, ...p }));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [graph]);
  useEffect(() => {
    if (Object.keys(pos).length) local.set(posKey, pos);
  }, [pos, posKey]);

  const fit = useCallback(() => {
    if (!graph || !wrap.current) return;
    const xs = graph.tables.map((t) => pos[t.name]).filter(Boolean);
    if (!xs.length) return;
    const minX = Math.min(...xs.map((p) => p.x));
    const minY = Math.min(...xs.map((p) => p.y));
    const maxX = Math.max(...xs.map((p) => p.x)) + BOX_W;
    const maxY = Math.max(...graph.tables.map((t) => (pos[t.name]?.y ?? 0) + heightOf(t)));
    const { clientWidth: w, clientHeight: h } = wrap.current;
    const z = Math.min(1.2, Math.max(0.2, Math.min((w - 60) / (maxX - minX), (h - 60) / (maxY - minY))));
    setView({ z, x: (w - (maxX - minX) * z) / 2 - minX * z, y: (h - (maxY - minY) * z) / 2 - minY * z });
  }, [graph, pos, heightOf]);
  const fitted = useRef(false);
  useEffect(() => {
    if (graph && Object.keys(pos).length && !fitted.current && active) {
      fitted.current = true;
      requestAnimationFrame(fit);
    }
  }, [graph, pos, fit, active]);

  if (error) return <div className="q-error" style={{ margin: 16 }}>{error}</div>;
  if (!graph)
    return (
      <div className="q-message" hidden={!active}>
        <span className="spinner" />
      </div>
    );

  const tablesByName = new Map(graph.tables.map((t) => [t.name, t]));
  const rowY = (t: GraphTable, col: string) => {
    const { cols } = shownColumns(t, keysOnly, fkCols);
    const i = cols.findIndex((c) => c.name === col);
    return HEAD_H + (i < 0 ? -HEAD_H / 2 : i * ROW_H + ROW_H / 2);
  };
  const related = (name: string) => graph.foreignKeys.some((f) => (f.table === hover && f.refTable === name) || (f.refTable === hover && f.table === name));
  const matches = (name: string) => !q || name.toLowerCase().includes(q.toLowerCase());

  const edges = graph.foreignKeys
    .map((f) => {
      const a = tablesByName.get(f.table);
      const b = tablesByName.get(f.refTable);
      const pa = pos[f.table];
      const pb = pos[f.refTable];
      if (!a || !b || !pa || !pb) return null;
      const ay = pa.y + rowY(a, f.columns[0]);
      const by = pb.y + rowY(b, f.refColumns[0]);
      const self = f.table === f.refTable;
      const aRight = pa.x + BOX_W < pb.x + BOX_W / 2;
      const ax = self ? pa.x + BOX_W : aRight ? pa.x + BOX_W : pa.x;
      const bx = self ? pb.x + BOX_W : aRight ? pb.x : pb.x + BOX_W;
      const bend = self ? 50 : Math.max(40, Math.abs(bx - ax) / 2);
      const d = self
        ? `M ${ax} ${ay} C ${ax + bend} ${ay}, ${bx + bend} ${by}, ${bx} ${by}`
        : `M ${ax} ${ay} C ${ax + (aRight ? bend : -bend)} ${ay}, ${bx + (aRight ? -bend : bend)} ${by}, ${bx} ${by}`;
      const lit = hover === f.table || hover === f.refTable;
      return { key: `${f.table}.${f.name}`, d, lit, dim: (!!hover && !lit) || (!!q && !matches(f.table) && !matches(f.refTable)), end: { x: bx, y: by } };
    })
    .filter(Boolean) as { key: string; d: string; lit: boolean; dim: boolean; end: { x: number; y: number } }[];

  const onMouseMove = (e: React.MouseEvent) => {
    const dr = drag.current;
    if (!dr) return;
    const dx = e.clientX - dr.sx;
    const dy = e.clientY - dr.sy;
    if (dr.kind === "pan") setView((v) => ({ ...v, x: dr.ox + dx, y: dr.oy + dy }));
    else if (dr.name) {
      const name = dr.name;
      setPos((p) => ({ ...p, [name]: { x: dr.ox + dx / view.z, y: dr.oy + dy / view.z } }));
    }
  };

  const exportSvg = async () => {
    const xs = graph.tables.map((t) => pos[t.name]).filter(Boolean);
    const minX = Math.min(...xs.map((p) => p.x)) - 20;
    const minY = Math.min(...xs.map((p) => p.y)) - 20;
    const maxX = Math.max(...xs.map((p) => p.x)) + BOX_W + 20;
    const maxY = Math.max(...graph.tables.map((t) => (pos[t.name]?.y ?? 0) + heightOf(t))) + 20;
    const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
    const parts: string[] = [];
    for (const e of edges) parts.push(`<path d="${e.d}" fill="none" stroke="#7c6cff" stroke-width="1.5" opacity="0.7"/>`);
    for (const t of graph.tables) {
      const p = pos[t.name];
      if (!p) continue;
      const { cols, more } = shownColumns(t, keysOnly, fkCols);
      const h = boxHeight(cols.length, more);
      parts.push(`<g transform="translate(${p.x},${p.y})"><rect width="${BOX_W}" height="${h}" rx="10" fill="#ffffff" stroke="#d6d6de"/>`);
      parts.push(`<rect width="${BOX_W}" height="${HEAD_H}" rx="10" fill="#efeff5"/><text x="12" y="22" font-weight="700" font-size="13">${esc(t.name)}</text>`);
      cols.forEach((c, i) => {
        const y = HEAD_H + i * ROW_H + 15;
        parts.push(`<text x="12" y="${y}" font-size="12">${c.primaryKey ? '<tspan fill="#c27c0e" font-weight="700">PK </tspan>' : ""}${esc(c.name)}</text><text x="${BOX_W - 12}" y="${y}" font-size="11" fill="#777" text-anchor="end">${esc(c.dataType.slice(0, 22))}</text>`);
      });
      if (more) parts.push(`<text x="12" y="${HEAD_H + cols.length * ROW_H + 15}" font-size="11" fill="#777">+${more} more</text>`);
      parts.push(`</g>`);
    }
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${minX} ${minY} ${maxX - minX} ${maxY - minY}" font-family="Helvetica, Arial, sans-serif">${parts.join("")}</svg>`;
    try {
      const path = await api.exportFile(`${schema}-er-diagram.svg`, svg);
      if (path) toast("Diagram exported", "success");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  return (
    <div className="view erd" hidden={!active}>
      <div className="toolbar">
        <div className="crumb">
          <Link2 />
          <span className="schema">{schema}</span>
          <span className="dim sep">/</span>
          <span>ER diagram</span>
        </div>
        <span className="dim" style={{ fontSize: 12 }}>
          {graph.tables.length} tables · {graph.foreignKeys.length} relations
        </span>
        <div className="spacer" />
        <div className="sb-search" style={{ width: 200 }}>
          <Search />
          <input className="input" placeholder="Find table" value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} />
        </div>
        <Checkbox checked={keysOnly} onChange={setKeysOnly} label="Keys only" />
        <div className="divider" />
        <button className="icon-btn" title="Zoom out" onClick={() => setView((v) => ({ ...v, z: Math.max(0.15, v.z / 1.2) }))}>
          <Minus />
        </button>
        <span className="mono dim" style={{ width: 44, textAlign: "center", fontSize: 12 }}>
          {Math.round(view.z * 100)}%
        </span>
        <button className="icon-btn" title="Zoom in" onClick={() => setView((v) => ({ ...v, z: Math.min(2.5, v.z * 1.2) }))}>
          <Plus />
        </button>
        <button className="icon-btn" title="Fit to screen" onClick={fit}>
          <Maximize />
        </button>
        <button
          className="icon-btn"
          title="Auto layout"
          onClick={() => {
            setPos(autoLayout(graph, heightOf));
            fitted.current = false;
          }}
        >
          <LayoutGrid />
        </button>
        <button className="icon-btn" title="Reload schema" onClick={() => setRev((r) => r + 1)}>
          <RefreshCw />
        </button>
        <button className="icon-btn" title="Export as SVG" onClick={exportSvg}>
          <Download />
        </button>
      </div>
      <div
        ref={wrap}
        className="erd-canvas"
        onMouseDown={(e) => {
          if (e.button !== 0) return;
          drag.current = { kind: "pan", sx: e.clientX, sy: e.clientY, ox: view.x, oy: view.y };
        }}
        onMouseMove={onMouseMove}
        onMouseUp={() => (drag.current = null)}
        onMouseLeave={() => (drag.current = null)}
        onWheel={(e) => {
          if (!wrap.current) return;
          if (e.ctrlKey || e.metaKey) {
            const r = wrap.current.getBoundingClientRect();
            const mx = e.clientX - r.left;
            const my = e.clientY - r.top;
            const z = Math.min(2.5, Math.max(0.15, view.z * (e.deltaY < 0 ? 1.1 : 1 / 1.1)));
            setView((v) => ({ z, x: mx - ((mx - v.x) / v.z) * z, y: my - ((my - v.y) / v.z) * z }));
          } else setView((v) => ({ ...v, x: v.x - e.deltaX, y: v.y - e.deltaY }));
        }}
      >
        <div className="erd-world" style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.z})` }}>
          <svg className="erd-edges" width="6000" height="6000">
            <defs>
              <marker id="erd-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor" />
              </marker>
            </defs>
            {edges.map((e) => (
              <path key={e.key} d={e.d} className={`erd-edge ${e.lit ? "lit" : ""} ${e.dim ? "dim" : ""}`} markerEnd="url(#erd-arrow)" />
            ))}
          </svg>
          {graph.tables.map((t) => {
            const p = pos[t.name];
            if (!p) return null;
            const { cols, more } = shownColumns(t, keysOnly, fkCols);
            const dim = (!!hover && hover !== t.name && !related(t.name)) || !matches(t.name);
            return (
              <div
                key={t.name}
                className={`erd-box ${hover === t.name ? "hot" : ""} ${dim ? "dim" : ""} ${t.kind !== "table" ? "view" : ""}`}
                style={{ left: p.x, top: p.y, width: BOX_W }}
                onMouseEnter={() => setHover(t.name)}
                onMouseLeave={() => setHover(null)}
              >
                <div
                  className="erd-head"
                  onMouseDown={(e) => {
                    e.stopPropagation();
                    drag.current = { kind: "box", name: t.name, sx: e.clientX, sy: e.clientY, ox: p.x, oy: p.y };
                  }}
                  onDoubleClick={() => openTable(session.id, schema, t.name)}
                  title="Drag to move · double-click to open"
                >
                  <span className="ellipsis">{t.name}</span>
                  {t.kind !== "table" && <span className="badge">{t.kind}</span>}
                </div>
                {cols.map((c) => (
                  <div key={c.name} className={`erd-row ${fkCols.has(`${t.name}.${c.name}`) ? "fk" : ""}`}>
                    {c.primaryKey ? <KeyRound className="pk-ico" /> : <span className="erd-dot" />}
                    <span className="ellipsis">{c.name}</span>
                    <span className="erd-type ellipsis">{c.dataType}</span>
                  </div>
                ))}
                {more > 0 && <div className="erd-row dim">+{more} more</div>}
              </div>
            );
          })}
        </div>
        {graph.tables.length === 0 && <div className="q-message dim">No tables in {schema}</div>}
      </div>
    </div>
  );
}
