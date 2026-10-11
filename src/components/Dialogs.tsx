import { useEffect, useRef } from "react";
import { ArrowUpCircle, Check, Copy, Monitor, Moon, Power, RefreshCw, Sun, SunMoon, TriangleAlert } from "lucide-react";
import { api, errorMessage } from "../lib/api";
import { Checkbox } from "./controls";
import { useApp, type FontChoice, type Theme } from "../lib/store";
import { Modal } from "./ui";

/** App-wide confirmations: disconnecting, and closing a tab with uncommitted edits. */
export function ConfirmDialogs() {
  const confirm = useApp((s) => s.confirm);
  const sessions = useApp((s) => s.sessions);
  const pending = useApp((s) => s.pending);
  const setConfirm = useApp((s) => s.setConfirm);
  const closeSession = useApp((s) => s.closeSession);
  const closeTab = useApp((s) => s.closeTab);
  if (!confirm) return null;
  const session = sessions.find((s) => s.id === confirm.sessionId);
  if (!session) return null;
  const close = () => setConfirm(null);

  if (confirm.kind === "disconnect") {
    const unsaved = Object.entries(pending)
      .filter(([k]) => k.startsWith(`${session.id}:`))
      .reduce((n, [, v]) => n + v, 0);
    const queryTabs = session.tabs.filter((t) => t.kind === "query").length;
    return (
      <Modal onClose={close}>
        <div className="modal-head">
          {unsaved ? <TriangleAlert color="var(--warn)" /> : <Power color="var(--accent-2)" />}
          <div>
            <h2>Disconnect from {session.connection.name}?</h2>
            <p>
              {unsaved
                ? `You have ${unsaved} uncommitted change${unsaved > 1 ? "s" : ""}. They will be lost.`
                : `${session.tabs.length} open tab${session.tabs.length === 1 ? "" : "s"} will close.${queryTabs ? " Query drafts are kept for next time." : ""}`}
            </p>
          </div>
        </div>
        <div className="modal-foot">
          <div className="spacer" />
          <button className="btn" onClick={close} autoFocus>
            Stay connected
          </button>
          <button
            className={`btn ${unsaved ? "danger solid" : "primary"}`}
            onClick={() => {
              close();
              closeSession(session.id);
            }}
          >
            {unsaved ? "Discard changes and disconnect" : "Disconnect"}
          </button>
        </div>
      </Modal>
    );
  }

  const tab = session.tabs.find((t) => t.id === confirm.tabId);
  const count = pending[`${session.id}:${confirm.tabId}`] ?? 0;
  return (
    <Modal onClose={close}>
      <div className="modal-head">
        <TriangleAlert color="var(--warn)" />
        <div>
          <h2>Close {tab?.kind === "table" ? tab.table : "tab"} without saving?</h2>
          <p>
            {count} uncommitted change{count > 1 ? "s" : ""} will be lost. Commit them first with ⌘S to keep them.
          </p>
        </div>
      </div>
      <div className="modal-foot">
        <div className="spacer" />
        <button className="btn" onClick={close} autoFocus>
          Keep editing
        </button>
        <button
          className="btn danger solid"
          onClick={() => {
            close();
            closeTab(session.id, confirm.tabId, true);
          }}
        >
          Discard and close
        </button>
      </div>
    </Modal>
  );
}

const FONTS: { id: FontChoice; label: string; family: string; note: string }[] = [
  { id: "geist", label: "Geist", family: '"Geist Variable"', note: "Clean and modern (default)" },
  { id: "arimo", label: "Arimo", family: '"Arimo"', note: "Same proportions as Helvetica" },
  { id: "inter", label: "Inter", family: '"Inter Variable"', note: "Compact" },
  { id: "system", label: "System", family: "-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui", note: "San Francisco / Segoe UI" },
];
const THEMES: { id: Theme; label: string; icon: React.ReactNode }[] = [
  { id: "system", label: "Auto", icon: <SunMoon /> },
  { id: "light", label: "Light", icon: <Sun /> },
  { id: "dark", label: "Dark", icon: <Moon /> },
];

export function AppearanceMenu({ onClose }: { onClose(): void }) {
  const theme = useApp((s) => s.theme);
  const font = useApp((s) => s.font);
  const setTheme = useApp((s) => s.setTheme);
  const setFont = useApp((s) => s.setFont);
  const autoCheck = useApp((s) => s.autoUpdateCheck);
  const setAutoCheck = useApp((s) => s.setAutoUpdateCheck);
  const checkNow = useApp((s) => s.checkForUpdates);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const down = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && onClose();
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", down);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("mousedown", down);
      window.removeEventListener("keydown", key);
    };
  }, [onClose]);
  return (
    <div ref={ref} className="appearance">
      <div className="ap-title">
        <Monitor size={13} /> Appearance
      </div>
      <div className="segmented ap-themes">
        {THEMES.map((t) => (
          <button key={t.id} className={theme === t.id ? "on" : ""} onClick={() => setTheme(t.id)}>
            {t.icon} {t.label}
          </button>
        ))}
      </div>
      <div className="ap-title" style={{ marginTop: 12 }}>
        Font
      </div>
      {FONTS.map((f) => (
        <button key={f.id} className={`ap-font ${font === f.id ? "on" : ""}`} onClick={() => setFont(f.id)} style={{ fontFamily: f.family }}>
          <span className="ap-check">{font === f.id && <Check />}</span>
          <span>
            <b>{f.label}</b>
            <small>{f.note}</small>
          </span>
          <span className="ap-sample">Aa 123</span>
        </button>
      ))}
      <div className="ap-title" style={{ marginTop: 12 }}>
        Updates
      </div>
      <div className="ap-updates">
        <Checkbox checked={autoCheck} onChange={setAutoCheck} label="Check for updates daily" />
        <button className="btn ghost" onClick={() => checkNow(true)}>
          <RefreshCw /> Check now
        </button>
      </div>
      <small className="dim ap-note">Asks GitHub for the latest release. Nothing about you or your databases is sent.</small>
    </div>
  );
}

export function UpdateBanner() {
  const update = useApp((s) => s.update);
  const toast = useApp((s) => s.toast);
  if (!update?.available) return null;
  const open = (url: string) => api.openProjectPage(url).catch((e) => toast(errorMessage(e), "error"));
  return (
    <div className="update-banner">
      <ArrowUpCircle />
      <div style={{ flex: 1, minWidth: 0 }}>
        <b>Tabla {update.latest} is available</b>
        <span className="dim"> · you have {update.current}</span>
        <div className="update-brew">
          Homebrew: <code>brew upgrade --cask tabla</code>
          <button className="icon-btn" style={{ width: 22, height: 22 }} title="Copy command" onClick={() => navigator.clipboard.writeText("brew upgrade --cask tabla").then(() => toast("Copied", "success"))}>
            <Copy size={12} />
          </button>
        </div>
      </div>
      <button className="btn ghost" onClick={() => open(update.url)}>
        Release notes
      </button>
      <button className="btn primary" onClick={() => open(update.url)}>
        Download
      </button>
    </div>
  );
}
