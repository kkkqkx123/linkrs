//! Retention and pruning of applied events, archives and degraded ranges.
//! One responsibility of [`SqliteOutbox`](super::SqliteOutbox); the method set is unchanged.

use super::*;

impl SqliteOutbox {
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
}
