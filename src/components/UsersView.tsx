import { useEffect, useMemo, useState } from "react";
import { Bot, KeyRound, Lock, Minus, Plus, RefreshCw, Search, ShieldCheck, Unlock, User, Users } from "lucide-react";
import { api, errorMessage, type Driver, type UserInfo } from "../lib/api";
import { quoteIdent, quoteLiteral } from "../lib/sql";
import type { Session } from "../lib/store";
import { Checkbox, Select } from "./controls";
import { SqlConfirm } from "./SqlConfirm";

type Level = "read" | "write" | "all";
const LEVELS: { value: Level; label: string; hint: string }[] = [
  { value: "read", label: "Read only", hint: "SELECT" },
  { value: "write", label: "Read & write", hint: "SELECT, INSERT, UPDATE, DELETE" },
  { value: "all", label: "Everything", hint: "ALL PRIVILEGES" },
];

const acct = (d: Driver, u: { name: string; host: string }) =>
  d === "mysql" ? `${quoteLiteral(d, u.name)}@${quoteLiteral(d, u.host || "%")}` : quoteIdent(d, u.name);

function grantSql(d: Driver, who: string, scope: string, level: Level, revoke = false): string[] {
  const s = quoteIdent(d, scope);
  const privs = level === "read" ? "SELECT" : level === "write" ? "SELECT, INSERT, UPDATE, DELETE" : "ALL PRIVILEGES";
  // SQL Server grants go to the database user, scoped to a schema.
  if (d === "mssql")
    return revoke
      ? [`REVOKE SELECT, INSERT, UPDATE, DELETE, EXECUTE, CONTROL ON SCHEMA::${s} FROM ${who}`]
      : [`GRANT ${level === "all" ? "CONTROL" : privs} ON SCHEMA::${s} TO ${who}`];
  if (d === "mysql") return revoke ? [`REVOKE ALL PRIVILEGES ON ${s}.* FROM ${who}`] : [`GRANT ${privs} ON ${s}.* TO ${who}`];
  const pgPrivs = level === "all" ? "ALL" : privs;
  if (revoke)
    return [
      `REVOKE ALL ON ALL TABLES IN SCHEMA ${s} FROM ${who}`,
      `REVOKE ALL ON ALL SEQUENCES IN SCHEMA ${s} FROM ${who}`,
      `ALTER DEFAULT PRIVILEGES IN SCHEMA ${s} REVOKE ALL ON TABLES FROM ${who}`,
      `REVOKE ALL ON SCHEMA ${s} FROM ${who}`,
    ];
  return [
    `GRANT USAGE ON SCHEMA ${s} TO ${who}`,
    `GRANT ${pgPrivs} ON ALL TABLES IN SCHEMA ${s} TO ${who}`,
    ...(level !== "read" ? [`GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA ${s} TO ${who}`] : []),
    // Tables created later get the same rights.
    `ALTER DEFAULT PRIVILEGES IN SCHEMA ${s} GRANT ${pgPrivs} ON TABLES TO ${who}`,
  ];
}

const MASK = "'•••'";

function passwordSql(d: Driver, who: string, secret: string, contained: boolean): string {
  if (d === "mysql") return `ALTER USER ${who} IDENTIFIED BY ${secret}`;
  if (d === "mssql") return contained ? `ALTER USER ${who} WITH PASSWORD = ${secret}` : `ALTER LOGIN ${who} WITH PASSWORD = ${secret}`;
  return `ALTER ROLE ${who} PASSWORD ${secret}`;
}

interface Pending {
  title: string;
  statements: string[];
  display?: string[];
  danger?: boolean;
  label: string;
  description?: string;
}

export function UsersView({ session, active }: { session: Session; active: boolean }) {
  const d = session.connection.driver;
  const [users, setUsers] = useState<UserInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [rev, setRev] = useState(0);
  const [q, setQ] = useState("");
  const [selKey, setSelKey] = useState<string | null>(null);
  const [tab, setTab] = useState<"general" | "privileges">("general");
  const [grants, setGrants] = useState<string[] | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [creating, setCreating] = useState<null | { preset: "blank" | "readonly" }>(null);
  const [newPw, setNewPw] = useState("");
  const [scope, setScope] = useState(session.schema);
  const [level, setLevel] = useState<Level>("read");
  const ro = session.connection.readOnly;
  // Azure SQL Database uses contained users (no server logins).
  const contained = session.connection.provider === "azuresql";

  const keyOf = (u: UserInfo) => `${u.name}@${u.host}`;
  useEffect(() => {
    if (!active) return;
    setError(null);
    api.listUsers(session.id).then(
      (list) => {
        setUsers(list);
        setSelKey((k) => (k && list.some((u) => keyOf(u) === k) ? k : list[0] ? keyOf(list[0]) : null));
      },
      (e) => setError(errorMessage(e)),
    );
  }, [session.id, rev, active]);

  const sel = users?.find((u) => keyOf(u) === selKey) ?? null;
  useEffect(() => {
    setGrants(null);
    setNewPw("");
    if (sel) api.userGrants(session.id, sel.name, sel.host).then(setGrants, (e) => setGrants([`Couldn't read privileges: ${errorMessage(e)}`]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selKey, rev]);

  const shown = useMemo(() => (users ?? []).filter((u) => !q || u.name.toLowerCase().includes(q.toLowerCase())), [users, q]);
  const reload = () => setRev((r) => r + 1);

  if (d === "sqlite")
    return (
      <div className="q-message">
        <div className="card dim">
          <Users />
          SQLite databases are files without user accounts. Control access with file permissions.
        </div>
      </div>
    );

  const who = sel ? acct(d, sel) : "";
  const attr = (flag: string, on: boolean) => setPending({ title: `${on ? "Grant" : "Remove"} ${flag}`, statements: [`ALTER ROLE ${who} ${on ? "" : "NO"}${flag.replace(/ /g, "")}`], label: "Apply" });

  return (
    <div className="view users" hidden={!active}>
      <div className="users-split">
        <aside className="users-list">
          <div className="users-list-head">
            <b>Accounts</b>
            <span className="dim">{users?.length ?? ""}</span>
          </div>
          <div className="sb-search" style={{ margin: "0 10px 8px" }}>
            <Search />
            <input className="input" placeholder="Filter" value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} />
          </div>
          <div className="users-items">
            {error && <div className="q-error">{error}</div>}
            {!users && !error && <div className="sb-empty"><span className="spinner" /></div>}
            {shown.map((u) => (
              <button key={keyOf(u)} className={`sb-item ${keyOf(u) === selKey ? "active" : ""}`} onClick={() => setSelKey(keyOf(u))}>
                {u.canLogin ? <User /> : <Users />}
                <span className="name">
                  {u.name}
                  {u.host && <span className="dim">@{u.host}</span>}
                </span>
                {u.superuser && <span className="badge production">admin</span>}
                {u.locked && <Lock size={12} />}
              </button>
            ))}
          </div>
          <div className="sg-bar">
            <button className="icon-btn" title="New account" disabled={ro} onClick={() => setCreating({ preset: "blank" })}>
              <Plus />
            </button>
            <button
              className="icon-btn"
              title="Delete account"
              disabled={ro || !sel}
              onClick={() =>
                sel &&
                setPending({
                  title: `Delete ${sel.name}`,
                  description:
                    d === "mysql" || d === "mssql"
                      ? "The account can no longer sign in."
                      : "Anything it owns is handed to you first, then its privileges are removed and the role is deleted.",
                  statements:
                    d === "mysql"
                      ? [`DROP USER ${who}`]
                      : d === "mssql"
                        ? [`DROP USER IF EXISTS ${who}`, ...(contained ? [] : [`DROP LOGIN ${who}`])]
                        : [`REASSIGN OWNED BY ${who} TO CURRENT_USER`, `DROP OWNED BY ${who}`, `DROP ROLE ${who}`],
                  danger: true,
                  label: "Delete account",
                })
              }
            >
              <Minus />
            </button>
            <button className="icon-btn" title="Refresh" onClick={reload}>
              <RefreshCw />
            </button>
            <div className="spacer" />
            <button className="btn ghost" disabled={ro} onClick={() => setCreating({ preset: "readonly" })} title="Create a SELECT-only account, ideal for AI assistants and dashboards">
              <Bot /> Read-only user
            </button>
          </div>
        </aside>

        <section className="users-detail">
          {!sel ? (
            <div className="q-message dim">Select an account</div>
          ) : (
            <>
              <div className="users-detail-head">
                <div className="ai-badge" style={{ width: 36, height: 36 }}>
                  {sel.canLogin ? <User /> : <Users />}
                </div>
                <div style={{ minWidth: 0 }}>
                  <h2 className="ellipsis">
                    {sel.name}
                    {sel.host && <span className="dim">@{sel.host}</span>}
                  </h2>
                  <div className="users-badges">
                    {sel.canLogin ? <span className="badge local">can sign in</span> : <span className="badge">group role</span>}
                    {sel.superuser && <span className="badge production">{d === "mysql" ? "super / grant" : d === "mssql" ? (contained ? "db_owner" : "sysadmin") : "superuser"}</span>}
                    {sel.locked && <span className="badge staging">locked</span>}
                    {sel.validUntil && <span className="badge">{sel.validUntil}</span>}
                    {sel.memberOf.map((m) => (
                      <span key={m} className="badge development">
                        {m}
                      </span>
                    ))}
                  </div>
                </div>
              </div>
              <div className="dialog-tabs" style={{ padding: 0 }}>
                {(["general", "privileges"] as const).map((t) => (
                  <button key={t} className={tab === t ? "on" : ""} onClick={() => setTab(t)}>
                    {t === "general" ? "General" : "Privileges"}
                  </button>
                ))}
              </div>

              {tab === "general" ? (
                <div className="users-body">
                  <div className="field">
                    <label>New password</label>
                    <div className="url-row">
                      <input className="input mono" type="password" autoComplete="new-password" value={newPw} onChange={(e) => setNewPw(e.target.value)} placeholder="Leave empty to keep the current one" disabled={ro} />
                      <button
                        className="btn primary"
                        disabled={ro || !newPw}
                        onClick={() =>
                          setPending({
                            title: `Change password for ${sel.name}`,
                            statements: [passwordSql(d, who, quoteLiteral(d, newPw), contained)],
                            display: [passwordSql(d, who, MASK, contained)],
                            label: "Change password",
                          })
                        }
                      >
                        <KeyRound /> Change
                      </button>
                    </div>
                    <span className="hint">
                      <ShieldCheck /> Sent once over the connection. It's never stored by Tabla and is masked in the Console.
                    </span>
                  </div>
                  {d === "postgres" ? (
                    <div className="users-attrs">
                      <Checkbox checked={sel.canLogin} disabled={ro} onChange={(v) => attr("LOGIN", v)} label="Can sign in (LOGIN)" />
                      <Checkbox checked={sel.createDb} disabled={ro} onChange={(v) => attr("CREATEDB", v)} label="Can create databases" />
                      <Checkbox checked={sel.createRole} disabled={ro} onChange={(v) => attr("CREATEROLE", v)} label="Can create roles" />
                      <Checkbox checked={sel.superuser} disabled={ro} onChange={(v) => attr("SUPERUSER", v)} label="Superuser (bypasses every permission)" />
                    </div>
                  ) : d === "mssql" ? (
                    <div className="users-attrs">
                      {!contained && (
                        <button
                          className="btn"
                          disabled={ro}
                          onClick={() =>
                            setPending({
                              title: `${sel.canLogin ? "Disable" : "Enable"} ${sel.name}`,
                              statements: [`ALTER LOGIN ${who} ${sel.canLogin ? "DISABLE" : "ENABLE"}`],
                              label: sel.canLogin ? "Disable" : "Enable",
                            })
                          }
                        >
                          {sel.canLogin ? <Lock /> : <Unlock />} {sel.canLogin ? "Disable login" : "Enable login"}
                        </button>
                      )}
                    </div>
                  ) : (
                    <div className="users-attrs">
                      <button
                        className="btn"
                        disabled={ro}
                        onClick={() =>
                          setPending({
                            title: `${sel.locked ? "Unlock" : "Lock"} ${sel.name}`,
                            statements: [`ALTER USER ${who} ACCOUNT ${sel.locked ? "UNLOCK" : "LOCK"}`],
                            label: sel.locked ? "Unlock" : "Lock",
                          })
                        }
                      >
                        {sel.locked ? <Unlock /> : <Lock />} {sel.locked ? "Unlock account" : "Lock account"}
                      </button>
                    </div>
                  )}
                </div>
              ) : (
                <div className="users-body">
                  <div className="grant-row">
                    <div className="field" style={{ flex: 1 }}>
                      <label>{d === "mysql" ? "Database" : "Schema"}</label>
                      <Select className="w-full" mono value={scope} options={session.schemas.map((s) => ({ value: s, label: s }))} onChange={(v) => setScope(String(v))} />
                    </div>
                    <div className="field" style={{ flex: 1 }}>
                      <label>Access</label>
                      <Select className="w-full" value={level} options={LEVELS} onChange={(v) => setLevel(v as Level)} />
                    </div>
                    <button className="btn primary" disabled={ro} onClick={() => setPending({ title: `Grant ${LEVELS.find((l) => l.value === level)!.label.toLowerCase()} on ${scope}`, statements: grantSql(d, who, scope, level), label: "Grant" })}>
                      Grant
                    </button>
                    <button className="btn" disabled={ro} onClick={() => setPending({ title: `Revoke access to ${scope}`, statements: grantSql(d, who, scope, level, true), danger: true, label: "Revoke" })}>
                      Revoke all
                    </button>
                  </div>
                  <div className="sg-section">Current privileges</div>
                  <div className="code-block">
                    <pre className="grants">{grants === null ? "Loading…" : grants.length ? grants.join("\n") : "No privileges."}</pre>
                  </div>
                </div>
              )}
            </>
          )}
        </section>
      </div>

      {creating && <CreateUser session={session} preset={creating.preset} onClose={() => setCreating(null)} onDone={reload} />}
      {pending && (
        <SqlConfirm
          session={session}
          title={pending.title}
          description={pending.description}
          statements={pending.statements}
          displayStatements={pending.display}
          danger={pending.danger}
          confirmLabel={pending.label}
          onDone={reload}
          onClose={() => setPending(null)}
        />
      )}
    </div>
  );
}

function CreateUser({ session, preset, onClose, onDone }: { session: Session; preset: "blank" | "readonly"; onClose(): void; onDone(): void }) {
  const d = session.connection.driver;
  const [name, setName] = useState(preset === "readonly" ? "tabla_readonly" : "");
  const [host, setHost] = useState("%");
  const [pw, setPw] = useState("");
  const [scope, setScope] = useState(session.schema);
  const [level, setLevel] = useState<Level | "none">(preset === "readonly" ? "read" : "none");
  const u = { name: name.trim(), host };
  const who = acct(d, u);
  const contained = session.connection.provider === "azuresql";
  const create = (secret: string) =>
    d === "mysql"
      ? `CREATE USER ${who} IDENTIFIED BY ${secret}`
      : d === "mssql"
        ? contained
          ? `CREATE USER ${who} WITH PASSWORD = ${secret}`
          : `CREATE LOGIN ${who} WITH PASSWORD = ${secret}`
        : `CREATE ROLE ${who} LOGIN PASSWORD ${secret}`;
  const grants = level === "none" ? [] : grantSql(d, who, scope, level);
  const ok = !!u.name && pw.length >= 8;
  const db =
    d === "postgres" && session.connection.database
      ? [`GRANT CONNECT ON DATABASE ${quoteIdent(d, session.connection.database)} TO ${who}`]
      : d === "mssql" && !contained
        ? [`CREATE USER ${who} FOR LOGIN ${who}`]
        : [];

  return (
    <SqlConfirm
      session={session}
      title={preset === "readonly" ? "Create a read-only user" : "Create account"}
      description={preset === "readonly" ? "A SELECT-only account. Use it for AI assistants, dashboards and analytics so nothing can write." : undefined}
      statements={ok ? [create(quoteLiteral(d, pw)), ...db, ...grants] : []}
      displayStatements={ok ? [create(MASK), ...db, ...grants] : [create(MASK)]}
      confirmLabel="Create account"
      onDone={onDone}
      onClose={onClose}
    >
      <div className="grid-2b">
        <div className="field">
          <label>User name</label>
          <input className="input mono" autoFocus value={name} onChange={(e) => setName(e.target.value)} spellCheck={false} />
        </div>
        {d === "mysql" ? (
          <div className="field">
            <label>Allowed from (host)</label>
            <Select
              className="w-full"
              mono
              value={host}
              options={[
                { value: "%", label: "%", hint: "anywhere" },
                { value: "localhost", label: "localhost", hint: "this server only" },
                { value: "127.0.0.1", label: "127.0.0.1", hint: "loopback / SSH tunnel" },
              ]}
              onChange={(v) => setHost(String(v))}
            />
          </div>
        ) : (
          <div />
        )}
      </div>
      <div className="field">
        <label>Password (at least 8 characters)</label>
        <div className="url-row">
          <input className="input mono" type="password" autoComplete="new-password" value={pw} onChange={(e) => setPw(e.target.value)} />
          <button
            type="button"
            className="btn"
            onClick={() => {
              const a = new Uint8Array(18);
              crypto.getRandomValues(a);
              const p = btoa(String.fromCharCode(...a)).replace(/[+/=]/g, "x");
              setPw(p);
              navigator.clipboard.writeText(p);
            }}
            title="Generate a strong password and copy it"
          >
            <KeyRound /> Generate
          </button>
        </div>
      </div>
      <div className="grid-2b">
        <div className="field">
          <label>Access to {d === "mysql" ? "database" : "schema"}</label>
          <Select className="w-full" mono value={scope} options={session.schemas.map((s) => ({ value: s, label: s }))} onChange={(v) => setScope(String(v))} />
        </div>
        <div className="field">
          <label>Level</label>
          <Select className="w-full" value={level} options={[{ value: "none", label: "No access yet", hint: "" }, ...LEVELS]} onChange={(v) => setLevel(v as Level | "none")} />
        </div>
      </div>
    </SqlConfirm>
  );
}
