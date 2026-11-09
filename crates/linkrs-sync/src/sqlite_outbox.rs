use std::path::Path;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

pub use crate::outbox_types::*;
use linkrs_core::types::{CommitLsn, LeaseEpoch, TargetId};
use linkrs_core::wal::{IndexMutation, OutboxIntent};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqliteConnection, SqlitePool};

/// Never update version in dev phase
const OUTBOX_SCHEMA_VERSION: i64 = 1;

mod degraded;
mod event;
mod frontier;
mod generation;
mod retention;
mod snapshot;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct SqliteOutbox {
    pool: SqlitePool,
    path: std::path::PathBuf,
}

impl SqliteOutbox {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let url = format!("sqlite://{}", path.as_ref().display());
        let options = SqliteConnectOptions::from_str(&url)
            .map_err(|error| error.to_string())?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full);
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await
            .map_err(|error| error.to_string())?;
        let outbox = Self {
            pool,
            path: path.as_ref().to_path_buf(),
        };
        outbox.migrate().await?;
        Ok(outbox)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn migrate(&self) -> Result<(), String> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
                version INTEGER PRIMARY KEY, applied_at_ms INTEGER NOT NULL\
            )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS events (\
                id INTEGER PRIMARY KEY AUTOINCREMENT,\
                transaction_id INTEGER NOT NULL,\
                intent_sequence INTEGER NOT NULL,\
                target TEXT NOT NULL,\
                index_id INTEGER NOT NULL,\
                generation INTEGER NOT NULL,\
                mutation BLOB NOT NULL,\
                commit_lsn INTEGER NOT NULL,\
                idempotency_key TEXT NOT NULL,\
                ordering_key TEXT NOT NULL,\
                status TEXT NOT NULL DEFAULT 'pending',\
                next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,\
                lease_owner TEXT,\
                lease_until_ms INTEGER NOT NULL DEFAULT 0,\
                lease_epoch INTEGER NOT NULL DEFAULT 0,\
                retry_count INTEGER NOT NULL DEFAULT 0,\
                created_at_ms INTEGER NOT NULL DEFAULT 0,\
                last_error TEXT,\
                UNIQUE(target, idempotency_key),\
                UNIQUE(transaction_id, intent_sequence)\
            )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        let has_created_at: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('events') WHERE name = 'created_at_ms')",
        )
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        if has_created_at == 0 {
            sqlx::query("ALTER TABLE events ADD COLUMN created_at_ms INTEGER NOT NULL DEFAULT 0")
                .execute(&mut *connection)
                .await
                .map_err(|error| error.to_string())?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS events_claim_idx \
             ON events(target, status, next_attempt_at_ms, commit_lsn, intent_sequence)",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS events_ordering_idx \
             ON events(target, ordering_key, commit_lsn, intent_sequence)",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS events_rebuild_idx \
             ON events(target, index_id, commit_lsn, intent_sequence)",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS commit_targets (\
                commit_lsn INTEGER NOT NULL,\
                target TEXT NOT NULL,\
                event_count INTEGER NOT NULL,\
                applied_count INTEGER NOT NULL DEFAULT 0,\
                completed INTEGER NOT NULL DEFAULT 0,\
                degraded INTEGER NOT NULL DEFAULT 0,\
                PRIMARY KEY(commit_lsn, target)\
            )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS idempotency (\
                target TEXT NOT NULL,\
                idempotency_key TEXT NOT NULL,\
                commit_lsn INTEGER NOT NULL,\
                PRIMARY KEY(target, idempotency_key)\
            )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS projection_state (\
                singleton INTEGER PRIMARY KEY CHECK(singleton = 1),\
                materialized_lsn INTEGER NOT NULL,\
                source_checkpoint INTEGER NOT NULL DEFAULT 0\
            )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT OR IGNORE INTO projection_state(singleton, materialized_lsn) VALUES(1, 0)",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS target_state (\
                target TEXT PRIMARY KEY,\
                applied_lsn INTEGER NOT NULL DEFAULT 0,\
                degraded INTEGER NOT NULL DEFAULT 0,\
                last_error TEXT\
            )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS generation_state (\
             target TEXT NOT NULL,\
             index_id INTEGER NOT NULL,\
             generation INTEGER NOT NULL,\
             state TEXT NOT NULL,\
             barrier_lsn INTEGER,\
             PRIMARY KEY(target, index_id, generation)\
             )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS index_frontier (\
             target TEXT NOT NULL,\
             index_id INTEGER NOT NULL,\
             generation INTEGER NOT NULL,\
             applied_lsn INTEGER NOT NULL DEFAULT 0,\
             degraded INTEGER NOT NULL DEFAULT 0,\
             PRIMARY KEY(target, index_id, generation)\
             )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS dead_letters (\
             event_id INTEGER PRIMARY KEY,\
             failed_at_ms INTEGER NOT NULL,\
             error TEXT NOT NULL,\
             FOREIGN KEY(event_id) REFERENCES events(id)\
             )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS degraded_ranges (\
              target TEXT NOT NULL,\
              index_id INTEGER NOT NULL,\
              generation INTEGER NOT NULL,\
              start_lsn INTEGER NOT NULL,\
              end_lsn INTEGER NOT NULL,\
              reason TEXT NOT NULL,\
              created_at_ms INTEGER NOT NULL,\
              PRIMARY KEY(target, index_id, generation, start_lsn, end_lsn)\
              )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        // Retention: add retention_lsn to projection_state, archive table, index.
        let has_retention_lsn: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('projection_state') WHERE name = 'retention_lsn')",
        )
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        if has_retention_lsn == 0 {
            sqlx::query(
                "ALTER TABLE projection_state ADD COLUMN retention_lsn INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS dead_letters_archive (\
              event_id INTEGER PRIMARY KEY,\
              target TEXT NOT NULL,\
              index_id INTEGER NOT NULL,\
              generation INTEGER NOT NULL,\
              commit_lsn INTEGER NOT NULL,\
              failed_at_ms INTEGER NOT NULL,\
              error TEXT NOT NULL,\
              archived_at_ms INTEGER NOT NULL\
              )",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS events_retention_idx ON events(status, commit_lsn)",
        )
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query("INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms) VALUES(?, 0)")
            .bind(OUTBOX_SCHEMA_VERSION)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

fn validate_intents(intents: &[OutboxIntent]) -> Result<(), String> {
    for (expected, intent) in intents.iter().enumerate() {
        intent.validate()?;
        if intent.intent_sequence as usize != expected {
            return Err(format!(
                "Intent sequence is not contiguous: expected {}, got {}",
                expected, intent.intent_sequence
            ));
        }
    }
    Ok(())
}

async fn begin_immediate(connection: &mut SqliteConnection) -> Result<(), String> {
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn finish_transaction<T>(
    connection: &mut SqliteConnection,
    result: Result<T, String>,
) -> Result<T, String> {
    match result {
        Ok(value) => {
            sqlx::query("COMMIT")
                .execute(&mut *connection)
                .await
                .map_err(|error| error.to_string())?;
            Ok(value)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

async fn advance_frontier(
    connection: &mut SqliteConnection,
    target: &TargetId,
) -> Result<(), String> {
    loop {
        let current: i64 =
            sqlx::query_scalar("SELECT applied_lsn FROM target_state WHERE target = ?")
                .bind(target.as_str())
                .fetch_one(&mut *connection)
                .await
                .map_err(|error| error.to_string())?;
        let next = sqlx::query(
            "SELECT commit_lsn, completed, degraded FROM commit_targets \
             WHERE target = ? AND commit_lsn > ? ORDER BY commit_lsn LIMIT 1",
        )
        .bind(target.as_str())
        .bind(current)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
        let Some(next) = next else {
            return Ok(());
        };
        let completed: i64 = next.get("completed");
        let degraded: i64 = next.get("degraded");
        if completed == 0 {
            return Ok(());
        }
        let next_lsn: i64 = next.get("commit_lsn");
        sqlx::query(
            "UPDATE target_state SET applied_lsn = ?, degraded = MAX(degraded, ?) \
             WHERE target = ?",
        )
        .bind(next_lsn)
        .bind(degraded)
        .bind(target.as_str())
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
    }
}

async fn advance_index_frontier(
    connection: &mut SqliteConnection,
    target: &TargetId,
    index_id: u64,
    generation: u64,
) -> Result<(), String> {
    let index_id_i64 = to_sql_i64(index_id, "index ID")?;
    let generation_i64 = to_sql_i64(generation, "index generation")?;

    sqlx::query(
        "INSERT INTO index_frontier(target, index_id, generation) \
         VALUES(?, ?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(target.as_str())
    .bind(index_id_i64)
    .bind(generation_i64)
    .execute(&mut *connection)
    .await
    .map_err(|error| error.to_string())?;

    loop {
        let current: i64 = sqlx::query_scalar(
            "SELECT applied_lsn FROM index_frontier \
             WHERE target = ? AND index_id = ? AND generation = ?",
        )
        .bind(target.as_str())
        .bind(index_id_i64)
        .bind(generation_i64)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;

        let next = sqlx::query(
            "SELECT c.commit_lsn, \
                    (SELECT COUNT(*) FROM events e \
                     WHERE e.target = c.target AND e.commit_lsn = c.commit_lsn \
                       AND e.index_id = ? AND e.generation = ?) AS total_count, \
                    (SELECT COUNT(*) FROM events e \
                     WHERE e.target = c.target AND e.commit_lsn = c.commit_lsn \
                       AND e.index_id = ? AND e.generation = ? \
                       AND e.status IN ('applied', 'skipped')) AS terminal_count, \
                    (SELECT COUNT(*) FROM events e \
                     WHERE e.target = c.target AND e.commit_lsn = c.commit_lsn \
                       AND e.index_id = ? AND e.generation = ? \
                       AND e.status = 'skipped') AS skipped_count \
             FROM commit_targets c \
             WHERE c.target = ? AND c.commit_lsn > ? \
             ORDER BY c.commit_lsn LIMIT 1",
        )
        .bind(index_id_i64)
        .bind(generation_i64)
        .bind(index_id_i64)
        .bind(generation_i64)
        .bind(index_id_i64)
        .bind(generation_i64)
        .bind(target.as_str())
        .bind(current)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;

        let Some(next) = next else {
            return Ok(());
        };
        let terminal_count: i64 = next.get("terminal_count");
        let total_count: i64 = next.get("total_count");
        let skipped_count: i64 = next.get("skipped_count");

        if terminal_count != total_count {
            return Ok(());
        }

        let next_lsn: i64 = next.get("commit_lsn");
        sqlx::query(
            "UPDATE index_frontier SET applied_lsn = ?, degraded = MAX(degraded, ?) \
             WHERE target = ? AND index_id = ? AND generation = ?",
        )
        .bind(next_lsn)
        .bind(i64::from(skipped_count != 0))
        .bind(target.as_str())
        .bind(index_id_i64)
        .bind(generation_i64)
        .execute(&mut *connection)
        .await
        .map_err(|error| error.to_string())?;
    }
}

fn to_sql_i64(value: u64, name: &str) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("{} exceeds SQLite integer range", name))
}

fn from_sql_i64(value: i64, name: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("{} cannot be negative", name))
}
