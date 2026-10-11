import { useMemo, useState } from "react";
import {
  Database,
  FolderOpen,
  KeyRound,
  Lock,
  Pencil,
  Plus,
  Search,
  ShieldCheck,
  Trash2,
  Zap,
  Command,
} from "lucide-react";
import { api, errorMessage, isLocal, type ConnectionConfig, type EnvUrl } from "../lib/api";
import { parseUrl } from "./ConnectionDialog";
import { detectProvider } from "../lib/providers";
import { newConnection, useApp } from "../lib/store";
import { DriverIcon, Logo } from "./icons";
import { providerById } from "../lib/providers";
import { Bot, FileSearch, Heart } from "lucide-react";
import { SupportDialog } from "./Support";
import { UpdateBanner } from "./Dialogs";
import { Kbd, Modal, TagBadge } from "./ui";

/** Hides the password part of a URL for display. */
function maskUrl(u: string) {
  return u.replace(/(\/\/[^:/@]+:)([^@]+)(@)/, "$1••••••$3");
}

function address(c: ConnectionConfig) {
  if (isLocal(c.driver)) return c.database;
  return `${c.user ? c.user + "@" : ""}${c.host}:${c.port}${c.database ? "/" + c.database : ""}`;
}

export function Home() {
  const connections = useApp((s) => s.connections);
  const connecting = useApp((s) => s.connecting);
  const sessions = useApp((s) => s.sessions);
  const openIds = useMemo(() => new Set(sessions.map((s) => s.connection.id)), [sessions]);
  const open = useApp((s) => s.openConnection);
  const setEditing = useApp((s) => s.setEditing);
  const save = useApp((s) => s.saveConnection);
  const remove = useApp((s) => s.deleteConnection);
  const toast = useApp((s) => s.toast);
  const [q, setQ] = useState("");
  const [confirmDelete, setConfirmDelete] = useState<ConnectionConfig | null>(null);
  const [envFound, setEnvFound] = useState<EnvUrl[] | null>(null);
  const [support, setSupport] = useState(false);
  const update = useApp((s) => s.update);
  const setAiOpen = useApp((s) => s.setAiOpen);

  const importEnv = async () => {
    try {
      const found = await api.scanEnvFiles();
      if (found === null) return;
      if (!found.length) return toast("No database URLs found in that folder's .env files.", "info");
      setEnvFound(found);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  const useEnvUrl = (e: EnvUrl) => {
    const parsed = parseUrl(e.url.replace(/^file:/, "sqlite://"));
    if (!parsed) return toast(`Couldn't read ${e.key}`, "error");
    const driver = parsed.driver!;
    const provider = detectProvider(parsed.host ?? "", driver);
    setEnvFound(null);
    setEditing({
      ...newConnection(),
      ...parsed,
      driver,
      provider,
      sslMode: parsed.sslMode ?? providerById(provider, driver).sslMode,
      name: `${parsed.database || parsed.host || "Database"} (${e.key})`,
      color: providerById(provider, driver).from,
    });
  };

  const list = useMemo(
    () =>
      connections
        // Same order as the left rail, so Mod+1…9 match what you see here.
        .filter((c) => !q || `${c.name} ${address(c)} ${c.tag}`.toLowerCase().includes(q.toLowerCase())),
    [connections, q],
  );

  const openSqlite = async () => {
    try {
      const path = await api.pickSqliteFile();
      if (!path) return;
      const existing = connections.find((c) => c.driver === "sqlite" && c.database === path);
      if (existing) return open(existing);
      const c: ConnectionConfig = {
        ...newConnection(),
        driver: "sqlite",
        provider: "sqlite",
        name: path.split(/[\\/]/).pop() ?? "SQLite",
        database: path,
        port: 0,
        host: "",
        password: undefined,
        color: "#5ec8f2",
      };
      const saved = await save(c);
      await open(saved);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  return (
    <div className="home">
      <div className="titlebar-space home-drag" data-tauri-drag-region />
      <div className="home-inner">
        {update?.available && <UpdateBanner />}
        <div className="hero">
          <Logo size={46} />
          <div>
            <h1>Tabla</h1>
            <p>A fast, open-source home for your databases.</p>
          </div>
        </div>

        <div className="home-bar">
          <div className="sb-search">
            <Search />
            <input
              className="input"
              placeholder="Search connections"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              spellCheck={false}
            />
          </div>
          <div className="spacer" />
          <button className="btn lg" onClick={() => setAiOpen(true)} title="Let Claude, Cursor and other AI tools read your databases">
            <Bot /> Connect AI
          </button>
          <button className="btn lg" onClick={importEnv} title="Find DATABASE_URL and similar in a project's .env files">
            <FileSearch /> Import from .env
          </button>
          <button className="btn lg" onClick={openSqlite}>
            <FolderOpen /> Open SQLite
          </button>
          <button className="btn primary lg" onClick={() => setEditing(newConnection())}>
            <Plus /> New connection
          </button>
        </div>

        {connections.length === 0 ? (
          <div className="home-empty">
            <Database size={28} className="dim" />
            <h2>Connect your first database</h2>
            <p>PostgreSQL, MySQL, MariaDB, SQL Server, Oracle, ClickHouse, SQLite, DuckDB, Redis and hosted services like Supabase, Neon, PlanetScale and CockroachDB. Passwords are kept in your system keychain, never in files.</p>
            <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
              <button className="btn primary lg" onClick={() => setEditing(newConnection())}>
                <Plus /> New connection
              </button>
              <button className="btn lg" onClick={importEnv}>
                <FileSearch /> Import from a project's .env
              </button>
              <button className="btn lg" onClick={openSqlite}>
                <FolderOpen /> Open SQLite file
              </button>
            </div>
          </div>
        ) : (
          <div className="conn-grid">
            {list.map((c) => (
              <div
                key={c.id}
                role="button"
                tabIndex={0}
                className="conn-card"
                style={{ "--c": c.color } as React.CSSProperties}
                onClick={() => open(c)}
                onKeyDown={(e) => e.key === "Enter" && open(c)}
              >
                <div className="top">
                  <DriverIcon driver={c.driver} provider={c.provider} size={34} />
                  <div style={{ minWidth: 0, flex: 1 }}>
                    <div className="name">{c.name}</div>
                    <div className="addr">{address(c)}</div>
                  </div>
                </div>
                <div className="foot">
                  {openIds.has(c.id) && (
                    <span className="badge local">
                      <span className="live-dot" /> connected
                    </span>
                  )}
                  <span className="badge">{providerById(c.provider, c.driver).label}</span>
                  <TagBadge tag={c.tag} />
                  {c.readOnly && (
                    <span className="badge" title="Read-only">
                      <Lock />
                    </span>
                  )}
                  <div className="acts">
                    <button
                      className="icon-btn"
                      title="Edit"
                      onClick={(e) => {
                        e.stopPropagation();
                        setEditing({ ...c, password: undefined });
                      }}
                    >
                      <Pencil />
                    </button>
                    <button
                      className="icon-btn"
                      title="Delete"
                      onClick={(e) => {
                        e.stopPropagation();
                        setConfirmDelete(c);
                      }}
                    >
                      <Trash2 />
                    </button>
                  </div>
                </div>
                {connecting === c.id && (
                  <div className="connecting">
                    <span className="spinner" />
                  </div>
                )}
              </div>
            ))}
            <button className="conn-card new" onClick={() => setEditing(newConnection())}>
              <Plus />
              New connection
            </button>
          </div>
        )}

        <div className="feature-row">
          <div className="feature">
            <Zap />
            <b>Native and fast</b>
            <span>Rust core, virtualized grids, tiny memory footprint.</span>
          </div>
          <div className="feature">
            <ShieldCheck />
            <b>Safe by default</b>
            <span>Keychain passwords, encrypted local data, read-only mode, production guardrails.</span>
          </div>
          <div className="feature">
            <Command />
            <b>Keyboard first</b>
            <span>
              Press <Kbd keys="Mod K" /> anywhere to jump to any table or action.
            </span>
          </div>
        </div>
      </div>

      <div className="home-foot">
        <span className="dim">Tabla is free and open source (MIT).</span>
        <button className="btn ghost" onClick={() => setSupport(true)}>
          <Heart /> Support Tabla
        </button>
      </div>
      {support && <SupportDialog onClose={() => setSupport(false)} />}

      {envFound && (
        <Modal onClose={() => setEnvFound(null)}>
          <div className="modal-head">
            <FileSearch color="var(--accent-2)" />
            <div>
              <h2>Found {envFound.length} database URL{envFound.length > 1 ? "s" : ""}</h2>
              <p>Pick one to review and save. The password goes to your keychain, not to Tabla's files.</p>
            </div>
          </div>
          <div className="modal-body">
            {envFound.map((e, i) => {
              const parsed = parseUrl(e.url.replace(/^file:/, "sqlite://"));
              const provider = parsed ? providerById(detectProvider(parsed.host ?? "", parsed.driver!), parsed.driver!) : null;
              return (
                <button key={i} className="env-item" onClick={() => useEnvUrl(e)} disabled={!parsed}>
                  {provider && <DriverIcon driver={provider.driver} provider={provider.id} size={30} />}
                  <div style={{ minWidth: 0, flex: 1 }}>
                    <div className="env-key">
                      {e.key} <span className="dim">in {e.file}</span>
                    </div>
                    <div className="env-url">{maskUrl(e.url)}</div>
                  </div>
                  {provider && <span className="badge">{provider.label}</span>}
                </button>
              );
            })}
          </div>
        </Modal>
      )}

      {confirmDelete && (
        <Modal onClose={() => setConfirmDelete(null)}>
          <div className="modal-head">
            <Trash2 color="var(--danger)" />
            <div>
              <h2>Delete “{confirmDelete.name}”?</h2>
              <p>
                <KeyRound size={11} style={{ verticalAlign: -1 }} /> The saved profile and its keychain password will be
                removed. Your database is not touched.
              </p>
            </div>
          </div>
          <div className="modal-foot">
            <div className="spacer" />
            <button className="btn" onClick={() => setConfirmDelete(null)}>
              Cancel
            </button>
            <button
              className="btn danger solid"
              autoFocus
              onClick={async () => {
                const c = confirmDelete;
                setConfirmDelete(null);
                try {
                  await remove(c.id);
                  toast(`Deleted ${c.name}`, "success");
                } catch (e) {
                  toast(errorMessage(e), "error");
                }
              }}
            >
              Delete
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
