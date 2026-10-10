# Security policy

Tabla connects to databases that often hold sensitive and production data, so
security reports are taken seriously.

## Reporting a vulnerability

Please **do not open a public issue**. Instead, report it privately through
GitHub: **Security → Report a vulnerability** on this repository
([direct link](https://github.com/naylinndev/tabla/security/advisories/new)).

Please include:

- what an attacker could do, and under which conditions;
- steps to reproduce or a proof of concept;
- the Tabla version and OS.

You'll get an acknowledgement as soon as possible, and we'll keep you updated
until a fix is released. Please give us a reasonable time to fix the issue
before disclosing it publicly.

## Supported versions

Security fixes go into the latest release.

## What Tabla does to keep you safe

- Passwords, SSH passwords and key passphrases are stored only in the OS
  keychain, and are masked in SQL previews and the Console.
- SSH host keys are verified, and a changed key is refused.
- AI access (MCP) is off by default, per connection, and read-only.
- No telemetry, no analytics, no accounts.

See the **Security** section of the [README](README.md#security) for details.
