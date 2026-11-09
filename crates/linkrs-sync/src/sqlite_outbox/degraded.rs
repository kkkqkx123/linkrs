//! Degraded ranges: skipping, tracking and clearing poisoned spans.
//! One responsibility of [`SqliteOutbox`](super::SqliteOutbox); the method set is unchanged.

use super::*;

impl SqliteOutbox {
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
}
