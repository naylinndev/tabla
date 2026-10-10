import { Command } from "cmdk";
import { ArrowDownUp, CornerDownLeft, Code2, Eye, Home, Moon, Plug, Plus, Power, Sun, Table2 } from "lucide-react";
import { newConnection, useActiveSession, useApp } from "../lib/store";
import { DriverIcon } from "./icons";
import { Modal } from "./ui";

export function CommandPalette() {
  const open = useApp((s) => s.paletteOpen);
  const setOpen = useApp((s) => s.setPalette);
  const connections = useApp((s) => s.connections);
  const sessions = useApp((s) => s.sessions);
  const session = useActiveSession();
  const st = useApp.getState;

  if (!open) return null;
  const run = (fn: () => void) => {
    setOpen(false);
    fn();
  };

  return (
    <Modal onClose={() => setOpen(false)} className="palette">
      <Command label="Command palette" loop>
        <Command.Input autoFocus placeholder={session ? `Search tables in ${session.schema}, actions, connections…` : "Search connections and actions…"} />
        <Command.List>
          <Command.Empty>Nothing found.</Command.Empty>
          {session && session.tables.length > 0 && (
            <Command.Group heading="Tables">
              {session.tables.map((t) => (
                <Command.Item
                  key={t.name}
                  value={`table ${t.name}`}
                  onSelect={() => run(() => st().openTable(session.id, session.schema, t.name))}
                >
                  {t.kind === "table" ? <Table2 /> : <Eye />}
                  {t.name}
                  <span className="hint-r">{session.schema}</span>
                </Command.Item>
              ))}
            </Command.Group>
          )}
          <Command.Group heading="Actions">
            {session && (
              <>
                <Command.Item value="new query sql editor" onSelect={() => run(() => st().newQuery(session.id))}>
                  <Code2 /> New query
                </Command.Item>
                <Command.Item value="refresh tables reload" onSelect={() => run(() => st().refreshTables(session.id))}>
                  <ArrowDownUp /> Refresh table list
                </Command.Item>
                <Command.Item value="disconnect close connection" onSelect={() => run(() => st().closeSession(session.id))}>
                  <Power /> Disconnect {session.connection.name}
                </Command.Item>
              </>
            )}
            <Command.Item value="new connection add" onSelect={() => run(() => st().setEditing(newConnection()))}>
              <Plus /> New connection
            </Command.Item>
            <Command.Item value="home connections" onSelect={() => run(() => st().setActiveSession(null))}>
              <Home /> Go to connections
            </Command.Item>
            <Command.Item value="theme dark mode" onSelect={() => run(() => st().setTheme("dark"))}>
              <Moon /> Dark theme
            </Command.Item>
            <Command.Item value="theme light mode" onSelect={() => run(() => st().setTheme("light"))}>
              <Sun /> Light theme
            </Command.Item>
          </Command.Group>
          {sessions.length > 0 && (
            <Command.Group heading="Open connections">
              {sessions.map((s) => (
                <Command.Item key={s.id} value={`switch ${s.connection.name}`} onSelect={() => run(() => st().setActiveSession(s.id))}>
                  <DriverIcon driver={s.connection.driver} provider={s.connection.provider} size={16} />
                  {s.connection.name}
                  <span className="hint-r">switch</span>
                </Command.Item>
              ))}
            </Command.Group>
          )}
          <Command.Group heading="Saved connections">
            {connections.map((c) => (
              <Command.Item key={c.id} value={`connect ${c.name} ${c.host} ${c.database}`} onSelect={() => run(() => st().openConnection(c))}>
                <Plug />
                {c.name}
                <span className="hint-r">connect</span>
              </Command.Item>
            ))}
          </Command.Group>
        </Command.List>
        <div className="palette-foot">
          <span>
            <ArrowDownUp size={11} /> navigate
          </span>
          <span>
            <CornerDownLeft size={11} /> open
          </span>
          <span>esc close</span>
        </div>
      </Command>
    </Modal>
  );
}
