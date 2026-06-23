# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.7.0] - 2026-06-23

### Added
- `mem-cli init` now ensures the `mem-cli` MCP server entry exists in
  `.mcp.json` at the repository root. The file is created if absent, and the
  entry is merged into `mcpServers` without touching other servers.

## [1.6.2] - 2026-06-23

### Fixed
- MCP stdio responses are now emitted as newline-delimited JSON-RPC instead of
  `Content-Length` framed (LSP-style) messages. The previous output framing was
  not understood by MCP stdio clients and caused connection/initialization to
  hang. Input still accepts both framings for compatibility.
- Added a `write_message` test asserting newline-delimited output without
  `Content-Length` headers.

## [1.6.1] - 2026-06-23

### Fixed
- MCP stdio input parsing now supports both `Content-Length` framed messages
  and newline-delimited raw JSON-RPC messages, preventing initialization
  timeouts with MCP clients that do not use LSP-style headers.
- Added MCP parser tests for both input formats to protect against regressions.

## [1.6.0] - 2026-06-23

### Added
- **MCP server (stdio transport)** via `mem-cli mcp`
  ([RFC-0005](docs/rfcs/0005.md)).
- MCP tools (v1): `list_facts`, `list_decisions`, `list_modules`,
  `list_commands`, `find_by_tag`, `add_fact`, `add_decision`, `add_module`,
  `add_command`, `tag_record`, `untag_record`, `doctor`, and `ping`.
- MCP resources: `mem://facts`, `mem://decisions`, `mem://modules`,
  `mem://commands` (through `resources/list` and `resources/read`).
- Unit tests for MCP request handling, tool flow, and resource listing.

## [1.5.0] - 2026-06-10

### Added
- **Tags** — lightweight thematic labels for any record of any entity
  ([RFC-0004](docs/rfcs/0004.md)):
  - `tag <entity> <id> <tag>...` — attach one or more tags (normalized to
    `[a-z0-9._-]`, lowercase, max 64 chars; idempotent, reactivates a
    soft-removed tag).
  - `untag <entity> <id> <tag>` — soft-remove a tag (`--hard` to purge).
  - `find <tag> [--entity <e>] [--json]` — recall a topic in one slice across
    all entities, grouped by entity (hides soft-deleted records and tags).
  - `tags [--json]` — overview of all tags with active record counts.
  - `doctor [--json] [--fix]` — diagnose tag/DB integrity (dangling tags, invalid
    entity, dirty tags, tags on soft-deleted records, schema version, record
    counts); exits non-zero on problems; `--fix` removes dangling and
    invalid-entity tags.
- DB schema migration v2: new `tags` table with indexes. `purge` now
  cascade-deletes a record's tags in the same transaction.
- Unit tests for tag normalization, idempotent tagging/reactivation, `find`
  grouping, tag overview counts, purge cascade, and `doctor` detection/repair.

## [1.4.0] - 2026-06-09

### Changed
- All in-code text (string literals — CLI output and error messages — and
  comments) is now in English. The `AGENTS.md` managed block written by `init`
  is in English as well. No behavior changes.

## [1.3.0] - 2026-06-09

### Added
- `info` — print the resolved DB path and status (`OK` when the DB file exists,
  otherwise a hint to run `mem-cli init`; surfaces marker/resolution errors).

## [1.2.0] - 2026-06-09

### Added
- `init [NAME]` now provisions a project: generates a `.mem-project` marker with
  a stable slug (`<name-slug>-<random-id>`), creates the DB, and appends a
  managed block to `AGENTS.md` documenting where context is stored.
- Project discovery: `.mem-project` marker is searched upward from the cwd (like
  `.git`), so commands work from any subdirectory.
- Backward-compatible migration: `init` copies an existing `./.memory/`
  database into the new per-user location when present.
- Unit tests for `slugify` (incl. Cyrillic), slug validation, marker discovery,
  and DB-dir resolution precedence.

### Changed
- **Default DB location moved out of the repository.** Resolution precedence is
  now `MEMORY_DB_DIR` (override) → `.mem-project` marker
  (`${XDG_DATA_HOME:-~/.local/share}/mem/<slug>/`) → `./.memory/` (legacy
  fallback). Implements [RFC-0002](docs/rfcs/0002.md).
- A corrupt/invalid `.mem-project` marker now fails loudly instead of silently
  falling back.

### Fixed
- `db_dir()` no longer depends on a relative `./.memory/` path that varied by
  current working directory.

## [1.1.0] - 2026-06-09

### Added
- `update <entity> <id> <content>` — replace a record by soft-deleting the old
  one and adding a new one; `--hard` flag purges the old record instead. The
  operation is atomic (single transaction). No-op if there is no active record
  with the given id.
- Unit tests for soft/hard update and the missing-record no-op case.
- `Makefile` with targets: `build`, `release`, `test`, `fmt`, `fmt-check`,
  `clippy`, `lint`, `run`, `clean`, `install`.

## [1.0.0] - 2026-06-09

### Added
- `README.md` with installation, usage, storage layout, development commands,
  and a "Copilot Integration" section.

### Changed
- RFC-0001 marked as implemented.

## [0.5.0] - 2026-06-09

### Added
- `delete <entity> <id>` — soft delete (sets `deleted_at`, hides from `list`).
- `purge <entity> <id>` — hard delete (physically removes the row).
- Unit tests for soft delete and purge behaviour.

## [0.4.0] - 2026-06-09

### Added
- `decisions` and `modules` subcommands as quick aliases for
  `list decisions` / `list modules` (support `--json`).

## [0.3.0] - 2026-06-09

### Added
- `add <entity> <content>` command to insert records.
- `list <entity>` command with table output (default) and `--json` flag.
- Generic data-access layer (`insert`, `list_active`) with a table whitelist
  to guard against SQL injection.
- `serde_json` dependency for JSON output.

## [0.2.0] - 2026-06-09

### Added
- `db` module: database path resolution via `MEMORY_DB_DIR` env var
  (defaults to `.memory/`), connection opening with directory creation.
- Schema versioning (`schema_version` table) and idempotent migrations.
- Entity tables `facts`, `decisions`, `commands`, `modules` with common
  fields (`id`, `content`, `created_at`, `updated_at`, `deleted_at`).
- `mem-cli init` command — creates the database and applies the schema.

## [0.1.0] - 2026-06-09

### Added
- Project skeleton (RFC-0001 / PLAN-0001).
- Dependencies: `rusqlite` (bundled), `clap` (derive), `anyhow`.
- CLI skeleton with subcommands: `init`, `add`, `list`, `decisions`,
  `modules`, `delete`, `purge`.
- `.gitignore` for `target/`, `*.db`, `.memory/`.
