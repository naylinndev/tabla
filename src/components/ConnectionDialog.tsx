import { useState } from "react";
import { BookOpen, CheckCircle2, ChevronDown, FolderOpen, Info, KeyRound, Link2, Lock, ShieldAlert, XCircle, Zap } from "lucide-react";
import { api, errorMessage, type ConnectionConfig, type Driver, type EnvTag, type SslMode } from "../lib/api";
import { useApp } from "../lib/store";
import { DriverIcon } from "./icons";
import { PROVIDERS, providerById, type Provider } from "../lib/providers";
import { Modal, Switch } from "./ui";

const COLORS = ["#7c6cff", "#5b8def", "#22b8cf", "#3ecf8e", "#f5b84b", "#ff8a4c", "#ff5d6c", "#e86bd0", "#8a909d"];
const TAGS: { value: EnvTag; label: string }[] = [
  { value: "local", label: "Local" },
  { value: "development", label: "Development" },
  { value: "staging", label: "Staging" },
  { value: "production", label: "Production" },
];
const DEFAULT_PORT: Record<Driver, number> = { postgres: 5432, mysql: 3306, sqlite: 0 };

/** Parses postgres://, mysql:// and sqlite:// URLs into form fields. */
function parseUrl(raw: string): Partial<ConnectionConfig> | null {
  try {
    const u = new URL(raw.trim());
    const scheme = u.protocol.replace(":", "");
    const driver: Driver | null = /^postgres(ql)?$/.test(scheme)
      ? "postgres"
      : /^(mysql|mariadb)$/.test(scheme)
        ? "mysql"
        : /^sqlite3?$/.test(scheme)
          ? "sqlite"
          : null;
    if (!driver) return null;
    if (driver === "sqlite") return { driver, database: decodeURIComponent(u.pathname), host: "", port: 0 };
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

  const [f, setF] = useState<ConnectionConfig>({ ...initial, password: undefined });
  const [password, setPassword] = useState("");
  const [pwTouched, setPwTouched] = useState(false);
  const [url, setUrl] = useState("");
  const [urlError, setUrlError] = useState(false);
  const [test, setTest] = useState<{ ok: boolean; msg: string } | null>(null);
  const [busy, setBusy] = useState<"test" | "save" | null>(null);
  const [guideOpen, setGuideOpen] = useState(isNew);
  const provider = providerById(f.provider, f.driver);

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
      // Keep a hosted preset if its engine matches; otherwise fall back to the plain engine.
      provider: providerById(x.provider, x.driver).driver === rest.driver ? x.provider : rest.driver!,
      name: x.name || rest.database || rest.host || "",
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

  return (
    <Modal onClose={close} className="modal wide">
      <div className="modal-head">
        <DriverIcon driver={f.driver} provider={f.provider} size={36} />
        <div>
          <h2>{isNew ? "New connection" : `Edit ${initial.name}`}</h2>
          <p>Passwords are stored in your system keychain, never on disk.</p>
        </div>
      </div>
      <form
        className="modal-body"
        onSubmit={(e) => {
          e.preventDefault();
          submit(true);
        }}
      >
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
            autoFocus
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
                <label>User</label>
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
            <div className="grid-2b">
              <div className="field">
                <label>{f.driver === "mysql" ? "Database (optional)" : "Database"}</label>
                <input
                  className="input mono"
                  placeholder={provider.database ?? ""}
                  value={f.database}
                  onChange={(e) => set("database", e.target.value)}
                  spellCheck={false}
                />
              </div>
              <div className="field">
                <label>SSL</label>
                <select className="select" value={f.sslMode} onChange={(e) => set("sslMode", e.target.value as SslMode)}>
                  <option value="prefer">Prefer</option>
                  <option value="require">Require</option>
                  <option value="verify-full">Verify (CA + hostname)</option>
                  <option value="disable">Disable</option>
                </select>
              </div>
            </div>
          </>
        )}

        <div className="grid-2b">
          <div className="field">
            <label>Environment</label>
            <select className="select" value={f.tag} onChange={(e) => set("tag", e.target.value as EnvTag)}>
              {TAGS.map((t) => (
                <option key={t.value} value={t.value}>
                  {t.label}
                </option>
              ))}
            </select>
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
        <button type="submit" hidden />
      </form>
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
        <button className="btn" type="button" onClick={() => submit(false)} disabled={busy !== null}>
          <KeyRound /> Save
        </button>
        <button className="btn primary" type="button" onClick={() => submit(true)} disabled={busy !== null}>
          {busy === "save" && <span className="spinner" style={{ borderTopColor: "#fff" }} />}
          Save & connect
        </button>
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
