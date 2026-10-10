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
- **Built for live production databases.** Connect through **SSH tunnels** (private key with passphrase, SSH agent, or password) with host-key verification, and use **TLS client certificates** (CA, client cert and key).
- **Structure editing.** Add, rename, retype and drop columns, change nullability and defaults, and **drag and drop to reorder** them. Tabla always shows the exact SQL first; Postgres changes run in one transaction.
- **Connect your AI.** Tabla is an **MCP server** (`tabla mcp`): Claude Code, Cursor, Claude Desktop, Windsurf and VS Code can read the databases you choose. AI access is read-only, guarded, and off by default for every connection.
- **Easy connections.** Import connections from a project's `.env` files (`DATABASE_URL` and similar), paste a URL, or follow step-by-step guides for each provider. Connection errors are explained in plain language with fix steps.
- **Spreadsheet-style editing.** Double-click a cell to edit, insert and delete rows, then review and commit everything in one transaction with <kbd>⌘ S</kbd>.
- **Safe writes.** Every UPDATE or DELETE must match exactly one row, or the whole batch rolls back.
- **Filters without SQL.** Stack conditions (`=`, `contains`, `in`, `is null` and more) or switch to a raw `WHERE` clause.
- **SQL editor.** Syntax highlighting, autocomplete for tables and columns, formatting, run the statement under the cursor (<kbd>⌘ ↵</kbd>) or the whole script (<kbd>⌘ ⇧ ↵</kbd>), and see multiple result sets.
- **Query history** for each connection, and query tabs that are kept between sessions.
- **Row inspector** with pretty-printed JSON and editable fields.
- **Structure view** showing columns, types, defaults, primary keys and indexes.
- **Several connections open at once,** switchable from the left rail.
- **Command palette** (<kbd>⌘ K</kbd>) to jump to any table, connection or action.
- **Export** results and table pages to CSV or typed JSON.
- **Copy for AI.** Copy a schema as an AI-ready prompt, or copy a failing query together with its error and related tables.
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

This also puts `tabla` on your PATH, so AI tools can run `tabla mcp`.
Early builds are unsigned; if macOS says the app can't be opened, right-click
**Tabla.app → Open** once (or remove the quarantine flag with
`xattr -dr com.apple.quarantine /Applications/Tabla.app`).

Maintainers: release and tap setup is described in [packaging/homebrew](packaging/homebrew/README.md).

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

Every connection can go through an **SSH tunnel**, for databases on a VPS or
behind a bastion host, and can use **TLS client certificates**.

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
  modes, using rustls, plus custom CA and client certificate/key files (only their paths are stored).
- **SSH tunnels** verify server host keys against `~/.ssh/known_hosts` (never modified)
  and Tabla's own `known_hosts`. Unknown hosts are trusted on first use, and a **changed key is always refused**.
  SSH passwords and key passphrases live in the keychain; private keys are read from disk and never copied.
- **AI access (MCP) is opt-in per connection and read-only.** Each AI query must be a
  single read statement (SELECT/WITH/SHOW/EXPLAIN), known escape hatches are blocked, and it runs in a
  read-only transaction with a timeout that is always rolled back. For a hard guarantee, share
  a connection that uses a database user with only `SELECT` rights.
- **Production guardrails.** Connections tagged *production* show a red accent and
  ask before committing data or structure changes. Every row edit must match exactly one row.
- **Locked-down web layer.** A strict Content Security Policy, no remote
  content, and minimal Tauri permissions. All database, file and dialog access
  goes through a small set of Rust commands. File export always goes through
  a native "Save as" dialog.
- **No telemetry, no analytics, no accounts.**

Found a vulnerability? Please open a private
[security advisory](https://github.com/naylinndev/tabla/security/advisories/new)
instead of a public issue.

## Connect your AI (MCP)

Open **Connect AI** (the robot icon in the left rail), switch on the connections
your AI may read, then add Tabla to your tool. For example, in Claude Code:

```bash
claude mcp add tabla -- "/Applications/Tabla.app/Contents/MacOS/tabla" mcp
```

Cursor, Claude Desktop, Windsurf and VS Code use the same command and `["mcp"]`
argument in their MCP config. The dialog shows the exact snippet for your system.
Tools exposed: `list_connections`, `get_schema`, `describe_table` and `run_query` (read-only).

## Keyboard shortcuts

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| Command palette | <kbd>⌘ K</kbd> | <kbd>Ctrl K</kbd> |
| Home (all connections) | <kbd>⌘ 0</kbd> | <kbd>Ctrl 0</kbd> |
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
TABLA_TEST_SSH="127.0.0.1:2222:<user>:/dir/with/test/keys" \
TABLA_CONFIG_DIR="$(mktemp -d)" \
cargo test
```

Use throwaway databases and a throwaway SSH server: the tests write to them.
`TABLA_CONFIG_DIR` keeps tests away from your real saved connections.

### Browser-driven UI tests (debug builds only)

`tabla bridge` serves the backend on `127.0.0.1:1430` so the real UI can be
driven in a normal browser. It needs a per-run token, accepts only the Vite dev
origin, and refuses to start without `TABLA_CONFIG_DIR`. It is not compiled
into release builds.

```bash
TABLA_BRIDGE_TOKEN=$(uuidgen) TABLA_CONFIG_DIR=$(mktemp -d) src-tauri/target/debug/tabla bridge
```

Then open `http://localhost:1420/?bridge=<token>` while `pnpm dev` is running.

### Project layout

```
src/                  React + TypeScript UI
  components/         Views: Home, Workspace, TableView, QueryView, DataGrid…
  lib/                API bindings, store, SQL helpers, provider presets
  styles/app.css      Design tokens and styles (dark + light)
src-tauri/            Rust backend
  src/config.rs       Connection profiles + keychain
  src/db.rs           Drivers, metadata, paging, transactional edits
  src/alter.rs        Structure editing (ALTER TABLE per engine)
  src/ssh.rs          SSH tunnels + host key verification
  src/mcp.rs          `tabla mcp` server for AI assistants
  src/ai_guard.rs     Read-only SQL validation for AI queries
  src/commands.rs     Tauri commands exposed to the UI
  src/bridge.rs       Debug-only HTTP bridge for UI tests
```

## Roadmap

- [ ] SQL Server, Redis, MongoDB, ClickHouse, DuckDB, Oracle
- [ ] Create tables, indexes and foreign keys visually; DDL view
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
