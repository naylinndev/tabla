import { useEffect, useMemo, useRef, useState } from "react";
import {
  Bot,
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
  X,
} from "lucide-react";
import { newConnection, useApp, type Session, type Tab } from "../lib/store";
import { DriverIcon } from "./icons";
import { schemaPrompt } from "../lib/ai";
import { Kbd, TagBadge } from "./ui";
import { TableView } from "./TableView";
import { QueryView } from "./QueryView";

function initials(name: string) {
  const parts = name.trim().split(/[\s_-]+/).filter(Boolean);
  return ((parts[0]?.[0] ?? "?") + (parts[1]?.[0] ?? parts[0]?.[1] ?? "")).toUpperCase();
}

export function Rail() {
  const sessions = useApp((s) => s.sessions);
  const activeId = useApp((s) => s.activeSessionId);
  const setActive = useApp((s) => s.setActiveSession);
  const theme = useApp((s) => s.theme);
  const setTheme = useApp((s) => s.setTheme);
  const setEditing = useApp((s) => s.setEditing);
  const nextTheme = theme === "system" ? "dark" : theme === "dark" ? "light" : "system";

  const setAiOpen = useApp((s) => s.setAiOpen);
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
      {sessions.length > 0 && <div className="rail-sep" />}
      {sessions.map((s) => (
        <button
          key={s.id}
          className={`rail-item ${s.id === activeId ? "active" : ""}`}
          style={s.id === activeId ? { background: s.connection.color } : undefined}
          onClick={() => setActive(s.id)}
          title={s.connection.name}
        >
          {initials(s.connection.name)}
          <span className="rail-dot" />
        </button>
      ))}
      <button className="rail-item add" onClick={() => setEditing(newConnection())} title="New connection (Mod+N)">
        <Plus />
      </button>
      <div className="spacer" data-tauri-drag-region style={{ width: "100%" }} />
      <button className="icon-btn rail-tool" onClick={() => setAiOpen(true)} title="Connect your AI (MCP)">
        <Bot />
      </button>
      <button className="icon-btn rail-tool" onClick={() => setTheme(nextTheme)} title={`Theme: ${theme}`}>
        {theme === "system" ? <SunMoon /> : theme === "dark" ? <Moon /> : <Sun />}
      </button>
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

export function Sidebar({ session }: { session: Session }) {
  const [q, setQ] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);
  const openTable = useApp((s) => s.openTable);
  const setSchema = useApp((s) => s.setSchema);
  const refresh = useApp((s) => s.refreshTables);
  const newQuery = useApp((s) => s.newQuery);
  const closeSession = useApp((s) => s.closeSession);
  const active = session.tabs.find((t) => t.id === session.activeTabId);
  const c = session.connection;
  const toast = useApp((s) => s.toast);
  const [copying, setCopying] = useState(false);

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

  const item = (t: (typeof session.tables)[number]) => (
    <button
      key={t.name}
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
    <aside className="sidebar">
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
              {c.driver === "sqlite" ? c.database.split(/[\\/]/).pop() : `${c.host}:${c.port}`} · v
              {session.serverVersion.split(/[\s-]/)[0]}
            </div>
          </div>
          <TagBadge tag={c.tag} />
          <button className="icon-btn" title="Disconnect" onClick={() => closeSession(session.id)}>
            <Power />
          </button>
        </div>
        {session.schemas.length > 1 && (
          <select
            className="select sm"
            style={{ marginBottom: 8 }}
            value={session.schema}
            onChange={(e) => setSchema(session.id, e.target.value)}
            title={c.driver === "mysql" ? "Database" : "Schema"}
          >
            {session.schemas.map((s) => (
              <option key={s}>{s}</option>
            ))}
          </select>
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
      </div>
      <div className="sb-foot">
        <button className="btn" onClick={() => newQuery(session.id)}>
          <Code2 /> New query <Kbd keys="Mod T" />
        </button>
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

  useEffect(() => {
    tabsRef.current?.querySelector(".tab.active")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [session.activeTabId]);

  return (
    <div className="shell" style={{ display: visible ? "flex" : "none", flex: 1, minWidth: 0 }}>
      <Sidebar session={session} />
      <main className={`main ${session.connection.tag === "production" ? "production" : ""}`}>
        <div className="tabbar" data-tauri-drag-region>
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
                {t.kind === "table" ? <Table2 /> : <FileCode2 />}
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
        </div>

        {session.tabs.length === 0 && <EmptyWorkspace session={session} />}
        {session.tabs.map((t) =>
          t.kind === "table" ? (
            <TableView key={t.id} session={session} tab={t} active={visible && t.id === session.activeTabId} />
          ) : (
            <QueryView key={t.id} session={session} tab={t} active={visible && t.id === session.activeTabId} />
          ),
        )}
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
