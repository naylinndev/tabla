import { useRef, useState } from "react";
import { Check, ChevronLeft, ChevronRight, Fingerprint, Network, ShieldCheck, Bot, LifeBuoy, BookOpen, CheckCircle2, ChevronDown, FolderOpen, Info, KeyRound, Link2, Lock, ShieldAlert, XCircle, Zap } from "lucide-react";
import { api, errorMessage, type ConnectionConfig, type Driver, type EnvTag, type SshConfig, type SslMode } from "../lib/api";
import { newConnection as newConnectionDefaults, useApp } from "../lib/store";
import { DriverIcon } from "./icons";
import { PROVIDERS, detectProvider, providerById, type Provider } from "../lib/providers";
import { explainError } from "../lib/errorHelp";
import { Modal, Switch } from "./ui";
import { Select } from "./controls";

const COLORS = ["#7c6cff", "#5b8def", "#22b8cf", "#3ecf8e", "#f5b84b", "#ff8a4c", "#ff5d6c", "#e86bd0", "#8a909d"];
const TAGS: { value: EnvTag; label: string }[] = [
  { value: "local", label: "Local" },
  { value: "development", label: "Development" },
  { value: "staging", label: "Staging" },
  { value: "production", label: "Production" },
];
type DialogTab = "connection" | "ssh" | "ssl" | "options";
const DEFAULT_PORT: Record<Driver, number> = { postgres: 5432, mysql: 3306, sqlite: 0, redis: 6379 };

/** Parses postgres://, mysql:// and sqlite:// URLs into form fields. */
export function parseUrl(raw: string): Partial<ConnectionConfig> | null {
  try {
    const u = new URL(raw.trim());
    const scheme = u.protocol.replace(":", "");
    const driver: Driver | null = /^postgres(ql)?$/.test(scheme)
      ? "postgres"
      : /^(mysql|mariadb)$/.test(scheme)
        ? "mysql"
        : /^sqlite3?$/.test(scheme)
          ? "sqlite"
          : /^rediss?$/.test(scheme)
            ? "redis"
            : null;
    if (!driver) return null;
    if (driver === "sqlite") return { driver, database: decodeURIComponent(u.pathname), host: "", port: 0 };
    if (driver === "redis")
      return {
        driver,
        host: decodeURIComponent(u.hostname) || "localhost",
        port: u.port ? +u.port : 6379,
        user: decodeURIComponent(u.username) === "default" ? "" : decodeURIComponent(u.username),
        password: u.password ? decodeURIComponent(u.password) : undefined,
        database: u.pathname.replace(/^\//, "") || "0",
        sslMode: scheme === "rediss" ? "require" : "disable",
      };
    const ssl = u.searchParams.get("sslmode") ?? u.searchParams.get("ssl-mode");
    const sslMode: SslMode | undefined =
      ssl === "disable" || ssl === "DISABLED"
        ? "disable"
        : ssl === "require" || ssl === "REQUIRED"
          ? "require"
          : ssl === "verify-full" || ssl === "VERIFY_IDENTITY"
            ? "verify-full"
            : undefined;
    return {
      driver,
      host: decodeURIComponent(u.hostname) || "localhost",
      port: u.port ? +u.port : DEFAULT_PORT[driver],
      user: decodeURIComponent(u.username),
      password: u.password ? decodeURIComponent(u.password) : undefined,
      database: decodeURIComponent(u.pathname.replace(/^\//, "")),
      ...(sslMode ? { sslMode } : {}),
    };
  } catch {
    return null;
  }
}

export function ConnectionDialog({ initial }: { initial: ConnectionConfig }) {
  const setEditing = useApp((s) => s.setEditing);
  const save = useApp((s) => s.saveConnection);
  const open = useApp((s) => s.openConnection);
  const isNew = !useApp((s) => s.connections.some((c) => c.id === initial.id));

  const [f, setF] = useState<ConnectionConfig>({
    ...newConnectionDefaults(),
    ...initial,
    ssh: { ...newConnectionDefaults().ssh, ...initial.ssh },
    password: undefined,
    sshSecret: undefined,
  });
  // A password can arrive pre-filled from a pasted/imported URL; it is only kept in memory until saved to the keychain.
  const [password, setPassword] = useState(initial.password ?? "");
  const [pwTouched, setPwTouched] = useState(!!initial.password);
  const [url, setUrl] = useState("");
  const [urlError, setUrlError] = useState(false);
  const [test, setTest] = useState<{ ok: boolean; msg: string } | null>(null);
  const [busy, setBusy] = useState<"test" | "save" | null>(null);
  const [guideOpen, setGuideOpen] = useState(isNew);
  const [tab, setTab] = useState<DialogTab>("connection");
  const bodyRef = useRef<HTMLFormElement>(null);
  const [sshSecret, setSshSecret] = useState(initial.sshSecret ?? "");
  const [sshTouched, setSshTouched] = useState(!!initial.sshSecret);
  const provider = providerById(f.provider, f.driver);
  const help = test && !test.ok ? explainError(test.msg, provider) : null;

  const set = <K extends keyof ConnectionConfig>(k: K, v: ConnectionConfig[K]) => {
    setF((x) => ({ ...x, [k]: v }));
    setTest(null);
  };
  const close = () => setEditing(null);

  const payload = (): ConnectionConfig => ({
    ...f,
    name: f.name.trim() || (f.driver === "sqlite" ? f.database.split(/[\\/]/).pop() ?? "SQLite" : f.host || "Untitled"),
    // undefined keeps the keychain value; a typed value replaces it.
    password: pwTouched || isNew ? password : undefined,
    sshSecret: sshTouched || isNew ? sshSecret : undefined,
  });

  const applyUrl = (raw: string) => {
    setUrl(raw);
    if (!raw.trim()) return setUrlError(false);
    const parsed = parseUrl(raw);
    setUrlError(!parsed);
    if (!parsed) return;
    const { password: pw, ...rest } = parsed;
    setF((x) => ({
      ...x,
      ...rest,
      // Prefer the provider the host name reveals (supabase.co, neon.tech…), else keep a matching preset.
      provider:
        detectProvider(rest.host ?? "", rest.driver!) !== rest.driver
          ? detectProvider(rest.host ?? "", rest.driver!)
          : providerById(x.provider, x.driver).driver === rest.driver
            ? x.provider
            : rest.driver!,
      name: x.name || (rest.driver === "redis" ? rest.host : rest.database || rest.host) || "",
    }));
    if (pw !== undefined) {
      setPassword(pw);
      setPwTouched(true);
    }
  };

  const runTest = async () => {
    setBusy("test");
    setTest(null);
    try {
      const version = await api.testConnection(payload());
      setTest({ ok: true, msg: `Connected · ${provider.label} ${version.split(" ")[0]}` });
    } catch (e) {
      setTest({ ok: false, msg: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  const submit = async (connect: boolean) => {
    setBusy("save");
    try {
      const saved = await save(payload());
      close();
      if (connect) await open(saved);
    } catch (e) {
      setTest({ ok: false, msg: errorMessage(e) });
      setBusy(null);
    }
  };

  const pickFile = async () => {
    const path = await api.pickSqliteFile();
    if (path) {
      set("database", path);
      if (!f.name) set("name", path.split(/[\\/]/).pop() ?? "");
    }
  };

  const tabs: { id: DialogTab; label: string; on?: boolean }[] =
    f.driver === "sqlite"
      ? [
          { id: "connection", label: "Connection" },
          { id: "options", label: "Options" },
        ]
      : [
          { id: "connection", label: "Connection" },
          { id: "ssh", label: "SSH tunnel", on: f.ssh.enabled },
          { id: "ssl", label: "SSL", on: f.sslMode === "require" || f.sslMode === "verify-full" || !!f.sslCa },
          { id: "options", label: "Options", on: f.readOnly || f.aiAccess || f.tag === "production" },
        ];
  const step = Math.max(0, tabs.findIndex((t) => t.id === tab));
  const lastStep = step === tabs.length - 1;
  const goTo = (id: DialogTab) => {
    setTab(id);
    bodyRef.current?.scrollTo({ top: 0 });
  };
  const next = () => {
    // Light validation so a step can't be skipped with the essentials missing.
    if (tab === "connection") {
      if (f.driver === "sqlite" ? !f.database.trim() : !f.host.trim())
        return setTest({ ok: false, msg: f.driver === "sqlite" ? "Choose a database file first" : "Enter the host first" });
    }
    if (tab === "ssh" && f.ssh.enabled && !f.ssh.host.trim()) return setTest({ ok: false, msg: "Enter the SSH host, or turn the tunnel off" });
    setTest(null);
    goTo(tabs[Math.min(tabs.length - 1, step + 1)].id);
  };
  const back = () => goTo(tabs[Math.max(0, step - 1)].id);
  const setSsh = <K extends keyof SshConfig>(k: K, v: SshConfig[K]) => {
    setF((x) => ({ ...x, ssh: { ...x.ssh, [k]: v } }));
    setTest(null);
  };
  const browse = async (kind: "ssh-key" | "cert", apply: (p: string) => void) => {
    try {
      const p = await api.pickFile(kind);
      if (p) apply(p);
    } catch (e) {
      setTest({ ok: false, msg: errorMessage(e) });
    }
  };

  return (
    <Modal onClose={close} className="modal wide">
      <div className="modal-head">
        <DriverIcon driver={f.driver} provider={f.provider} size={38} />
        <div>
          <h2>{isNew ? "New connection" : `Edit ${initial.name}`}</h2>
          <p>Passwords and SSH secrets are stored in your system keychain, never on disk.</p>
        </div>
      </div>
      <div className={`dialog-tabs ${isNew ? "wizard" : ""}`} role="tablist">
        {tabs.map((t, i) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            className={`${tab === t.id ? "on" : ""} ${isNew && i < step ? "done" : ""}`}
            onClick={() => goTo(t.id)}
          >
            {isNew && <span className="step-num">{i < step ? <Check /> : i + 1}</span>}
            {t.label}
            {t.on && <span className="tab-dot" />}
          </button>
        ))}
      </div>
      <form
        ref={bodyRef}
        className="modal-body"
        onSubmit={(e) => {
          e.preventDefault();
          if (isNew && !lastStep) next();
          else submit(true);
        }}
      >
        {tab === "connection" && (
          <>
        <ProviderPicker
          value={provider.id}
          onChange={(p) => {
            setF((x) => {
              const prev = providerById(x.provider, x.driver);
              // Replace values that were just the previous preset's defaults.
              const keep = (cur: string, prevDefault?: string) => (cur && cur !== (prevDefault ?? "") ? cur : "");
              return {
                ...x,
                provider: p.id,
                driver: p.driver,
                port: p.port,
                sslMode: p.sslMode,
                host: p.driver === "sqlite" ? "" : keep(x.host, prev.hostPlaceholder === "localhost" ? "localhost" : "") || (p.hostPlaceholder === "localhost" ? "localhost" : ""),
                user: keep(x.user, prev.user) || p.user || "",
                database: p.driver === "sqlite" || prev.driver === "sqlite" ? "" : keep(x.database, prev.database) || p.database || "",
              };
            });
            setTest(null);
            setGuideOpen(true);
          }}
        />

        <div className={`guide ${guideOpen ? "open" : ""}`}>
          <button type="button" className="guide-head" onClick={() => setGuideOpen(!guideOpen)}>
            <BookOpen />
            How to connect to {provider.label}
            <ChevronDown className="chev" />
          </button>
          {guideOpen && (
            <div className="guide-body">
              <ol>
                {provider.steps.map((step, i) => (
                  <li key={i}>{step}</li>
                ))}
              </ol>
              {provider.note && (
                <div className="hint">
                  <Info /> {provider.note}
                </div>
              )}
            </div>
          )}
        </div>

        {provider.id === "rds" && (
          <div className="field">
            <label>Engine</label>
            <div className="segmented" style={{ alignSelf: "flex-start" }}>
              {(["postgres", "mysql"] as Driver[]).map((d) => (
                <button
                  type="button"
                  key={d}
                  className={f.driver === d ? "on" : ""}
                  onClick={() => setF((x) => ({ ...x, driver: d, port: DEFAULT_PORT[d] }))}
                >
                  {d === "postgres" ? "PostgreSQL / Aurora PostgreSQL" : "MySQL / Aurora MySQL"}
                </button>
              ))}
            </div>
          </div>
        )}

        {f.driver !== "sqlite" && (
          <div className="field">
            <label>Import from URL</label>
            <div className="url-row">
              <input
                className="input mono"
                placeholder={provider.urlExample}
                value={url}
                onChange={(e) => applyUrl(e.target.value)}
                spellCheck={false}
                autoComplete="off"
                style={urlError ? { borderColor: "var(--danger)" } : undefined}
              />
            </div>
            <span className="hint">
              <Link2 /> Pasting a URL fills in the fields below. It is not saved as-is.
            </span>
          </div>
        )}

        <div className="field">
          <label>Name</label>
          <input
            className="input"
            placeholder="My database"
            value={f.name}
            onChange={(e) => set("name", e.target.value)}
          />
        </div>

        {f.driver === "sqlite" ? (
          <div className="field">
            <label>Database file</label>
            <div className="url-row">
              <input
                className="input mono"
                placeholder="/path/to/database.sqlite"
                value={f.database}
                onChange={(e) => set("database", e.target.value)}
                spellCheck={false}
              />
              <button type="button" className="btn" onClick={pickFile}>
                <FolderOpen /> Browse
              </button>
            </div>
          </div>
        ) : (
          <>
            <div className="grid-2">
              <div className="field">
                <label>Host</label>
                <input
                  className="input mono"
                  placeholder={provider.hostPlaceholder}
                  value={f.host}
                  onChange={(e) => set("host", e.target.value)}
                  spellCheck={false}
                />
              </div>
              <div className="field">
                <label>Port</label>
                <input
                  className="input mono"
                  inputMode="numeric"
                  value={f.port || ""}
                  onChange={(e) => set("port", +e.target.value.replace(/\D/g, "") || 0)}
                />
              </div>
            </div>
            <div className="grid-2b">
              <div className="field">
                <label>{f.driver === "redis" ? "User (ACL, optional)" : "User"}</label>
                <input
                  className="input mono"
                  value={f.user}
                  onChange={(e) => set("user", e.target.value)}
                  spellCheck={false}
                  autoComplete="off"
                />
              </div>
              <div className="field">
                <label>Password</label>
                <input
                  className="input mono"
                  type="password"
                  autoComplete="new-password"
                  placeholder={!isNew && !pwTouched ? "Saved in keychain" : ""}
                  value={password}
                  onChange={(e) => {
                    setPassword(e.target.value);
                    setPwTouched(true);
                    setTest(null);
                  }}
                />
              </div>
            </div>
            <div className="field">
                <label>{f.driver === "mysql" ? "Database (optional)" : f.driver === "redis" ? "Database index (0–15)" : "Database"}</label>
                <input
                  className="input mono"
                  placeholder={provider.database ?? ""}
                  value={f.database}
                  onChange={(e) => set("database", e.target.value)}
                  spellCheck={false}
                />
              </div>
          </>
        )}

            {f.ssh.enabled && f.driver !== "sqlite" && (
              <div className="hint">
                <Network /> Connecting through SSH {f.ssh.user || "root"}@{f.ssh.host || "server"}. The host above is
                resolved on that server (often 127.0.0.1).
              </div>
            )}
          </>
        )}

        {tab === "ssh" && (
          <>
            <div className="toggle-row">
              <Network />
              <div style={{ flex: 1 }}>
                <b>Connect through an SSH server</b>
                <small>
                  Use this when the database lives on a VPS or behind a bastion host and only SSH is open to the
                  internet.
                </small>
              </div>
              <Switch on={f.ssh.enabled} onChange={(v) => setSsh("enabled", v)} label="Use SSH tunnel" />
            </div>
            {f.ssh.enabled && (
              <>
                <div className="grid-2">
                  <div className="field">
                    <label>SSH host</label>
                    <input
                      className="input mono"
                      placeholder="203.0.113.10 or server.example.com"
                      value={f.ssh.host}
                      onChange={(e) => setSsh("host", e.target.value)}
                      spellCheck={false}
                    />
                  </div>
                  <div className="field">
                    <label>SSH port</label>
                    <input
                      className="input mono"
                      inputMode="numeric"
                      value={f.ssh.port || ""}
                      placeholder="22"
                      onChange={(e) => setSsh("port", +e.target.value.replace(/\D/g, "") || 0)}
                    />
                  </div>
                </div>
                <div className="field">
                  <label>SSH user</label>
                  <input
                    className="input mono"
                    placeholder="ubuntu, root, ec2-user…"
                    value={f.ssh.user}
                    onChange={(e) => setSsh("user", e.target.value)}
                    spellCheck={false}
                    autoComplete="off"
                  />
                </div>
                <div className="field">
                  <label>Sign in with</label>
                  <div className="segmented" style={{ alignSelf: "flex-start" }}>
                    {(
                      [
                        ["key", "Private key", <KeyRound key="k" />],
                        ["agent", "SSH agent", <Fingerprint key="a" />],
                        ["password", "Password", <Lock key="p" />],
                      ] as const
                    ).map(([id, label, icon]) => (
                      <button type="button" key={id} className={f.ssh.auth === id ? "on" : ""} onClick={() => setSsh("auth", id)}>
                        {icon} {label}
                      </button>
                    ))}
                  </div>
                </div>
                {f.ssh.auth === "key" && (
                  <div className="field">
                    <label>Private key file</label>
                    <div className="url-row">
                      <input
                        className="input mono"
                        placeholder="~/.ssh/id_ed25519"
                        value={f.ssh.keyPath}
                        onChange={(e) => setSsh("keyPath", e.target.value)}
                        spellCheck={false}
                      />
                      <button type="button" className="btn" onClick={() => browse("ssh-key", (p) => setSsh("keyPath", p))}>
                        <FolderOpen /> Browse
                      </button>
                    </div>
                    <span className="hint">
                      <Info /> OpenSSH, RSA, ed25519 and ECDSA keys. Tabla only stores the path, never the key.
                    </span>
                  </div>
                )}
                {f.ssh.auth !== "agent" ? (
                  <div className="field">
                    <label>{f.ssh.auth === "key" ? "Key passphrase (if the key has one)" : "SSH password"}</label>
                    <input
                      className="input mono"
                      type="password"
                      autoComplete="new-password"
                      placeholder={!isNew && !sshTouched ? "Saved in keychain" : ""}
                      value={sshSecret}
                      onChange={(e) => {
                        setSshSecret(e.target.value);
                        setSshTouched(true);
                        setTest(null);
                      }}
                    />
                  </div>
                ) : (
                  <div className="hint">
                    <Info /> Uses keys loaded in your running agent (ssh-agent, 1Password, Secretive). Add one with
                    ssh-add.
                  </div>
                )}
                <div className="hint">
                  <ShieldCheck /> The server's identity is checked against ~/.ssh/known_hosts. New servers are
                  remembered, and Tabla refuses to connect if a server's key ever changes.
                </div>
              </>
            )}
          </>
        )}

        {tab === "ssl" && (
          <>
            <div className="field">
              <label>SSL mode</label>
              <div className="ssl-modes">
                {(
                  [
                    ["disable", "Disable", "No encryption. Fine for local databases."],
                    ["prefer", "Prefer", "Encrypt if the server supports it."],
                    ["require", "Require", "Always encrypt. Use for anything on the internet."],
                    ["verify-full", "Verify", "Encrypt and verify the server's certificate and host name."],
                  ] as const
                ).map(([id, label, desc]) => (
                  <button type="button" key={id} className={f.sslMode === id ? "on" : ""} onClick={() => set("sslMode", id)}>
                    <b>{label}</b>
                    <span>{desc}</span>
                  </button>
                ))}
              </div>
            </div>
            {f.driver !== "redis" && (
              [
                ["sslCa", "CA certificate", "Server CA (.pem / .crt), for Verify with a private CA"],
                ["sslCert", "Client certificate", "Only if your server requires client certificates"],
                ["sslKey", "Client key", "Private key that matches the client certificate"],
              ] as const
            ).map(([k, label, hint]) => (
              <div className="field" key={k}>
                <label>{label}</label>
                <div className="url-row">
                  <input
                    className="input mono"
                    placeholder={hint}
                    value={f[k]}
                    onChange={(e) => set(k, e.target.value)}
                    spellCheck={false}
                  />
                  {f[k] && (
                    <button type="button" className="icon-btn" title="Clear" onClick={() => set(k, "")}>
                      <XCircle />
                    </button>
                  )}
                  <button type="button" className="btn" onClick={() => browse("cert", (p) => set(k, p))}>
                    <FolderOpen /> Browse
                  </button>
                </div>
              </div>
            ))}
            {f.driver === "redis" && (
              <p className="hint">
                <Info /> Require turns on TLS (rediss://). Prefer and Disable connect without TLS. Verify also checks the certificate.
              </p>
            )}
            {f.ssh.enabled && f.sslMode === "verify-full" && (
              <div className="hint">
                <Info /> With an SSH tunnel, host name verification sees 127.0.0.1. Use Require instead of Verify.
              </div>
            )}
          </>
        )}

        {tab === "options" && (
          <>
        <div className="grid-2b">
          <div className="field">
            <label>Environment</label>
            <Select
              className="w-full"
              value={f.tag}
              options={TAGS.map((t) => ({ value: t.value, label: t.label, icon: <span className={`env-dot ${t.value}`} /> }))}
              onChange={(v) => set("tag", v as EnvTag)}
            />
          </div>
          <div className="field">
            <label>Color</label>
            <div className="swatches" style={{ height: 30 }}>
              {COLORS.map((c) => (
                <button
                  type="button"
                  key={c}
                  className={`swatch ${f.color === c ? "on" : ""}`}
                  style={{ "--c": c } as React.CSSProperties}
                  onClick={() => set("color", c)}
                  aria-label={`Color ${c}`}
                />
              ))}
            </div>
          </div>
        </div>

        {f.tag === "production" && (
          <div className="toggle-row" style={{ borderColor: "color-mix(in srgb, var(--danger) 35%, transparent)" }}>
            <ShieldAlert color="var(--danger)" />
            <div>
              <b>Production safeguards on</b>
              <small>Tabla shows a red accent and asks before committing any change.</small>
            </div>
          </div>
        )}

        <div className="toggle-row">
          <Lock />
          <div style={{ flex: 1 }}>
            <b>Read-only</b>
            <small>Blocks edits in Tabla and opens a read-only session on the server.</small>
          </div>
          <Switch on={f.readOnly} onChange={(v) => set("readOnly", v)} label="Read-only" />
        </div>

        {f.driver !== "redis" && (
        <div className="toggle-row">
          <Bot />
          <div style={{ flex: 1 }}>
            <b>Allow AI assistants</b>
            <small>Let Claude, Cursor and other MCP tools read this database through Tabla. They can never write.</small>
          </div>
          <Switch on={f.aiAccess} onChange={(v) => set("aiAccess", v)} label="Allow AI assistants" />
        </div>
        )}
          </>
        )}
        <button type="submit" hidden />
      </form>
      {test && !test.ok && help && (
        <div className="err-help">
          <div className="err-help-title">
            <LifeBuoy /> {help.title}
          </div>
          <ul>
            {help.tips.map((t, i) => (
              <li key={i}>{t}</li>
            ))}
          </ul>
        </div>
      )}
      <div className="modal-foot">
        <button className="btn" type="button" onClick={runTest} disabled={busy !== null}>
          {busy === "test" ? <span className="spinner" /> : <Zap />} Test
        </button>
        {test && (
          <div className={`test-result ${test.ok ? "ok" : "err"}`} title={test.msg}>
            {test.ok ? <CheckCircle2 /> : <XCircle />}
            <span>{test.msg}</span>
          </div>
        )}
        <div className="spacer" />
        <button className="btn ghost" type="button" onClick={close}>
          Cancel
        </button>
        {isNew && step > 0 && (
          <button className="btn" type="button" onClick={back}>
            <ChevronLeft /> Back
          </button>
        )}
        {isNew && !lastStep ? (
          <button className="btn primary" type="button" onClick={next}>
            Next <ChevronRight />
          </button>
        ) : (
          <>
            <button className="btn" type="button" onClick={() => submit(false)} disabled={busy !== null}>
              <KeyRound /> Save
            </button>
            <button className="btn primary" type="button" onClick={() => submit(true)} disabled={busy !== null}>
              {busy === "save" && <span className="spinner" style={{ borderTopColor: "#fff" }} />}
              Save & connect
            </button>
          </>
        )}
      </div>
    </Modal>
  );
}

function ProviderPicker({ value, onChange }: { value: string; onChange(p: Provider): void }) {
  const groups = ["Databases", "Cloud & hosted"] as const;
  return (
    <div className="provider-pick">
      {groups.map((g) => (
        <div key={g}>
          <div className="provider-group">{g}</div>
          <div className="provider-grid">
            {PROVIDERS.filter((p) => p.group === g).map((p) => (
              <button type="button" key={p.id} className={value === p.id ? "on" : ""} onClick={() => onChange(p)} title={p.label}>
                <DriverIcon driver={p.driver} provider={p.id} size={22} />
                <span>{p.label}</span>
              </button>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
