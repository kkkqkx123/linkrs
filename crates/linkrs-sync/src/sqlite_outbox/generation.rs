//! Rebuild generation state machine for derived indexes.
//! One responsibility of [`SqliteOutbox`](super::SqliteOutbox); the method set is unchanged.

use super::*;

impl SqliteOutbox {
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

    /// List all known generations (with states) for one index. Used by the
    /// rebuild driver to detect and fail generations left behind by crashes.
    pub async fn list_index_generations(
        &self,
        target: &TargetId,
        index_id: u64,
    ) -> Result<Vec<(u64, String)>, String> {
        let rows = sqlx::query(
            "SELECT generation, state FROM generation_state \
             WHERE target = ? AND index_id = ? ORDER BY generation",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|row| {
                let generation: i64 = row.get("generation");
                let state: String = row.get("state");
                Ok((from_sql_i64(generation, "index generation")?, state))
            })
            .collect()
    }

    /// Fetch materialized events for catch-up replay in a commit-LSN range.
    /// Status-agnostic on purpose: retried, dead-lettered, or skipped events
    /// in the range are replayed idempotently so the rebuilt index converges
    /// even when the live delivery plane degraded mid-rebuild.
    pub async fn fetch_rebuild_events(
        &self,
        target: &TargetId,
        index_id: u64,
        after_lsn: CommitLsn,
        upto_lsn: CommitLsn,
        limit: u64,
    ) -> Result<Vec<RebuildEvent>, String> {
        let rows = sqlx::query(
            "SELECT commit_lsn, intent_sequence, mutation FROM events \
             WHERE target = ? AND index_id = ? AND commit_lsn > ? AND commit_lsn <= ? \
             ORDER BY commit_lsn, intent_sequence LIMIT ?",
        )
        .bind(target.as_str())
        .bind(to_sql_i64(index_id, "index ID")?)
        .bind(to_sql_i64(after_lsn.get(), "after LSN")?)
        .bind(to_sql_i64(upto_lsn.get(), "upto LSN")?)
        .bind(to_sql_i64(limit, "fetch limit")?)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|row| {
                let commit_lsn: i64 = row.get("commit_lsn");
                let mutation_bytes: Vec<u8> = row.get("mutation");
                let mutation: IndexMutation =
                    postcard::from_bytes(&mutation_bytes).map_err(|error| error.to_string())?;
                Ok(RebuildEvent {
                    commit_lsn: CommitLsn::new(from_sql_i64(commit_lsn, "commit LSN")?),
                    mutation,
                })
            })
            .collect()
    }
}
