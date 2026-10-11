import type { Driver } from "../lib/api";
import { providerById } from "../lib/providers";

/** Tabla mark: a rounded table grid with a highlighted header row. */
export function Logo({ size = 28 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="tabla-g" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#9b8cff" />
          <stop offset="1" stopColor="#5b6cff" />
        </linearGradient>
      </defs>
      <rect x="1" y="1" width="30" height="30" rx="8" fill="url(#tabla-g)" />
      <rect x="7" y="8" width="18" height="4.5" rx="1.6" fill="#fff" />
      <rect x="7" y="14.5" width="7.5" height="3.5" rx="1.2" fill="#fff" opacity=".7" />
      <rect x="16.5" y="14.5" width="8.5" height="3.5" rx="1.2" fill="#fff" opacity=".7" />
      <rect x="7" y="20" width="7.5" height="3.5" rx="1.2" fill="#fff" opacity=".45" />
      <rect x="16.5" y="20" width="8.5" height="3.5" rx="1.2" fill="#fff" opacity=".45" />
    </svg>
  );
}

const DRIVER_LABEL: Record<Driver, string> = { postgres: "PostgreSQL", mysql: "MySQL", sqlite: "SQLite", redis: "Redis", mssql: "SQL Server", clickhouse: "ClickHouse", duckdb: "DuckDB", oracle: "Oracle", mongodb: "MongoDB" };

export const driverLabel = (d: Driver) => DRIVER_LABEL[d];

/** Brand-colored monogram tile per engine/provider (no third-party logos shipped). */
export function DriverIcon({ driver, provider, size = 28 }: { driver: Driver; provider?: string; size?: number }) {
  const m = providerById(provider, driver);
  const id = `drv-${m.id}`;
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-label={m.label} role="img">
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor={m.from} />
          <stop offset="1" stopColor={m.to} />
        </linearGradient>
      </defs>
      <rect x="1" y="1" width="30" height="30" rx="8" fill={`url(#${id})`} />
      {m.id === "sqlite" || m.id === "duckdb" ? (
        // A small cylinder: the classic "database file" glyph.
        <g fill="none" stroke="#fff" strokeWidth="2">
          <ellipse cx="16" cy="10.5" rx="7" ry="2.8" />
          <path d="M9 10.5v11c0 1.6 3.1 2.8 7 2.8s7-1.2 7-2.8v-11" />
          <path d="M9 16c0 1.6 3.1 2.8 7 2.8s7-1.2 7-2.8" opacity=".7" />
        </g>
      ) : (
        <text
          x="16"
          y="21"
          textAnchor="middle"
          fontFamily="Inter Variable, system-ui, sans-serif"
          fontWeight="700"
          fontSize="13"
          fill="#fff"
        >
          {m.mono}
        </text>
      )}
    </svg>
  );
}
