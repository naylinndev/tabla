export function formatBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 10 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

export const formatNumber = (n: number | string | null | undefined) =>
  n == null || n === "" ? "—" : Number(n).toLocaleString();

/** Splits "varchar(255)" / "int(11) unsigned" / "numeric(12,2)" into parts. */
export function splitType(t: string): { base: string; len: string; unsigned: boolean; rest: string } {
  let s = t.trim();
  const unsigned = /\bunsigned\b/i.test(s);
  s = s.replace(/\s*\bunsigned\b/i, "");
  const m = /^([^(]*?)\s*\(([^)]*)\)(.*)$/.exec(s);
  return m ? { base: m[1].trim(), len: m[2], unsigned, rest: m[3].trim() } : { base: s, len: "", unsigned, rest: "" };
}

export function joinType(p: { base: string; len: string; unsigned: boolean; rest: string }): string {
  return [p.base.trim() + (p.len.trim() ? `(${p.len.trim()})` : ""), p.rest, p.unsigned ? "unsigned" : ""]
    .filter(Boolean)
    .join(" ");
}
