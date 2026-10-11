import { api } from "./api";
import { providerById } from "./providers";
import type { Session } from "./store";

const ENGINE = { postgres: "PostgreSQL", mysql: "MySQL", sqlite: "SQLite", redis: "Redis", mssql: "SQL Server" } as const;

export function engineLabel(s: Session) {
  const p = providerById(s.connection.provider, s.connection.driver);
  const base = ENGINE[s.connection.driver];
  return p.id === s.connection.driver ? `${base} ${s.serverVersion.split(/[\s-]/)[0]}` : `${base} (${p.label})`;
}

/** One line per table: `users(id integer PK, email text NOT NULL, …)`. */
export async function schemaLines(s: Session, tables = s.tables.map((t) => t.name), max = 150): Promise<string> {
  const lines: string[] = [];
  for (const name of tables.slice(0, max)) {
    const st = await api.tableStructure(s.id, s.schema, name);
    const cols = st.columns
      .map((c) => `${c.name} ${c.dataType}${c.primaryKey ? " PK" : ""}${c.nullable ? "" : " NOT NULL"}`)
      .join(", ");
    lines.push(`${name}(${cols})`);
  }
  if (tables.length > max) lines.push(`… and ${tables.length - max} more tables`);
  return lines.join("\n");
}

export async function schemaPrompt(s: Session): Promise<string> {
  return [
    `Here is the schema of my ${engineLabel(s)} database (schema "${s.schema}").`,
    "Use it to answer my questions and write SQL that matches these exact table and column names.",
    "",
    "```",
    await schemaLines(s),
    "```",
  ].join("\n");
}

/** Tables mentioned in a SQL string, for focused AI context. */
export function tablesIn(sql: string, s: Session): string[] {
  const lower = sql.toLowerCase();
  return s.tables.map((t) => t.name).filter((n) => new RegExp(`\\b${n.toLowerCase().replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\b`).test(lower));
}
