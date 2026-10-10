<div align="center">

# Tabla

**A fast, free and open-source database client for macOS, Windows and Linux.**

Browse, edit and query PostgreSQL, MySQL, MariaDB, SQLite and popular hosted
databases from one native app, with no paywall, no tab limits and no telemetry.

[Features](#features) · [Install](#install) · [Supported databases](#supported-databases) · [Security](#security) · [Development](#development) · [Roadmap](#roadmap)

</div>

---

## Features

- **Native and light.** Rust core with a React UI on Tauri. The app is roughly 10 MB, not 150 MB.
- **Spreadsheet-style editing.** Double-click a cell to edit, insert and delete rows, then review and commit everything in one transaction with <kbd>⌘ S</kbd>.
- **Safe writes.** Every UPDATE or DELETE must match exactly one row, or the whole batch rolls back.
- **Filters without SQL.** Stack conditions (`=`, `contains`, `in`, `is null` and more) or switch to a raw `WHERE` clause.
- **SQL editor.** Syntax highlighting, autocomplete for tables and columns, formatting, run the statement under the cursor (<kbd>⌘ ↵</kbd>) or the whole script (<kbd>⌘ ⇧ ↵</kbd>), and see multiple result sets.
- **Query history** for each connection, and query tabs that are kept between sessions.
- **Row inspector** with pretty-printed JSON and editable fields.
- **Structure view** showing columns, types, defaults, primary keys and indexes.
- **Several connections open at once,** switchable from the left rail.
- **Command palette** (<kbd>⌘ K</kbd>) to jump to any table, connection or action.
- **Export** results and table pages to CSV or JSON.
- **Environment tags and production guardrails.** Production connections get a red accent and ask for confirmation before committing changes.
- **Read-only mode** for each connection, enforced on the database server, not only in the UI.
- **Dark and light themes** (or follow the system), with native vibrancy on macOS.
- **Unlimited tabs, windows and connections.** Nothing is paywalled.

## Install

> Tabla is in early development (v0.1). Pre-built downloads arrive with the first release.

**Homebrew (macOS)**, once the first release is published:

```bash
brew install --cask naylinndev/tabla/tabla
```

**Direct download:** grab the `.dmg` (macOS), `.msi` (Windows) or `.AppImage`/`.deb` (Linux) from [Releases](https://github.com/naylinndev/tabla/releases).

**From source:** see [Development](#development).

## Supported databases

| Engine / service | Status | Notes |
| --- | --- | --- |
| PostgreSQL 10+ | Supported | Schemas, views, materialized views |
| MySQL 5.7+ / 8.x | Supported | |
| MariaDB | Supported | Uses the MySQL driver |
| SQLite 3 | Supported | Opens local files; read-only option |
| CockroachDB | Supported | PostgreSQL wire protocol |
| YugabyteDB (YSQL) | Supported | PostgreSQL wire protocol |
| Supabase · Neon · Timescale · Amazon RDS / Aurora | Supported | Hosted PostgreSQL / MySQL |
| PlanetScale · TiDB Cloud | Supported | Hosted MySQL protocol |
| Amazon Redshift | Beta | Browsing and queries |
| SQL Server · Redis · MongoDB · ClickHouse · DuckDB · Oracle | Planned | See [Roadmap](#roadmap) |

Each provider in the **New connection** dialog shows step-by-step instructions
for finding its credentials. You can also paste a connection URL
(`postgres://…`, `mysql://…`) and Tabla fills in the form.

## Security

Tabla connects straight to your database and touches nothing else.

- **Passwords are stored only in your OS keychain**: macOS Keychain, Windows
  Credential Manager, or Secret Service (GNOME Keyring / KWallet) on Linux.
  They are never written to disk by Tabla and never returned to the UI.
- **Connection profiles** (host, port, user, database, without the password) are
  kept in your user config directory with owner-only permissions (`0600`):
  - macOS: `~/Library/Application Support/dev.tabla.desktop/connections.json`
  - Windows: `%APPDATA%\dev.tabla.desktop\connections.json`
  - Linux: `~/.config/dev.tabla.desktop/connections.json`
- **TLS** is supported with `prefer`, `require` and `verify-full` (CA plus hostname)
  modes, using rustls.
- **Locked-down web layer.** A strict Content Security Policy, no remote
  content, and minimal Tauri permissions. All database, file and dialog access
  goes through a small set of Rust commands. File export always goes through
  a native "Save as" dialog.
- **No telemetry, no analytics, no accounts.**

Found a vulnerability? Please open a private
[security advisory](https://github.com/naylinndev/tabla/security/advisories/new)
instead of a public issue.

## Keyboard shortcuts

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| Command palette | <kbd>⌘ K</kbd> | <kbd>Ctrl K</kbd> |
| New query tab | <kbd>⌘ T</kbd> | <kbd>Ctrl T</kbd> |
| Close tab | <kbd>⌘ W</kbd> | <kbd>Ctrl W</kbd> |
| Switch tab | <kbd>⌘ 1–9</kbd>, <kbd>⌘ ⇧ [ ]</kbd> | <kbd>Ctrl 1–9</kbd> |
| New connection | <kbd>⌘ N</kbd> | <kbd>Ctrl N</kbd> |
| Search tables | <kbd>⌘ L</kbd> | <kbd>Ctrl L</kbd> |
| Run statement / selection | <kbd>⌘ ↵</kbd> | <kbd>Ctrl ↵</kbd> |
| Run all | <kbd>⌘ ⇧ ↵</kbd> | <kbd>Ctrl Shift ↵</kbd> |
| Format SQL | <kbd>⌘ ⇧ F</kbd> | <kbd>Ctrl Shift F</kbd> |
| Commit changes | <kbd>⌘ S</kbd> | <kbd>Ctrl S</kbd> |
| Refresh table | <kbd>⌘ R</kbd> | <kbd>Ctrl R</kbd> |
| Filter rows | <kbd>⌘ F</kbd> | <kbd>Ctrl F</kbd> |
| Row inspector | <kbd>⌘ I</kbd> | <kbd>Ctrl I</kbd> |
| Edit cell | <kbd>↵</kbd> or start typing | same |
| Delete selected rows | <kbd>Delete</kbd> | same |

## Development

### Prerequisites

- [Node.js](https://nodejs.org) 20+ and [pnpm](https://pnpm.io) 9+
- [Rust](https://rustup.rs) (stable)
- Platform requirements for Tauri: see the [Tauri prerequisites](https://tauri.app/start/prerequisites/)
  - **Linux:** also `libdbus-1-dev` (keychain access)

### Run

```bash
pnpm install
pnpm tauri dev
```

### Build

```bash
pnpm tauri build
```

### Tests

The backend has integration tests that run against real servers. Each test is
skipped unless its environment variable is set:

```bash
cd src-tauri
TABLA_TEST_PG="localhost:5432:<user>:tabla_test" \
TABLA_TEST_MYSQL="localhost:3306:<user>:tabla_test" \
TABLA_TEST_SQLITE="/path/to/tabla_test.db" \
cargo test
```

Use throwaway databases: the tests write to them.

### Project layout

```
src/                  React + TypeScript UI
  components/         Views: Home, Workspace, TableView, QueryView, DataGrid…
  lib/                API bindings, store, SQL helpers, provider presets
  styles/app.css      Design tokens and styles (dark + light)
src-tauri/            Rust backend
  src/config.rs       Connection profiles + keychain
  src/db.rs           Drivers, metadata, paging, transactional edits
  src/commands.rs     Tauri commands exposed to the UI
```

## Roadmap

- [ ] SSH tunnels (bastion hosts)
- [ ] SQL Server, Redis, MongoDB, ClickHouse, DuckDB, Oracle
- [ ] Create and alter tables visually; DDL view
- [ ] Import CSV
- [ ] Query cancellation and streaming large results
- [ ] Saved queries and snippets
- [ ] ER diagram
- [ ] Auto-update and signed builds

## Contributing

Issues and pull requests are welcome. For larger changes, please open an issue
first so we can agree on the approach. Please keep personal data, credentials
and real database dumps out of commits, issues and screenshots.

## License

[MIT](LICENSE) © Tabla contributors
