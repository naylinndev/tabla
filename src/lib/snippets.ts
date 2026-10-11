import type { Driver } from "./api";

interface Snippet {
  name: string;
  hint: string;
  sql: string;
}

const common: Snippet[] = [
  { name: "Select with limit", hint: "First 100 rows", sql: "SELECT *\nFROM table_name\nLIMIT 100;" },
  { name: "Count by column", hint: "Group and count", sql: "SELECT column_name, COUNT(*) AS n\nFROM table_name\nGROUP BY column_name\nORDER BY n DESC;" },
  {
    name: "Find duplicates",
    hint: "Values that appear more than once",
    sql: "SELECT column_name, COUNT(*) AS n\nFROM table_name\nGROUP BY column_name\nHAVING COUNT(*) > 1\nORDER BY n DESC;",
  },
  { name: "Recent rows", hint: "Newest first", sql: "SELECT *\nFROM table_name\nORDER BY created_at DESC\nLIMIT 50;" },
];

const byDriver: Record<string, Snippet[]> = {
  postgres: [
    {
      name: "Table sizes",
      hint: "Biggest tables with indexes",
      sql: "SELECT relname AS table, pg_size_pretty(pg_total_relation_size(relid)) AS total,\n       n_live_tup AS rows\nFROM pg_stat_user_tables\nORDER BY pg_total_relation_size(relid) DESC;",
    },
    {
      name: "Running queries",
      hint: "What's executing right now",
      sql: "SELECT pid, now() - query_start AS running_for, state, query\nFROM pg_stat_activity\nWHERE state <> 'idle' AND pid <> pg_backend_pid()\nORDER BY running_for DESC;",
    },
    {
      name: "Unused indexes",
      hint: "Indexes never scanned",
      sql: "SELECT schemaname, relname AS table, indexrelname AS index,\n       pg_size_pretty(pg_relation_size(indexrelid)) AS size\nFROM pg_stat_user_indexes\nWHERE idx_scan = 0\nORDER BY pg_relation_size(indexrelid) DESC;",
    },
    {
      name: "Explain analyze",
      hint: "Real query plan with timing",
      sql: "EXPLAIN (ANALYZE, BUFFERS)\nSELECT * FROM table_name WHERE column_name = 'value';",
    },
    { name: "Locks", hint: "Blocked and blocking sessions", sql: "SELECT pid, pg_blocking_pids(pid) AS blocked_by, query\nFROM pg_stat_activity\nWHERE cardinality(pg_blocking_pids(pid)) > 0;" },
  ],
  mysql: [
    {
      name: "Table sizes",
      hint: "Biggest tables with indexes",
      sql: "SELECT table_name, table_rows,\n       ROUND((data_length + index_length) / 1024 / 1024, 1) AS size_mb\nFROM information_schema.tables\nWHERE table_schema = DATABASE()\nORDER BY data_length + index_length DESC;",
    },
    { name: "Running queries", hint: "What's executing right now", sql: "SELECT id, user, time, state, info\nFROM information_schema.processlist\nWHERE command <> 'Sleep'\nORDER BY time DESC;" },
    {
      name: "Slowest statements",
      hint: "From performance_schema",
      sql: "SELECT digest_text, count_star AS calls,\n       ROUND(avg_timer_wait / 1e9, 2) AS avg_ms\nFROM performance_schema.events_statements_summary_by_digest\nORDER BY avg_timer_wait DESC\nLIMIT 20;",
    },
    { name: "Explain analyze", hint: "Real query plan with timing", sql: "EXPLAIN ANALYZE\nSELECT * FROM table_name WHERE column_name = 'value';" },
  ],
  sqlite: [
    { name: "All tables", hint: "With their SQL", sql: "SELECT name, sql\nFROM sqlite_master\nWHERE type = 'table'\nORDER BY name;" },
    { name: "Integrity check", hint: "Look for corruption", sql: "PRAGMA integrity_check;" },
    { name: "Explain query plan", hint: "How SQLite runs a query", sql: "EXPLAIN QUERY PLAN\nSELECT * FROM table_name WHERE column_name = 'value';" },
  ],
};

export const snippetsFor = (d: Driver): Snippet[] => [...common, ...(byDriver[d] ?? [])];
