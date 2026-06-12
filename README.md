# mem-cli

`mem-cli` is a minimal Rust + SQLite CLI for storing and quickly restoring long-term project context between sessions.

This project implements [RFC-0001](docs/rfcs/0001.md) (plan: [PLAN-0001](docs/PLAN-0001.md)).

## Quick Start

```sh
# 1) Build the binary
cargo build --release

# 2) (Optional) Make it available in PATH
cp target/release/mem-cli ~/.local/bin/

# 3) Initialize in your repository
mem-cli init "My Project"

# 4) Add and read context
mem-cli add facts "project uses Rust 2024 edition"
mem-cli list facts
```

## Features

Four context entities:

- `facts` — stable facts about the project;
- `decisions` — accepted technical decisions;
- `commands` — verified build/test/lint commands;
- `modules` — short descriptions of modules and their responsibilities.

Records can be grouped thematically with **tags** (any record, any entity), so a
whole topic (e.g. `auth`) can be recalled in one slice with `mem-cli find <tag>`.

## Installation

```sh
cargo build --release
# binary: target/release/mem-cli
```

Optionally copy the binary into a directory from your `PATH`:

```sh
cp target/release/mem-cli ~/.local/bin/
```

## Storage

The database file is `project_context.db`. Context is **personal** to each developer and is stored **outside the repository**. The directory is resolved in this order:

1. `MEMORY_DB_DIR` environment variable (override for tests/CI), if set.
2. Otherwise, by the `.mem-project` project marker (searched upward through parent directories, like `.git`): `${XDG_DATA_HOME:-~/.local/share}/mem/<slug>/`.
3. Otherwise (no marker), `.memory/` in the current directory (legacy).

The project slug (`<name>-<random-id>`) is written to `.mem-project` during `init` and then remains unchanged regardless of path changes or re-clones. The directory is created automatically.

The `.mem-project` file is **committed to the repository**: the slug is shared by the whole team, while each developer’s database remains local (in `$HOME`, outside the repository).

## Usage

```sh
# initialize the project: create .mem-project marker, DB, and schema
mem-cli init "Project Name"
# name is optional — by default, the repository root directory name is used
mem-cli init

# show the DB path and status (OK if the DB exists)
mem-cli info

# add records
mem-cli add facts "project uses Rust 2024 edition"
mem-cli add decisions "DB is stored in .memory/"
mem-cli add commands "cargo test"
mem-cli add modules "db: storage and migrations layer"

# list records (table output by default)
mem-cli list facts
# JSON output
mem-cli list facts --json

# quick aliases
mem-cli decisions
mem-cli modules --json

# deletion
mem-cli delete facts 1   # soft delete: sets deleted_at, hides from list
mem-cli purge  facts 1   # hard delete: physically removes the row

# update (delete the old record + add a new one)
mem-cli update commands 1 "make build"          # soft delete old + add
mem-cli update commands 1 "make build" --hard   # hard delete old + add

# tags: thematic labels for any record
mem-cli tag facts 1 auth security   # attach one or more tags
mem-cli untag facts 1 security      # soft-remove a tag (--hard to purge)

# context slice by topic (across all entities, grouped by entity)
mem-cli find auth
mem-cli find auth --entity facts    # narrow to one entity
mem-cli find auth --json

# overview of all tags with record counts
mem-cli tags

# diagnose tag/DB integrity (exit code 1 if problems found)
mem-cli doctor
mem-cli doctor --fix   # remove dangling tags and tags with an invalid entity
```

> The primary interface for working with data is `mem-cli`. Direct SQL queries to
> SQLite are allowed only for debugging and diagnostics.

## Copilot Integration

At the start of a session, an agent (for example, GitHub Copilot CLI) can read project context via `mem-cli`, then append new facts and decisions as work progresses.

1. Build the binary and put it in your `PATH` (see “Installation”).

2. Pin a stable DB directory for the project so it remains consistent across sessions (for example, in `.envrc` or your shell environment):

   ```sh
   export MEMORY_DB_DIR="$PWD/.memory"
   ```

3. Add agent instructions to `AGENTS.md` (or Copilot Custom Instructions) in the project root:

   ```markdown
   ## Project context (mem-cli)
   - At the start, read context:
     `mem-cli list facts`, `mem-cli decisions`, `mem-cli modules`,
     `mem-cli list commands`.
   - Save new stable facts: `mem-cli add facts "..."`.
   - Save accepted technical decisions: `mem-cli add decisions "..."`.
   - Save verified build/test/lint commands: `mem-cli add commands "..."`.
   - Recall a topic in one slice: `mem-cli find <tag>`; tag new records as you
     go: `mem-cli tag <entity> <id> <tag>`.
   - The DB directory is set via `MEMORY_DB_DIR` (default: `.memory/`).
   ```

4. Use the `--json` flag for machine-readable output:

   ```sh
   mem-cli list facts --json
   ```

## Development

```sh
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt
```
