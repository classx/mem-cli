use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension};

/// Current DB schema version.
pub const SCHEMA_VERSION: i64 = 2;

/// Maximum length of a normalized tag.
pub const TAG_MAX_LEN: usize = 64;

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

    if current < 2 {
        conn.execute_batch(MIGRATION_V2)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (2)", [])?;
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

/// Migration v2: create the tags table (thematic labels for records) and indexes.
const MIGRATION_V2: &str = r#"
CREATE TABLE IF NOT EXISTS tags (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    entity     TEXT    NOT NULL,
    record_id  INTEGER NOT NULL,
    tag        TEXT    NOT NULL,
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    deleted_at TEXT,
    UNIQUE (entity, record_id, tag)
);
CREATE INDEX IF NOT EXISTS idx_tags_tag    ON tags (tag);
CREATE INDEX IF NOT EXISTS idx_tags_record ON tags (entity, record_id);
"#;
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

/// Hard delete: physically remove a row and cascade-delete its tags.
/// Returns the number of deleted entity rows.
pub fn purge(conn: &Connection, table: &str, id: i64) -> Result<usize> {
    ensure_valid_table(table)?;
    conn.execute(
        "DELETE FROM tags WHERE entity = ?1 AND record_id = ?2",
        rusqlite::params![table, id],
    )?;
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

/// A tag is valid if it matches `[a-z0-9._-]+` (ASCII).
fn tag_char_is_valid(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
}

/// Normalize and validate a tag: trim, lowercase (ASCII), enforce the allowed
/// character set and length. Returns an error for an empty or invalid tag.
pub fn normalize_tag(tag: &str) -> Result<String> {
    let normalized = tag.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(anyhow!("empty tag"));
    }
    if normalized.chars().count() > TAG_MAX_LEN {
        return Err(anyhow!(
            "tag is too long (max {TAG_MAX_LEN} characters): {normalized:?}"
        ));
    }
    if !normalized.chars().all(tag_char_is_valid) {
        return Err(anyhow!(
            "invalid tag {tag:?}: allowed characters are [a-z0-9._-]"
        ));
    }
    Ok(normalized)
}

/// Check that an active (non-deleted) record exists in the entity table.
fn record_is_active(conn: &Connection, table: &str, id: i64) -> Result<bool> {
    ensure_valid_table(table)?;
    let sql = format!("SELECT 1 FROM {table} WHERE id = ?1 AND deleted_at IS NULL");
    let found = conn.query_row(&sql, [id], |_| Ok(())).optional()?;
    Ok(found.is_some())
}

/// Outcome of adding a tag to a record.
#[derive(Debug, PartialEq, Eq)]
pub enum TagOutcome {
    /// The tag was newly attached (or a soft-deleted tag was reactivated).
    Added,
    /// The tag was already present and active (no-op).
    AlreadyPresent,
    /// No active record with the given id exists in the entity.
    NoRecord,
}

/// Attach a (normalized) tag to an active record. Idempotent: an already-active
/// tag is a no-op; a previously soft-deleted tag is reactivated.
pub fn add_tag(conn: &Connection, table: &str, record_id: i64, tag: &str) -> Result<TagOutcome> {
    ensure_valid_table(table)?;
    let tag = normalize_tag(tag)?;
    if !record_is_active(conn, table, record_id)? {
        return Ok(TagOutcome::NoRecord);
    }
    let active: Option<i64> = conn
        .query_row(
            "SELECT id FROM tags \
             WHERE entity = ?1 AND record_id = ?2 AND tag = ?3 AND deleted_at IS NULL",
            rusqlite::params![table, record_id, tag],
            |r| r.get(0),
        )
        .optional()?;
    if active.is_some() {
        return Ok(TagOutcome::AlreadyPresent);
    }
    conn.execute(
        "INSERT INTO tags (entity, record_id, tag) VALUES (?1, ?2, ?3) \
         ON CONFLICT (entity, record_id, tag) DO UPDATE SET deleted_at = NULL",
        rusqlite::params![table, record_id, tag],
    )?;
    Ok(TagOutcome::Added)
}

/// Remove a tag from a record (soft by default, hard physically deletes the row).
/// Returns the number of affected tag rows.
pub fn remove_tag(
    conn: &Connection,
    table: &str,
    record_id: i64,
    tag: &str,
    hard: bool,
) -> Result<usize> {
    ensure_valid_table(table)?;
    let tag = normalize_tag(tag)?;
    let affected = if hard {
        conn.execute(
            "DELETE FROM tags WHERE entity = ?1 AND record_id = ?2 AND tag = ?3",
            rusqlite::params![table, record_id, tag],
        )?
    } else {
        conn.execute(
            "UPDATE tags SET deleted_at = datetime('now') \
             WHERE entity = ?1 AND record_id = ?2 AND tag = ?3 AND deleted_at IS NULL",
            rusqlite::params![table, record_id, tag],
        )?
    };
    Ok(affected)
}

/// Active records carrying the (normalized) tag, grouped per entity. Soft-deleted
/// records and soft-deleted tags are excluded. `entity_filter` narrows to one entity.
pub fn find_by_tag(
    conn: &Connection,
    tag: &str,
    entity_filter: Option<&str>,
) -> Result<Vec<(String, Vec<Record>)>> {
    let tag = normalize_tag(tag)?;
    let tables: Vec<&str> = match entity_filter {
        Some(t) => {
            ensure_valid_table(t)?;
            vec![t]
        }
        None => ENTITY_TABLES.to_vec(),
    };
    let mut out = Vec::new();
    for table in tables {
        let sql = format!(
            "SELECT e.id, e.content, e.created_at, e.updated_at, e.deleted_at \
             FROM {table} e JOIN tags t ON t.entity = ?1 AND t.record_id = e.id \
             WHERE t.tag = ?2 AND t.deleted_at IS NULL AND e.deleted_at IS NULL \
             ORDER BY e.id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![table, tag], |r| {
            Ok(Record {
                id: r.get(0)?,
                content: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                deleted_at: r.get(4)?,
            })
        })?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        if !records.is_empty() {
            out.push((table.to_string(), records));
        }
    }
    Ok(out)
}

/// Overview of all active tags with the count of active records carrying each.
pub fn tags_overview(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let exists_clauses: Vec<String> = ENTITY_TABLES
        .iter()
        .map(|table| {
            format!(
                "(t.entity = '{table}' AND EXISTS \
                 (SELECT 1 FROM {table} e WHERE e.id = t.record_id AND e.deleted_at IS NULL))"
            )
        })
        .collect();
    let sql = format!(
        "SELECT t.tag, COUNT(*) AS cnt FROM tags t \
         WHERE t.deleted_at IS NULL AND ({}) \
         GROUP BY t.tag ORDER BY t.tag",
        exists_clauses.join(" OR ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// A single integrity issue found by [`doctor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagIssue {
    pub id: i64,
    pub entity: String,
    pub record_id: i64,
    pub tag: String,
}

/// Integrity and diagnostics report for the tag store and the DB in general.
#[derive(Debug, Default)]
pub struct DoctorReport {
    pub schema_version: i64,
    /// Active tag rows whose record was purged (record_id not present).
    pub dangling: Vec<TagIssue>,
    /// Active tag rows whose `entity` is not in the whitelist.
    pub invalid_entity: Vec<TagIssue>,
    /// Active tag rows whose tag would not pass normalization.
    pub dirty: Vec<TagIssue>,
    /// Active tag rows attached to soft-deleted records (informational).
    pub on_soft_deleted: Vec<TagIssue>,
    /// Active record counts per entity.
    pub active_counts: Vec<(String, i64)>,
    /// Soft-deleted record counts per entity.
    pub soft_deleted_counts: Vec<(String, i64)>,
}

impl DoctorReport {
    /// Whether there are integrity problems (excluding informational findings).
    pub fn has_problems(&self) -> bool {
        !self.dangling.is_empty() || !self.invalid_entity.is_empty() || !self.dirty.is_empty()
    }
}

/// Read all active tag rows as issues (used by the diagnostics below).
fn active_tag_rows(conn: &Connection) -> Result<Vec<TagIssue>> {
    let mut stmt = conn.prepare(
        "SELECT id, entity, record_id, tag FROM tags WHERE deleted_at IS NULL ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TagIssue {
            id: r.get(0)?,
            entity: r.get(1)?,
            record_id: r.get(2)?,
            tag: r.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Run integrity checks over the tag store and collect general DB diagnostics.
pub fn doctor(conn: &Connection) -> Result<DoctorReport> {
    let mut report = DoctorReport {
        schema_version: conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )?,
        ..Default::default()
    };

    for issue in active_tag_rows(conn)? {
        if !ENTITY_TABLES.contains(&issue.entity.as_str()) {
            report.invalid_entity.push(issue);
            continue;
        }
        if normalize_tag(&issue.tag)
            .map(|n| n != issue.tag)
            .unwrap_or(true)
        {
            report.dirty.push(issue.clone());
        }
        let sql = format!(
            "SELECT deleted_at FROM {} WHERE id = ?1",
            issue.entity.as_str()
        );
        let state: Option<Option<String>> = conn
            .query_row(&sql, [issue.record_id], |r| r.get::<_, Option<String>>(0))
            .optional()?;
        match state {
            None => report.dangling.push(issue),
            Some(Some(_)) => report.on_soft_deleted.push(issue),
            Some(None) => {}
        }
    }

    for table in ENTITY_TABLES {
        let active: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE deleted_at IS NULL"),
            [],
            |r| r.get(0),
        )?;
        let soft: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE deleted_at IS NOT NULL"),
            [],
            |r| r.get(0),
        )?;
        report.active_counts.push((table.to_string(), active));
        report.soft_deleted_counts.push((table.to_string(), soft));
    }

    Ok(report)
}

/// Safe auto-repair: physically remove dangling tags and tags with an invalid
/// `entity`, in a single transaction. Returns the number of removed tag rows.
pub fn doctor_fix(conn: &Connection, report: &DoctorReport) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut removed = 0;
    {
        let mut stmt = tx.prepare("DELETE FROM tags WHERE id = ?1")?;
        for issue in report.dangling.iter().chain(report.invalid_entity.iter()) {
            removed += stmt.execute([issue.id])?;
        }
    }
    tx.commit()?;
    Ok(removed)
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

    #[test]
    fn normalize_tag_rules() {
        assert_eq!(normalize_tag("  Auth ").unwrap(), "auth");
        assert_eq!(normalize_tag("API-v2.0_x").unwrap(), "api-v2.0_x");
        assert!(normalize_tag("   ").is_err());
        assert!(normalize_tag("with space").is_err());
        assert!(normalize_tag("привет").is_err());
        assert!(normalize_tag(&"a".repeat(TAG_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn add_tag_is_idempotent_and_reactivates() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        let id = insert(&conn, "facts", "f").unwrap();

        assert_eq!(
            add_tag(&conn, "facts", id, "Auth").unwrap(),
            TagOutcome::Added
        );
        // Normalized duplicate is a no-op.
        assert_eq!(
            add_tag(&conn, "facts", id, "auth").unwrap(),
            TagOutcome::AlreadyPresent
        );
        // Only one physical row.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Soft-remove then re-add reactivates the same row.
        assert_eq!(remove_tag(&conn, "facts", id, "auth", false).unwrap(), 1);
        assert_eq!(
            add_tag(&conn, "facts", id, "auth").unwrap(),
            TagOutcome::Added
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn add_tag_requires_active_record() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        assert_eq!(
            add_tag(&conn, "facts", 999, "auth").unwrap(),
            TagOutcome::NoRecord
        );
        let id = insert(&conn, "facts", "f").unwrap();
        soft_delete(&conn, "facts", id).unwrap();
        assert_eq!(
            add_tag(&conn, "facts", id, "auth").unwrap(),
            TagOutcome::NoRecord
        );
    }

    #[test]
    fn add_tag_rejects_unknown_table() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        assert!(add_tag(&conn, "users; DROP TABLE facts", 1, "auth").is_err());
    }

    #[test]
    fn find_by_tag_groups_and_hides_deleted() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        let f = insert(&conn, "facts", "fact-auth").unwrap();
        let d = insert(&conn, "decisions", "dec-auth").unwrap();
        let f2 = insert(&conn, "facts", "fact-other").unwrap();
        add_tag(&conn, "facts", f, "auth").unwrap();
        add_tag(&conn, "decisions", d, "auth").unwrap();
        add_tag(&conn, "facts", f2, "auth").unwrap();
        // Soft-delete one record → its tag hidden from find.
        soft_delete(&conn, "facts", f2).unwrap();

        let groups = find_by_tag(&conn, "AUTH", None).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "facts");
        assert_eq!(groups[0].1.len(), 1);
        assert_eq!(groups[0].1[0].content, "fact-auth");
        assert_eq!(groups[1].0, "decisions");

        // Entity filter narrows to one entity.
        let only = find_by_tag(&conn, "auth", Some("facts")).unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].0, "facts");
    }

    #[test]
    fn tags_overview_counts_active_only() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        let f = insert(&conn, "facts", "f").unwrap();
        let d = insert(&conn, "decisions", "d").unwrap();
        add_tag(&conn, "facts", f, "auth").unwrap();
        add_tag(&conn, "decisions", d, "auth").unwrap();
        add_tag(&conn, "facts", f, "db").unwrap();
        soft_delete(&conn, "decisions", d).unwrap();

        let overview = tags_overview(&conn).unwrap();
        assert_eq!(
            overview,
            vec![("auth".to_string(), 1), ("db".to_string(), 1)]
        );
    }

    #[test]
    fn purge_cascades_tags() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        let id = insert(&conn, "facts", "f").unwrap();
        add_tag(&conn, "facts", id, "auth").unwrap();
        assert_eq!(purge(&conn, "facts", id).unwrap(), 1);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn doctor_detects_and_fixes_issues() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        let f = insert(&conn, "facts", "f").unwrap();
        add_tag(&conn, "facts", f, "auth").unwrap();

        // Dangling tag: points to a missing record.
        conn.execute(
            "INSERT INTO tags (entity, record_id, tag) VALUES ('facts', 999, 'ghost')",
            [],
        )
        .unwrap();
        // Invalid entity (manual DB edit).
        conn.execute(
            "INSERT INTO tags (entity, record_id, tag) VALUES ('bogus', 1, 'x')",
            [],
        )
        .unwrap();
        // Dirty tag: would not pass normalization.
        conn.execute(
            "INSERT INTO tags (entity, record_id, tag) VALUES ('facts', ?1, 'Bad Tag')",
            [f],
        )
        .unwrap();

        let report = doctor(&conn).unwrap();
        assert_eq!(report.schema_version, SCHEMA_VERSION);
        assert_eq!(report.dangling.len(), 1);
        assert_eq!(report.invalid_entity.len(), 1);
        assert_eq!(report.dirty.len(), 1);
        assert!(report.has_problems());

        let removed = doctor_fix(&conn, &report).unwrap();
        assert_eq!(removed, 2); // dangling + invalid_entity

        let after = doctor(&conn).unwrap();
        assert!(after.dangling.is_empty());
        assert!(after.invalid_entity.is_empty());
        // Dirty tag is reported but not auto-removed.
        assert_eq!(after.dirty.len(), 1);
    }

    #[test]
    fn doctor_reports_tags_on_soft_deleted_as_info() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        let f = insert(&conn, "facts", "f").unwrap();
        add_tag(&conn, "facts", f, "auth").unwrap();
        soft_delete(&conn, "facts", f).unwrap();

        let report = doctor(&conn).unwrap();
        assert_eq!(report.on_soft_deleted.len(), 1);
        // Informational only — not an integrity problem.
        assert!(!report.has_problems());
    }
}
