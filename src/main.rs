use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use std::io::Write;
use std::path::{Path, PathBuf};

mod db;

/// mem-cli — long-lived project context (Rust + SQLite).
#[derive(Parser)]
#[command(name = "mem-cli", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the DB and apply the schema.
    Init {
        /// Project name (defaults to the repository root directory name).
        name: Option<String>,
    },
    /// Show the DB path and status (OK if the DB exists).
    Info,
    /// Add a record to an entity.
    Add { entity: Entity, content: String },
    /// List records of an entity.
    List {
        entity: Entity,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Quick access to decisions (alias for `list decisions`).
    Decisions {
        #[arg(long)]
        json: bool,
    },
    /// Quick access to modules (alias for `list modules`).
    Modules {
        #[arg(long)]
        json: bool,
    },
    /// Soft delete: set deleted_at.
    Delete { entity: Entity, id: i64 },
    /// Hard delete: physically remove the row.
    Purge { entity: Entity, id: i64 },
    /// Update a record: remove the old one and add a new one.
    Update {
        entity: Entity,
        id: i64,
        content: String,
        /// Hard-delete the old record (purge instead of soft delete).
        #[arg(long)]
        hard: bool,
    },
}

/// Supported entities.
#[derive(Copy, Clone, ValueEnum)]
enum Entity {
    Facts,
    Decisions,
    Commands,
    Modules,
}

impl Entity {
    /// Table name of the entity.
    fn table(self) -> &'static str {
        match self {
            Entity::Facts => "facts",
            Entity::Decisions => "decisions",
            Entity::Commands => "commands",
            Entity::Modules => "modules",
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { name } => cmd_init(name.as_deref()),
        Command::Info => cmd_info(),
        Command::Add { entity, content } => cmd_add(entity, &content),
        Command::List { entity, json } => cmd_list(entity, json),
        Command::Decisions { json } => cmd_list(Entity::Decisions, json),
        Command::Modules { json } => cmd_list(Entity::Modules, json),
        Command::Delete { entity, id } => cmd_delete(entity, id),
        Command::Purge { entity, id } => cmd_purge(entity, id),
        Command::Update {
            entity,
            id,
            content,
            hard,
        } => cmd_update(entity, id, &content, hard),
    }
}

/// `init` — initialize the project: create the marker (if needed), DB and schema.
fn cmd_init(name: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to determine the current directory")?;

    // If the project is already initialized (marker found up the tree) — reuse the slug.
    let (root, slug) = match db::find_marker(&cwd)? {
        Some((root, slug)) => (root, slug),
        None => {
            let root = repo_root(&cwd);
            let base = name
                .map(str::to_string)
                .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "project".to_string());
            let slug = format!("{}-{}", db::slugify(&base), db::random_id());
            write_marker(&root, &slug)?
        }
    };

    let db_path = db::db_path()?;

    // Backward compatibility: migrate the old ./.memory/ DB if the new one does not exist yet.
    let legacy = root.join(".memory").join("project_context.db");
    if legacy.is_file() && !db_path.exists() {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create DB directory: {}", parent.display()))?;
        }
        std::fs::copy(&legacy, &db_path)
            .with_context(|| format!("failed to migrate the old DB from {}", legacy.display()))?;
        println!("migrated existing DB from {}", legacy.display());
    }

    let conn = db::open()?;
    db::apply_migrations(&conn)?;

    update_agents_md(&root, &slug)?;

    println!(
        "DB ready: {} (slug={slug}, schema v{})",
        db::db_path()?.display(),
        db::SCHEMA_VERSION
    );
    Ok(())
}

/// `info` — show the DB path and status (OK if the DB file exists).
fn cmd_info() -> Result<()> {
    match db::db_path() {
        Ok(path) => {
            println!("DB path: {}", path.display());
            if path.is_file() {
                println!("Status: OK");
            } else if path.parent().map(Path::is_dir).unwrap_or(false) {
                println!("Status: DB not found (run `mem-cli init`)");
            } else {
                println!("Status: directory and DB are missing (run `mem-cli init`)");
            }
        }
        Err(e) => {
            println!("DB path: could not be determined");
            println!("Status: error — {e}");
        }
    }
    Ok(())
}

/// Find the repository root (directory containing `.git`), otherwise return `start`.
fn repo_root(start: &Path) -> PathBuf {
    let mut cur = start.to_path_buf();
    loop {
        if cur.join(".git").exists() {
            return cur;
        }
        if !cur.pop() {
            return start.to_path_buf();
        }
    }
}

/// Atomically write the marker with the slug. On a race — re-read the existing one.
fn write_marker(root: &Path, slug: &str) -> Result<(PathBuf, String)> {
    let marker = root.join(db::MARKER_FILE);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut f) => {
            writeln!(f, "{slug}")
                .with_context(|| format!("failed to write the marker: {}", marker.display()))?;
            Ok((root.to_path_buf(), slug.to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => db::find_marker(root)?
            .ok_or_else(|| anyhow::anyhow!("marker {} disappeared", marker.display())),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("failed to create the marker: {}", marker.display()))),
    }
}

const AGENTS_START: &str = "<!-- mem-cli:start -->";
const AGENTS_END: &str = "<!-- mem-cli:end -->";

/// Append (or update the managed block of) `AGENTS.md` with storage information.
fn update_agents_md(root: &Path, slug: &str) -> Result<()> {
    let path = root.join("AGENTS.md");
    let block = format!(
        "{AGENTS_START}\n## mem-cli context storage\n\n\
         Project context is stored locally per developer, outside the repository:\n\
         `${{XDG_DATA_HOME:-~/.local/share}}/mem/<slug>/project_context.db`.\n\
         The project slug (`{slug}`) is fixed in the `.mem-project` file.\n\
         The path can be overridden via the `MEMORY_DB_DIR` variable.\n{AGENTS_END}\n"
    );

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let new_content = match (existing.find(AGENTS_START), existing.find(AGENTS_END)) {
        (Some(s), Some(e)) if e > s => {
            let end = e + AGENTS_END.len();
            format!("{}{}{}", &existing[..s], block, &existing[end..])
        }
        _ if existing.trim().is_empty() => block,
        _ => format!("{}\n\n{}", existing.trim_end(), block),
    };
    std::fs::write(&path, new_content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// `add` — add a record to an entity.
fn cmd_add(entity: Entity, content: &str) -> Result<()> {
    let conn = db::open()?;
    db::apply_migrations(&conn)?;
    let id = db::insert(&conn, entity.table(), content)?;
    println!("added to {}: id={id}", entity.table());
    Ok(())
}

/// `list` — list active records of an entity.
fn cmd_list(entity: Entity, json: bool) -> Result<()> {
    let conn = db::open()?;
    db::apply_migrations(&conn)?;
    let records = db::list_active(&conn, entity.table())?;
    if json {
        print_json(&records);
    } else {
        print_table(&records);
    }
    Ok(())
}

/// `delete` — soft delete a record (set `deleted_at`).
fn cmd_delete(entity: Entity, id: i64) -> Result<()> {
    let conn = db::open()?;
    db::apply_migrations(&conn)?;
    let n = db::soft_delete(&conn, entity.table(), id)?;
    if n > 0 {
        println!("deleted (soft) from {}: id={id}", entity.table());
    } else {
        println!("no active record id={id} in {}", entity.table());
    }
    Ok(())
}

/// `purge` — physically remove a record.
fn cmd_purge(entity: Entity, id: i64) -> Result<()> {
    let conn = db::open()?;
    db::apply_migrations(&conn)?;
    let n = db::purge(&conn, entity.table(), id)?;
    if n > 0 {
        println!("deleted (purge) from {}: id={id}", entity.table());
    } else {
        println!("no record id={id} in {}", entity.table());
    }
    Ok(())
}

/// `update` — replace a record: soft delete (or purge with `--hard`) + add.
fn cmd_update(entity: Entity, id: i64, content: &str, hard: bool) -> Result<()> {
    let conn = db::open()?;
    db::apply_migrations(&conn)?;
    match db::update(&conn, entity.table(), id, content, hard)? {
        Some(new_id) => {
            let mode = if hard { "purge" } else { "soft" };
            println!(
                "updated ({mode}) in {}: id={id} -> id={new_id}",
                entity.table()
            );
        }
        None => {
            println!("no active record id={id} in {}", entity.table());
        }
    }
    Ok(())
}

/// Print records as JSON.
fn print_json(records: &[db::Record]) {
    let items: Vec<serde_json::Value> = records
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "content": r.content,
                "created_at": r.created_at,
                "updated_at": r.updated_at,
                "deleted_at": r.deleted_at,
            })
        })
        .collect();
    println!("{}", serde_json::Value::Array(items));
}

/// Print records as a simple table.
fn print_table(records: &[db::Record]) {
    if records.is_empty() {
        println!("(empty)");
        return;
    }
    println!("{:>4}  {:<19}  CONTENT", "ID", "CREATED");
    for r in records {
        println!("{:>4}  {:<19}  {}", r.id, r.created_at, r.content);
    }
}
