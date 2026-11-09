//! Durable event lifecycle: materialization, claim, acknowledgement, retry and dead letters.
//! One responsibility of [`SqliteOutbox`](super::SqliteOutbox); the method set is unchanged.

use super::*;

impl SqliteOutbox {
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

    pub async fn retry_count(&self, event_id: i64) -> Result<u64, String> {
        let value: i64 = sqlx::query_scalar("SELECT retry_count FROM events WHERE id = ?")
            .bind(event_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        from_sql_i64(value, "retry count")
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
}
