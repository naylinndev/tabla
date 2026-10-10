import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { CheckCircle2, Info, X, XCircle } from "lucide-react";
import { useApp } from "../lib/store";

export const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform || navigator.userAgent);
export const mod = isMac ? "⌘" : "Ctrl";

export function Kbd({ keys }: { keys: string }) {
  return <span className="kbd">{keys.replace(/Mod/g, mod).replace(/Shift/g, isMac ? "⇧" : "Shift")}</span>;
}

export function Modal({
  children,
  onClose,
  className = "modal",
}: {
  children: ReactNode;
  onClose(): void;
  className?: string;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return createPortal(
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={className} role="dialog" aria-modal="true">
        {children}
      </div>
    </div>,
    document.body,
  );
}

export interface MenuItem {
  label: string;
  icon?: ReactNode;
  kbd?: string;
  danger?: boolean;
  onClick(): void;
}

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: (MenuItem | "sep")[];
  onClose(): void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({ x: Math.min(x, window.innerWidth - r.width - 8), y: Math.min(y, window.innerHeight - r.height - 8) });
  }, [x, y]);
  useEffect(() => {
    const close = () => onClose();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("blur", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);
  return createPortal(
    <div ref={ref} className="ctx" style={{ left: pos.x, top: pos.y }} onMouseDown={(e) => e.stopPropagation()}>
      {items.map((it, i) =>
        it === "sep" ? (
          <hr key={i} />
        ) : (
          <button
            key={i}
            className={it.danger ? "danger" : ""}
            onClick={() => {
              it.onClick();
              onClose();
            }}
          >
            {it.icon}
            {it.label}
            {it.kbd && <Kbd keys={it.kbd} />}
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}

export function Switch({ on, onChange, label }: { on: boolean; onChange(v: boolean): void; label?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      className={`switch ${on ? "on" : ""}`}
      onClick={() => onChange(!on)}
    />
  );
}

export function Toasts() {
  const toasts = useApp((s) => s.toasts);
  const dismiss = useApp((s) => s.dismissToast);
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.kind}`}>
          {t.kind === "success" ? <CheckCircle2 /> : t.kind === "error" ? <XCircle /> : <Info />}
          <div className="msg">{t.message}</div>
          <button onClick={() => dismiss(t.id)} aria-label="Dismiss">
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}

export function TagBadge({ tag }: { tag: string }) {
  if (!tag) return null;
  return <span className={`badge ${tag}`}>{tag === "development" ? "dev" : tag === "production" ? "prod" : tag}</span>;
}

/** Drag handle hook: reports pixel deltas while dragging. */
export function useDrag(onMove: (dx: number, dy: number) => void, onEnd?: () => void) {
  const [active, setActive] = useState(false);
  const start = (e: React.MouseEvent) => {
    e.preventDefault();
    const sx = e.clientX;
    const sy = e.clientY;
    setActive(true);
    const move = (ev: MouseEvent) => onMove(ev.clientX - sx, ev.clientY - sy);
    const up = () => {
      setActive(false);
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      document.body.style.cursor = "";
      onEnd?.();
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };
  return { active, start };
}
