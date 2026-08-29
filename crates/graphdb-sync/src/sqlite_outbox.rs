use std::path::Path;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use graphdb_core::types::{CommitLsn, LeaseEpoch, TargetId};
use graphdb_core::wal::{IndexMutation, OutboxIntent};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqliteConnection, SqlitePool};

/// Never update version in dev phase
const OUTBOX_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedEvent {
    pub event_id: i64,
    pub commit_lsn: CommitLsn,
    pub intent_sequence: u32,
    pub mutation: IndexMutation,
    pub lease_owner: String,
    pub lease_epoch: LeaseEpoch,
}

#[derive(Debug, Clone)]
pub struct SqliteOutbox {
    pool: SqlitePool,
    path: std::path::PathBuf,
}

/// Immutable copy of the SQLite projection used by checkpoint recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxSnapshot {
    pub path: std::path::PathBuf,
    pub size_bytes: u64,
    pub checksum: u32,
    pub materialized_lsn: CommitLsn,
}

/// Point-in-time operational state of the durable outbox.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncDiagnostics {
    pub materialized_lsn: CommitLsn,
    pub targets: Vec<TargetSyncDiagnostics>,
    pub indexes: Vec<IndexSyncDiagnostics>,
    /// Total vector change items skipped due to disabled engine (delivery-plane
    /// accounting). Sourced from `VectorSyncCoordinator::disabled_skip_count`
    /// and merged by `SyncManager::sync_diagnostics`; 0 when vector is not
    /// configured.
    pub vector_disabled_skips: u64,
}

/// Delivery health for one synchronization target.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TargetSyncDiagnostics {
    pub target: String,
    pub applied_lsn: CommitLsn,
    pub frontier_lag: u64,
    pub pending: u64,
    pub retrying: u64,
    pub leased: u64,
    pub dead_lettered: u64,
    pub oldest_event_age_ms: Option<u64>,
    pub degraded: bool,
}

/// Generation and frontier health for one secondary index target.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IndexSyncDiagnostics {
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub state: String,
    pub barrier_lsn: Option<CommitLsn>,
    pub applied_lsn: CommitLsn,
    pub frontier_lag: u64,
    pub degraded: bool,
}

/// One dead-letter entry joined with its event metadata.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeadLetterRow {
    pub event_id: i64,
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub commit_lsn: CommitLsn,
    pub retry_count: u64,
    pub failed_at_ms: u64,
    pub error: String,
}

/// One degraded range entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DegradedRangeRow {
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub start_lsn: CommitLsn,
    pub end_lsn: CommitLsn,
    pub reason: String,
    pub created_at_ms: u64,
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

    /// Create an immutable SQLite backup at `destination`.
    ///
    /// `VACUUM INTO` runs against a consistent SQLite snapshot and includes
    /// the WAL contents in the resulting file. The temporary file is synced
    /// before an atomic rename so a crash cannot publish a partial backup.
    pub async fn create_snapshot(
        &self,
        destination: impl AsRef<Path>,
    ) -> Result<OutboxSnapshot, String> {
        let destination = destination.as_ref();
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let temporary = destination.with_extension(format!(
            "tmp-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos()
        ));
        if temporary.exists() {
            std::fs::remove_file(&temporary).map_err(|error| error.to_string())?;
        }
        let destination_sql = temporary.to_string_lossy().replace('\'', "''");
        sqlx::query(&format!("VACUUM INTO '{}'", destination_sql))
            .execute(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        let bytes = std::fs::read(&temporary).map_err(|error| error.to_string())?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        if destination.exists() {
            std::fs::remove_file(destination).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&temporary, destination).map_err(|error| error.to_string())?;
        let checksum_path = destination.with_extension("checksum");
        let checksum_temporary = checksum_path.with_extension("checksum.tmp");
        std::fs::write(&checksum_temporary, crc32fast::hash(&bytes).to_string())
            .map_err(|error| error.to_string())?;
        std::fs::File::open(&checksum_temporary)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        std::fs::rename(&checksum_temporary, &checksum_path).map_err(|error| error.to_string())?;
        if let Some(parent) = destination.parent() {
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| error.to_string())?;
        }
        let materialized_lsn = self.materialized_lsn().await?;
        Ok(OutboxSnapshot {
            path: destination.to_path_buf(),
            size_bytes: bytes.len() as u64,
            checksum: crc32fast::hash(&bytes),
            materialized_lsn,
        })
    }

    pub fn verify_snapshot(snapshot: &OutboxSnapshot) -> Result<(), String> {
        let bytes = std::fs::read(&snapshot.path).map_err(|error| error.to_string())?;
        if bytes.len() as u64 != snapshot.size_bytes {
            return Err("outbox snapshot size mismatch".to_string());
        }
        if crc32fast::hash(&bytes) != snapshot.checksum {
            return Err("outbox snapshot checksum mismatch".to_string());
        }
        Ok(())
    }

    /// Restore a verified snapshot before opening the live database.
    pub fn restore_snapshot(
        snapshot: &OutboxSnapshot,
        destination: impl AsRef<Path>,
    ) -> Result<(), String> {
        Self::verify_snapshot(snapshot)?;
        let destination = destination.as_ref();
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let temporary = destination.with_extension("restore.tmp");
        std::fs::copy(&snapshot.path, &temporary).map_err(|error| error.to_string())?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        if destination.exists() {
            std::fs::remove_file(destination).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&temporary, destination).map_err(|error| error.to_string())?;
        if let Some(parent) = destination.parent() {
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| error.to_string())?;
        }
        Ok(())
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

    pub async fn register_target(&self, target: &TargetId) -> Result<(), String> {
        sqlx::query("INSERT OR IGNORE INTO target_state(target) VALUES(?)")
            .bind(target.as_str())
            .execute(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub async fn create_index_generation(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
        snapshot_lsn: CommitLsn,
    ) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO generation_state(target, index_id, generation, state, barrier_lsn) \
             VALUES(?, ?, ?, 'creating', ?) \
             ON CONFLICT(target, index_id, generation) DO UPDATE SET state = 'creating'",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .bind(to_sql_i64(snapshot_lsn.get(), "snapshot LSN")?)
        .execute(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub async fn transition_generation_to_backfilling(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
    ) -> Result<(), String> {
        sqlx::query(
            "UPDATE generation_state SET state = 'backfilling' \
             WHERE target = ? AND index_id = ? AND generation = ? AND state = 'creating'",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .execute(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub async fn transition_generation_to_catching_up(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
        barrier_lsn: CommitLsn,
    ) -> Result<(), String> {
        sqlx::query(
            "UPDATE generation_state SET state = 'catching_up', barrier_lsn = ? \
             WHERE target = ? AND index_id = ? AND generation = ? AND state IN ('creating', 'backfilling')",
        )
        .bind(to_sql_i64(barrier_lsn.get(), "barrier LSN")?)
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .execute(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub async fn transition_generation_to_publishing(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
        barrier_lsn: CommitLsn,
    ) -> Result<(), String> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        begin_immediate(&mut connection).await?;
        let result = async {
            let current_lsn: i64 = sqlx::query_scalar(
                "SELECT barrier_lsn FROM generation_state \
                 WHERE target = ? AND index_id = ? AND generation = ? AND state = 'catching_up'",
            )
            .bind(target.as_str())
            .bind(to_sql_i64(index_id, "index ID")?)
            .bind(to_sql_i64(generation, "index generation")?)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Generation not in catching_up state".to_string())?;

            if current_lsn < to_sql_i64(barrier_lsn.get(), "barrier LSN")? {
                return Err(format!(
                    "Generation has not caught up to barrier LSN {} < {}",
                    current_lsn,
                    barrier_lsn.get()
                ));
            }

            sqlx::query(
                "UPDATE generation_state SET state = 'publishing', barrier_lsn = ? \
                 WHERE target = ? AND index_id = ? AND generation = ? AND state = 'catching_up'",
            )
            .bind(to_sql_i64(barrier_lsn.get(), "barrier LSN")?)
            .bind(target.as_str())
            .bind(to_sql_i64(index_id, "index ID")?)
            .bind(to_sql_i64(generation, "index generation")?)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            Ok(())
        }
        .await;
        finish_transaction(&mut connection, result).await
    }

    pub async fn activate_generation(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
        barrier_lsn: CommitLsn,
    ) -> Result<(), String> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        begin_immediate(&mut connection).await?;
        let result = async {
            let row = sqlx::query(
                "SELECT barrier_lsn FROM generation_state \
                 WHERE target = ? AND index_id = ? AND generation = ? \
                 AND state IN ('catching_up', 'publishing')",
            )
            .bind(target.as_str())
            .bind(to_sql_i64(index_id, "index ID")?)
            .bind(to_sql_i64(generation, "index generation")?)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Generation not in catching_up or publishing state".to_string())?;

            let current_lsn: i64 = row.get("barrier_lsn");
            if current_lsn < to_sql_i64(barrier_lsn.get(), "barrier LSN")? {
                return Err(format!(
                    "Generation has not caught up to barrier LSN {} < {}",
                    current_lsn,
                    barrier_lsn.get()
                ));
            }

            sqlx::query(
                "UPDATE generation_state SET state = 'active', barrier_lsn = NULL \
                 WHERE target = ? AND index_id = ? AND generation = ?",
            )
            .bind(target.as_str())
            .bind(to_sql_i64(index_id, "index ID")?)
            .bind(to_sql_i64(generation, "index generation")?)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            Ok(())
        }
        .await;
        finish_transaction(&mut connection, result).await
    }

    pub async fn drain_generation(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
    ) -> Result<(), String> {
        sqlx::query(
            "UPDATE generation_state SET state = 'draining' \
             WHERE target = ? AND index_id = ? AND generation = ? AND state = 'active'",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .execute(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub async fn drop_generation(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
    ) -> Result<(), String> {
        sqlx::query(
            "UPDATE generation_state SET state = 'dropped' \
             WHERE target = ? AND index_id = ? AND generation = ? \
             AND state IN ('active', 'draining', 'publishing', 'failed')",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .execute(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub async fn fail_generation(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
    ) -> Result<(), String> {
        sqlx::query(
            "UPDATE generation_state SET state = 'failed' \
             WHERE target = ? AND index_id = ? AND generation = ? AND state NOT IN ('failed', 'dropped')",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .execute(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub async fn get_generation_state(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
    ) -> Result<Option<(String, Option<CommitLsn>)>, String> {
        let row = sqlx::query(
            "SELECT state, barrier_lsn FROM generation_state \
                               WHERE target = ? AND index_id = ? AND generation = ?",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| error.to_string())?;

        row.map(|row| {
            let state: String = row.get("state");
            let barrier_lsn: Option<i64> = row.get("barrier_lsn");
            let barrier = barrier_lsn
                .map(|lsn| CommitLsn::new(from_sql_i64(lsn, "barrier LSN").unwrap_or(0)));
            Ok((state, barrier))
        })
        .transpose()
    }

    pub async fn get_active_generation(
        &self,
        target: &TargetId,
        index_id: u64,
    ) -> Result<Option<u64>, String> {
        let row = sqlx::query(
            "SELECT generation FROM generation_state \
                               WHERE target = ? AND index_id = ? AND state = 'active' \
                               ORDER BY generation DESC LIMIT 1",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| error.to_string())?;

        row.map(|row| {
            let generation: i64 = row.get("generation");
            from_sql_i64(generation, "index generation")
        })
        .transpose()
    }

    pub async fn materialize_commit(
        &self,
        commit_lsn: CommitLsn,
        intents: &[OutboxIntent],
        targets: &[TargetId],
    ) -> Result<(), String> {
        validate_intents(intents)?;
        let lsn = to_sql_i64(commit_lsn.get(), "commit LSN")?;
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        begin_immediate(&mut connection).await?;
        let result = async {
            let current: i64 = sqlx::query_scalar(
                "SELECT materialized_lsn FROM projection_state WHERE singleton = 1",
            )
            .fetch_one(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            if lsn < current {
                return Ok(());
            }
            let materialized_at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_millis() as u64;

            // A normal data mutation can arrive before an explicit index
            // rebuild has registered a generation. The first generation for
            // a live target is registered as active atomically with the
            // projection row. Existing lifecycle state is preserved, so a
            // rebuilding generation remains fenced until its barrier is
            // published.
            for intent in intents {
                sqlx::query(
                    "INSERT OR IGNORE INTO generation_state(\
                        target, index_id, generation, state, barrier_lsn\
                     ) VALUES(?, ?, ?, 'active', NULL)",
                )
                .bind(intent.mutation.target.as_str())
                .bind(to_sql_i64(intent.mutation.index_id, "index ID")?)
                .bind(to_sql_i64(
                    intent.mutation.index_generation.get(),
                    "index generation",
                )?)
                .execute(&mut *connection)
                .await
                .map_err(|error| error.to_string())?;
            }

            for target in targets {
                sqlx::query("INSERT OR IGNORE INTO target_state(target) VALUES(?)")
                    .bind(target.as_str())
                    .execute(&mut *connection)
                    .await
                    .map_err(|error| error.to_string())?;
                let event_count = intents
                    .iter()
                    .filter(|intent| intent.mutation.target == *target)
                    .count();
                sqlx::query(
                    "INSERT OR IGNORE INTO commit_targets(\
                        commit_lsn, target, event_count, completed\
                     ) VALUES(?, ?, ?, ?)",
                )
                .bind(lsn)
                .bind(target.as_str())
                .bind(to_sql_i64(event_count as u64, "event count")?)
                .bind(i64::from(event_count == 0))
                .execute(&mut *connection)
                .await
                .map_err(|error| error.to_string())?;
            }

            for intent in intents {
                let mutation =
                    postcard::to_allocvec(&intent.mutation).map_err(|error| error.to_string())?;
                sqlx::query(
                    "INSERT OR IGNORE INTO events(\
                        transaction_id, intent_sequence, target, index_id, generation, mutation,\
                        commit_lsn, idempotency_key, ordering_key, created_at_ms\
                     ) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(to_sql_i64(
                    intent.transaction_id.as_u64(),
                    "transaction ID",
                )?)
                .bind(i64::from(intent.intent_sequence))
                .bind(intent.mutation.target.as_str())
                .bind(to_sql_i64(intent.mutation.index_id, "index ID")?)
                .bind(to_sql_i64(
                    intent.mutation.index_generation.get(),
                    "index generation",
                )?)
                .bind(mutation)
                .bind(lsn)
                .bind(intent.mutation.idempotency_key.as_str())
                .bind(intent.mutation.ordering_key.as_str())
                .bind(to_sql_i64(materialized_at_ms, "event creation timestamp")?)
                .execute(&mut *connection)
                .await
                .map_err(|error| error.to_string())?;
                sqlx::query(
                    "INSERT OR IGNORE INTO idempotency(target, idempotency_key, commit_lsn) \
                     VALUES(?, ?, ?)",
                )
                .bind(intent.mutation.target.as_str())
                .bind(intent.mutation.idempotency_key.as_str())
                .bind(lsn)
                .execute(&mut *connection)
                .await
                .map_err(|error| error.to_string())?;
            }
            sqlx::query(
                "UPDATE projection_state SET materialized_lsn = MAX(materialized_lsn, ?) \
                 WHERE singleton = 1",
            )
            .bind(lsn)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            Ok(())
        }
        .await;
        finish_transaction(&mut connection, result).await
    }

    pub async fn claim_next(
        &self,
        target: &TargetId,
        owner: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Option<ClaimedEvent>, String> {
        if owner.is_empty() {
            return Err("Lease owner cannot be empty".to_string());
        }
        let now = to_sql_i64(now_ms, "current time")?;
        let lease_until = to_sql_i64(now_ms.saturating_add(lease_duration_ms), "lease deadline")?;
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        begin_immediate(&mut connection).await?;
        let result = async {
            let row = sqlx::query(
                "SELECT e.id, e.commit_lsn, e.intent_sequence, e.mutation, e.lease_epoch \
                 FROM events e \
                 WHERE e.target = ? \
                   AND e.status IN ('pending', 'retry', 'leased') \
                   AND e.next_attempt_at_ms <= ? \
                   AND (e.lease_owner IS NULL OR e.lease_until_ms <= ?) \
                   AND EXISTS (\
                       SELECT 1 FROM generation_state g \
                       WHERE g.target = e.target AND g.index_id = e.index_id \
                         AND g.generation = e.generation AND g.state = 'active'\
                   ) \
                   AND NOT EXISTS (\
                       SELECT 1 FROM events earlier \
                       WHERE earlier.target = e.target \
                         AND earlier.ordering_key = e.ordering_key \
                         AND (earlier.commit_lsn < e.commit_lsn OR (\
                             earlier.commit_lsn = e.commit_lsn \
                             AND earlier.intent_sequence < e.intent_sequence\
                         )) \
                         AND earlier.status NOT IN ('applied', 'skipped')\
                   ) \
                 ORDER BY e.commit_lsn, e.intent_sequence LIMIT 1",
            )
            .bind(target.as_str())
            .bind(now)
            .bind(now)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            let Some(row) = row else {
                return Ok(None);
            };
            let event_id: i64 = row.get("id");
            let previous_epoch: i64 = row.get("lease_epoch");
            let lease_epoch = previous_epoch
                .checked_add(1)
                .ok_or_else(|| "Lease epoch overflow".to_string())?;
            let updated = sqlx::query(
                "UPDATE events SET status = 'leased', lease_owner = ?, lease_until_ms = ?, \
                    lease_epoch = ? WHERE id = ? AND lease_epoch = ?",
            )
            .bind(owner)
            .bind(lease_until)
            .bind(lease_epoch)
            .bind(event_id)
            .bind(previous_epoch)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            if updated.rows_affected() != 1 {
                return Err(format!("Failed to fence claimed event {}", event_id));
            }
            let mutation_bytes: Vec<u8> = row.get("mutation");
            let mutation: IndexMutation =
                postcard::from_bytes(&mutation_bytes).map_err(|error| error.to_string())?;
            mutation.validate()?;
            Ok(Some(ClaimedEvent {
                event_id,
                commit_lsn: CommitLsn::new(from_sql_i64(row.get("commit_lsn"), "commit LSN")?),
                intent_sequence: u32::try_from(row.get::<i64, _>("intent_sequence"))
                    .map_err(|_| "Intent sequence is out of range".to_string())?,
                mutation,
                lease_owner: owner.to_string(),
                lease_epoch: LeaseEpoch::new(from_sql_i64(lease_epoch, "lease epoch")?),
            }))
        }
        .await;
        finish_transaction(&mut connection, result).await
    }

    pub async fn acknowledge(&self, event: &ClaimedEvent) -> Result<bool, String> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        begin_immediate(&mut connection).await?;
        let result = async {
            let updated = sqlx::query(
                "UPDATE events SET status = 'applied', lease_owner = NULL, lease_until_ms = 0 \
                 WHERE id = ? AND status = 'leased' AND lease_owner = ? AND lease_epoch = ?",
            )
            .bind(event.event_id)
            .bind(&event.lease_owner)
            .bind(to_sql_i64(event.lease_epoch.get(), "lease epoch")?)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            if updated.rows_affected() == 0 {
                return Ok(false);
            }
            let lsn = to_sql_i64(event.commit_lsn.get(), "commit LSN")?;
            sqlx::query(
                "UPDATE commit_targets SET applied_count = applied_count + 1, \
                    completed = (applied_count + 1 >= event_count) \
                 WHERE commit_lsn = ? AND target = ?",
            )
            .bind(lsn)
            .bind(event.mutation.target.as_str())
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            advance_frontier(&mut connection, &event.mutation.target).await?;
            advance_index_frontier(
                &mut connection,
                &event.mutation.target,
                event.mutation.index_id,
                event.mutation.index_generation.get(),
            )
            .await?;
            Ok(true)
        }
        .await;
        finish_transaction(&mut connection, result).await
    }

    pub async fn retry(
        &self,
        event: &ClaimedEvent,
        next_attempt_at_ms: u64,
        error: &str,
    ) -> Result<bool, String> {
        let updated = sqlx::query(
            "UPDATE events SET status = 'retry', next_attempt_at_ms = ?, retry_count = retry_count + 1, \
                last_error = ?, lease_owner = NULL, lease_until_ms = 0 \
             WHERE id = ? AND status = 'leased' AND lease_owner = ? AND lease_epoch = ?",
        )
        .bind(to_sql_i64(next_attempt_at_ms, "next attempt time")?)
        .bind(error)
        .bind(event.event_id)
        .bind(&event.lease_owner)
        .bind(to_sql_i64(event.lease_epoch.get(), "lease epoch")?)
        .execute(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn dead_letter(
        &self,
        event: &ClaimedEvent,
        failed_at_ms: u64,
        error: &str,
    ) -> Result<bool, String> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        begin_immediate(&mut connection).await?;
        let result = async {
            let updated = sqlx::query(
                "UPDATE events SET status = 'dead_letter', last_error = ?, lease_owner = NULL, \
                    lease_until_ms = 0 \
                 WHERE id = ? AND status = 'leased' AND lease_owner = ? AND lease_epoch = ?",
            )
            .bind(error)
            .bind(event.event_id)
            .bind(&event.lease_owner)
            .bind(to_sql_i64(event.lease_epoch.get(), "lease epoch")?)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            if updated.rows_affected() == 0 {
                return Ok(false);
            }
            sqlx::query(
                "INSERT INTO dead_letters(event_id, failed_at_ms, error) VALUES(?, ?, ?) \
                 ON CONFLICT(event_id) DO UPDATE SET failed_at_ms = excluded.failed_at_ms, \
                    error = excluded.error",
            )
            .bind(event.event_id)
            .bind(to_sql_i64(failed_at_ms, "failure time")?)
            .bind(error)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            Ok(true)
        }
        .await;
        finish_transaction(&mut connection, result).await
    }

    pub async fn requeue_dead_letter(&self, event_id: i64) -> Result<bool, String> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        begin_immediate(&mut connection).await?;
        let result = async {
            let updated = sqlx::query(
                "UPDATE events SET status = 'pending', next_attempt_at_ms = 0, \
                 lease_owner = NULL, lease_until_ms = 0, last_error = NULL \
                 WHERE id = ? AND status = 'dead_letter'",
            )
            .bind(event_id)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            if updated.rows_affected() == 0 {
                return Ok(false);
            }
            sqlx::query("DELETE FROM dead_letters WHERE event_id = ?")
                .bind(event_id)
                .execute(&mut *connection)
                .await
                .map_err(|error| error.to_string())?;
            Ok(true)
        }
        .await;
        finish_transaction(&mut connection, result).await
    }

    /// Batch requeue dead letters filtered by target/index/generation.
    /// Returns number of events requeued.
    pub async fn requeue_dead_letters_batch(
        &self,
        target: Option<&TargetId>,
        index_id: Option<u64>,
        generation: Option<u64>,
        limit: usize,
    ) -> Result<usize, String> {
        let limit = limit.clamp(1, 1000) as i64;
        // Fetch candidate event_ids first, then requeue one by one to reuse transaction fencing.
        let mut query = String::from(
            "SELECT e.id FROM events e JOIN dead_letters d ON d.event_id = e.id WHERE e.status = 'dead_letter'",
        );
        if target.is_some() {
            query.push_str(" AND e.target = ?");
        }
        if index_id.is_some() {
            query.push_str(" AND e.index_id = ?");
        }
        if generation.is_some() {
            query.push_str(" AND e.generation = ?");
        }
        query.push_str(" ORDER BY e.commit_lsn, e.intent_sequence LIMIT ?");
        let mut q = sqlx::query_scalar::<_, i64>(&query);
        if let Some(t) = target {
            q = q.bind(t.as_str());
        }
        if let Some(id) = index_id {
            q = q.bind(to_sql_i64(id, "index ID")?);
        }
        if let Some(gen) = generation {
            q = q.bind(to_sql_i64(gen, "index generation")?);
        }
        q = q.bind(limit);
        let ids: Vec<i64> = q.fetch_all(&self.pool).await.map_err(|e| e.to_string())?;
        let mut requeued = 0usize;
        for id in ids {
            if self.requeue_dead_letter(id).await? {
                requeued += 1;
            }
        }
        Ok(requeued)
    }

    pub async fn list_dead_letters(
        &self,
        target: Option<&TargetId>,
        index_id: Option<u64>,
        generation: Option<u64>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<DeadLetterRow>, String> {
        let limit = limit.clamp(1, 1000) as i64;
        let offset = offset as i64;
        let mut query = String::from(
            "SELECT e.id, e.target, e.index_id, e.generation, e.commit_lsn, e.retry_count, d.failed_at_ms, d.error \
             FROM events e JOIN dead_letters d ON d.event_id = e.id WHERE e.status = 'dead_letter'",
        );
        if target.is_some() {
            query.push_str(" AND e.target = ?");
        }
        if index_id.is_some() {
            query.push_str(" AND e.index_id = ?");
        }
        if generation.is_some() {
            query.push_str(" AND e.generation = ?");
        }
        query.push_str(" ORDER BY e.commit_lsn, e.intent_sequence LIMIT ? OFFSET ?");
        let mut q = sqlx::query(&query);
        if let Some(t) = target {
            q = q.bind(t.as_str());
        }
        if let Some(id) = index_id {
            q = q.bind(to_sql_i64(id, "index ID")?);
        }
        if let Some(gen) = generation {
            q = q.bind(to_sql_i64(gen, "index generation")?);
        }
        q = q.bind(limit).bind(offset);
        let rows = q.fetch_all(&self.pool).await.map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(DeadLetterRow {
                event_id: row.get("id"),
                target: row.get("target"),
                index_id: from_sql_i64(row.get("index_id"), "index ID")?,
                generation: from_sql_i64(row.get("generation"), "index generation")?,
                commit_lsn: CommitLsn::new(from_sql_i64(row.get("commit_lsn"), "commit LSN")?),
                retry_count: from_sql_i64(row.get("retry_count"), "retry count")?,
                failed_at_ms: from_sql_i64(row.get("failed_at_ms"), "failure time")?,
                error: row.get("error"),
            });
        }
        Ok(out)
    }

    pub async fn list_degraded_ranges(
        &self,
        target: Option<&TargetId>,
        index_id: Option<u64>,
        generation: Option<u64>,
    ) -> Result<Vec<DegradedRangeRow>, String> {
        let mut query = String::from(
            "SELECT target, index_id, generation, start_lsn, end_lsn, reason, created_at_ms FROM degraded_ranges WHERE 1=1",
        );
        if target.is_some() {
            query.push_str(" AND target = ?");
        }
        if index_id.is_some() {
            query.push_str(" AND index_id = ?");
        }
        if generation.is_some() {
            query.push_str(" AND generation = ?");
        }
        query.push_str(" ORDER BY created_at_ms DESC");
        let mut q = sqlx::query(&query);
        if let Some(t) = target {
            q = q.bind(t.as_str());
        }
        if let Some(id) = index_id {
            q = q.bind(to_sql_i64(id, "index ID")?);
        }
        if let Some(gen) = generation {
            q = q.bind(to_sql_i64(gen, "index generation")?);
        }
        let rows = q.fetch_all(&self.pool).await.map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(DegradedRangeRow {
                target: row.get("target"),
                index_id: from_sql_i64(row.get("index_id"), "index ID")?,
                generation: from_sql_i64(row.get("generation"), "index generation")?,
                start_lsn: CommitLsn::new(from_sql_i64(row.get("start_lsn"), "start LSN")?),
                end_lsn: CommitLsn::new(from_sql_i64(row.get("end_lsn"), "end LSN")?),
                reason: row.get("reason"),
                created_at_ms: from_sql_i64(row.get("created_at_ms"), "created time")?,
            });
        }
        Ok(out)
    }

    pub async fn clear_degraded_range(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
        start_lsn: CommitLsn,
        end_lsn: CommitLsn,
    ) -> Result<bool, String> {
        let result = sqlx::query(
            "DELETE FROM degraded_ranges WHERE target = ? AND index_id = ? AND generation = ? AND start_lsn = ? AND end_lsn = ?",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .bind(to_sql_i64(start_lsn.get(), "start LSN")?)
        .bind(to_sql_i64(end_lsn.get(), "end LSN")?)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn delivery_targets(&self) -> Result<Vec<TargetId>, String> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT target FROM events WHERE status IN ('pending', 'retry', 'leased') \
             ORDER BY target",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        rows.into_iter().map(TargetId::new).collect()
    }

    /// Return aggregate delivery statistics from the durable projection.
    ///
    /// The in-memory transaction staging map is intentionally not used here:
    /// once a commit has crossed the WAL and SQLite materialization fences,
    /// SQLite is the source of truth for backlog, leases, retries, and
    /// dead-letter state.
    pub async fn stats(&self) -> Result<crate::OutboxStats, String> {
        let row = sqlx::query(
            "SELECT \
                COALESCE(SUM(CASE WHEN status IN ('pending', 'retry') THEN 1 ELSE 0 END), 0) AS pending, \
                COALESCE(SUM(retry_count), 0) AS retries, \
                COALESCE(SUM(CASE WHEN status = 'leased' THEN 1 ELSE 0 END), 0) AS leased, \
                COALESCE(SUM(CASE WHEN status = 'dead_letter' THEN 1 ELSE 0 END), 0) AS dead_lettered, \
                MIN(CASE WHEN status IN ('pending', 'retry', 'leased') AND created_at_ms > 0 \
                         THEN created_at_ms END) AS oldest_created_at_ms \
             FROM events",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis() as u64;
        let oldest_created_at_ms: Option<i64> = row.get("oldest_created_at_ms");
        let oldest_event_age_ms = oldest_created_at_ms
            .map(|created_at_ms| from_sql_i64(created_at_ms, "event creation timestamp"))
            .transpose()?
            .map(|created_at_ms| now_ms.saturating_sub(created_at_ms));
        let projection_size = std::fs::metadata(&self.path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let durable_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&self.pool)
            .await
            .map_err(|error| error.to_string())?;

        Ok(crate::OutboxStats {
            pending: from_sql_i64(row.get("pending"), "pending event count")?
                .try_into()
                .map_err(|_| "pending event count exceeds usize range".to_string())?,
            retries: from_sql_i64(row.get("retries"), "retry count")?,
            oldest_event_age_ms: oldest_event_age_ms.unwrap_or(0),
            dead_lettered: from_sql_i64(row.get("dead_lettered"), "dead letter count")?
                .try_into()
                .map_err(|_| "dead letter count exceeds usize range".to_string())?,
            leased: from_sql_i64(row.get("leased"), "leased event count")?
                .try_into()
                .map_err(|_| "leased event count exceeds usize range".to_string())?,
            write_amplification_bytes: projection_size,
            persist_operations: from_sql_i64(durable_rows, "durable event row count")?,
            ..Default::default()
        })
    }

    pub async fn prune_applied_events(&self, retention_lsn: CommitLsn) -> Result<u64, String> {
        let retention = to_sql_i64(retention_lsn.get(), "retention LSN")?;
        let result = sqlx::query(
            "DELETE FROM events WHERE status IN ('applied','skipped') AND commit_lsn <= ?",
        )
        .bind(retention)
        .execute(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(result.rows_affected())
    }

    pub async fn retention_lsn(&self) -> Result<CommitLsn, String> {
        let value: i64 =
            sqlx::query_scalar("SELECT retention_lsn FROM projection_state WHERE singleton = 1")
                .fetch_one(&self.pool)
                .await
                .map_err(|error| error.to_string())?;
        Ok(CommitLsn::new(from_sql_i64(value, "retention LSN")?))
    }

    pub async fn update_retention_lsn(&self, retention_lsn: CommitLsn) -> Result<(), String> {
        sqlx::query(
            "UPDATE projection_state SET retention_lsn = MAX(retention_lsn, ?) WHERE singleton = 1",
        )
        .bind(to_sql_i64(retention_lsn.get(), "retention LSN")?)
        .execute(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Archive dead letters older than retention_lsn into dead_letters_archive and remove from events.
    pub async fn archive_dead_letters(&self, retention_lsn: CommitLsn) -> Result<u64, String> {
        let retention = to_sql_i64(retention_lsn.get(), "retention LSN")?;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;
        // Insert into archive
        let _archived = sqlx::query(
            "INSERT OR IGNORE INTO dead_letters_archive(event_id, target, index_id, generation, commit_lsn, failed_at_ms, error, archived_at_ms) \
             SELECT e.id, e.target, e.index_id, e.generation, e.commit_lsn, d.failed_at_ms, d.error, ? \
             FROM events e JOIN dead_letters d ON d.event_id = e.id WHERE e.status = 'dead_letter' AND e.commit_lsn <= ?",
        )
        .bind(to_sql_i64(now_ms, "archived time")?)
        .bind(retention)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        // Delete archived events (cascade via foreign key not set, so delete dead_letters first)
        sqlx::query(
            "DELETE FROM dead_letters WHERE event_id IN (SELECT id FROM events WHERE status = 'dead_letter' AND commit_lsn <= ?)",
        )
        .bind(retention)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        let deleted =
            sqlx::query("DELETE FROM events WHERE status = 'dead_letter' AND commit_lsn <= ?")
                .bind(retention)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        Ok(deleted.rows_affected())
    }

    /// Prune degraded ranges older than max age.
    pub async fn prune_degraded_ranges(&self, max_age_ms: u64) -> Result<u64, String> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;
        let cutoff = now_ms.saturating_sub(max_age_ms);
        let result = sqlx::query("DELETE FROM degraded_ranges WHERE created_at_ms < ?")
            .bind(to_sql_i64(cutoff, "cutoff time")?)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }

    /// Compute a safe retention LSN based on min applied frontier.
    /// Safe = min(target frontiers, index frontiers) - grace, at least 0.
    pub async fn compute_safe_retention_lsn(
        &self,
        grace_lsn_distance: u64,
    ) -> Result<CommitLsn, String> {
        let diag = self.diagnostics().await?;
        let mut min_lsn = diag.materialized_lsn;
        for t in &diag.targets {
            if t.applied_lsn < min_lsn {
                min_lsn = t.applied_lsn;
            }
        }
        for idx in &diag.indexes {
            if idx.applied_lsn < min_lsn {
                min_lsn = idx.applied_lsn;
            }
        }
        let safe = min_lsn.get().saturating_sub(grace_lsn_distance);
        Ok(CommitLsn::new(safe))
    }

    pub async fn retry_count(&self, event_id: i64) -> Result<u64, String> {
        let value: i64 = sqlx::query_scalar("SELECT retry_count FROM events WHERE id = ?")
            .bind(event_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        from_sql_i64(value, "retry count")
    }

    pub async fn materialized_lsn(&self) -> Result<CommitLsn, String> {
        let value: i64 =
            sqlx::query_scalar("SELECT materialized_lsn FROM projection_state WHERE singleton = 1")
                .fetch_one(&self.pool)
                .await
                .map_err(|error| error.to_string())?;
        Ok(CommitLsn::new(from_sql_i64(value, "materialized LSN")?))
    }

    pub async fn target_frontier(&self, target: &TargetId) -> Result<CommitLsn, String> {
        let value: Option<i64> =
            sqlx::query_scalar("SELECT applied_lsn FROM target_state WHERE target = ?")
                .bind(target.as_str())
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| error.to_string())?;
        Ok(CommitLsn::new(from_sql_i64(
            value.unwrap_or(0),
            "target frontier",
        )?))
    }

    pub async fn index_frontier(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
    ) -> Result<CommitLsn, String> {
        let value: Option<i64> = sqlx::query_scalar(
            "SELECT applied_lsn FROM index_frontier \
             WHERE target = ? AND index_id = ? AND generation = ?",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(CommitLsn::new(from_sql_i64(
            value.unwrap_or(0),
            "index frontier",
        )?))
    }

    pub async fn wait_for_minimum_lsn(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
        minimum_lsn: CommitLsn,
        timeout_ms: u64,
    ) -> Result<bool, String> {
        let deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64
            + timeout_ms;

        loop {
            if self
                .has_degraded_range_through(target, index_id, generation, minimum_lsn)
                .await?
            {
                return Err(format!(
                    "Consistency frontier for target {} index {} generation {} is degraded through LSN {}",
                    target.as_str(),
                    index_id,
                    generation,
                    minimum_lsn
                ));
            }
            let frontier = self.index_frontier(target, index_id, generation).await?;
            if frontier >= minimum_lsn {
                return Ok(true);
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_millis() as u64;
            if now >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    pub async fn skip_event_degraded(
        &self,
        event: &ClaimedEvent,
        reason: &str,
    ) -> Result<bool, String> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        begin_immediate(&mut connection).await?;
        let result = async {
            let updated = sqlx::query(
                "UPDATE events SET status = 'skipped', last_error = ?, lease_owner = NULL, \
                     lease_until_ms = 0 \
                 WHERE id = ? AND status = 'leased' AND lease_owner = ? AND lease_epoch = ?",
            )
            .bind(reason)
            .bind(event.event_id)
            .bind(&event.lease_owner)
            .bind(to_sql_i64(event.lease_epoch.get(), "lease epoch")?)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
            if updated.rows_affected() == 0 {
                return Ok(false);
            }
            let lsn = to_sql_i64(event.commit_lsn.get(), "commit LSN")?;
            sqlx::query(
                "UPDATE commit_targets SET applied_count = applied_count + 1, \
                     degraded = 1, completed = (applied_count + 1 >= event_count) \
                 WHERE commit_lsn = ? AND target = ?",
            )
            .bind(lsn)
            .bind(event.mutation.target.as_str())
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;

            sqlx::query(
                "UPDATE index_frontier SET degraded = 1 \
                 WHERE target = ? AND index_id = ? AND generation = ?",
            )
            .bind(event.mutation.target.as_str())
            .bind(to_sql_i64(event.mutation.index_id, "index ID")?)
            .bind(to_sql_i64(
                event.mutation.index_generation.get(),
                "index generation",
            )?)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;

            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_millis() as u64;
            sqlx::query(
                "INSERT INTO degraded_ranges(\
                     target, index_id, generation, start_lsn, end_lsn, reason, created_at_ms\
                 ) VALUES(?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(target, index_id, generation, start_lsn, end_lsn) \
                 DO UPDATE SET reason = excluded.reason, created_at_ms = excluded.created_at_ms",
            )
            .bind(event.mutation.target.as_str())
            .bind(to_sql_i64(event.mutation.index_id, "index ID")?)
            .bind(to_sql_i64(
                event.mutation.index_generation.get(),
                "index generation",
            )?)
            .bind(lsn)
            .bind(lsn)
            .bind(reason)
            .bind(to_sql_i64(now_ms, "degraded range timestamp")?)
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;

            advance_frontier(&mut connection, &event.mutation.target).await?;
            advance_index_frontier(
                &mut connection,
                &event.mutation.target,
                event.mutation.index_id,
                event.mutation.index_generation.get(),
            )
            .await?;
            Ok(true)
        }
        .await;
        finish_transaction(&mut connection, result).await
    }

    pub async fn has_degraded_range(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
    ) -> Result<bool, String> {
        let value: Option<i64> = sqlx::query_scalar(
            "SELECT degraded FROM index_frontier \
             WHERE target = ? AND index_id = ? AND generation = ?",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(value.unwrap_or(0) != 0)
    }

    pub async fn has_degraded_range_through(
        &self,
        target: &TargetId,
        index_id: u64,
        generation: u64,
        minimum_lsn: CommitLsn,
    ) -> Result<bool, String> {
        let value: i64 = sqlx::query_scalar(
            "SELECT EXISTS(\
                 SELECT 1 FROM degraded_ranges \
                 WHERE target = ? AND index_id = ? AND generation = ? AND start_lsn <= ?\
             )",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(generation, "index generation")?)
        .bind(to_sql_i64(minimum_lsn.get(), "minimum LSN")?)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(value != 0)
    }

    pub async fn dead_lettered_count(&self, target: &TargetId) -> Result<u64, String> {
        let value: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events e \
             JOIN dead_letters d ON d.event_id = e.id \
             WHERE e.target = ?",
        )
        .bind(target.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        from_sql_i64(value, "dead letter count")
    }

    /// Return a single consistent view of outbox backlog, delivery frontiers,
    /// degraded ranges, and index-generation progress.
    pub async fn diagnostics(&self) -> Result<SyncDiagnostics, String> {
        let materialized_lsn = self.materialized_lsn().await?;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis() as u64;
        let target_rows = sqlx::query(
            "SELECT s.target, s.applied_lsn, s.degraded, \
                    SUM(CASE WHEN e.status = 'pending' THEN 1 ELSE 0 END) AS pending, \
                    SUM(CASE WHEN e.status = 'retry' THEN 1 ELSE 0 END) AS retrying, \
                    SUM(CASE WHEN e.status = 'leased' THEN 1 ELSE 0 END) AS leased, \
                    SUM(CASE WHEN e.status = 'dead_letter' THEN 1 ELSE 0 END) AS dead_lettered, \
                    MIN(CASE WHEN e.status IN ('pending', 'retry', 'leased') \
                              AND e.created_at_ms > 0 THEN e.created_at_ms END) AS oldest_created_at_ms \
             FROM target_state s LEFT JOIN events e ON e.target = s.target \
             GROUP BY s.target, s.applied_lsn, s.degraded ORDER BY s.target",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        let mut targets = Vec::with_capacity(target_rows.len());
        for row in target_rows {
            let applied_lsn =
                CommitLsn::new(from_sql_i64(row.get("applied_lsn"), "target frontier")?);
            let oldest_created_at_ms: Option<i64> = row.get("oldest_created_at_ms");
            let oldest_event_age_ms = oldest_created_at_ms
                .map(|created_at_ms| from_sql_i64(created_at_ms, "event creation timestamp"))
                .transpose()?
                .map(|created_at_ms| now_ms.saturating_sub(created_at_ms));
            targets.push(TargetSyncDiagnostics {
                target: row.get("target"),
                applied_lsn,
                frontier_lag: materialized_lsn.get().saturating_sub(applied_lsn.get()),
                pending: from_sql_i64(row.get("pending"), "pending event count")?,
                retrying: from_sql_i64(row.get("retrying"), "retry event count")?,
                leased: from_sql_i64(row.get("leased"), "leased event count")?,
                dead_lettered: from_sql_i64(row.get("dead_lettered"), "dead-letter event count")?,
                oldest_event_age_ms,
                degraded: row.get::<i64, _>("degraded") != 0,
            });
        }

        let index_rows = sqlx::query(
            "SELECT g.target, g.index_id, g.generation, g.state, g.barrier_lsn, \
                    COALESCE(f.applied_lsn, 0) AS applied_lsn, COALESCE(f.degraded, 0) AS degraded \
             FROM generation_state g LEFT JOIN index_frontier f \
                  ON f.target = g.target AND f.index_id = g.index_id AND f.generation = g.generation \
             ORDER BY g.target, g.index_id, g.generation",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        let mut indexes = Vec::with_capacity(index_rows.len());
        for row in index_rows {
            let applied_lsn =
                CommitLsn::new(from_sql_i64(row.get("applied_lsn"), "index frontier")?);
            let barrier_lsn: Option<i64> = row.get("barrier_lsn");
            indexes.push(IndexSyncDiagnostics {
                target: row.get("target"),
                index_id: from_sql_i64(row.get("index_id"), "index ID")?,
                generation: from_sql_i64(row.get("generation"), "index generation")?,
                state: row.get("state"),
                barrier_lsn: barrier_lsn
                    .map(|value| from_sql_i64(value, "generation barrier LSN"))
                    .transpose()?
                    .map(CommitLsn::new),
                applied_lsn,
                frontier_lag: materialized_lsn.get().saturating_sub(applied_lsn.get()),
                degraded: row.get::<i64, _>("degraded") != 0,
            });
        }
        Ok(SyncDiagnostics {
            materialized_lsn,
            targets,
            indexes,
            vector_disabled_skips: 0,
        })
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

#[cfg(test)]
mod tests {
    use graphdb_core::types::{
        IdempotencyKey, IndexGeneration, OrderingKey, TargetId, TransactionId, VertexId,
    };
    use graphdb_core::wal::{
        EntityRef, IndexMutation, IndexOperation, OutboxIntent, WAL_SYNC_WIRE_VERSION,
    };
    use tempfile::TempDir;

    use super::SqliteOutbox;
    use graphdb_core::types::CommitLsn;

    fn intent(sequence: u32, entity: i64, target: &TargetId) -> OutboxIntent {
        OutboxIntent {
            wire_version: WAL_SYNC_WIRE_VERSION,
            transaction_id: TransactionId::new(1),
            intent_sequence: sequence,
            mutation: IndexMutation {
                wire_version: WAL_SYNC_WIRE_VERSION,
                target: target.clone(),
                index_id: 1,
                index_generation: IndexGeneration::new(1),
                entity_ref: EntityRef::Vertex(VertexId::from_int64(entity)),
                operation: IndexOperation::Upsert,
                document_or_vector: vec![entity as u8],
                idempotency_key: IdempotencyKey::new(format!("event-{sequence}"))
                    .expect("key should be valid"),
                ordering_key: OrderingKey::new(format!("entity-{entity}"))
                    .expect("key should be valid"),
            },
        }
    }

    #[tokio::test]
    async fn materialize_claim_and_ack_are_durable() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let path = directory.path().join("outbox.sqlite");
        let target = TargetId::new("fulltext").expect("target should be valid");
        let outbox = SqliteOutbox::open(&path).await.expect("outbox should open");
        outbox
            .create_index_generation(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should be created");
        outbox
            .transition_generation_to_backfilling(&target, 1, 1)
            .await
            .expect("generation should transition to backfilling");
        outbox
            .transition_generation_to_catching_up(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should transition to catching up");
        outbox
            .activate_generation(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should activate");
        outbox
            .materialize_commit(
                CommitLsn::new(100),
                &[intent(0, 7, &target)],
                std::slice::from_ref(&target),
            )
            .await
            .expect("commit should materialize");
        let event = outbox
            .claim_next(&target, "worker-1", 10, 100)
            .await
            .expect("claim should succeed")
            .expect("event should be available");
        assert!(outbox
            .acknowledge(&event)
            .await
            .expect("ack should succeed"));
        assert_eq!(
            outbox
                .target_frontier(&target)
                .await
                .expect("frontier should load"),
            CommitLsn::new(100)
        );
        drop(outbox);
        let reopened = SqliteOutbox::open(&path)
            .await
            .expect("outbox should reopen");
        assert_eq!(
            reopened
                .materialized_lsn()
                .await
                .expect("materialized LSN should load"),
            CommitLsn::new(100)
        );
    }

    #[tokio::test]
    async fn materialization_registers_a_missing_first_generation() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let target = TargetId::new("fulltext").expect("target should be valid");
        let outbox = SqliteOutbox::open(directory.path().join("outbox.sqlite"))
            .await
            .expect("outbox should open");
        outbox
            .materialize_commit(
                CommitLsn::new(10),
                &[intent(0, 1, &target)],
                std::slice::from_ref(&target),
            )
            .await
            .expect("commit should materialize");

        let state = outbox
            .get_generation_state(&target, 1, 1)
            .await
            .expect("generation state should load")
            .expect("generation should be registered");
        assert_eq!(state.0, "active");
        assert!(outbox
            .claim_next(&target, "worker-1", 0, 100)
            .await
            .expect("event should be claimable")
            .is_some());
    }

    #[tokio::test]
    async fn frontier_does_not_cross_an_unacknowledged_commit() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let target = TargetId::new("vector").expect("target should be valid");
        let outbox = SqliteOutbox::open(directory.path().join("outbox.sqlite"))
            .await
            .expect("outbox should open");
        outbox
            .create_index_generation(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should be created");
        outbox
            .transition_generation_to_backfilling(&target, 1, 1)
            .await
            .expect("generation should transition to backfilling");
        outbox
            .transition_generation_to_catching_up(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should transition to catching up");
        outbox
            .activate_generation(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should activate");
        outbox
            .materialize_commit(
                CommitLsn::new(100),
                &[intent(0, 1, &target)],
                std::slice::from_ref(&target),
            )
            .await
            .expect("first commit should materialize");
        let mut second = intent(0, 2, &target);
        second.transaction_id = TransactionId::new(2);
        second.mutation.idempotency_key =
            IdempotencyKey::new("second").expect("key should be valid");
        outbox
            .materialize_commit(
                CommitLsn::new(200),
                &[second],
                std::slice::from_ref(&target),
            )
            .await
            .expect("second commit should materialize");

        let first = outbox
            .claim_next(&target, "worker-1", 10, 100)
            .await
            .expect("first claim should succeed")
            .expect("first event should exist");
        let second = outbox
            .claim_next(&target, "worker-2", 10, 100)
            .await
            .expect("second claim should succeed")
            .expect("second event should exist");
        assert_eq!(second.commit_lsn, CommitLsn::new(200));
        assert!(outbox
            .acknowledge(&second)
            .await
            .expect("second ack should succeed"));
        assert_eq!(
            outbox
                .target_frontier(&target)
                .await
                .expect("frontier should load"),
            CommitLsn::ZERO
        );
        assert!(outbox
            .acknowledge(&first)
            .await
            .expect("first ack should succeed"));
        assert_eq!(
            outbox
                .target_frontier(&target)
                .await
                .expect("frontier should load"),
            CommitLsn::new(200)
        );
    }

    #[tokio::test]
    async fn stale_lease_cannot_ack() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let target = TargetId::new("fulltext").expect("target should be valid");
        let outbox = SqliteOutbox::open(directory.path().join("outbox.sqlite"))
            .await
            .expect("outbox should open");
        outbox
            .create_index_generation(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should be created");
        outbox
            .transition_generation_to_backfilling(&target, 1, 1)
            .await
            .expect("generation should transition to backfilling");
        outbox
            .transition_generation_to_catching_up(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should transition to catching up");
        outbox
            .activate_generation(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should activate");
        outbox
            .materialize_commit(
                CommitLsn::new(10),
                &[intent(0, 1, &target)],
                std::slice::from_ref(&target),
            )
            .await
            .expect("commit should materialize");
        let stale = outbox
            .claim_next(&target, "worker-1", 0, 10)
            .await
            .expect("claim should succeed")
            .expect("event should exist");
        let current = outbox
            .claim_next(&target, "worker-2", 11, 10)
            .await
            .expect("reclaim should succeed")
            .expect("event should be reclaimed");
        assert!(!outbox
            .acknowledge(&stale)
            .await
            .expect("stale ack should be rejected cleanly"));
        assert!(outbox
            .acknowledge(&current)
            .await
            .expect("current ack should succeed"));
    }

    #[tokio::test]
    async fn snapshot_is_atomic_and_checksum_verified() {
        let directory = TempDir::new().expect("temporary directory");
        let path = directory.path().join("outbox.sqlite");
        let snapshot_path = directory.path().join("checkpoints/outbox.sqlite");
        let outbox = SqliteOutbox::open(&path).await.expect("outbox should open");
        let snapshot = outbox
            .create_snapshot(&snapshot_path)
            .await
            .expect("snapshot should be created");
        SqliteOutbox::verify_snapshot(&snapshot).expect("snapshot checksum should verify");
        let mut bytes = std::fs::read(&snapshot_path).expect("snapshot should be readable");
        bytes[0] ^= 0xff;
        std::fs::write(&snapshot_path, bytes).expect("snapshot should be corrupted");
        assert!(SqliteOutbox::verify_snapshot(&snapshot).is_err());
    }

    #[tokio::test]
    async fn diagnostics_report_backlog_and_generation_frontiers() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let target = TargetId::new("fulltext").expect("target should be valid");
        let outbox = SqliteOutbox::open(directory.path().join("outbox.sqlite"))
            .await
            .expect("outbox should open");
        outbox
            .create_index_generation(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should be created");
        outbox
            .transition_generation_to_backfilling(&target, 1, 1)
            .await
            .expect("generation should transition to backfilling");
        outbox
            .transition_generation_to_catching_up(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should transition to catching up");
        outbox
            .activate_generation(&target, 1, 1, CommitLsn::ZERO)
            .await
            .expect("generation should activate");
        outbox
            .materialize_commit(
                CommitLsn::new(42),
                &[intent(0, 1, &target)],
                std::slice::from_ref(&target),
            )
            .await
            .expect("commit should materialize");

        let diagnostics = outbox.diagnostics().await.expect("diagnostics should load");
        assert_eq!(diagnostics.materialized_lsn, CommitLsn::new(42));
        assert_eq!(diagnostics.targets.len(), 1);
        assert_eq!(diagnostics.targets[0].target, "fulltext");
        assert_eq!(diagnostics.targets[0].pending, 1);
        assert_eq!(diagnostics.targets[0].frontier_lag, 42);
        assert!(diagnostics.targets[0].oldest_event_age_ms.is_some());
        assert_eq!(diagnostics.indexes.len(), 1);
        assert_eq!(diagnostics.indexes[0].state, "active");
        assert_eq!(diagnostics.indexes[0].frontier_lag, 42);

        let stats = outbox.stats().await.expect("outbox stats should load");
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.leased, 0);
        assert_eq!(stats.dead_lettered, 0);
        assert_eq!(stats.retries, 0);

        let event = outbox
            .claim_next(&target, "stats-worker", 0, 1_000)
            .await
            .expect("event should be claimable")
            .expect("event should exist");
        let leased = outbox.stats().await.expect("leased stats should load");
        assert_eq!(leased.pending, 0);
        assert_eq!(leased.leased, 1);
        outbox
            .retry(&event, 1_001, "temporary failure")
            .await
            .expect("event should be retryable");
        let retrying = outbox.stats().await.expect("retry stats should load");
        assert_eq!(retrying.pending, 1);
        assert_eq!(retrying.leased, 0);
        assert_eq!(retrying.retries, 1);
    }
}
