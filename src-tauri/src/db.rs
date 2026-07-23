//! SQLite skeleton (S23) — open DB, schema migrations, artifacts registry.
//!
//! Chats/projects still live in `data.json` until Phase B. This module must
//! not take the store mutex for long; failures are non-fatal to the app shell.

use crate::paths;
use rusqlite::{params, Connection};
use std::sync::Mutex;

/// Current schema version applied by [`migrate`].
pub const SCHEMA_VERSION: i32 = 1;

static DB: Mutex<Option<Connection>> = Mutex::new(None);

/// Open (or create) `swervebuild.db` and apply migrations. Safe to call often.
pub fn init() -> Result<(), String> {
    let path = paths::db_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(&path).map_err(|e| format!("open sqlite: {e}"))?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
        .map_err(|e| format!("pragma: {e}"))?;
    migrate(&conn)?;
    let mut guard = DB.lock().map_err(|_| "db lock poisoned".to_string())?;
    *guard = Some(conn);
    Ok(())
}

fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )
    .map_err(|e| format!("create schema_migrations: {e}"))?;

    let current: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if current < 1 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS artifacts (
                id TEXT PRIMARY KEY NOT NULL,
                kind TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE,
                bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                project_id TEXT,
                origin TEXT,
                purged INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_artifacts_kind ON artifacts(kind);
            CREATE INDEX IF NOT EXISTS idx_artifacts_created ON artifacts(created_at);",
        )
        .map_err(|e| format!("create artifacts: {e}"))?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![1, crate::store::Store::now()],
        )
        .map_err(|e| format!("record migration 1: {e}"))?;
    }

    let _ = SCHEMA_VERSION;
    Ok(())
}

/// Best-effort init for app start — logs via eprintln, never panics.
pub fn init_best_effort() {
    if let Err(e) = init() {
        eprintln!("[swervebuild] sqlite init: {e}");
    }
}

/// Run a closure with the shared connection, if open.
pub fn with_conn<T>(f: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
    let guard = DB.lock().map_err(|_| "db lock poisoned".to_string())?;
    let conn = guard
        .as_ref()
        .ok_or_else(|| "sqlite not open — call db::init".to_string())?;
    f(conn)
}

/// Upsert an artifact row (registry). Path is absolute display path.
pub fn upsert_artifact(
    id: &str,
    kind: &str,
    path: &str,
    bytes: u64,
    created_at: &str,
    project_id: Option<&str>,
    origin: Option<&str>,
) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO artifacts (id, kind, path, bytes, created_at, project_id, origin, purged)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
             ON CONFLICT(path) DO UPDATE SET
               bytes = excluded.bytes,
               kind = excluded.kind,
               purged = 0",
            params![
                id,
                kind,
                path,
                bytes as i64,
                created_at,
                project_id,
                origin
            ],
        )
        .map_err(|e| format!("upsert artifact: {e}"))?;
        Ok(())
    })
}

/// Mark path purged in the registry (file may already be deleted).
pub fn mark_purged(path: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "UPDATE artifacts SET purged = 1 WHERE path = ?1",
            params![path],
        )
        .map_err(|e| format!("mark purged: {e}"))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn migrate_creates_artifacts_table() {
        let _g = LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("swerve-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let conn = Connection::open(&path).unwrap();
        migrate(&conn).unwrap();
        let n: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='artifacts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        let ver: i32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, 1);
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
