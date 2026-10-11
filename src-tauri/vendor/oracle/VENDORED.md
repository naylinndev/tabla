# Vendored `oracle` crate (rust-oracle 0.6.3)

Upstream: https://github.com/kubo/rust-oracle (UPL-1.0 / Apache-2.0), taken
from crates.io unchanged except for one patch, so Tabla can show every column
type Oracle returns:

- `src/sql_type/oracle_type.rs`: `JSON` (Oracle 21c+) and `VECTOR` (23ai)
  columns are fetched as text (`DPI_ORACLE_TYPE_LONG_VARCHAR`); the server
  converts them. Upstream 0.6.3 fails with "unsupported Oracle type JSON" and
  "unknown Oracle type number 2033". With every type handled, the
  catch-all "unsupported Oracle type" arm became unreachable and was removed.
- `Cargo.toml`: allows the `mismatched_lifetime_syntaxes` lint so newer
  compilers don't print warnings for this code.

Docs, tests and examples were removed. Wired in through `[patch.crates-io]`
in `src-tauri/Cargo.toml`; drop the patch once upstream supports these types.
