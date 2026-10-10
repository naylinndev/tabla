import { memo, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, Inbox, KeyRound } from "lucide-react";
import { isBoolType, isJsonType, isNumericType, toTSV } from "../lib/sql";

export interface GridColumn {
  name: string;
  typeName: string;
  pk?: boolean;
}

export interface Cursor {
  row: number;
  col: number;
}

interface Props {
  columns: GridColumn[];
  rowCount: number;
  /** `undefined` renders as DEFAULT (an untouched cell of a new row). */
  getCell(row: number, col: number): string | null | undefined;
  rowState?(row: number): "deleted" | "inserted" | null;
  isEdited?(row: number, col: number): boolean;
  editable?: boolean;
  onEdit?(row: number, col: number, value: string | null): void;
  sort?: { column: string; desc: boolean } | null;
  onSort?(column: string): void;
  rowNumberOffset?: number;
  cursor: Cursor | null;
  onCursor(c: Cursor | null): void;
  selectedRows: Set<number>;
  onSelectRows(rows: Set<number>): void;
  onContextMenu?(e: React.MouseEvent, row: number, col: number): void;
  onDeleteRows?(rows: number[]): void;
  loading?: boolean;
  emptyText?: string;
  onCopied?(what: string): void;
}

const ROW_H = 32;
const HEAD_H = 34;
const MAX_RENDER_CHARS = 400;

function initialWidth(col: GridColumn, sample: (string | null | undefined)[]) {
  const longest = Math.max(
    col.name.length + (col.pk ? 3 : 0),
    Math.min(col.typeName.length, 14) * 0.8 + col.name.length * 0.4,
    ...sample.map((v) => (v == null ? 4 : Math.min(v.length, 60))),
  );
  return Math.round(Math.min(400, Math.max(80, longest * 7.6 + 34)));
}

function CellValue({ v, type }: { v: string | null | undefined; type: string }) {
  if (v === undefined) return <span className="default">DEFAULT</span>;
  if (v === null) return <span className="null">NULL</span>;
  if (isBoolType(type)) {
    const t = v === "t" || v === "true" || v === "1";
    return <span className={`bool ${t ? "t" : ""}`}>{t ? "true" : "false"}</span>;
  }
  const shown = v.length > MAX_RENDER_CHARS ? v.slice(0, MAX_RENDER_CHARS) + "…" : v;
  if (isJsonType(type)) return <span className="v json">{shown}</span>;
  if (/^(\\x|0x)[0-9a-f]/i.test(v) && /bytea|blob|binary/i.test(type)) return <span className="v bin">{shown}</span>;
  return <span className="v">{shown}</span>;
}

export const DataGrid = memo(function DataGrid(p: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [editing, setEditing] = useState<{ row: number; col: number; value: string } | null>(null);
  const [widths, setWidths] = useState<number[]>([]);
  const [resizing, setResizing] = useState<number | null>(null);
  const anchorRow = useRef<number | null>(null);

  const colKey = p.columns.map((c) => c.name).join("\u0000");
  useEffect(() => {
    const sampleN = Math.min(p.rowCount, 60);
    setWidths(
      p.columns.map((c, ci) =>
        initialWidth(
          c,
          Array.from({ length: sampleN }, (_, r) => p.getCell(r, ci)),
        ),
      ),
    );
    setEditing(null);
    // Recompute only when the column set changes, not on every data refresh.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [colKey]);

  const gutterW = Math.max(44, String((p.rowNumberOffset ?? 0) + p.rowCount).length * 8 + 22);
  const offsets = useMemo(() => {
    const o: number[] = [];
    let x = gutterW;
    for (const w of widths) {
      o.push(x);
      x += w;
    }
    return { left: o, total: x };
  }, [widths, gutterW]);
  const numeric = useMemo(() => p.columns.map((c) => isNumericType(c.typeName)), [p.columns]);

  const virt = useVirtualizer({
    count: p.rowCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_H,
    overscan: 14,
  });

  const ensureVisible = (c: Cursor) => {
    virt.scrollToIndex(c.row, { align: "auto" });
    const el = scrollRef.current;
    if (!el) return;
    const left = offsets.left[c.col] ?? 0;
    const right = left + (widths[c.col] ?? 0);
    if (left - gutterW < el.scrollLeft) el.scrollLeft = left - gutterW;
    else if (right > el.scrollLeft + el.clientWidth) el.scrollLeft = right - el.clientWidth;
  };

  const move = (dr: number, dc: number) => {
    const cur = p.cursor ?? { row: 0, col: 0 };
    const next = {
      row: Math.max(0, Math.min(p.rowCount - 1, cur.row + dr)),
      col: Math.max(0, Math.min(p.columns.length - 1, cur.col + dc)),
    };
    p.onCursor(next);
    p.onSelectRows(new Set());
    ensureVisible(next);
  };

  const startEdit = (row: number, col: number, initial?: string) => {
    if (!p.editable || p.rowState?.(row) === "deleted") return;
    const v = p.getCell(row, col);
    setEditing({ row, col, value: initial ?? (v ?? "") });
  };

  const commitEdit = (advance?: Cursor) => {
    if (!editing) return;
    const original = p.getCell(editing.row, editing.col);
    if (editing.value !== (original ?? "") || (original == null && editing.value !== "")) {
      p.onEdit?.(editing.row, editing.col, editing.value);
    }
    setEditing(null);
    scrollRef.current?.focus();
    if (advance) {
      p.onCursor(advance);
      ensureVisible(advance);
    }
  };

  const copy = async () => {
    let text = "";
    let what = "";
    if (p.selectedRows.size) {
      const rows = [...p.selectedRows].sort((a, b) => a - b);
      text = toTSV(rows.map((r) => p.columns.map((_, c) => p.getCell(r, c) ?? null)));
      what = `${rows.length} row${rows.length > 1 ? "s" : ""}`;
    } else if (p.cursor) {
      text = p.getCell(p.cursor.row, p.cursor.col) ?? "NULL";
      what = "value";
    }
    if (!what) return;
    await navigator.clipboard.writeText(text);
    p.onCopied?.(what);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (editing) return;
    const modKey = e.metaKey || e.ctrlKey;
    const k = e.key;
    if (k === "ArrowDown") move(modKey ? p.rowCount : 1, 0);
    else if (k === "ArrowUp") move(modKey ? -p.rowCount : -1, 0);
    else if (k === "ArrowRight" || (k === "Tab" && !e.shiftKey)) move(0, modKey ? p.columns.length : 1);
    else if (k === "ArrowLeft" || (k === "Tab" && e.shiftKey)) move(0, modKey ? -p.columns.length : -1);
    else if (k === "PageDown") move(20, 0);
    else if (k === "PageUp") move(-20, 0);
    else if ((k === "Enter" || k === "F2") && p.cursor) startEdit(p.cursor.row, p.cursor.col);
    else if (k === "Escape") {
      p.onSelectRows(new Set());
    } else if (modKey && k.toLowerCase() === "c") copy();
    else if (modKey && k.toLowerCase() === "a") {
      p.onSelectRows(new Set(Array.from({ length: p.rowCount }, (_, i) => i)));
    } else if ((k === "Delete" || (k === "Backspace" && modKey)) && p.onDeleteRows) {
      const rows = p.selectedRows.size ? [...p.selectedRows] : p.cursor ? [p.cursor.row] : [];
      if (rows.length) p.onDeleteRows(rows);
    } else if (p.editable && p.cursor && k.length === 1 && !modKey && !e.altKey) {
      startEdit(p.cursor.row, p.cursor.col, k);
    } else return;
    e.preventDefault();
  };

  const clickGutter = (e: React.MouseEvent, row: number) => {
    const next = new Set(e.metaKey || e.ctrlKey ? p.selectedRows : []);
    if (e.shiftKey && anchorRow.current !== null) {
      const [a, b] = [anchorRow.current, row].sort((x, y) => x - y);
      for (let i = a; i <= b; i++) next.add(i);
    } else if (next.has(row) && (e.metaKey || e.ctrlKey)) next.delete(row);
    else {
      next.add(row);
      anchorRow.current = row;
    }
    p.onSelectRows(next);
    p.onCursor({ row, col: p.cursor?.col ?? 0 });
  };

  const startResize = (e: React.MouseEvent, ci: number) => {
    e.preventDefault();
    e.stopPropagation();
    const sx = e.clientX;
    const sw = widths[ci];
    setResizing(ci);
    const mv = (ev: MouseEvent) =>
      setWidths((w) => w.map((x, i) => (i === ci ? Math.max(48, sw + ev.clientX - sx) : x)));
    const up = () => {
      setResizing(null);
      window.removeEventListener("mousemove", mv);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", mv);
    window.addEventListener("mouseup", up);
  };

  const autoFit = (ci: number) => {
    const sample = virt.getVirtualItems().map((v) => p.getCell(v.index, ci));
    setWidths((w) => w.map((x, i) => (i === ci ? initialWidth(p.columns[ci], sample) + 10 : x)));
  };

  const items = virt.getVirtualItems();
  const totalH = virt.getTotalSize() + HEAD_H;

  return (
    <div className="grid-wrap">
      {p.loading && <div className="grid-loading" />}
      <div
        ref={scrollRef}
        className="grid"
        tabIndex={0}
        onKeyDown={onKeyDown}
        role="grid"
        aria-rowcount={p.rowCount}
        aria-colcount={p.columns.length}
      >
        <div className="grid-inner" style={{ width: offsets.total, height: totalH }}>
          <div className="grid-head" style={{ width: offsets.total }}>
            <div className="gutter" style={{ width: gutterW }} />
            {p.columns.map((c, ci) => {
              const sorted = p.sort?.column === c.name ? p.sort : null;
              return (
                <div
                  key={c.name + ci}
                  className={`gh-cell ${numeric[ci] ? "num" : ""}`}
                  style={{ width: widths[ci] }}
                  onClick={() => p.onSort?.(c.name)}
                  title={`${c.name} · ${c.typeName}`}
                  role="columnheader"
                >
                  {c.pk && <KeyRound className="pk" />}
                  <span className="nm">{c.name}</span>
                  <span className="ty">{c.typeName.toLowerCase()}</span>
                  {sorted && (sorted.desc ? <ArrowDown className="sort" /> : <ArrowUp className="sort" />)}
                  <div
                    className={`gh-resize ${resizing === ci ? "on" : ""}`}
                    onMouseDown={(e) => startResize(e, ci)}
                    onDoubleClick={(e) => {
                      e.stopPropagation();
                      autoFit(ci);
                    }}
                    onClick={(e) => e.stopPropagation()}
                  />
                </div>
              );
            })}
          </div>

          {items.map((vr) => {
            const r = vr.index;
            const state = p.rowState?.(r) ?? null;
            const selected = p.selectedRows.has(r);
            return (
              <div
                key={vr.key}
                className={`g-row ${selected ? "selected" : ""} ${state ?? ""}`}
                style={{ top: HEAD_H + vr.start, width: offsets.total }}
                role="row"
              >
                <div className="gutter" style={{ width: gutterW }} onMouseDown={(e) => clickGutter(e, r)}>
                  {state === "inserted" ? "new" : (p.rowNumberOffset ?? 0) + r + 1}
                </div>
                {p.columns.map((c, ci) => {
                  const focus = p.cursor?.row === r && p.cursor?.col === ci;
                  const isEditing = editing?.row === r && editing?.col === ci;
                  const edited = p.isEdited?.(r, ci);
                  return (
                    <div
                      key={ci}
                      className={`g-cell ${numeric[ci] ? "num" : ""} ${focus ? "focus" : ""} ${edited ? "edited" : ""}`}
                      style={{ width: widths[ci] }}
                      onMouseDown={(e) => {
                        if (e.button !== 0) return;
                        if (editing && !isEditing) commitEdit();
                        p.onCursor({ row: r, col: ci });
                        if (!e.shiftKey) p.onSelectRows(new Set());
                      }}
                      onDoubleClick={() => startEdit(r, ci)}
                      onContextMenu={(e) => {
                        e.preventDefault();
                        p.onCursor({ row: r, col: ci });
                        p.onContextMenu?.(e, r, ci);
                      }}
                      role="gridcell"
                    >
                      {isEditing ? (
                        <input
                          autoFocus
                          value={editing.value}
                          onChange={(e) => setEditing({ ...editing, value: e.target.value })}
                          onBlur={() => commitEdit()}
                          onFocus={(e) => {
                            // Typing to start an edit should append, not select.
                            const len = e.target.value.length;
                            e.target.setSelectionRange(len, len);
                          }}
                          onKeyDown={(e) => {
                            e.stopPropagation();
                            if (e.key === "Enter")
                              commitEdit({ row: Math.min(r + 1, p.rowCount - 1), col: ci });
                            else if (e.key === "Tab") {
                              e.preventDefault();
                              commitEdit({ row: r, col: Math.min(ci + (e.shiftKey ? -1 : 1), p.columns.length - 1) });
                            } else if (e.key === "Escape") {
                              setEditing(null);
                              scrollRef.current?.focus();
                            }
                          }}
                        />
                      ) : (
                        <CellValue v={p.getCell(r, ci)} type={c.typeName} />
                      )}
                    </div>
                  );
                })}
              </div>
            );
          })}
        </div>
      </div>
      {!p.loading && p.rowCount === 0 && (
        <div className="grid-empty">
          <div>
            <Inbox />
            {p.emptyText ?? "No rows"}
          </div>
        </div>
      )}
    </div>
  );
});
