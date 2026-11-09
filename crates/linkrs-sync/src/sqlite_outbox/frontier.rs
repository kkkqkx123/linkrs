//! Projection frontiers, delivery watermarks and diagnostics.
//! One responsibility of [`SqliteOutbox`](super::SqliteOutbox); the method set is unchanged.

use super::*;

impl SqliteOutbox {
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
