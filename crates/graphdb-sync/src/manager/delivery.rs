//! Outbox delivery: claim-apply-ack loop and per-target mutation appliers.
use super::*;
use crate::outbox::OutboxPayload;
#[cfg(any(feature = "fulltext", feature = "vector"))]
use crate::types::ChangeType;
#[cfg(feature = "vector")]
use crate::vector_sync::VectorSyncCoordinator;
use graphdb_core::types::CommitLsn;
#[cfg(feature = "fulltext")]
use graphdb_core::Value;
#[cfg(feature = "fulltext")]
use graphdb_fulltext::engine::FulltextSearchEngine;
use graphdb_metrics::OutboxState;
#[cfg(feature = "vector")]
use std::collections::HashMap;
#[cfg(feature = "fulltext")]
pub(crate) struct FulltextFieldApply<'a> {
    manager: Arc<graphdb_fulltext::manager::FulltextIndexManager>,
    mutation: &'a graphdb_core::wal::IndexMutation,
    commit_lsn: CommitLsn,
    space_id: u64,
    index_name: &'a str,
    entity_id: String,
    properties: Vec<(String, Value)>,
    deleted: bool,
    /// Rebuild catch-up replay targets the scratch engine instead of the
    /// live map lookup. `None` = resolve via the manager (live delivery).
    engine_override: Option<Arc<dyn FulltextSearchEngine>>,
}
#[cfg(feature = "fulltext")]
pub(crate) fn edge_entity_id(
    src: impl std::fmt::Display,
    dst: impl std::fmt::Display,
    ranking: i64,
) -> String {
    format!("{}->{}#{}", src, dst, ranking)
}
#[cfg_attr(
    not(any(feature = "fulltext", feature = "vector")),
    allow(unused_variables)
)]
impl super::SyncManager {
    pub fn retry_outbox_sync(&self) -> Result<usize, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok(0);
        };
        // Auth pause fence: if the last failure was an auth error, suppress
        // delivery until the pause window expires.
        let now_for_pause = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let paused_until = self
            .auth_paused_until_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        if paused_until > now_for_pause {
            tracing::warn!(
                paused_until,
                now = now_for_pause,
                "outbox delivery paused due to auth failure"
            );
            return Ok(0);
        }
        let stats_manager = self.stats_manager.clone();
        // Capture consumer config for the async block (Clone is cheap).
        let consumer = self.outbox_consumer.clone();
        let backend_policy_name = {
            #[cfg(feature = "vector")]
            {
                self.backend_policy
                    .as_ref()
                    .map(|p| p.policy_name.clone())
                    .unwrap_or_default()
            }
            #[cfg(not(feature = "vector"))]
            {
                String::new()
            }
        };
        let self_clone = self.clone();
        self.execute_sync(move || async move {
            let consumer = consumer.clone();
            let stats_manager = stats_manager.clone();
            let self_clone = self_clone.clone();
            let backend_policy_name = backend_policy_name.clone();
            let targets = outbox
                .delivery_targets()
                .await
                .map_err(SyncError::PersistenceError)?;
            let mut processed = 0usize;
            for target in targets {
                let max_concurrency = if target.as_str() == "vector"
                    && backend_policy_name == "qdrant"
                {
                    consumer.max_concurrency.max(4)
                } else if target.as_str() == "vector" && backend_policy_name == "local" {
                    1
                } else {
                    consumer.max_concurrency.max(1)
                };
                if max_concurrency <= 1 {
                    // Single-threaded path (Local): preserves global
                    // commit_lsn ordering and keeps SQLite contention minimal.
                    let mut target_processed = 0usize;
                    while target_processed < consumer.batch_size {
                        let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
                        // Re-check auth pause inside loop in case a worker set it.
                        let paused = self_clone
                            .auth_paused_until_ms
                            .load(std::sync::atomic::Ordering::Relaxed);
                        if paused > now {
                            break;
                        }
                        let Some(event) = outbox
                            .claim_next(
                                &target,
                                &consumer.consumer_id,
                                now,
                                consumer.lease_duration_ms,
                            )
                            .await
                            .map_err(SyncError::PersistenceError)?
                        else {
                            break;
                        };
                        let apply_started = std::time::Instant::now();
                        let result = self_clone
                            .apply_index_mutation(&event.mutation, event.commit_lsn)
                            .await;
                        match result {
                            Ok(()) => {
                                if let Some(stats) = &stats_manager {
                                    stats.record_transport_latency(
                                        apply_started.elapsed().as_millis() as u64
                                    );
                                }
                                outbox
                                    .acknowledge(&event)
                                    .await
                                    .map_err(SyncError::PersistenceError)?;
                            }
                            Err(error) => {
                                if let Some(stats) = &stats_manager {
                                    stats.record_transport_latency(
                                        apply_started.elapsed().as_millis() as u64
                                    );
                                }
                                // Qdrant disabled events are retained for retry after
                                // engine recovery and must not be dead-lettered even after
                                // max_retries. Detect by the EngineDisabled message.
                                let is_disabled_error = error.contains("Vector engine is disabled")
                                    || error.contains("EngineDisabled");
                                if is_disabled_error {
                                    let backoff = 5_000u64;
                                    outbox
                                        .retry(&event, now.saturating_add(backoff), &error)
                                        .await
                                        .map_err(SyncError::PersistenceError)?;
                                } else {
                                    // Classify the error so NonRetryable goes straight to
                                    // dead_letter and Auth pauses delivery.
                                    let kind = crate::vector_error::VectorErrorKind::classify_str(
                                        &error,
                                    );
                                    match kind {
                                        crate::vector_error::VectorErrorKind::Auth => {
                                            let pause_until = now.saturating_add(60_000);
                                            self_clone.auth_paused_until_ms.store(
                                                pause_until,
                                                std::sync::atomic::Ordering::Relaxed,
                                            );
                                            tracing::error!(
                                                target = target.as_str(),
                                                error = %error,
                                                pause_until,
                                                "auth failure: pausing outbox delivery for 60s"
                                            );
                                            let backoff = 60_000u64;
                                            outbox
                                                .retry(
                                                    &event,
                                                    now.saturating_add(backoff),
                                                    &error,
                                                )
                                                .await
                                                .map_err(SyncError::PersistenceError)?;
                                            // Pause the target after recording the retry.
                                            break;
                                        }
                                        crate::vector_error::VectorErrorKind::NonRetryable => {
                                            outbox
                                                .dead_letter(&event, now, &error)
                                                .await
                                                .map_err(SyncError::PersistenceError)?;
                                        }
                                        crate::vector_error::VectorErrorKind::Retryable => {
                                            let retry_count = outbox
                                                .retry_count(event.event_id)
                                                .await
                                                .map_err(SyncError::PersistenceError)?;
                                            if retry_count.saturating_add(1)
                                                >= consumer.max_retries
                                            {
                                                outbox
                                                    .dead_letter(&event, now, &error)
                                                    .await
                                                    .map_err(SyncError::PersistenceError)?;
                                            } else {
                                                let backoff = 100u64
                                                    .saturating_mul(1u64 << retry_count.min(16))
                                                    .min(300_000);
                                                outbox
                                                    .retry(
                                                        &event,
                                                        now.saturating_add(backoff),
                                                        &error,
                                                    )
                                                    .await
                                                    .map_err(SyncError::PersistenceError)?;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        processed = processed.saturating_add(1);
                        target_processed = target_processed.saturating_add(1);
                    }
                } else {
                    // Concurrent delivery (Qdrant): multiple workers claim with
                    // distinct lease owners so `claim_next`'s BEGIN IMMEDIATE +
                    // lease_epoch fencing guarantees no overlapping acks. Each
                    // worker respects the global commit_lsn order via the
                    // `NOT EXISTS (ordering_key)` fence plus `ORDER BY`.
                    let per_worker_batch =
                        consumer.batch_size.div_ceil(max_concurrency);
                    let outbox_for_workers = outbox.clone();
                    let target_for_workers = target.clone();
                    let consumer_for_workers = consumer.clone();
                    let self_for_workers = self_clone.clone();
                    let stats_for_workers = stats_manager.clone();
                    let mut handles = Vec::with_capacity(max_concurrency);
                    for worker_idx in 0..max_concurrency {
                        let outbox_c = outbox_for_workers.clone();
                        let target_c = target_for_workers.clone();
                        let consumer_c = consumer_for_workers.clone();
                        let self_c = self_for_workers.clone();
                        let stats_c = stats_for_workers.clone();
                        let worker_consumer_id =
                            format!("{}-worker-{}", consumer_c.consumer_id, worker_idx);
                        handles.push(tokio::spawn(async move {
                            let mut local_processed = 0usize;
                            while local_processed < per_worker_batch {
                                let now =
                                    chrono::Utc::now().timestamp_millis().max(0) as u64;
                                let paused = self_c
                                    .auth_paused_until_ms
                                    .load(std::sync::atomic::Ordering::Relaxed);
                                if paused > now {
                                    break;
                                }
                                let Some(event) = outbox_c
                                    .claim_next(
                                        &target_c,
                                        &worker_consumer_id,
                                        now,
                                        consumer_c.lease_duration_ms,
                                    )
                                    .await
                                    .map_err(SyncError::PersistenceError)?
                                else {
                                    break;
                                };
                                let apply_started = std::time::Instant::now();
                                let result = self_c
                                    .apply_index_mutation(&event.mutation, event.commit_lsn)
                                    .await;
                                match result {
                                    Ok(()) => {
                                        if let Some(stats) = &stats_c {
                                            stats.record_transport_latency(
                                                apply_started.elapsed().as_millis() as u64
                                            );
                                        }
                                        outbox_c
                                            .acknowledge(&event)
                                            .await
                                            .map_err(SyncError::PersistenceError)?;
                                    }
                                    Err(error) => {
                                        if let Some(stats) = &stats_c {
                                            stats.record_transport_latency(
                                                apply_started.elapsed().as_millis() as u64
                                            );
                                        }
                                        let is_disabled_error = error
                                            .contains("Vector engine is disabled")
                                            || error.contains("EngineDisabled");
                                        if is_disabled_error {
                                            let backoff = 5_000u64;
                                            outbox_c
                                                .retry(
                                                    &event,
                                                    now.saturating_add(backoff),
                                                    &error,
                                                )
                                                .await
                                                .map_err(SyncError::PersistenceError)?;
                                        } else {
                                            let kind =
                                                crate::vector_error::VectorErrorKind::classify_str(
                                                    &error,
                                                );
                                            match kind {
                                                crate::vector_error::VectorErrorKind::Auth => {
                                                    let pause_until = now.saturating_add(60_000);
                                                    self_c.auth_paused_until_ms.store(
                                                        pause_until,
                                                        std::sync::atomic::Ordering::Relaxed,
                                                    );
                                                    tracing::error!(
                                                        target = target_c.as_str(),
                                                        error = %error,
                                                        pause_until,
                                                        "auth failure: pausing outbox delivery"
                                                    );
                                                    outbox_c
                                                        .retry(
                                                            &event,
                                                            now.saturating_add(60_000),
                                                            &error,
                                                        )
                                                        .await
                                                        .map_err(
                                                            SyncError::PersistenceError,
                                                        )?;
                                                    break;
                                                }
                                                crate::vector_error::VectorErrorKind::NonRetryable => {
                                                    outbox_c
                                                        .dead_letter(&event, now, &error)
                                                        .await
                                                        .map_err(
                                                            SyncError::PersistenceError,
                                                        )?;
                                                }
                                                crate::vector_error::VectorErrorKind::Retryable => {
                                                    let retry_count = outbox_c
                                                        .retry_count(event.event_id)
                                                        .await
                                                        .map_err(
                                                            SyncError::PersistenceError,
                                                        )?;
                                                    if retry_count.saturating_add(1)
                                                        >= consumer_c.max_retries
                                                    {
                                                        outbox_c
                                                            .dead_letter(&event, now, &error)
                                                            .await
                                                            .map_err(
                                                                SyncError::PersistenceError,
                                                            )?;
                                                    } else {
                                                        let backoff = 100u64
                                                            .saturating_mul(
                                                                1u64 << retry_count.min(16),
                                                            )
                                                            .min(300_000);
                                                        outbox_c
                                                            .retry(
                                                                &event,
                                                                now.saturating_add(backoff),
                                                                &error,
                                                            )
                                                            .await
                                                            .map_err(
                                                                SyncError::PersistenceError,
                                                            )?;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                local_processed += 1;
                            }
                            Ok::<usize, SyncError>(local_processed)
                        }));
                    }
                    for h in handles {
                        match h.await {
                            Ok(Ok(n)) => processed = processed.saturating_add(n),
                            Ok(Err(e)) => return Err(e),
                            Err(e) => return Err(SyncError::Internal(e.to_string())),
                        }
                    }
                }
            }
            if let Some(stats) = &stats_manager {
                let durable = outbox.stats().await.map_err(SyncError::PersistenceError)?;
                let diagnostics = outbox
                    .diagnostics()
                    .await
                    .map_err(SyncError::PersistenceError)?;
                let frontier_lag = diagnostics
                    .targets
                    .iter()
                    .map(|target| target.frontier_lag)
                    .chain(diagnostics.indexes.iter().map(|index| index.frontier_lag))
                    .max()
                    .unwrap_or(0);
                let degraded = diagnostics.targets.iter().any(|target| target.degraded)
                    || diagnostics.indexes.iter().any(|index| index.degraded);
                stats.record_outbox_state(OutboxState {
                    pending: durable.pending as u64,
                    retries: durable.retries,
                    dead_lettered: durable.dead_lettered as u64,
                    leased: durable.leased as u64,
                    oldest_event_age_ms: durable.oldest_event_age_ms,
                    frontier_lag,
                    degraded,
                });
                // Per-target lag for granular alerting.
                for target in &diagnostics.targets {
                    stats.record_target_frontier_lag(&target.target, target.frontier_lag);
                }
                for index in &diagnostics.indexes {
                    let label = format!("{}:{}", index.target, index.index_id);
                    stats.record_target_frontier_lag(&label, index.frontier_lag);
                }
                #[cfg(feature = "vector")]
                if let Some(coord) = self.vector_coordinator.as_ref() {
                    stats.record_vector_disabled_skips(coord.disabled_skip_count());
                }
            }
            Ok(processed)
        })
    }

    async fn apply_index_mutation(
        &self,
        mutation: &graphdb_core::wal::IndexMutation,
        commit_lsn: CommitLsn,
    ) -> Result<(), String> {
        let payload: OutboxPayload = postcard::from_bytes(&mutation.document_or_vector)
            .map_err(|error| format!("Failed to decode index mutation: {}", error))?;
        match mutation.target.as_str() {
            #[cfg(feature = "fulltext")]
            "fulltext" => {
                self.apply_fulltext_mutation(mutation, commit_lsn, &payload, None)
                    .await
            }
            #[cfg(feature = "vector")]
            "vector" => {
                self.apply_vector_mutation(mutation, commit_lsn, &payload)
                    .await
            }
            _ => self.apply_payload(&payload),
        }
    }

    fn apply_payload(&self, payload: &OutboxPayload) -> Result<(), String> {
        match payload {
            OutboxPayload::CreateIndex { .. }
            | OutboxPayload::DropIndex { .. }
            | OutboxPayload::DropSpace { .. }
            | OutboxPayload::DropTag { .. } => Ok(()),
            OutboxPayload::Vertex { .. }
            | OutboxPayload::EdgeInsert { .. }
            | OutboxPayload::EdgeDelete { .. } => Err(
                "Outbox mutation has no registered target receiver; direct delivery is disabled"
                    .to_string(),
            ),
        }
    }

    #[cfg(feature = "fulltext")]
    pub(crate) async fn apply_fulltext_mutation(
        &self,
        mutation: &graphdb_core::wal::IndexMutation,
        commit_lsn: CommitLsn,
        payload: &OutboxPayload,
        engine_override: Option<Arc<dyn FulltextSearchEngine>>,
    ) -> Result<(), String> {
        let manager = self
            .sync_coordinator
            .as_ref()
            .ok_or_else(|| "fulltext target is not configured".to_string())?
            .fulltext_manager()
            .clone();

        match payload {
            OutboxPayload::Vertex {
                space_id,
                tag_name,
                vertex_id,
                properties,
                change_type,
            } => {
                let properties =
                    if matches!(change_type, ChangeType::Delete) && properties.is_empty() {
                        manager
                            .get_space_indexes(*space_id)
                            .into_iter()
                            .filter(|metadata| metadata.tag_name == *tag_name)
                            .map(|metadata| (metadata.field_name, Value::string("")))
                            .collect()
                    } else {
                        properties.clone()
                    };
                Self::apply_fulltext_fields(FulltextFieldApply {
                    manager: manager.clone(),
                    mutation,
                    commit_lsn,
                    space_id: *space_id,
                    index_name: tag_name,
                    entity_id: crate::rebuild::fulltext_vertex_doc_id(vertex_id),
                    properties,
                    deleted: matches!(change_type, ChangeType::Delete),
                    engine_override: engine_override.clone(),
                })
                .await
            }
            OutboxPayload::EdgeInsert { space_id, edge } => {
                let properties = edge
                    .props
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect::<Vec<_>>();
                Self::apply_fulltext_fields(FulltextFieldApply {
                    manager: manager.clone(),
                    mutation,
                    commit_lsn,
                    space_id: *space_id,
                    index_name: &edge.edge_type,
                    entity_id: edge_entity_id(edge.src, edge.dst, edge.ranking),
                    properties,
                    deleted: false,
                    engine_override: engine_override.clone(),
                })
                .await
            }
            OutboxPayload::EdgeDelete {
                space_id,
                src,
                dst,
                edge_type,
                ranking,
            } => {
                let properties = manager
                    .get_space_indexes(*space_id)
                    .into_iter()
                    .filter(|metadata| metadata.tag_name == *edge_type)
                    .map(|metadata| (metadata.field_name, Value::string("")))
                    .collect::<Vec<_>>();
                Self::apply_fulltext_fields(FulltextFieldApply {
                    manager,
                    mutation,
                    commit_lsn,
                    space_id: *space_id,
                    index_name: edge_type,
                    entity_id: edge_entity_id(src, dst, *ranking),
                    properties,
                    deleted: true,
                    engine_override: engine_override.clone(),
                })
                .await
            }
            OutboxPayload::CreateIndex {
                space_id,
                index_name,
                schema_name,
                fields,
                ..
            } => {
                for (field_name, value_type) in fields {
                    if !matches!(value_type, Value::String(_) | Value::FixedString(_)) {
                        continue;
                    }
                    if manager
                        .create_index(*space_id, schema_name, field_name, None)
                        .await
                        .is_err()
                        && !manager.has_index(*space_id, schema_name, field_name)
                    {
                        return Err(format!(
                            "Failed to create fulltext receiver index {}.{}.{}",
                            space_id, index_name, field_name
                        ));
                    }
                }
                Ok(())
            }
            OutboxPayload::DropIndex {
                space_id,
                index_name: _index_name,
                schema_name,
                ..
            } => {
                let indexes = manager
                    .get_space_indexes(*space_id)
                    .into_iter()
                    .filter(|metadata| metadata.tag_name == *schema_name)
                    .map(|metadata| metadata.field_name)
                    .collect::<Vec<_>>();
                for field_name in indexes {
                    manager
                        .drop_index(*space_id, schema_name, &field_name)
                        .await
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            }
            OutboxPayload::DropSpace { space_id } => {
                manager
                    .drop_space_indexes(*space_id)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(())
            }
            OutboxPayload::DropTag { space_id, tag_name } => {
                let indexes = manager
                    .get_space_indexes(*space_id)
                    .into_iter()
                    .filter(|metadata| metadata.tag_name == *tag_name)
                    .map(|metadata| metadata.field_name)
                    .collect::<Vec<_>>();
                for field_name in indexes {
                    manager
                        .drop_index(*space_id, tag_name, &field_name)
                        .await
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            }
        }
    }

    #[cfg(feature = "fulltext")]
    pub(crate) async fn apply_fulltext_fields(
        request: FulltextFieldApply<'_>,
    ) -> Result<(), String> {
        for (field_name, value) in request.properties {
            // Serialize against rebuild publish: the fence write guard held
            // by publish covers final replay and engine swap, so an apply is
            // either fully before or fully after the swap.
            let fence = request.manager.publish_fence_for(
                request.space_id,
                request.index_name,
                &field_name,
            );
            let _fence = fence.read().await;
            let engine = match request.engine_override.clone() {
                Some(engine) => Some(engine),
                None => {
                    request
                        .manager
                        .get_engine(request.space_id, request.index_name, &field_name)
                }
            };
            let Some(engine) = engine else {
                continue;
            };
            let document = if request.deleted {
                Vec::new()
            } else if let Value::String(text) = value {
                text.as_bytes().to_vec()
            } else {
                continue;
            };
            let mut field_mutation = request.mutation.clone();
            field_mutation.index_id = stable_hash(
                format!("{}:{}:{}", request.space_id, request.index_name, field_name).as_bytes(),
            );
            field_mutation.document_or_vector = document;
            field_mutation.idempotency_key = graphdb_core::types::IdempotencyKey::new(format!(
                "{}:{}",
                request.mutation.idempotency_key.as_str(),
                field_name
            ))?;
            let receiver = crate::receiver::FulltextReceiver::new(engine);
            let late_arrival = receiver
                .check_late_arrival(request.commit_lsn, field_mutation.idempotency_key.as_str())
                .await;
            if !late_arrival.accepted && !late_arrival.reason.contains("duplicate") {
                return Err(late_arrival.reason);
            }
            receiver
                .apply_index_batch(&[(&field_mutation, request.commit_lsn)])
                .await?;
            log::debug!(
                "Applied fulltext mutation for {} at {}",
                request.entity_id.as_str(),
                request.commit_lsn
            );
        }
        Ok(())
    }

    #[cfg(feature = "vector")]
    async fn apply_vector_mutation(
        &self,
        mutation: &graphdb_core::wal::IndexMutation,
        commit_lsn: CommitLsn,
        payload: &OutboxPayload,
    ) -> Result<(), String> {
        let Some(coordinator) = self.vector_coordinator.as_ref() else {
            return Err("vector target is not configured".to_string());
        };
        let Some(receiver) = self.vector_receiver.as_ref() else {
            return Err("vector receiver is not configured".to_string());
        };
        let late_arrival = receiver
            .check_late_arrival(commit_lsn, mutation.idempotency_key.as_str())
            .await;
        if !late_arrival.accepted && !late_arrival.reason.contains("duplicate") {
            return Err(late_arrival.reason);
        }
        if !late_arrival.accepted {
            return Ok(());
        }

        let mut contexts = Vec::new();
        match payload {
            OutboxPayload::Vertex { .. }
            | OutboxPayload::EdgeInsert { .. }
            | OutboxPayload::EdgeDelete { .. } => {
                // Vertex/edge data-plane mapping is shared with rebuild
                // catch-up replay (`vector_contexts_for_payload`).
                contexts = Self::vector_contexts_for_payload(coordinator, payload)?;
            }
            OutboxPayload::CreateIndex {
                space_id,
                index_name: _index_name,
                schema_name,
                fields,
                ..
            } => {
                for (field_name, value_type) in fields {
                    let Some(vector_size) = value_type.as_vector().map(|vector| vector.len())
                    else {
                        continue;
                    };
                    coordinator
                        .create_vector_index(
                            *space_id,
                            schema_name,
                            field_name,
                            vector_size,
                            vector_search::DistanceMetric::default(),
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            OutboxPayload::DropIndex {
                space_id,
                index_name: _index_name,
                schema_name,
                fields,
                ..
            } => {
                for field_name in fields {
                    coordinator
                        .drop_vector_index(*space_id, schema_name, field_name)
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            OutboxPayload::DropSpace { space_id } => {
                coordinator
                    .index_manager()
                    .drop_space_indexes(*space_id)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            OutboxPayload::DropTag { space_id, tag_name } => {
                coordinator
                    .index_manager()
                    .drop_tag_indexes(*space_id, tag_name)
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
        // Serialize data-plane delivery against rebuild purge/snapshot and
        // the final catch-up drain: an apply is either fully before or
        // fully after each fenced window.
        let mut fence_keys: Vec<(u64, String, String)> = Vec::new();
        for ctx in &contexts {
            let key = (
                ctx.location.space_id,
                ctx.location.tag_name.clone(),
                ctx.location.field_name.clone(),
            );
            if !fence_keys.contains(&key) {
                fence_keys.push(key);
            }
        }
        fence_keys.sort();
        let mut _fence_guards = Vec::with_capacity(fence_keys.len());
        for (space_id, tag_name, field_name) in &fence_keys {
            let fence = coordinator.publish_fence_for(*space_id, tag_name, field_name);
            _fence_guards.push(fence.clone().read_owned().await);
        }
        if !contexts.is_empty() {
            coordinator
                .on_vector_change_batch(contexts)
                .await
                .map_err(|error| error.to_string())?;
        }
        receiver
            .record_application(commit_lsn, mutation.idempotency_key.as_str())
            .await
    }

    /// Map one vertex/edge outbox payload to vector change contexts without
    /// any receiver gating. Shared by live delivery (`apply_vector_mutation`,
    /// which gates + batches + records) and rebuild catch-up replay (which
    /// must accept LSNs below the receiver water-level and never records).
    #[cfg(feature = "vector")]
    pub(crate) fn vector_contexts_for_payload(
        coordinator: &VectorSyncCoordinator,
        payload: &OutboxPayload,
    ) -> Result<Vec<crate::vector_sync::VectorChangeContext>, String> {
        let mut contexts = Vec::new();
        match payload {
            OutboxPayload::Vertex {
                space_id,
                tag_name,
                vertex_id,
                properties,
                change_type,
            } => {
                for (field_name, value) in properties {
                    if !coordinator.index_exists(*space_id, tag_name, field_name) {
                        continue;
                    }
                    let (vector, vector_change_type) = match (value.as_vector(), change_type) {
                        (Some(vector), ChangeType::Insert | ChangeType::Update) => (
                            vector.to_vec(),
                            crate::vector_sync::VectorChangeType::Insert,
                        ),
                        _ => (Vec::new(), crate::vector_sync::VectorChangeType::Delete),
                    };
                    let payload = properties
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect::<HashMap<_, _>>();
                    contexts.push(crate::vector_sync::VectorChangeContext::new(
                        *space_id,
                        tag_name,
                        field_name,
                        vector_change_type,
                        crate::vector_sync::VectorPointData {
                            id: format_vector_point_id(vertex_id, tag_name, field_name),
                            vector,
                            payload,
                        },
                    ));
                }
                if matches!(change_type, ChangeType::Delete) {
                    let staged_fields = properties
                        .iter()
                        .map(|(field_name, _)| field_name.as_str())
                        .collect::<std::collections::HashSet<_>>();
                    for metadata in coordinator.list_indexes() {
                        if metadata.space_id == *space_id
                            && metadata.tag_name == *tag_name
                            && !staged_fields.contains(metadata.field_name.as_str())
                        {
                            contexts.push(crate::vector_sync::VectorChangeContext::new(
                                *space_id,
                                tag_name,
                                &metadata.field_name,
                                crate::vector_sync::VectorChangeType::Delete,
                                crate::vector_sync::VectorPointData {
                                    id: format_vector_point_id(
                                        vertex_id,
                                        tag_name,
                                        &metadata.field_name,
                                    ),
                                    vector: Vec::new(),
                                    payload: HashMap::new(),
                                },
                            ));
                        }
                    }
                }
            }
            // Edge mutations do not trigger vector index updates. Vector
            // indexes are maintained per-vertex; edge-only changes do not
            // carry vector fields and are not indexed.
            OutboxPayload::EdgeInsert { .. } | OutboxPayload::EdgeDelete { .. } => {}
            _ => {
                return Err(
                    "vector_contexts_for_payload only maps vertex/edge payloads".to_string()
                );
            }
        }
        Ok(contexts)
    }
}
