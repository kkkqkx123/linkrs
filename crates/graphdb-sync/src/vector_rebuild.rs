#![cfg(feature = "vector")]

//! True online rebuild for vector indexes.
//!
//! Rebuilds follow the fulltext temp-swap shape: backfill and catch-up replay
//! converge a scratch (`*.rebuild-{generation}`) collection while the live
//! slice keeps serving reads and writes, then a fenced publish swaps the
//! temp in. Publish is the only irreversible step: any failure before it
//! discards the temp and fails the generation, leaving live data untouched;
//! rollback is "publish nothing".
//!
//! Publish diverges by granularity: Field indexes (one logical index per
//! physical collection) switch collection pointers (local backends rename
//! directories, remote backends remap the logical pointer); Space indexes
//! (sibling slices share one collection) copy the temp slice back into the
//! live collection under the fence, so siblings see zero impact.
//!
//! Correctness argument: backfill plus strictly-newer replay converges
//! because point upserts/deletes are idempotent by point ID and the final
//! drain (under the fence) replays everything up to the publish frontier
//! after backfill has fully completed. Replay bypasses the vector receiver
//! water-level (it must accept LSNs below it) and never records, so live
//! delivery accounting is untouched; overlap between backfill, replay
//! rounds, and live delivery only rewrites the same points.
//!
//! Anything after `activate_generation` cannot fail the rebuild, and any
//! failure before publish only fails the generation: the logical index stays
//! registered and live delivery keeps serving it untouched.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use graphdb_core::types::{CommitLsn, TargetId, VertexId};
use graphdb_core::Value;
use graphdb_fulltext::{IndexEvent, RebuildPhase, RebuildProgress};

use crate::backend::VectorBackend;
use crate::manager::{format_vector_point_id, stable_hash, SyncError, SyncManager};
use crate::outbox::OutboxPayload;
use crate::vector_sync::{
    VectorChangeContext, VectorChangeType, VectorIndexLocation, VectorPointData,
    VectorSyncCoordinator,
};

/// Outbox target name for vector intents. Must match the writer path.
const VECTOR_TARGET: &str = "vector";
/// Backfill points per delivery batch: bounds one group-commit txn.
const BACKFILL_BATCH_CHUNK: usize = 500;

/// One vertex snapshot enumerated from primary storage for backfill.
#[derive(Debug, Clone)]
pub struct VectorRebuildDoc {
    pub vertex_id: VertexId,
    pub properties: Vec<(String, Value)>,
}

/// Primary-storage scan feeding vector backfill. Batches stream; `None`
/// ends the scan.
#[async_trait]
pub trait VectorDocSource: Send + Sync {
    async fn next_batch(&mut self) -> Result<Option<Vec<VectorRebuildDoc>>, String>;
}

/// Tunables for [`SyncManager::rebuild_vector_index`].
#[derive(Debug, Clone)]
pub struct VectorRebuildOptions {
    /// Outbox events fetched per catch-up round.
    pub catchup_fetch_limit: u64,
    /// Non-final catch-up rounds before publish; the final drain always runs
    /// to the publish frontier.
    pub max_catchup_rounds: usize,
    /// Refuse to purge when the primary-storage source yields no documents.
    /// Purging an index with an empty or unreadable source would delete the
    /// live slice with nothing to backfill; aborting before the purge keeps
    /// the live data servable. Set to true only for intentional truncation
    /// (prefer explicit clear for that case).
    pub allow_empty_source: bool,
}

impl Default for VectorRebuildOptions {
    fn default() -> Self {
        Self {
            catchup_fetch_limit: 1000,
            max_catchup_rounds: 10,
            allow_empty_source: false,
        }
    }
}

/// Tag-level outbox index ID for vector intents of one space/tag.
/// Must match `payload_to_intent`: `stable_hash("vector:{space}:{tag}")`.
pub fn vector_tag_index_id(space_id: u64, tag_name: &str) -> u64 {
    stable_hash(format!("{VECTOR_TARGET}:{space_id}:{tag_name}").as_bytes())
}

fn vector_index_name(space_id: u64, tag_name: &str, field_name: &str) -> String {
    format!("vec_{space_id}_{tag_name}_{field_name}")
}

impl SyncManager {
    /// Rebuild one vector index from primary storage without dropping it.
    ///
    /// Reads keep serving the live collection; backfill and catch-up replay
    /// converge a temp collection, and a fenced publish swaps it in. Returns
    /// the number of applied point operations after activation. Failures
    /// before publish discard the temp and fail the generation, leaving the
    /// live slice untouched for retry.
    pub async fn rebuild_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        source: &mut dyn VectorDocSource,
        options: VectorRebuildOptions,
    ) -> Result<u64, SyncError> {
        let coordinator = self.vector_coordinator().cloned().ok_or_else(|| {
            SyncError::PersistenceError("vector target is not configured".to_string())
        })?;
        let outbox = self.sqlite_outbox_opt().ok_or_else(|| {
            SyncError::PersistenceError(
                "vector rebuild requires a configured durable outbox".to_string(),
            )
        })?;
        if !coordinator.index_exists(space_id, tag_name, field_name) {
            return Err(SyncError::PersistenceError(format!(
                "Vector index not found: {space_id}.{tag_name}.{field_name}"
            )));
        }
        if coordinator.is_disabled_engine() {
            return Err(SyncError::PersistenceError(
                "vector rebuild requires an active vector engine".to_string(),
            ));
        }
        // Admission: at most one rebuild per index (same rationale as the
        // fulltext driver).
        let rebuild_lock = self.rebuild_lock_for("vector", space_id, tag_name, field_name);
        let _rebuild_guard = rebuild_lock.try_lock_owned().map_err(|_| {
            SyncError::RebuildBusy(format!(
                "Vector rebuild already running for {space_id}.{tag_name}.{field_name}"
            ))
        })?;

        let target = TargetId::new(VECTOR_TARGET).map_err(SyncError::PersistenceError)?;
        let tag_index_id = vector_tag_index_id(space_id, tag_name);
        let active_generation = outbox
            .get_active_generation(&target, tag_index_id)
            .await
            .map_err(SyncError::PersistenceError)?
            .unwrap_or(1);
        let generation = active_generation.saturating_add(1);

        // Fail generations stranded by earlier crashed attempts so retry is
        // idempotent; the current attempt re-registers its own generation.
        let known = outbox
            .list_index_generations(&target, tag_index_id)
            .await
            .map_err(SyncError::PersistenceError)?;
        for (known_generation, state) in known {
            if known_generation != generation
                && matches!(
                    state.as_str(),
                    "creating" | "backfilling" | "catching_up" | "publishing"
                )
            {
                outbox
                    .fail_generation(&target, tag_index_id, known_generation)
                    .await
                    .map_err(SyncError::PersistenceError)?;
            }
        }
        outbox
            .create_index_generation(&target, tag_index_id, generation, CommitLsn::ZERO)
            .await
            .map_err(SyncError::PersistenceError)?;

        let index_name = vector_index_name(space_id, tag_name, field_name);
        coordinator
            .index_manager()
            .emit_rebuild_event(IndexEvent::VectorRebuildStarted {
                index_name: index_name.clone(),
                generation,
            });
        coordinator.update_rebuild_progress(RebuildProgress {
            space_id,
            tag_name: tag_name.to_string(),
            field_name: field_name.to_string(),
            generation,
            phase: RebuildPhase::Preparing,
            docs_scanned: 0,
            docs_applied: 0,
            docs_skipped: 0,
            started_at: chrono::Utc::now(),
        });

        let result = self
            .run_vector_rebuild_phases(
                &coordinator,
                outbox,
                &target,
                tag_index_id,
                generation,
                &index_name,
                space_id,
                tag_name,
                field_name,
                source,
                &options,
            )
            .await;
        match &result {
            Ok(applied) => {
                coordinator.set_rebuild_phase(
                    space_id,
                    tag_name,
                    field_name,
                    RebuildPhase::Completed,
                );
                coordinator.index_manager().emit_rebuild_event(
                    IndexEvent::VectorRebuildCompleted {
                        index_name,
                        generation,
                        vectors_count: *applied,
                    },
                );
            }
            Err(error) => {
                let reason = error.to_string();
                if let Some(stats) = self.stats_manager_opt() {
                    stats.record_generation_rebuild_failure();
                }
                outbox
                    .fail_generation(&target, tag_index_id, generation)
                    .await
                    .ok();
                coordinator.set_rebuild_phase(space_id, tag_name, field_name, RebuildPhase::Failed);
                coordinator
                    .index_manager()
                    .emit_rebuild_event(IndexEvent::VectorRebuildFailed {
                        index_name,
                        generation,
                        reason,
                    });
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_vector_rebuild_phases(
        &self,
        coordinator: &Arc<VectorSyncCoordinator>,
        outbox: &crate::sqlite_outbox::SqliteOutbox,
        target: &TargetId,
        tag_index_id: u64,
        generation: u64,
        index_name: &str,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        source: &mut dyn VectorDocSource,
        options: &VectorRebuildOptions,
    ) -> Result<u64, SyncError> {
        let result = self
            .run_vector_rebuild_phases_inner(
                coordinator,
                outbox,
                target,
                tag_index_id,
                generation,
                index_name,
                space_id,
                tag_name,
                field_name,
                source,
                options,
            )
            .await;
        if result.is_err() {
            // Publish never ran (or failed before swapping): discarding the
            // temp is the whole rollback, live data was never touched.
            // After a successful publish the temp is unregistered, so this
            // is a no-op and can never delete serving data.
            coordinator
                .index_manager()
                .drop_temp_collection(space_id, tag_name, field_name, generation)
                .await
                .ok();
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_vector_rebuild_phases_inner(
        &self,
        coordinator: &Arc<VectorSyncCoordinator>,
        outbox: &crate::sqlite_outbox::SqliteOutbox,
        target: &TargetId,
        tag_index_id: u64,
        generation: u64,
        index_name: &str,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        source: &mut dyn VectorDocSource,
        options: &VectorRebuildOptions,
    ) -> Result<u64, SyncError> {
        // Pre-flight peek before building any temp state: a broken source
        // must fail the rebuild while live data is still servable. The
        // peeked batch is buffered and backfilled first so the source
        // contract (streaming `next_batch` until `None`) is preserved.
        let peeked = source.next_batch().await.map_err(SyncError::Internal)?;
        let peeked_empty = match &peeked {
            None => true,
            Some(docs) => docs.is_empty(),
        };
        if peeked_empty && !options.allow_empty_source {
            return Err(SyncError::PersistenceError(format!(
                "Vector rebuild aborted before temp creation: primary-storage source for {space_id}.{tag_name}.{field_name} yielded no documents (retry with allow_empty_source = true for intentional truncation)"
            )));
        }
        // Snapshot first, unfenced: without an in-place purge there is no
        // purge+snapshot window to fence. Live delivery keeps writing the
        // live slice throughout; backfill plus strictly-newer replay
        // converges the temp by point-ID idempotence.
        let snapshot_lsn = outbox
            .materialized_lsn()
            .await
            .map_err(SyncError::PersistenceError)?;
        let temp_name = coordinator
            .index_manager()
            .create_temp_collection(space_id, tag_name, field_name, generation)
            .await
            .map_err(|error| SyncError::PersistenceError(error.to_string()))?;
        tracing::info!(
            "Vector rebuild temp created (live untouched): space {} tag {} field {} generation {} temp {}",
            space_id,
            tag_name,
            field_name,
            generation,
            temp_name
        );
        outbox
            .transition_generation_to_backfilling(target, tag_index_id, generation)
            .await
            .map_err(SyncError::PersistenceError)?;
        coordinator.set_rebuild_phase(space_id, tag_name, field_name, RebuildPhase::Backfilling);
        let mut progress: RebuildProgress = coordinator
            .rebuild_progress(space_id, tag_name, field_name)
            .ok_or_else(|| {
                SyncError::Internal(
                    "rebuild progress missing after generation creation".to_string(),
                )
            })?;

        // ── Backfill (peeked batch first, then the rest of the stream) ──
        let backfill_started = Instant::now();
        let mut buffered: Option<Option<Vec<VectorRebuildDoc>>> = Some(peeked);
        loop {
            if !coordinator.index_exists(space_id, tag_name, field_name) {
                return Err(SyncError::PersistenceError(format!(
                    "Vector index {space_id}.{tag_name}.{field_name} was dropped during rebuild"
                )));
            }
            let batch = match buffered.take() {
                Some(first) => first,
                None => source.next_batch().await.map_err(SyncError::Internal)?,
            };
            let Some(docs) = batch else {
                break;
            };
            if docs.is_empty() {
                continue;
            }
            progress.docs_scanned += docs.len() as u64;
            let mut contexts = Vec::with_capacity(docs.len());
            for doc in &docs {
                // Only the rebuilt field is backfilled; anything else is
                // skipped exactly as the live path skips non-vector values
                // (the temp starts empty, so there is nothing to delete).
                let Some(vector) = doc
                    .properties
                    .iter()
                    .find(|(name, _)| name == field_name)
                    .and_then(|(_, value)| value.as_vector())
                else {
                    progress.docs_skipped += 1;
                    continue;
                };
                let payload = doc
                    .properties
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect::<HashMap<_, _>>();
                contexts.push(VectorChangeContext::new(
                    space_id,
                    tag_name,
                    field_name,
                    VectorChangeType::Insert,
                    VectorPointData {
                        id: format_vector_point_id(
                            &Value::from(doc.vertex_id),
                            tag_name,
                            field_name,
                        ),
                        vector: vector.to_vec(),
                        payload,
                    },
                ));
            }
            for chunk in contexts.chunks(BACKFILL_BATCH_CHUNK) {
                if chunk.is_empty() {
                    continue;
                }
                coordinator
                    .on_vector_change_batch_to(&temp_name, chunk.to_vec())
                    .await
                    .map_err(|error| SyncError::Internal(error.to_string()))?;
                progress.docs_applied += chunk.len() as u64;
            }
            coordinator.update_rebuild_progress(progress.clone());
        }
        if let Some(stats) = self.stats_manager_opt() {
            stats.record_rebuild_phase_latency(
                "vector",
                "backfilling",
                backfill_started
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            );
        }

        // ── Catch-up rounds (unfenced; final drain runs under the fence) ──
        coordinator.set_rebuild_phase(space_id, tag_name, field_name, RebuildPhase::CatchingUp);
        let catchup_started = Instant::now();
        coordinator
            .index_manager()
            .emit_rebuild_event(IndexEvent::VectorRebuildProgress {
                index_name: index_name.to_string(),
                generation,
                phase: RebuildPhase::CatchingUp.as_str().to_string(),
                vectors_applied: progress.docs_applied,
            });
        let mut last_replayed = snapshot_lsn;
        for _ in 0..options.max_catchup_rounds {
            if !coordinator.index_exists(space_id, tag_name, field_name) {
                return Err(SyncError::PersistenceError(format!(
                    "Vector index {space_id}.{tag_name}.{field_name} was dropped during rebuild"
                )));
            }
            let frontier = outbox
                .materialized_lsn()
                .await
                .map_err(SyncError::PersistenceError)?;
            if frontier <= last_replayed {
                break;
            }
            let replayed = self
                .replay_vector_range(
                    coordinator,
                    outbox,
                    target,
                    tag_index_id,
                    space_id,
                    tag_name,
                    field_name,
                    &temp_name,
                    &mut last_replayed,
                    frontier,
                    options.catchup_fetch_limit,
                )
                .await?;
            progress.docs_applied += replayed;
            coordinator.update_rebuild_progress(progress.clone());
            if replayed == 0 {
                break;
            }
        }

        // ── Publish (fenced final drain, atomic swap, generation activation) ──
        // The fence write guard covers the final drain plus the swap, so no
        // live delivery lands between the converged temp and its publication.
        // Field granularity switches collection pointers; Space granularity
        // copies the temp slice back into the shared live collection. Either
        // way the swap is the only irreversible step.
        if let Some(stats) = self.stats_manager_opt() {
            stats.record_rebuild_phase_latency(
                "vector",
                "catching_up",
                catchup_started
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            );
        }
        coordinator.set_rebuild_phase(space_id, tag_name, field_name, RebuildPhase::Publishing);
        let publish_started = Instant::now();
        let fence = coordinator.publish_fence_for(space_id, tag_name, field_name);
        let _publish_guard = fence.write().await;
        let publish_frontier = outbox
            .materialized_lsn()
            .await
            .map_err(SyncError::PersistenceError)?;
        while last_replayed < publish_frontier {
            let replayed = self
                .replay_vector_range(
                    coordinator,
                    outbox,
                    target,
                    tag_index_id,
                    space_id,
                    tag_name,
                    field_name,
                    &temp_name,
                    &mut last_replayed,
                    publish_frontier,
                    options.catchup_fetch_limit,
                )
                .await?;
            progress.docs_applied += replayed;
            if replayed == 0 {
                break;
            }
        }
        coordinator.update_rebuild_progress(progress.clone());
        coordinator
            .index_manager()
            .emit_rebuild_event(IndexEvent::VectorRebuildProgress {
                index_name: index_name.to_string(),
                generation,
                phase: RebuildPhase::Publishing.as_str().to_string(),
                vectors_applied: progress.docs_applied,
            });

        if progress.docs_scanned > 0 && progress.docs_applied == 0 {
            return Err(SyncError::PersistenceError(format!(
                "Rebuild of {space_id}.{tag_name}.{field_name} scanned {} docs but applied nothing",
                progress.docs_scanned
            )));
        }

        outbox
            .transition_generation_to_catching_up(
                target,
                tag_index_id,
                generation,
                publish_frontier,
            )
            .await
            .map_err(SyncError::PersistenceError)?;
        outbox
            .transition_generation_to_publishing(target, tag_index_id, generation, publish_frontier)
            .await
            .map_err(SyncError::PersistenceError)?;
        // Atomic publish while the fence is held: the only irreversible
        // step. Field switches pointers (local rename / remote remap);
        // Space copies the converged slice back into live.
        let outcome = coordinator
            .index_manager()
            .publish_temp_collection(
                space_id,
                tag_name,
                field_name,
                generation,
                options.catchup_fetch_limit.max(1) as usize,
            )
            .await
            .map_err(|error| SyncError::PersistenceError(error.to_string()))?;
        if outcome.copied_points > 0 {
            tracing::info!(
                "Vector rebuild slice copy-on-publish: space {} tag {} field {} generation {} copied {} points",
                space_id,
                tag_name,
                field_name,
                generation,
                outcome.copied_points
            );
        }
        if let Some(backup) = &outcome.backup {
            tracing::info!(
                "Vector rebuild publish retained backup: space {} tag {} field {} generation {} backup {}",
                space_id,
                tag_name,
                field_name,
                generation,
                backup
            );
        }
        drop(_publish_guard);
        outbox
            .activate_generation(target, tag_index_id, generation, publish_frontier)
            .await
            .map_err(SyncError::PersistenceError)?;
        if let Some(stats) = self.stats_manager_opt() {
            stats.record_rebuild_phase_latency(
                "vector",
                "publishing",
                publish_started
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            );
        }
        Ok(progress.docs_applied)
    }

    /// Replay one LSN range into the rebuild temp, advancing `cursor`.
    /// Only vertex payloads for the rebuilt `(space, tag)` are applied (and
    /// only contexts addressing the rebuilt field); DDL is skipped except a
    /// drop of the rebuilt index itself, which aborts the rebuild.
    #[allow(clippy::too_many_arguments)]
    async fn replay_vector_range(
        &self,
        coordinator: &Arc<VectorSyncCoordinator>,
        outbox: &crate::sqlite_outbox::SqliteOutbox,
        target: &TargetId,
        tag_index_id: u64,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        temp_name: &str,
        cursor: &mut CommitLsn,
        upto: CommitLsn,
        limit: u64,
    ) -> Result<u64, SyncError> {
        let events = outbox
            .fetch_rebuild_events(target, tag_index_id, *cursor, upto, limit)
            .await
            .map_err(SyncError::PersistenceError)?;
        if events.is_empty() {
            return Ok(0);
        }
        let mut replayed = 0u64;
        let mut pending: Vec<VectorChangeContext> = Vec::new();
        for event in &events {
            let payload: OutboxPayload =
                postcard::from_bytes(&event.mutation.document_or_vector)
                    .map_err(|error| SyncError::PersistenceError(error.to_string()))?;
            match &payload {
                OutboxPayload::DropIndex {
                    space_id: drop_space,
                    schema_name,
                    fields,
                    ..
                } if *drop_space == space_id
                    && schema_name == tag_name
                    && fields.iter().any(|field| field == field_name) =>
                {
                    return Err(SyncError::PersistenceError(format!(
                        "Vector index {space_id}.{tag_name}.{field_name} was dropped during rebuild"
                    )));
                }
                OutboxPayload::DropSpace {
                    space_id: drop_space,
                } if *drop_space == space_id => {
                    return Err(SyncError::PersistenceError(format!(
                        "Vector index {space_id}.{tag_name}.{field_name} was dropped during rebuild (space dropped)"
                    )));
                }
                OutboxPayload::DropTag {
                    space_id: drop_space,
                    tag_name: drop_tag,
                } if *drop_space == space_id && drop_tag == tag_name => {
                    return Err(SyncError::PersistenceError(format!(
                        "Vector index {space_id}.{tag_name}.{field_name} was dropped during rebuild (tag dropped)"
                    )));
                }
                OutboxPayload::CreateIndex { .. }
                | OutboxPayload::DropIndex { .. }
                | OutboxPayload::DropSpace { .. }
                | OutboxPayload::DropTag { .. } => {
                    continue;
                }
                OutboxPayload::Vertex {
                    space_id: event_space,
                    tag_name: event_tag,
                    ..
                } => {
                    if *event_space != space_id || event_tag != tag_name {
                        continue;
                    }
                }
                OutboxPayload::EdgeInsert { .. } | OutboxPayload::EdgeDelete { .. } => {
                    continue;
                }
            }
            // Receiver-bypassing replay: replayed LSNs sit below the live
            // water-level by construction, so gating would reject them; the
            // batch itself is idempotent by point ID.
            let contexts = SyncManager::vector_contexts_for_payload(coordinator, &payload)
                .map_err(SyncError::Internal)?;
            for context in contexts {
                if context.location.space_id == space_id
                    && context.location.tag_name == tag_name
                    && context.location.field_name == field_name
                {
                    pending.push(context);
                }
            }
            if pending.len() >= BACKFILL_BATCH_CHUNK {
                let batch = std::mem::take(&mut pending);
                coordinator
                    .on_vector_change_batch_to(temp_name, batch)
                    .await
                    .map_err(|error| SyncError::Internal(error.to_string()))?;
            }
            replayed += 1;
        }
        if !pending.is_empty() {
            coordinator
                .on_vector_change_batch_to(temp_name, pending)
                .await
                .map_err(|error| SyncError::Internal(error.to_string()))?;
        }
        if let Some(last) = events.last() {
            *cursor = last.commit_lsn;
        }
        Ok(replayed)
    }

    /// Fail non-terminal vector generations stranded by crashed rebuild
    /// attempts for every known vector index, then reconcile physical
    /// scratch state. Idempotent; safe to run at startup before serving
    /// traffic.
    ///
    /// Scratch reconciliation, in order:
    /// 1. Heal interrupted Field publishes (local backend): a crash between
    ///    the rename steps can leave the live name missing with the
    ///    converged temp and the `.old` backup beside it. The temp wins
    ///    (it holds the publish frontier); otherwise the newest backup is
    ///    restored. Either way the failed generation stays failed and the
    ///    operator retries to converge bookkeeping.
    /// 2. Re-adopt published remote temps: Qdrant publishes remap the
    ///    in-memory logical pointer, so a restart loses the mapping. A temp
    ///    matching the active generation is re-adopted as the serving
    ///    collection instead of being dropped.
    /// 3. Drop every remaining `*.rebuild-*` temp: no active rebuild runs
    ///    at recovery time, so all of them are orphans.
    /// 4. Prune `.old-*` backups, keeping the newest generation per live
    ///    collection for reverse recovery.
    pub async fn recover_stale_vector_rebuilds(&self) -> Result<usize, SyncError> {
        let coordinator = self.vector_coordinator().cloned().ok_or_else(|| {
            SyncError::PersistenceError("vector target is not configured".to_string())
        })?;
        let outbox = self.sqlite_outbox_opt().ok_or_else(|| {
            SyncError::PersistenceError(
                "vector rebuild recovery requires a configured durable outbox".to_string(),
            )
        })?;
        let target = TargetId::new(VECTOR_TARGET).map_err(SyncError::PersistenceError)?;
        let mut seen: Vec<(u64, String)> = Vec::new();
        for metadata in coordinator.list_indexes() {
            let key = (metadata.space_id, metadata.tag_name.clone());
            if !seen.contains(&key) {
                seen.push(key);
            }
        }
        let mut failed = 0usize;
        for (space_id, tag_name) in seen {
            let tag_index_id = vector_tag_index_id(space_id, &tag_name);
            let known = outbox
                .list_index_generations(&target, tag_index_id)
                .await
                .map_err(SyncError::PersistenceError)?;
            for (generation, state) in known {
                if matches!(
                    state.as_str(),
                    "creating" | "backfilling" | "catching_up" | "publishing"
                ) {
                    outbox
                        .fail_generation(&target, tag_index_id, generation)
                        .await
                        .map_err(SyncError::PersistenceError)?;
                    failed += 1;
                }
            }
        }
        self.heal_interrupted_field_publishes(&coordinator).await;
        self.readopt_published_remote_temps(&coordinator, outbox, &target)
            .await;
        self.drop_orphan_temp_collections(&coordinator).await;
        self.prune_stale_promote_backups(&coordinator).await;
        Ok(failed)
    }

    /// Heal Field-granularity publishes interrupted mid-rename (local
    /// backend only): when the live name is missing but a temp for a
    /// registered index survives, promote the newest temp to live,
    /// restoring from the newest `.old` backup when no temp survives.
    async fn heal_interrupted_field_publishes(&self, coordinator: &Arc<VectorSyncCoordinator>) {
        use crate::vector_sync::CollectionGranularity;
        if !coordinator.backend().is_local() {
            return;
        }
        let manager = coordinator.index_manager();
        if manager.granularity() != CollectionGranularity::Field {
            return;
        }
        let Ok(physical) = manager.backend().list_collections().await else {
            return;
        };
        for index in manager.list_indexes() {
            let loc = VectorIndexLocation::new(index.space_id, &index.tag_name, &index.field_name);
            let live = manager.live_collection_name(&loc);
            if manager.backend().index_exists(&live) {
                continue;
            }
            let mut temps: Vec<(u64, String)> = physical
                .iter()
                .filter_map(|name| {
                    let (temp_live, generation) = VectorBackend::parse_temp_collection(name)?;
                    (temp_live == live).then(|| (generation, name.clone()))
                })
                .collect();
            temps.sort_by_key(|(generation, _)| std::cmp::Reverse(*generation));
            if let Some((generation, temp)) = temps.into_iter().next() {
                if temp.is_empty() {
                    continue;
                }
                let suffix = VectorBackend::promote_backup_suffix(generation);
                match manager
                    .backend()
                    .promote_temp_collection(&live, &temp, &suffix)
                    .await
                {
                    Ok(()) => tracing::warn!(
                        "Healed interrupted vector publish: promoted temp {} to live {}",
                        temp,
                        live
                    ),
                    Err(error) => {
                        tracing::warn!(
                            "Failed to heal interrupted vector publish for {}: {}; restoring newest backup",
                            live,
                            error
                        );
                        if let Ok(restored) = manager.backend().restore_promote_backup(&live).await
                        {
                            tracing::warn!(
                                "Restored vector live {} from backup {}",
                                live,
                                restored
                            );
                        }
                    }
                }
            } else if let Ok(restored) = manager.backend().restore_promote_backup(&live).await {
                tracing::warn!("Restored vector live {} from backup {}", live, restored);
            }
        }
    }

    /// Re-adopt remote temps that already serve live traffic. Qdrant Field
    /// publishes remap the in-memory logical pointer, which a restart
    /// forgets; the temp matching the outbox active generation is the
    /// published data and must be re-adopted, not dropped.
    async fn readopt_published_remote_temps(
        &self,
        coordinator: &Arc<VectorSyncCoordinator>,
        outbox: &crate::sqlite_outbox::SqliteOutbox,
        target: &TargetId,
    ) {
        use crate::vector_sync::CollectionGranularity;
        if coordinator.backend().is_local() {
            return;
        }
        let manager = coordinator.index_manager();
        if manager.granularity() != CollectionGranularity::Field {
            return;
        }
        for index in manager.list_indexes() {
            let tag_index_id = vector_tag_index_id(index.space_id, &index.tag_name);
            let Ok(active) = outbox.get_active_generation(target, tag_index_id).await else {
                continue;
            };
            let Some(active) = active else { continue };
            let temp =
                manager.temp_name_for(index.space_id, &index.tag_name, &index.field_name, active);
            // Membership in the physical listing, not the loaded-handle set:
            // temps are never loaded at open, so `index_exists` misses them.
            let Ok(physical) = manager.backend().list_collections().await else {
                continue;
            };
            if !physical.contains(&temp) {
                continue;
            }
            let loc = VectorIndexLocation::new(index.space_id, &index.tag_name, &index.field_name);
            manager.set_logical_collection_name(&loc, temp.clone());
            manager.set_collection_override(&loc, temp.clone());
            tracing::warn!(
                "Re-adopted published vector temp {} for {}.{}.{}",
                temp,
                index.space_id,
                index.tag_name,
                index.field_name
            );
        }
    }

    /// Drop every `*.rebuild-*` temp that is not currently serving traffic.
    /// At recovery time no rebuild runs, so anything not re-adopted above
    /// is an orphan from a failed or crashed attempt.
    async fn drop_orphan_temp_collections(&self, coordinator: &Arc<VectorSyncCoordinator>) {
        let manager = coordinator.index_manager();
        let Ok(physical) = manager.backend().list_collections().await else {
            return;
        };
        let serving: std::collections::HashSet<String> = manager
            .list_indexes()
            .into_iter()
            .map(|index| {
                let loc =
                    VectorIndexLocation::new(index.space_id, &index.tag_name, &index.field_name);
                manager.collection_name_for(&loc)
            })
            .collect();
        for name in physical {
            if !VectorBackend::is_temp_collection(&name) || serving.contains(&name) {
                continue;
            }
            for ((space_id, tag_name, field_name, generation), temp) in manager.list_temps() {
                if temp.temp_name == name {
                    manager
                        .drop_temp_collection(space_id, &tag_name, &field_name, generation)
                        .await
                        .ok();
                }
            }
            if let Err(error) = manager.drop_temp_by_name(&name).await {
                tracing::warn!("Failed to drop orphan vector temp {}: {}", name, error);
            } else {
                tracing::warn!("Dropped orphan vector temp {}", name);
            }
        }
    }

    /// Prune `.old-*` publish backups, keeping the newest generation per
    /// live collection for reverse recovery.
    async fn prune_stale_promote_backups(&self, coordinator: &Arc<VectorSyncCoordinator>) {
        let manager = coordinator.index_manager();
        let Ok(physical) = manager.backend().list_collections().await else {
            return;
        };
        let mut by_live: HashMap<String, Vec<(u64, String)>> = HashMap::new();
        for name in physical {
            if let Some((live, generation)) = VectorBackend::parse_backup_collection(&name) {
                by_live.entry(live).or_default().push((generation, name));
            }
        }
        for (live, mut backups) in by_live {
            backups.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
            for (_, name) in backups.into_iter().skip(1) {
                if let Err(error) = manager.backend().delete_collection(&name).await {
                    tracing::warn!("Failed to prune vector promote backup {}: {}", name, error);
                } else {
                    tracing::warn!(
                        "Pruned superseded vector promote backup {} of {}",
                        name,
                        live
                    );
                }
            }
        }
    }
}
