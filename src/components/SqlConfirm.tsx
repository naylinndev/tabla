import { useState, type ReactNode } from "react";
import { ShieldAlert, Wand2 } from "lucide-react";
import { api, errorMessage } from "../lib/api";
import { joinStatements } from "../lib/sql";
import { useApp, type Session } from "../lib/store";
import { Modal } from "./ui";

/** Shows the exact SQL, then runs it. Production and destructive actions get a red button. */
export function SqlConfirm({
  session,
  title,
  description,
  statements,
  displayStatements,
  danger,
  confirmLabel = "Run",
  children,
  onDone,
  onClose,
}: {
  session: Session;
  title: string;
  description?: ReactNode;
  statements: string[];
  /** What to show instead of `statements` (e.g. with passwords masked). */
  displayStatements?: string[];
  danger?: boolean;
  confirmLabel?: string;
  children?: ReactNode;
  onDone(): void;
  onClose(): void;
}) {
  const toast = useApp((s) => s.toast);
  const prod = session.connection.tag === "production";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const red = danger || prod;

  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.runQuery(session.id, joinStatements(session.connection.driver, statements));
      toast(`${title}: done`, "success");
      onDone();
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };

  return (
    <Modal onClose={onClose} className="modal wide">
      <div className="modal-head">
        {red ? <ShieldAlert color="var(--danger)" /> : <Wand2 color="var(--accent-2)" />}
        <div>
          <h2>
            {title}
            {prod ? " on production" : ""}
          </h2>
          {description && <p>{description}</p>}
        </div>
      </div>
      <div className="modal-body">
        {children}
        <div className="code-block">
          <div className="code-head">
            <span>SQL</span>
            <button className="btn ghost" onClick={() => navigator.clipboard.writeText(joinStatements(session.connection.driver, displayStatements ?? statements, true))}>
              Copy
            </button>
          </div>
          <pre>{joinStatements(session.connection.driver, displayStatements ?? statements, true)}</pre>
        </div>
        {error && (
          <div className="q-error" style={{ margin: 0 }}>
            {error}
          </div>
        )}
      </div>
      <div className="modal-foot">
        <div className="spacer" />
        <button className="btn" onClick={onClose}>
          Cancel
        </button>
        <button className={`btn ${red ? "danger solid" : "primary"}`} onClick={run} disabled={busy || statements.length === 0}>
          {busy && <span className="spinner" style={{ borderTopColor: "#fff" }} />}
          {confirmLabel}
        </button>
      </div>
    </Modal>
  );
}
