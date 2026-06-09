use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use rusqlite::Connection;

/// Current DB schema version.
pub const SCHEMA_VERSION: i64 = 1;

/// DB file name.
const DB_FILE: &str = "project_context.db";

/// Environment variable with the DB directory (highest-priority override).
const ENV_DB_DIR: &str = "MEMORY_DB_DIR";

/// Project marker file name in the repository root (stores the slug).
pub const MARKER_FILE: &str = ".mem-project";

/// Storage subdirectory inside the base data directory.
const DATA_SUBDIR: &str = "mem";

/// Maximum length of a slug produced from a name.
const SLUG_MAX_LEN: usize = 48;

/// User base data directory: `XDG_DATA_HOME`, otherwise `~/.local/share`.
/// `None` if it cannot be determined (no `HOME`).
fn data_home() -> Option<PathBuf> {
    if let Some(x) = env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(&x);
        if p.is_absolute() {
            return Some(p);
        }
    }
    env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
}

/// Convert a name into a filesystem-safe slug. Keeps unicode letters and digits
/// (including Cyrillic), replaces other characters with `-`. Empty result → `project`.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.trim().chars() {
        if ch.is_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let slug: String = out.trim_matches('-').chars().take(SLUG_MAX_LEN).collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "project".to_string()
    } else {
        slug.to_string()
    }
}

/// Check that the slug is safe as a single path component.
fn slug_is_valid(slug: &str) -> bool {
    !slug.is_empty()
        && slug != "."
        && slug != ".."
        && slug.chars().count() <= 128
        && slug
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Generate a random hexadecimal identifier (16 characters).
pub fn random_id() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut h);
    std::process::id().hash(&mut h);
    let probe = 0u8;
    (std::ptr::addr_of!(probe) as usize).hash(&mut h);
    std::time::SystemTime::now().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Find the `.mem-project` marker up the tree from `start`.
/// Returns the marker directory and the read slug. A corrupt marker is an error.
pub fn find_marker(start: &Path) -> Result<Option<(PathBuf, String)>> {
    let mut cur = start.to_path_buf();
    loop {
        let marker = cur.join(MARKER_FILE);
        if marker.is_file() {
            let content = fs::read_to_string(&marker)
                .with_context(|| format!("failed to read the marker: {}", marker.display()))?;
            let slug = content.trim().to_string();
            if !slug_is_valid(&slug) {
                return Err(anyhow!(
                    "corrupt marker {}: invalid slug {:?}",
                    marker.display(),
                    slug
                ));
            }
            return Ok(Some((cur, slug)));
        }
        if !cur.pop() {
            return Ok(None);
        }
    }
}

/// Pure DB-directory selection logic (reads no globals — for testability).
///
/// Priority: `env_dir` (override) > project marker up the tree > `.memory`.
fn resolve_db_dir_from(
    env_dir: Option<&OsStr>,
    start: &Path,
    data_home: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(dir) = env_dir
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    if let Some((_, slug)) = find_marker(start)? {
        let base = data_home.ok_or_else(|| {
            anyhow!("failed to determine the data directory (HOME/XDG_DATA_HOME)")
        })?;
        return Ok(base.join(DATA_SUBDIR).join(slug));
    }
    Ok(PathBuf::from(".memory"))
}

/// DB storage directory considering the override, the project marker and the default.
pub fn db_dir() -> Result<PathBuf> {
    let env_dir = env::var_os(ENV_DB_DIR);
    let cwd = env::current_dir().context("failed to determine the current directory")?;
    let home = data_home();
    resolve_db_dir_from(env_dir.as_deref(), &cwd, home.as_deref())
}

/// Full path to the DB file.
pub fn db_path() -> Result<PathBuf> {
    Ok(db_dir()?.join(DB_FILE))
}

/// Open (creating the directory and file if needed) a connection to the DB.
pub fn open() -> Result<Connection> {
    let dir = db_dir()?;
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create DB directory: {}", dir.display()))?;
    let path = dir.join(DB_FILE);
    let conn = Connection::open(&path)
        .with_context(|| format!("failed to open the DB: {}", path.display()))?;
    Ok(conn)
}

/// Apply schema migrations (idempotent).
pub fn apply_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);")?;

    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;

    if current < 1 {
        conn.execute_batch(MIGRATION_V1)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])?;
    }

    Ok(())
}

/// Migration v1: create the entity tables with shared fields.
const MIGRATION_V1: &str = r#"
CREATE TABLE IF NOT EXISTS facts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    content    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at TEXT
);
CREATE TABLE IF NOT EXISTS decisions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    content    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at TEXT
);
CREATE TABLE IF NOT EXISTS commands (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    content    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at TEXT
);
CREATE TABLE IF NOT EXISTS modules (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    content    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at TEXT
);
"#;

/// Whitelist of allowed entity tables (for safe substitution into SQL).
pub const ENTITY_TABLES: [&str; 4] = ["facts", "decisions", "commands", "modules"];

/// An entity record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: i64,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// Check that the table name is in the whitelist.
fn ensure_valid_table(table: &str) -> Result<()> {
    if ENTITY_TABLES.contains(&table) {
        Ok(())
    } else {
        Err(anyhow::anyhow!("unknown entity: {table}"))
    }
}

/// Add a record to an entity. Returns the id of the new row.
pub fn insert(conn: &Connection, table: &str, content: &str) -> Result<i64> {
    ensure_valid_table(table)?;
    let sql = format!("INSERT INTO {table} (content) VALUES (?1)");
    conn.execute(&sql, [content])?;
    Ok(conn.last_insert_rowid())
}

/// Return active (non-deleted) records of an entity, ordered by ascending id.
pub fn list_active(conn: &Connection, table: &str) -> Result<Vec<Record>> {
    ensure_valid_table(table)?;
    let sql = format!(
        "SELECT id, content, created_at, updated_at, deleted_at \
         FROM {table} WHERE deleted_at IS NULL ORDER BY id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(Record {
            id: r.get(0)?,
            content: r.get(1)?,
            created_at: r.get(2)?,
            updated_at: r.get(3)?,
            deleted_at: r.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Soft delete: set `deleted_at` for an active record.
/// Returns the number of affected rows.
pub fn soft_delete(conn: &Connection, table: &str, id: i64) -> Result<usize> {
    ensure_valid_table(table)?;
    let sql = format!(
        "UPDATE {table} SET deleted_at = datetime('now'), updated_at = datetime('now') \
         WHERE id = ?1 AND deleted_at IS NULL"
    );
    Ok(conn.execute(&sql, [id])?)
}

/// Hard delete: physically remove a row. Returns the number of deleted rows.
pub fn purge(conn: &Connection, table: &str, id: i64) -> Result<usize> {
    ensure_valid_table(table)?;
    let sql = format!("DELETE FROM {table} WHERE id = ?1");
    Ok(conn.execute(&sql, [id])?)
}

/// Update a record: remove the old one (`soft_delete`, or `purge` when `hard`)
/// and add a new one with the given `content`. The operation is atomic (one transaction).
/// Returns the id of the new record, or `None` if there is nothing to update
/// (no active record with the given id).
pub fn update(
    conn: &Connection,
    table: &str,
    id: i64,
    content: &str,
    hard: bool,
) -> Result<Option<i64>> {
    ensure_valid_table(table)?;
    let tx = conn.unchecked_transaction()?;
    let affected = if hard {
        purge(&tx, table, id)?
    } else {
        soft_delete(&tx, table, id)?
    };
    if affected == 0 {
        return Ok(None);
    }
    let new_id = insert(&tx, table, content)?;
    tx.commit()?;
    Ok(Some(new_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_handles_unicode_and_separators() {
        assert_eq!(slugify("My Project"), "my-project");
        assert_eq!(slugify("a/b\\c:d"), "a-b-c-d");
        assert_eq!(slugify("  multiple   spaces  "), "multiple-spaces");
        assert_eq!(slugify("dots...and---dashes"), "dots-and-dashes");
        // Cyrillic is preserved (unicode-aware), case is lowered.
        assert_eq!(slugify("Мой Проект"), "мой-проект");
        // Separators only → fallback.
        assert_eq!(slugify("///   ///"), "project");
        assert_eq!(slugify(""), "project");
    }

    #[test]
    fn slug_validation() {
        assert!(slug_is_valid("my-proj-a1b2c3"));
        assert!(slug_is_valid("проект_1"));
        assert!(!slug_is_valid(""));
        assert!(!slug_is_valid(".."));
        assert!(!slug_is_valid("a/b"));
        assert!(!slug_is_valid("a b"));
        assert!(!slug_is_valid("a\nb"));
    }

    #[test]
    fn find_marker_walks_up_tree() {
        let base = std::env::temp_dir().join(format!("memtest-{}", random_id()));
        let nested = base.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(base.join(MARKER_FILE), "my-proj-deadbeef\n").unwrap();

        let (root, slug) = find_marker(&nested)
            .unwrap()
            .expect("marker should be found");
        assert_eq!(root, base);
        assert_eq!(slug, "my-proj-deadbeef");

        // Corrupt slug — error.
        fs::write(base.join(MARKER_FILE), "bad/slug").unwrap();
        assert!(find_marker(&nested).is_err());

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn find_marker_absent_returns_none() {
        let base = std::env::temp_dir().join(format!("memtest-{}", random_id()));
        fs::create_dir_all(&base).unwrap();
        assert!(find_marker(&base).unwrap().is_none());
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn resolve_db_dir_precedence() {
        let data = std::env::temp_dir().join(format!("memdata-{}", random_id()));

        // 1. env override beats everything.
        let with_marker = std::env::temp_dir().join(format!("memtest-{}", random_id()));
        fs::create_dir_all(&with_marker).unwrap();
        fs::write(with_marker.join(MARKER_FILE), "proj-abc123").unwrap();
        let p = resolve_db_dir_from(Some(OsStr::new("/tmp/override")), &with_marker, Some(&data))
            .unwrap();
        assert_eq!(p, PathBuf::from("/tmp/override"));

        // 2. Marker → data directory/mem/<slug>.
        let p = resolve_db_dir_from(None, &with_marker, Some(&data)).unwrap();
        assert_eq!(p, data.join("mem").join("proj-abc123"));

        // 3. No marker and no env → .memory.
        let no_marker = std::env::temp_dir().join(format!("memtest-{}", random_id()));
        fs::create_dir_all(&no_marker).unwrap();
        let p = resolve_db_dir_from(None, &no_marker, Some(&data)).unwrap();
        assert_eq!(p, PathBuf::from(".memory"));

        fs::remove_dir_all(&with_marker).ok();
        fs::remove_dir_all(&no_marker).ok();
    }

    #[test]
    fn migrations_create_schema_and_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        apply_migrations(&conn).unwrap();

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        for table in ["facts", "decisions", "commands", "modules"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} should exist");
        }
    }

    #[test]
    fn insert_and_list_active() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();

        let id1 = insert(&conn, "facts", "first fact").unwrap();
        let id2 = insert(&conn, "facts", "second fact").unwrap();
        assert!(id2 > id1);

        let records = list_active(&conn, "facts").unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].content, "first fact");
        assert_eq!(records[1].content, "second fact");
        assert!(records[0].deleted_at.is_none());
    }

    #[test]
    fn insert_rejects_unknown_table() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        assert!(insert(&conn, "users; DROP TABLE facts", "x").is_err());
    }

    #[test]
    fn soft_delete_hides_from_list() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        let id = insert(&conn, "facts", "to delete").unwrap();

        assert_eq!(soft_delete(&conn, "facts", id).unwrap(), 1);
        assert!(list_active(&conn, "facts").unwrap().is_empty());
        // A repeated soft delete affects no rows.
        assert_eq!(soft_delete(&conn, "facts", id).unwrap(), 0);

        // The row is still physically present.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn purge_removes_row() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        let id = insert(&conn, "facts", "to purge").unwrap();

        assert_eq!(purge(&conn, "facts", id).unwrap(), 1);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(purge(&conn, "facts", id).unwrap(), 0);
    }

    #[test]
    fn update_soft_replaces_record() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        let old = insert(&conn, "commands", "cargo build").unwrap();

        let new = update(&conn, "commands", old, "make build", false)
            .unwrap()
            .expect("a new record should be returned");
        assert!(new > old);

        let records = list_active(&conn, "commands").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, new);
        assert_eq!(records[0].content, "make build");

        // The old row remains physically but is soft-deleted.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn update_hard_purges_old_record() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        let old = insert(&conn, "commands", "cargo build").unwrap();

        let new = update(&conn, "commands", old, "make build", true)
            .unwrap()
            .expect("a new record should be returned");
        assert!(new > old);

        // The old row is physically removed, only the new one remains.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let records = list_active(&conn, "commands").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].content, "make build");
    }

    #[test]
    fn update_missing_record_is_noop() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();

        // No record with this id — nothing to update, nothing is added.
        assert!(update(&conn, "facts", 42, "x", false).unwrap().is_none());
        assert!(update(&conn, "facts", 42, "x", true).unwrap().is_none());
        assert!(list_active(&conn, "facts").unwrap().is_empty());

        // An already soft-deleted record is also not updated in soft mode.
        let id = insert(&conn, "facts", "a").unwrap();
        soft_delete(&conn, "facts", id).unwrap();
        assert!(update(&conn, "facts", id, "b", false).unwrap().is_none());
    }
}
