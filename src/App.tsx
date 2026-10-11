import { useEffect } from "react";
import { newConnection, useApp } from "./lib/store";
import { Home } from "./components/Home";
import { Rail, Workspace } from "./components/Workspace";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { CommandPalette } from "./components/CommandPalette";
import { AiConnect } from "./components/AiConnect";
import { ConfirmDialogs } from "./components/Dialogs";
import { RedisWorkspace } from "./components/RedisWorkspace";
import { Toasts, isMac } from "./components/ui";

function useTheme() {
  const theme = useApp((s) => s.theme);
  useEffect(() => {
    const root = document.documentElement;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      root.dataset.theme = theme === "system" ? (mq.matches ? "dark" : "light") : theme;
    };
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme]);
}

function useFont() {
  const font = useApp((s) => s.font);
  useEffect(() => {
    document.documentElement.dataset.font = font;
  }, [font]);
}

function useGlobalShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const st = useApp.getState();
      const m = isMac ? e.metaKey : e.ctrlKey;
      if (!m) return;
      const k = e.key.toLowerCase();
      const session = st.sessions.find((s) => s.id === st.activeSessionId);
      if (k === "k" || k === "p") {
        e.preventDefault();
        st.setPalette(!st.paletteOpen);
      } else if (k === "0") {
        e.preventDefault();
        st.setActiveSession(null);
      } else if (k === "t" && session) {
        e.preventDefault();
        st.newQuery(session.id);
      } else if (k === "w" && session?.activeTabId) {
        e.preventDefault();
        st.closeTab(session.id, session.activeTabId);
      } else if (k === "n" && !e.shiftKey) {
        e.preventDefault();
        st.setEditing(newConnection());
      } else if (/^[1-9]$/.test(k) && session) {
        e.preventDefault();
        const tab = k === "9" ? session.tabs[session.tabs.length - 1] : session.tabs[+k - 1];
        if (tab) st.setActiveTab(session.id, tab.id);
      } else if ((k === "]" || k === "[") && e.shiftKey && session && session.tabs.length) {
        e.preventDefault();
        const i = session.tabs.findIndex((t) => t.id === session.activeTabId);
        const n = session.tabs.length;
        st.setActiveTab(session.id, session.tabs[(i + (k === "]" ? 1 : -1) + n) % n].id);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

export default function App() {
  const sessions = useApp((s) => s.sessions);
  const activeId = useApp((s) => s.activeSessionId);
  const editing = useApp((s) => s.editing);
  const loadConnections = useApp((s) => s.loadConnections);

  useTheme();
  useFont();
  useGlobalShortcuts();
  useEffect(() => {
    loadConnections();
    // At most once a day, and only if enabled (Appearance menu).
    const t = setTimeout(() => useApp.getState().checkForUpdates(), 4000);
    return () => clearTimeout(t);
  }, [loadConnections]);

  return (
    <div className="shell" onContextMenu={(e) => e.preventDefault()}>
      <Rail />
      {activeId === null && <Home />}
      {sessions.map((s) =>
        s.connection.driver === "redis" ? (
          <RedisWorkspace key={s.id} session={s} visible={s.id === activeId} />
        ) : (
          <Workspace key={s.id} session={s} visible={s.id === activeId} />
        ),
      )}
      {editing && <ConnectionDialog key={editing.id} initial={editing} />}
      <ConfirmDialogs />
      <CommandPalette />
      <AiConnect />
      <Toasts />
    </div>
  );
}
