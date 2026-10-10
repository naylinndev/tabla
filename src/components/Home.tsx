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
import { api, errorMessage, type ConnectionConfig } from "../lib/api";
import { newConnection, useApp } from "../lib/store";
import { DriverIcon, Logo } from "./icons";
import { providerById } from "../lib/providers";
import { Kbd, Modal, TagBadge } from "./ui";

function address(c: ConnectionConfig) {
  if (c.driver === "sqlite") return c.database;
  return `${c.user ? c.user + "@" : ""}${c.host}:${c.port}${c.database ? "/" + c.database : ""}`;
}

export function Home() {
  const connections = useApp((s) => s.connections);
  const connecting = useApp((s) => s.connecting);
  const open = useApp((s) => s.openConnection);
  const setEditing = useApp((s) => s.setEditing);
  const save = useApp((s) => s.saveConnection);
  const remove = useApp((s) => s.deleteConnection);
  const toast = useApp((s) => s.toast);
  const [q, setQ] = useState("");
  const [confirmDelete, setConfirmDelete] = useState<ConnectionConfig | null>(null);

  const list = useMemo(
    () =>
      connections
        .filter((c) => !q || `${c.name} ${address(c)} ${c.tag}`.toLowerCase().includes(q.toLowerCase()))
        .sort((a, b) => a.name.localeCompare(b.name)),
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
      <div className="titlebar-space drag" />
      <div className="home-inner">
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
          <button className="btn lg" onClick={openSqlite}>
            <FolderOpen /> Open SQLite file
          </button>
          <button className="btn primary lg" onClick={() => setEditing(newConnection())}>
            <Plus /> New connection
          </button>
        </div>

        {connections.length === 0 ? (
          <div className="home-empty">
            <Database size={28} className="dim" />
            <h2>Connect your first database</h2>
            <p>PostgreSQL, MySQL, MariaDB, SQLite and hosted services like Supabase, Neon, PlanetScale and CockroachDB. Passwords are kept in your system keychain, never in files.</p>
            <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
              <button className="btn primary lg" onClick={() => setEditing(newConnection())}>
                <Plus /> New connection
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
            <span>Keychain passwords, read-only mode, production guardrails.</span>
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
