# Contributing to Tabla

Thanks for helping make Tabla better! Bug reports, ideas, docs and code are all welcome.

## Ways to help

- **Report a bug:** open an issue with steps to reproduce, what you expected, and
  your OS, Tabla version and database engine and version.
- **Suggest a feature:** open an issue describing the problem it solves first.
- **Send a pull request:** for anything larger than a small fix, please open an
  issue first so we can agree on the approach.
- **Security issues:** please don't open a public issue; see [SECURITY.md](SECURITY.md).

## Development setup

You need Node.js 20+, pnpm 9+, Rust (stable) and the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS
(on Linux also `libdbus-1-dev`).

```bash
pnpm install
pnpm tauri dev
```

## Before you open a pull request

```bash
pnpm build                     # typecheck + UI build
cd src-tauri && cargo test     # Rust tests
```

Database integration tests are skipped unless you point them at throwaway
servers (they write to them). See the **Tests** section in the README for the
`TABLA_TEST_*` variables. Set `TABLA_CONFIG_DIR` to a temp folder so tests never
touch your real saved connections.

## Guidelines

- **Safety first.** Anything that writes to a database must show what will
  happen (the SQL or command) and respect read-only connections and the
  production confirmation.
- **Never handle secrets loosely.** Passwords and passphrases belong in the OS
  keychain only; never log them, put them in URLs, or write them to files.
- **Match the surrounding code:** naming, comments and structure.
- **Keep the UI consistent:** reuse the existing components (Select, Checkbox,
  SqlConfirm, grids) and design tokens instead of new one-off styles.
- **Keep personal data out:** no real database dumps, credentials, internal
  hostnames or personal details in commits, issues, tests or screenshots.

## Commit messages

Write a short summary line in the imperative mood ("Add Redis key browser"),
then a blank line and details if needed.

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE).
