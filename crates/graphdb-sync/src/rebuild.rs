#![cfg(feature = "fulltext")]

//! True online rebuild for fulltext indexes.
//!
//! The previous `rebuild_index` only cleared the engine. This driver rebuilds
//! from primary storage instead: snapshot scan, backfill into a scratch
//! engine, catch-up replay of outbox events newer than the snapshot, then an
//! atomic publish under the per-index publish fence.
//!
//! Correctness argument: delivery applies hold the fence read guard, so every
//! commit is either fully before or fully after the swap. Catch-up replay
//! reads the events table by commit-LSN range regardless of delivery status,
//! so degraded or dead-lettered events still converge. Any event materialized
//! after the final replay drains through normal delivery to the swapped
//! engine (its generation stays active). Upserts are idempotent, so overlap
//! between backfill, replay rounds, and live delivery only rewrites the same
//! document content.
//!
//! Anything after `publish_rebuilt_engine` cannot fail the rebuild: publish
//! is the single irreversible point, and rollback is "publish nothing".

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use graphdb_core::types::{
    CommitLsn, IdempotencyKey, IndexGeneration, OrderingKey, TargetId, VertexId,
};
use graphdb_core::wal::{EntityRef, IndexMutation, IndexOperation, WAL_SYNC_WIRE_VERSION};
use graphdb_core::Value;
use graphdb_fulltext::engine::FulltextSearchEngine;
use graphdb_fulltext::{RebuildPhase, RebuildProgress};

use crate::manager::{stable_hash, SyncError, SyncManager};
use crate::outbox::OutboxPayload;
use crate::receiver::FulltextReceiver;

/// Outbox target name for fulltext intents. Must match the writer path.
const FULLTEXT_TARGET: &str = "fulltext";
/// Commit-LSN stamp for backfill batches: catch-up replays strictly newer events.
const BACKFILL_RECEIVER_CHUNK: usize = 500;

/// One text document enumerated from primary storage.
#[derive(Debug, Clone)]
pub struct RebuildDoc {
    pub entity: RebuildEntity,
    pub text: String,
}

/// Entity identity for a rebuild document. Carries storage-native IDs so no
/// value roundtrip can diverge doc IDs from the live path.
#[derive(Debug, Clone)]
pub enum RebuildEntity {
    Vertex {
        id: VertexId,
    },
    Edge {
        src: VertexId,
        dst: VertexId,
        edge_type: String,
        ranking: i64,
    },
}

/// Primary-storage scan feeding backfill. Batches stream; `None` ends the scan.
#[async_trait]
pub trait RebuildDocSource: Send + Sync {
    async fn next_batch(&mut self) -> Result<Option<Vec<RebuildDoc>>, String>;
}

/// Tunables for [`SyncManager::rebuild_fulltext_index`].
#[derive(Debug, Clone)]
pub struct FulltextRebuildOptions {
    /// Outbox events fetched per catch-up round.
    pub catchup_fetch_limit: u64,
    /// Non-final catch-up rounds before publish; the final drain always runs
    /// to the publish frontier.
    pub max_catchup_rounds: usize,
    /// Refuse to publish when the primary-storage source yields no documents.
    /// Publishing an empty rebuild over a live index with an empty or
    /// unreadable source would delete the live data with nothing to backfill;
    /// aborting before scratch creation keeps the live index servable. Set to
    /// true only for intentional truncation (prefer explicit clear for that).
    pub allow_empty_source: bool,
}

impl Default for FulltextRebuildOptions {
    fn default() -> Self {
        Self {
            catchup_fetch_limit: 1000,
            max_catchup_rounds: 10,
            allow_empty_source: false,
        }
    }
}

/// Tantivy doc ID for a vertex text value. Shared with the live delivery path
/// so backfill and live writes address the same document.
pub fn fulltext_vertex_doc_id(vertex_id: &Value) -> String {
    format!("{vertex_id}")
}

/// Tantivy doc ID basis for edge text (idempotency keys). Live upserts land
/// under the receiver-formatted `{src}->{dst}` ID; the ranking suffix is kept
/// in keys only to distinguish parallel edge versions of one pair.
///
/// Note: live edge deletes use the unsuffixed ID while upserts carry ranking
/// in the operation entity, so stale edge versions can orphan exactly as they
/// do on the live path. Rebuild stays faithful to live behavior here.
pub fn fulltext_edge_doc_basis(src: &Value, dst: &Value, ranking: i64) -> String {
    format!("{src}->{dst}#{ranking}")
}

/// Tag-level outbox index ID for fulltext intents of one space/tag.
/// Must match `payload_to_intent`: `stable_hash("fulltext:{space}:{tag}")`.
pub fn fulltext_tag_index_id(space_id: u64, tag_name: &str) -> u64 {
    stable_hash(format!("{FULLTEXT_TARGET}:{space_id}:{tag_name}").as_bytes())
}

/// Field-level index ID used for receiver mutations.
/// Must match `apply_fulltext_fields`: `stable_hash("{space}:{tag}:{field}")`.
pub fn fulltext_field_index_id(space_id: u64, tag_name: &str, field_name: &str) -> u64 {
    stable_hash(format!("{space_id}:{tag_name}:{field_name}").as_bytes())
}

impl SyncManager {
    /// Rebuild one fulltext index from primary storage without dropping it.
    ///
    /// Reads keep serving from the live engine; writes keep delivering to it
    /// until publish swaps in the rebuilt engine. Returns the live doc count
    /// after publish. Failures leave the previous engine serving and mark
    /// metadata `Error` for operator attention.
    pub async fn rebuild_fulltext_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        source: &mut dyn RebuildDocSource,
        options: FulltextRebuildOptions,
    ) -> Result<u64, SyncError> {
        let coordinator = self.sync_coordinator_opt().ok_or_else(|| {
            SyncError::PersistenceError("fulltext target is not configured".to_string())
        })?;
        let manager = coordinator.fulltext_manager().clone();
        let outbox = self.sqlite_outbox_opt().ok_or_else(|| {
            SyncError::PersistenceError(
                "fulltext rebuild requires a configured durable outbox".to_string(),
            )
        })?;
        if manager
            .get_metadata(space_id, tag_name, field_name)
            .is_none()
        {
            return Err(SyncError::PersistenceError(format!(
                "Fulltext index not found: {space_id}.{tag_name}.{field_name}"
            )));
        }
        // Admission: at most one rebuild per index. The guard is held until
        // this function returns, so a concurrent rebuild of the same index
        // fails fast instead of racing `active_generation + 1` and
        // overwriting shared `rebuild_progress`.
        let rebuild_lock = self.rebuild_lock_for("fulltext", space_id, tag_name, field_name);
        let _rebuild_guard = rebuild_lock.try_lock_owned().map_err(|_| {
            SyncError::RebuildBusy(format!(
                "Fulltext rebuild already running for {space_id}.{tag_name}.{field_name}"
            ))
        })?;

        // Pre-flight peek before building any scratch state: a broken source
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
                "Fulltext rebuild aborted before scratch creation: primary-storage source for {space_id}.{tag_name}.{field_name} yielded no documents (retry with allow_empty_source = true for intentional truncation)"
            )));
        }

        let target = TargetId::new(FULLTEXT_TARGET).map_err(SyncError::PersistenceError)?;
        let tag_index_id = fulltext_tag_index_id(space_id, tag_name);
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

        let engine = manager
            .create_rebuild_engine(space_id, tag_name, field_name, generation)
            .await
            .map_err(|error| SyncError::PersistenceError(error.to_string()))?;

        let result = self
            .run_rebuild_phases(
                &manager,
                outbox,
                &target,
                tag_index_id,
                generation,
                &engine,
                space_id,
                tag_name,
                field_name,
                source,
                peeked,
                &options,
            )
            .await;
        if let Err(error) = &result {
            let reason = error.to_string();
            if let Some(stats) = self.stats_manager_opt() {
                stats.record_generation_rebuild_failure();
            }
            outbox
                .fail_generation(&target, tag_index_id, generation)
                .await
                .ok();
            manager
                .discard_rebuild_engine(space_id, tag_name, field_name, generation, &reason)
                .await;
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_rebuild_phases(
        &self,
        manager: &Arc<graphdb_fulltext::manager::FulltextIndexManager>,
        outbox: &crate::sqlite_outbox::SqliteOutbox,
        target: &TargetId,
        tag_index_id: u64,
        generation: u64,
        engine: &Arc<dyn FulltextSearchEngine>,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        source: &mut dyn RebuildDocSource,
        peeked: Option<Vec<RebuildDoc>>,
        options: &FulltextRebuildOptions,
    ) -> Result<u64, SyncError> {
        // Snapshot boundary read before the scan: every event at or below it
        // is materialized, so backfill plus strictly-newer replay converges.
        let snapshot_lsn = outbox
            .materialized_lsn()
            .await
            .map_err(SyncError::PersistenceError)?;
        outbox
            .transition_generation_to_backfilling(target, tag_index_id, generation)
            .await
            .map_err(SyncError::PersistenceError)?;
        manager.set_rebuild_phase(space_id, tag_name, field_name, RebuildPhase::Backfilling);

        let field_index_id = fulltext_field_index_id(space_id, tag_name, field_name);
        let receiver = FulltextReceiver::new(Arc::clone(engine));
        let mut progress: RebuildProgress = manager
            .rebuild_progress(space_id, tag_name, field_name)
            .ok_or_else(|| {
                SyncError::Internal("rebuild progress missing after engine creation".to_string())
            })?;

        // ── Backfill (peeked batch first, then the rest of the stream) ──
        let backfill_started = Instant::now();
        let mut buffered: Option<Option<Vec<RebuildDoc>>> = Some(peeked);
        loop {
            if !manager.has_index(space_id, tag_name, field_name) {
                return Err(SyncError::PersistenceError(format!(
                    "Fulltext index {space_id}.{tag_name}.{field_name} was dropped during rebuild"
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
            let mut owned: Vec<IndexMutation> = Vec::with_capacity(docs.len());
            for doc in &docs {
                // Entity carries storage-native IDs, so construction cannot
                // fail on representable data; anything else aborts loudly.
                owned.push(
                    backfill_mutation(&doc.entity, &doc.text, field_index_id, generation)
                        .map_err(SyncError::Internal)?,
                );
            }
            progress.docs_scanned += docs.len() as u64;
            // `FulltextReceiver` takes references; chunk to bound memory.
            for chunk in owned.chunks(BACKFILL_RECEIVER_CHUNK) {
                let refs: Vec<(&IndexMutation, CommitLsn)> =
                    chunk.iter().map(|m| (m, snapshot_lsn)).collect();
                receiver
                    .apply_index_batch(&refs)
                    .await
                    .map_err(SyncError::Internal)?;
                progress.docs_applied += chunk.len() as u64;
            }
            manager.update_rebuild_progress(progress.clone());
        }
        if let Some(stats) = self.stats_manager_opt() {
            stats.record_rebuild_phase_latency(
                "fulltext",
                "backfilling",
                backfill_started
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            );
        }

        // ── Catch-up rounds (unfenced; final drain runs under the fence) ──
        manager.set_rebuild_phase(space_id, tag_name, field_name, RebuildPhase::CatchingUp);
        let catchup_started = Instant::now();
        let mut last_replayed = snapshot_lsn;
        for _ in 0..options.max_catchup_rounds {
            if !manager.has_index(space_id, tag_name, field_name) {
                return Err(SyncError::PersistenceError(format!(
                    "Fulltext index {space_id}.{tag_name}.{field_name} was dropped during rebuild"
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
                .replay_rebuild_range(
                    manager,
                    outbox,
                    engine,
                    target,
                    tag_index_id,
                    space_id,
                    tag_name,
                    &mut last_replayed,
                    frontier,
                    options.catchup_fetch_limit,
                )
                .await?;
            progress.docs_applied += replayed;
            manager.update_rebuild_progress(progress.clone());
            if replayed == 0 {
                break;
            }
        }

        // ── Publish (fenced: no delivery apply straddles swap) ──
        if let Some(stats) = self.stats_manager_opt() {
            stats.record_rebuild_phase_latency(
                "fulltext",
                "catching_up",
                catchup_started
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            );
        }
        manager.set_rebuild_phase(space_id, tag_name, field_name, RebuildPhase::Publishing);
        let publish_started = Instant::now();
        let fence = manager.publish_fence_for(space_id, tag_name, field_name);
        let _publish_guard = fence.write().await;
        let publish_frontier = outbox
            .materialized_lsn()
            .await
            .map_err(SyncError::PersistenceError)?;
        while last_replayed < publish_frontier {
            let replayed = self
                .replay_rebuild_range(
                    manager,
                    outbox,
                    engine,
                    target,
                    tag_index_id,
                    space_id,
                    tag_name,
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
        manager.update_rebuild_progress(progress.clone());
        engine
            .commit()
            .await
            .map_err(|error| SyncError::PersistenceError(error.to_string()))?;

        let stats = engine
            .stats()
            .await
            .map_err(|error| SyncError::PersistenceError(error.to_string()))?;
        if progress.docs_scanned > 0 && stats.doc_count == 0 {
            return Err(SyncError::PersistenceError(format!(
                "Rebuild of {space_id}.{tag_name}.{field_name} scanned {} docs but produced an empty index",
                progress.docs_scanned
            )));
        }
        if progress.docs_skipped > 0 {
            log::warn!(
                "Rebuild of {space_id}.{tag_name}.{field_name} skipped {} unrepresentable docs",
                progress.docs_skipped
            );
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

        let docs = manager
            .publish_rebuilt_engine(
                space_id,
                tag_name,
                field_name,
                generation,
                Arc::clone(engine),
            )
            .await
            .map_err(|error| SyncError::PersistenceError(error.to_string()))?;
        if let Some(coordinator) = self.sync_coordinator_opt() {
            coordinator
                .repoint_fulltext_processor(space_id, tag_name, field_name, Arc::clone(engine))
                .await;
        }
        drop(_publish_guard);
        outbox
            .activate_generation(target, tag_index_id, generation, publish_frontier)
            .await
            .map_err(SyncError::PersistenceError)?;
        if let Some(stats) = self.stats_manager_opt() {
            stats.record_rebuild_phase_latency(
                "fulltext",
                "publishing",
                publish_started
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            );
        }
        Ok(docs)
    }

    /// Replay one LSN range into the scratch engine, advancing `cursor`.
    /// DDL payloads are skipped except a drop of the rebuilt index itself,
    /// which aborts the rebuild.
    #[allow(clippy::too_many_arguments)]
    async fn replay_rebuild_range(
        &self,
        _manager: &Arc<graphdb_fulltext::manager::FulltextIndexManager>,
        outbox: &crate::sqlite_outbox::SqliteOutbox,
        engine: &Arc<dyn FulltextSearchEngine>,
        target: &TargetId,
        tag_index_id: u64,
        space_id: u64,
        tag_name: &str,
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
        for event in &events {
            let payload: OutboxPayload =
                postcard::from_bytes(&event.mutation.document_or_vector)
                    .map_err(|error| SyncError::PersistenceError(error.to_string()))?;
            match &payload {
                OutboxPayload::DropIndex { schema_name, .. } if schema_name == tag_name => {
                    return Err(SyncError::PersistenceError(format!(
                        "Fulltext index {space_id}.{tag_name} was dropped during rebuild"
                    )));
                }
                OutboxPayload::DropSpace {
                    space_id: drop_space,
                } if *drop_space == space_id => {
                    return Err(SyncError::PersistenceError(format!(
                        "Fulltext index {space_id}.{tag_name} was dropped during rebuild (space dropped)"
                    )));
                }
                OutboxPayload::DropTag {
                    space_id: drop_space,
                    tag_name: drop_tag,
                } if *drop_space == space_id && drop_tag == tag_name => {
                    return Err(SyncError::PersistenceError(format!(
                        "Fulltext index {space_id}.{tag_name} was dropped during rebuild (tag dropped)"
                    )));
                }
                OutboxPayload::CreateIndex { .. }
                | OutboxPayload::DropIndex { .. }
                | OutboxPayload::DropSpace { .. }
                | OutboxPayload::DropTag { .. } => {
                    continue;
                }
                _ => {}
            }
            self.apply_fulltext_mutation(
                &event.mutation,
                event.commit_lsn,
                &payload,
                Some(Arc::clone(engine)),
            )
            .await
            .map_err(SyncError::Internal)?;
            replayed += 1;
        }
        if let Some(last) = events.last() {
            *cursor = last.commit_lsn;
        }
        Ok(replayed)
    }

    /// Fail non-terminal generations stranded by crashed rebuild attempts for
    /// every known fulltext index, and collect scratch poll left by crashes.
    /// Idempotent; safe to run at startup before serving traffic.
    pub async fn recover_stale_fulltext_rebuilds(&self) -> Result<usize, SyncError> {
        let coordinator = self.sync_coordinator_opt().ok_or_else(|| {
            SyncError::PersistenceError("fulltext target is not configured".to_string())
        })?;
        let manager = coordinator.fulltext_manager();
        let outbox = self.sqlite_outbox_opt().ok_or_else(|| {
            SyncError::PersistenceError(
                "fulltext rebuild recovery requires a configured durable outbox".to_string(),
            )
        })?;
        let target = TargetId::new(FULLTEXT_TARGET).map_err(SyncError::PersistenceError)?;
        let mut failed = 0usize;
        for metadata in manager.list_indexes() {
            let tag_index_id = fulltext_tag_index_id(metadata.space_id, &metadata.tag_name);
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
        manager.cleanup_stale_rebuild_state();
        Ok(failed)
    }
}

/// Build a backfill mutation for one scanned document, stamped by the caller
/// with the snapshot LSN. Entity conversions mirror the live intent path, so
/// backfilled tantivy doc IDs match live ones.
fn backfill_mutation(
    entity: &RebuildEntity,
    text: &str,
    field_index_id: u64,
    generation: u64,
) -> Result<IndexMutation, String> {
    let (entity_ref, doc_basis) = match entity {
        // `Value::from(*id)` is exactly how the live write path formats the
        // vertex ID (`SyncWrapper` stages `Value::from(*vertex_id)`).
        RebuildEntity::Vertex { id } => (
            EntityRef::Vertex(*id),
            fulltext_vertex_doc_id(&Value::from(*id)),
        ),
        RebuildEntity::Edge {
            src,
            dst,
            edge_type,
            ranking,
        } => (
            EntityRef::Edge {
                src: *src,
                dst: *dst,
                ranking: *ranking,
                edge_type: stable_hash(edge_type.as_bytes()) as u32,
            },
            fulltext_edge_doc_basis(&Value::from(*src), &Value::from(*dst), *ranking),
        ),
    };
    Ok(IndexMutation {
        wire_version: WAL_SYNC_WIRE_VERSION,
        target: TargetId::new(FULLTEXT_TARGET)?,
        index_id: field_index_id,
        index_generation: IndexGeneration::new(generation),
        entity_ref,
        operation: IndexOperation::Upsert,
        document_or_vector: text.as_bytes().to_vec(),
        idempotency_key: IdempotencyKey::new(format!("rebuild:g{generation}:{doc_basis}"))?,
        ordering_key: OrderingKey::new(format!("rebuild:g{generation}:{doc_basis}"))?,
    })
}
