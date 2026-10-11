import { create } from "zustand";
import { api, errorMessage, isSql, type ConnectionConfig, type TableInfo } from "./api";

export type TableView = "data" | "structure" | "relations" | "triggers" | "info";
export type TableTab = {
  id: string;
  kind: "table";
  schema: string;
  table: string;
  view: TableView;
  /** Opened by following a foreign key: start filtered to this value. */
  initialFilter?: { column: string; value: string | null };
};
export type QueryTab = { id: string; kind: "query"; title: string; sql: string; savedId?: string };
export interface SavedQuery {
  id: string;
  name: string;
  sql: string;
  updatedAt: number;
}
type UsersTab = { id: string; kind: "users"; title: string };
type ErdTab = { id: string; kind: "erd"; title: string; schema: string };
export type Tab = TableTab | QueryTab | UsersTab | ErdTab;

export interface Session {
  id: string;
  connection: ConnectionConfig;
  serverVersion: string;
  schemas: string[];
  systemSchemas: string[];
  schema: string;
  tables: TableInfo[];
  tablesLoading: boolean;
  /** Column names per table, filled lazily, used for SQL autocomplete. */
  columns: Record<string, string[]>;
  tabs: Tab[];
  activeTabId: string | null;
}

interface Toast {
  id: string;
  kind: "success" | "error" | "info";
  message: string;
}

export type Theme = "system" | "dark" | "light";
export type FontChoice = "geist" | "arimo" | "inter" | "system";

interface State {
  connections: ConnectionConfig[];
  sessions: Session[];
  activeSessionId: string | null;
  connecting: string | null;
  theme: Theme;
  font: FontChoice;
  /** Uncommitted edits per tab ("sessionId:tabId" -> count), for close/disconnect warnings. */
  pending: Record<string, number>;
  /** A confirmation the user must answer (disconnect, close a tab with edits). */
  update: import("./api").UpdateInfo | null;
  autoUpdateCheck: boolean;
  checkForUpdates(manual?: boolean): Promise<void>;
  setAutoUpdateCheck(v: boolean): void;
  confirm: { kind: "disconnect"; sessionId: string } | { kind: "closeTab"; sessionId: string; tabId: string } | null;
  paletteOpen: boolean;
  aiOpen: boolean;
  editing: ConnectionConfig | null;
  toasts: Toast[];

  loadConnections(): Promise<void>;
  saveConnection(c: ConnectionConfig): Promise<ConnectionConfig>;
  deleteConnection(id: string): Promise<void>;
  openConnection(c: ConnectionConfig): Promise<void>;
  closeSession(id: string): Promise<void>;
  setActiveSession(id: string | null): void;
  setSchema(sessionId: string, schema: string): Promise<void>;
  /** Re-reads the database/schema list (after creating one) and switches to `select`. */
  reloadSchemas(sessionId: string, select?: string): Promise<void>;
  refreshTables(sessionId: string): Promise<void>;
  setColumns(sessionId: string, table: string, cols: string[]): void;
  openTable(sessionId: string, schema: string, table: string, view?: TableView, forceNew?: boolean, initialFilter?: TableTab["initialFilter"]): void;
  newQuery(sessionId: string, sql?: string, opts?: { title?: string; savedId?: string }): void;
  /** Saved queries per connection (stored locally on this computer). */
  saved: Record<string, SavedQuery[]>;
  saveQuery(connId: string, q: { id?: string; name: string; sql: string }): SavedQuery;
  deleteSaved(connId: string, id: string): void;
  openUsers(sessionId: string): void;
  openErd(sessionId: string): void;
  updateTab(sessionId: string, tabId: string, patch: Partial<Tab>): void;
  /** Asks first if the tab has uncommitted edits, unless `force`. */
  closeTab(sessionId: string, tabId: string, force?: boolean): void;
  requestDisconnect(sessionId: string): void;
  setConfirm(c: State["confirm"]): void;
  setActiveTab(sessionId: string, tabId: string): void;
  setTheme(t: Theme): void;
  setFont(f: FontChoice): void;
  setPending(sessionId: string, tabId: string, count: number): void;
  setPalette(open: boolean): void;
  setAiOpen(open: boolean): void;
  setEditing(c: ConnectionConfig | null): void;
  toast(message: string, kind?: Toast["kind"]): void;
  dismissToast(id: string): void;
}

const uid = () => crypto.randomUUID();

// Saved app state (history, drafts, layout), encrypted on disk. Never passwords.
export { local } from "./local";
import { local } from "./local";

const draftsKey = (connId: string) => `tabla.drafts.${connId}`;

function saveDrafts(s: Session) {
  const drafts = s.tabs.filter((t): t is QueryTab => t.kind === "query").map((t) => ({ title: t.title, sql: t.sql, savedId: t.savedId }));
  local.set(draftsKey(s.connection.id), drafts);
}

interface HistoryEntry {
  sql: string;
  at: number;
  ms: number;
  ok: boolean;
}

export function pushHistory(connId: string, entry: HistoryEntry) {
  const key = `tabla.history.${connId}`;
  const list = local.get<HistoryEntry[]>(key, []).filter((h) => h.sql !== entry.sql);
  local.set(key, [entry, ...list].slice(0, 200));
}

export function readHistory(connId: string): HistoryEntry[] {
  return local.get<HistoryEntry[]>(`tabla.history.${connId}`, []);
}

export const useApp = create<State>((set, get) => {
  const patchSession = (id: string, fn: (s: Session) => Partial<Session>) =>
    set((st) => ({ sessions: st.sessions.map((s) => (s.id === id ? { ...s, ...fn(s) } : s)) }));

  return {
    connections: [],
    sessions: [],
    activeSessionId: null,
    connecting: null,
    theme: local.get<Theme>("tabla.theme", "system"),
    font: local.get<FontChoice>("tabla.font", "geist"),
    pending: {},
    confirm: null,
    update: null,
    autoUpdateCheck: local.get("tabla.updates.auto", true),

    async checkForUpdates(manual = false) {
      if (!manual) {
        if (!get().autoUpdateCheck) return;
        const last = local.get<number>("tabla.updates.last", 0);
        if (Date.now() - last < 24 * 3600 * 1000) return;
      }
      local.set("tabla.updates.last", Date.now());
      try {
        const info = await api.checkUpdate();
        set({ update: info });
        if (manual) get().toast(info.available ? `Tabla ${info.latest} is available` : `You're up to date (Tabla ${info.current})`, info.available ? "info" : "success");
      } catch (e) {
        if (manual) get().toast(errorMessage(e), "info");
      }
    },

    setAutoUpdateCheck(v) {
      local.set("tabla.updates.auto", v);
      set({ autoUpdateCheck: v });
    },
    paletteOpen: false,
    aiOpen: false,
    editing: null,
    toasts: [],

    async loadConnections() {
      try {
        set({ connections: await api.listConnections() });
      } catch (e) {
        get().toast(errorMessage(e), "error");
      }
    },

    async saveConnection(c) {
      const saved = await api.saveConnection(c);
      set((st) => {
        const exists = st.connections.some((x) => x.id === saved.id);
        return {
          connections: exists ? st.connections.map((x) => (x.id === saved.id ? saved : x)) : [...st.connections, saved],
          // Keep open sessions' metadata (name, color, tag) in sync.
          sessions: st.sessions.map((s) => (s.connection.id === saved.id ? { ...s, connection: saved } : s)),
        };
      });
      return saved;
    },

    async deleteConnection(id) {
      await api.deleteConnection(id);
      set((st) => ({ connections: st.connections.filter((c) => c.id !== id) }));
    },

    async openConnection(c) {
      const existing = get().sessions.find((s) => s.connection.id === c.id);
      if (existing) return set({ activeSessionId: existing.id });
      set({ connecting: c.id });
      try {
        const info = await api.openSession(c);
        const schema =
          info.currentSchema && info.schemas.includes(info.currentSchema) ? info.currentSchema : info.schemas[0] ?? "";
        const drafts = local.get<{ title: string; sql: string; savedId?: string }[]>(draftsKey(c.id), []);
        const tabs: Tab[] = drafts.map((d) => ({ id: uid(), kind: "query", title: d.title, sql: d.sql, savedId: d.savedId }));
        set((st) => ({ saved: { ...st.saved, [c.id]: local.get<SavedQuery[]>(`tabla.saved.${c.id}`, []) } }));
        const session: Session = {
          id: info.sessionId,
          connection: { ...c, password: undefined, sshSecret: undefined },
          serverVersion: info.serverVersion,
          schemas: info.schemas,
          systemSchemas: info.systemSchemas,
          schema,
          tables: [],
          tablesLoading: true,
          columns: {},
          tabs,
          activeTabId: tabs[0]?.id ?? null,
        };
        set((st) => ({ sessions: [...st.sessions, session], activeSessionId: session.id }));
        if (isSql(c.driver)) await get().refreshTables(session.id);
        else patchSession(session.id, () => ({ tablesLoading: false }));
      } catch (e) {
        get().toast(`Couldn't connect to ${c.name}: ${errorMessage(e)}`, "error");
      } finally {
        set({ connecting: null });
      }
    },

    async closeSession(id) {
      const s = get().sessions.find((x) => x.id === id);
      if (s) saveDrafts(s);
      set((st) => {
        const sessions = st.sessions.filter((x) => x.id !== id);
        return {
          sessions,
          activeSessionId: st.activeSessionId === id ? sessions[sessions.length - 1]?.id ?? null : st.activeSessionId,
        };
      });
      await api.closeSession(id).catch(() => {});
    },

    setActiveSession(id) {
      set({ activeSessionId: id });
    },

    async setSchema(sessionId, schema) {
      patchSession(sessionId, () => ({ schema, columns: {} }));
      await get().refreshTables(sessionId);
    },

    async reloadSchemas(sessionId, select) {
      try {
        const schemas = await api.listSchemas(sessionId);
        patchSession(sessionId, () => ({ schemas }));
        if (select && schemas.includes(select)) await get().setSchema(sessionId, select);
      } catch (e) {
        get().toast(errorMessage(e), "error");
      }
    },

    async refreshTables(sessionId) {
      const s = get().sessions.find((x) => x.id === sessionId);
      if (!s) return;
      patchSession(sessionId, () => ({ tablesLoading: true }));
      try {
        const tables = await api.listTables(sessionId, s.schema);
        patchSession(sessionId, () => ({ tables, tablesLoading: false }));
      } catch (e) {
        patchSession(sessionId, () => ({ tablesLoading: false }));
        get().toast(errorMessage(e), "error");
      }
    },

    setColumns(sessionId, table, cols) {
      patchSession(sessionId, (s) => ({ columns: { ...s.columns, [table]: cols } }));
    },

    openTable(sessionId, schema, table, view = "data", forceNew = false, initialFilter) {
      const s = get().sessions.find((x) => x.id === sessionId);
      if (!s) return;
      const found = forceNew
        ? undefined
        : (s.tabs.find((t) => t.kind === "table" && t.schema === schema && t.table === table) as TableTab | undefined);
      if (found) {
        patchSession(sessionId, (s) => ({
          activeTabId: found.id,
          tabs: s.tabs.map((t) => (t.id === found.id ? { ...found, view } : t)),
        }));
        return;
      }
      const tab: TableTab = { id: uid(), kind: "table", schema, table, view, initialFilter };
      patchSession(sessionId, (s) => {
        // Insert next to the active tab, like a browser.
        const idx = s.tabs.findIndex((t) => t.id === s.activeTabId);
        const tabs = [...s.tabs];
        tabs.splice(idx + 1, 0, tab);
        return { tabs, activeTabId: tab.id };
      });
    },

    newQuery(sessionId, sql = "", opts) {
      const s = get().sessions.find((x) => x.id === sessionId);
      if (!s) return;
      if (opts?.savedId) {
        const open = s.tabs.find((t) => t.kind === "query" && t.savedId === opts.savedId);
        if (open) return patchSession(sessionId, () => ({ activeTabId: open.id }));
      }
      const n = s.tabs.filter((t) => t.kind === "query").length + 1;
      const tab: QueryTab = { id: uid(), kind: "query", title: opts?.title ?? `Query ${n}`, sql, savedId: opts?.savedId };
      patchSession(sessionId, (s) => ({ tabs: [...s.tabs, tab], activeTabId: tab.id }));
    },

    openUsers(sessionId) {
      const s = get().sessions.find((x) => x.id === sessionId);
      if (!s) return;
      const found = s.tabs.find((t) => t.kind === "users");
      if (found) return patchSession(sessionId, () => ({ activeTabId: found.id }));
      const tab: UsersTab = { id: uid(), kind: "users", title: "Users" };
      patchSession(sessionId, (s) => ({ tabs: [...s.tabs, tab], activeTabId: tab.id }));
    },

    openErd(sessionId) {
      const s = get().sessions.find((x) => x.id === sessionId);
      if (!s) return;
      const found = s.tabs.find((t) => t.kind === "erd" && t.schema === s.schema);
      if (found) return patchSession(sessionId, () => ({ activeTabId: found.id }));
      const tab: ErdTab = { id: uid(), kind: "erd", title: `${s.schema} diagram`, schema: s.schema };
      patchSession(sessionId, (s) => ({ tabs: [...s.tabs, tab], activeTabId: tab.id }));
    },

    updateTab(sessionId, tabId, patch) {
      patchSession(sessionId, (s) => ({
        tabs: s.tabs.map((t) => (t.id === tabId ? ({ ...t, ...patch } as Tab) : t)),
      }));
      const s = get().sessions.find((x) => x.id === sessionId);
      if (s) saveDrafts(s);
    },

    requestDisconnect(sessionId) {
      set({ confirm: { kind: "disconnect", sessionId } });
    },

    setConfirm(confirm) {
      set({ confirm });
    },

    closeTab(sessionId, tabId, force = false) {
      if (!force && get().pending[`${sessionId}:${tabId}`]) {
        set({ confirm: { kind: "closeTab", sessionId, tabId } });
        return;
      }
      patchSession(sessionId, (s) => {
        const idx = s.tabs.findIndex((t) => t.id === tabId);
        const tabs = s.tabs.filter((t) => t.id !== tabId);
        const activeTabId =
          s.activeTabId === tabId ? tabs[Math.min(idx, tabs.length - 1)]?.id ?? null : s.activeTabId;
        return { tabs, activeTabId };
      });
      const s = get().sessions.find((x) => x.id === sessionId);
      if (s) saveDrafts(s);
    },

    setActiveTab(sessionId, tabId) {
      patchSession(sessionId, () => ({ activeTabId: tabId }));
    },

    saved: {},

    saveQuery(connId, q) {
      const list = get().saved[connId] ?? local.get<SavedQuery[]>(`tabla.saved.${connId}`, []);
      const item: SavedQuery = { id: q.id ?? uid(), name: q.name.trim() || "Untitled query", sql: q.sql, updatedAt: Date.now() };
      const next = [item, ...list.filter((x) => x.id !== item.id)].sort((a, b) => a.name.localeCompare(b.name));
      local.set(`tabla.saved.${connId}`, next);
      set((st) => ({
        saved: { ...st.saved, [connId]: next },
        // Keep open tabs that show this query in sync.
        sessions: st.sessions.map((s) =>
          s.connection.id !== connId
            ? s
            : { ...s, tabs: s.tabs.map((t) => (t.kind === "query" && t.savedId === item.id ? { ...t, title: item.name } : t)) },
        ),
      }));
      return item;
    },

    deleteSaved(connId, id) {
      const next = (get().saved[connId] ?? local.get<SavedQuery[]>(`tabla.saved.${connId}`, [])).filter((x) => x.id !== id);
      local.set(`tabla.saved.${connId}`, next);
      set((st) => ({ saved: { ...st.saved, [connId]: next } }));
    },

    setTheme(theme) {
      local.set("tabla.theme", theme);
      set({ theme });
    },

    setFont(font) {
      local.set("tabla.font", font);
      set({ font });
    },

    setPending(sessionId, tabId, count) {
      const key = `${sessionId}:${tabId}`;
      if ((get().pending[key] ?? 0) === count) return;
      set((st) => {
        const pending = { ...st.pending };
        if (count) pending[key] = count;
        else delete pending[key];
        return { pending };
      });
    },

    setPalette(paletteOpen) {
      set({ paletteOpen });
    },

    setAiOpen(aiOpen) {
      set({ aiOpen });
    },

    setEditing(editing) {
      set({ editing });
    },

    toast(message, kind = "info") {
      const id = uid();
      set((st) => ({ toasts: [...st.toasts, { id, kind, message }] }));
      setTimeout(() => get().dismissToast(id), kind === "error" ? 7000 : 3500);
    },

    dismissToast(id) {
      set((st) => ({ toasts: st.toasts.filter((t) => t.id !== id) }));
    },
  };
});

export const useActiveSession = () =>
  useApp((st) => st.sessions.find((s) => s.id === st.activeSessionId) ?? null);

export function newConnection(): ConnectionConfig {
  return {
    id: uid(),
    name: "",
    driver: "postgres",
    provider: "postgres",
    host: "localhost",
    port: 5432,
    user: "",
    password: "",
    database: "",
    sslMode: "prefer",
    sslCa: "",
    sslCert: "",
    sslKey: "",
    ssh: { enabled: false, host: "", port: 22, user: "", auth: "key", keyPath: "~/.ssh/id_ed25519" },
    color: "#7c6cff",
    tag: "local",
    readOnly: false,
    aiAccess: false,
  };
}

/** Built-in databases are browsable but never edited from the UI. */
export function isSystemSchema(s: Pick<Session, "systemSchemas">, schema: string) {
  return s.systemSchemas.includes(schema);
}
