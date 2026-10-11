import { useEffect, useMemo, useRef, useState } from "react";
import {
  Bot,
  ChevronLeft,
  ChevronRight,
  Code2,
  Eye,
  FileCode2,
  Home,
  Layers,
  Lock,
  Moon,
  Sparkles,
  Plus,
  Power,
  RefreshCw,
  Search,
  Sun,
  SunMoon,
  Table2,
  TerminalSquare,
  Waypoints,
  Pencil,
  Copy,
  Trash2,
  Users as UsersIcon,
  X,
} from "lucide-react";
import { local, newConnection, useApp, type SavedQuery, type Session, type Tab } from "../lib/store";
import { isLocal } from "../lib/api";
import { DriverIcon } from "./icons";
import { schemaPrompt, engineName } from "../lib/ai";
import { ContextMenu, Kbd, Modal, TagBadge, useDrag } from "./ui";
import { useTableMenu } from "./TableActions";
import { Console } from "./Console";
import { CreateTableDialog } from "./CreateTable";
import { AppearanceMenu } from "./Dialogs";
import { UsersView } from "./UsersView";
import { ErdView } from "./ErdView";
import { Select } from "./controls";
import { TableView } from "./TableView";
import { QueryView } from "./QueryView";


export function Rail() {
  const sessions = useApp((s) => s.sessions);
  const activeId = useApp((s) => s.activeSessionId);
  const setActive = useApp((s) => s.setActiveSession);
  const theme = useApp((s) => s.theme);
  const setEditing = useApp((s) => s.setEditing);

  const setAiOpen = useApp((s) => s.setAiOpen);
  const [appearance, setAppearance] = useState(false);
  const connections = useApp((s) => s.connections);
  const connecting = useApp((s) => s.connecting);
  const openConnection = useApp((s) => s.openConnection);
  // Every saved connection, in saved order, plus any open session not (or no longer) saved.
  const railItems = [
    ...connections.map((conn) => ({ conn, session: sessions.find((s) => s.connection.id === conn.id) })),
    ...sessions.filter((s) => !connections.some((c) => c.id === s.connection.id)).map((s) => ({ conn: s.connection, session: s })),
  ];
  return (
    <nav className="rail">
      <div className="titlebar-space" data-tauri-drag-region style={{ width: "100%" }} />
      <button
        className={`rail-item rail-home ${activeId === null ? "active" : ""}`}
        onClick={() => setActive(null)}
        title="Home: all connections (Mod+0)"
        aria-label="Home"
      >
        <Home />
      </button>
      {railItems.length > 0 && <div className="rail-sep" />}
      <div className="rail-list">
        {railItems.map(({ conn: c, session: s }) => {
          const active = !!s && s.id === activeId;
          return (
            <div
              key={c.id}
              className={`rail-conn ${active ? "active" : ""} ${s ? "open" : "closed"}`}
              title={`${c.name} · ${isLocal(c.driver) ? c.database : `${c.host}:${c.port}`}${s ? "" : " · click to connect"}`}
            >
              <button
                className={`rail-item conn ${active ? "active" : ""}`}
                style={{ "--c": c.color } as React.CSSProperties}
                onClick={() => (s ? setActive(s.id) : openConnection(c))}
                aria-label={s ? c.name : `Connect to ${c.name}`}
              >
                <DriverIcon driver={c.driver} provider={c.provider} size={30} />
                {s && <span className="rail-dot" />}
                {connecting === c.id && <span className="rail-spin spinner" />}
                {c.tag === "production" && <span className="rail-prod" />}
              </button>
              <span className="rail-name">{c.name}</span>
            </div>
          );
        })}
      </div>
      <button className="rail-item add" onClick={() => setEditing(newConnection())} title="New connection (Mod+N)">
        <Plus />
      </button>
      <div className="spacer" data-tauri-drag-region style={{ width: "100%" }} />
      <button className="icon-btn rail-tool" onClick={() => setAiOpen(true)} title="Connect your AI (MCP)">
        <Bot />
      </button>
      <div style={{ position: "relative" }}>
        <button className={`icon-btn rail-tool ${appearance ? "on" : ""}`} onClick={() => setAppearance(!appearance)} title="Appearance: theme and font">
          {theme === "system" ? <SunMoon /> : theme === "dark" ? <Moon /> : <Sun />}
        </button>
        {appearance && <AppearanceMenu onClose={() => setAppearance(false)} />}
      </div>
    </nav>
  );
}

function highlight(name: string, q: string) {
  if (!q) return name;
  const i = name.toLowerCase().indexOf(q.toLowerCase());
  if (i < 0) return name;
  return (
    <>
      {name.slice(0, i)}
      <mark>{name.slice(i, i + q.length)}</mark>
      {name.slice(i + q.length)}
    </>
  );
}

function fuzzy(name: string, q: string) {
  if (!q) return true;
  const n = name.toLowerCase();
  const s = q.toLowerCase();
  if (n.includes(s)) return true;
  let j = 0;
  for (const ch of n) if (ch === s[j]) j++;
  return j === s.length;
}

function compact(n: number) {
  return Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 }).format(n);
}

function Sidebar({ session }: { session: Session }) {
  const [q, setQ] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);
  const openTable = useApp((s) => s.openTable);
  const setSchema = useApp((s) => s.setSchema);
  const refresh = useApp((s) => s.refreshTables);
  const newQuery = useApp((s) => s.newQuery);
  const active = session.tabs.find((t) => t.id === session.activeTabId);
  const c = session.connection;
  const toast = useApp((s) => s.toast);
  const [copying, setCopying] = useState(false);
  const [creating, setCreating] = useState(false);
  const savedQueries = useApp((s) => s.saved[c.id]) ?? [];
  const [savedMenu, setSavedMenu] = useState<{ x: number; y: number; id: string } | null>(null);
  const [renamingSaved, setRenamingSaved] = useState<SavedQuery | null>(null);
  const [width, setWidth] = useState(() => local.get("tabla.sidebarWidth", 272));
  const startW = useRef(width);
  const saveWidth = (w: number) => {
    setWidth(w);
    local.set("tabla.sidebarWidth", w);
  };
  const resize = useDrag((dx) => saveWidth(Math.max(200, Math.min(520, startW.current + dx))));

  const copySchema = async () => {
    setCopying(true);
    try {
      const text = await schemaPrompt(session);
      await navigator.clipboard.writeText(text);
      toast("Schema copied. Paste it into ChatGPT, Claude or Cursor.", "success");
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setCopying(false);
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "f") return;
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "l") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const { tables, views } = useMemo(() => {
    const list = session.tables.filter((t) => fuzzy(t.name, q));
    return { tables: list.filter((t) => t.kind === "table"), views: list.filter((t) => t.kind !== "table") };
  }, [session.tables, q]);

  const tableMenu = useTableMenu(session);
  const item = (t: (typeof session.tables)[number]) => (
    <button
      key={t.name}
      onContextMenu={(e) => {
        e.preventDefault();
        tableMenu.open(e, t);
      }}
      className={`sb-item ${active?.kind === "table" && active.table === t.name && active.schema === session.schema ? "active" : ""}`}
      onClick={() => openTable(session.id, session.schema, t.name)}
      onDoubleClick={() => openTable(session.id, session.schema, t.name, "structure")}
      title={t.name}
    >
      {t.kind === "table" ? <Table2 /> : t.kind === "materialized" ? <Layers /> : <Eye />}
      <span className="name">{highlight(t.name, q)}</span>
      {t.rowsEstimate !== null && t.rowsEstimate > 0 && <span className="rows">{compact(t.rowsEstimate)}</span>}
    </button>
  );

  return (
    <aside className="sidebar" style={{ width }}>
      {tableMenu.element}
      {savedMenu && (
        <ContextMenu
          x={savedMenu.x}
          y={savedMenu.y}
          onClose={() => setSavedMenu(null)}
          items={[
            {
              label: "Open",
              icon: <FileCode2 />,
              onClick: () => {
                const sq = savedQueries.find((x) => x.id === savedMenu.id);
                if (sq) newQuery(session.id, sq.sql, { title: sq.name, savedId: sq.id });
              },
            },
            {
              label: "Rename…",
              icon: <Pencil />,
              onClick: () => setRenamingSaved(savedQueries.find((x) => x.id === savedMenu.id) ?? null),
            },
            {
              label: "Copy SQL",
              icon: <Copy />,
              onClick: () => navigator.clipboard.writeText(savedQueries.find((x) => x.id === savedMenu.id)?.sql ?? ""),
            },
            "sep",
            { label: "Delete", icon: <Trash2 />, danger: true, onClick: () => useApp.getState().deleteSaved(c.id, savedMenu.id) },
          ]}
        />
      )}
      {renamingSaved && (
        <Modal onClose={() => setRenamingSaved(null)}>
          <div className="modal-head">
            <Pencil color="var(--accent-2)" />
            <div>
              <h2>Rename saved query</h2>
            </div>
          </div>
          <div className="modal-body">
            <input className="input" autoFocus defaultValue={renamingSaved.name} id="rename-saved" onKeyDown={(e) => e.key === "Enter" && (document.getElementById("rename-saved-ok") as HTMLButtonElement)?.click()} />
          </div>
          <div className="modal-foot">
            <div className="spacer" />
            <button className="btn" onClick={() => setRenamingSaved(null)}>
              Cancel
            </button>
            <button
              id="rename-saved-ok"
              className="btn primary"
              onClick={() => {
                const v = (document.getElementById("rename-saved") as HTMLInputElement).value;
                useApp.getState().saveQuery(c.id, { id: renamingSaved.id, name: v, sql: renamingSaved.sql });
                setRenamingSaved(null);
              }}
            >
              Rename
            </button>
          </div>
        </Modal>
      )}
      {creating && <CreateTableDialog session={session} onClose={() => setCreating(false)} />}
      <div
        className={`sb-resize ${resize.active ? "on" : ""}`}
        onMouseDown={(e) => {
          startW.current = width;
          resize.start(e);
        }}
        onDoubleClick={() => saveWidth(272)}
        title="Drag to resize · double-click to reset"
      />
      <div className="titlebar-space" data-tauri-drag-region />
      <div className="sb-head">
        <div className="sb-conn">
          <DriverIcon driver={c.driver} provider={c.provider} size={30} />
          <div style={{ minWidth: 0, flex: 1 }}>
            <div className="sb-conn-name" style={{ display: "flex", gap: 6, alignItems: "center" }}>
              <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{c.name}</span>
              {c.readOnly && <Lock size={11} className="dim" />}
            </div>
            <div className="sb-conn-meta">
              {/* The file name is already the title for local databases. */}
              {isLocal(c.driver) ? engineName(c.driver) : `${c.host}:${c.port}`} · v
              {session.serverVersion.split(/[\s-]/)[0].replace(/^v/i, "")}
            </div>
          </div>
          <TagBadge tag={c.tag} />
          <button className="icon-btn" title="Disconnect" onClick={() => useApp.getState().requestDisconnect(session.id)}>
            <Power />
          </button>
        </div>
        {session.schemas.length > 1 && (
          <div style={{ marginBottom: 8 }}>
            <Select
              size="sm"
              className="w-full"
              value={session.schema}
              title={c.driver === "mysql" || c.driver === "clickhouse" ? "Database" : "Schema"}
              options={session.schemas.map((s) => ({ value: s, label: s }))}
              onChange={(v) => setSchema(session.id, v)}
            />
          </div>
        )}
        <div className="sb-search">
          <Search />
          <input
            ref={searchRef}
            className="input"
            placeholder="Search tables"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const first = tables[0] ?? views[0];
                if (first) openTable(session.id, session.schema, first.name);
              } else if (e.key === "Escape") setQ("");
            }}
            spellCheck={false}
          />
          {!q && <Kbd keys="Mod L" />}
        </div>
      </div>
      <div className="sb-list">
        <div className="sb-section">
          Tables <span className="count">{tables.length}</span>
          <div className="spacer" />
          {!c.readOnly && (
            <button className="icon-btn" style={{ width: 20, height: 20 }} title="Create table" onClick={() => setCreating(true)}>
              <Plus size={12} />
            </button>
          )}
          <button
            className="icon-btn"
            style={{ width: 20, height: 20 }}
            title="Refresh"
            onClick={() => refresh(session.id)}
          >
            {session.tablesLoading ? <span className="spinner" style={{ width: 11, height: 11 }} /> : <RefreshCw size={12} />}
          </button>
        </div>
        {tables.map(item)}
        {!session.tablesLoading && tables.length === 0 && (
          <div className="sb-empty">{q ? "No matching tables" : "No tables in this schema"}</div>
        )}
        {views.length > 0 && (
          <>
            <div className="sb-section">
              Views <span className="count">{views.length}</span>
            </div>
            {views.map(item)}
          </>
        )}
        {savedQueries.length > 0 && (
          <>
            <div className="sb-section">
              Saved queries <span className="count">{savedQueries.length}</span>
            </div>
            {savedQueries
              .filter((sq) => fuzzy(sq.name, q))
              .map((sq) => (
                <button
                  key={sq.id}
                  className={`sb-item ${active?.kind === "query" && active.savedId === sq.id ? "active" : ""}`}
                  onClick={() => newQuery(session.id, sq.sql, { title: sq.name, savedId: sq.id })}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setSavedMenu({ x: e.clientX, y: e.clientY, id: sq.id });
                  }}
                  title={sq.sql}
                >
                  <FileCode2 />
                  <span className="name">{highlight(sq.name, q)}</span>
                </button>
              ))}
          </>
        )}
      </div>
      <div className="sb-foot">
        <button className="btn" onClick={() => newQuery(session.id)}>
          <Code2 /> New query <Kbd keys="Mod T" />
        </button>
        <button className="icon-btn" style={{ width: 32, height: 32 }} title="ER diagram of this schema" onClick={() => useApp.getState().openErd(session.id)}>
          <Waypoints />
        </button>
        {!isLocal(c.driver) && (
          <button className="icon-btn" style={{ width: 32, height: 32 }} title="Users & privileges" onClick={() => useApp.getState().openUsers(session.id)}>
            <UsersIcon />
          </button>
        )}
        <button className="icon-btn" style={{ width: 32, height: 32 }} title="Copy schema as an AI prompt" onClick={copySchema} disabled={copying}>
          {copying ? <span className="spinner" /> : <Sparkles />}
        </button>
      </div>
    </aside>
  );
}

function tabTitle(t: Tab) {
  return t.kind === "table" ? t.table : t.title;
}

export function Workspace({ session, visible }: { session: Session; visible: boolean }) {
  const setActiveTab = useApp((s) => s.setActiveTab);
  const closeTab = useApp((s) => s.closeTab);
  const newQuery = useApp((s) => s.newQuery);
  const tabsRef = useRef<HTMLDivElement>(null);
  const [consoleOpen, setConsoleOpen] = useState(false);

  // Browser-style back/forward over the tabs you've visited.
  const nav = useRef<{ stack: string[]; idx: number; jumping: boolean }>({ stack: [], idx: -1, jumping: false });
  const [, forceNav] = useState(0);
  useEffect(() => {
    const n = nav.current;
    const id = session.activeTabId;
    if (!id) return;
    if (n.jumping) {
      n.jumping = false;
      return;
    }
    if (n.stack[n.idx] === id) return;
    n.stack = [...n.stack.slice(0, n.idx + 1), id].slice(-100);
    n.idx = n.stack.length - 1;
    forceNav((x) => x + 1);
  }, [session.activeTabId]);
  const stepRef = useRef<(d: -1 | 1) => void>(() => {});
  const liveIds = new Set(session.tabs.map((t) => t.id));
  const step = (dir: -1 | 1) => {
    const n = nav.current;
    let i = n.idx + dir;
    while (i >= 0 && i < n.stack.length && !liveIds.has(n.stack[i])) i += dir;
    if (i < 0 || i >= n.stack.length) return;
    n.idx = i;
    n.jumping = true;
    setActiveTab(session.id, n.stack[i]);
    forceNav((x) => x + 1);
  };
  stepRef.current = step;
  const canBack = nav.current.stack.slice(0, nav.current.idx).some((id) => liveIds.has(id));
  const canFwd = nav.current.stack.slice(nav.current.idx + 1).some((id) => liveIds.has(id));

  useEffect(() => {
    if (!visible) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "c") {
        e.preventDefault();
        setConsoleOpen((v) => !v);
      } else if ((e.metaKey || e.ctrlKey) && e.altKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
        e.preventDefault();
        stepRef.current(e.key === "ArrowLeft" ? -1 : 1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [visible]);

  useEffect(() => {
    tabsRef.current?.querySelector(".tab.active")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [session.activeTabId]);

  return (
    <div className="shell" style={{ display: visible ? "flex" : "none", flex: 1, minWidth: 0 }}>
      <Sidebar session={session} />
      <main className={`main ${session.connection.tag === "production" ? "production" : ""}`}>
        <div className="tabbar" data-tauri-drag-region>
          <div className="nav-btns">
            <button className="icon-btn" disabled={!canBack} onClick={() => step(-1)} title="Back (Mod+Option+←)">
              <ChevronLeft />
            </button>
            <button className="icon-btn" disabled={!canFwd} onClick={() => step(1)} title="Forward (Mod+Option+→)">
              <ChevronRight />
            </button>
          </div>
          <div className="tabs-scroll" ref={tabsRef}>
            {session.tabs.map((t) => (
              <div
                key={t.id}
                className={`tab ${t.id === session.activeTabId ? "active" : ""}`}
                onMouseDown={(e) => {
                  if (e.button === 1) {
                    e.preventDefault();
                    closeTab(session.id, t.id);
                  } else setActiveTab(session.id, t.id);
                }}
                title={t.kind === "table" ? `${t.schema}.${t.table}` : t.title}
              >
                {t.kind === "table" ? <Table2 /> : t.kind === "users" ? <UsersIcon /> : t.kind === "erd" ? <Waypoints /> : <FileCode2 />}
                <span className="title">{tabTitle(t)}</span>
                <button
                  className="close"
                  onMouseDown={(e) => e.stopPropagation()}
                  onClick={() => closeTab(session.id, t.id)}
                  aria-label="Close tab"
                >
                  <X />
                </button>
              </div>
            ))}
          </div>
          <button className="icon-btn" onClick={() => newQuery(session.id)} title="New query (Mod+T)">
            <Plus />
          </button>
          <div className="spacer" data-tauri-drag-region style={{ height: "100%" }} />
          <button className={`icon-btn ${consoleOpen ? "on" : ""}`} onClick={() => setConsoleOpen(!consoleOpen)} title="Console: every query Tabla sends (Mod+Shift+C)">
            <TerminalSquare />
          </button>
        </div>

        {session.tabs.length === 0 && <EmptyWorkspace session={session} />}
        {session.tabs.map((t) =>
          t.kind === "table" ? (
            <TableView key={t.id} session={session} tab={t} active={visible && t.id === session.activeTabId} />
          ) : t.kind === "users" ? (
            <UsersView key={t.id} session={session} active={visible && t.id === session.activeTabId} />
          ) : t.kind === "erd" ? (
            <ErdView key={t.id} session={session} schema={t.schema} active={visible && t.id === session.activeTabId} />
          ) : (
            <QueryView key={t.id} session={session} tab={t} active={visible && t.id === session.activeTabId} />
          ),
        )}
        {consoleOpen && <Console session={session} onClose={() => setConsoleOpen(false)} />}
      </main>
    </div>
  );
}

function EmptyWorkspace({ session }: { session: Session }) {
  const newQuery = useApp((s) => s.newQuery);
  const setPalette = useApp((s) => s.setPalette);
  return (
    <div className="q-message">
      <div className="card">
        <DriverIcon driver={session.connection.driver} provider={session.connection.provider} size={44} />
        <div style={{ fontSize: 16, fontWeight: 600, color: "var(--text)" }}>Connected to {session.connection.name}</div>
        <div className="dim">
          {session.tables.length} tables in <span className="mono">{session.schema}</span>. Pick one from the sidebar or
          start a query.
        </div>
        <div style={{ display: "flex", gap: 8, marginTop: 6 }}>
          <button className="btn primary lg" onClick={() => newQuery(session.id)}>
            <Code2 /> New query
          </button>
          <button className="btn lg" onClick={() => setPalette(true)}>
            <Search /> Open anything <Kbd keys="Mod K" />
          </button>
        </div>
      </div>
    </div>
  );
}
