import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown } from "lucide-react";

export interface Option<T extends string | number = string> {
  value: T;
  label: string;
  icon?: ReactNode;
  hint?: string;
}

/** Positions a popover under (or above, if there's no room) its anchor. */
function usePopover(open: boolean, anchor: React.RefObject<HTMLElement | null>, maxH = 300) {
  const [style, setStyle] = useState<React.CSSProperties>({});
  useLayoutEffect(() => {
    if (!open || !anchor.current) return;
    const place = () => {
      const r = anchor.current!.getBoundingClientRect();
      const below = window.innerHeight - r.bottom - 8;
      const up = below < Math.min(maxH, 180) && r.top > below;
      setStyle({
        position: "fixed",
        left: Math.min(r.left, window.innerWidth - Math.max(r.width, 180) - 8),
        minWidth: r.width,
        maxHeight: Math.min(maxH, (up ? r.top : below) - 6),
        ...(up ? { bottom: window.innerHeight - r.top + 4 } : { top: r.bottom + 4 }),
      });
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, anchor, maxH]);
  return style;
}

function useOutside(open: boolean, refs: React.RefObject<HTMLElement | null>[], close: () => void) {
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!refs.some((r) => r.current?.contains(e.target as Node))) close();
    };
    window.addEventListener("mousedown", onDown, true);
    return () => window.removeEventListener("mousedown", onDown, true);
  }, [open, refs, close]);
}

function ListBox<T extends string | number>({
  options,
  active,
  selected,
  onPick,
  onHover,
  style,
  listRef,
  empty,
}: {
  options: Option<T>[];
  active: number;
  selected?: T;
  onPick(o: Option<T>): void;
  onHover(i: number): void;
  style: React.CSSProperties;
  listRef: React.RefObject<HTMLDivElement | null>;
  empty?: string;
}) {
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-i="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active, listRef]);
  return createPortal(
    <div ref={listRef} className="listbox" role="listbox" style={style} onMouseDown={(e) => e.preventDefault()}>
      {options.length === 0 && <div className="listbox-empty">{empty ?? "No matches"}</div>}
      {options.map((o, i) => (
        <div
          key={String(o.value)}
          data-i={i}
          role="option"
          aria-selected={o.value === selected}
          className={`listbox-item ${i === active ? "active" : ""} ${o.value === selected ? "selected" : ""}`}
          onMouseEnter={() => onHover(i)}
          onClick={() => onPick(o)}
        >
          <span className="li-check">{o.value === selected && <Check />}</span>
          {o.icon && <span className="li-icon">{o.icon}</span>}
          <span className="li-label">{o.label}</span>
          {o.hint && <span className="li-hint">{o.hint}</span>}
        </div>
      ))}
    </div>,
    document.body,
  );
}

/** Styled replacement for <select>: keyboard navigation, type-to-jump, checkmarks. */
export function Select<T extends string | number>({
  value,
  options,
  onChange,
  size = "md",
  className = "",
  disabled,
  placeholder = "Select…",
  title,
  mono,
}: {
  value: T;
  options: Option<T>[];
  onChange(v: T): void;
  size?: "sm" | "md";
  className?: string;
  disabled?: boolean;
  placeholder?: string;
  title?: string;
  mono?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const btn = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const typed = useRef({ text: "", at: 0 });
  const style = usePopover(open, btn);
  const close = () => setOpen(false);
  useOutside(open, [btn, list], close);
  const current = options.find((o) => o.value === value);

  const openAt = () => {
    setActive(Math.max(0, options.findIndex((o) => o.value === value)));
    setOpen(true);
  };
  const pick = (o: Option<T>) => {
    onChange(o.value);
    setOpen(false);
    btn.current?.focus();
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (disabled) return;
    if (!open && ["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
      e.preventDefault();
      return openAt();
    }
    if (!open) return;
    if (e.key === "ArrowDown") setActive((a) => Math.min(options.length - 1, a + 1));
    else if (e.key === "ArrowUp") setActive((a) => Math.max(0, a - 1));
    else if (e.key === "Home") setActive(0);
    else if (e.key === "End") setActive(options.length - 1);
    else if (e.key === "Enter" || e.key === " ") options[active] && pick(options[active]);
    else if (e.key === "Escape" || e.key === "Tab") return setOpen(false);
    else if (e.key.length === 1) {
      const now = Date.now();
      typed.current.text = (now - typed.current.at > 700 ? "" : typed.current.text) + e.key.toLowerCase();
      typed.current.at = now;
      const i = options.findIndex((o) => o.label.toLowerCase().startsWith(typed.current.text));
      if (i >= 0) setActive(i);
    } else return;
    e.preventDefault();
    e.stopPropagation();
  };

  return (
    <>
      <button
        ref={btn}
        type="button"
        className={`select-btn ${size} ${open ? "open" : ""} ${mono ? "mono" : ""} ${className}`}
        onClick={() => (open ? setOpen(false) : openAt())}
        onKeyDown={onKey}
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        title={title}
      >
        {current?.icon && <span className="li-icon">{current.icon}</span>}
        <span className={`sb-value ${current ? "" : "dim"}`}>{current?.label ?? placeholder}</span>
        <ChevronDown className="chev" />
      </button>
      {open && (
        <ListBox
          options={options}
          active={active}
          selected={value}
          onPick={pick}
          onHover={setActive}
          style={style}
          listRef={list}
        />
      )}
    </>
  );
}

/** Free-text input with a filtered suggestion list (e.g. SQL column types). */
export function Combobox({
  value,
  onChange,
  suggestions,
  placeholder,
  disabled,
  className = "",
}: {
  value: string;
  onChange(v: string): void;
  suggestions: string[];
  placeholder?: string;
  disabled?: boolean;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState(false);
  const [active, setActive] = useState(0);
  const wrap = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const style = usePopover(open, wrap, 260);
  const close = () => setOpen(false);
  useOutside(open, [wrap, list], close);
  const options = useMemo(() => {
    const q = value.trim().toLowerCase();
    const all = suggestions.map((s) => ({ value: s, label: s }));
    return filter && q ? all.filter((o) => o.value.toLowerCase().includes(q)) : all;
  }, [suggestions, value, filter]);

  const pick = (o: Option<string>) => {
    onChange(o.value);
    setOpen(false);
  };

  return (
    <div ref={wrap} className={`combo ${open ? "open" : ""} ${className}`}>
      <input
        className="input sm mono"
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        spellCheck={false}
        onChange={(e) => {
          onChange(e.target.value);
          setFilter(true);
          setActive(0);
          setOpen(true);
        }}
        onFocus={() => {
          setFilter(false);
          setActive(Math.max(0, suggestions.indexOf(value)));
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            if (!open) return setOpen(true);
            setActive((a) => Math.min(options.length - 1, a + 1));
          } else if (e.key === "ArrowUp" && open) {
            e.preventDefault();
            setActive((a) => Math.max(0, a - 1));
          } else if (e.key === "Enter" && open && options[active]) {
            e.preventDefault();
            pick(options[active]);
          } else if (e.key === "Escape") setOpen(false);
        }}
      />
      {!disabled && (
        <button
          type="button"
          className="combo-toggle"
          tabIndex={-1}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            setFilter(false);
            setActive(Math.max(0, suggestions.indexOf(value)));
            setOpen(!open);
          }}
          aria-label="Show suggestions"
        >
          <ChevronDown />
        </button>
      )}
      {open && options.length > 0 && (
        <ListBox options={options} active={active} selected={value} onPick={pick} onHover={setActive} style={style} listRef={list} />
      )}
    </div>
  );
}

export function Checkbox({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange(v: boolean): void;
  disabled?: boolean;
  label?: ReactNode;
}) {
  return (
    <label className={`checkbox ${disabled ? "disabled" : ""}`}>
      <button
        type="button"
        role="checkbox"
        aria-checked={checked}
        disabled={disabled}
        className={`cb-box ${checked ? "on" : ""}`}
        onClick={() => onChange(!checked)}
      >
        {checked && <Check />}
      </button>
      {label && <span>{label}</span>}
    </label>
  );
}
